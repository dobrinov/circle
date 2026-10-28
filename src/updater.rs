//! Updates: finds newer releases on GitHub, downloads the one for this
//! platform in the background, verifies it against the release's
//! `SHA256SUMS.txt`, and installs it when Circle quits (or restarts).
//!
//! Installing swaps the app for the new one from a small script that waits
//! for Circle to exit: on macOS it copies the app out of the `.dmg`, on
//! Windows it runs the installer silently, and on Linux it unpacks the
//! `.tar.gz` over a writable install (a `.deb` install can only be told
//! that an update exists).
//!
//! `CIRCLE_UPDATE_FEED` points the check at another URL (a JSON document
//! shaped like GitHub's "latest release"), for testing.

use crate::controller::{self, Cmd};
use crate::remote::run_on_ui;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// Updates must be signed by this Apple Developer team (and notarized)
/// before they replace Circle on macOS, so a release published by anyone
/// else, or tampered with, never installs.
#[cfg(not(windows))]
const TEAM_ID: &str = "5ZX3P79WQZ";

/// One check or download at a time.
static BUSY: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Status {
    /// Nothing newer (or not checked yet).
    UpToDate {
        checked: u64,
    },
    /// A newer version, not downloaded: automatic updates are off, or this
    /// install can't update itself (`installable` false: download it).
    Available {
        version: String,
        url: String,
        installable: bool,
    },
    Downloading {
        version: String,
        percent: u8,
    },
    /// Downloaded and verified; installs on quit or restart.
    Ready {
        version: String,
    },
    Failed {
        error: String,
    },
}

/// What a check found and where it came from.
pub struct Update {
    pub version: String,
    pub page: String,
    pub file: PathBuf,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

fn feed_url() -> String {
    // Tests point development builds at a fake feed; release builds ignore
    // it, so nothing in the environment can redirect real updates.
    #[cfg(debug_assertions)]
    if let Ok(url) = std::env::var("CIRCLE_UPDATE_FEED") {
        return url;
    }
    let repo = env!("CARGO_PKG_REPOSITORY").trim_start_matches("https://github.com/");
    format!("https://api.github.com/repos/{repo}/releases/latest")
}

/// Checks for a newer release and, if this install can update itself and
/// `download` is set, fetches it. Reports to the controller as it goes.
pub fn check(dir: PathBuf, download: bool) {
    if BUSY.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        let result = check_and_fetch(&dir, download);
        BUSY.store(false, Ordering::SeqCst);
        let status = match result {
            Ok(status) => status,
            Err(error) => Status::Failed { error },
        };
        report(status);
    });
}

fn report(status: Status) {
    run_on_ui(move || controller::dispatch(Cmd::UpdateStatus(status)));
}

/// The system's `curl` (macOS, Linux and Windows 10+ ship it), which
/// follows redirects and honours proxy settings.
pub(crate) fn curl() -> Command {
    let mut cmd = Command::new(if cfg!(windows) { "curl.exe" } else { "curl" });
    cmd.args([
        "--silent",
        "--show-error",
        "--fail",
        "--location",
        // HTTPS only, redirects included.
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--tlsv1.2",
        "--connect-timeout",
        "15",
    ])
    .arg("--user-agent")
    .arg(format!("Circle/{CURRENT}"));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // no console window
    }
    cmd
}

