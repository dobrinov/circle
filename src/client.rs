//! CEF client handlers. Each browser gets a client tagged with its [`Role`] so
//! events can be attributed to a tab (or to the UI) without lookups.

use crate::{
    controller::{self, Action, Cmd, DownloadInfo, PermissionCallback, PermissionRequest},
    keys,
    model::TabId,
    scheme,
};
use cef::*;
use std::{cell::RefCell, os::raw::c_int, path::PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Tab(TabId),
    Sidebar,
    Palette,
    /// A script-opened popup window (OAuth flows and the like).
    Popup,
    /// Developer tools docked in the window, inspecting a tab.
    DevTools(TabId),
    /// The sidebar shown over the page when the pointer reaches the left
    /// edge while the real sidebar is collapsed.
    Peek,
    /// The hidden page that encodes video recordings.
    Recorder,
    /// The splash shown in the page area when the space has no open tab.
    Start,
    /// Recording controls floating over the page while the sidebar is hidden.
    RecBar,
}

/// Prefix of the console messages the docked DevTools' resize grip sends.
const DEVTOOLS_GRIP: &str = "__circle_devtools_grip__:";

/// Phases of dragging the docked DevTools' resize grip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GripDrag {
    Start,
    Move(i32, i32),
    End,
}

fn parse_grip(message: &str) -> Option<GripDrag> {
    let rest = message.strip_prefix(DEVTOOLS_GRIP)?;
    let mut parts = rest.split(':');
    match parts.next()? {
        "start" => Some(GripDrag::Start),
        "end" => Some(GripDrag::End),
        "move" => {
            let dx = parts.next()?.parse().ok()?;
            let dy = parts.next()?.parse().ok()?;
            Some(GripDrag::Move(dx, dy))
        }
        _ => None,
    }
}

/// A thin strip along the docked DevTools' inner edge; dragging it reports
/// the distance moved through the console (see `parse_grip`).
pub const DEVTOOLS_GRIP_JS: &str = r#"(side) => {
  let grip = window.__circleGrip;
  if (!grip) {
    grip = window.__circleGrip = document.createElement("div");
    let start = null;
    const say = (text) => console.debug("__circle_devtools_grip__:" + text);
    grip.addEventListener("pointerdown", (e) => {
      start = [e.screenX, e.screenY];
      grip.setPointerCapture(e.pointerId);
      e.preventDefault();
      say("start");
    });
    grip.addEventListener("pointermove", (e) => {
      if (start) say("move:" + Math.round(e.screenX - start[0]) + ":" + Math.round(e.screenY - start[1]));
    });
    const end = () => { if (start) say("end"); start = null; };
    grip.addEventListener("pointerup", end);
    grip.addEventListener("pointercancel", end);
    document.documentElement.appendChild(grip);
  }
  const right = side === "right";
  grip.style.cssText = "position:fixed;z-index:2147483647;background:transparent;" +
    (right ? "left:0;top:0;bottom:0;width:6px;cursor:col-resize;" : "left:0;right:0;top:0;height:6px;cursor:row-resize;");
}"#;

thread_local! {
    /// Every live browser, used to drive an orderly shutdown.
    static BROWSERS: RefCell<Vec<Browser>> = const { RefCell::new(Vec::new()) };
}

pub fn live_browsers() -> Vec<Browser> {
    BROWSERS.with(|b| b.borrow().clone())
}

pub fn new_client(role: Role) -> Client {
    CircleClient::new(role)
}

fn string(value: Option<&CefString>) -> String {
    value.map(CefString::to_string).unwrap_or_default()
}

wrap_client! {
    struct CircleClient {
        role: Role,
    }

    impl Client {
        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(CircleDisplayHandler::new(self.role))
        }

        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(CircleLifeSpanHandler::new(self.role))
        }

        fn load_handler(&self) -> Option<LoadHandler> {
            Some(CircleLoadHandler::new(self.role))
        }

        fn keyboard_handler(&self) -> Option<KeyboardHandler> {
            Some(CircleKeyboardHandler::new(self.role))
        }

        fn download_handler(&self) -> Option<DownloadHandler> {
            Some(CircleDownloadHandler::new(self.role))
        }

        fn find_handler(&self) -> Option<FindHandler> {
            Some(CircleFindHandler::new(self.role))
        }

        fn request_handler(&self) -> Option<RequestHandler> {
            Some(CircleRequestHandler::new(self.role))
        }

        fn permission_handler(&self) -> Option<PermissionHandler> {
            Some(CirclePermissionHandler::new(self.role))
        }

        fn drag_handler(&self) -> Option<DragHandler> {
            Some(CircleDragHandler::new(self.role))
        }

        fn context_menu_handler(&self) -> Option<ContextMenuHandler> {
            Some(CircleContextMenuHandler::new(self.role))
        }
    }
}

