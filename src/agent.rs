//! The agent API: high-level browser actions for AI agents, served by
//! `remote.rs` at `POST /api/<command>` while the user allows agent control.
//!
//! Commands run on the control server's worker threads. Browser state is
//! touched on the UI thread through the controller; page work goes through a
//! CDP session, so clicks and typing are real input events.

use crate::{
    controller,
    model::TabId,
    remote::{self, Session, call_on_ui},
};
use serde_json::{Value, json};
use std::{
    sync::atomic::{AtomicU64, Ordering},
    thread::sleep,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const CALL_TIMEOUT: Duration = Duration::from_secs(20);
const LOAD_TIMEOUT: Duration = Duration::from_secs(20);

static LAST_ACTIVITY: AtomicU64 = AtomicU64::new(0);

/// Records that an agent is active, so Circle can show it in the sidebar.
pub fn note_activity() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    LAST_ACTIVITY.store(now, Ordering::SeqCst);
    remote::run_on_ui(controller::note_agent_activity);
}

/// The commands, with a one-line description each (also used by `help`).
pub const COMMANDS: &[(&str, &str)] = &[
    ("tabs", "List open tabs and spaces"),
    ("open", "Open a URL in a new tab {url, background?}"),
    ("activate", "Switch to a tab {tab}"),
    ("close", "Close a tab {tab}"),
    ("navigate", "Load a URL or search in a tab {url, tab?}"),
    ("back", "Go back {tab?}"),
    ("forward", "Go forward {tab?}"),
    ("reload", "Reload {tab?}"),
    ("read", "Read a page's text {tab?, max_chars?}"),
    ("snapshot", "List interactive elements with refs {tab?}"),
    ("click", "Click an element {ref | selector, tab?}"),
    (
        "type",
        "Type into a field {text, ref | selector, clear?, submit?, tab?}",
    ),
    ("press", "Press a key {key, tab?}"),
    (
        "scroll",
        "Scroll the page {direction: up|down, amount?, tab?}",
    ),
    (
        "wait_for",
        "Wait for text or a selector {text | selector, timeout_ms?, tab?}",
    ),
    ("screenshot", "Capture the visible page as PNG {tab?}"),
    ("eval", "Evaluate JavaScript in the page {expression, tab?}"),
];