fn get_text(url: &str) -> Result<String, String> {
    let output = curl()
        .args(["--max-time", "30"])
        .args(["--header", "Accept: application/vnd.github+json"])
        .arg(url)
        .output()
        .map_err(|e| format!("couldn't run curl: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    String::from_utf8(output.stdout).map_err(|e| e.to_string())
}

fn check_and_fetch(dir: &Path, download: bool) -> Result<Status, String> {
    let text = get_text(&feed_url()).map_err(|e| format!("couldn't check for updates: {e}"))?;
    let release: Release =
        serde_json::from_str(&text).map_err(|e| format!("unexpected update feed: {e}"))?;
    let version = release.tag_name.trim_start_matches('v').to_owned();
    if release.draft || release.prerelease || !is_newer(&version, CURRENT) {
        return Ok(Status::UpToDate {
            checked: crate::model::now(),
        });
    }
    let asset = release
        .assets
        .iter()
        .find(|a| a.name.ends_with(asset_suffix()) && is_plain_file_name(&a.name));
    let (Some(asset), true) = (asset, can_self_update()) else {
        return Ok(Status::Available {
            version,
            url: release.html_url,
            installable: false,
        });
    };
    if !download {
        return Ok(Status::Available {
            version,
            url: release.html_url,
            installable: true,
        });
    }

    let target = dir.join(&asset.name);
    let sha256 = match verify(&release, asset, &target) {
        Ok(sha256) if target.exists() => sha256,
        _ => {
            fetch(asset, &target, &version)?;
            verify(&release, asset, &target).inspect_err(|_| {
                let _ = fs::remove_file(&target);
            })?
        }
    };
    #[cfg(target_os = "macos")]
    verify_signature_in_dmg(&target).inspect_err(|_| {
        let _ = fs::remove_file(&target);
    })?;
    fs::write(
        dir.join("pending.json"),
        serde_json::json!({
            "version": version,
            "file": target,
            "sha256": sha256,
            "page": release.html_url,
        })
        .to_string(),
    )
    .map_err(|e| e.to_string())?;
    Ok(Status::Ready { version })
}

/// The update downloaded earlier and waiting to be installed, if any.
pub fn pending(dir: &Path) -> Option<Update> {
    let text = fs::read_to_string(dir.join("pending.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let version = value["version"].as_str()?.to_owned();
    let file = PathBuf::from(value["file"].as_str()?);
    // Already installed (or older), not where downloads go, or changed
    // since it was checked: forget it.
    let usable = is_newer(&version, CURRENT)
        && file.parent() == Some(dir)
        && value["sha256"]
            .as_str()
            .is_some_and(|sha| sha256_of(&file).is_ok_and(|actual| actual == sha));
    if !usable {
        let _ = fs::remove_file(dir.join("pending.json"));
        return None;
    }
    Some(Update {
        version,
        page: value["page"].as_str().unwrap_or_default().to_owned(),
        file,
    })
}

fn fetch(asset: &Asset, target: &Path, version: &str) -> Result<(), String> {
    let partial = target.with_extension("part");
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut child = curl()
        .arg("--output")
        .arg(&partial)
        .arg(&asset.browser_download_url)
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("couldn't run curl: {e}"))?;
    // Progress is how much of the file has arrived so far.
    let mut last_percent = 255u8;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        let done = fs::metadata(&partial).map_or(0, |m| m.len());
        if let Some(percent) = (done * 100)
            .checked_div(asset.size)
            .map(|p| p.min(100) as u8)
            && percent != last_percent
        {
            last_percent = percent;
            report(Status::Downloading {
                version: version.to_owned(),
                percent,
            });
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    if !status.success() {
        let mut err = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            let _ = stderr.read_to_string(&mut err);
        }
        let _ = fs::remove_file(&partial);
        return Err(format!("couldn't download the update: {}", err.trim()));
    }
    fs::rename(&partial, target).map_err(|e| e.to_string())
}

/// A release asset name that is just a file name (no folders), so it can't
/// write outside the updates folder.
fn is_plain_file_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['/', '\\'])
        && Path::new(name).file_name() == Some(std::ffi::OsStr::new(name))
}

fn sha256_of(file: &Path) -> Result<String, String> {
    let mut hasher = Sha256::new();
    let mut f = File::open(file).map_err(|e| e.to_string())?;
    std::io::copy(&mut f, &mut hasher).map_err(|e| e.to_string())?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// The code-signing check the installer repeats right before it swaps the
/// app in: valid, notarized, and from Circle's team.
#[cfg(not(windows))]
fn signature_check(app: &str) -> String {
    format!(
        "codesign --verify --deep --strict {app} && \\\n\
         codesign --verify -R='anchor apple generic and certificate leaf[subject.OU] = \"{TEAM_ID}\"' {app} && \\\n\
         spctl --assess --type execute {app}"
    )
}

/// Mounts a downloaded disk image and checks the app in it is signed by
/// Circle's team and notarized, before offering it.
#[cfg(target_os = "macos")]
fn verify_signature_in_dmg(dmg: &Path) -> Result<(), String> {
    let mount = dmg.with_extension("verify");
    let _ = fs::create_dir_all(&mount);
    let attached = Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-readonly", "-mountpoint"])
        .arg(&mount)
        .arg(dmg)
        .output()
        .map_err(|e| e.to_string())?;
    if !attached.status.success() {
        let _ = fs::remove_dir(&mount);
        return Err("couldn't open the downloaded update".to_owned());
    }
    let app = fs::read_dir(&mount).ok().and_then(|entries| {
        entries
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e == "app"))
    });
    let ok = app.is_some_and(|app| {
        Command::new("/bin/sh")
            .arg("-c")
            .arg(signature_check(&shell_quote(&app.to_string_lossy())))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    });
    let _ = Command::new("hdiutil")
        .args(["detach", "-quiet"])
        .arg(&mount)
        .status();
    let _ = fs::remove_dir(&mount);
    if ok {
        Ok(())
    } else {
        Err("the update isn't signed by Circle's developer, so it wasn't installed".to_owned())
    }
}

