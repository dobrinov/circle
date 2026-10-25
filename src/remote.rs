//! A local debugging and control channel.
//!
//! Circle runs a small HTTP/WebSocket server on `127.0.0.1` (random port)
//! that speaks the Chrome DevTools Protocol by bridging to CEF's
//! `send_dev_tools_message`, without enabling Chromium's unauthenticated
//! remote-debugging server. It serves:
//!
//! - `ws://…/devtools/<token>/page/<tab>`: CDP for one tab. Circle's own
//!   DevTools window connects here with a per-launch secret.
//! - `GET /json/list`: the tabs, for CDP clients.
//! - `POST /api/<command>`: the agent API (see `agent.rs`), only when the user
//!   has allowed agent control. Its token is written to `agent.json` in the
//!   profile directory, readable only by the user.
//!
//! Several clients can attach to one tab: request ids are rewritten so each
//! client gets its own responses, and events go to everyone attached.

use crate::{agent, controller, model::TabId};
use cef::*;
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU16, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    time::Duration,
};
use tungstenite::{Message, WebSocket, protocol::Role as WsRole};

static PORT: AtomicU16 = AtomicU16::new(0);
static AGENT_ENABLED: AtomicBool = AtomicBool::new(false);
/// Used only by Circle's own DevTools windows; never leaves the process.
static DEVTOOLS_TOKEN: OnceLock<String> = OnceLock::new();
/// Handed to agents through `agent.json` while agent control is on. A new
/// one each time it is turned on, so a token an agent kept stops working.
static AGENT_TOKEN: Mutex<String> = Mutex::new(String::new());
/// Bumped whenever agent control changes, which ends agents' open CDP
/// connections.
static AGENT_GENERATION: AtomicU64 = AtomicU64::new(0);
static AGENT_FILE: OnceLock<PathBuf> = OnceLock::new();
/// Connections being served at once, so a flood can't exhaust threads.
static CONNECTIONS: AtomicUsize = AtomicUsize::new(0);
const MAX_CONNECTIONS: usize = 64;

fn random_token() -> String {
    let mut bytes = [0u8; 24];
    getrandom::fill(&mut bytes).expect("no system randomness");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn devtools_token() -> &'static str {
    DEVTOOLS_TOKEN.get_or_init(random_token)
}

/// Constant-time comparison so tokens can't be guessed byte by byte.
fn token_eq(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

fn is_devtools_token(token: &str) -> bool {
    token_eq(token, devtools_token())
}

fn is_agent_token(token: &str) -> bool {
    let agent = AGENT_TOKEN.lock().unwrap();
    AGENT_ENABLED.load(Ordering::SeqCst) && !agent.is_empty() && token_eq(token, &agent)
}

pub fn port() -> u16 {
    PORT.load(Ordering::SeqCst)
}

/// Starts the server. Called once on the UI thread at startup.
pub fn start(profile_dir: &Path) {
    let _ = AGENT_FILE.set(profile_dir.join("agent.json"));
    let listener = match TcpListener::bind(("127.0.0.1", 0)) {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("circle: control server unavailable: {err}");
            return;
        }
    };
    PORT.store(
        listener.local_addr().map(|a| a.port()).unwrap_or(0),
        Ordering::SeqCst,
    );
    std::thread::Builder::new()
        .name("circle-control".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                if CONNECTIONS.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                    CONNECTIONS.fetch_sub(1, Ordering::SeqCst);
                    continue;
                }
                std::thread::spawn(move || {
                    if let Err(err) = handle_connection(stream) {
                        eprintln!("circle: control connection: {err}");
                    }
                    CONNECTIONS.fetch_sub(1, Ordering::SeqCst);
                });
            }
        })
        .expect("failed to start control server");
}

