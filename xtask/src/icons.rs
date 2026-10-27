//! Draws the Circle app icon: a complete ring whose color flows blue → teal
//! → green and back (an angular gradient) on a dark tile, in the style of the
//! other products on dobrinov.github.io. The PNGs, `.ico` and `.icns` files
//! are committed, so this only runs when the design changes.
//!
//! tiny-skia has no angular gradients, so the ring is computed per pixel.

use crate::{root, run};
use anyhow::Result;
use std::{fs, process::Command};
use tiny_skia::*;

/// The ring's colors around the circle (0..1 clockwise from the top). The
/// loop starts and ends on the same color, so it is seamless.
const RING_STOPS: [(f32, u32); 4] = [
    (0.0, 0x4A6CFF),
    (1.0 / 3.0, 0x2A9BA3),
    (2.0 / 3.0, 0x45C75B),
    (1.0, 0x4A6CFF),
];
const TILE: u32 = 0x10141D;
/// Ring diameter relative to the tile, and thickness relative to its radius.
const RING_SCALE: f32 = 0.67;
const RING_THICKNESS: f32 = 0.42;

fn rgb(hex: u32) -> [f32; 3] {
    [
        (hex >> 16) as f32,
        ((hex >> 8) & 0xFF) as f32,
        (hex & 0xFF) as f32,
    ]
    .map(|c| c / 255.0)
}

/// The ring color at `t` (0..1 around the circle), interpolated in sRGB like CSS.
fn ring_color(t: f32) -> [f32; 3] {
    for pair in RING_STOPS.windows(2) {
        let ((t0, c0), (t1, c1)) = (pair[0], pair[1]);
        if t <= t1 {
            let f = ((t - t0) / (t1 - t0)).clamp(0.0, 1.0);
            let (a, b) = (rgb(c0), rgb(c1));
            return [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * f);
        }
    }
    rgb(RING_STOPS[0].1)
}

/// Composites a straight-alpha color over a premultiplied RGBA pixel.
fn blend(px: &mut [u8], color: [f32; 3], alpha: f32) {
    let a = alpha.clamp(0.0, 1.0);
    if a <= 0.0 {
        return;
    }
    for i in 0..3 {
        px[i] = (color[i] * 255.0 * a + px[i] as f32 * (1.0 - a)).round() as u8;
    }
    px[3] = (255.0 * a + px[3] as f32 * (1.0 - a)).round() as u8;
}

fn rounded_rect(x0: f32, y0: f32, side: f32, radius: f32) -> Path {
    let (x1, y1) = (x0 + side, y0 + side);
    // Cubic corners (0.5523 ≈ a circle's control-point ratio).
    let k = radius * (1.0 - 0.552_284_8);
    let mut pb = PathBuilder::new();
    pb.move_to(x0 + radius, y0);
    pb.line_to(x1 - radius, y0);
    pb.cubic_to(x1 - k, y0, x1, y0 + k, x1, y0 + radius);
    pb.line_to(x1, y1 - radius);
    pb.cubic_to(x1, y1 - k, x1 - k, y1, x1 - radius, y1);
    pb.line_to(x0 + radius, y1);
    pb.cubic_to(x0 + k, y1, x0, y1 - k, x0, y1 - radius);
    pb.line_to(x0, y0 + radius);
    pb.cubic_to(x0, y0 + k, x0 + k, y0, x0 + radius, y0);
    pb.close();
    pb.finish().unwrap()
}

