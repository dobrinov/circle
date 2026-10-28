//! Saved passwords in pages: the helper script in each page's isolated
//! world, the suggestions under a login field, the save prompt after
//! signing in, and the Passwords settings.

use super::*;
use crate::passwords::{self, Proposal, Vault};
use crate::remote;

const AUTOFILL_URL: &str = "circle://ui/autofill.html";
/// How long a copied password stays on the clipboard.
const CLIPBOARD_SECONDS: u64 = 60;

/// What the overlay over the page shows.
pub(super) enum Autofill {
    /// Saved logins for the focused field, whose rect (CSS px in the page)
    /// it sits under.
    Suggest {
        tab: TabId,
        context: i64,
        origin: String,
        rect: (f64, f64, f64, f64),
    },
    /// "Save password?" after signing in.
    Save {
        tab: TabId,
        origin: String,
        username: String,
        password: String,
        update: Option<u64>,
    },
}

impl Autofill {
    fn tab(&self) -> TabId {
        match self {
            Autofill::Suggest { tab, .. } | Autofill::Save { tab, .. } => *tab,
        }
    }
}

/// Runs `expression` in the password helper's world of the browser's main
/// frame, creating the world if needed; `done` gets the result's value and
/// the world's execution context.
fn in_world(
    browser: &Browser,
    expression: String,
    done: impl FnOnce(Option<(Value, i64)>) + 'static,
) {
    let target = browser.clone();
    remote::command(browser, "Page.getFrameTree", json!({}), move |tree| {
        let frame = tree
            .ok()
            .and_then(|t| t["frameTree"]["frame"]["id"].as_str().map(str::to_owned));
        let Some(frame) = frame else {
            return done(None);
        };
        let world = json!({ "frameId": frame, "worldName": passwords::WORLD });
        let again = target.clone();
        remote::command(&target, "Page.createIsolatedWorld", world, move |world| {
            let Some(context) = world.ok().and_then(|w| w["executionContextId"].as_i64()) else {
                return done(None);
            };
            let params =
                json!({ "contextId": context, "expression": expression, "returnByValue": true });
            remote::command(&again, "Runtime.evaluate", params, move |result| {
                done(result.ok().map(|r| (r["result"]["value"].clone(), context)));
            });
        });
    });
}

/// Asks for Touch ID or Windows Hello off the UI thread, then shows or
/// copies the password.
pub(super) fn confirm_owner(id: u64, copy: bool) {
    std::thread::spawn(move || {
        let reason = if copy {
            "copy a saved password"
        } else {
            "show a saved password"
        };
        let ok = passwords::confirm_owner(reason);
        remote::run_on_ui(move || dispatch(Cmd::PasswordConfirmed { id, copy, ok }));
    });
}

impl Controller {
    /// The vault, opened on first use.
    pub(super) fn vault(&mut self) -> Option<&mut Vault> {
        if self.vault.is_none() && self.vault_error.is_none() {
            let dir = self
                .session_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default();
            match Vault::open(&dir) {
                Ok(vault) => self.vault = Some(vault),
                Err(error) => self.vault_error = Some(error),
            }
        }
        self.vault.as_mut()
    }

    /// Saved logins for Settings, without their passwords.
    pub(super) fn password_list(&self) -> Value {
        let Some(vault) = &self.vault else {
            return json!([]);
        };
        let mut list: Vec<_> = vault
            .all()
            .iter()
            .map(|c| json!({ "id": c.id, "origin": c.origin, "username": c.username, "updated": c.updated }))
            .collect();
        list.sort_by_key(|c| c["origin"].as_str().map(str::to_owned));
        Value::Array(list)
    }

    fn helper_wanted(&self, tab: TabId) -> bool {
        self.session.passwords_enabled
            && !self.session.is_private_tab(tab)
            && self
                .session
                .tab(tab)
                .and_then(|t| passwords::origin_of(&t.url))
                .is_some()
    }

    pub(super) fn inject_password_helper(&mut self, tab: TabId) {
        if matches!(&self.autofill_state, Some(Autofill::Suggest { tab: t, .. }) if *t == tab) {
            self.hide_autofill();
        }
        if !self.helper_wanted(tab) {
            return;
        }
        if let Some(browser) = self.live.get(&tab).and_then(|l| l.view.browser()) {
            in_world(&browser, passwords::CONTENT_JS.to_owned(), |_| {});
        }
    }

