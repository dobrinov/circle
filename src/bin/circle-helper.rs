//! macOS sub-process helper (GPU, renderer, utility processes). It must
//! register the same custom schemes as the browser process.

#[cfg(target_os = "macos")]
fn main() {
    use cef::*;

    let args = args::Args::new();

    #[cfg(feature = "sandbox")]
    let _sandbox = {
        let mut sandbox = sandbox::Sandbox::new();
        sandbox.initialize(args.as_main_args());
        sandbox
    };

    let _library = {
        let exe = std::env::current_exe().expect("no current executable");
        let loader = library_loader::LibraryLoader::new(&exe, true);
        assert!(
            loader.load(),
            "failed to load the Chromium Embedded Framework"
        );
        loader
    };

    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);

    let mut app = circle::app::CircleApp::new(circle::profile_dir(), Vec::new());
    let code = execute_process(
        Some(args.as_main_args()),
        Some(&mut app),
        std::ptr::null_mut(),
    );
    std::process::exit(code.max(0));
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("circle-helper is only used on macOS");
}
