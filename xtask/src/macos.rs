//! macOS: `Circle.app` with the CEF framework and helper apps, ad-hoc or
//! Developer ID signed, packaged as a `.dmg`.

use crate::{
    APP_NAME, BIN_NAME, BUNDLE_ID, Options, build_mcp, cargo_build, copy_dir, reset_dir, root, run,
    version,
};
use anyhow::{Result, bail};
use plist::{Dictionary, Value};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

const FRAMEWORK: &str = "Chromium Embedded Framework.framework";
const FRAMEWORK_BINARY: &str = "Chromium Embedded Framework";
const HELPERS: &[(&str, &str)] = &[
    ("", "helper"),
    (" (GPU)", "helper.gpu"),
    (" (Renderer)", "helper.renderer"),
    (" (Plugin)", "helper.plugin"),
    (" (Alerts)", "helper.alerts"),
];

fn dict(entries: Vec<(&str, Value)>) -> Value {
    Value::Dictionary(
        entries
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect::<Dictionary>(),
    )
}

fn s(value: &str) -> Value {
    Value::String(value.to_owned())
}

fn array(values: &[&str]) -> Value {
    Value::Array(values.iter().map(|v| s(v)).collect())
}

fn info_plist(executable: &str, identifier: &str, helper: bool) -> Result<Value> {
    let version = version()?;
    let mut entries = vec![
        ("CFBundleDevelopmentRegion", s("en")),
        ("CFBundleDisplayName", s(executable)),
        ("CFBundleExecutable", s(executable)),
        ("CFBundleIdentifier", s(identifier)),
        ("CFBundleInfoDictionaryVersion", s("6.0")),
        ("CFBundleName", s(executable)),
        ("CFBundlePackageType", s("APPL")),
        ("CFBundleShortVersionString", s(&version)),
        ("CFBundleVersion", s(&version)),
        ("LSMinimumSystemVersion", s("12.0")),
        ("LSEnvironment", dict(vec![("MallocNanoZone", s("0"))])),
        ("NSHighResolutionCapable", Value::Boolean(true)),
        ("NSSupportsAutomaticGraphicsSwitching", Value::Boolean(true)),
    ];
    if helper {
        entries.push(("LSUIElement", s("1")));
    } else {
        let usage = "Websites you visit in Circle may ask to use this.";
        entries.extend([
            ("CFBundleIconFile", s("circle")),
            (
                "LSApplicationCategoryType",
                s("public.app-category.productivity"),
            ),
            ("NSCameraUsageDescription", s(usage)),
            ("NSMicrophoneUsageDescription", s(usage)),
            ("NSBluetoothAlwaysUsageDescription", s(usage)),
            ("NSLocationUsageDescription", s(usage)),
            ("NSWebBrowserPublicKeyCredentialUsageDescription", s(usage)),
            (
                "CFBundleURLTypes",
                Value::Array(vec![dict(vec![
                    ("CFBundleURLName", s("Web site URL")),
                    ("CFBundleURLSchemes", array(&["http", "https"])),
                ])]),
            ),
            (
                "CFBundleDocumentTypes",
                Value::Array(vec![dict(vec![
                    ("CFBundleTypeName", s("HTML document")),
                    ("CFBundleTypeRole", s("Viewer")),
                    ("LSHandlerRank", s("Default")),
                    (
                        "LSItemContentTypes",
                        array(&["public.html", "public.xhtml"]),
                    ),
                ])]),
            ),
        ]);
    }
    Ok(dict(entries))
}

fn write_app(
    app: &Path,
    executable: &str,
    identifier: &str,
    binary: &Path,
    helper: bool,
) -> Result<()> {
    let contents = app.join("Contents");
    fs::create_dir_all(contents.join("MacOS"))?;
    fs::create_dir_all(contents.join("Resources"))?;
    info_plist(executable, identifier, helper)?.to_file_xml(contents.join("Info.plist"))?;
    fs::copy(binary, contents.join("MacOS").join(executable))?;
    Ok(())
}

/// The keychain item that holds the key encrypting cookies and saved data.
/// Chromium names it "Chromium Safe Storage", which every Chromium-based
/// app and test browser shares; Circle gets its own (same length, so the
/// name can be swapped in place before the framework is signed).
const KEYCHAIN_ITEM: (&[u8], &[u8]) = (b"Chromium Safe Storage\0", b"Circle Cookie Storage\0");

fn own_keychain_item(binary: &Path) -> Result<()> {
    let (from, to) = KEYCHAIN_ITEM;
    assert_eq!(from.len(), to.len());
    let mut bytes = fs::read(binary)?;
    let found: Vec<usize> = bytes
        .windows(from.len())
        .enumerate()
        .filter(|(_, w)| *w == from)
        .map(|(i, _)| i)
        .collect();
    let [at] = found[..] else {
        bail!(
            "expected the keychain name once in {}, found it {} times (did CEF change?)",
            binary.display(),
            found.len()
        );
    };
    bytes[at..at + to.len()].copy_from_slice(to);
    fs::write(binary, bytes)?;
    Ok(())
}