wrap_display_handler! {
    struct CircleDisplayHandler {
        role: Role,
    }

    impl DisplayHandler {
        fn on_title_change(&self, _browser: Option<&mut Browser>, title: Option<&CefString>) {
            if let Role::Tab(id) = self.role {
                controller::dispatch(Cmd::Title(id, string(title)));
            }
        }

        fn on_address_change(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            url: Option<&CefString>,
        ) {
            let is_main = frame.is_some_and(|f| f.is_main() != 0);
            if let (Role::Tab(id), true) = (self.role, is_main) {
                controller::dispatch(Cmd::Url(id, string(url)));
            }
        }

        fn on_favicon_urlchange(
            &self,
            _browser: Option<&mut Browser>,
            icon_urls: Option<&mut CefStringList>,
        ) {
            let Role::Tab(id) = self.role else {
                return;
            };
            // The list is borrowed from CEF; cloning copies the pointer only.
            let first = icon_urls.and_then(|urls| urls.clone().into_iter().next());
            if let Some(url) = first {
                controller::dispatch(Cmd::Favicon(id, url));
            }
        }

        fn on_console_message(
            &self,
            _browser: Option<&mut Browser>,
            level: LogSeverity,
            message: Option<&CefString>,
            source: Option<&CefString>,
            line: c_int,
        ) -> c_int {
            // Page warnings and errors go into a bug report being recorded.
            if matches!(self.role, Role::Tab(_))
                && crate::bugreport::is_recording()
                && (level == LogSeverity::WARNING || level == LogSeverity::ERROR)
            {
                let kind = if level == LogSeverity::ERROR { "console error" } else { "console warning" };
                let source = string(source);
                let place = if source.is_empty() { String::new() } else { format!("{source}:{line}") };
                controller::dispatch(Cmd::BugLog {
                    tab: match self.role { Role::Tab(id) => Some(id), _ => None },
                    kind,
                    text: string(message),
                    url: Some(place),
                });
            }
            let Role::DevTools(tab) = self.role else {
                return 0;
            };
            match parse_grip(&string(message)) {
                Some(drag) => {
                    controller::dispatch(Cmd::DevToolsGrip(tab, drag));
                    1
                }
                None => 0,
            }
        }

        fn on_fullscreen_mode_change(&self, _browser: Option<&mut Browser>, fullscreen: c_int) {
            if let Role::Tab(id) = self.role {
                controller::dispatch(Cmd::Fullscreen(id, fullscreen != 0));
            }
        }
    }
}

wrap_life_span_handler! {
    struct CircleLifeSpanHandler {
        role: Role,
    }

    impl LifeSpanHandler {
        fn on_before_popup(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _popup_id: c_int,
            target_url: Option<&CefString>,
            _target_frame_name: Option<&CefString>,
            target_disposition: WindowOpenDisposition,
            user_gesture: c_int,
            _popup_features: Option<&PopupFeatures>,
            _window_info: Option<&mut WindowInfo>,
            client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _no_javascript_access: Option<&mut c_int>,
        ) -> c_int {
            let url = string(target_url);
            // Pop-up blocking: pages open tabs and windows only when clicked.
            if user_gesture == 0 && matches!(self.role, Role::Tab(_) | Role::Popup) {
                return 1;
            }
            let opener = match self.role {
                Role::Tab(id) => Some(id),
                _ => None,
            };
            // Real popups (window.open with features, e.g. sign-in flows) keep
            // their opener and get their own small window.
            if matches!(self.role, Role::Tab(_) | Role::Popup)
                && target_disposition == WindowOpenDisposition::NEW_POPUP
            {
                if let Some(client) = client {
                    *client = Some(new_client(Role::Popup));
                }
                return 0;
            }
            let background = target_disposition == WindowOpenDisposition::NEW_BACKGROUND_TAB;
            controller::dispatch(Cmd::OpenUrl { url, background, opener });
            1
        }

        fn on_after_created(&self, browser: Option<&mut Browser>) {
            let Some(browser) = browser.cloned() else {
                return;
            };
            if matches!(
                self.role,
                Role::Sidebar
                    | Role::Palette
                    | Role::Peek
                    | Role::Recorder
                    | Role::Start
                    | Role::RecBar
            ) {
                scheme::allow_browser(browser.identifier());
            }
            BROWSERS.with(|b| b.borrow_mut().push(browser.clone()));
            if matches!(self.role, Role::Tab(_) | Role::Palette) {
                controller::dispatch(Cmd::BrowserCreated(self.role, browser));
            }
        }

        /// Returning false here makes CEF close the browser's top-level
        /// window too. Circle's main window hosts many browsers, so closing a
        /// tab or a docked panel must not close it: Circle removes the view
        /// itself. Popups own their window, and during shutdown the main
        /// window is closing anyway.
        fn do_close(&self, _browser: Option<&mut Browser>) -> c_int {
            match self.role {
                Role::Popup => 0,
                _ => (!controller::is_window_closing()) as c_int,
            }
        }

        fn on_before_close(&self, browser: Option<&mut Browser>) {
            let Some(browser) = browser else {
                return;
            };
            let id = browser.identifier();
            scheme::forget_browser(id);
            BROWSERS.with(|b| b.borrow_mut().retain(|b| b.identifier() != id));
            controller::on_browser_closed();
        }
    }
}