/// Checks a download against the release's `SHA256SUMS.txt` and returns
/// its hash.
fn verify(release: &Release, asset: &Asset, file: &Path) -> Result<String, String> {
    let sums = release
        .assets
        .iter()
        .find(|a| a.name == "SHA256SUMS.txt")
        .ok_or("the release has no checksums")?;
    let text = get_text(&sums.browser_download_url)
        .map_err(|e| format!("couldn't fetch checksums: {e}"))?;
    let expected = text
        .lines()
        .find_map(|line| {
            let (hash, name) = line.split_once(char::is_whitespace)?;
            (name.trim().trim_start_matches('*') == asset.name).then(|| hash.to_lowercase())
        })
        .ok_or("the update isn't in the checksums")?;
    let actual = sha256_of(file)?;
    if actual == expected {
        Ok(actual)
    } else {
        Err("the downloaded update didn't match its checksum".to_owned())
    }
}

/// `a` is a newer version than `b` (MAJOR.MINOR.PATCH; a pre-release is
/// older than its release).
pub fn is_newer(a: &str, b: &str) -> bool {
    fn parse(v: &str) -> (Vec<u64>, bool) {
        let (core, pre) = v.split_once('-').map_or((v, false), |(c, _)| (c, true));
        (
            core.split('.').map(|p| p.parse().unwrap_or(0)).collect(),
            pre,
        )
    }
    let ((a, a_pre), (b, b_pre)) = (parse(a), parse(b));
    match a.cmp(&b) {
        std::cmp::Ordering::Equal => !a_pre && b_pre,
        other => other == std::cmp::Ordering::Greater,
    }
}

/// The release file for this platform.
fn asset_suffix() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "-macos-arm64.dmg",
        ("macos", _) => "-macos-x86_64.dmg",
        ("windows", _) => "-windows-x86_64-setup.exe",
        ("linux", "aarch64") => "-linux-aarch64.tar.gz",
        ("linux", _) => "-linux-x86_64.tar.gz",
        _ => "\u{0}",
    }
}

/// Where Circle is installed: the `.app` on macOS, else the executable's
/// folder.
fn install_location() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    if cfg!(target_os = "macos") {
        // Circle.app/Contents/MacOS/Circle
        exe.ancestors()
            .nth(3)
            .map(Path::to_path_buf)
            .filter(|p| p.extension().is_some_and(|e| e == "app"))
    } else {
        exe.parent().map(Path::to_path_buf)
    }
}

/// Whether this install can replace itself. A development build can't,
/// and neither can a Linux package install (`/opt`, owned by root).
fn can_self_update() -> bool {
    let Some(location) = install_location() else {
        return false;
    };
    if location.components().any(|c| c.as_os_str() == "target") {
        return false;
    }
    // Run as root (`sudo circle`), a package install looks writable; it
    // belongs to the package manager.
    #[cfg(unix)]
    if unsafe { libc::geteuid() } == 0 {
        return false;
    }
    match std::env::consts::OS {
        // The installer takes care of permissions.
        "windows" => true,
        // The .app is replaced, which needs its folder to be writable; or,
        // where it isn't (/Applications for a standard account), the .app's
        // contents are, if the .app is ours.
        "macos" => location.parent().is_some_and(probe_writable) || owns_bundle(&location),
        _ => probe_writable(&location),
    }
}

/// Whether this user owns the .app and its Contents, so it can swap them.
#[cfg(unix)]
fn owns_bundle(app: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let uid = unsafe { libc::geteuid() };
    let owned = |p: &Path| fs::symlink_metadata(p).is_ok_and(|m| m.uid() == uid);
    owned(app) && owned(&app.join("Contents")) && probe_writable(app)
}

