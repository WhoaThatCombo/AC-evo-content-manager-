//! Small helpers shared by the car-function stages (lights, LEDs, display,
//! cameras, exhaust, needles, driver): protobuf vectors and transforms, the
//! AC1 Custom Shaders Patch config, uniform mask textures, per-section edits
//! of an encoded `.mesh`, camera / emitter angles.

use std::collections::HashMap;

use crate::ac1::car::Car;
use crate::files::Files;
use crate::io::pb::{Msg, Val};

/// Vector3Data / Vector4Data: {f1 x, f2 y, f3 z, f4 w}, zeros left out.
pub fn vec_msg(v: &[f32]) -> Msg {
    let mut m = Msg::default();
    for (i, x) in v.iter().enumerate() {
        if *x != 0.0 {
            m.set_f32(i as u32 + 1, *x);
        }
    }
    m
}

pub fn vec_of(m: Option<Msg>, n: usize) -> Vec<f32> {
    let m = m.unwrap_or_default();
    (1..=n as u32).map(|k| m.f32(k).unwrap_or(0.0)).collect()
}

/// TransformData {f1 position, f2 rotation (degrees), f3 scale}. Stock
/// writes position and rotation even when zero, and scale (1, 1, 1).
pub fn transform(pos: [f32; 3], rot: [f32; 3]) -> Msg {
    let mut t = Msg::default();
    t.push(1, Val::Msg(vec_msg(&pos)));
    t.push(2, Val::Msg(vec_msg(&rot)));
    t.push(3, Val::Msg(vec_msg(&[1.0, 1.0, 1.0])));
    t
}

/// (position, rotation) of a TransformData.
pub fn transform_of(t: &Msg) -> ([f32; 3], [f32; 3]) {
    let p = vec_of(t.sub(1), 3);
    let r = vec_of(t.sub(2), 3);
    ([p[0], p[1], p[2]], [r[0], r[1], r[2]])
}

/// Euler degrees (x pitch, y yaw, 0) that turn local -Z onto `dir`, with
/// R = Ry * Rx (X first; the scene convention, docs/FORMATS.md): local -Z
/// becomes (-cos a sin b, sin a, -cos a cos b). A car-forward camera is
/// (pitch, 180, 0), as on every stock actor.
pub fn euler_minus_z(dir: [f64; 3]) -> [f32; 3] {
    let l = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
    if !(l > 1e-9) {
        return [0.0, 180.0, 0.0];
    }
    let d = [dir[0] / l, dir[1] / l, dir[2] / l];
    let a = d[1].clamp(-1.0, 1.0).asin();
    let b = (-d[0]).atan2(-d[2]);
    let mut yaw = b.to_degrees();
    if yaw.abs() < 1e-4 {
        yaw = 0.0;
    }
    [a.to_degrees() as f32, yaw as f32, 0.0]
}

/// Linear RGB normalised so the largest channel is 1 (0 stays 0).
pub fn hue(c: [f32; 3]) -> [f32; 3] {
    let m = c[0].max(c[1]).max(c[2]);
    if m <= 1e-6 { [0.0; 3] } else { [c[0] / m, c[1] / m, c[2] / m] }
}

pub fn nums(s: &str) -> Vec<f32> {
    crate::ac1::ini::nums_of(s).into_iter().map(|x| x as f32).collect()
}

/// An AC1 colour triple ("r,g,b[,a]").
pub fn rgb(s: Option<&String>) -> [f32; 3] {
    let v = s.map(|s| nums(s)).unwrap_or_default();
    [v.first().copied().unwrap_or(0.0), v.get(1).copied().unwrap_or(0.0), v.get(2).copied().unwrap_or(0.0)]
}

// ------------------------------------------------------------- CSP config --

/// One section of a Custom Shaders Patch config, in file order, every key
/// kept (CSP repeats sections like `[CustomEmissive]` and keys like `@`).
pub struct CspSection {
    pub name: String,
    pub keys: Vec<(String, String)>,
}

impl CspSection {
    pub fn get(&self, k: &str) -> Option<&str> {
        self.keys.iter().find(|(a, _)| a.eq_ignore_ascii_case(k)).map(|(_, v)| v.as_str())
    }
}

/// The car's CSP config: `extension\ext_config.ini` in the car folder, else
/// the AC1 install's `extension\config\cars\loaded\<folder>.ini` (the
/// configs CSP ships for Kunos cars). Comments: `;` and `//`.
pub fn csp_config(car: &Car) -> Option<(std::path::PathBuf, Vec<CspSection>)> {
    csp_config_at(&car.dir, &car.folder)
}

/// `csp_config` for a car folder `dir` named `folder`.
pub fn csp_config_at(dir: &std::path::Path, folder: &str) -> Option<(std::path::PathBuf, Vec<CspSection>)> {
    let mut cands = vec![dir.join("extension").join("ext_config.ini")];
    // <ac1>\content\cars\<folder> -> <ac1>\extension\config\cars\...
    if let Some(root) = dir.parent().and_then(|p| p.parent()).and_then(|p| p.parent()) {
        for sub in ["loaded", "kunos", ""] {
            let mut p = root.join("extension").join("config").join("cars");
            if !sub.is_empty() {
                p = p.join(sub);
            }
            cands.push(p.join(format!("{folder}.ini")));
        }
    }
    for p in cands {
        if let Ok(b) = std::fs::read(&p) {
            return Some((p, parse_csp(&String::from_utf8_lossy(&b))));
        }
    }
    None
}