wrap_request_handler! {
    struct CircleRequestHandler {
        role: Role,
    }

    impl RequestHandler {
        /// Circle's own views (sidebar, command bar, splash, …) only ever
        /// show Circle's pages: a link dropped on them, or anything else,
        /// can't navigate them to a website, which would then receive the
        /// state Circle pushes there and could pass for Circle's UI.
        fn on_before_browse(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            request: Option<&mut Request>,
            _user_gesture: c_int,
            _is_redirect: c_int,
        ) -> c_int {
            let ui = matches!(
                self.role,
                Role::Sidebar | Role::Palette | Role::Peek | Role::Recorder | Role::Start | Role::RecBar
            );
            let url = request.map(|r| CefString::from(&r.url()).to_string()).unwrap_or_default();
            (ui && !url.starts_with("circle://ui/")) as c_int
        }

        fn on_render_process_terminated(
            &self,
            browser: Option<&mut Browser>,
            _status: TerminationStatus,
            error_code: c_int,
            error_string: Option<&CefString>,
        ) {
            if matches!(self.role, Role::Tab(_)) && crate::bugreport::is_recording() {
                let url = browser
                    .and_then(|b| b.main_frame())
                    .map(|f| CefString::from(&f.url()).to_string());
                controller::dispatch(Cmd::BugLog {
                    tab: match self.role { Role::Tab(id) => Some(id), _ => None },
                    kind: "page crashed",
                    text: format!("{} (code {error_code})", string(error_string)),
                    url,
                });
            }
        }
    }
}

wrap_load_handler! {
    struct CircleLoadHandler {
        role: Role,
    }

    impl LoadHandler {
        fn on_load_end(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, _status: c_int) {
            let is_main = frame.is_some_and(|f| f.is_main() != 0);
            if let (Role::DevTools(tab), true) = (self.role, is_main) {
                controller::dispatch(Cmd::DevToolsLoaded(tab));
            }
        }

        fn on_load_error(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            _error_code: Errorcode,
            error_text: Option<&CefString>,
            failed_url: Option<&CefString>,
        ) {
            let text = string(error_text);
            // ERR_ABORTED is a navigation replaced by another, not a failure.
            if matches!(self.role, Role::Tab(_)) && crate::bugreport::is_recording() && text != "ERR_ABORTED" {
                let main = frame.is_some_and(|f| f.is_main() != 0);
                controller::dispatch(Cmd::BugLog {
                    tab: match self.role { Role::Tab(id) => Some(id), _ => None },
                    kind: if main { "load failed" } else { "frame load failed" },
                    text,
                    url: Some(string(failed_url)),
                });
            }
        }

        fn on_loading_state_change(
            &self,
            _browser: Option<&mut Browser>,
            is_loading: c_int,
            can_go_back: c_int,
            can_go_forward: c_int,
        ) {
            if let Role::Tab(id) = self.role {
                controller::dispatch(Cmd::Loading {
                    id,
                    loading: is_loading != 0,
                    back: can_go_back != 0,
                    forward: can_go_forward != 0,
                });
            }
        }
    }
}

