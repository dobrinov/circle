#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

#[cfg(not(all(target_os = "windows", feature = "sandbox")))]
fn main() {
    use cef::*;

    #[cfg(target_os = "macos")]
    let _library = {
        let exe = std::env::current_exe().expect("no current executable");
        let loader = library_loader::LibraryLoader::new(&exe, false);
        assert!(
            loader.load(),
            "failed to load the Chromium Embedded Framework"
        );
        loader
    };

    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);

    #[cfg(target_os = "macos")]
    circle::mac::init_application();

    let args = args::Args::new();
    std::process::exit(circle::run(args.as_main_args(), std::ptr::null_mut()));
}

#[cfg(all(target_os = "windows", feature = "sandbox"))]
fn main() {
    eprintln!(
        "With the sandbox enabled, Circle on Windows starts through bootstrap.exe. \
         Use `cargo xtask run` or build without default features."
    );
    std::process::exit(1);
}
