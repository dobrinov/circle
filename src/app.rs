//! The CEF application object, shared by the browser and all sub-processes.

use crate::{controller, scheme};
use cef::*;
use std::{os::raw::c_int, path::PathBuf};

/// Command-line arguments that look like something to open.
pub fn urls_from_args(args: impl IntoIterator<Item = String>) -> Vec<String> {
    args.into_iter()
        .filter(|arg| !arg.starts_with('-'))
        .filter_map(|arg| {
            let path = std::path::Path::new(&arg);
            if arg.contains("://") || arg.starts_with("about:") {
                Some(arg)
            } else if path.exists() {
                std::fs::canonicalize(path)
                    .ok()
                    .and_then(|p| url::Url::from_file_path(p).ok())
                    .map(|u| u.to_string())
            } else {
                None
            }
        })
        .collect()
}

wrap_app! {
    pub struct CircleApp {
        profile_dir: PathBuf,
        initial_urls: Vec<String>,
    }

    impl App {
        fn on_before_command_line_processing(
            &self,
            process_type: Option<&CefString>,
            command_line: Option<&mut CommandLine>,
        ) {
            let is_browser = process_type.is_none_or(|t| t.to_string().is_empty());
            let mut command_line = command_line;
            if let (true, Some(command_line)) = (is_browser, command_line.as_deref_mut()) {
                for (name, value) in crate::engine::switches() {
                    let name = CefString::from(name.as_str());
                    match value {
                        Some(value) => command_line
                            .append_switch_with_value(Some(&name), Some(&CefString::from(value.as_str()))),
                        None => command_line.append_switch(Some(&name)),
                    }
                }
            }
            // Every debug rebuild changes the ad-hoc code signature, which would
            // make macOS ask for keychain access on each run. Release builds
            // keep using the real keychain to encrypt cookies.
            if let (true, true, Some(command_line)) =
                (cfg!(all(debug_assertions, target_os = "macos")), is_browser, command_line)
            {
                command_line.append_switch(Some(&CefString::from("use-mock-keychain")));
            }
        }

        fn on_register_custom_schemes(&self, registrar: Option<&mut SchemeRegistrar>) {
            if let Some(registrar) = registrar {
                scheme::register_custom_scheme(registrar);
            }
        }

        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(CircleBrowserProcessHandler::new(
                self.profile_dir.clone(),
                self.initial_urls.clone(),
            ))
        }
    }
}

wrap_browser_process_handler! {
    struct CircleBrowserProcessHandler {
        profile_dir: PathBuf,
        initial_urls: Vec<String>,
    }

    impl BrowserProcessHandler {
        fn on_context_initialized(&self) {
            scheme::register_handler_factory();
            controller::start(&self.profile_dir, self.initial_urls.clone());
        }

        /// Chrome-style windows only come from Chromium itself, e.g. an
        /// extension calling `chrome.windows.create`: their pages move to
        /// Circle tabs.
        fn default_client(&self) -> Option<Client> {
            Some(crate::client::new_client(crate::client::Role::Stray))
        }

        /// A second launch (e.g. clicking a link with Circle as the default
        /// browser) is forwarded here instead of starting a new instance.
        fn on_already_running_app_relaunch(
            &self,
            command_line: Option<&mut CommandLine>,
            _current_directory: Option<&CefString>,
        ) -> c_int {
            let mut args = CefStringList::new();
            if let Some(command_line) = command_line {
                command_line.arguments(Some(&mut args));
            }
            controller::dispatch(controller::Cmd::OpenUrls(urls_from_args(args)));
            1
        }
    }
}