    pub(super) fn collect_password_news(&mut self, tab: TabId) {
        if !self.helper_wanted(tab) {
            return;
        }
        let Some(browser) = self.live.get(&tab).and_then(|l| l.view.browser()) else {
            return;
        };
        let take = "window.__circlePw ? __circlePw.take() : '[]'".to_owned();
        in_world(&browser, take, move |result| {
            if let Some((Value::String(text), context)) = result {
                let events = serde_json::from_str(&text).unwrap_or_default();
                dispatch(Cmd::PasswordsNews(tab, context, events));
            }
        });
    }

    pub(super) fn password_news(&mut self, tab: TabId, context: i64, events: Vec<Value>) {
        if !self.helper_wanted(tab) {
            return;
        }
        // The helper's world belongs to one page, whose origin it reports;
        // the page's own scripts can't reach it to fake that.
        for event in events {
            let Some(origin) = event["origin"].as_str().and_then(passwords::origin_of) else {
                continue;
            };
            match event["type"].as_str() {
                Some("focus") => {
                    let saved = self
                        .vault()
                        .is_some_and(|v| !v.for_origin(&origin).is_empty());
                    if !saved || self.session.active_tab() != Some(tab) {
                        continue;
                    }
                    let r = &event["rect"];
                    let num = |k: &str| r[k].as_f64().unwrap_or(0.0);
                    self.autofill_state = Some(Autofill::Suggest {
                        tab,
                        context,
                        origin,
                        rect: (num("x"), num("y"), num("width"), num("height")),
                    });
                    self.show_autofill();
                }
                Some("blur" | "typing") => {
                    if matches!(&self.autofill_state, Some(Autofill::Suggest { tab: t, .. }) if *t == tab)
                    {
                        self.hide_autofill();
                    }
                }
                Some("submit") => {
                    if !self.session.passwords_offer {
                        continue;
                    }
                    let username = event["username"].as_str().unwrap_or("").trim().to_owned();
                    let password = event["password"].as_str().unwrap_or("").to_owned();
                    let Some(vault) = self.vault() else {
                        continue;
                    };
                    let update = match vault.propose(&origin, &username, &password) {
                        Proposal::Nothing => continue,
                        Proposal::New => None,
                        Proposal::Update(id) => Some(id),
                    };
                    self.autofill_state = Some(Autofill::Save {
                        tab,
                        origin,
                        username,
                        password,
                        update,
                    });
                    self.show_autofill();
                }
                _ => {}
            }
        }
    }

    fn show_autofill(&mut self) {
        if self.autofill.is_none() {
            let Some(window) = &self.window else {
                return;
            };
            let view = views::create_browser_view(Role::Autofill, AUTOFILL_URL, self.background());
            let overlay =
                window.add_overlay_view(Some(&mut View::from(&view)), DockingMode::CUSTOM, 1);
            self.autofill = Some((view, overlay));
        }
        self.push_autofill();
        self.place_autofill();
    }

    pub(super) fn hide_autofill(&mut self) {
        self.autofill_state = None;
        if let Some((_, Some(overlay))) = &self.autofill {
            overlay.set_visible(0);
        }
    }

    /// Hides the suggestions or prompt when their tab isn't the one showing.
    pub(super) fn autofill_follow_tab(&mut self) {
        if self
            .autofill_state
            .as_ref()
            .is_some_and(|s| Some(s.tab()) != self.session.active_tab())
        {
            self.hide_autofill();
        }
    }

    pub(super) fn push_autofill(&self) {
        let (Some((view, _)), true) = (&self.autofill, self.autofill_ready) else {
            return;
        };
        let state = match &self.autofill_state {
            Some(Autofill::Suggest { origin, .. }) => {
                let accounts: Vec<Value> = self
                    .vault
                    .as_ref()
                    .map(|v| {
                        v.for_origin(origin)
                            .iter()
                            .map(|c| json!({ "id": c.id, "username": c.username }))
                            .collect()
                    })
                    .unwrap_or_default();
                json!({ "mode": "suggest", "host": host_of(origin), "accounts": accounts })
            }
            Some(Autofill::Save {
                origin,
                username,
                update,
                ..
            }) => json!({
                "mode": "save",
                "host": host_of(origin),
                "username": username,
                "update": update.is_some(),
            }),
            None => json!({ "mode": "none" }),
        };
        let mut state = state;
        state["hue"] = json!(self.session.space().hue);
        run_js(view, &format!("window.circle&&circle.render({state})"));
    }