/// Turns agent control on or off. While on, `agent.json` (mode 0600) holds
/// the port and token so local agents such as `circle-mcp` can connect.
pub fn set_agent_access(enabled: bool) {
    let token = {
        let mut agent = AGENT_TOKEN.lock().unwrap();
        *agent = if enabled {
            random_token()
        } else {
            String::new()
        };
        agent.clone()
    };
    AGENT_ENABLED.store(enabled, Ordering::SeqCst);
    AGENT_GENERATION.fetch_add(1, Ordering::SeqCst);
    let Some(path) = AGENT_FILE.get() else {
        return;
    };
    if !enabled || port() == 0 {
        let _ = std::fs::remove_file(path);
        return;
    }
    let info = json!({ "port": port(), "token": token, "pid": std::process::id() });
    if let Err(err) = write_private(path, info.to_string().as_bytes()) {
        eprintln!("circle: failed to write {}: {err}", path.display());
    }
}

fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(contents)
}

/// The URL of Chrome's DevTools front-end attached to `tab` through the bridge.
pub fn devtools_url(tab: TabId) -> Option<String> {
    let port = port();
    (port != 0).then(|| {
        format!(
            "devtools://devtools/bundled/devtools_app.html?ws=127.0.0.1:{port}/devtools/{}/page/{tab}",
            devtools_token()
        )
    })
}

/// Points to inspect once a DevTools window for the tab connects.
static PENDING_INSPECT: Mutex<Vec<(TabId, i32, i32)>> = Mutex::new(Vec::new());

/// After the next DevTools window for `tab` connects, reveal the element at
/// `point` (page coordinates) in its Elements panel.
pub fn inspect_when_ready(tab: TabId, point: Point) {
    PENDING_INSPECT
        .lock()
        .unwrap()
        .push((tab, point.x, point.y));
}

fn take_pending_inspect(tab: TabId) -> Option<(i32, i32)> {
    let mut pending = PENDING_INSPECT.lock().unwrap();
    let index = pending.iter().position(|(t, ..)| *t == tab)?;
    let (_, x, y) = pending.remove(index);
    Some((x, y))
}

/// Asks the page for the node at a point, then tells the DevTools client to
/// reveal it, the same event DevTools gets from its own "inspect" mode.
fn reveal_node(tab: TabId, (x, y): (i32, i32), devtools: Sender<String>) {
    std::thread::spawn(move || {
        // Give the front-end time to load and enable its panels.
        std::thread::sleep(Duration::from_millis(1500));
        let Ok(mut session) = Session::open(tab) else {
            return;
        };
        let timeout = Duration::from_secs(5);
        let _ = session.call("DOM.getDocument", json!({ "depth": 0 }), timeout);
        let node = session.call("DOM.getNodeForLocation", json!({ "x": x, "y": y }), timeout);
        if let Some(id) = node.ok().and_then(|n| n.get("backendNodeId").cloned()) {
            let event = json!({ "method": "Overlay.inspectNodeRequested", "params": { "backendNodeId": id } });
            let _ = devtools.send(event.to_string());
        }
    });
}

// ----- running code on the UI thread -----------------------------------------

type Job = Box<dyn FnOnce() + Send>;

wrap_task! {
    struct UiJob {
        job: Arc<Mutex<Option<Job>>>,
    }

    impl Task {
        fn execute(&self) {
            if let Some(job) = self.job.lock().unwrap().take() {
                job();
            }
        }
    }
}

pub fn run_on_ui(job: impl FnOnce() + Send + 'static) {
    let mut task = UiJob::new(Arc::new(Mutex::new(Some(Box::new(job) as Job))));
    post_task(ThreadId::UI, Some(&mut task));
}

/// Runs `job` on the UI thread and waits for its result.
pub fn call_on_ui<T: Send + 'static>(
    job: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    let (tx, rx) = mpsc::channel();
    run_on_ui(move || {
        let _ = tx.send(job());
    });
    rx.recv_timeout(Duration::from_secs(10))
        .map_err(|_| "the browser did not respond".to_owned())
}

// ----- the CDP bridge (UI thread only) ---------------------------------------

struct Attachment {
    _registration: Option<Registration>,
    clients: HashMap<u64, Sender<String>>,
}

#[derive(Default)]
struct Bridge {
    attachments: HashMap<i32, Attachment>,
    /// Rewritten request id → (client, the client's original id).
    pending: HashMap<i64, (u64, Value)>,
    next_id: i64,
    next_client: u64,
}

thread_local! {
    static BRIDGE: RefCell<Bridge> = RefCell::new(Bridge::default());
}

