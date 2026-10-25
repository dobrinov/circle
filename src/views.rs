//! CEF Views delegates: the main window, browser views and popup windows.
//! Note: cef-rs gives every callback a zero/null default, unlike the C++ API
//! (where e.g. `can_resize` is true and the popup delegate is the opener's).
//! Callbacks whose C++ default is non-zero are therefore implemented here.
//! Everything uses Alloy style because a Chrome-style window can host only a
//! single Chrome-style browser, and Circle shows several at once.

use crate::{
    client::{Role, new_client},
    controller::{self, Cmd},
};
use cef::*;
use std::os::raw::c_int;

pub const SIDEBAR_WIDTH: c_int = 252;
const MAC: bool = cfg!(target_os = "macos");
/// Height of the row holding the traffic lights; the sidebar's top bar matches it.
const TITLEBAR_HEIGHT: f32 = 40.0;

thread_local! {
    static SIDEBAR_CURRENT_WIDTH: std::cell::Cell<c_int> = const { std::cell::Cell::new(SIDEBAR_WIDTH) };
}

thread_local! {
    static DEVTOOLS_SIZE: std::cell::Cell<c_int> = const { std::cell::Cell::new(480) };
}

/// The docked DevTools' width (docked right) or height (docked bottom).
pub fn set_devtools_size(size: c_int) {
    DEVTOOLS_SIZE.set(size);
}

/// The width the layout gives the sidebar; animated by the controller.
pub fn set_sidebar_width(width: c_int) {
    SIDEBAR_CURRENT_WIDTH.set(width);
}

pub fn browser_settings(background: u32) -> BrowserSettings {
    BrowserSettings {
        background_color: background,
        ..Default::default()
    }
}

/// Creates a browser view for a tab or for one of Circle's own UI pages.
pub fn create_browser_view(role: Role, url: &str, background: u32) -> BrowserView {
    create_browser_view_in(role, url, background, None)
}

/// A browser view in a given request context (a private session), or the
/// shared one with `None`.
pub fn create_browser_view_in(
    role: Role,
    url: &str,
    background: u32,
    context: Option<&mut RequestContext>,
) -> BrowserView {
    let mut client = new_client(role);
    let mut delegate = CircleBrowserViewDelegate::new(role);
    browser_view_create(
        Some(&mut client),
        Some(&CefString::from(url)),
        Some(&browser_settings(background)),
        None,
        context,
        Some(&mut delegate),
    )
    .expect("failed to create browser view")
}

/// A fresh memory-only browsing session: cookies, cache and storage live
/// only as long as it does.
pub fn create_private_context() -> Option<RequestContext> {
    let settings = RequestContextSettings::default();
    let mut context = request_context_create_context(Some(&settings), None)?;
    crate::scheme::register_on(&mut context);
    Some(context)
}

pub fn create_main_window(
    sidebar: BrowserView,
    content: Panel,
    palette: BrowserView,
    peek: BrowserView,
) {
    let mut delegate = MainWindowDelegate::new(sidebar, content, palette, peek);
    window_create_top_level(Some(&mut delegate));
}

/// Opens Chrome's DevTools front-end (served from `url`) in its own window.
pub fn open_dev_tools_window(url: &str, page_title: &str) {
    let view = create_browser_view(Role::Popup, url, 0xFF20_2124);
    let title = if page_title.is_empty() {
        "Developer Tools".to_owned()
    } else {
        format!("Developer Tools – {page_title}")
    };
    let mut delegate = PopupWindowDelegate::new(view, (1100, 760), title, false);
    window_create_top_level(Some(&mut delegate));
}

pub fn app_icon() -> Option<Image> {
    let image = image_create()?;
    image.add_png(
        1.0,
        Some(include_bytes!("../resources/icons/circle-128.png")),
    );
    image.add_png(
        2.0,
        Some(include_bytes!("../resources/icons/circle-256.png")),
    );
    Some(image)
}

