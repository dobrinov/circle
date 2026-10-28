//! Chrome extensions in Circle: loading them into Chromium, the card that
//! installs them from Chrome Web Store pages, their buttons and popups next
//! to the address bar, and the Extensions settings.
//!
//! Chromium can add an extension at any moment but can only take one away
//! by uninstalling it, which deletes its data. So turning an extension off
//! (or updating it) takes effect when Circle next starts.

use super::*;
use crate::engine;
use crate::extensions::{self, BRIDGE_ID, Extension, Manifest};
use crate::remote;

const CARD_URL: &str = "circle://ui/extcard.html";
const POPUP_SHIM: &str = include_str!("../../ui/ext-popup-shim.js");
/// Chrome's limits for an extension popup.
const POPUP_MIN: (c_int, c_int) = (25, 25);
const POPUP_MAX: (c_int, c_int) = (800, 600);
/// How long the window waits for extensions to load at startup, so that
/// restored pages get their content scripts (and ad blocking).
pub(super) const STARTUP_WAIT_MS: i64 = 4000;
pub(super) const UPDATE_EVERY_MS: i64 = 24 * 60 * 60 * 1000;

/// What the card over a Chrome Web Store page shows.
pub(super) struct Offer {
    tab: TabId,
    id: String,
    name: String,
    stage: Stage,
    /// Closed by the person; stays closed while they're on this page.
    dismissed: bool,
}

enum Stage {
    Available,
    Fetching,
    /// Downloaded and verified; waiting for the person to confirm.
    Confirm(PathBuf, Box<Manifest>),
    Added,
    Installed,
    Failed(String),
}

/// Why an extension is being loaded into Chromium.
#[derive(Clone, Debug)]
pub enum LoadPurpose {
    Startup,
    /// Turned on again, or just installed from the store.
    Enable,
    /// A folder the person chose to load unpacked.
    Unpacked,
}

pub(super) struct Popup {
    id: String,
    view: BrowserView,
    overlay: Option<OverlayController>,
    /// The button it belongs to, in window coordinates.
    anchor: Rect,
    size: Option<(c_int, c_int)>,
    url: String,
    /// The tab it was opened over, for the popup's `chrome.tabs`.
    tab: Value,
}

impl Controller {
    /// Loads the enabled extensions into Chromium. Returns whether there is
    /// anything to wait for before showing the window.
    pub(super) fn begin_loading_extensions(&mut self) -> bool {
        self.ext.tidy();
        if !engine::available() {
            return false;
        }
        let enabled: Vec<Extension> = self
            .ext
            .items
            .iter()
            .filter(|e| e.enabled)
            .cloned()
            .collect();
        if enabled.is_empty() {
            return false;
        }
        self.load_bridge();
        for extension in enabled {
            self.ext_waiting += 1;
            load(
                extension.path.clone(),
                Some(extension.id.clone()),
                LoadPurpose::Startup,
            );
        }
        true
    }

    /// Circle's helper extension, loaded alongside the first real one.
    fn load_bridge(&mut self) {
        if self.ext_loaded.contains_key(BRIDGE_ID) || !engine::available() {
            return;
        }
        match self.ext.write_bridge() {
            Ok(path) => {
                self.ext_waiting += 1;
                load(path, Some(BRIDGE_ID.to_owned()), LoadPurpose::Startup);
            }
            Err(err) => eprintln!("circle: couldn't write the extension bridge: {err}"),
        }
    }

