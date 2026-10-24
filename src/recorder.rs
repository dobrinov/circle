//! Recording the current tab to a video or a GIF in the Downloads folder.
//!
//! Frames come from Chromium's screencast of the tab (CDP
//! `Page.startScreencast`), which sends a JPEG each time the page repaints.
//! A GIF is encoded here. A video is encoded by Chromium itself: frames are
//! drawn into a canvas on a tiny hidden Circle page (`ui/recorder.html`)
//! whose `MediaRecorder` posts the encoded file back in chunks.

use crate::{
    controller::{self, Cmd},
    model::TabId,
    remote::{Session, run_on_ui},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs::File,
    io::Write,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    Video,
    Gif,
}

/// GIFs get big quickly; recordings stop by themselves after this.
const GIF_LIMIT: Duration = Duration::from_secs(60);
const VIDEO_LIMIT: Duration = Duration::from_secs(30 * 60);
/// GIF frames closer together than this are dropped (about 20 fps).
const GIF_FRAME_GAP: f64 = 0.05;
const GIF_MAX_WIDTH: u32 = 960;
/// Color quantization effort, 1 (best) to 30 (fastest).
const GIF_SPEED: i32 = 20;
const VIDEO_MAX_WIDTH: u32 = 1920;

/// What a recording draws into the page, which the screencast can't show.
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct Marks {
    /// The pointer and a ripple on each click.
    pub clicks: bool,
    /// The keys being pressed (secrets as dots).
    pub keys: bool,
}

const OVERLAY_JS: &str = include_str!("../ui/rec-overlay.js");
/// The isolated world the marks are drawn from.
const OVERLAY_WORLD: &str = "circle-recording";

/// Set to stop the recording in progress.
static STOP: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);

/// The video file being written by the recorder page.
static VIDEO_FILE: Mutex<Option<(File, PathBuf)>> = Mutex::new(None);

/// Starts recording `tab` on a worker thread. For a video, the recorder page
/// must already be loaded. The controller hears back via
/// `Cmd::RecordingFinished`.
pub fn start(tab: TabId, format: Format, marks: Marks) {
    let stop = Arc::new(AtomicBool::new(false));
    *STOP.lock().unwrap() = Some(stop.clone());
    std::thread::spawn(move || {
        let result = capture(tab, format, marks, &stop);
        STOP.lock().unwrap().take();
        run_on_ui(|| controller::dispatch(Cmd::RecordingSaving));
        match (format, result) {
            // The recorder page saves the video when it has flushed.
            (Format::Video, Ok(_)) => {
                run_on_ui(|| controller::dispatch(Cmd::RecorderStop));
            }
            (Format::Gif, Ok(frames)) => {
                let saved = encode_gif(&frames).map_err(|e| format!("Couldn't save the GIF: {e}"));
                run_on_ui(move || controller::dispatch(Cmd::RecordingFinished(saved)));
            }
            (_, Err(err)) => {
                run_on_ui(move || controller::dispatch(Cmd::RecordingFinished(Err(err))));
            }
        }
    });
}

pub fn stop() {
    if let Some(stop) = STOP.lock().unwrap().as_ref() {
        stop.store(true, Ordering::SeqCst);
    }
}

struct Frame {
    jpeg: Vec<u8>,
    /// Seconds, from the screencast.
    at: f64,
}

/// Runs the screencast until stopped. Video frames go straight to the
/// recorder page; GIF frames are collected and returned.
fn capture(
    tab: TabId,
    format: Format,
    marks: Marks,
    stop: &AtomicBool,
) -> Result<Vec<Frame>, String> {
    let mut session = Session::open(tab).map_err(|e| format!("Can't record this tab: {e}"))?;
    let overlay = (marks.clicks || marks.keys)
        .then(|| show_marks(&mut session, marks))
        .flatten();
    let max_width = match format {
        Format::Video => VIDEO_MAX_WIDTH,
        Format::Gif => GIF_MAX_WIDTH,
    };
    session.call(
        "Page.startScreencast",
        json!({ "format": "jpeg", "quality": 85, "maxWidth": max_width, "maxHeight": max_width }),
        Duration::from_secs(10),
    )?;
    let limit = match format {
        Format::Video => VIDEO_LIMIT,
        Format::Gif => GIF_LIMIT,
    };
    let started = Instant::now();
    let mut frames = Vec::new();
    let mut last_at = f64::MIN;
    while !stop.load(Ordering::SeqCst) && started.elapsed() < limit {
        let event = match session.next_event(Duration::from_millis(100)) {
            Ok(Some(event)) => event,
            Ok(None) => continue,
            // The tab closed: keep what was recorded.
            Err(_) => break,
        };
        if event["method"] != "Page.screencastFrame" {
            continue;
        }
        let params = &event["params"];
        if let Some(id) = params["sessionId"].as_i64() {
            session.send("Page.screencastFrameAck", json!({ "sessionId": id }))?;
        }
        let Some(data) = params["data"].as_str() else {
            continue;
        };
        match format {
            Format::Video => {
                let code = format!(
                    "window.circleRecorder&&circleRecorder.frame({})",
                    Value::from(data)
                );
                run_on_ui(move || controller::dispatch(Cmd::RecorderJs(code)));
            }
            Format::Gif => {
                let at = params["metadata"]["timestamp"]
                    .as_f64()
                    .unwrap_or_else(|| started.elapsed().as_secs_f64());
                if at - last_at < GIF_FRAME_GAP {
                    continue;
                }
                if let Ok(jpeg) = base64_decode(data) {
                    frames.push(Frame { jpeg, at });
                    last_at = at;
                }
            }
        }
    }
    let ended = frames.last().map(|f| f.at).unwrap_or(0.0);
    let _ = session.call("Page.stopScreencast", json!({}), Duration::from_secs(5));
    if let Some(identifier) = overlay {
        hide_marks(&mut session, &identifier);
    }
    if format == Format::Gif {
        if frames.is_empty() {
            return Err("Nothing was recorded".to_owned());
        }
        // The last frame stays up until the recording stopped.
        let held = started.elapsed().as_secs_f64() - (ended - frames[0].at);
        frames.push(Frame {
            jpeg: Vec::new(),
            at: ended + held.clamp(0.5, 3.0),
        });
    }
    Ok(frames)
}