/// Draws the icon at `n`×`n` pixels.
fn draw(n: u32, mac: bool) -> Pixmap {
    let s = n as f32 / 1024.0;
    let mut pixmap = Pixmap::new(n, n).unwrap();
    // `mac` follows the macOS icon grid (an 824pt tile inside 1024pt with room
    // for a shadow); otherwise the tile fills the canvas, which reads better
    // at the small sizes Windows and Linux use.
    let (inset, side, radius) = if mac {
        (100.0 * s, 824.0 * s, 186.0 * s)
    } else {
        (0.0, 1024.0 * s, 230.0 * s)
    };
    let tile = rounded_rect(inset, inset, side, radius);

    if mac {
        // A soft shadow: stacked, slightly larger translucent copies of the
        // tile, offset downwards (the design's 14pt-down, 36pt-blur shadow).
        for step in 0..12 {
            let spread = step as f32 * 3.0 * s;
            let shadow_tile = rounded_rect(
                inset - spread,
                inset - spread + 14.0 * s,
                side + 2.0 * spread,
                radius + spread,
            );
            let mut shadow = Paint {
                anti_alias: true,
                ..Default::default()
            };
            shadow.set_color_rgba8(11, 7, 32, 7);
            pixmap.fill_path(
                &shadow_tile,
                &shadow,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }
    let [r, g, b] = rgb(TILE).map(|c| (c * 255.0).round() as u8);
    let mut paint = Paint {
        anti_alias: true,
        ..Default::default()
    };
    paint.set_color_rgba8(r, g, b, 255);
    pixmap.fill_path(
        &tile,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );

    // The ring, per pixel.
    let center = inset + side / 2.0;
    let outer = side * RING_SCALE / 2.0;
    let inner = outer * (1.0 - RING_THICKNESS);
    let width = n as usize;
    let data = pixmap.data_mut();
    for y in 0..n {
        for x in 0..n {
            let (dx, dy) = (x as f32 + 0.5 - center, y as f32 + 0.5 - center);
            let r = dx.hypot(dy);
            if r > outer + 1.0 || r < inner - 1.0 {
                continue;
            }
            let i = (y as usize * width + x as usize) * 4;
            let t = dx.atan2(-dy).to_degrees().rem_euclid(360.0) / 360.0;
            let coverage = (outer - r + 0.5).clamp(0.0, 1.0) * (r - inner + 0.5).clamp(0.0, 1.0);
            blend(&mut data[i..i + 4], ring_color(t), coverage);
        }
    }
    pixmap
}

/// Averages `factor`×`factor` blocks of a premultiplied pixmap.
fn downsample(src: &Pixmap, factor: u32) -> Pixmap {
    let n = src.width() / factor;
    let mut out = Pixmap::new(n, n).unwrap();
    let (data, width) = (src.data(), src.width() as usize);
    let area = factor * factor;
    for y in 0..n {
        for x in 0..n {
            let mut sum = [0u32; 4];
            for sy in 0..factor {
                for sx in 0..factor {
                    let i = (((y * factor + sy) as usize) * width + (x * factor + sx) as usize) * 4;
                    for c in 0..4 {
                        sum[c] += data[i + c] as u32;
                    }
                }
            }
            let o = ((y * n + x) * 4) as usize;
            for (dst, total) in out.data_mut()[o..o + 4].iter_mut().zip(sum) {
                *dst = (total / area) as u8;
            }
        }
    }
    out
}

/// Renders the icon at `size`, supersampling small sizes so they stay crisp.
fn render(size: u32, mac: bool) -> Pixmap {
    let factor = (512 / size).max(1);
    let big = draw(size * factor, mac);
    if factor == 1 {
        big
    } else {
        downsample(&big, factor)
    }
}

pub fn generate() -> Result<()> {
    let dir = root().join("resources/icons");
    fs::create_dir_all(&dir)?;
    let sizes = [16, 24, 32, 48, 64, 128, 256, 512, 1024];
    for size in sizes {
        render(size, false).save_png(dir.join(format!("circle-{size}.png")))?;
    }

    let mut icon_dir = ico::IconDir::new(ico::ResourceType::Icon);
    for size in [16, 24, 32, 48, 64, 128, 256] {
        let pixmap = render(size, false);
        // tiny-skia stores premultiplied alpha; .ico wants straight alpha.
        let rgba = pixmap
            .pixels()
            .iter()
            .flat_map(|p| {
                let c = p.demultiply();
                [c.red(), c.green(), c.blue(), c.alpha()]
            })
            .collect();
        let image = ico::IconImage::from_rgba_data(size, size, rgba);
        icon_dir.add_entry(ico::IconDirEntry::encode(&image)?);
    }
    icon_dir.write(fs::File::create(dir.join("circle.ico"))?)?;

    if cfg!(target_os = "macos") {
        let iconset = std::env::temp_dir().join("circle.iconset");
        let _ = fs::remove_dir_all(&iconset);
        fs::create_dir_all(&iconset)?;
        for size in [16, 32, 128, 256, 512] {
            render(size, true).save_png(iconset.join(format!("icon_{size}x{size}.png")))?;
            render(size * 2, true).save_png(iconset.join(format!("icon_{size}x{size}@2x.png")))?;
        }
        run(Command::new("iconutil")
            .args(["-c", "icns", "-o"])
            .arg(dir.join("circle.icns"))
            .arg(&iconset))?;
    } else {
        eprintln!("note: circle.icns is only regenerated on macOS");
    }
    println!("Icons written to {}", dir.display());
    Ok(())
}