pub fn handle(command: &str, args: &Value) -> Result<Value, String> {
    match command {
        "help" => Ok(json!(
            COMMANDS
                .iter()
                .map(|(n, d)| json!({ "command": n, "description": d }))
                .collect::<Vec<_>>()
        )),
        "tabs" => call_on_ui(controller::agent_tabs),
        "open" => {
            let url = str_arg(args, "url")?;
            let background = args
                .get("background")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let tab = call_on_ui(move || controller::agent_open(&url, background))??;
            if !background {
                wait_for_load(tab)?;
            }
            Ok(json!({ "tab": tab }))
        }
        "activate" => {
            let tab = tab_arg(args)?;
            call_on_ui(move || controller::agent_activate(tab))?;
            Ok(json!({ "tab": tab }))
        }
        "close" => {
            let tab = tab_arg(args)?;
            call_on_ui(move || controller::agent_close(tab))?;
            Ok(json!({ "closed": tab }))
        }
        "navigate" => {
            let tab = tab_arg(args)?;
            let url = str_arg(args, "url")?;
            call_on_ui(move || controller::agent_navigate(tab, &url))??;
            wait_for_load(tab)?;
            page_info(tab)
        }
        "back" | "forward" | "reload" => {
            let tab = tab_arg(args)?;
            let expression = match command {
                "back" => "history.back()",
                "forward" => "history.forward()",
                _ => "location.reload()",
            };
            evaluate(tab, expression)?;
            wait_for_load(tab)?;
            page_info(tab)
        }
        "read" => {
            let tab = tab_arg(args)?;
            let max = args
                .get("max_chars")
                .and_then(Value::as_u64)
                .unwrap_or(20_000);
            evaluate(tab, &format!("({READ_JS})({max})"))
        }
        "snapshot" => {
            let tab = tab_arg(args)?;
            evaluate(tab, SNAPSHOT_JS)
        }
        "click" => {
            let tab = foreground_tab(args)?;
            let target = element_target(args)?;
            let mut session = open_session(tab)?;
            let point = element_center(&mut session, &target)?;
            click_at(&mut session, point)?;
            sleep(Duration::from_millis(300));
            drop(session);
            page_info(tab)
        }
        "type" => {
            let tab = foreground_tab(args)?;
            let text = str_arg(args, "text")?;
            let clear = args.get("clear").and_then(Value::as_bool).unwrap_or(true);
            let submit = args.get("submit").and_then(Value::as_bool).unwrap_or(false);
            let mut session = open_session(tab)?;
            if let Ok(target) = element_target(args) {
                let point = element_center(&mut session, &target)?;
                click_at(&mut session, point)?;
                if clear {
                    run(&mut session, &format!("({CLEAR_JS})({})", json!(target)))?;
                }
            }
            session.call("Input.insertText", json!({ "text": text }), CALL_TIMEOUT)?;
            if submit {
                press_key(&mut session, "Enter")?;
                sleep(Duration::from_millis(500));
            }
            drop(session);
            if submit {
                wait_for_load(tab)?;
            }
            page_info(tab)
        }
        "press" => {
            let tab = foreground_tab(args)?;
            let key = str_arg(args, "key")?;
            let mut session = open_session(tab)?;
            press_key(&mut session, &key)?;
            Ok(json!({ "pressed": key }))
        }
        "scroll" => {
            let tab = tab_arg(args)?;
            let amount = args.get("amount").and_then(Value::as_i64).unwrap_or(600);
            let sign = if args.get("direction").and_then(Value::as_str) == Some("up") {
                -1
            } else {
                1
            };
            evaluate(
                tab,
                &format!(
                    "(window.scrollBy(0, {}), {{scrollY: Math.round(scrollY), height: document.documentElement.scrollHeight}})",
                    sign * amount
                ),
            )
        }
        "wait_for" => {
            let tab = tab_arg(args)?;
            let timeout = Duration::from_millis(
                args.get("timeout_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(10_000),
            );
            let check = match (
                args.get("text").and_then(Value::as_str),
                args.get("selector").and_then(Value::as_str),
            ) {
                (Some(text), _) => format!(
                    "document.body && document.body.innerText.includes({})",
                    json!(text)
                ),
                (_, Some(selector)) => format!("!!document.querySelector({})", json!(selector)),
                _ => return Err("give text or selector".into()),
            };
            let start = Instant::now();
            while start.elapsed() < timeout {
                if evaluate(tab, &check)? == json!(true) {
                    return Ok(json!({ "found": true }));
                }
                sleep(Duration::from_millis(250));
            }
            Err("timed out waiting".into())
        }
        "screenshot" => {
            let tab = foreground_tab(args)?;
            sleep(Duration::from_millis(150));
            let mut session = open_session(tab)?;
            let result = session.call(
                "Page.captureScreenshot",
                json!({ "format": "png" }),
                CALL_TIMEOUT,
            )?;
            Ok(
                json!({ "mime": "image/png", "data": result.get("data").cloned().unwrap_or_default() }),
            )
        }
        "eval" => {
            let tab = tab_arg(args)?;
            let expression = str_arg(args, "expression")?;
            evaluate(tab, &expression)
        }
        _ => Err(format!("unknown command '{command}'")),
    }
}

fn str_arg(args: &Value, name: &str) -> Result<String, String> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("missing '{name}'"))
}

/// The tab named in `args`, or the active one; loaded if it wasn't.
fn tab_arg(args: &Value) -> Result<TabId, String> {
    let tab = match args.get("tab") {
        Some(Value::Number(n)) => n.as_u64().ok_or("bad tab id")?,
        Some(Value::String(s)) => s.parse().map_err(|_| "bad tab id")?,
        _ => call_on_ui(controller::agent_active_tab)?.ok_or("no tab is open")?,
    };
    call_on_ui(move || controller::agent_ensure_loaded(tab))??;
    Ok(tab)
}

/// Like `tab_arg`, but also shows the tab: input and screenshots need it on screen.
fn foreground_tab(args: &Value) -> Result<TabId, String> {
    let tab = tab_arg(args)?;
    call_on_ui(move || controller::agent_activate(tab))?;
    Ok(tab)
}