/// Draws the pointer, clicks and keys into the page (and into pages the tab
/// navigates to while recording). Returns the id of the new-document script.
fn show_marks(session: &mut Session, marks: Marks) -> Option<String> {
    let source = OVERLAY_JS.replace("__OPTIONS__", &json!(marks).to_string());
    let timeout = Duration::from_secs(5);
    let added = session
        .call(
            "Page.addScriptToEvaluateOnNewDocument",
            json!({ "source": source, "worldName": OVERLAY_WORLD }),
            timeout,
        )
        .ok()?;
    // And into the page that is already open.
    let frame = session
        .call("Page.getFrameTree", json!({}), timeout)
        .ok()
        .and_then(|tree| tree["frameTree"]["frame"]["id"].as_str().map(str::to_owned));
    let context = frame.and_then(|frame| {
        session
            .call(
                "Page.createIsolatedWorld",
                json!({ "frameId": frame, "worldName": OVERLAY_WORLD }),
                timeout,
            )
            .ok()?["executionContextId"]
            .as_i64()
    });
    if let Some(context) = context {
        let _ = session.call(
            "Runtime.evaluate",
            json!({ "expression": source, "contextId": context }),
            timeout,
        );
    }
    added["identifier"].as_str().map(str::to_owned)
}

fn hide_marks(session: &mut Session, identifier: &str) {
    let timeout = Duration::from_secs(5);
    let _ = session.call(
        "Page.removeScriptToEvaluateOnNewDocument",
        json!({ "identifier": identifier }),
        timeout,
    );
    // The marks' listeners check that their element is still in the page.
    let _ = session.call(
        "Runtime.evaluate",
        json!({ "expression": "document.getElementById('__circle_recording_overlay')?.remove()" }),
        timeout,
    );
}

fn encode_gif(frames: &[Frame]) -> Result<PathBuf, String> {
    let path = output_path("gif");
    // Written under another name until complete, like the video.
    let partial = partial_path(&path);
    let file = File::create(&partial).map_err(|e| e.to_string())?;
    // Every frame is scaled to the first one's size.
    let (_, width, height) = decode(&frames[0].jpeg)?;
    let mut encoder = gif::Encoder::new(file, width, height, &[]).map_err(|e| e.to_string())?;
    encoder
        .set_repeat(gif::Repeat::Infinite)
        .map_err(|e| e.to_string())?;

    // Color quantization is the slow part, so frames are quantized on every
    // core, a batch at a time to bound memory, then written in order.
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    let pairs: Vec<&[Frame]> = frames.windows(2).collect();
    for batch in pairs.chunks(workers * 2) {
        let quantized: Vec<Result<gif::Frame<'static>, String>> = std::thread::scope(|scope| {
            let jobs: Vec<_> = batch
                .iter()
                .map(|pair| {
                    scope.spawn(move || {
                        let (frame, next) = (&pair[0], &pair[1]);
                        let (mut rgba, w, h) = decode(&frame.jpeg)?;
                        // The window was resized mid-recording.
                        if (w, h) != (width, height) {
                            rgba = resize(&rgba, w, h, width, height);
                        }
                        let mut gif_frame =
                            gif::Frame::from_rgba_speed(width, height, &mut rgba, GIF_SPEED);
                        gif_frame.delay = (((next.at - frame.at) * 100.0).round() as u16).max(2);
                        Ok(gif_frame)
                    })
                })
                .collect();
            jobs.into_iter()
                .map(|job| {
                    job.join()
                        .unwrap_or_else(|_| Err("encoder crashed".to_owned()))
                })
                .collect()
        });
        for gif_frame in quantized {
            encoder
                .write_frame(&gif_frame?)
                .map_err(|e| e.to_string())?;
        }
    }
    drop(encoder);
    std::fs::rename(&partial, &path).map_err(|e| e.to_string())?;
    Ok(path)
}

