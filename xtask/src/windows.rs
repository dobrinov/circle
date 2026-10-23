//! Windows: a directory with the CEF runtime. With the sandbox (default) CEF's
//! `bootstrap.exe` becomes `circle.exe` and loads our `circle.dll`. Shipped as
//! a `.zip` and, when Inno Setup is installed, a `-setup.exe` installer.

use crate::{
    BIN_NAME, Options, build_mcp, cargo_build, copy_cef_runtime, reset_dir, root, run, version,
};
use anyhow::Result;
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

pub fn bundle(options: &Options) -> Result<PathBuf> {
    let build = options.build_dir();
    let app = options.dist_dir().join("Circle");
    if options.sandbox {
        cargo_build(options, &["--lib"])?;
        reset_dir(&app)?;
        let cef = options.cef_dir()?;
        copy_cef_runtime(&cef, &app)?;
        fs::copy(
            cef.join("bootstrap.exe"),
            app.join(format!("{BIN_NAME}.exe")),
        )?;
        fs::copy(
            build.join(format!("{BIN_NAME}.dll")),
            app.join(format!("{BIN_NAME}.dll")),
        )?;
    } else {
        cargo_build(options, &["--bin", BIN_NAME])?;
        reset_dir(&app)?;
        copy_cef_runtime(&options.cef_dir()?, &app)?;
        fs::copy(
            build.join(format!("{BIN_NAME}.exe")),
            app.join(format!("{BIN_NAME}.exe")),
        )?;
    }
    fs::copy(build_mcp(options)?, app.join("circle-mcp.exe"))?;
    fs::copy(
        root().join("packaging/windows/circle.exe.manifest"),
        app.join(format!("{BIN_NAME}.exe.manifest")),
    )?;
    fs::copy(
        root().join("resources/icons/circle.ico"),
        app.join("circle.ico"),
    )?;
    Ok(app)
}

fn find_iscc() -> Option<PathBuf> {
    if let Ok(path) = env::var("ISCC") {
        return Some(PathBuf::from(path));
    }
    if Command::new("iscc").arg("/?").output().is_ok() {
        return Some(PathBuf::from("iscc"));
    }
    ["ProgramFiles(x86)", "ProgramFiles"]
        .iter()
        .filter_map(|var| env::var(var).ok())
        .map(|dir| PathBuf::from(dir).join("Inno Setup 6").join("ISCC.exe"))
        .find(|p| p.exists())
}

pub fn package(options: &Options, app: &Path) -> Result<Vec<PathBuf>> {
    let version = version()?;
    let arch = options.target_arch();
    let dist = root().join("dist");
    let mut artifacts = Vec::new();

    // Windows 10+ ships bsdtar, which writes zip files with `-a`.
    let zip = dist.join(format!("Circle-{version}-windows-{arch}.zip"));
    let _ = fs::remove_file(&zip);
    run(Command::new("tar")
        .arg("-a")
        .arg("-cf")
        .arg(&zip)
        .arg("-C")
        .arg(app.parent().unwrap())
        .arg("Circle"))?;
    artifacts.push(zip);

    match find_iscc() {
        Some(iscc) => {
            run(Command::new(iscc)
                .arg(format!("/DAppVersion={version}"))
                .arg(format!("/DSourceDir={}", app.display()))
                .arg(format!("/DOutputDir={}", dist.display()))
                .arg(format!(
                    "/DOutputName=Circle-{version}-windows-{arch}-setup"
                ))
                .arg(format!("/DArch={arch}"))
                .arg(root().join("packaging/windows/circle.iss")))?;
            artifacts.push(dist.join(format!("Circle-{version}-windows-{arch}-setup.exe")));
        }
        None => eprintln!("note: Inno Setup (ISCC) not found, skipping installer"),
    }
    Ok(artifacts)
}