pub fn parse_csp(text: &str) -> Vec<CspSection> {
    let mut out: Vec<CspSection> = Vec::new();
    for raw in text.lines() {
        let mut l = raw;
        // a `;` or `//` outside quotes starts a comment
        let mut q = false;
        let b = l.as_bytes();
        let mut end = l.len();
        for i in 0..b.len() {
            if b[i] == b'"' {
                q = !q;
            }
            if !q && (b[i] == b';' || (b[i] == b'/' && b.get(i + 1) == Some(&b'/'))) {
                end = i;
                break;
            }
        }
        l = l[..end].trim();
        if l.is_empty() {
            continue;
        }
        if l.starts_with('[') {
            if let Some(e) = l.find(']') {
                out.push(CspSection { name: l[1..e].trim().to_uppercase(), keys: Vec::new() });
                continue;
            }
        }
        if let Some((k, v)) = l.split_once('=') {
            if out.is_empty() {
                out.push(CspSection { name: String::new(), keys: Vec::new() });
            }
            out.last_mut().unwrap().keys.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    out
}

/// Mesh names in a CSP list value ("a, b, c"; quotes dropped).
pub fn name_list(v: &str) -> Vec<String> {
    v.split(',').map(|s| s.trim().trim_matches('"').trim().to_lowercase()).filter(|s| !s.is_empty() && !s.starts_with('?')).collect()
}

// ---------------------------------------------------------------- textures --

/// A uniform RGBA8 texture (one 128 x 128 page) at `path`. `srgb`: header
/// format 6 (lamp masks), else 5 (cockpit masks), as stock stores them.
pub fn uniform_texture(files: &mut Files, path: &str, rgba: [u8; 4], srgb: bool) {
    let n = 128u32;
    let px: Vec<u8> = (0..n * n).flat_map(|_| rgba).collect();
    let img = crate::io::texture::Image { w: n, h: n, px };
    let fmt = if srgb { crate::io::texture::Fmt::Rgba8 } else { crate::io::texture::Fmt::Rgba8b };
    let e = crate::io::texture::encode(&img, fmt, srgb, path.rsplit('\\').next().unwrap_or(path));
    files.put(path, e.header);
    files.put(&format!("{path}mips"), e.mips);
}

// ------------------------------------------------------------- mesh edits --

/// The vertex range and index range of every section of every LOD of an
/// encoded `.mesh`: (lod, section name, vertex indices used).
pub fn section_vertices(lod: &Msg) -> Vec<(String, Vec<u32>)> {
    let idx: Vec<u32> = lod.bytes(11).map(|b| crate::io::pb::varints(b).into_iter().map(|x| x as u32).collect()).unwrap_or_default();
    let mut out = Vec::new();
    for s in lod.subs(4) {
        let a = s.varint(2).unwrap_or(0) as usize;
        let c = s.varint(3).unwrap_or(0) as usize;
        let mut v: Vec<u32> = idx.get(a..(a + c).min(idx.len())).map(|x| x.to_vec()).unwrap_or_default();
        v.sort_unstable();
        v.dedup();
        out.push((s.str(1).unwrap_or_default(), v));
    }
    out
}

/// Every LOD body of a mesh, opened for editing.
pub fn lod_indices(top: &Msg) -> Vec<usize> {
    top.idx(5)
}

/// f32 stream `f` of a LOD (positions f5, colours f13, ...).
pub fn stream(lod: &Msg, f: u32) -> Vec<f32> {
    lod.bytes(f).map(|b| b.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect()).unwrap_or_default()
}

pub fn set_stream(lod: &mut Msg, f: u32, v: &[f32]) {
    lod.set(f, Val::Bytes(crate::io::pb::f32_bytes(v)));
}

/// Section name -> (min, max) of its LOD0 vertices, for a mesh in `files`.
pub fn section_bounds(files: &Files, mesh: &str) -> HashMap<String, ([f32; 3], [f32; 3])> {
    let mut out = HashMap::new();
    let Some(top) = files.get(mesh).and_then(|b| Msg::parse(b).ok()) else { return out };
    let Some(lod) = top.subs(5).into_iter().next() else { return out };
    let pos = stream(&lod, 5);
    for (name, vs) in section_vertices(&lod) {
        let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for v in vs {
            let i = v as usize * 3;
            if i + 2 >= pos.len() {
                continue;
            }
            for k in 0..3 {
                lo[k] = lo[k].min(pos[i + k]);
                hi[k] = hi[k].max(pos[i + k]);
            }
        }
        if lo[0].is_finite() {
            out.insert(name, (lo, hi));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_angles() {
        // car forward, level: (0, 180, 0) like every stock cockpit camera
        let e = euler_minus_z([0.0, 0.0, 1.0]);
        assert!(e[0].abs() < 1e-4 && (e[1].abs() - 180.0).abs() < 1e-3);
        // looking back and slightly down
        let e = euler_minus_z([0.0, -0.1, -1.0]);
        assert!(e[0] < -5.0 && e[1].abs() < 1e-3);
    }

    #[test]
    fn csp_sections_repeat() {
        let s = parse_csp("[CustomEmissive]\nMeshes = a, b ; x\n@ = TurningLightsFront, Channel = 3\n[CustomEmissive]\nMeshes = \"c\"\n[]\n@ = ReverseLights, Meshes = d\n");
        assert_eq!(s.len(), 3);
        assert_eq!(name_list(s[0].get("meshes").unwrap()), vec!["a", "b"]);
        assert_eq!(name_list(s[1].get("Meshes").unwrap()), vec!["c"]);
        assert_eq!(s[2].keys[0].1, "ReverseLights, Meshes = d");
    }
}
