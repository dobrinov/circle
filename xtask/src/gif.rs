//! `cargo xtask gif <frames-dir> <out.gif> [--width N] [--bg RRGGBB]`
//!
//! Turns a folder of captured window frames into a looping GIF, the same way
//! Circle's recorder encodes GIFs. Used for the website's demos, which show
//! Circle's whole window rather than just a page.
//!
//! The folder holds PNGs plus `frames.json`: per frame, its capture time in
//! seconds (`t`) and the windows captured, back to front (`layers`: `file`,
//! offset `x`/`y` in points from the main window, `main`, and the main
//! window's width `w` in points). Overlays such as the command bar are
//! separate windows on macOS, so they are composited onto the main one.

use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::{fs::File, path::Path};
use tiny_skia::{Pixmap, PixmapPaint, Transform};

/// Color quantization effort, as in the recorder: 1 (best) to 30 (fastest).
const SPEED: i32 = 10;

pub fn run(args: &[String]) -> Result<()> {
    let mut positional = Vec::new();
    let (mut width, mut bg) = (None, [255u8, 255, 255]);
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--width" => width = Some(args.next().context("--width needs a value")?.parse()?),
            "--bg" => {
                let hex = args.next().context("--bg needs a value")?;
                let v = u32::from_str_radix(hex.trim_start_matches('#'), 16)?;
                bg = [(v >> 16) as u8, (v >> 8) as u8, v as u8];
            }
            other => positional.push(other.to_owned()),
        }
    }
    let [dir, out] = positional.as_slice() else {
        bail!("usage: cargo xtask gif <frames-dir> <out.gif> [--width N] [--bg RRGGBB]");
    };
    encode(Path::new(dir), Path::new(out), width, bg)
}

fn encode(dir: &Path, out: &Path, width: Option<u32>, bg: [u8; 3]) -> Result<()> {
    let meta: Vec<Value> =
        serde_json::from_str(&std::fs::read_to_string(dir.join("frames.json"))?)?;
    let times: Vec<f64> = meta
        .iter()
        .map(|f| f["t"].as_f64().unwrap_or(0.0))
        .collect();
    let first = compose(dir, &meta[0])?;
    let (src_w, src_h) = (first.width(), first.height());
    let out_w = width.unwrap_or(src_w).min(src_w);
    let out_h = (src_h as u64 * out_w as u64 / src_w as u64) as u32;

    // Load and scale every frame, dropping ones identical to the one before
    // (their time goes to the previous frame).
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    let indices: Vec<usize> = (0..times.len()).collect();
    let mut frames: Vec<(Vec<u8>, f64)> = Vec::new();
    for batch in indices.chunks(workers * 2) {
        let loaded: Vec<Result<Vec<u8>>> = std::thread::scope(|scope| {
            let jobs: Vec<_> = batch
                .iter()
                .map(|&i| {
                    let meta = &meta[i];
                    scope.spawn(move || -> Result<Vec<u8>> {
                        let pixmap = compose(dir, meta).with_context(|| format!("frame {i}"))?;
                        let rgb = flatten(&pixmap, bg);
                        Ok(scale(&rgb, pixmap.width(), pixmap.height(), out_w, out_h))
                    })
                })
                .collect();
            jobs.into_iter().map(|j| j.join().unwrap()).collect()
        });
        for (rgba, &i) in loaded.into_iter().zip(batch) {
            let rgba = rgba?;
            if frames.last().is_some_and(|(prev, _)| *prev == rgba) {
                continue;
            }
            frames.push((rgba, times[i]));
        }
    }
    // How long each frame stays up; the last one holds for a second.
    let end = times.last().copied().unwrap_or(0.0) + 1.0;
    let starts: Vec<f64> = frames.iter().map(|(_, t)| *t).collect();

    let mut encoder = gif::Encoder::new(File::create(out)?, out_w as u16, out_h as u16, &[])?;
    encoder.set_repeat(gif::Repeat::Infinite)?;
    for (n, batch) in frames.chunks_mut(workers * 2).enumerate() {
        let base = n * workers * 2;
        let quantized: Vec<gif::Frame<'static>> = std::thread::scope(|scope| {
            let jobs: Vec<_> = batch
                .iter_mut()
                .enumerate()
                .map(|(k, (rgba, start))| {
                    let next = starts.get(base + k + 1).copied().unwrap_or(end);
                    let start = *start;
                    scope.spawn(move || {
                        let mut frame =
                            gif::Frame::from_rgba_speed(out_w as u16, out_h as u16, rgba, SPEED);
                        frame.delay = (((next - start) * 100.0).round() as u16).max(2);
                        frame
                    })
                })
                .collect();
            jobs.into_iter().map(|j| j.join().unwrap()).collect()
        });
        for frame in quantized {
            encoder.write_frame(&frame)?;
        }
    }
    println!(
        "{}: {} frames, {}x{}, {:.1}s",
        out.display(),
        frames.len(),
        out_w,
        out_h,
        end - starts.first().copied().unwrap_or(0.0)
    );
    Ok(())
}