/// A JPEG frame as RGBA pixels, with its size.
fn decode(jpeg: &[u8]) -> Result<(Vec<u8>, u16, u16), String> {
    let mut decoder = jpeg_decoder::Decoder::new(jpeg);
    let pixels = decoder.decode().map_err(|e| e.to_string())?;
    let info = decoder.info().ok_or("bad frame")?;
    Ok((to_rgba(&pixels, info.pixel_format), info.width, info.height))
}

fn to_rgba(pixels: &[u8], format: jpeg_decoder::PixelFormat) -> Vec<u8> {
    match format {
        jpeg_decoder::PixelFormat::RGB24 => pixels
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        jpeg_decoder::PixelFormat::L8 => pixels.iter().flat_map(|&l| [l, l, l, 255]).collect(),
        _ => vec![0; pixels.len() / 2 * 4],
    }
}

/// Nearest-neighbor resize, for the rare frame whose size changed.
fn resize(rgba: &[u8], w: u16, h: u16, to_w: u16, to_h: u16) -> Vec<u8> {
    let (w, h, to_w, to_h) = (w as usize, h as usize, to_w as usize, to_h as usize);
    let mut out = vec![0u8; to_w * to_h * 4];
    for y in 0..to_h {
        let sy = y * h / to_h;
        for x in 0..to_w {
            let sx = x * w / to_w;
            let (src, dst) = ((sy * w + sx) * 4, (y * to_w + x) * 4);
            out[dst..dst + 4].copy_from_slice(&rgba[src..src + 4]);
        }
    }
    out
}

/// `~/Downloads/Circle Recording 2026-10-23 at 13.05.12.<ext>`, made unique.
fn output_path(ext: &str) -> PathBuf {
    let dir = crate::downloads_dir();
    let stamp = local_timestamp();
    let mut path = dir.join(format!("Circle Recording {stamp}.{ext}"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("Circle Recording {stamp} ({n}).{ext}"));
        n += 1;
    }
    path
}

fn local_timestamp() -> String {
    // `date` knows the local time zone; Rust's std doesn't.
    std::process::Command::new("date")
        .arg("+%Y-%m-%d at %H.%M.%S")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| crate::model::now().to_string())
}

fn base64_decode(text: &str) -> Result<Vec<u8>, ()> {
    fn value(c: u8) -> Result<u32, ()> {
        Ok(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return Err(()),
        } as u32)
    }
    let bytes: Vec<u8> = text
        .bytes()
        .filter(|&c| c != b'=' && !c.is_ascii_whitespace())
        .collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= value(c)? << (18 - 6 * i);
        }
        out.push((n >> 16) as u8);
        if chunk.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(n as u8);
        }
    }
    Ok(out)
}

// ----- the video file, written by the recorder page (IO thread) -------------

/// Handles `circle://ui/api/recording?op=…` from the recorder page:
/// `begin&ext=mp4`, then `chunk` (body: encoded bytes) any number of times,
/// then `done` or `fail`.
pub fn handle_upload(op: &str, ext: &str, body: &[u8]) -> Result<(), String> {
    let mut file = VIDEO_FILE.lock().unwrap();
    match op {
        "begin" => {
            let ext = if matches!(ext, "mp4" | "webm") {
                ext
            } else {
                "webm"
            };
            let path = output_path(ext);
            let partial = partial_path(&path);
            *file = Some((File::create(&partial).map_err(|e| e.to_string())?, path));
        }
        "chunk" => {
            let (f, _) = file.as_mut().ok_or("no recording")?;
            f.write_all(body).map_err(|e| e.to_string())?;
        }
        "done" => {
            let (f, path) = file.take().ok_or("no recording")?;
            drop(f);
            let saved = std::fs::rename(partial_path(&path), &path)
                .map(|_| path)
                .map_err(|e| format!("Couldn't save the video: {e}"));
            run_on_ui(move || controller::dispatch(Cmd::RecordingFinished(saved)));
        }
        _ => {
            if let Some((_, path)) = file.take() {
                let _ = std::fs::remove_file(partial_path(&path));
            }
            let error = String::from_utf8_lossy(body).into_owned();
            run_on_ui(move || {
                controller::dispatch(Cmd::RecordingFinished(Err(format!(
                    "Couldn't record the video: {error}"
                ))))
            });
        }
    }
    Ok(())
}

fn partial_path(path: &std::path::Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_base64() {
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64_decode("aGk=").unwrap(), b"hi");
        assert_eq!(base64_decode("YWJj").unwrap(), b"abc");
        assert!(base64_decode("a$==").is_err());
    }

    #[test]
    fn resizes_frames() {
        let red = [255, 0, 0, 255].repeat(4);
        assert_eq!(resize(&red, 2, 2, 1, 1), vec![255, 0, 0, 255]);
        assert_eq!(resize(&red, 2, 2, 3, 3).len(), 36);
    }
}