wrap_browser_view_delegate! {
    struct CircleBrowserViewDelegate {
        role: Role,
    }

    impl ViewDelegate {
        fn on_layout_changed(&self, _view: Option<&mut View>, _new_bounds: Option<&Rect>) {
            if let Role::Tab(_) | Role::DevTools(_) = self.role {
                controller::dispatch(Cmd::PageLayout);
            }
        }

        fn preferred_size(&self, _view: Option<&mut View>) -> Size {
            match self.role {
                // Box layouts read only the main axis, so one value serves both.
                Role::DevTools(_) => Size { width: DEVTOOLS_SIZE.get(), height: DEVTOOLS_SIZE.get() },
                Role::Sidebar => Size { width: SIDEBAR_CURRENT_WIDTH.get(), height: 600 },
                _ => Size { width: 600, height: 400 },
            }
        }
    }

    impl BrowserViewDelegate {
        /// CEF's C++ default returns the opener's delegate; the Rust default
        /// returns null, which crashes when DevTools or a popup opens.
        fn delegate_for_popup_browser_view(
            &self,
            _browser_view: Option<&mut BrowserView>,
            _settings: Option<&BrowserSettings>,
            _client: Option<&mut Client>,
            _is_devtools: c_int,
        ) -> Option<BrowserViewDelegate> {
            Some(CircleBrowserViewDelegate::new(Role::Popup))
        }

        fn chrome_toolbar_type(&self, _browser_view: Option<&mut BrowserView>) -> ChromeToolbarType {
            ChromeToolbarType::NONE
        }

        fn on_popup_browser_view_created(
            &self,
            _browser_view: Option<&mut BrowserView>,
            popup_browser_view: Option<&mut BrowserView>,
            is_devtools: c_int,
        ) -> c_int {
            let Some(popup) = popup_browser_view else {
                return 0;
            };
            // DevTools needs room; script popups (sign-in flows) stay small.
            let size = if is_devtools != 0 { (1100, 760) } else { (520, 680) };
            // A window opened from fap mode hides from screen capture too.
            let private = match self.role {
                Role::Tab(tab) => controller::tab_is_private(tab),
                _ => false,
            };
            let mut delegate = PopupWindowDelegate::new(popup.clone(), size, String::new(), private);
            window_create_top_level(Some(&mut delegate));
            1
        }

        fn browser_runtime_style(&self) -> RuntimeStyle {
            RuntimeStyle::ALLOY
        }
    }
}

const MIN_WIDTH: c_int = 640;
const MIN_HEIGHT: c_int = 420;

/// The main window's size when it opens: `--window-size=WIDTHxHEIGHT` or
/// `CIRCLE_WINDOW_SIZE=WIDTHxHEIGHT` (e.g. for recordings at an exact
/// shape), else 1360×860.
fn main_window_size() -> (c_int, c_int) {
    let arg = std::env::args().find_map(|a| a.strip_prefix("--window-size=").map(str::to_owned));
    arg.or_else(|| std::env::var("CIRCLE_WINDOW_SIZE").ok())
        .and_then(|s| parse_size(&s))
        .unwrap_or((1360, 860))
}

fn parse_size(text: &str) -> Option<(c_int, c_int)> {
    let (w, h) = text.trim().split_once(['x', 'X', ','])?;
    let (w, h): (c_int, c_int) = (w.trim().parse().ok()?, h.trim().parse().ok()?);
    Some((w.max(MIN_WIDTH), h.max(MIN_HEIGHT)))
}

