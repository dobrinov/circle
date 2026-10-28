//! The Network tool: Circle's own view of a tab's requests, which knows
//! GraphQL, REST and WebSockets. It docks next to the pages like DevTools,
//! follows the active tab, and talks to the tab over the same local CDP
//! bridge DevTools uses (see `remote.rs`).

use super::*;

const NETWORK_URL: &str = "circle://ui/network.html";

impl Controller {
    pub(super) fn toggle_network(&mut self) {
        if let Some(view) = self.network.take() {
            if let Some(host) = view.browser().and_then(|b| b.host()) {
                host.close_browser(1);
            }
            self.network_ready = false;
            self.network_cdp.clear();
            self.network_pushed.borrow_mut().clear();
            self.content.remove_child_view(Some(&mut View::from(&view)));
            self.content.layout();
            self.update_page_corners();
            if let Some(live) = self.session.active_tab().and_then(|id| self.live.get(&id)) {
                live.view.request_focus();
            }
            return;
        }
        if let Some(tab) = self.session.active_tab() {
            self.ensure_live(tab);
        }
        let view = views::create_browser_view(Role::Network, NETWORK_URL, 0xFF14_1821);
        self.content.add_child_view(Some(&mut View::from(&view)));
        self.network = Some(view);
        self.apply_layout();
        self.layout_tabs();
    }

    /// Keeps the Network tool last in the row, after any docked DevTools.
    pub(super) fn keep_network_last(&self) {
        if let Some(view) = &self.network {
            self.content
                .reorder_child_view(Some(&mut View::from(view)), -1);
        }
    }

    /// Tells the Network tool which tab to show, when that changed.
    pub(super) fn push_network(&self) {
        let (Some(view), true) = (&self.network, self.network_ready) else {
            return;
        };
        let tab = self.session.active_tab();
        let info = tab.and_then(|id| self.session.tab(id));
        let state = json!({
            "tab": tab,
            "title": info.map(|t| t.display_title()),
            "url": info.map(|t| t.url.as_str()),
            "live": tab.is_some_and(|id| self.live.contains_key(&id)),
            "dark": self.dark,
            "hue": self.session.space().hue,
            "dock": if self.session.devtools_dock == DevToolsDock::Bottom { "bottom" } else { "right" },
        })
        .to_string();
        let mut pushed = self.network_pushed.borrow_mut();
        if *pushed != state {
            run_js(view, &format!("window.circle&&circle.render({state})"));
            *pushed = state;
        }
    }

    pub(super) fn network_connect(&mut self, tab: TabId) {
        if !self.network_cdp.contains_key(&tab)
            && let Some(browser) = self.live.get(&tab).and_then(|l| l.view.browser())
            && let Some(client) = crate::remote::LocalClient::open(tab, browser)
        {
            self.network_cdp.insert(tab, client);
        }
        self.network_state(tab, self.network_cdp.contains_key(&tab));
    }

    pub(super) fn network_send(&mut self, tab: TabId, message: &str) {
        let sent = self.network_cdp.get(&tab).map(|c| c.send(message));
        if !matches!(sent, Some(Ok(()))) {
            self.network_detach(tab);
        }
    }

    pub(super) fn network_detach(&mut self, tab: TabId) {
        if self.network_cdp.remove(&tab).is_some() {
            self.network_state(tab, false);
        }
    }

    pub(super) fn network_messages(&self, tab: TabId, batch: Vec<String>) {
        if let (Some(view), true) = (&self.network, self.network_cdp.contains_key(&tab)) {
            // Each message is a JSON object, which is also a JS expression.
            run_js(view, &format!("circle.cdp&&circle.cdp({tab},[{}])", batch.join(",")));
        }
    }

    fn network_state(&self, tab: TabId, open: bool) {
        if let Some(view) = &self.network {
            run_js(view, &format!("circle.cdpState&&circle.cdpState({tab},{open})"));
        }
    }

    /// Resizes the Network tool as its edge is dragged.
    pub(super) fn drag_network(&mut self, phase: &str, dx: i32, dy: i32) {
        let bottom = self.session.devtools_dock == DevToolsDock::Bottom;
        match phase {
            "start" => {
                self.network_drag_start = Some(if bottom {
                    self.session.network_height
                } else {
                    self.session.network_width
                });
            }
            "move" => {
                let Some(start) = self.network_drag_start else {
                    return;
                };
                let area = self.content.bounds();
                let max = if bottom { area.height } else { area.width } - 200;
                // Dragging the edge towards the page makes the tool bigger.
                let size = (start - if bottom { dy } else { dx }).clamp(280, max.max(280));
                if bottom {
                    self.session.network_height = size;
                } else {
                    self.session.network_width = size;
                }
                self.apply_layout();
            }
            _ => {
                self.network_drag_start = None;
                self.changed();
            }
        }
    }
}