    pub(super) fn extension_loaded(
        &mut self,
        path: PathBuf,
        expected: Option<String>,
        purpose: LoadPurpose,
        result: Result<String, String>,
    ) {
        if matches!(purpose, LoadPurpose::Startup) {
            self.ext_waiting = self.ext_waiting.saturating_sub(1);
        }
        match result {
            Ok(id) => {
                if let Some(expected) = expected.as_ref().filter(|e| **e != id) {
                    eprintln!("circle: extension {expected} loaded as {id}");
                }
                self.ext_failures.remove(&id);
                self.ext_loaded.insert(id.clone(), path.clone());
                if matches!(purpose, LoadPurpose::Unpacked) {
                    let manifest = Manifest::read(&path).unwrap_or_default();
                    self.ext.put(Extension {
                        id: id.clone(),
                        version: manifest.version,
                        path,
                        from_store: false,
                        enabled: true,
                        pinned: manifest.popup.is_some(),
                        installed: now(),
                    });
                    if let Some(e) = self.ext.get_mut(&id) {
                        e.enabled = true;
                    }
                    self.save_extensions();
                    self.load_bridge();
                    self.toast(format!("Loaded “{}”", manifest.name));
                }
            }
            Err(err) => {
                let key = expected.unwrap_or_else(|| path.display().to_string());
                eprintln!("circle: couldn't load extension {key}: {err}");
                if matches!(purpose, LoadPurpose::Unpacked) {
                    self.toast(format!("Couldn't load the extension: {err}"));
                }
                if let Some(offer) = self.ext_offer.as_mut().filter(|o| o.id == key) {
                    offer.stage = Stage::Failed(err.clone());
                    self.push_ext_card();
                }
                self.ext_failures.insert(key, err);
            }
        }
        if self.ext_waiting == 0 {
            self.show_window_once();
        }
        self.ui_dirty = true;
        self.push_settings();
    }

    /// Creates the main window, once (after extensions loaded, or gave up).
    pub(super) fn show_window_once(&mut self) {
        if self.window_requested {
            return;
        }
        self.window_requested = true;
        views::create_main_window(
            self.sidebar.clone(),
            self.content.clone(),
            self.palette.clone(),
            self.peek.clone(),
        );
    }

    fn save_extensions(&mut self) {
        if let Err(err) = self.ext.save() {
            self.toast(format!("Couldn't save the extension list: {err}"));
        }
    }

    // ----- the toolbar and popups -------------------------------------------

    /// Extension buttons for the sidebar: those running, minus Circle's own.
    pub(super) fn extensions_json(&self) -> Value {
        if self.session.space().private {
            return json!([]);
        }
        let list: Vec<Value> = self
            .ext
            .items
            .iter()
            .filter(|e| e.enabled && self.ext_loaded.contains_key(&e.id))
            .map(|e| {
                let manifest = Manifest::read(&e.path).unwrap_or_default();
                json!({
                    "id": e.id,
                    "name": manifest.name,
                    "title": manifest.action_title.unwrap_or_else(|| manifest.name.clone()),
                    "pinned": e.pinned,
                    "popup": manifest.popup.is_some(),
                    "open": self.ext_popup.as_ref().is_some_and(|p| p.id == e.id),
                })
            })
            .collect();
        Value::Array(list)
    }

    /// Opens (or, if it's open, closes) an extension's popup under its
    /// button, at `rect` (CSS px) in the sidebar or the peek sidebar.
    pub(super) fn open_extension_popup(&mut self, id: String, rect: Rect, from_peek: bool) {
        // Clicking the button of the open popup closes it. The click moves
        // focus first, which may have closed it a moment ago.
        let just_closed = self.ext_popup_closed.as_ref().is_some_and(|(closed, at)| {
            *closed == id && at.elapsed() < std::time::Duration::from_millis(400)
        });
        let was_open = self.ext_popup.as_ref().is_some_and(|p| p.id == id);
        self.close_extension_popup();
        if was_open || just_closed {
            self.ext_popup_closed = None;
            return;
        }
        let Some(extension) = self.ext.get(&id).filter(|e| e.enabled).cloned() else {
            return;
        };
        let manifest = Manifest::read(&extension.path).unwrap_or_default();
        let Some(popup) = manifest.popup else {
            self.toast(format!(
                "“{}” works on its own; it has no popup",
                manifest.name
            ));
            return;
        };
        let Some(window) = &self.window else {
            return;
        };
        let source = if from_peek { &self.peek } else { &self.sidebar };
        let mut origin = Point { x: 0, y: 0 };
        source.convert_point_to_window(Some(&mut origin));
        let anchor = Rect {
            x: origin.x + rect.x,
            y: origin.y + rect.y,
            width: rect.width,
            height: rect.height,
        };
        let view = views::create_browser_view(Role::ExtPopup, "about:blank", 0xFFFF_FFFF);
        let overlay = window.add_overlay_view(Some(&mut View::from(&view)), DockingMode::CUSTOM, 1);
        if let Some(overlay) = &overlay {
            overlay.set_visible(0);
        }
        self.ext_popup = Some(Popup {
            id: id.clone(),
            view,
            overlay,
            anchor,
            size: None,
            url: format!("chrome-extension://{id}/{popup}"),
            tab: self.popup_tab_json(),
        });
        self.ui_dirty = true;
    }