wrap_keyboard_handler! {
    struct CircleKeyboardHandler {
        role: Role,
    }

    impl KeyboardHandler {
        // The native event's type differs per platform, hence three versions.
        #[cfg(target_os = "macos")]
        fn on_pre_key_event(
            &self,
            _browser: Option<&mut Browser>,
            event: Option<&KeyEvent>,
            os_event: *mut u8,
            _is_keyboard_shortcut: Option<&mut c_int>,
        ) -> c_int {
            match shortcut_action(self.role, event) {
                Some(action) => {
                    crate::mac::note_handled_key_event(os_event, action);
                    controller::dispatch(Cmd::Action(action));
                    1
                }
                None => 0,
            }
        }

        #[cfg(target_os = "linux")]
        fn on_pre_key_event(
            &self,
            _browser: Option<&mut Browser>,
            event: Option<&KeyEvent>,
            _os_event: Option<&mut cef::sys::XEvent>,
            _is_keyboard_shortcut: Option<&mut c_int>,
        ) -> c_int {
            run_shortcut(self.role, event)
        }

        #[cfg(target_os = "windows")]
        fn on_pre_key_event(
            &self,
            _browser: Option<&mut Browser>,
            event: Option<&KeyEvent>,
            _os_event: Option<&mut cef::sys::MSG>,
            _is_keyboard_shortcut: Option<&mut c_int>,
        ) -> c_int {
            run_shortcut(self.role, event)
        }
    }
}

/// The Circle action a key press triggers in a browser with `role`.
fn shortcut_action(role: Role, event: Option<&KeyEvent>) -> Option<Action> {
    match role {
        Role::Popup => None,
        // DevTools has its own shortcuts; only keep the one that closes it.
        Role::DevTools(_) => event
            .and_then(keys::action_for)
            .filter(|a| *a == Action::DevTools),
        _ => event.and_then(keys::action_for),
    }
}

/// Runs the shortcut for a key press, if any; 1 when handled.
#[cfg(not(target_os = "macos"))]
fn run_shortcut(role: Role, event: Option<&KeyEvent>) -> c_int {
    match shortcut_action(role, event) {
        Some(action) => {
            controller::dispatch(Cmd::Action(action));
            1
        }
        None => 0,
    }
}

/// File types that run code when opened.
fn is_dangerous_download(name: &str) -> bool {
    const DANGEROUS: &[&str] = &[
        "app",
        "command",
        "tool",
        "terminal",
        "pkg",
        "mpkg",
        "dmg",
        "workflow",
        "scpt",
        "scptd",
        "applescript",
        "jar",
        "sh",
        "bash",
        "zsh",
        "csh",
        "ksh",
        "py",
        "pl",
        "rb",
        "php",
        "desktop",
        "appimage",
        "run",
        "deb",
        "rpm",
        "exe",
        "msi",
        "msix",
        "appx",
        "bat",
        "cmd",
        "com",
        "scr",
        "pif",
        "ps1",
        "psm1",
        "vbs",
        "vbe",
        "js",
        "jse",
        "wsf",
        "wsh",
        "hta",
        "lnk",
        "reg",
        "cpl",
        "inf",
        "url",
        "webloc",
        "dll",
        "msc",
        "jnlp",
    ];
    std::path::Path::new(name)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .is_some_and(|e| DANGEROUS.contains(&e.as_str()))
}

/// Downloads each browser started recently, to throttle floods.
static RECENT_DOWNLOADS: std::sync::Mutex<Vec<(i32, std::time::Instant)>> =
    std::sync::Mutex::new(Vec::new());

fn allow_download(browser: i32) -> bool {
    const WINDOW: std::time::Duration = std::time::Duration::from_secs(10);
    const MAX: usize = 5;
    let now = std::time::Instant::now();
    let mut recent = RECENT_DOWNLOADS.lock().unwrap();
    recent.retain(|(_, at)| now.duration_since(*at) < WINDOW);
    if recent.iter().filter(|(b, _)| *b == browser).count() >= MAX {
        return false;
    }
    recent.push((browser, now));
    true
}