wrap_window_delegate! {
    struct MainWindowDelegate {
        sidebar: BrowserView,
        content: Panel,
        palette: BrowserView,
        peek: BrowserView,
    }

    impl ViewDelegate {
        fn preferred_size(&self, _view: Option<&mut View>) -> Size {
            let (width, height) = main_window_size();
            Size { width, height }
        }

        fn minimum_size(&self, _view: Option<&mut View>) -> Size {
            Size { width: MIN_WIDTH, height: MIN_HEIGHT }
        }
    }

    impl PanelDelegate {}

    impl WindowDelegate {
        fn on_window_created(&self, window: Option<&mut Window>) {
            let Some(window) = window else {
                return;
            };
            window.set_title(Some(&CefString::from("Circle")));
            if let Some(mut icon) = app_icon() {
                window.set_window_app_icon(Some(&mut icon));
            }
            window.add_child_view(Some(&mut View::from(&self.sidebar)));
            window.add_child_view(Some(&mut View::from(&self.content)));
            // Overlays stack in order: the peeking sidebar, then the command bar.
            let peek =
                window.add_overlay_view(Some(&mut View::from(&self.peek)), DockingMode::CUSTOM, 1);
            let overlay =
                window.add_overlay_view(Some(&mut View::from(&self.palette)), DockingMode::CUSTOM, 1);
            for overlay in overlay.iter().chain(&peek) {
                overlay.set_visible(0);
            }
            controller::dispatch(Cmd::WindowCreated(window.clone(), overlay, peek));
            window.show();
        }

        fn on_window_destroyed(&self, _window: Option<&mut Window>) {
            controller::on_window_destroyed();
        }

        fn on_window_bounds_changed(&self, _window: Option<&mut Window>, _new_bounds: Option<&Rect>) {
            controller::dispatch(Cmd::WindowResized);
        }

        fn can_close(&self, _window: Option<&mut Window>) -> c_int {
            controller::can_close()
        }

        fn is_frameless(&self, _window: Option<&mut Window>) -> c_int {
            MAC as c_int
        }

        fn with_standard_window_buttons(&self, _window: Option<&mut Window>) -> c_int {
            1
        }

        fn titlebar_height(&self, _window: Option<&mut Window>, titlebar_height: Option<&mut f32>) -> c_int {
            match (MAC, titlebar_height) {
                (true, Some(height)) => {
                    *height = TITLEBAR_HEIGHT;
                    1
                }
                _ => 0,
            }
        }

        fn can_resize(&self, _window: Option<&mut Window>) -> c_int {
            1
        }

        fn can_maximize(&self, _window: Option<&mut Window>) -> c_int {
            1
        }

        fn can_minimize(&self, _window: Option<&mut Window>) -> c_int {
            1
        }

        fn initial_show_state(&self, _window: Option<&mut Window>) -> ShowState {
            ShowState::NORMAL
        }

        fn window_runtime_style(&self) -> RuntimeStyle {
            RuntimeStyle::ALLOY
        }
    }
}

wrap_window_delegate! {
    struct PopupWindowDelegate {
        browser_view: BrowserView,
        size: (c_int, c_int),
        title: String,
        private: bool,
    }

    impl ViewDelegate {
        fn preferred_size(&self, _view: Option<&mut View>) -> Size {
            Size { width: self.size.0, height: self.size.1 }
        }
    }

    impl PanelDelegate {}

    impl WindowDelegate {
        fn on_window_created(&self, window: Option<&mut Window>) {
            let Some(window) = window else {
                return;
            };
            if let Some(mut icon) = app_icon() {
                window.set_window_app_icon(Some(&mut icon));
            }
            if !self.title.is_empty() {
                window.set_title(Some(&CefString::from(self.title.as_str())));
            }
            window.add_child_view(Some(&mut View::from(&self.browser_view)));
            #[cfg(target_os = "macos")]
            if self.private {
                crate::mac::set_capture_hidden(window, true);
            }
            window.show();
            self.browser_view.request_focus();
        }

        fn can_close(&self, _window: Option<&mut Window>) -> c_int {
            match self.browser_view.browser().and_then(|b| b.host()) {
                Some(host) => host.try_close_browser(),
                None => 1,
            }
        }

        fn can_resize(&self, _window: Option<&mut Window>) -> c_int {
            1
        }

        fn can_maximize(&self, _window: Option<&mut Window>) -> c_int {
            1
        }

        fn can_minimize(&self, _window: Option<&mut Window>) -> c_int {
            1
        }

        fn initial_show_state(&self, _window: Option<&mut Window>) -> ShowState {
            ShowState::NORMAL
        }

        fn window_runtime_style(&self) -> RuntimeStyle {
            RuntimeStyle::ALLOY
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_window_sizes() {
        assert_eq!(parse_size("1280x800"), Some((1280, 800)));
        assert_eq!(parse_size(" 1440 X 900 "), Some((1440, 900)));
        assert_eq!(parse_size("1280,720"), Some((1280, 720)));
        assert_eq!(parse_size("100x100"), Some((MIN_WIDTH, MIN_HEIGHT)));
        assert_eq!(parse_size("wide"), None);
    }
}