    /// The tab a popup is opened over, as `chrome.tabs` describes tabs.
    fn popup_tab_json(&self) -> Value {
        let active = self.session.active_tab();
        let tab = active.and_then(|id| self.session.tab(id));
        let area = self.content.bounds();
        json!({
            "id": active.and_then(|id| self.ext_tab_ids.get(&id)).copied().unwrap_or(-1),
            "url": tab.map(|t| t.url.clone()).unwrap_or_default(),
            "title": tab.map(|t| t.display_title().to_owned()).unwrap_or_default(),
            "favIconUrl": tab.and_then(|t| t.favicon.clone()),
            "width": area.width,
            "height": area.height,
        })
    }

    /// The popup's browser exists: teach it about Circle's tabs, then load
    /// the extension's page.
    pub(super) fn extension_popup_created(&mut self, browser: Browser) {
        let Some(popup) = &self.ext_popup else {
            return;
        };
        if popup.view.browser().map(|b| b.identifier()) != Some(browser.identifier()) {
            return;
        }
        if let Some(host) = browser.host() {
            let min = Size {
                width: POPUP_MIN.0,
                height: POPUP_MIN.1,
            };
            let max = Size {
                width: POPUP_MAX.0,
                height: POPUP_MAX.1,
            };
            host.set_auto_resize_enabled(1, Some(&min), Some(&max));
        }
        let source = format!("({POPUP_SHIM})({});", popup.tab);
        let url = popup.url.clone();
        let target = browser.clone();
        // Scripts for new documents only run with the Page domain on. That
        // would be visible to a website, but this is the extension's page.
        remote::command(&browser, "Page.enable", json!({}), move |_| {
            let again = target.clone();
            remote::command(
                &target,
                "Page.addScriptToEvaluateOnNewDocument",
                json!({ "source": source }),
                move |_| {
                    if let Some(frame) = again.main_frame() {
                        frame.load_url(Some(&CefString::from(url.as_str())));
                    }
                },
            );
        });
    }

    pub(super) fn extension_popup_resized(
        &mut self,
        browser_id: c_int,
        width: c_int,
        height: c_int,
    ) {
        let Some(popup) = &mut self.ext_popup else {
            return;
        };
        if popup.view.browser().map(|b| b.identifier()) != Some(browser_id) {
            return;
        }
        popup.size = Some((
            width.clamp(POPUP_MIN.0, POPUP_MAX.0),
            height.clamp(POPUP_MIN.1, POPUP_MAX.1),
        ));
        self.place_extension_popup();
        if let Some(popup) = &self.ext_popup {
            popup.view.request_focus();
        }
    }

    pub(super) fn place_extension_popup(&self) {
        let (Some(popup), Some(window)) = (&self.ext_popup, &self.window) else {
            return;
        };
        let (Some(overlay), Some((width, height))) = (&popup.overlay, popup.size) else {
            return;
        };
        let bounds = window.bounds();
        let margin = GAP;
        let x = popup
            .anchor
            .x
            .min(bounds.width - width - margin)
            .max(margin);
        let below = popup.anchor.y + popup.anchor.height + 6;
        let y = below.min(bounds.height - height - margin).max(margin);
        overlay.set_bounds(Some(&Rect {
            x,
            y,
            width,
            height,
        }));
        overlay.set_visible(1);
        #[cfg(target_os = "macos")]
        if let Some(browser) = popup.view.browser() {
            crate::mac::round_browser_window(&browser, 10.0, crate::mac::ALL_CORNERS);
        }
    }

    pub(super) fn close_extension_popup(&mut self) {
        let Some(popup) = self.ext_popup.take() else {
            return;
        };
        self.ext_popup_closed = Some((popup.id.clone(), std::time::Instant::now()));
        if let Some(overlay) = &popup.overlay {
            overlay.set_visible(0);
            overlay.destroy();
        }
        if let Some(host) = popup.view.browser().and_then(|b| b.host()) {
            host.close_browser(1);
        }
        self.ui_dirty = true;
    }

