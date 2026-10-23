//! Circle: a sidebar-first browser built on the Chromium Embedded Framework.

pub mod agent;
pub mod app;
pub mod bugreport;
pub mod client;
pub mod controller;
pub mod history;
pub mod keys;
pub mod model;
pub mod recorder;
pub mod remote;
pub mod scheme;
pub mod updater;
pub mod views;

#[cfg(target_os = "macos")]
pub mod mac;

use cef::*;
use std::path::PathBuf;

/// Where cookies, cache and the session file live. `CIRCLE_PROFILE_DIR`
/// overrides it, e.g. for a throwaway profile while developing.
/// Where downloads and recordings go. `CIRCLE_DOWNLOADS_DIR` overrides it
/// (handy for tests).
pub fn downloads_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("CIRCLE_DOWNLOADS_DIR") {
        return PathBuf::from(dir);
    }
    dirs::download_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(std::env::temp_dir)
}

pub fn profile_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("CIRCLE_PROFILE_DIR") {
        return PathBuf::from(dir);
    }
    let base = dirs::data_local_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(std::env::temp_dir);
    let name = if cfg!(target_os = "linux") {
        "circle"
    } else {
        "Circle"
    };
    base.join(name)
}

/// Runs the browser (or, for sub-processes, the requested process type) and
/// returns the process exit code.
pub fn run(main_args: &MainArgs, sandbox_info: *mut u8) -> i32 {
    let profile = profile_dir();
    let urls = app::urls_from_args(std::env::args().skip(1));
    let mut app = app::CircleApp::new(profile.clone(), urls);

    let code = execute_process(Some(main_args), Some(&mut app), sandbox_info);
    if code >= 0 {
        return code;
    }

    let settings = Settings {
        no_sandbox: !cfg!(feature = "sandbox") as _,
        root_cache_path: CefString::from(profile.to_string_lossy().as_ref()),
        cache_path: CefString::from(profile.join("Default").to_string_lossy().as_ref()),
        persist_session_cookies: 1,
        log_severity: LogSeverity::WARNING,
        ..Default::default()
    };
    if initialize(
        Some(main_args),
        Some(&settings),
        Some(&mut app),
        sandbox_info,
    ) != 1
    {
        // Most likely another instance owns the profile; it has been asked to
        // open our arguments via `on_already_running_app_relaunch`.
        return 0;
    }

    #[cfg(target_os = "macos")]
    let _delegate = mac::install_app_delegate();

    run_message_loop();
    shutdown();
    // A downloaded update installs once Circle has quit.
    updater::install_after_quit(&profile.join("updates"));
    0
}

/// Entry point used by CEF's sandboxed `bootstrap.exe` on Windows, which
/// loads `circle.dll` and calls this function.
///
/// # Safety
///
/// Only `bootstrap.exe` calls this, with its module handle and the sandbox
/// information CEF expects (or null).
#[cfg(all(target_os = "windows", feature = "sandbox"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn RunWinMain(
    instance: sys::HINSTANCE,
    _command_line: *const u8,
    _command_show: i32,
    sandbox_info: *mut u8,
) -> i32 {
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
    let main_args = MainArgs { instance };
    run(&main_args, sandbox_info)
}
