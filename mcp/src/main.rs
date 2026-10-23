//! `circle-mcp`: a Model Context Protocol server (stdio) for the Circle browser.
//!
//! It exposes Circle's agent API as MCP tools. Circle must be running with
//! agent control allowed (command bar › "Allow Agent Control"), which writes
//! the local port and token to `agent.json` in Circle's profile directory.
//!
//! Register it with Claude Code:
//!   claude mcp add circle -- /Applications/Circle.app/Contents/MacOS/circle-mcp

use serde_json::{Value, json};
use std::{
    io::{BufRead, Read, Write},
    net::TcpStream,
    path::PathBuf,
    time::Duration,
};

const PROTOCOL_VERSION: &str = "2025-06-18";

const INSTRUCTIONS: &str = "Controls the user's Circle web browser: their real tabs, \
logins and sessions. Typical loop: circle_tabs or circle_open, then circle_snapshot to \
get element refs, then circle_click / circle_type with those refs, then circle_read or \
circle_screenshot to check the result. Take a new snapshot after the page changes. \
Ask the user before submitting forms that buy, send, post or delete anything.";

fn tab_prop() -> Value {
    json!({ "type": "integer", "description": "Tab id from circle_tabs. Defaults to the active tab." })
}

fn tools() -> Value {
    let tab = tab_prop();
    let target = json!({
        "ref": { "type": "string", "description": "Element ref from circle_snapshot, e.g. \"e12\"." },
        "selector": { "type": "string", "description": "CSS selector, if there is no ref." },
        "tab": tab,
    });
    let mut type_props = target.clone();
    type_props["text"] = json!({ "type": "string", "description": "Text to type." });
    type_props["clear"] =
        json!({ "type": "boolean", "description": "Clear the field first (default true)." });
    type_props["submit"] =
        json!({ "type": "boolean", "description": "Press Enter afterwards (default false)." });

    let tool =
        |name: &str, description: &str, properties: Value, required: &[&str], read_only: bool| {
            json!({
                "name": format!("circle_{name}"),
                "description": description,
                "inputSchema": { "type": "object", "properties": properties, "required": required },
                "annotations": { "readOnlyHint": read_only },
            })
        };
    json!([
        tool(
            "tabs",
            "List the browser's spaces and tabs, with ids, titles and URLs, and which tab is active.",
            json!({}),
            &[],
            true
        ),
        tool(
            "open",
            "Open a URL (or a search for any text) in a new tab and wait for it to load. Returns the new tab id.",
            json!({ "url": { "type": "string" }, "background": { "type": "boolean", "description": "Open without switching to it." } }),
            &["url"],
            false
        ),
        tool(
            "navigate",
            "Load a URL or search in a tab and wait for it to load.",
            json!({ "url": { "type": "string" }, "tab": tab }),
            &["url"],
            false
        ),
        tool(
            "activate",
            "Switch the browser to a tab.",
            json!({ "tab": tab }),
            &["tab"],
            false
        ),
        tool(
            "close",
            "Close a tab.",
            json!({ "tab": tab }),
            &["tab"],
            false
        ),
        tool(
            "back",
            "Go back in a tab's history.",
            json!({ "tab": tab }),
            &[],
            false
        ),
        tool(
            "forward",
            "Go forward in a tab's history.",
            json!({ "tab": tab }),
            &[],
            false
        ),
        tool("reload", "Reload a tab.", json!({ "tab": tab }), &[], false),
        tool(
            "read",
            "Read the visible text of a page.",
            json!({ "tab": tab, "max_chars": { "type": "integer", "description": "Default 20000." } }),
            &[],
            true
        ),
        tool(
            "snapshot",
            "List the page's interactive elements (links, buttons, inputs...) with refs to use in circle_click and circle_type. Refs change when the page changes.",
            json!({ "tab": tab }),
            &[],
            true
        ),
        tool(
            "click",
            "Click an element (real mouse events). Brings the tab to the front.",
            target.clone(),
            &[],
            false
        ),
        tool(
            "type",
            "Type into a field (real keyboard input). Clicks the field first when ref or selector is given.",
            type_props,
            &["text"],
            false
        ),
        tool(
            "press",
            "Press a key: Enter, Tab, Escape, Backspace, ArrowUp/Down/Left/Right, PageUp/Down, Space.",
            json!({ "key": { "type": "string" }, "tab": tab }),
            &["key"],
            false
        ),
        tool(
            "scroll",
            "Scroll a page up or down.",
            json!({ "direction": { "type": "string", "enum": ["up", "down"] }, "amount": { "type": "integer", "description": "Pixels, default 600." }, "tab": tab }),
            &[],
            false
        ),
        tool(
            "wait_for",
            "Wait until text or a CSS selector appears on the page.",
            json!({ "text": { "type": "string" }, "selector": { "type": "string" }, "timeout_ms": { "type": "integer" }, "tab": tab }),
            &[],
            true
        ),
        tool(
            "screenshot",
            "Take a PNG screenshot of what the tab shows. Brings the tab to the front.",
            json!({ "tab": tab }),
            &[],
            true
        ),
        tool(
            "eval",
            "Evaluate a JavaScript expression in the page and return its JSON value.",
            json!({ "expression": { "type": "string" }, "tab": tab }),
            &["expression"],
            false
        ),
    ])
}