    /// The popup closed itself (`window.close()`).
    pub(super) fn extension_popup_closed(&mut self, browser_id: c_int) {
        if self.ext_popup.as_ref().is_some_and(|p| {
            p.view
                .browser()
                .is_none_or(|b| b.identifier() == browser_id)
        }) {
            if let Some(popup) = self.ext_popup.take()
                && let Some(overlay) = &popup.overlay
            {
                overlay.set_visible(0);
                overlay.destroy();
            }
            self.ui_dirty = true;
        }
    }

    /// The bridge reported a page's Chrome tab id.
    pub(super) fn extension_tab_id(&mut self, tab: TabId, chrome_id: i64) {
        self.ext_tab_ids.insert(tab, chrome_id);
    }

    // ----- the Chrome Web Store card ----------------------------------------

    /// Shows the card when the tab showing is a store page for an extension.
    pub(super) fn ext_follow_tab(&mut self) {
        let active = self.session.active_tab();
        let store = active
            .filter(|id| !self.session.is_private_tab(*id) && engine::available())
            .and_then(|id| {
                let tab = self.session.tab(id)?;
                let ext = extensions::store_id(&tab.url)?;
                let name = tab
                    .title
                    .trim_end_matches(" - Chrome Web Store")
                    .trim()
                    .to_owned();
                Some((id, ext, name))
            });
        match (store, &mut self.ext_offer) {
            (Some((tab, id, name)), Some(offer)) if offer.tab == tab && offer.id == id => {
                if matches!(offer.stage, Stage::Available)
                    && !name.is_empty()
                    && !name.contains("chromewebstore")
                {
                    offer.name = name;
                }
            }
            (Some((tab, id, name)), _) => {
                self.drop_offer();
                let stage = if self.ext.get(&id).is_some() {
                    Stage::Installed
                } else {
                    Stage::Available
                };
                let name = match self.ext.get(&id) {
                    Some(e) => Manifest::read(&e.path).map(|m| m.name).unwrap_or(name),
                    None => name,
                };
                self.ext_offer = Some(Offer {
                    tab,
                    id,
                    name,
                    stage,
                    dismissed: false,
                });
            }
            (None, _) => self.drop_offer(),
        }
        self.push_ext_card();
        self.place_ext_card();
    }

    /// Forgets the card, cleaning up a download nobody confirmed.
    fn drop_offer(&mut self) {
        if let Some(Offer {
            id,
            stage: Stage::Confirm(path, _),
            ..
        }) = self.ext_offer.take()
        {
            self.discard_download(&id, &path);
        }
    }

