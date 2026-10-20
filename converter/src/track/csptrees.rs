//! CSP tree lists (requirement track-vegetation, "CSP tree lists").
//!
//! AC1 input (Custom Shaders Patch track config, public format; read off
//! the library's tracks, chq_sepang):
//! - `ext_config.ini` `[TREES]` `LIST_n = <file>` (relative to the track's
//!   `extension` folder), `SURFACE_MATERIALS`, `COMPILED_LIST` (ignored);
//! - a list file: `configure: size variance = a,b`, `width variance = a,b`,
//!   `angle variance = a,b` (degrees), `brightness variance`, `color
//!   variance`, then one `tree:<model>; x,y,z` per tree (more `;` fields
//!   may follow and are ignored). A later `configure:` applies to the trees
//!   after it;
//! - a model is a zip (`.zip`, or `.bin` holding the same zip) next to the
//!   list with `tree.ini` `[BASIC] HEIGHT / WIDTH` (model units), the LOD
//!   meshes and the leaf / bark DDS textures. The drawn tree is HEIGHT x
//!   size (size drawn from the size variance) tall and WIDTH x size x
//!   width-factor wide: chq_sepang's palm (HEIGHT 55.9, size 0.15..0.2)
//!   stands 8..11 m, a palm's height.
//!
//! Ours: the per-tree draws (size, width factor, yaw) come from SplitMix64
//! seeded by FNV-1a of (track id, list index, entry index); the leaf colour
//! is the mean of the opaque texels of the model's largest colour texture
//! (not a normal map); a model that cannot be read stands 10 m tall and 7 m
//! wide.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use super::scan::Scan;

#[derive(Clone, Debug)]
pub struct CspTree {
    /// position from the list (the plant is put on the ground later)
    pub pos: [f32; 3],
    pub height: f32,
    pub width: f32,
    /// yaw in degrees
    pub yaw: f32,
    /// linear leaf colour of the model
    pub colour: [f32; 3],
    /// the model file name (hints: palm, pine, ...)
    pub model: String,
}

#[derive(Default)]
pub struct Lists {
    pub trees: Vec<CspTree>,
    pub notes: Vec<String>,
    pub lists: usize,
}

#[derive(Clone, Copy)]
struct Variance {
    size: [f32; 2],
    width: [f32; 2],
    angle: [f32; 2],
}

impl Default for Variance {
    fn default() -> Self {
        Variance { size: [1.0, 1.0], width: [1.0, 1.0], angle: [0.0, 360.0] }
    }
}

/// SplitMix64 step.
pub fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Uniform [0, 1) from SplitMix64.
pub fn uniform(state: &mut u64) -> f32 {
    (splitmix(state) >> 40) as f32 / (1u64 << 24) as f32
}

/// FNV-1a 64 of a string.
pub fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn pair(v: &str) -> Option<[f32; 2]> {
    let n: Vec<f32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
    (n.len() >= 2).then(|| [n[0], n[1]])
}

/// One model's facts: (height, width, colour).
struct Model {
    height: f32,
    width: f32,
    colour: Option<[f32; 3]>,
}

fn read_model(path: &Path) -> Option<Model> {
    let f = std::fs::File::open(path).ok()?;
    let mut z = zip::ZipArchive::new(f).ok()?;
    let mut ini_text = None;
    let mut best: Option<(u64, String)> = None;
    for i in 0..z.len() {
        let e = z.by_index(i).ok()?;
        let n = e.name().map(|c| c.to_string()).unwrap_or_default();
        let l = n.to_lowercase();
        if l.ends_with("tree.ini") {
            let mut s = String::new();
            let mut e = e;
            e.read_to_string(&mut s).ok()?;
            ini_text = Some(s);
            continue;
        }
        let is_img = l.ends_with(".dds") || l.ends_with(".png") || l.ends_with(".jpg");
        let is_nm = l.contains("_nrm") || l.contains("_nm") || l.contains("normal");
        // leaves before bark: prefer names that say so, then the larger file
        if is_img && !is_nm {
            let bonus = if l.contains("leaf") || l.contains("leaves") || l.contains("foliage") { 1 << 40 } else { 0 };
            let score = e.size() + bonus;
            if best.as_ref().map(|b| score > b.0).unwrap_or(true) {
                best = Some((score, n));
            }
        }
    }
    let ini = super::super::ac1::ini::parse(&ini_text?);
    let height = ini.get("BASIC", "HEIGHT").and_then(|v| v.trim().parse::<f32>().ok()).filter(|v| *v > 0.0)?;
    let width = ini.get("BASIC", "WIDTH").and_then(|v| v.trim().parse::<f32>().ok()).filter(|v| *v > 0.0).unwrap_or(height * 0.7);
    let colour = best.and_then(|(_, n)| {
        let mut e = z.by_name(&n).ok()?;
        let mut b = Vec::new();
        e.read_to_end(&mut b).ok()?;
        let img = crate::io::texture::decode_any(&b).ok()?;
        let s = if img.w > 64 { crate::io::texture::resize_to(&img, 64, true) } else { img };
        let px: Vec<[u8; 4]> = s.px.chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]]).collect();
        super::vegcat::opaque_mean(&px, true)
    });
    Some(Model { height, width, colour })
}