/// One frame: the main window with any overlay windows drawn over it.
fn compose(dir: &Path, frame: &Value) -> Result<Pixmap> {
    let layers = frame["layers"].as_array().context("frame without layers")?;
    let main = layers
        .iter()
        .find(|l| l["main"] == true)
        .context("frame without a main window")?;
    let load = |l: &Value| {
        let file = l["file"].as_str().unwrap_or_default();
        Pixmap::load_png(dir.join(file)).with_context(|| format!("reading {file}"))
    };
    let mut base = load(main)?;
    let scale = base.width() as f64 / main["w"].as_f64().unwrap_or(base.width() as f64);
    for layer in layers.iter().filter(|l| l["main"] != true) {
        let Ok(top) = load(layer) else { continue };
        let x = (layer["x"].as_f64().unwrap_or(0.0) * scale).round() as i32;
        let y = (layer["y"].as_f64().unwrap_or(0.0) * scale).round() as i32;
        base.draw_pixmap(
            x,
            y,
            top.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
    }
    Ok(base)
}

/// Opaque RGB pixels, with transparency (the window's rounded corners)
/// composited onto `bg`.
fn flatten(pixmap: &Pixmap, bg: [u8; 3]) -> Vec<u8> {
    let mut rgb = Vec::with_capacity((pixmap.width() * pixmap.height() * 3) as usize);
    for p in pixmap.pixels() {
        // Pixmap pixels are premultiplied.
        let a = p.alpha() as u32;
        for (c, b) in [p.red(), p.green(), p.blue()].into_iter().zip(bg) {
            rgb.push((c as u32 + b as u32 * (255 - a) / 255).min(255) as u8);
        }
    }
    rgb
}

/// Box-filter downscale of RGB pixels to RGBA.
fn scale(rgb: &[u8], w: u32, h: u32, to_w: u32, to_h: u32) -> Vec<u8> {
    let (w, h, to_w, to_h) = (w as usize, h as usize, to_w as usize, to_h as usize);
    let mut out = Vec::with_capacity(to_w * to_h * 4);
    for y in 0..to_h {
        let (y0, y1) = (y * h / to_h, ((y + 1) * h / to_h).max(y * h / to_h + 1));
        for x in 0..to_w {
            let (x0, x1) = (x * w / to_w, ((x + 1) * w / to_w).max(x * w / to_w + 1));
            let mut sum = [0u32; 3];
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let i = (sy * w + sx) * 3;
                    for c in 0..3 {
                        sum[c] += rgb[i + c] as u32;
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u32;
            out.extend([sum[0] / n, sum[1] / n, sum[2] / n].map(|v| v as u8));
            out.push(255);
        }
    }
    out
}