    fn discard_download(&self, id: &str, path: &Path) {
        let in_use = self.ext.get(id).is_some_and(|e| e.path == path)
            || self.ext_loaded.get(id).is_some_and(|p| p == path);
        if !in_use && path.starts_with(self.ext.dir()) {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    pub(super) fn ext_card_command(&mut self, cmd: &str) {
        match cmd {
            "add" => {
                let Some(offer) = &mut self.ext_offer else {
                    return;
                };
                offer.stage = Stage::Fetching;
                let id = offer.id.clone();
                let dir = self.ext.dir().to_path_buf();
                std::thread::spawn(move || {
                    let result = extensions::fetch_from_store(&id, &dir);
                    remote::run_on_ui(move || dispatch(Cmd::ExtFetched { id, result }));
                });
            }
            "confirm" => {
                let Some(offer) = &mut self.ext_offer else {
                    return;
                };
                let Stage::Confirm(path, manifest) =
                    std::mem::replace(&mut offer.stage, Stage::Added)
                else {
                    return;
                };
                offer.name = manifest.name.clone();
                let id = offer.id.clone();
                let existing = self.ext.get(&id).cloned();
                self.ext.put(Extension {
                    id: id.clone(),
                    version: manifest.version.clone(),
                    path: path.clone(),
                    from_store: true,
                    enabled: true,
                    pinned: existing.as_ref().is_none_or(|e| e.pinned),
                    installed: existing.as_ref().map_or_else(now, |e| e.installed),
                });
                if let Some(e) = self.ext.get_mut(&id) {
                    e.enabled = true;
                }
                self.save_extensions();
                if !self.ext_loaded.contains_key(&id) {
                    self.load_bridge();
                    load(path, Some(id), LoadPurpose::Enable);
                }
            }
            "cancel" => {
                if let Some(offer) = &mut self.ext_offer
                    && let Stage::Confirm(path, _) =
                        std::mem::replace(&mut offer.stage, Stage::Available)
                {
                    let id = offer.id.clone();
                    self.discard_download(&id, &path);
                }
            }
            "manage" => {
                self.hide_ext_card();
                self.show_settings_section("extensions");
                return;
            }
            "close" => {
                self.hide_ext_card();
                return;
            }
            _ => return,
        }
        self.push_ext_card();
        self.place_ext_card();
        self.ui_dirty = true;
        self.push_settings();
    }

    pub(super) fn extension_fetched(
        &mut self,
        id: String,
        result: Result<(PathBuf, Manifest), String>,
    ) {
        let Some(offer) = self
            .ext_offer
            .as_mut()
            .filter(|o| o.id == id && matches!(o.stage, Stage::Fetching))
        else {
            // The card went away meanwhile.
            if let Ok((path, _)) = result {
                self.discard_download(&id, &path);
            }
            return;
        };
        offer.stage = match result {
            Ok((path, manifest)) => {
                offer.name = manifest.name.clone();
                if let Some(icon) = &manifest.icon {
                    extensions::publish_icon(&id, path.join(icon));
                }
                Stage::Confirm(path, Box::new(manifest))
            }
            Err(err) => Stage::Failed(err),
        };
        self.push_ext_card();
        self.place_ext_card();
    }

    fn hide_ext_card(&mut self) {
        // Closing the card only dismisses it; the page can bring it back.
        if let Some(offer) = &mut self.ext_offer {
            if let Stage::Confirm(path, _) = std::mem::replace(&mut offer.stage, Stage::Available) {
                let id = offer.id.clone();
                self.discard_download(&id, &path);
            }
            if let Some(offer) = &mut self.ext_offer {
                offer.dismissed = true;
            }
        }
        if let Some((_, Some(overlay))) = &self.ext_card {
            overlay.set_visible(0);
        }
    }

    fn ext_card_shown(&self) -> bool {
        self.ext_offer
            .as_ref()
            .is_some_and(|o| !o.dismissed && Some(o.tab) == self.session.active_tab())
    }

    pub(super) fn push_ext_card(&mut self) {
        let shown = self.ext_card_shown();
        if shown && self.ext_card.is_none() {
            let Some(window) = &self.window else {
                return;
            };
            let view = views::create_browser_view(Role::ExtCard, CARD_URL, self.background());
            let overlay =
                window.add_overlay_view(Some(&mut View::from(&view)), DockingMode::CUSTOM, 1);
            if let Some(overlay) = &overlay {
                overlay.set_visible(0);
            }
            self.ext_card = Some((view, overlay));
        }
        let (Some((view, _)), true) = (&self.ext_card, self.ext_card_ready) else {
            return;
        };
        let state = match self.ext_offer.as_ref().filter(|_| shown) {
            None => json!({ "stage": "none" }),
            Some(offer) => {
                let mut state = json!({ "id": offer.id, "name": offer.name });
                match &offer.stage {
                    Stage::Available => state["stage"] = json!("available"),
                    Stage::Fetching => state["stage"] = json!("fetching"),
                    Stage::Confirm(_, manifest) => {
                        state["stage"] = json!("confirm");
                        state["version"] = json!(manifest.version);
                        state["warnings"] = json!(manifest.warnings);
                    }
                    Stage::Added => {
                        state["stage"] = json!("added");
                        state["pinned"] = json!(self.ext.get(&offer.id).is_some_and(|e| e.pinned));
                    }
                    Stage::Installed => {
                        state["stage"] = json!("installed");
                        state["enabled"] =
                            json!(self.ext.get(&offer.id).is_some_and(|e| e.enabled));
                    }
                    Stage::Failed(err) => {
                        state["stage"] = json!("failed");
                        state["error"] = json!(err);
                    }
                }
                state
            }
        };
        let mut state = state;
        state["hue"] = json!(self.session.space().hue);
        run_js(view, &format!("window.circle&&circle.render({state})"));
    }

    pub(super) fn place_ext_card(&self) {
        let Some((_, Some(overlay))) = &self.ext_card else {
            return;
        };
        let covered = self
            .palette_mode
            .is_some_and(|mode| mode != PaletteMode::Find);
        if !self.ext_card_shown() || covered {
            overlay.set_visible(0);
            return;
        }
        let (width, height) = self.ext_card_size;
        let area = self.content.bounds();
        overlay.set_bounds(Some(&Rect {
            x: area.x + area.width - width - 2 * GAP,
            y: area.y + 2 * GAP,
            width,
            height,
        }));
        overlay.set_visible(1);
        #[cfg(target_os = "macos")]
        if let Some((view, _)) = &self.ext_card
            && let Some(browser) = view.browser()
        {
            crate::mac::round_browser_window(&browser, 10.0, crate::mac::ALL_CORNERS);
        }
    }

    // ----- settings ----------------------------------------------------------

    pub(super) fn extensions_settings_json(&self) -> Value {
        let list: Vec<Value> = self
            .ext
            .items
            .iter()
            .map(|e| {
                let manifest = Manifest::read(&e.path);
                let error = manifest
                    .as_ref()
                    .err()
                    .cloned()
                    .or_else(|| self.ext_failures.get(&e.id).cloned());
                let manifest = manifest.unwrap_or_default();
                let loaded = self.ext_loaded.get(&e.id);
                json!({
                    "id": e.id,
                    "name": if manifest.name.is_empty() { e.id.clone() } else { manifest.name.clone() },
                    "description": manifest.description,
                    "version": manifest.version,
                    "from_store": e.from_store,
                    "path": (!e.from_store).then(|| e.path.display().to_string()),
                    "enabled": e.enabled,
                    "pinned": e.pinned,
                    "running": loaded.is_some(),
                    "options": manifest.options.is_some(),
                    "warnings": manifest.warnings,
                    "error": error,
                    // Takes effect when Circle restarts: turned off while
                    // running, or updated.
                    "restart": loaded.is_some_and(|p| !e.enabled || *p != e.path),
                })
            })
            .collect();
        json!({
            "available": engine::available(),
            "list": list,
            "restart": self.extensions_need_restart(),
        })
    }

    fn extensions_need_restart(&self) -> bool {
        self.ext.items.iter().any(|e| {
            self.ext_loaded
                .get(&e.id)
                .is_some_and(|p| !e.enabled || *p != e.path)
        })
    }

    pub(super) fn set_extension_enabled(&mut self, id: &str, on: bool) {
        let Some(extension) = self.ext.get_mut(id) else {
            return;
        };
        extension.enabled = on;
        let path = extension.path.clone();
        self.save_extensions();
        if on && !self.ext_loaded.contains_key(id) {
            self.load_bridge();
            load(path, Some(id.to_owned()), LoadPurpose::Enable);
        }
        if !on && self.ext_popup.as_ref().is_some_and(|p| p.id == id) {
            self.close_extension_popup();
        }
        self.ui_dirty = true;
        self.push_settings();
    }

    pub(super) fn set_extension_pinned(&mut self, id: &str, pinned: bool) {
        if let Some(extension) = self.ext.get_mut(id) {
            extension.pinned = pinned;
            self.save_extensions();
        }
        self.ui_dirty = true;
        self.push_settings();
    }

    pub(super) fn remove_extension(&mut self, id: String) {
        if self.ext.get(&id).is_none() {
            return;
        }
        if self.ext_popup.as_ref().is_some_and(|p| p.id == id) {
            self.close_extension_popup();
        }
        if self.ext_loaded.contains_key(&id) {
            // Uninstalling also deletes what the extension stored.
            let target = id.clone();
            engine::call("Extensions.uninstall", json!({ "id": id }), move |result| {
                dispatch(Cmd::ExtUninstalled {
                    id: target,
                    result: result.map(|_| ()),
                });
            });
        } else {
            self.extension_uninstalled(id, Ok(()));
        }
    }

    pub(super) fn extension_uninstalled(&mut self, id: String, result: Result<(), String>) {
        if let Err(err) = result {
            self.toast(format!("Couldn't remove the extension: {err}"));
            return;
        }
        self.ext_loaded.remove(&id);
        self.ext_failures.remove(&id);
        if let Some(removed) = self.ext.remove(&id) {
            let name = Manifest::read(&removed.path)
                .map(|m| m.name)
                .unwrap_or_else(|_| "the extension".into());
            self.toast(format!("Removed “{name}”"));
        }
        self.save_extensions();
        if self.ext_offer.as_ref().is_some_and(|o| o.id == id)
            && let Some(offer) = &mut self.ext_offer
        {
            offer.stage = Stage::Available;
        }
        self.push_ext_card();
        self.ui_dirty = true;
        self.push_settings();
    }

    pub(super) fn open_extension_options(&mut self, id: &str) {
        let Some(extension) = self.ext.get(id) else {
            return;
        };
        if let Some(page) = Manifest::read(&extension.path).ok().and_then(|m| m.options) {
            self.open_tab(&format!("chrome-extension://{id}/{page}"), true, None);
        }
    }

    /// Asks for a folder with an unpacked extension (for developers).
    pub(super) fn choose_unpacked_extension(&mut self) {
        let host = self
            .session
            .active_tab()
            .and_then(|id| self.host(id))
            .or_else(|| self.sidebar.browser().and_then(|b| b.host()));
        let Some(host) = host else {
            return;
        };
        let mut callback = UnpackedChosen::new();
        host.run_file_dialog(
            FileDialogMode::OPEN_FOLDER,
            Some(&CefString::from("Choose an extension folder")),
            None,
            None,
            Some(&mut callback),
        );
    }

    pub(super) fn load_unpacked_extension(&mut self, path: PathBuf) {
        match Manifest::read(&path).and_then(|m| extensions::check_supported(&m).map(|_| m)) {
            Ok(_) => {
                self.load_bridge();
                load(path, None, LoadPurpose::Unpacked);
            }
            Err(err) => self.toast(err),
        }
    }

    // ----- updates -----------------------------------------------------------

    /// Looks for newer versions of store extensions (in the background).
    pub(super) fn check_extension_updates(&self) {
        let store: Vec<Extension> = self
            .ext
            .items
            .iter()
            .filter(|e| e.from_store)
            .cloned()
            .collect();
        if store.is_empty() {
            return;
        }
        let dir = self.ext.dir().to_path_buf();
        std::thread::spawn(move || {
            for extension in store {
                match extensions::fetch_update(&extension, &dir) {
                    Ok(Some((path, manifest))) => {
                        let id = extension.id.clone();
                        remote::run_on_ui(move || {
                            dispatch(Cmd::ExtUpdated {
                                id,
                                path,
                                version: manifest.version,
                            })
                        });
                    }
                    Ok(None) => {}
                    Err(err) => {
                        eprintln!("circle: update check for {} failed: {err}", extension.id)
                    }
                }
            }
        });
    }

    /// A newer version is unpacked; it runs from the next start.
    pub(super) fn extension_updated(&mut self, id: String, path: PathBuf, version: String) {
        if let Some(extension) = self.ext.get_mut(&id) {
            extension.path = path;
            extension.version = version;
            self.save_extensions();
        }
        self.push_settings();
    }
}

fn load(path: PathBuf, expected: Option<String>, purpose: LoadPurpose) {
    let params = json!({ "path": path.to_string_lossy() });
    engine::call("Extensions.loadUnpacked", params, move |result| {
        let result = result.and_then(|r| {
            r["id"]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| "no extension ID".to_owned())
        });
        dispatch(Cmd::ExtLoaded {
            path,
            expected,
            purpose,
            result,
        });
    });
}

wrap_run_file_dialog_callback! {
    struct UnpackedChosen;

    impl RunFileDialogCallback {
        fn on_file_dialog_dismissed(&self, file_paths: Option<&mut CefStringList>) {
            let first = file_paths.and_then(|paths| paths.clone().into_iter().next());
            if let Some(path) = first {
                dispatch(Cmd::ExtUnpackedChosen(PathBuf::from(path)));
            }
        }
    }
}