/// Attaches a client to a tab's browser. Returns the client id and browser id.
fn attach(tab: TabId, tx: Sender<String>) -> Option<(u64, i32)> {
    let browser = controller::tab_browser(tab)?;
    let browser_id = browser.identifier();
    BRIDGE.with(|bridge| {
        let mut bridge = bridge.borrow_mut();
        bridge.next_client += 1;
        let client = bridge.next_client;
        let attachment = bridge.attachments.entry(browser_id).or_insert_with(|| {
            let mut observer = BridgeObserver::new();
            let registration = browser
                .host()
                .and_then(|host| host.add_dev_tools_message_observer(Some(&mut observer)));
            Attachment {
                _registration: registration,
                clients: HashMap::new(),
            }
        });
        attachment.clients.insert(client, tx);
        Some((client, browser_id))
    })
}

fn detach(browser_id: i32, client: u64) {
    BRIDGE.with(|bridge| {
        let mut bridge = bridge.borrow_mut();
        bridge.pending.retain(|_, (c, _)| *c != client);
        if let Some(attachment) = bridge.attachments.get_mut(&browser_id) {
            attachment.clients.remove(&client);
            if attachment.clients.is_empty() {
                // Dropping the registration removes the observer.
                bridge.attachments.remove(&browser_id);
            }
        }
    });
}

/// Forwards one CDP request from a client, rewriting its id.
fn forward(tab: TabId, client: u64, text: &str) -> Result<(), String> {
    let mut message: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let object = message.as_object_mut().ok_or("expected a JSON object")?;
    let original = object.get("id").cloned().ok_or("missing id")?;
    let id = BRIDGE.with(|bridge| {
        let mut bridge = bridge.borrow_mut();
        bridge.next_id += 1;
        let id = bridge.next_id;
        bridge.pending.insert(id, (client, original));
        id
    });
    object.insert("id".into(), json!(id));
    let host = controller::tab_browser(tab)
        .and_then(|b| b.host())
        .ok_or("the tab is gone")?;
    let bytes = message.to_string().into_bytes();
    host.send_dev_tools_message(Some(&bytes));
    Ok(())
}

fn deliver(browser_id: i32, message: &[u8]) {
    let Ok(mut value) = serde_json::from_slice::<Value>(message) else {
        return;
    };
    BRIDGE.with(|bridge| {
        let mut bridge = bridge.borrow_mut();
        let id = value.get("id").and_then(Value::as_i64);
        match id {
            Some(id) => {
                let Some((client, original)) = bridge.pending.remove(&id) else {
                    return;
                };
                value["id"] = original;
                let target = bridge
                    .attachments
                    .get(&browser_id)
                    .and_then(|a| a.clients.get(&client));
                if let Some(tx) = target {
                    let _ = tx.send(value.to_string());
                }
            }
            None => {
                if let Some(attachment) = bridge.attachments.get(&browser_id) {
                    let text = value.to_string();
                    for tx in attachment.clients.values() {
                        let _ = tx.send(text.clone());
                    }
                }
            }
        }
    });
}

wrap_dev_tools_message_observer! {
    struct BridgeObserver;

    impl DevToolsMessageObserver {
        fn on_dev_tools_message(&self, browser: Option<&mut Browser>, message: Option<&[u8]>) -> std::os::raw::c_int {
            if let (Some(browser), Some(message)) = (browser, message) {
                deliver(browser.identifier(), message);
            }
            1
        }
    }
}

/// A CDP session used from a worker thread (the agent API).
pub struct Session {
    tab: TabId,
    client: u64,
    browser_id: i32,
    rx: Receiver<String>,
    next_id: i64,
    /// Events that arrived while waiting for a response.
    events: std::collections::VecDeque<Value>,
}

impl Session {
    pub fn open(tab: TabId) -> Result<Session, String> {
        let (tx, rx) = mpsc::channel();
        let (client, browser_id) =
            call_on_ui(move || attach(tab, tx))?.ok_or("that tab isn't loaded")?;
        Ok(Session {
            tab,
            client,
            browser_id,
            rx,
            next_id: 0,
            events: std::collections::VecDeque::new(),
        })
    }

