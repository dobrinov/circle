//! The `circle://ui/` scheme serves the browser's own UI (sidebar and command
//! bar) and receives commands from it via `fetch("circle://ui/api", ...)`.
//!
//! Requests arrive on the IO thread. Commands are only accepted from the UI
//! browsers themselves, so a web page cannot drive the browser.

use crate::controller::{self, Action, UiCmd};
use cef::*;
use std::{
    os::raw::c_int,
    sync::{Arc, Mutex},
};

pub const SCHEME: &str = "circle";
pub const HOST: &str = "ui";

static UI_BROWSERS: Mutex<Vec<c_int>> = Mutex::new(Vec::new());

pub fn allow_browser(id: c_int) {
    UI_BROWSERS.lock().unwrap().push(id);
}

pub fn forget_browser(id: c_int) {
    UI_BROWSERS.lock().unwrap().retain(|b| *b != id);
}

fn is_ui_browser(browser: Option<&mut Browser>) -> bool {
    browser.is_some_and(|b| UI_BROWSERS.lock().unwrap().contains(&b.identifier()))
}

/// Registers the scheme in every process. Must match on all processes.
pub fn register_custom_scheme(registrar: &mut SchemeRegistrar) {
    use sys::cef_scheme_options_t::*;
    let options = CEF_SCHEME_OPTION_STANDARD as c_int
        | CEF_SCHEME_OPTION_SECURE as c_int
        | CEF_SCHEME_OPTION_DISPLAY_ISOLATED as c_int
        | CEF_SCHEME_OPTION_FETCH_ENABLED as c_int;
    registrar.add_custom_scheme(Some(&CefString::from(SCHEME)), options);
}

/// Serves `circle://ui/` in a separate request context (private sessions)
/// too, which doesn't inherit the global registration.
pub fn register_on(context: &mut RequestContext) {
    let mut factory = UiSchemeFactory::new();
    context.register_scheme_handler_factory(
        Some(&CefString::from(SCHEME)),
        Some(&CefString::from(HOST)),
        Some(&mut factory),
    );
}

pub fn register_handler_factory() {
    let mut factory = UiSchemeFactory::new();
    register_scheme_handler_factory(
        Some(&CefString::from(SCHEME)),
        Some(&CefString::from(HOST)),
        Some(&mut factory),
    );
}

const ASSETS: &[(&str, &str, &[u8])] = &[
    (
        "/sidebar.html",
        "text/html",
        include_bytes!("../ui/sidebar.html"),
    ),
    (
        "/palette.html",
        "text/html",
        include_bytes!("../ui/palette.html"),
    ),
    (
        "/common.css",
        "text/css",
        include_bytes!("../ui/common.css"),
    ),
    (
        "/common.js",
        "text/javascript",
        include_bytes!("../ui/common.js"),
    ),
    (
        "/sidebar.js",
        "text/javascript",
        include_bytes!("../ui/sidebar.js"),
    ),
    (
        "/palette.js",
        "text/javascript",
        include_bytes!("../ui/palette.js"),
    ),
    (
        "/settings.html",
        "text/html",
        include_bytes!("../ui/settings.html"),
    ),
    (
        "/settings.js",
        "text/javascript",
        include_bytes!("../ui/settings.js"),
    ),
    (
        "/autofill.html",
        "text/html",
        include_bytes!("../ui/autofill.html"),
    ),
    (
        "/autofill.js",
        "text/javascript",
        include_bytes!("../ui/autofill.js"),
    ),
    (
        "/splitdrop.html",
        "text/html",
        include_bytes!("../ui/splitdrop.html"),
    ),
    (
        "/splitdrop.js",
        "text/javascript",
        include_bytes!("../ui/splitdrop.js"),
    ),
    (
        "/extcard.html",
        "text/html",
        include_bytes!("../ui/extcard.html"),
    ),
    (
        "/extcard.js",
        "text/javascript",
        include_bytes!("../ui/extcard.js"),
    ),
    (
        "/splitter.html",
        "text/html",
        include_bytes!("../ui/splitter.html"),
    ),
    (
        "/splitter.js",
        "text/javascript",
        include_bytes!("../ui/splitter.js"),
    ),
    (
        "/recbar.html",
        "text/html",
        include_bytes!("../ui/recbar.html"),
    ),
    (
        "/recbar.js",
        "text/javascript",
        include_bytes!("../ui/recbar.js"),
    ),
    (
        "/recorder.js",
        "text/javascript",
        include_bytes!("../ui/recorder.js"),
    ),
    (
        "/start.js",
        "text/javascript",
        include_bytes!("../ui/start.js"),
    ),
    (
        "/recorder.html",
        "text/html",
        include_bytes!("../ui/recorder.html"),
    ),
    (
        "/bugreport.html",
        "text/html",
        include_bytes!("../ui/bugreport.html"),
    ),
    (
        "/bugreport.js",
        "text/javascript",
        include_bytes!("../ui/bugreport.js"),
    ),
    (
        "/start.html",
        "text/html",
        include_bytes!("../ui/start.html"),
    ),
    (
        "/history.html",
        "text/html",
        include_bytes!("../ui/history.html"),
    ),
    (
        "/history.js",
        "text/javascript",
        include_bytes!("../ui/history.js"),
    ),
    (
        "/network.html",
        "text/html",
        include_bytes!("../ui/network.html"),
    ),
    (
        "/network.js",
        "text/javascript",
        include_bytes!("../ui/network.js"),
    ),
    (
        "/icon.png",
        "image/png",
        include_bytes!("../resources/icons/circle-128.png"),
    ),
];