/// Marks a finished download as coming from the internet (macOS
/// quarantine, Windows Mark of the Web), unless Chromium already did, so
/// the system checks it before it first runs.
fn mark_downloaded(path: &std::path::Path, url: &str) {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let Ok(file) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
            return;
        };
        let name = c"com.apple.quarantine";
        // SAFETY: both strings are NUL-terminated and outlive the calls.
        let present =
            unsafe { libc::getxattr(file.as_ptr(), name.as_ptr(), std::ptr::null_mut(), 0, 0, 0) }
                >= 0;
        if !present {
            let seconds = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            let value = format!("0081;{seconds:08x};Circle;");
            unsafe {
                libc::setxattr(
                    file.as_ptr(),
                    name.as_ptr(),
                    value.as_ptr().cast(),
                    value.len(),
                    0,
                    0,
                );
            }
        }
    }
    #[cfg(target_os = "windows")]
    {
        let mut stream = path.as_os_str().to_owned();
        stream.push(":Zone.Identifier");
        let stream = std::path::PathBuf::from(stream);
        if !stream.exists() {
            let host = url::Url::parse(url)
                .map(|u| u.origin().ascii_serialization())
                .unwrap_or_default();
            let _ = std::fs::write(
                stream,
                format!("[ZoneTransfer]\r\nZoneId=3\r\nHostUrl={host}\r\n"),
            );
        }
    }
    let _ = (path, url);
}

fn unique_download_path(dir: PathBuf, name: &str) -> PathBuf {
    let name = if name.trim().is_empty() {
        "download"
    } else {
        name
    };
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let path = std::path::Path::new(name);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    let ext = path.extension().and_then(|s| s.to_str());
    (1..)
        .map(|n| match ext {
            Some(ext) => dir.join(format!("{stem} ({n}).{ext}")),
            None => dir.join(format!("{stem} ({n})")),
        })
        .find(|p| !p.exists())
        .unwrap()
}

