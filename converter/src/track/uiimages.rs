//! The track's UI images: the picture and the maps the menus and the HUD
//! ask for per (track, layout).
//!
//! Game facts (uiresources\js\components.js and the stock files in
//! content.kspkg):
//! - track select / loading page: `images\tracks\<getTrackId>.texture`, a
//!   1920x1080 BC7 sRGB picture (stock: brands_hatch-gp);
//! - track select map: `images\trackmaps\<getTrackId>.svg` (stock: viewBox
//!   380x240, the circuit as white / grey paths);
//! - in-session map: `images\trackmaps\<slug>-<slug>.texture`, 1024x1024 BC7,
//!   a white outline on transparent, framed exactly like the mode row's
//!   layout_image (f18): square of side f4 from (f2, f3), the shorter axis
//!   centred, x to the right and z DOWN (measured: brands_hatch-gp's
//!   texture over its layout_gp.scene track limits).
//!   (`getTrackId` and `slug`: `declare::ui_track_id` / `ui_map_id`.)
//! - coui serves `coui://uiresources/<path>` from the ROOT path
//!   `uiresources\<path>` of the mounted file system (content.kspkg, the
//!   mods' kspkg, then the loose `Saved Games\ACE\mods\` folder), never from
//!   inside a track folder; a missing image is only a "Failed loading
//!   resource" warning and an empty panel.
//!
//! Ours: the picture is AC1 `ui\<layout>\preview.png` scaled to cover
//! 1920x1080 (else a dark panel); both maps are drawn from the layout's
//! centre line (AC1's outline.png has no known world frame). The files are
//! written under `<track>\ui\uiresources\...` and listed in
//! track_decl.json `ui_files` (from -> to) for the installer.

use std::path::Path;

use anyhow::Result;

use super::declare::{ui_map_id, ui_track_id};
use crate::io::texture::{self, Fmt, Image};

/// Map frame of a layout: (x0, z0, side) - the f18 square, already shifted
/// so the shorter axis is centred.
pub fn frame(bounds: ([f32; 2], [f32; 2])) -> (f32, f32, f32) {
    let (lo, hi) = bounds;
    let (ex, ez) = (hi[0] - lo[0], hi[1] - lo[1]);
    let k = ex.max(ez).max(1.0);
    (lo[0] - (k - ex) / 2.0, lo[1] - (k - ez) / 2.0, k)
}

fn sample(img: &Image, x: f32, y: f32) -> [f32; 4] {
    let (w, h) = (img.w as i32, img.h as i32);
    let (x, y) = (x.clamp(0.0, (w - 1) as f32), y.clamp(0.0, (h - 1) as f32));
    let (x0, y0) = (x.floor() as i32, y.floor() as i32);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let px = |x: i32, y: i32, c: usize| img.px[((y * w + x) * 4) as usize + c] as f32;
    let mut o = [0.0; 4];
    for (c, v) in o.iter_mut().enumerate() {
        let a = px(x0, y0, c) * (1.0 - fx) + px(x1, y0, c) * fx;
        let b = px(x0, y1, c) * (1.0 - fx) + px(x1, y1, c) * fx;
        *v = a * (1.0 - fy) + b * fy;
    }
    o
}