fn element_target(args: &Value) -> Result<String, String> {
    if let Some(r) = args.get("ref").and_then(Value::as_str) {
        return Ok(format!("[data-circle-ref=\"{}\"]", r.replace('"', "")));
    }
    args.get("selector")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "give a ref (from snapshot) or a selector".into())
}

/// Opens a session, retrying briefly while a new tab's browser starts.
fn open_session(tab: TabId) -> Result<Session, String> {
    // Private tabs are off limits to agents.
    if !crate::remote::call_on_ui(move || crate::controller::agent_may_use(tab))? {
        return Err(format!("no tab {tab}"));
    }
    let start = Instant::now();
    loop {
        match Session::open(tab) {
            Ok(session) => return Ok(session),
            Err(err) if start.elapsed() > Duration::from_secs(5) => return Err(err),
            Err(_) => sleep(Duration::from_millis(100)),
        }
    }
}

fn run(session: &mut Session, expression: &str) -> Result<Value, String> {
    let result = session.call(
        "Runtime.evaluate",
        json!({ "expression": expression, "returnByValue": true, "awaitPromise": true }),
        CALL_TIMEOUT,
    )?;
    if let Some(details) = result.get("exceptionDetails") {
        let text = details
            .pointer("/exception/description")
            .or_else(|| details.get("text"))
            .and_then(Value::as_str)
            .unwrap_or("script error");
        return Err(text.to_owned());
    }
    Ok(result
        .pointer("/result/value")
        .cloned()
        .unwrap_or(Value::Null))
}

fn evaluate(tab: TabId, expression: &str) -> Result<Value, String> {
    let mut session = open_session(tab)?;
    run(&mut session, expression)
}

fn page_info(tab: TabId) -> Result<Value, String> {
    let info = evaluate(tab, "({url: location.href, title: document.title})")?;
    Ok(json!({ "tab": tab, "url": info["url"], "title": info["title"] }))
}

fn wait_for_load(tab: TabId) -> Result<(), String> {
    // Give a navigation a moment to replace the old document first.
    sleep(Duration::from_millis(300));
    let start = Instant::now();
    while start.elapsed() < LOAD_TIMEOUT {
        if let Ok(state) = evaluate(tab, "document.readyState")
            && state == "complete"
        {
            return Ok(());
        }
        sleep(Duration::from_millis(200));
    }
    Ok(()) // Slow pages are still usable; the agent can wait_for content.
}

fn element_center(session: &mut Session, selector: &str) -> Result<(f64, f64), String> {
    let point = run(session, &format!("({CENTER_JS})({})", json!(selector)))?;
    match (
        point.get("x").and_then(Value::as_f64),
        point.get("y").and_then(Value::as_f64),
    ) {
        (Some(x), Some(y)) => Ok((x, y)),
        _ => Err(point
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("element not found; take a new snapshot")
            .to_owned()),
    }
}

fn click_at(session: &mut Session, (x, y): (f64, f64)) -> Result<(), String> {
    for (kind, buttons) in [("mouseMoved", 0), ("mousePressed", 1), ("mouseReleased", 0)] {
        let mut params = json!({ "type": kind, "x": x, "y": y, "buttons": buttons });
        if kind != "mouseMoved" {
            params["button"] = json!("left");
            params["clickCount"] = json!(1);
        }
        session.call("Input.dispatchMouseEvent", params, CALL_TIMEOUT)?;
    }
    Ok(())
}

fn press_key(session: &mut Session, key: &str) -> Result<(), String> {
    let (code, key_code, text) = match key {
        "Enter" => ("Enter", 13, "\r"),
        "Tab" => ("Tab", 9, ""),
        "Escape" => ("Escape", 27, ""),
        "Backspace" => ("Backspace", 8, ""),
        "ArrowDown" => ("ArrowDown", 40, ""),
        "ArrowUp" => ("ArrowUp", 38, ""),
        "ArrowLeft" => ("ArrowLeft", 37, ""),
        "ArrowRight" => ("ArrowRight", 39, ""),
        "PageDown" => ("PageDown", 34, ""),
        "PageUp" => ("PageUp", 33, ""),
        "Space" | " " => ("Space", 32, " "),
        other => return Err(format!("unsupported key '{other}'")),
    };
    let key_name = if key == "Space" { " " } else { key };
    let mut down = json!({ "type": "keyDown", "key": key_name, "code": code, "windowsVirtualKeyCode": key_code });
    if !text.is_empty() {
        down["text"] = json!(text);
    }
    session.call("Input.dispatchKeyEvent", down, CALL_TIMEOUT)?;
    session.call(
        "Input.dispatchKeyEvent",
        json!({ "type": "keyUp", "key": key_name, "code": code, "windowsVirtualKeyCode": key_code }),
        CALL_TIMEOUT,
    )?;
    Ok(())
}