#[cfg(not(unix))]
fn owns_bundle(_app: &Path) -> bool {
    false
}

fn probe_writable(dir: &Path) -> bool {
    let probe = dir.join(".circle-update-probe");
    let ok = File::create(&probe).is_ok();
    let _ = fs::remove_file(&probe);
    ok
}

/// Set by "Restart to update": relaunch after installing.
static RELAUNCH: AtomicBool = AtomicBool::new(false);

pub fn relaunch_after_update() {
    RELAUNCH.store(true, Ordering::SeqCst);
}

/// Called as Circle exits: installs a downloaded update, if any, and
/// starts Circle again if asked to.
pub fn install_after_quit(dir: &Path) {
    let relaunch = RELAUNCH.load(Ordering::SeqCst);
    if let Some(update) = pending(dir)
        && can_self_update()
    {
        match install_on_exit(&update, relaunch) {
            Ok(()) => return,
            Err(err) => eprintln!("circle: couldn't install the update: {err}"),
        }
    }
    if relaunch && let Err(err) = relaunch_on_exit() {
        eprintln!("circle: couldn't restart: {err}");
    }
}

/// Starts Circle again once this process has exited (it holds the profile
/// until then).
fn relaunch_on_exit() -> Result<(), String> {
    let pid = std::process::id();
    #[cfg(not(target_os = "windows"))]
    {
        let start = match install_location() {
            Some(app) if cfg!(target_os = "macos") => {
                // A test profile stays a test profile.
                let profile = std::env::var("CIRCLE_PROFILE_DIR")
                    .map(|p| format!(" --env CIRCLE_PROFILE_DIR={}", shell_quote(&p)))
                    .unwrap_or_default();
                format!("open -n{profile} {}", shell_quote(&app.to_string_lossy()))
            }
            _ => {
                let exe = std::env::current_exe().map_err(|e| e.to_string())?;
                format!(
                    "nohup {} >/dev/null 2>&1 &",
                    shell_quote(&exe.to_string_lossy())
                )
            }
        };
        let script = format!("while kill -0 {pid} 2>/dev/null; do sleep 0.2; done\n{start}\n");
        Command::new("/bin/sh")
            .arg("-c")
            .arg(script)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let script = format!(
            "@echo off\r\n\
             :wait\r\n\
             tasklist /FI \"PID eq {pid}\" | find \"{pid}\" >nul && (timeout /t 1 /nobreak >nul & goto wait)\r\n\
             start \"\" \"{}\"\r\n\
             del \"%~f0\"\r\n",
            exe.display()
        );
        let path = std::env::temp_dir().join(format!("circle-restart-{pid}.cmd"));
        fs::write(&path, script).map_err(|e| e.to_string())?;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        Command::new("cmd")
            .arg("/C")
            .arg(&path)
            .creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Starts installing `update` once Circle exits, relaunching it afterwards
/// if `relaunch`. Circle must quit right after this returns.
pub fn install_on_exit(update: &Update, relaunch: bool) -> Result<(), String> {
    let location = install_location().ok_or("can't tell where Circle is installed")?;
    let pid = std::process::id();
    let dir = update.file.parent().unwrap_or(Path::new("."));
    let quote = |p: &Path| shell_quote(&p.to_string_lossy());
    let pending = dir.join("pending.json");

    #[cfg(not(target_os = "windows"))]
    {
        let body = if cfg!(target_os = "macos") {
            let mount = dir.join("mount");
            format!(
                "mkdir -p {m}\n\
                 hdiutil attach -nobrowse -readonly -mountpoint {m} {dmg} >/dev/null || exit 1\n\
                 new=$(ls -d {m}/*.app | head -1)\n\
                 if ! ( {check} ); then hdiutil detach {m} >/dev/null; exit 1; fi\n\
                 if [ -w {parent} ]; then\n\
                 \x20 rm -rf {app}.old\n\
                 \x20 if mv {app} {app}.old; then\n\
                 \x20   if ditto \"$new\" {app}; then rm -rf {app}.old; else rm -rf {app}; mv {app}.old {app}; fi\n\
                 \x20 fi\n\
                 else\n\
                 \x20 # The folder isn't writable (a standard account): swap the contents.\n\
                 \x20 rm -rf {app}/Contents.old\n\
                 \x20 if mv {app}/Contents {app}/Contents.old; then\n\
                 \x20   if ditto \"$new/Contents\" {app}/Contents; then rm -rf {app}/Contents.old; else rm -rf {app}/Contents; mv {app}/Contents.old {app}/Contents; fi\n\
                 \x20 fi\n\
                 \x20 touch {app}\n\
                 fi\n\
                 xattr -dr com.apple.quarantine {app} 2>/dev/null\n\
                 hdiutil detach {m} >/dev/null\n\
                 {relaunch}",
                check = signature_check("\"$new\""),
                m = quote(&mount),
                dmg = quote(&update.file),
                app = quote(&location),
                parent = quote(location.parent().unwrap_or(Path::new("/"))),
                relaunch = if relaunch {
                    format!("open -n {}", quote(&location))
                } else {
                    String::new()
                },
            )
        } else {
            let unpacked = dir.join("unpacked");
            format!(
                "rm -rf {u} && mkdir -p {u} && tar -xzf {tar} -C {u} || exit 1\n\
                 cp -a {u}/circle/. {app}/ && rm -rf {u}\n\
                 {relaunch}",
                u = quote(&unpacked),
                tar = quote(&update.file),
                app = quote(&location),
                relaunch = if relaunch {
                    format!(
                        "nohup {} >/dev/null 2>&1 &",
                        quote(&location.join("circle"))
                    )
                } else {
                    String::new()
                },
            )
        };
        let script = format!(
            "#!/bin/sh\n# Installs a Circle update once Circle has quit.\n\
             while kill -0 {pid} 2>/dev/null; do sleep 0.2; done\n\
             {body}\n\
             rm -f {pending} {file}\n\
             rm -rf \"$(dirname \"$0\")/mount\" \"$0\"\n",
            pending = quote(&pending),
            file = quote(&update.file),
        );
        let path = dir.join("install.sh");
        fs::write(&path, script).map_err(|e| e.to_string())?;
        Command::new("/bin/sh")
            .arg(&path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("couldn't start the installer: {e}"))?;
    }

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let exe = location.join("circle.exe");
        let script = format!(
            "@echo off\r\n\
             :wait\r\n\
             tasklist /FI \"PID eq {pid}\" | find \"{pid}\" >nul && (timeout /t 1 /nobreak >nul & goto wait)\r\n\
             \"{setup}\" /VERYSILENT /SUPPRESSMSGBOXES /NORESTART\r\n\
             {relaunch}\r\n\
             del \"{pending}\" \"{setup}\"\r\n",
            setup = update.file.display(),
            pending = pending.display(),
            relaunch = if relaunch {
                format!("start \"\" \"{}\"", exe.display())
            } else {
                String::new()
            },
        );
        let path = dir.join("install.cmd");
        fs::write(&path, script).map_err(|e| e.to_string())?;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        Command::new("cmd")
            .arg("/C")
            .arg(&path)
            .creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| format!("couldn't start the installer: {e}"))?;
        let _ = quote;
    }
    Ok(())
}

fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_names_stay_in_the_updates_folder() {
        assert!(is_plain_file_name("Circle-0.1.2-macos-arm64.dmg"));
        assert!(!is_plain_file_name("../Circle-macos-arm64.dmg"));
        assert!(!is_plain_file_name("/tmp/x-macos-arm64.dmg"));
        assert!(!is_plain_file_name("a\\b-windows-x86_64-setup.exe"));
        assert!(!is_plain_file_name(""));
    }

    #[test]
    #[cfg(not(windows))]
    fn the_installer_checks_the_signature_and_team() {
        let check = signature_check("\"$new\"");
        assert!(check.contains("codesign --verify --deep --strict \"$new\""));
        assert!(check.contains("certificate leaf[subject.OU] = \"5ZX3P79WQZ\""));
        assert!(check.contains("spctl --assess --type execute \"$new\""));
    }

    #[test]
    fn compares_versions() {
        assert!(is_newer("0.1.1", "0.1.0"));
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("0.2.0", "0.2.0-beta.1"));
        assert!(!is_newer("0.2.0-beta.1", "0.2.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
    }

    #[test]
    fn quotes_for_the_shell() {
        assert_eq!(
            shell_quote("/Applications/Circle.app"),
            "'/Applications/Circle.app'"
        );
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }
}