fn sign(path: &Path, identity: &str, entitlements: Option<&Path>) -> Result<()> {
    let mut cmd = Command::new("codesign");
    cmd.args(["--force", "--sign", identity]);
    if identity != "-" {
        cmd.args(["--options", "runtime", "--timestamp"]);
        if let Some(entitlements) = entitlements {
            cmd.arg("--entitlements").arg(entitlements);
        }
    }
    run(cmd.arg(path))
}

pub fn bundle(options: &Options) -> Result<PathBuf> {
    cargo_build(options, &["--bin", BIN_NAME, "--bin", "circle-helper"])?;
    let build = options.build_dir();
    let cef = options.cef_dir()?;
    let dist = options.dist_dir();
    fs::create_dir_all(&dist)?;
    let app = dist.join(format!("{APP_NAME}.app"));
    reset_dir(&app)?;

    write_app(&app, APP_NAME, BUNDLE_ID, &build.join(BIN_NAME), false)?;
    // The MCP bridge for AI agents ships inside the app.
    let mcp = app.join("Contents/MacOS/circle-mcp");
    fs::copy(build_mcp(options)?, &mcp)?;
    let resources = app.join("Contents/Resources");
    fs::copy(
        root().join("resources/icons/circle.icns"),
        resources.join("circle.icns"),
    )?;
    fs::copy(
        cef.join("CREDITS.html"),
        resources.join("CHROMIUM_CREDITS.html"),
    )?;

    let frameworks = app.join("Contents/Frameworks");
    copy_dir(&cef.join(FRAMEWORK), &frameworks.join(FRAMEWORK))?;
    own_keychain_item(&frameworks.join(FRAMEWORK).join(FRAMEWORK_BINARY))?;
    for (suffix, id) in HELPERS {
        let name = format!("{APP_NAME} Helper{suffix}");
        write_app(
            &frameworks.join(format!("{name}.app")),
            &name,
            &format!("{BUNDLE_ID}.{id}"),
            &build.join("circle-helper"),
            true,
        )?;
    }

    // Sign inside-out. Ad-hoc ("-") unless a Developer ID identity is given.
    let identity = env::var("CIRCLE_CODESIGN_IDENTITY").unwrap_or_else(|_| "-".into());
    let entitlements = root().join("packaging/macos");
    let framework = frameworks.join(FRAMEWORK);
    for library in fs::read_dir(framework.join("Libraries"))? {
        let library = library?.path();
        if library.extension().is_some_and(|e| e == "dylib") {
            sign(&library, &identity, None)?;
        }
    }
    sign(&framework, &identity, None)?;
    sign(&mcp, &identity, None)?;
    // Each process gets only the exceptions Chrome gives it: JIT for the
    // renderer and GPU, the plugin's for plugins, none for the rest (nor
    // for the app, which holds the camera, microphone and location).
    for (suffix, _) in HELPERS {
        let helper = frameworks.join(format!("{APP_NAME} Helper{suffix}.app"));
        let plist = match *suffix {
            " (Renderer)" => Some("helper-renderer.entitlements"),
            " (GPU)" => Some("helper-gpu.entitlements"),
            " (Plugin)" => Some("helper-plugin.entitlements"),
            _ => None,
        };
        let plist = plist.map(|p| entitlements.join(p));
        sign(&helper, &identity, plist.as_deref())?;
    }
    sign(
        &app,
        &identity,
        Some(&entitlements.join("app.entitlements")),
    )?;
    Ok(app)
}

pub fn package(options: &Options, app: &Path) -> Result<Vec<PathBuf>> {
    let version = version()?;
    let arch = match options.target_arch() {
        "aarch64" => "arm64",
        other => other,
    };
    let dmg = root()
        .join("dist")
        .join(format!("{APP_NAME}-{version}-macos-{arch}.dmg"));
    let staging = options.dist_dir().join("dmg");
    reset_dir(&staging)?;
    run(Command::new("ditto")
        .arg(app)
        .arg(staging.join(format!("{APP_NAME}.app"))))?;
    std::os::unix::fs::symlink("/Applications", staging.join("Applications"))?;
    let _ = fs::remove_file(&dmg);
    run(Command::new("hdiutil")
        .args([
            "create",
            "-volname",
            APP_NAME,
            "-fs",
            "HFS+",
            "-format",
            "UDZO",
            "-ov",
            "-srcfolder",
        ])
        .arg(&staging)
        .arg(&dmg))?;
    fs::remove_dir_all(&staging)?;
    // With a Developer ID, sign the disk image too (Gatekeeper checks it).
    let identity = env::var("CIRCLE_CODESIGN_IDENTITY").unwrap_or_else(|_| "-".into());
    if identity != "-" {
        sign(&dmg, &identity, None)?;
    }

    // Optional notarization (CI provides these for signed releases).
    if let (Ok(id), Ok(password), Ok(team)) = (
        env::var("APPLE_ID"),
        env::var("APPLE_APP_PASSWORD"),
        env::var("APPLE_TEAM_ID"),
    ) {
        run(Command::new("xcrun")
            .args([
                "notarytool",
                "submit",
                "--wait",
                "--apple-id",
                &id,
                "--password",
                &password,
                "--team-id",
                &team,
            ])
            .arg(&dmg))?;
        run(Command::new("xcrun").args(["stapler", "staple"]).arg(&dmg))?;
    }
    Ok(vec![dmg])
}
