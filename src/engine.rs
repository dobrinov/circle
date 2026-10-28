//! A private, browser-wide DevTools connection to Chromium over a pipe
//! (`--remote-debugging-pipe`), which only this process holds: no port is
//! opened, so neither websites nor agents can reach it. Circle uses it for
//! what has no CEF API, such as loading and removing extensions.
//!
//! Chromium reads commands from fd 3 and writes replies to fd 4 (on Windows,
//! the handles named by `--remote-debugging-io-pipes`); each message is JSON
//! ending in a NUL byte.

use crate::remote::run_on_ui;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

type Reply = Box<dyn FnOnce(Result<Value, String>) + Send>;

struct Pipe {
    writer: Mutex<std::fs::File>,
    pending: Mutex<HashMap<u64, Reply>>,
    next: AtomicU64,
}

static PIPE: OnceLock<Pipe> = OnceLock::new();
/// The switches that tell Chromium about the pipe, once it is set up.
static SWITCHES: OnceLock<Vec<(String, Option<String>)>> = OnceLock::new();

/// Sets up the pipe. Call once in the browser process, before CEF starts.
pub fn init() {
    match open() {
        Ok((reader, writer, mut switches)) => {
            switches.push(("enable-unsafe-extension-debugging".into(), None));
            let _ = SWITCHES.set(switches);
            let _ = PIPE.set(Pipe {
                writer: Mutex::new(writer),
                pending: Mutex::new(HashMap::new()),
                next: AtomicU64::new(1),
            });
            std::thread::Builder::new()
                .name("circle-engine".into())
                .spawn(move || read_loop(reader))
                .ok();
        }
        Err(err) => eprintln!("circle: no engine pipe ({err}); extensions are off"),
    }
}

/// The command-line switches to add to the browser process.
pub fn switches() -> &'static [(String, Option<String>)] {
    SWITCHES.get().map(Vec::as_slice).unwrap_or(&[])
}

pub fn available() -> bool {
    PIPE.get().is_some()
}

/// Sends a browser-level DevTools command; `reply` runs on the UI thread.
pub fn call(
    method: &str,
    params: Value,
    reply: impl FnOnce(Result<Value, String>) + Send + 'static,
) {
    let Some(pipe) = PIPE.get() else {
        reply(Err("the engine pipe isn't available".into()));
        return;
    };
    let id = pipe.next.fetch_add(1, Ordering::Relaxed);
    pipe.pending.lock().unwrap().insert(id, Box::new(reply));
    let mut message = json!({ "id": id, "method": method, "params": params })
        .to_string()
        .into_bytes();
    message.push(0);
    let sent = pipe.writer.lock().unwrap().write_all(&message);
    if let Err(err) = sent
        && let Some(reply) = pipe.pending.lock().unwrap().remove(&id)
    {
        reply(Err(format!("couldn't reach the engine: {err}")));
    }
}

fn read_loop(reader: std::fs::File) {
    let mut reader = BufReader::new(reader);
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        match reader.read_until(0, &mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if buffer.last() == Some(&0) {
            buffer.pop();
        }
        let Ok(message) = serde_json::from_slice::<Value>(&buffer) else {
            continue;
        };
        let Some(id) = message.get("id").and_then(Value::as_u64) else {
            continue; // an event; none are subscribed to
        };
        let Some(reply) = PIPE
            .get()
            .and_then(|p| p.pending.lock().unwrap().remove(&id))
        else {
            continue;
        };
        let result = match message.get("error") {
            Some(error) => Err(error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
                .to_owned()),
            None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
        };
        run_on_ui(move || reply(result));
    }
}

type Opened = (std::fs::File, std::fs::File, Vec<(String, Option<String>)>);

#[cfg(unix)]
fn open() -> Result<Opened, String> {
    use std::os::fd::FromRawFd;
    let pipe = || {
        let mut fds = [0; 2];
        // SAFETY: `fds` has room for the two descriptors.
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(fds)
    };
    // SAFETY: plain descriptor juggling on descriptors this function owns.
    unsafe {
        let to_engine = pipe()?;
        let from_engine = pipe()?;
        // Move all four out of the way of 3 and 4 first.
        let high = |fd: i32, cloexec: bool| {
            let cmd = if cloexec {
                libc::F_DUPFD_CLOEXEC
            } else {
                libc::F_DUPFD
            };
            let moved = libc::fcntl(fd, cmd, 10);
            libc::close(fd);
            moved
        };
        let engine_reads = high(to_engine[0], false);
        let ours_write = high(to_engine[1], true);
        let ours_read = high(from_engine[0], true);
        let engine_writes = high(from_engine[1], false);
        if [engine_reads, ours_write, ours_read, engine_writes].contains(&-1)
            || libc::dup2(engine_reads, 3) != 3
            || libc::dup2(engine_writes, 4) != 4
        {
            return Err(std::io::Error::last_os_error().to_string());
        }
        libc::close(engine_reads);
        libc::close(engine_writes);
        // Child processes mustn't inherit Chromium's ends.
        libc::fcntl(3, libc::F_SETFD, libc::FD_CLOEXEC);
        libc::fcntl(4, libc::F_SETFD, libc::FD_CLOEXEC);
        Ok((
            std::fs::File::from_raw_fd(ours_read),
            std::fs::File::from_raw_fd(ours_write),
            vec![("remote-debugging-pipe".into(), None)],
        ))
    }
}

#[cfg(windows)]
fn open() -> Result<Opened, String> {
    use std::os::windows::io::{FromRawHandle, RawHandle};
    use windows::Win32::{Foundation::HANDLE, System::Pipes::CreatePipe};
    let pipe = || {
        let (mut read, mut write) = (HANDLE::default(), HANDLE::default());
        // SAFETY: both out-pointers are valid; the handles are owned below.
        unsafe { CreatePipe(&mut read, &mut write, None, 0) }.map_err(|e| e.to_string())?;
        Ok::<_, String>((read, write))
    };
    let (engine_reads, ours_write) = pipe()?;
    let (ours_read, engine_writes) = pipe()?;
    let handles = format!("{},{}", engine_reads.0 as usize, engine_writes.0 as usize);
    // SAFETY: these handles were just created and are owned by the files.
    unsafe {
        Ok((
            std::fs::File::from_raw_handle(ours_read.0 as RawHandle),
            std::fs::File::from_raw_handle(ours_write.0 as RawHandle),
            vec![
                ("remote-debugging-pipe".into(), None),
                ("remote-debugging-io-pipes".into(), Some(handles)),
            ],
        ))
    }
}

#[cfg(not(any(unix, windows)))]
fn open() -> Result<Opened, String> {
    Err("unsupported platform".into())
}