fn request_body(request: &mut Request) -> Vec<u8> {
    let Some(post) = request.post_data() else {
        return Vec::new();
    };
    let mut elements = vec![None; post.element_count()];
    post.elements(Some(&mut elements));
    let mut body = Vec::new();
    for element in elements.into_iter().flatten() {
        let len = element.bytes_count();
        let start = body.len();
        body.resize(start + len, 0);
        element.bytes(len, body[start..].as_mut_ptr());
    }
    body
}

/// Circle's pages that open in ordinary tabs (exactly these, with an
/// optional query or fragment).
fn is_tab_page(url: &str) -> bool {
    ["history.html", "settings.html", "bugreport.html"]
        .iter()
        .any(|page| {
            url.strip_prefix("circle://ui/")
                .and_then(|rest| rest.strip_prefix(page))
                .is_some_and(|rest| rest.is_empty() || rest.starts_with(['?', '#']))
        })
}

/// What Circle's pages may load and run: only their own scripts, so an
/// escaping slip in a page can't become script execution. Images come
/// from anywhere (site icons).
const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; \
    style-src 'self' 'unsafe-inline'; img-src * data: blob:; media-src 'self' blob: data:; \
    connect-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'; \
    frame-ancestors 'self'";

fn icon_mime(path: &std::path::Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "bmp" => "image/bmp",
        // Served with a policy that blocks script (see `response_headers`).
        "svg" => "image/svg+xml",
        _ => return None,
    })
}

fn response(status: c_int, mime: &'static str, data: Vec<u8>) -> Option<ResourceHandler> {
    Some(BytesResource::new(
        Arc::new(data),
        mime,
        status,
        Arc::new(Mutex::new(0)),
    ))
}

wrap_scheme_handler_factory! {
    struct UiSchemeFactory;

    impl SchemeHandlerFactory {
        fn create(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            _scheme_name: Option<&CefString>,
            request: Option<&mut Request>,
        ) -> Option<ResourceHandler> {
            let request = request?;
            let url = CefString::from(&request.url()).to_string();
            let path = url::Url::parse(&url).ok()?.path().to_owned();

            // Circle's own pages may use the API: the sidebar and command bar
            // (by browser), and the history, settings and bug report pages
            // while one is the top-level page of its tab. Websites can't
            // load or embed circle:// pages.
            let is_internal_page = frame.is_some_and(|f| {
                f.is_main() != 0 && is_tab_page(&CefString::from(&f.url()).to_string())
            });
            let ui_browser = is_ui_browser(browser);
            let trusted = is_internal_page || ui_browser;

            if path == "/api/history" {
                if !trusted {
                    return response(403, "text/plain", b"forbidden".to_vec());
                }
                let query: std::collections::HashMap<String, String> = url::Url::parse(&url)
                    .map(|u| u.query_pairs().into_owned().collect())
                    .unwrap_or_default();
                let q = query.get("q").map(String::as_str).unwrap_or("");
                let before = query.get("before").and_then(|b| b.parse().ok());
                let limit = query
                    .get("limit")
                    .and_then(|l| l.parse().ok())
                    .unwrap_or(200usize)
                    .min(1000);
                let visits = crate::history::search(q, before, limit);
                let body = serde_json::to_vec(&serde_json::json!({ "visits": visits }))
                    .unwrap_or_default();
                return response(200, "application/json", body);
            }

            if path == "/api/recording" {
                if !ui_browser {
                    return response(403, "text/plain", b"forbidden".to_vec());
                }
                let query: std::collections::HashMap<String, String> = url::Url::parse(&url)
                    .map(|u| u.query_pairs().into_owned().collect())
                    .unwrap_or_default();
                let field = |k: &str| query.get(k).map(String::as_str).unwrap_or("");
                return match crate::recorder::handle_upload(field("op"), field("ext"), &request_body(request)) {
                    Ok(()) => response(200, "application/json", b"{}".to_vec()),
                    Err(err) => response(500, "text/plain", err.into_bytes()),
                };
            }

            if path == "/api" {
                if !trusted {
                    return response(403, "text/plain", b"forbidden".to_vec());
                }
                let body = request_body(request);
                return match serde_json::from_slice::<UiCmd>(&body) {
                    // The history page is an ordinary tab, so it may only
                    // manage history.
                    Ok(cmd)
                        if !ui_browser
                            && !matches!(
                                cmd,
                                UiCmd::DeleteHistory { .. }
                                    | UiCmd::ClearBrowsingData { .. }
                                    | UiCmd::Settings { .. }
                                    | UiCmd::SettingsReady
                                    | UiCmd::ExtEnable { .. }
                                    | UiCmd::ExtPin { .. }
                                    | UiCmd::ExtOptions { .. }
                                    | UiCmd::ExtRemove { .. }
                                    | UiCmd::ExtLoadUnpacked
                                    | UiCmd::ExtStore
                                    | UiCmd::BugReportStart { .. }
                                    | UiCmd::BugReportStop
                                    | UiCmd::BugReportReady
                                    | UiCmd::Navigate { new_tab: true, .. }
                                    | UiCmd::Action {
                                        action: Action::ShowHistory
                                            | Action::ClearBrowsingData
                                            | Action::ReportBug
                                    }
                            ) =>
                    {
                        response(403, "text/plain", b"forbidden".to_vec())
                    }
                    Ok(cmd) => {
                        controller::dispatch_from_any_thread(cmd);
                        response(200, "application/json", b"{}".to_vec())
                    }
                    Err(err) => {
                        eprintln!("circle: bad UI command: {err}");
                        response(400, "text/plain", err.to_string().into_bytes())
                    }
                };
            }

            // An extension's icon, for its button and in Settings.
            if let Some(id) = path.strip_prefix("/ext-icon/") {
                return match crate::extensions::icon_file(id).and_then(|f| {
                    let mime = icon_mime(&f)?;
                    let data = std::fs::read(&f).ok().filter(|d| d.len() <= 1 << 20)?;
                    Some((mime, data))
                }) {
                    Some((mime, data)) => response(200, mime, data),
                    None => response(404, "text/plain", b"not found".to_vec()),
                };
            }

            match ASSETS.iter().find(|(p, _, _)| *p == path) {
                Some((_, mime, data)) => response(200, mime, data.to_vec()),
                None => response(404, "text/plain", b"not found".to_vec()),
            }
        }
    }
}

