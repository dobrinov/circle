//! Build tasks for Circle: `cargo xtask <command>`.
//!
//!   run      [--release] [--no-sandbox]          bundle and launch
//!   bundle   [--release] [--target T] [--no-sandbox]
//!   dist     [--target T] [--no-sandbox]          release bundle + installers
//!   icons                                         regenerate resources/icons
//!   release  <version>                           bump versions, stamp CHANGELOG, commit and tag
//!   gif      <frames-dir> <out.gif> [--width N] [--bg RRGGBB]
//!                                                 encode captured frames (website demos)

mod gif;
mod icons;
#[cfg(unix)]
mod linux;
#[cfg(unix)]
mod macos;
mod release;
mod windows;

use anyhow::{Context, Result, bail};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

pub const APP_NAME: &str = "Circle";
pub const BIN_NAME: &str = "circle";
pub const BUNDLE_ID: &str = "dev.circlebrowser.Circle";

#[derive(Clone, Debug)]
pub struct Options {
    pub release: bool,
    pub target: Option<String>,
    pub sandbox: bool,
}

impl Options {
    pub fn profile(&self) -> &'static str {
        if self.release { "release" } else { "debug" }
    }

    pub fn target_os(&self) -> &str {
        match &self.target {
            Some(t) if t.contains("apple-darwin") => "macos",
            Some(t) if t.contains("windows") => "windows",
            Some(t) if t.contains("linux") => "linux",
            Some(t) => panic!("unsupported target {t}"),
            None => env::consts::OS,
        }
    }

    pub fn target_arch(&self) -> &str {
        match &self.target {
            Some(t) => t.split('-').next().unwrap(),
            None => env::consts::ARCH,
        }
    }

    /// e.g. `macos-aarch64`
    pub fn platform(&self) -> String {
        format!("{}-{}", self.target_os(), self.target_arch())
    }

    /// The cargo output directory for the configured target and profile.
    pub fn build_dir(&self) -> PathBuf {
        let mut dir = root().join("target");
        if let Some(target) = &self.target {
            dir.push(target);
        }
        dir.join(self.profile())
    }

    pub fn dist_dir(&self) -> PathBuf {
        root().join("dist").join(self.platform())
    }

    /// The CEF binary distribution `cef-dll-sys` resolved for this target.
    pub fn cef_dir(&self) -> Result<PathBuf> {
        let record = self.build_dir().join("cef-dir.txt");
        let dir = fs::read_to_string(&record)
            .with_context(|| format!("missing {} (build first)", record.display()))?;
        Ok(PathBuf::from(dir.trim()))
    }
}

pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

pub fn version() -> Result<String> {
    let manifest = fs::read_to_string(root().join("Cargo.toml"))?;
    let package = manifest.split("[package]").nth(1).context("no [package]")?;
    let line = package
        .lines()
        .find(|l| l.trim_start().starts_with("version"))
        .context("no version")?;
    Ok(line.split('"').nth(1).context("bad version")?.to_owned())
}

pub fn run(cmd: &mut Command) -> Result<()> {
    eprintln!("$ {cmd:?}");
    let status = cmd
        .status()
        .with_context(|| format!("failed to start {cmd:?}"))?;
    if !status.success() {
        bail!("command failed ({status}): {cmd:?}");
    }
    Ok(())
}

pub fn cargo_build(options: &Options, args: &[&str]) -> Result<()> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.current_dir(root())
        .args(["build", "--package", BIN_NAME]);
    if options.release {
        cmd.arg("--release");
    }
    if let Some(target) = &options.target {
        cmd.args(["--target", target]);
    }
    if !options.sandbox {
        cmd.arg("--no-default-features");
    }
    cmd.args(args);
    run(&mut cmd)
}

/// Builds the `circle-mcp` agent bridge and returns its path.
pub fn build_mcp(options: &Options) -> Result<PathBuf> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.current_dir(root())
        .args(["build", "--package", "circle-mcp"]);
    if options.release {
        cmd.arg("--release");
    }
    if let Some(target) = &options.target {
        cmd.args(["--target", target]);
    }
    run(&mut cmd)?;
    let exe = if options.target_os() == "windows" {
        "circle-mcp.exe"
    } else {
        "circle-mcp"
    };
    Ok(options.build_dir().join(exe))
}

