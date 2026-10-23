//! Linux: a self-contained directory (binary + CEF runtime), shipped as a
//! `.tar.gz` and a `.deb` that installs to `/opt/circle`.

use crate::{
    BIN_NAME, Options, build_mcp, cargo_build, copy_cef_runtime, copy_dir, reset_dir, root, run,
    version,
};
use anyhow::Result;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub fn bundle(options: &Options) -> Result<PathBuf> {
    cargo_build(options, &["--bin", BIN_NAME])?;
    let app = options.dist_dir().join(BIN_NAME);
    reset_dir(&app)?;
    copy_cef_runtime(&options.cef_dir()?, &app)?;
    fs::copy(options.build_dir().join(BIN_NAME), app.join(BIN_NAME))?;
    fs::copy(build_mcp(options)?, app.join("circle-mcp"))?;
    fs::copy(
        root().join("resources/icons/circle-256.png"),
        app.join("circle.png"),
    )?;
    fs::copy(
        root().join("packaging/linux/circle.desktop"),
        app.join("circle.desktop"),
    )?;
    strip_binaries(&app)?;
    Ok(app)
}

/// Strips symbols from every ELF file in the bundle. CEF's `libcef.so` ships
/// with its symbols, which roughly double the download.
fn strip_binaries(dir: &Path) -> Result<()> {
    if Command::new("strip").arg("--version").output().is_err() {
        eprintln!("note: strip not found, leaving binaries unstripped");
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            strip_binaries(&path)?;
            continue;
        }
        let mut magic = [0u8; 4];
        let is_elf = fs::File::open(&path)
            .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut magic))
            .is_ok()
            && magic == *b"\x7fELF";
        if is_elf {
            let before = fs::metadata(&path)?.len();
            run(Command::new("strip").arg("--strip-unneeded").arg(&path))?;
            let after = fs::metadata(&path)?.len();
            if before > 10_000_000 {
                println!(
                    "stripped {}: {} MB -> {} MB",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    before / 1_000_000,
                    after / 1_000_000
                );
            }
        }
    }
    Ok(())
}

fn deb_arch(arch: &str) -> &str {
    match arch {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    }
}

pub fn package(options: &Options, app: &Path) -> Result<Vec<PathBuf>> {
    let version = version()?;
    let arch = options.target_arch();
    let dist = root().join("dist");
    let mut artifacts = Vec::new();

    let tarball = dist.join(format!("circle-{version}-linux-{arch}.tar.gz"));
    run(Command::new("tar")
        .arg("-czf")
        .arg(&tarball)
        .arg("-C")
        .arg(app.parent().unwrap())
        .arg(BIN_NAME))?;
    artifacts.push(tarball);

    if Command::new("dpkg-deb").arg("--version").output().is_err() {
        eprintln!("note: dpkg-deb not found, skipping .deb");
        return Ok(artifacts);
    }
    let staging = options.dist_dir().join("deb");
    reset_dir(&staging)?;
    copy_dir(app, &staging.join("opt/circle"))?;
    fs::create_dir_all(staging.join("usr/bin"))?;
    std::os::unix::fs::symlink("/opt/circle/circle", staging.join("usr/bin/circle"))?;
    let applications = staging.join("usr/share/applications");
    fs::create_dir_all(&applications)?;
    fs::copy(
        root().join("packaging/linux/circle.desktop"),
        applications.join("circle.desktop"),
    )?;
    for size in [16, 32, 48, 64, 128, 256, 512] {
        let dir = staging.join(format!("usr/share/icons/hicolor/{size}x{size}/apps"));
        fs::create_dir_all(&dir)?;
        fs::copy(
            root().join(format!("resources/icons/circle-{size}.png")),
            dir.join("circle.png"),
        )?;
    }
    let debian = staging.join("DEBIAN");
    fs::create_dir_all(&debian)?;
    let control = fs::read_to_string(root().join("packaging/linux/control"))?
        .replace("@VERSION@", &version)
        .replace("@ARCH@", deb_arch(arch));
    fs::write(debian.join("control"), control)?;
    for script in ["postinst", "postrm"] {
        let target = debian.join(script);
        fs::copy(root().join("packaging/linux").join(script), &target)?;
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755))?;
    }
    let deb = dist.join(format!("circle_{version}_{}.deb", deb_arch(arch)));
    run(Command::new("dpkg-deb")
        .args(["--build", "--root-owner-group"])
        .arg(&staging)
        .arg(&deb))?;
    fs::remove_dir_all(&staging)?;
    artifacts.push(deb);
    Ok(artifacts)
}