const READ_JS: &str = r#"(max) => {
  const text = (document.body ? document.body.innerText : "").replace(/\n{3,}/g, "\n\n");
  return { url: location.href, title: document.title, text: text.slice(0, max), truncated: text.length > max };
}"#;

const SNAPSHOT_JS: &str = r#"(() => {
  document.querySelectorAll("[data-circle-ref]").forEach((e) => e.removeAttribute("data-circle-ref"));
  const selector = 'a[href],button,input:not([type=hidden]),textarea,select,summary,[contenteditable=""],[contenteditable=true],[role=button],[role=link],[role=checkbox],[role=radio],[role=tab],[role=menuitem],[role=option],[role=switch],[role=combobox],[role=textbox],[onclick],[tabindex]:not([tabindex="-1"])';
  const out = [];
  let n = 0;
  for (const el of document.querySelectorAll(selector)) {
    const r = el.getBoundingClientRect();
    const style = getComputedStyle(el);
    if (r.width < 1 || r.height < 1 || style.visibility === "hidden" || style.display === "none" || el.disabled) continue;
    const ref = "e" + ++n;
    el.setAttribute("data-circle-ref", ref);
    const label = (el.getAttribute("aria-label") || el.innerText || el.value || el.placeholder || el.title || el.getAttribute("alt") || "")
      .trim().replace(/\s+/g, " ").slice(0, 100);
    const item = { ref, tag: el.tagName.toLowerCase(), label };
    if (el.getAttribute("role")) item.role = el.getAttribute("role");
    if (el.type && el.tagName !== "BUTTON") item.type = el.type;
    if (el.href) item.href = el.href;
    if (el.checked) item.checked = true;
    if (r.bottom < 0 || r.top > innerHeight) item.offscreen = true;
    out.push(item);
    if (n >= 400) break;
  }
  return { url: location.href, title: document.title, elements: out };
})()"#;

const CENTER_JS: &str = r#"(selector) => {
  const el = document.querySelector(selector);
  if (!el) return { error: "element not found; take a new snapshot" };
  el.scrollIntoView({ block: "center", inline: "center" });
  const r = el.getBoundingClientRect();
  return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
}"#;

const CLEAR_JS: &str = r#"(selector) => {
  const el = document.querySelector(selector);
  if (!el) return false;
  if ("value" in el) {
    const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    const setter = Object.getOwnPropertyDescriptor(proto, "value")?.set;
    setter ? setter.call(el, "") : (el.value = "");
    el.dispatchEvent(new Event("input", { bubbles: true }));
  } else if (el.isContentEditable) {
    el.textContent = "";
  }
  return true;
}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn element_targets_prefer_refs() {
        assert_eq!(
            element_target(&json!({ "ref": "e3" })).unwrap(),
            "[data-circle-ref=\"e3\"]"
        );
        assert_eq!(
            element_target(&json!({ "ref": "e\"]x" })).unwrap(),
            "[data-circle-ref=\"e]x\"]"
        );
        assert_eq!(element_target(&json!({ "selector": "#q" })).unwrap(), "#q");
        assert!(element_target(&json!({})).is_err());
    }

    #[test]
    fn every_command_is_documented() {
        let names: Vec<_> = COMMANDS.iter().map(|(name, _)| *name).collect();
        for name in [
            "tabs",
            "open",
            "snapshot",
            "click",
            "type",
            "read",
            "screenshot",
        ] {
            assert!(names.contains(&name), "{name} missing from COMMANDS");
        }
    }
}