    /// Sends a CDP command without waiting for its result.
    pub fn send(&mut self, method: &str, params: Value) -> Result<(), String> {
        self.next_id += 1;
        let text = json!({ "id": self.next_id, "method": method, "params": params }).to_string();
        let (tab, client) = (self.tab, self.client);
        call_on_ui(move || forward(tab, client, &text))?
    }

    /// The next CDP event (a message with a `method`), waiting up to
    /// `timeout`. `Err` means the tab went away.
    pub fn next_event(&mut self, timeout: Duration) -> Result<Option<Value>, String> {
        if let Some(event) = self.events.pop_front() {
            return Ok(Some(event));
        }
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let text = match self.rx.recv_timeout(left) {
                Ok(text) => text,
                Err(mpsc::RecvTimeoutError::Timeout) => return Ok(None),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("the tab closed".to_owned());
                }
            };
            if let Ok(message) = serde_json::from_str::<Value>(&text)
                && message.get("method").is_some()
            {
                return Ok(Some(message));
            }
        }
    }

    /// Sends a CDP command and waits for its result.
    pub fn call(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        let text = json!({ "id": id, "method": method, "params": params }).to_string();
        let (tab, client) = (self.tab, self.client);
        call_on_ui(move || forward(tab, client, &text))??;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let text = self
                .rx
                .recv_timeout(left)
                .map_err(|_| format!("{method} timed out"))?;
            let Ok(message) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            if message.get("id").and_then(Value::as_i64) != Some(id) {
                if message.get("method").is_some() {
                    self.events.push_back(message);
                }
                continue; // another response
            }
            if let Some(error) = message.get("error") {
                return Err(error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("CDP error")
                    .to_owned());
            }
            return Ok(message.get("result").cloned().unwrap_or(Value::Null));
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let (browser_id, client) = (self.browser_id, self.client);
        run_on_ui(move || detach(browser_id, client));
    }
}

// ----- HTTP and WebSocket handling -------------------------------------------

struct Request {
    method: String,
    path: String,
    query: HashMap<String, String>,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl Request {
    fn bearer(&self) -> Option<&str> {
        self.headers
            .get("authorization")
            .and_then(|v| v.strip_prefix("Bearer "))
            .or_else(|| self.query.get("token").map(String::as_str))
    }
}

fn read_request(stream: &mut BufReader<TcpStream>) -> std::io::Result<Request> {
    let mut line = String::new();
    stream.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let target = parts.next().unwrap_or("/").to_owned();
    let mut headers = HashMap::new();
    loop {
        line.clear();
        stream.read_line(&mut line)?;
        let header = line.trim_end();
        if header.is_empty() {
            break;
        }
        if headers.len() > 100 {
            return Err(std::io::Error::other("too many headers"));
        }
        if let Some((name, value)) = header.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    let query = url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect();
    Ok(Request {
        method,
        path: path.to_owned(),
        query,
        headers,
        body: Vec::new(),
    })
}

/// Reads the request body; only once the request is authorized.
fn read_body(stream: &mut impl Read, request: &mut Request) -> std::io::Result<()> {
    let length: usize = request
        .headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if length > 8 * 1024 * 1024 {
        return Err(std::io::Error::other("body too large"));
    }
    request.body = vec![0; length];
    stream.read_exact(&mut request.body)
}

fn respond(stream: &mut TcpStream, status: u16, body: &Value) -> std::io::Result<()> {
    let text = body.to_string();
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        _ => "Error",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{text}",
        text.len()
    )
}

fn handle_connection(stream: TcpStream) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request = read_request(&mut reader)?;
    let mut stream = stream;

    // Only accept requests addressed to the loopback interface, which keeps
    // web pages from reaching the server through DNS rebinding.
    let host = request
        .headers
        .get("host")
        .map(String::as_str)
        .unwrap_or("");
    let host_name = host.rsplit_once(':').map_or(host, |(h, _)| h);
    if !matches!(host_name, "127.0.0.1" | "localhost" | "[::1]") {
        return respond(&mut stream, 403, &json!({ "error": "bad host" }));
    }

