use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo::rerun-if-changed=build.rs");

    // `cef-dll-sys` resolves (and downloads, if needed) the CEF binary
    // distribution for the *target* platform and exposes it as links metadata.
    // Record it next to the build artifacts so `cargo xtask bundle` can copy
    // the matching runtime files into the app bundle.
    if let Ok(cef_dir) = env::var("DEP_CEF_DLL_WRAPPER_CEF_DIR") {
        let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
        // OUT_DIR = <target>/<profile>/build/circle-<hash>/out
        let profile_dir = out_dir.ancestors().nth(3).unwrap();
        fs::write(profile_dir.join("cef-dir.txt"), &cef_dir).unwrap();
    }

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    if target_os == "linux" {
        // libcef.so lives next to the executable.
        println!("cargo::rustc-link-arg-bins=-Wl,-rpath,$ORIGIN");
    }
}