wrap_resource_handler! {
    struct BytesResource {
        data: Arc<Vec<u8>>,
        mime: &'static str,
        status: c_int,
        offset: Arc<Mutex<usize>>,
    }

    impl ResourceHandler {
        fn open(
            &self,
            _request: Option<&mut Request>,
            handle_request: Option<&mut c_int>,
            _callback: Option<&mut Callback>,
        ) -> c_int {
            if let Some(handle_request) = handle_request {
                *handle_request = 1;
            }
            1
        }

        fn response_headers(
            &self,
            response: Option<&mut Response>,
            response_length: Option<&mut i64>,
            _redirect_url: Option<&mut CefString>,
        ) {
            if let Some(response) = response {
                response.set_status(self.status);
                response.set_mime_type(Some(&CefString::from(self.mime)));
                response.set_header_by_name(
                    Some(&CefString::from("Cache-Control")),
                    Some(&CefString::from("no-store")),
                    1,
                );
                // An extension's SVG icon, opened on its own, runs nothing.
                let policy = match self.mime {
                    "text/html" => Some(CONTENT_SECURITY_POLICY),
                    "image/svg+xml" => Some("default-src 'none'; style-src 'unsafe-inline'; sandbox"),
                    _ => None,
                };
                if let Some(policy) = policy {
                    response.set_header_by_name(
                        Some(&CefString::from("Content-Security-Policy")),
                        Some(&CefString::from(policy)),
                        1,
                    );
                }
            }
            if let Some(length) = response_length {
                *length = self.data.len() as i64;
            }
        }

        fn read(
            &self,
            data_out: *mut u8,
            bytes_to_read: c_int,
            bytes_read: Option<&mut c_int>,
            _callback: Option<&mut ResourceReadCallback>,
        ) -> c_int {
            let mut offset = self.offset.lock().unwrap();
            let remaining = &self.data[*offset..];
            let count = remaining.len().min(bytes_to_read.max(0) as usize);
            if let Some(bytes_read) = bytes_read {
                *bytes_read = count as c_int;
            }
            if count == 0 {
                return 0;
            }
            unsafe { std::ptr::copy_nonoverlapping(remaining.as_ptr(), data_out, count) };
            *offset += count;
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_exact_internal_pages_are_trusted() {
        for url in [
            "circle://ui/history.html",
            "circle://ui/history.html?clear",
            "circle://ui/settings.html#updates",
            "circle://ui/bugreport.html",
        ] {
            assert!(is_tab_page(url), "{url}");
        }
        for url in [
            "circle://ui/history.htmlx",
            "circle://ui/settings.html.evil",
            "circle://ui/sidebar.html",
            "https://circle/ui/settings.html",
            "circle://uix/settings.html",
        ] {
            assert!(!is_tab_page(url), "{url}");
        }
    }
}