    let path = request.path.clone();
    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
    let is_upgrade = request
        .headers
        .get("upgrade")
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"));

    match (request.method.as_str(), segments.as_slice()) {
        ("GET", ["devtools", token, "page", tab]) if is_upgrade => {
            if !is_devtools_token(token) && !is_agent_token(token) {
                return respond(&mut stream, 401, &json!({ "error": "bad token" }));
            }
            let Ok(tab) = tab.parse::<TabId>() else {
                return respond(&mut stream, 404, &json!({ "error": "no such tab" }));
            };
            // Agents get only tabs they may use (never private ones, nor
            // Circle's own or local-file pages); DevTools gets any tab.
            let agent = !is_devtools_token(token);
            if agent {
                if !call_on_ui(move || controller::agent_may_use(tab)).unwrap_or(false) {
                    return respond(&mut stream, 404, &json!({ "error": "no such tab" }));
                }
                agent::note_activity();
            }
            serve_cdp(stream, &request, tab, agent)
        }
        ("GET", ["json"] | ["json", "list"]) => {
            let Some(token) = request.bearer().map(str::to_owned) else {
                return respond(&mut stream, 401, &json!({ "error": "missing token" }));
            };
            if !is_devtools_token(&token) && !is_agent_token(&token) {
                return respond(&mut stream, 401, &json!({ "error": "bad token" }));
            }
            let port = port();
            let agent = !is_devtools_token(&token);
            let tabs = call_on_ui(move || controller::tab_summaries(agent)).unwrap_or_default();
            let targets: Vec<Value> = tabs
                .into_iter()
                .map(|t| {
                    json!({
                        "id": t.id.to_string(),
                        "type": "page",
                        "title": t.title,
                        "url": t.url,
                        "webSocketDebuggerUrl": format!("ws://127.0.0.1:{port}/devtools/{token}/page/{}", t.id),
                    })
                })
                .collect();
            respond(&mut stream, 200, &Value::Array(targets))
        }
        ("POST", ["api", command]) => {
            let authorized = request.bearer().is_some_and(is_agent_token);
            if !authorized {
                let error = if AGENT_ENABLED.load(Ordering::SeqCst) {
                    "bad token"
                } else {
                    "agent control is turned off in Circle (command bar › Allow Agent Control)"
                };
                return respond(&mut stream, 401, &json!({ "error": error }));
            }
            read_body(&mut reader, &mut request)?;
            let args: Value = if request.body.is_empty() {
                json!({})
            } else {
                match serde_json::from_slice(&request.body) {
                    Ok(args) => args,
                    Err(err) => {
                        return respond(&mut stream, 400, &json!({ "error": err.to_string() }));
                    }
                }
            };
            agent::note_activity();
            match agent::handle(command, &args) {
                Ok(result) => respond(&mut stream, 200, &result),
                Err(error) => respond(&mut stream, 400, &json!({ "error": error })),
            }
        }
        _ => respond(&mut stream, 404, &json!({ "error": "not found" })),
    }
}

