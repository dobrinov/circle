//! Bug reports: while the person reproduces a problem, Circle keeps a log of
//! what happens (navigations, tab switches, actions, page errors, failed
//! loads, crashes). The bug report page (`ui/bugreport.html`) combines it with
//! their answers into the text of a GitHub issue.
//!
//! Nothing is logged unless a bug report recording is running.

use serde::Serialize;
use serde_json::{Value, json};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};

/// Whether a recording is running; read by the browser callbacks so they
/// skip all work otherwise.
static RECORDING: AtomicBool = AtomicBool::new(false);

pub fn is_recording() -> bool {
    RECORDING.load(Ordering::Relaxed)
}

/// More than this and the oldest entries are dropped.
const MAX_EVENTS: usize = 400;
const MAX_TEXT: usize = 600;

#[derive(Clone, Debug, Serialize)]
pub struct Event {
    /// Milliseconds since the recording started.
    pub at: u64,
    pub kind: &'static str,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// How many times in a row this happened.
    pub count: u32,
}

pub struct Session {
    started: Instant,
    events: Vec<Event>,
    dropped: usize,
    pub video: bool,
}

impl Session {
    pub fn start(video: bool) -> Session {
        RECORDING.store(true, Ordering::Relaxed);
        Session {
            started: Instant::now(),
            events: Vec::new(),
            dropped: 0,
            video,
        }
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    pub fn log(&mut self, kind: &'static str, text: &str, url: Option<&str>) {
        let mut text = text.trim().to_owned();
        if text.len() > MAX_TEXT {
            let mut end = MAX_TEXT;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
            text.push('…');
        }
        let url = url.filter(|u| !u.is_empty()).map(str::to_owned);
        // Repeats (a console error in a loop) are counted, not stored.
        if let Some(last) = self.events.last_mut()
            && last.kind == kind
            && last.text == text
            && last.url == url
        {
            last.count += 1;
            return;
        }
        if self.events.len() == MAX_EVENTS {
            self.events.remove(0);
            self.dropped += 1;
        }
        self.events.push(Event {
            at: self.elapsed_ms(),
            kind,
            text,
            url,
            count: 1,
        });
    }

    /// Ends the recording, returning what the report page needs.
    pub fn finish(self) -> Value {
        RECORDING.store(false, Ordering::Relaxed);
        json!({
            "duration": self.elapsed_ms(),
            "events": self.events,
            "dropped": self.dropped,
            "video": self.video,
        })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        RECORDING.store(false, Ordering::Relaxed);
    }
}

/// Versions and platform, for the report's Environment section.
pub fn environment() -> Value {
    let chromium = std::str::from_utf8(cef::sys::CEF_VERSION)
        .unwrap_or_default()
        .trim_end_matches('\0')
        .split("chromium-")
        .nth(1)
        .unwrap_or("unknown")
        .to_owned();
    json!({
        "circle": env!("CARGO_PKG_VERSION"),
        "chromium": chromium,
        "os": os_name(),
        "arch": std::env::consts::ARCH,
        "repository": env!("CARGO_PKG_REPOSITORY"),
    })
}

fn os_name() -> String {
    let run = |cmd: &str, args: &[&str]| {
        std::process::Command::new(cmd)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .filter(|s| !s.is_empty())
    };
    let name = match std::env::consts::OS {
        "macos" => run("sw_vers", &["-productVersion"]).map(|v| format!("macOS {v}")),
        "linux" => std::fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|text| {
                text.lines()
                    .find_map(|l| l.strip_prefix("PRETTY_NAME="))
                    .map(|v| v.trim_matches('"').to_owned())
            }),
        "windows" => run("cmd", &["/C", "ver"]),
        _ => None,
    };
    name.unwrap_or_else(|| std::env::consts::OS.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    // One test: the recording flag is shared, so parallel tests would race.
    #[test]
    fn logs_events_and_stops_recording() {
        let mut session = Session::start(false);
        assert!(is_recording());
        session.log(
            "console",
            "TypeError: x is undefined",
            Some("https://a.test/"),
        );
        session.log(
            "console",
            "TypeError: x is undefined",
            Some("https://a.test/"),
        );
        session.log("navigate", "", Some("https://b.test/"));
        session.log("console", &"é".repeat(1000), None);
        let report = session.finish();
        assert!(!is_recording());
        let events = report["events"].as_array().unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0]["count"], 2, "repeats are counted, not stored");
        assert!(events[1].get("url").is_some());
        let long = events[2]["text"].as_str().unwrap();
        assert!(
            long.ends_with('…') && long.len() <= MAX_TEXT + 3,
            "long text is trimmed"
        );
    }
}
