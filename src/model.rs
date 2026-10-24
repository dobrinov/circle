//! The persisted browsing session: spaces, favorites, pinned and "today"
//! tabs, plus the archive of closed tabs. This is plain data and knows
//! nothing about CEF.

use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub type TabId = u64;

pub const START_URL: &str = "https://www.google.com/";
const ARCHIVE_LIMIT: usize = 200;
const SPACE_HUES: [u16; 8] = [262, 200, 340, 150, 28, 300, 180, 50];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Tab {
    pub id: TabId,
    pub url: String,
    #[serde(default)]
    pub title: String,
    /// A name the user gave the tab; shown instead of the page title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_title: Option<String>,
    /// The folder (pinned tabs only) the tab is in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<u64>,
    #[serde(default)]
    pub favicon: Option<String>,
    /// For pinned tabs and favorites: the URL the tab returns to when it is
    /// "closed" (Arc-style reset).
    #[serde(default)]
    pub home_url: Option<String>,
    #[serde(default)]
    pub last_active: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Space {
    pub id: u64,
    pub name: String,
    pub icon: String,
    pub hue: u16,
    #[serde(default)]
    pub pinned: Vec<Tab>,
    #[serde(default)]
    pub tabs: Vec<Tab>,
    #[serde(default)]
    pub active: Option<TabId>,
    /// Folders that group the space's pinned tabs.
    #[serde(default)]
    pub folders: Vec<Folder>,
    /// A private space: its tabs use a memory-only browsing session and it
    /// is never saved, archived or recorded in history.
    #[serde(skip)]
    pub private: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Folder {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub collapsed: bool,
}

impl Tab {
    /// The user's name for the tab, or the page title.
    pub fn display_title(&self) -> &str {
        self.custom_title.as_deref().unwrap_or(&self.title)
    }
}

impl Space {
    /// Pinned tabs in the order the sidebar shows them: folder by folder,
    /// then those outside any folder.
    pub fn pinned_in_display_order(&self) -> Vec<&Tab> {
        let mut tabs: Vec<&Tab> = Vec::with_capacity(self.pinned.len());
        for folder in &self.folders {
            tabs.extend(self.pinned.iter().filter(|t| t.folder == Some(folder.id)));
        }
        tabs.extend(self.pinned.iter().filter(|t| {
            t.folder
                .is_none_or(|f| !self.folders.iter().any(|folder| folder.id == f))
        }));
        tabs
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Archived {
    pub url: String,
    pub title: String,
    #[serde(default)]
    pub favicon: Option<String>,
    pub closed_at: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Section {
    Favorites,
    Pinned,
    Today,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    #[serde(default)]
    pub spaces: Vec<Space>,
    #[serde(default)]
    pub active_space: usize,
    #[serde(default)]
    pub favorites: Vec<Tab>,
    #[serde(default)]
    pub archive: Vec<Archived>,
    #[serde(default)]
    pub next_id: u64,
    #[serde(default = "yes")]
    pub sidebar_visible: bool,
    /// Unpinned tabs idle for longer than this are archived (0 = never).
    #[serde(default = "default_archive_hours")]
    pub archive_after_hours: u64,
    #[serde(default = "default_search")]
    pub search_url: String,
    /// Remembered permission grants, keyed by `"<kind>:<origin>"`.
    #[serde(default)]
    pub grants: BTreeMap<String, u32>,
    /// Whether local AI agents may control the browser (off by default).
    #[serde(default)]
    pub agent_access: bool,
    /// Download updates in the background and install them on quit.
    #[serde(default = "yes")]
    pub auto_update: bool,
    /// Recordings show the pointer and clicks.
    #[serde(default = "yes")]
    pub recording_clicks: bool,
    /// Recordings show the keys being pressed.
    #[serde(default = "yes")]
    pub recording_keys: bool,
    /// While the sidebar is hidden, recording controls float over the page.
    #[serde(default = "yes")]
    pub recording_float: bool,
    /// The Circle version that last ran with this profile, so the next
    /// version knows to show what's new.
    #[serde(default)]
    pub seen_version: Option<String>,
    /// Blur the page behind Spotlight (otherwise it is only dimmed).
    #[serde(default = "yes")]
    pub spotlight_blur: bool,
    /// Where developer tools open.
    #[serde(default)]
    pub devtools_dock: DevToolsDock,
    /// Docked developer tools' width (right) and height (bottom).
    #[serde(default = "default_devtools_width")]
    pub devtools_width: i32,
    #[serde(default = "default_devtools_height")]
    pub devtools_height: i32,
    /// Light or dark, for Circle and the pages it shows.
    #[serde(default)]
    pub appearance: Appearance,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    /// Follow the operating system.
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DevToolsDock {
    #[default]
    Right,
    Bottom,
    Window,
}

fn default_devtools_width() -> i32 {
    480
}

fn default_devtools_height() -> i32 {
    320
}

fn yes() -> bool {
    true
}

fn default_archive_hours() -> u64 {
    12
}

fn default_search() -> String {
    "https://www.google.com/search?q=%s".to_owned()
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

impl Default for Session {
    fn default() -> Self {
        let mut session = Session {
            spaces: Vec::new(),
            active_space: 0,
            favorites: Vec::new(),
            archive: Vec::new(),
            next_id: 1,
            sidebar_visible: true,
            archive_after_hours: default_archive_hours(),
            search_url: default_search(),
            grants: BTreeMap::new(),
            agent_access: false,
            auto_update: true,
            spotlight_blur: true,
            seen_version: None,
            recording_clicks: true,
            recording_keys: true,
            recording_float: true,
            devtools_dock: DevToolsDock::default(),
            devtools_width: default_devtools_width(),
            devtools_height: default_devtools_height(),
            appearance: Appearance::default(),
        };
        session.add_space("Personal");
        let tab = session.new_tab(START_URL);
        let space = &mut session.spaces[0];
        space.active = Some(tab.id);
        space.tabs.push(tab);
        session
    }
}

/// Where a tab lives in the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Location {
    Favorite(usize),
    Pinned(usize, usize),
    Today(usize, usize),
}

impl Session {
    pub fn load(path: &Path) -> Self {
        let mut session = fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<Session>(&text).ok())
            .unwrap_or_default();
        if session.spaces.is_empty() {
            session.add_space("Personal");
        }
        session.active_space = session.active_space.min(session.spaces.len() - 1);
        let max_id = session
            .all_tabs()
            .map(|t| t.id)
            .chain(
                session
                    .spaces
                    .iter()
                    .flat_map(|s| s.folders.iter().map(|f| f.id)),
            )
            .max()
            .unwrap_or(0);
        session.next_id = session.next_id.max(max_id + 1);
        session
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(&self.without_private())?)?;
        fs::rename(tmp, path)
    }

    pub fn path(profile_dir: &Path) -> PathBuf {
        profile_dir.join("session.json")
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn new_tab(&mut self, url: &str) -> Tab {
        Tab {
            id: self.alloc_id(),
            url: url.to_owned(),
            title: String::new(),
            custom_title: None,
            folder: None,
            favicon: None,
            home_url: None,
            last_active: now(),
        }
    }

    pub fn add_space(&mut self, name: &str) -> usize {
        let index = self.spaces.len();
        let id = self.alloc_id();
        self.spaces.push(Space {
            id,
            name: name.to_owned(),
            icon: String::new(),
            hue: SPACE_HUES[index % SPACE_HUES.len()],
            pinned: Vec::new(),
            tabs: Vec::new(),
            active: None,
            folders: Vec::new(),
            private: false,
        });
        index
    }

    /// Whether a tab lives in a private space.
    pub fn is_private_tab(&self, id: TabId) -> bool {
        match self.locate(id) {
            Some(Location::Pinned(space, _) | Location::Today(space, _)) => {
                self.spaces[space].private
            }
            _ => false,
        }
    }

    /// The session as written to disk: without private spaces.
    pub fn without_private(&self) -> Session {
        let mut saved = self.clone();
        let active_id = saved.spaces.get(saved.active_space).map(|s| s.id);
        saved.spaces.retain(|s| !s.private);
        saved.active_space = active_id
            .and_then(|id| saved.spaces.iter().position(|s| s.id == id))
            .unwrap_or(0);
        saved
    }

    /// Where a folder lives: (space index, folder index).
    pub fn locate_folder(&self, folder: u64) -> Option<(usize, usize)> {
        self.spaces.iter().enumerate().find_map(|(si, space)| {
            space
                .folders
                .iter()
                .position(|f| f.id == folder)
                .map(|fi| (si, fi))
        })
    }

    pub fn add_folder(&mut self, space: usize, name: &str) -> u64 {
        let id = self.alloc_id();
        self.spaces[space].folders.push(Folder {
            id,
            name: name.to_owned(),
            collapsed: false,
        });
        id
    }

    /// Moves a tab within its list so it sits just before `anchor` (or last).
    pub fn place_before(&mut self, id: TabId, anchor: Option<TabId>) {
        let list = match self.locate(id) {
            Some(Location::Favorite(_)) => &mut self.favorites,
            Some(Location::Pinned(s, _)) => &mut self.spaces[s].pinned,
            Some(Location::Today(s, _)) => &mut self.spaces[s].tabs,
            None => return,
        };
        let Some(from) = list.iter().position(|t| t.id == id) else {
            return;
        };
        let tab = list.remove(from);
        let to = anchor
            .and_then(|a| list.iter().position(|t| t.id == a))
            .unwrap_or(list.len());
        list.insert(to, tab);
    }

    /// Removes a folder; its tabs stay pinned in the space.
    pub fn delete_folder(&mut self, folder: u64) {
        let Some((si, fi)) = self.locate_folder(folder) else {
            return;
        };
        let space = &mut self.spaces[si];
        space.folders.remove(fi);
        for tab in &mut space.pinned {
            if tab.folder == Some(folder) {
                tab.folder = None;
            }
        }
    }

    /// Puts a tab in a folder (or takes it out with `None`), pinning it in
    /// the folder's space if it wasn't already.
    pub fn move_to_folder(&mut self, id: TabId, folder: Option<u64>) {
        match folder {
            Some(folder) => {
                let Some((si, _)) = self.locate_folder(folder) else {
                    return;
                };
                if !matches!(self.locate(id), Some(Location::Pinned(s, _)) if s == si) {
                    let end = self.spaces[si].pinned.len();
                    self.move_tab(id, Section::Pinned, si, end);
                }
                if let Some(tab) = self.tab_mut(id) {
                    tab.folder = Some(folder);
                }
            }
            None => {
                if let Some(tab) = self.tab_mut(id) {
                    tab.folder = None;
                }
            }
        }
    }

    pub fn space_index(&self, space_id: u64) -> Option<usize> {
        self.spaces.iter().position(|s| s.id == space_id)
    }

    pub fn space(&self) -> &Space {
        &self.spaces[self.active_space]
    }

    pub fn space_mut(&mut self) -> &mut Space {
        &mut self.spaces[self.active_space]
    }

    pub fn all_tabs(&self) -> impl Iterator<Item = &Tab> {
        self.favorites.iter().chain(
            self.spaces
                .iter()
                .flat_map(|s| s.pinned.iter().chain(s.tabs.iter())),
        )
    }

    pub fn locate(&self, id: TabId) -> Option<Location> {
        if let Some(i) = self.favorites.iter().position(|t| t.id == id) {
            return Some(Location::Favorite(i));
        }
        for (si, space) in self.spaces.iter().enumerate() {
            if let Some(i) = space.pinned.iter().position(|t| t.id == id) {
                return Some(Location::Pinned(si, i));
            }
            if let Some(i) = space.tabs.iter().position(|t| t.id == id) {
                return Some(Location::Today(si, i));
            }
        }
        None
    }

    pub fn tab(&self, id: TabId) -> Option<&Tab> {
        Some(match self.locate(id)? {
            Location::Favorite(i) => &self.favorites[i],
            Location::Pinned(s, i) => &self.spaces[s].pinned[i],
            Location::Today(s, i) => &self.spaces[s].tabs[i],
        })
    }

    pub fn tab_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        Some(match self.locate(id)? {
            Location::Favorite(i) => &mut self.favorites[i],
            Location::Pinned(s, i) => &mut self.spaces[s].pinned[i],
            Location::Today(s, i) => &mut self.spaces[s].tabs[i],
        })
    }

    /// The tab shown in the active space, if any.
    pub fn active_tab(&self) -> Option<TabId> {
        self.space().active.filter(|id| self.locate(*id).is_some())
    }

    /// Removes a tab from wherever it lives and returns it.
    pub fn take(&mut self, id: TabId) -> Option<Tab> {
        Some(match self.locate(id)? {
            Location::Favorite(i) => self.favorites.remove(i),
            Location::Pinned(s, i) => self.spaces[s].pinned.remove(i),
            Location::Today(s, i) => self.spaces[s].tabs.remove(i),
        })
    }

    /// Moves a tab into `section` of the space at `space` (ignored for
    /// favorites, which are shared by all spaces) at `index`.
    pub fn move_tab(&mut self, id: TabId, section: Section, space: usize, index: usize) {
        let Some(mut tab) = self.take(id) else {
            return;
        };
        tab.folder = None;
        match section {
            Section::Favorites | Section::Pinned => {
                tab.home_url.get_or_insert_with(|| tab.url.clone());
            }
            Section::Today => tab.home_url = None,
        }
        let list = match section {
            Section::Favorites => &mut self.favorites,
            Section::Pinned => &mut self.spaces[space].pinned,
            Section::Today => &mut self.spaces[space].tabs,
        };
        let index = index.min(list.len());
        list.insert(index, tab);
    }

    pub fn archive(&mut self, tab: &Tab) {
        if tab.url.is_empty() || tab.url.starts_with("circle:") {
            return;
        }
        self.archive.retain(|a| a.url != tab.url);
        self.archive.insert(
            0,
            Archived {
                url: tab.url.clone(),
                title: tab.title.clone(),
                favicon: tab.favicon.clone(),
                closed_at: now(),
            },
        );
        self.archive.truncate(ARCHIVE_LIMIT);
    }

    /// Archives "today" tabs that have not been used for a while, like Arc.
    /// Returns the ids that were removed.
    pub fn auto_archive(&mut self, keep: &[TabId]) -> Vec<TabId> {
        if self.archive_after_hours == 0 {
            return Vec::new();
        }
        let cutoff = now().saturating_sub(self.archive_after_hours * 3600);
        let mut removed = Vec::new();
        for si in 0..self.spaces.len() {
            if self.spaces[si].private {
                continue;
            }
            let (stale, fresh): (Vec<Tab>, Vec<Tab>) = std::mem::take(&mut self.spaces[si].tabs)
                .into_iter()
                .partition(|t| t.last_active < cutoff && !keep.contains(&t.id));
            self.spaces[si].tabs = fresh;
            for tab in stale {
                self.archive(&tab);
                removed.push(tab.id);
            }
        }
        removed
    }

    /// The URL to load for user input: either a URL or a search query.
    pub fn resolve_input(&self, input: &str) -> String {
        let input = input.trim();
        if input.is_empty() {
            return START_URL.to_owned();
        }
        let has_scheme = input.contains("://")
            || [
                "about:",
                "data:",
                "file:",
                "circle:",
                "view-source:",
                "chrome:",
            ]
            .iter()
            .any(|p| input.starts_with(p));
        if has_scheme {
            return input.to_owned();
        }
        let looks_like_host = !input.contains(' ')
            && (input.starts_with("localhost")
                || input
                    .split(['/', ':'])
                    .next()
                    .is_some_and(|host| host.contains('.') && !host.ends_with('.')));
        if looks_like_host {
            let candidate = format!("https://{input}");
            if url::Url::parse(&candidate).is_ok() {
                return if input.starts_with("localhost") || input.starts_with("127.") {
                    format!("http://{input}")
                } else {
                    candidate
                };
            }
        }
        let query: String = url::form_urlencoded::byte_serialize(input.as_bytes()).collect();
        self.search_url.replace("%s", &query)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_urls_and_searches() {
        let s = Session::default();
        assert_eq!(s.resolve_input("example.com"), "https://example.com");
        assert_eq!(
            s.resolve_input("example.com/a b"),
            s.search_url.replace("%s", "example.com%2Fa+b")
        );
        assert_eq!(s.resolve_input("localhost:3000"), "http://localhost:3000");
        assert_eq!(s.resolve_input("https://x.y/z"), "https://x.y/z");
        assert!(
            s.resolve_input("rust ownership")
                .contains("q=rust+ownership")
        );
    }

    #[test]
    fn moving_between_sections_sets_home_url() {
        let mut s = Session::default();
        let id = s.spaces[0].tabs[0].id;
        s.move_tab(id, Section::Pinned, 0, 0);
        assert_eq!(s.locate(id), Some(Location::Pinned(0, 0)));
        assert_eq!(s.tab(id).unwrap().home_url.as_deref(), Some(START_URL));
        s.move_tab(id, Section::Favorites, 0, 0);
        assert_eq!(s.locate(id), Some(Location::Favorite(0)));
        s.move_tab(id, Section::Today, 0, 5);
        assert_eq!(s.locate(id), Some(Location::Today(0, 0)));
        assert_eq!(s.tab(id).unwrap().home_url, None);
    }

    #[test]
    fn folders_group_pinned_tabs() {
        let mut s = Session::default();
        let today = s.spaces[0].tabs[0].id;
        let folder = s.add_folder(0, "Work");
        s.move_to_folder(today, Some(folder));
        // A today tab moved into a folder becomes pinned.
        assert_eq!(s.locate(today), Some(Location::Pinned(0, 0)));
        assert_eq!(s.tab(today).unwrap().folder, Some(folder));
        // Folder tabs come first in display order.
        let loose = s.new_tab("https://loose.example/");
        let loose_id = loose.id;
        s.spaces[0].pinned.insert(0, loose);
        let order: Vec<_> = s.spaces[0]
            .pinned_in_display_order()
            .iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(order, vec![today, loose_id]);
        // Moving to a section leaves the folder; deleting keeps the tabs.
        s.move_tab(today, Section::Pinned, 0, 0);
        assert_eq!(s.tab(today).unwrap().folder, None);
        s.move_to_folder(today, Some(folder));
        s.delete_folder(folder);
        assert!(s.spaces[0].folders.is_empty());
        assert_eq!(s.tab(today).unwrap().folder, None);
        assert!(s.locate(today).is_some());
    }

    #[test]
    fn custom_titles_override_page_titles() {
        let mut s = Session::default();
        let id = s.spaces[0].tabs[0].id;
        s.tab_mut(id).unwrap().title = "Page".into();
        assert_eq!(s.tab(id).unwrap().display_title(), "Page");
        s.tab_mut(id).unwrap().custom_title = Some("Mine".into());
        assert_eq!(s.tab(id).unwrap().display_title(), "Mine");
    }

    #[test]
    fn auto_archive_keeps_recent_and_kept_tabs() {
        let mut s = Session::default();
        let old = s.new_tab("https://old.example/");
        let kept = s.new_tab("https://kept.example/");
        let (old_id, kept_id) = (old.id, kept.id);
        s.spaces[0].tabs.push(Tab {
            last_active: 1,
            ..old
        });
        s.spaces[0].tabs.push(Tab {
            last_active: 1,
            ..kept
        });
        let removed = s.auto_archive(&[kept_id]);
        assert_eq!(removed, vec![old_id]);
        assert_eq!(s.archive[0].url, "https://old.example/");
        assert!(s.locate(kept_id).is_some());
    }

    #[test]
    fn private_spaces_are_never_saved() {
        let mut s = Session::default();
        let index = s.add_space("Private");
        s.spaces[index].private = true;
        s.active_space = index;
        let tab = s.new_tab("https://secret.example/");
        let id = tab.id;
        s.spaces[index].tabs.push(tab);
        assert!(s.is_private_tab(id));
        let saved = s.without_private();
        assert!(saved.spaces.iter().all(|sp| !sp.private));
        assert!(saved.all_tabs().all(|t| t.id != id));
        assert_eq!(saved.active_space, 0);
        let text = serde_json::to_string(&saved).unwrap();
        assert!(!text.contains("secret.example"));
    }
}