/// Completes the WebSocket handshake and relays CDP messages for one tab.
fn serve_cdp(
    mut stream: TcpStream,
    request: &Request,
    tab: TabId,
    agent: bool,
) -> std::io::Result<()> {
    let generation = AGENT_GENERATION.load(Ordering::SeqCst);
    let Some(key) = request.headers.get("sec-websocket-key") else {
        return respond(
            &mut stream,
            400,
            &json!({ "error": "missing websocket key" }),
        );
    };
    let accept = tungstenite::handshake::derive_accept_key(key.as_bytes());
    write!(
        stream,
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
    )?;

    let (tx, rx) = mpsc::channel::<String>();
    let inspect = take_pending_inspect(tab).map(|point| (point, tx.clone()));
    let Ok(Some((client, browser_id))) = call_on_ui(move || attach(tab, tx)) else {
        return Ok(());
    };
    if let Some((point, tx)) = inspect {
        reveal_node(tab, point, tx);
    }
    stream.set_read_timeout(Some(Duration::from_millis(15)))?;
    let mut socket = WebSocket::from_raw_socket(stream, WsRole::Server, None);
    let result = (|| -> Result<(), tungstenite::Error> {
        loop {
            // Turning agent control off (or on again) ends agents' sessions.
            if agent && AGENT_GENERATION.load(Ordering::SeqCst) != generation {
                let _ = socket.close(None);
                return Ok(());
            }
            while let Ok(text) = rx.try_recv() {
                socket.send(Message::Text(text.into()))?;
            }
            match socket.read() {
                Ok(Message::Text(text)) => {
                    let text = text.to_string();
                    if agent && let Err(refusal) = agent_may_send(&text) {
                        socket.send(Message::Text(refusal.to_string().into()))?;
                        continue;
                    }
                    run_on_ui(move || {
                        if let Err(err) = forward(tab, client, &text) {
                            eprintln!("circle: CDP: {err}");
                        }
                    });
                }
                Ok(Message::Close(_)) => return Ok(()),
                Ok(_) => {}
                Err(tungstenite::Error::Io(err))
                    if matches!(
                        err.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(err) => return Err(err),
            }
        }
    })();
    run_on_ui(move || detach(browser_id, client));
    // A dropped connection is a normal way for a client to leave.
    drop(result);
    Ok(())
}

/// Whether an agent may send this CDP message. Agents drive pages; they
/// may not steer the whole browser, reach other targets or change where
/// downloads go, and may navigate only to web pages. The refusal is the
/// error response to send back.
fn agent_may_send(text: &str) -> Result<(), Value> {
    let message: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    let method = message["method"].as_str().unwrap_or("");
    let refuse = |why: &str| {
        Err(json!({ "id": message["id"], "error": { "code": -32000, "message": why } }))
    };
    let domain = method.split('.').next().unwrap_or("");
    if matches!(domain, "Browser" | "Target" | "SystemInfo" | "Tethering")
        || matches!(
            method,
            "Page.setDownloadBehavior"
                | "Page.navigateToHistoryEntry"
                | "Storage.clearDataForOrigin"
        )
    {
        return refuse("not available to agents");
    }
    if method == "Page.navigate"
        && !message["params"]["url"]
            .as_str()
            .is_some_and(controller::is_web_url)
    {
        return refuse("agents can only open http and https pages");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agents_are_kept_to_pages() {
        let ok = |m: &str| agent_may_send(m).is_ok();
        assert!(ok(
            r#"{"id":1,"method":"Runtime.evaluate","params":{"expression":"1"}}"#
        ));
        assert!(ok(
            r#"{"id":2,"method":"Page.navigate","params":{"url":"https://example.com"}}"#
        ));
        assert!(!ok(
            r#"{"id":3,"method":"Page.navigate","params":{"url":"file:///etc/passwd"}}"#
        ));
        assert!(!ok(
            r#"{"id":4,"method":"Page.navigate","params":{"url":"circle://ui/settings.html"}}"#
        ));
        assert!(!ok(r#"{"id":5,"method":"Target.attachToBrowserTarget"}"#));
        assert!(!ok(
            r#"{"id":6,"method":"Browser.setDownloadBehavior","params":{}}"#
        ));
        assert!(!ok(
            r#"{"id":7,"method":"Page.setDownloadBehavior","params":{}}"#
        ));
    }

    #[test]
    fn tokens_compare_exactly() {
        assert!(token_eq("abc123", "abc123"));
        assert!(!token_eq("abc123", "abc124"));
        assert!(!token_eq("abc", "abc123"));
        assert!(!token_eq("", "x"));
    }

    #[test]
    fn random_tokens_are_long_and_distinct() {
        let (a, b) = (random_token(), random_token());
        assert_eq!(a.len(), 48);
        assert_ne!(a, b);
    }

    #[test]
    fn agent_token_requires_agent_access_and_changes() {
        set_agent_access(true);
        let first = AGENT_TOKEN.lock().unwrap().clone();
        assert!(is_agent_token(&first));
        assert!(!is_devtools_token(&first));
        set_agent_access(false);
        assert!(!is_agent_token(&first));
        set_agent_access(true);
        assert!(!is_agent_token(&first), "a new token each time");
        set_agent_access(false);
    }
}