wrap_download_handler! {
    struct CircleDownloadHandler {
        role: Role,
    }

    impl DownloadHandler {
        /// The C++ default allows downloads; the Rust default would block them.
        /// A page may start a few downloads at a time, not a flood.
        fn can_download(
            &self,
            browser: Option<&mut Browser>,
            _url: Option<&CefString>,
            _request_method: Option<&CefString>,
        ) -> c_int {
            let Some(browser) = browser else {
                return 1;
            };
            allow_download(browser.identifier()) as c_int
        }

        fn on_before_download(
            &self,
            _browser: Option<&mut Browser>,
            _download_item: Option<&mut DownloadItem>,
            suggested_name: Option<&CefString>,
            callback: Option<&mut BeforeDownloadCallback>,
        ) -> c_int {
            let Some(callback) = callback else {
                return 0;
            };
            let dir = crate::downloads_dir();
            let name = string(suggested_name);
            let path = unique_download_path(dir, &name);
            // Files that run when opened are saved only once the person
            // confirms them in the save dialog.
            let confirm = is_dangerous_download(&name);
            callback.cont(Some(&CefString::from(path.to_string_lossy().as_ref())), confirm as c_int);
            1
        }

        fn on_download_updated(
            &self,
            _browser: Option<&mut Browser>,
            item: Option<&mut DownloadItem>,
            _callback: Option<&mut DownloadItemCallback>,
        ) {
            let Some(item) = item else {
                return;
            };
            let path = CefString::from(&item.full_path()).to_string();
            if path.is_empty() {
                return;
            }
            let name = std::path::Path::new(&path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if item.is_complete() != 0 {
                mark_downloaded(
                    std::path::Path::new(&path),
                    &CefString::from(&item.url()).to_string(),
                );
            }
            controller::dispatch(Cmd::Download(DownloadInfo {
                id: item.id(),
                at: 0,
                name,
                path,
                percent: item.percent_complete(),
                complete: item.is_complete() != 0,
                failed: item.is_canceled() != 0 || item.is_interrupted() != 0,
            }));
        }
    }
}

wrap_find_handler! {
    struct CircleFindHandler {
        role: Role,
    }

    impl FindHandler {
        fn on_find_result(
            &self,
            _browser: Option<&mut Browser>,
            _identifier: c_int,
            count: c_int,
            _selection_rect: Option<&Rect>,
            active_match_ordinal: c_int,
            _final_update: c_int,
        ) {
            controller::dispatch(Cmd::FindResult { count, active: active_match_ordinal });
        }
    }
}

fn origin_of(url: Option<&CefString>) -> String {
    let url = string(url);
    url::Url::parse(&url)
        .map(|u| u.origin().ascii_serialization())
        .unwrap_or(url)
}

wrap_permission_handler! {
    struct CirclePermissionHandler {
        role: Role,
    }

    impl PermissionHandler {
        fn on_request_media_access_permission(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            requesting_origin: Option<&CefString>,
            requested_permissions: u32,
            callback: Option<&mut MediaAccessCallback>,
        ) -> c_int {
            let (Role::Tab(tab), Some(callback)) = (self.role, callback) else {
                return 0;
            };
            controller::dispatch(Cmd::Permission(PermissionRequest {
                tab,
                cef_prompt_id: None,
                origin: origin_of(requesting_origin),
                mask: requested_permissions,
                media: true,
                callback: PermissionCallback::Media(callback.clone()),
            }));
            1
        }

        fn on_show_permission_prompt(
            &self,
            _browser: Option<&mut Browser>,
            prompt_id: u64,
            requesting_origin: Option<&CefString>,
            requested_permissions: u32,
            callback: Option<&mut PermissionPromptCallback>,
        ) -> c_int {
            let (Role::Tab(tab), Some(callback)) = (self.role, callback) else {
                return 0;
            };
            controller::dispatch(Cmd::Permission(PermissionRequest {
                tab,
                cef_prompt_id: Some(prompt_id),
                origin: origin_of(requesting_origin),
                mask: requested_permissions,
                media: false,
                callback: PermissionCallback::Prompt(callback.clone()),
            }));
            1
        }

        fn on_dismiss_permission_prompt(
            &self,
            _browser: Option<&mut Browser>,
            prompt_id: u64,
            _result: PermissionRequestResult,
        ) {
            controller::dispatch(Cmd::PermissionDismissed(prompt_id));
        }
    }
}

wrap_drag_handler! {
    struct CircleDragHandler {
        role: Role,
    }

    impl DragHandler {
        fn on_draggable_regions_changed(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            regions: Option<&[DraggableRegion]>,
        ) {
            if self.role == Role::Sidebar {
                controller::dispatch(Cmd::DragRegions(regions.unwrap_or_default().to_vec()));
            }
        }
    }
}

/// Our own context menu command ids start here (CEF's MENU_ID_USER_FIRST).
const MENU_INSPECT: c_int = 26500;

wrap_context_menu_handler! {
    struct CircleContextMenuHandler {
        role: Role,
    }

    impl ContextMenuHandler {
        fn on_before_context_menu(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _params: Option<&mut ContextMenuParams>,
            model: Option<&mut MenuModel>,
        ) {
            let (Role::Tab(_), Some(model)) = (self.role, model) else {
                return;
            };
            if model.count() > 0 {
                model.add_separator();
            }
            model.add_item(MENU_INSPECT, Some(&CefString::from("Inspect Element")));
        }

        fn on_context_menu_command(
            &self,
            browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            params: Option<&mut ContextMenuParams>,
            command_id: c_int,
            _event_flags: EventFlags,
        ) -> c_int {
            let _ = browser;
            if command_id != MENU_INSPECT {
                return 0;
            }
            if let (Role::Tab(tab), Some(params)) = (self.role, params) {
                let point = Point { x: params.xcoord(), y: params.ycoord() };
                controller::dispatch(Cmd::Inspect(tab, point));
            }
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dangerous_downloads_need_confirming() {
        for name in [
            "Setup.exe",
            "install.COMMAND",
            "x.pkg",
            "run.sh",
            "a.tar.gz.app",
        ] {
            assert!(is_dangerous_download(name), "{name}");
        }
        for name in [
            "report.pdf",
            "photo.jpg",
            "notes.txt",
            "archive.zip",
            "noext",
        ] {
            assert!(!is_dangerous_download(name), "{name}");
        }
    }

    #[test]
    fn download_floods_are_throttled() {
        let browser = -77;
        let allowed = (0..8).filter(|_| allow_download(browser)).count();
        assert_eq!(allowed, 5);
        assert!(allow_download(-78), "other pages aren't affected");
    }

    #[test]
    fn parses_grip_messages() {
        assert_eq!(
            parse_grip("__circle_devtools_grip__:start"),
            Some(GripDrag::Start)
        );
        assert_eq!(
            parse_grip("__circle_devtools_grip__:move:-12:30"),
            Some(GripDrag::Move(-12, 30))
        );
        assert_eq!(
            parse_grip("__circle_devtools_grip__:end"),
            Some(GripDrag::End)
        );
        assert_eq!(parse_grip("hello"), None);
        assert_eq!(parse_grip("__circle_devtools_grip__:move:x:1"), None);
    }
}
