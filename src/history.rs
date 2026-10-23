//! Browsing history: every page visited, newest first, saved to
//! `history.json` in the profile.
//!
//! The store is shared behind a mutex so the history page and Spotlight can
//! query it from the scheme handler's IO thread without a round trip to the
//! UI thread.

use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

/// Old visits beyond this many are dropped.
const LIMIT: usize = 20_000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Visit {
    pub url: String,
    #[serde(default)]
    pub title: String,
    /// Unix seconds.
    pub at: u64,
}

struct Store {
    path: Option<PathBuf>,
    /// Newest first.
    visits: Vec<Visit>,
    dirty: bool,
}

static STORE: Mutex<Store> = Mutex::new(Store {
    path: None,
    visits: Vec::new(),
    dirty: false,
});

fn store() -> std::sync::MutexGuard<'static, Store> {
    STORE.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn load(profile_dir: &Path) {
    let path = profile_dir.join("history.json");
    let visits = fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    let mut store = store();
    store.path = Some(path);
    store.visits = visits;
}

/// Writes the history if it changed since the last save.
pub fn save() {
    let (path, text) = {
        let mut store = store();
        if !store.dirty {
            return;
        }
        store.dirty = false;
        let Some(path) = store.path.clone() else {
            return;
        };
        (path, serde_json::to_vec(&store.visits).unwrap_or_default())
    };
    let tmp = path.with_extension("json.tmp");
    if let Err(err) = fs::write(&tmp, text).and_then(|_| fs::rename(&tmp, &path)) {
        eprintln!("circle: failed to save history: {err}");
    }
}

/// Whether a URL belongs in history (not Circle's own pages and the like).
fn recordable(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://") || url.starts_with("file://")
}

/// Records a visit, unless it repeats the newest one (reloads, redirects
/// back to the same page).
pub fn record(url: &str, title: &str, at: u64) {
    if !recordable(url) {
        return;
    }
    let mut store = store();
    if let Some(last) = store.visits.first_mut()
        && last.url == url
        && at.saturating_sub(last.at) < 60
    {
        last.at = at;
        if !title.is_empty() {
            last.title = title.to_owned();
        }
        store.dirty = true;
        return;
    }
    store.visits.insert(
        0,
        Visit {
            url: url.to_owned(),
            title: title.to_owned(),
            at,
        },
    );
    store.visits.truncate(LIMIT);
    store.dirty = true;
}

/// Sets the title of the most recent visit to `url` (titles arrive after
/// the address changes).
pub fn update_title(url: &str, title: &str) {
    if title.is_empty() || !recordable(url) {
        return;
    }
    let mut store = store();
    if let Some(visit) = store.visits.iter_mut().take(50).find(|v| v.url == url)
        && visit.title != title
    {
        visit.title = title.to_owned();
        store.dirty = true;
    }
}

/// Visits matching every word of `query` (in title or URL), newest first,
/// older than `before` (Unix seconds, if given).
pub fn search(query: &str, before: Option<u64>, limit: usize) -> Vec<Visit> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    store()
        .visits
        .iter()
        .filter(|v| before.is_none_or(|b| v.at < b))
        .filter(|v| {
            let haystack = format!("{} {}", v.title, v.url).to_lowercase();
            words.iter().all(|w| haystack.contains(w.as_str()))
        })
        .take(limit)
        .cloned()
        .collect()
}

pub fn delete(url: &str, at: u64) {
    let mut store = store();
    let before = store.visits.len();
    store.visits.retain(|v| !(v.url == url && v.at == at));
    store.dirty |= store.visits.len() != before;
}

/// Removes visits made at or after `since` (Unix seconds; 0 clears all).
pub fn clear_since(since: u64) {
    let mut store = store();
    store.visits.retain(|v| v.at < since);
    store.dirty = true;
}

#[cfg(test)]
pub fn reset_for_tests() {
    let mut store = store();
    store.visits.clear();
    store.path = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    // One test touches the shared store, so the cases run in sequence.
    #[test]
    fn records_searches_and_clears() {
        reset_for_tests();
        record("circle://ui/history.html", "History", 1);
        assert!(
            search("", None, 10).is_empty(),
            "Circle's pages aren't recorded"
        );

        record("https://example.com/", "", 100);
        update_title("https://example.com/", "Example Domain");
        record("https://example.com/", "", 120); // a reload a moment later
        record("https://rust-lang.org/", "Rust Programming Language", 200);
        let all = search("", None, 10);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].url, "https://rust-lang.org/");
        assert_eq!(all[1].title, "Example Domain");

        assert_eq!(search("rust lang", None, 10).len(), 1);
        assert_eq!(search("", Some(150), 10)[0].url, "https://example.com/");

        delete("https://rust-lang.org/", 200);
        assert_eq!(search("", None, 10).len(), 1);
        record("https://docs.rs/", "Docs.rs", 500);
        clear_since(300);
        assert_eq!(search("", None, 10).len(), 1);
        clear_since(0);
        assert!(search("", None, 10).is_empty());
    }
}