pub fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            let link = fs::read_link(entry.path())?;
            let _ = fs::remove_file(&target);
            #[cfg(unix)]
            std::os::unix::fs::symlink(link, &target)?;
            #[cfg(not(unix))]
            fs::copy(entry.path().parent().unwrap().join(link), &target).map(|_| ())?;
        } else if kind.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Copies the CEF runtime (everything at the top level of the distribution
/// that is not part of the SDK) plus `locales/`.
pub fn copy_cef_runtime(cef: &Path, dest: &Path) -> Result<()> {
    const SKIP: &[&str] = &[
        "archive.json",
        "CMakeLists.txt",
        "README.txt",
        "LICENSE.txt",
    ];
    for entry in fs::read_dir(cef)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if entry.file_type()?.is_file()
            && !SKIP.contains(&name.as_str())
            && !matches!(ext, "lib" | "exe" | "pdb")
        {
            if name == "CREDITS.html" {
                fs::copy(&path, dest.join("CHROMIUM_CREDITS.html"))?;
            } else {
                fs::copy(&path, dest.join(&name))?;
            }
        }
    }
    copy_dir(&cef.join("locales"), &dest.join("locales"))
}

pub fn reset_dir(dir: &Path) -> Result<()> {
    if dir.exists() {
        fs::remove_dir_all(dir).with_context(|| format!("removing {}", dir.display()))?;
    }
    fs::create_dir_all(dir)?;
    Ok(())
}

/// Builds the app and assembles a runnable bundle. Returns the path to launch.
fn bundle(options: &Options) -> Result<PathBuf> {
    match options.target_os() {
        #[cfg(unix)]
        "macos" => macos::bundle(options),
        #[cfg(unix)]
        "linux" => linux::bundle(options),
        "windows" => windows::bundle(options),
        os => bail!("unsupported OS {os}"),
    }
}

fn dist(options: &Options) -> Result<Vec<PathBuf>> {
    let app = bundle(options)?;
    match options.target_os() {
        #[cfg(unix)]
        "macos" => macos::package(options, &app),
        #[cfg(unix)]
        "linux" => linux::package(options, &app),
        "windows" => windows::package(options, &app),
        os => bail!("unsupported OS {os}"),
    }
}

fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "help".into());
    match command.as_str() {
        "gif" => return gif::run(&args.collect::<Vec<_>>()),
        "release" => return release::run(&args.collect::<Vec<_>>()),
        _ => {}
    }
    let mut options = Options {
        release: false,
        target: None,
        sandbox: true,
    };
    let mut passthrough = Vec::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--release" => options.release = true,
            "--no-sandbox" => options.sandbox = false,
            "--target" => options.target = Some(args.next().context("--target needs a value")?),
            "--" => passthrough.extend(args.by_ref()),
            other => bail!("unknown argument {other}"),
        }
    }

    match command.as_str() {
        "icons" => icons::generate(),
        "bundle" => {
            let path = bundle(&options)?;
            println!("Bundle ready: {}", path.display());
            Ok(())
        }
        "run" => {
            let path = bundle(&options)?;
            let exe = match options.target_os() {
                "macos" => path.join("Contents/MacOS").join(APP_NAME),
                "windows" => path.join(format!("{BIN_NAME}.exe")),
                _ => path.join(BIN_NAME),
            };
            run(Command::new(exe).args(passthrough))
        }
        "dist" => {
            options.release = true;
            for artifact in dist(&options)? {
                println!("Built {}", artifact.display());
            }
            Ok(())
        }
        _ => {
            eprintln!(
                "usage: cargo xtask <run|bundle|dist|icons|release|gif> [--release] [--target TRIPLE] [--no-sandbox] [-- app args]"
            );
            Ok(())
        }
    }
}