/// `img` scaled to cover w x h (centre crop), bilinear.
pub fn cover(img: &Image, w: u32, h: u32) -> Image {
    let s = (w as f32 / img.w as f32).max(h as f32 / img.h as f32);
    let (ox, oy) = ((img.w as f32 * s - w as f32) / 2.0, (img.h as f32 * s - h as f32) / 2.0);
    let mut px = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let v = sample(img, (x as f32 + 0.5 + ox) / s - 0.5, (y as f32 + 0.5 + oy) / s - 0.5);
            let i = ((y * w + x) * 4) as usize;
            for c in 0..4 {
                px[i + c] = v[c].round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    Image { w, h, px }
}

/// The in-session map texture: the line as a white band with a soft grey
/// rim on transparent, in the f18 frame.
pub fn map_texture(line: &[[f32; 3]], closed: bool, bounds: ([f32; 2], [f32; 2]), size: u32) -> Image {
    let (x0, z0, k) = frame(bounds);
    let s = size as f32 / k;
    let pts: Vec<(f32, f32)> = line.iter().map(|p| ((p[0] - x0) * s, (p[2] - z0) * s)).collect();
    // band half-widths in pixels (stock: ~12 px white on 1024)
    let (core, rim) = (5.0f32 * size as f32 / 1024.0, 7.0f32 * size as f32 / 1024.0);
    let mut dist = vec![f32::INFINITY; (size * size) as usize];
    let n = pts.len();
    let segs = if closed && n > 2 { n } else { n.saturating_sub(1) };
    for i in 0..segs {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        let (lx, hx) = ((a.0.min(b.0) - rim - 1.0).max(0.0) as u32, ((a.0.max(b.0) + rim + 1.0).max(0.0) as u32).min(size - 1));
        let (ly, hy) = ((a.1.min(b.1) - rim - 1.0).max(0.0) as u32, ((a.1.max(b.1) + rim + 1.0).max(0.0) as u32).min(size - 1));
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let l2 = (dx * dx + dy * dy).max(1e-6);
        for y in ly..=hy {
            for x in lx..=hx {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let t = (((px - a.0) * dx + (py - a.1) * dy) / l2).clamp(0.0, 1.0);
                let (qx, qy) = (a.0 + t * dx - px, a.1 + t * dy - py);
                let d = (qx * qx + qy * qy).sqrt();
                let c = &mut dist[(y * size + x) as usize];
                if d < *c {
                    *c = d;
                }
            }
        }
    }
    let mut px = vec![0u8; (size * size * 4) as usize];
    for (i, d) in dist.iter().enumerate() {
        if !d.is_finite() || *d > rim + 1.0 {
            continue;
        }
        let white = (core + 0.5 - d).clamp(0.0, 1.0);
        let alpha = (rim + 0.5 - d).clamp(0.0, 1.0);
        let g = 150.0 + 105.0 * white;
        px[i * 4..i * 4 + 4].copy_from_slice(&[g as u8, g as u8, g as u8, (alpha * 255.0) as u8]);
    }
    Image { w: size, h: size, px }
}

/// The track select SVG: the line fitted into stock's 380x240 view box,
/// the same orientation as the map texture.
pub fn map_svg(line: &[[f32; 3]], closed: bool) -> String {
    let (w, h, m) = (380.0f32, 240.0f32, 12.0f32);
    let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
    for p in line {
        lo = [lo[0].min(p[0]), lo[1].min(p[2])];
        hi = [hi[0].max(p[0]), hi[1].max(p[2])];
    }
    let (ex, ez) = ((hi[0] - lo[0]).max(1.0), (hi[1] - lo[1]).max(1.0));
    let s = ((w - 2.0 * m) / ex).min((h - 2.0 * m) / ez);
    let (ox, oy) = ((w - ex * s) / 2.0, (h - ez * s) / 2.0);
    // about one point per 1.5 px of view box
    let step = (line.len() as f32 / ((ex + ez) * s / 0.75).max(8.0)).max(1.0) as usize;
    let mut d = String::new();
    for (i, p) in line.iter().enumerate().filter(|(i, _)| i % step == 0) {
        d.push_str(&format!("{}{:.1},{:.1}", if i == 0 { "M" } else { " L" }, ox + (p[0] - lo[0]) * s, oy + (p[2] - lo[1]) * s));
    }
    if closed {
        d.push_str(" Z");
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 380 240\">\n  \
         <path d=\"{d}\" fill=\"none\" stroke=\"#3e3c46\" stroke-width=\"9\" stroke-linejoin=\"round\" stroke-linecap=\"round\"/>\n  \
         <path d=\"{d}\" fill=\"none\" stroke=\"#aba8b2\" stroke-width=\"6\" stroke-linejoin=\"round\" stroke-linecap=\"round\"/>\n  \
         <path d=\"{d}\" fill=\"none\" stroke=\"#ffffff\" stroke-width=\"3\" stroke-linejoin=\"round\" stroke-linecap=\"round\"/>\n</svg>\n"
    )
}

/// A dark neutral panel (no AC1 preview).
fn blank(w: u32, h: u32) -> Image {
    Image { w, h, px: [30u8, 30, 34, 255].iter().copied().cycle().take((w * h * 4) as usize).collect() }
}

/// Write one layout's three UI images under `<out>\ui\uiresources\images`.
/// Returns (from relative to `out`, to relative to the mods root) per file.
pub fn write(out: &Path, track: &str, layout: &str, ac1_ui_dir: Option<&Path>, line: &[[f32; 3]], closed: bool, bounds: ([f32; 2], [f32; 2]), notes: &mut Vec<String>) -> Result<Vec<(String, String)>> {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let tid = ui_track_id(track, layout);
    let mid = ui_map_id(track, layout);
    // picture
    let preview = ac1_ui_dir.map(|d| d.join("preview.png")).filter(|p| p.is_file()).and_then(|p| std::fs::read(p).ok()).and_then(|b| texture::decode_any(&b).ok());
    if preview.is_none() {
        notes.push(format!("layout {layout:?}: no AC1 preview.png - the track picture is a plain panel"));
    }
    let pic = preview.map(|p| cover(&p, 1920, 1080)).unwrap_or_else(|| blank(1920, 1080));
    let e = texture::encode(&pic, Fmt::Bc7, true, &format!("uiresources\\images\\tracks\\{tid}.png"));
    files.push((format!("uiresources\\images\\tracks\\{tid}.texture"), e.header));
    files.push((format!("uiresources\\images\\tracks\\{tid}.texturemips"), e.mips));
    if line.len() > 2 {
        files.push((format!("uiresources\\images\\trackmaps\\{tid}.svg"), map_svg(line, closed).into_bytes()));
        let m = map_texture(line, closed, bounds, 1024);
        let e = texture::encode(&m, Fmt::Bc7, true, &format!("uiresources\\images\\trackmaps\\{mid}.png"));
        files.push((format!("uiresources\\images\\trackmaps\\{mid}.texture"), e.header));
        files.push((format!("uiresources\\images\\trackmaps\\{mid}.texturemips"), e.mips));
    } else {
        notes.push(format!("layout {layout:?}: no centre line - no track maps"));
    }
    let mut list = Vec::new();
    for (to, bytes) in files {
        let from = format!("ui\\{to}");
        let p = out.join(from.replace('\\', "/"));
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(&p, bytes)?;
        list.push((from, to));
    }
    Ok(list)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_centres_the_short_axis() {
        // brands_hatch-gp: f2 -423.66, f3 -451.79, f4 949.02, f5 126.79
        let (x0, z0, k) = frame(([-423.66, -451.79], [-423.66 + 949.02 - 126.79, -451.79 + 949.02]));
        assert!((x0 - (-423.66 - 63.395)).abs() < 0.01 && (z0 + 451.79).abs() < 0.01 && (k - 949.02).abs() < 0.01);
    }

    #[test]
    fn map_draws_the_line() {
        let line: Vec<[f32; 3]> = (0..64).map(|i| { let a = i as f32 / 64.0 * std::f32::consts::TAU; [a.cos() * 100.0, 0.0, a.sin() * 50.0] }).collect();
        let img = map_texture(&line, true, ([-100.0, -50.0], [100.0, 50.0]), 256);
        let lit = img.px.chunks_exact(4).filter(|p| p[3] > 0).count();
        assert!(lit > 500 && lit < 256 * 256 / 4, "{lit}");
        assert!(map_svg(&line, true).contains(" Z\""));
    }
}