    pub(super) fn place_autofill(&self) {
        let Some((_, Some(overlay))) = &self.autofill else {
            return;
        };
        let covered = self
            .palette_mode
            .is_some_and(|mode| mode != PaletteMode::Find);
        let (Some(state), false) = (&self.autofill_state, covered) else {
            overlay.set_visible(0);
            return;
        };
        let (width, height) = self.autofill_size;
        let area = self.content.bounds();
        let bounds = match state {
            Autofill::Suggest { tab, rect, .. } => {
                let Some(live) = self.live.get(tab) else {
                    overlay.set_visible(0);
                    return;
                };
                let pages = self.pages.bounds();
                let page = live.view.bounds();
                let zoom = live
                    .view
                    .browser()
                    .and_then(|b| b.host())
                    .map_or(1.0, |h| 1.2f64.powf(h.zoom_level()));
                let left = area.x + pages.x + page.x;
                let top = area.y + pages.y + page.y;
                let x = left + (rect.0 * zoom).round() as c_int;
                let below = top + ((rect.1 + rect.3) * zoom).round() as c_int + 4;
                let above = top + (rect.1 * zoom).round() as c_int - height - 4;
                let bottom = top + page.height;
                Rect {
                    x: x.clamp(left, (left + page.width - width).max(left)),
                    y: if below + height > bottom && above >= top {
                        above
                    } else {
                        below
                    },
                    width,
                    height,
                }
            }
            Autofill::Save { .. } => Rect {
                x: area.x + area.width - width - 2 * GAP,
                y: area.y + 2 * GAP,
                width,
                height,
            },
        };
        overlay.set_bounds(Some(&bounds));
        overlay.set_visible(1);
        #[cfg(target_os = "macos")]
        if let Some((view, _)) = &self.autofill
            && let Some(browser) = view.browser()
        {
            crate::mac::round_browser_window(&browser, 10.0, crate::mac::ALL_CORNERS);
        }
    }

    pub(super) fn fill_login(&mut self, id: u64) {
        let Some(Autofill::Suggest {
            tab,
            context,
            origin,
            ..
        }) = self.autofill_state.take()
        else {
            return;
        };
        self.hide_autofill();
        let Some(credential) = self.vault().and_then(|v| v.get(id)).cloned() else {
            return;
        };
        // Only the site a password was saved on gets it.
        if credential.origin != origin {
            return;
        }
        let Some(live) = self.live.get(&tab) else {
            return;
        };
        if let Some(browser) = live.view.browser() {
            let expression = format!(
                "window.__circlePw && __circlePw.fill({}, {})",
                json!(credential.username),
                json!(credential.password)
            );
            let params = json!({ "contextId": context, "expression": expression });
            remote::command(&browser, "Runtime.evaluate", params, |_| {});
        }
        live.view.request_focus();
        if let Some(vault) = self.vault() {
            vault.mark_used(id);
        }
    }

    pub(super) fn save_offered_login(&mut self, never: bool) {
        let Some(Autofill::Save {
            origin,
            username,
            password,
            ..
        }) = self.autofill_state.take()
        else {
            return;
        };
        self.hide_autofill();
        if let Some(vault) = self.vault() {
            let _ = if never {
                vault.never_save(&origin)
            } else {
                vault.save_login(&origin, &username, &password).map(|_| ())
            };
        }
        self.push_settings();
    }

    pub(super) fn password_confirmed(&mut self, id: u64, copy: bool, ok: bool) {
        if !ok {
            return;
        }
        let Some(password) = self
            .vault()
            .and_then(|v| v.get(id))
            .map(|c| c.password.clone())
        else {
            return;
        };
        if copy {
            if arboard::Clipboard::new()
                .and_then(|mut c| c.set_text(password.clone()))
                .is_ok()
            {
                // Taken off the clipboard again, unless something else was
                // copied since.
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(CLIPBOARD_SECONDS));
                    if let Ok(mut clipboard) = arboard::Clipboard::new()
                        && clipboard.get_text().is_ok_and(|t| t == password)
                    {
                        let _ = clipboard.clear();
                    }
                });
            }
            self.push_settings_with(json!({ "copied": id }));
        } else {
            self.push_settings_with(json!({ "revealed": { "id": id, "password": password } }));
        }
    }
}

fn host_of(origin: &str) -> String {
    url::Url::parse(origin)
        .ok()
        .and_then(|u| {
            u.host_str()
                .map(|h| h.trim_start_matches("www.").to_owned())
        })
        .unwrap_or_else(|| origin.to_owned())
}