fn profile_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("CIRCLE_PROFILE_DIR") {
        return PathBuf::from(dir);
    }
    let base = dirs::data_local_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(std::env::temp_dir);
    base.join(if cfg!(target_os = "linux") {
        "circle"
    } else {
        "Circle"
    })
}

fn connection() -> Result<(u16, String), String> {
    let path = profile_dir().join("agent.json");
    let text = std::fs::read_to_string(&path).map_err(|_| {
        "Circle isn't reachable. Make sure Circle is running and agent control is on \
         (Circle's command bar, ⌘T › \"Allow Agent Control\")."
            .to_owned()
    })?;
    let info: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let port = info["port"].as_u64().ok_or("agent.json has no port")? as u16;
    let token = info["token"]
        .as_str()
        .ok_or("agent.json has no token")?
        .to_owned();
    Ok((port, token))
}

/// POSTs one command to Circle's agent API.
fn call_circle(command: &str, args: &Value) -> Result<Value, String> {
    let (port, token) = connection()?;
    let mut stream = TcpStream::connect(("127.0.0.1", port)).map_err(|_| {
        "Circle isn't running (or was restarted; turn agent control on again).".to_owned()
    })?;
    stream.set_read_timeout(Some(Duration::from_secs(90))).ok();
    let body = args.to_string();
    write!(
        stream,
        "POST /api/{command} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .map_err(|e| e.to_string())?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|e| e.to_string())?;
    let response = String::from_utf8_lossy(&response);
    let (head, body) = response
        .split_once("\r\n\r\n")
        .ok_or("bad response from Circle")?;
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(500);
    let value: Value = serde_json::from_str(body).unwrap_or(Value::String(body.to_owned()));
    if status == 200 {
        Ok(value)
    } else {
        Err(value["error"]
            .as_str()
            .unwrap_or("request failed")
            .to_owned())
    }
}

fn call_tool(name: &str, args: &Value) -> Value {
    let Some(command) = name.strip_prefix("circle_") else {
        return tool_error(format!("unknown tool {name}"));
    };
    match call_circle(command, args) {
        Ok(result) if command == "screenshot" => json!({
            "content": [{ "type": "image", "data": result["data"], "mimeType": "image/png" }],
        }),
        Ok(result) => json!({
            "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default() }],
            "structuredContent": if result.is_object() { result } else { json!({ "result": result }) },
        }),
        Err(error) => tool_error(error),
    }
}

fn tool_error(message: String) -> Value {
    json!({ "content": [{ "type": "text", "text": message }], "isError": true })
}

fn handle(request: &Value) -> Option<Value> {
    let id = request.get("id")?.clone(); // notifications get no reply
    let method = request["method"].as_str().unwrap_or_default();
    let result = match method {
        "initialize" => json!({
            "protocolVersion": request["params"]["protocolVersion"].as_str().unwrap_or(PROTOCOL_VERSION),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "circle", "title": "Circle browser", "version": env!("CARGO_PKG_VERSION") },
            "instructions": INSTRUCTIONS,
        }),
        "ping" => json!({}),
        "tools/list" => json!({ "tools": tools() }),
        "tools/call" => {
            let params = &request["params"];
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            call_tool(params["name"].as_str().unwrap_or_default(), &args)
        }
        _ => {
            return Some(json!({
                "jsonrpc": "2.0", "id": id,
                "error": { "code": -32601, "message": format!("method not found: {method}") },
            }));
        }
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn main() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(request) => handle(&request),
            Err(err) => Some(json!({
                "jsonrpc": "2.0", "id": null,
                "error": { "code": -32700, "message": err.to_string() },
            })),
        };
        if let Some(reply) = reply {
            let _ = writeln!(stdout, "{reply}");
            let _ = stdout.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_tools_with_schemas() {
        let reply = handle(&json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" })).unwrap();
        let tools = reply["result"]["tools"].as_array().unwrap();
        assert!(tools.len() >= 15);
        assert!(tools.iter().all(|t| t["inputSchema"]["type"] == "object"));
    }

    #[test]
    fn notifications_get_no_reply() {
        assert!(
            handle(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).is_none()
        );
    }

    #[test]
    fn missing_circle_is_a_tool_error() {
        // SAFETY: tests in this module don't read the variable concurrently.
        unsafe { std::env::set_var("CIRCLE_PROFILE_DIR", "/nonexistent/circle-profile") };
        let result = call_tool("circle_tabs", &json!({}));
        assert_eq!(result["isError"], true);
    }
}