/// The track's CSP tree lists (empty when it has none).
pub fn read(scan: &Scan, track_id: &str) -> Lists {
    let mut out = Lists::default();
    let Some((cfg_path, ini)) = &scan.csp else { return out };
    let Some(sec) = ini.section("TREES") else { return out };
    let mut lists: Vec<(String, String)> = sec.iter().filter(|(k, _)| k.starts_with("LIST")).map(|(k, v)| (k.clone(), v.trim().to_string())).collect();
    lists.sort();
    let ext_dir = scan.src.dir.join("extension");
    let cfg_dir = cfg_path.parent().map(Path::to_path_buf).unwrap_or_else(|| ext_dir.clone());
    let mut models: HashMap<PathBuf, Option<Model>> = HashMap::new();
    for (li, (_, rel)) in lists.iter().enumerate() {
        if rel.is_empty() {
            continue;
        }
        let rel_os = rel.replace('/', "\\");
        let cand = [ext_dir.join(&rel_os), cfg_dir.join(&rel_os)];
        let Some(file) = cand.iter().find(|p| p.is_file()).cloned() else {
            out.notes.push(format!("CSP tree list {rel} not found"));
            continue;
        };
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        out.lists += 1;
        let dir = file.parent().map(Path::to_path_buf).unwrap_or_default();
        let mut var = Variance::default();
        let mut missing = 0usize;
        for (ei, line) in text.lines().enumerate() {
            let line = line.trim();
            if let Some(c) = line.strip_prefix("configure:") {
                let Some((k, v)) = c.split_once('=') else { continue };
                let k = k.trim().to_lowercase();
                if let Some(p) = pair(v) {
                    match k.as_str() {
                        "size variance" => var.size = p,
                        "width variance" => var.width = p,
                        "angle variance" => var.angle = p,
                        _ => {}
                    }
                }
                continue;
            }
            let Some(t) = line.strip_prefix("tree:") else { continue };
            let mut parts = t.split(';');
            let model = parts.next().unwrap_or("").trim().to_string();
            let Some(xyz) = parts.next() else { continue };
            let p: Vec<f32> = xyz.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            if p.len() < 3 || !p.iter().all(|v| v.is_finite()) {
                continue;
            }
            let mp = dir.join(&model);
            let m = models.entry(mp.clone()).or_insert_with(|| read_model(&mp));
            let (mh, mw, col) = match m {
                Some(m) => (m.height, m.width, m.colour),
                None => {
                    missing += 1;
                    (10.0, 7.0, None)
                }
            };
            let mut st = fnv(&format!("{track_id}|{li}|{ei}"));
            let size = var.size[0] + (var.size[1] - var.size[0]) * uniform(&mut st);
            let wf = var.width[0] + (var.width[1] - var.width[0]) * uniform(&mut st);
            let yaw = var.angle[0] + (var.angle[1] - var.angle[0]) * uniform(&mut st);
            let (height, width) = if m.is_some() { (mh * size, mw * size * wf) } else { (mh, mw) };
            out.trees.push(CspTree { pos: [p[0], p[1], p[2]], height, width, yaw, colour: col.unwrap_or([0.08, 0.12, 0.05]), model: model.to_lowercase() });
        }
        if missing > 0 {
            out.notes.push(format!("CSP tree list {rel}: {missing} trees of unreadable models (10 m assumed)"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeded_draws_repeat() {
        let mut a = fnv("x|0|5");
        let mut b = fnv("x|0|5");
        assert_eq!(uniform(&mut a), uniform(&mut b));
        let mut c = fnv("x|0|6");
        assert_ne!(uniform(&mut a), uniform(&mut c));
        assert_eq!(pair(" 0.15,0.2"), Some([0.15, 0.2]));
    }
}
