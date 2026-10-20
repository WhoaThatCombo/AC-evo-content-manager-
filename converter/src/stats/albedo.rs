//! Effective albedo of track materials, for the brightness calibration
//! (requirement track-textures-materials: converted albedo per class within
//! the stock interquartile range).
//!
//! Per material: the mean linear colour it shows, estimated from its own
//! textures (one small mip, decoded) and variables, with the layered track
//! shader's maths (track/look.rs: Base / Red / Green / Blue weighted by the
//! SpatMap or Splatmap_Constant, sum or multiply mode, then the Detail
//! multiplier). Classes by material name (ours, the same rule for stock and
//! converted tracks): road (the DynamicTrack shader, asph / tarmac / road),
//! kerb, grass (grass / terrain / gravel / sand / dirt), tree (tree / leaf /
//! bush / branch / pine), building (build / wall / house / stand / pit /
//! concrete / brick / roof), other.
//!
//! `aceconv albedo <track dir | content.kspkg> [stock track id...]`.

use std::collections::{BTreeMap, HashMap};

use crate::io::material::{get_prop, get_slot};
use crate::io::pb::{self, Msg};

fn lin(v: u8) -> f32 {
    let c = v as f32 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// One small single-page mip of a `.texture` (`page(i)` returns page i of
/// its `.texturemips`, 64 KiB), decoded: (width, height, RGBA texels,
/// sRGB format).
pub fn texture_small(header: &[u8], page: &dyn Fn(usize) -> Option<Vec<u8>>) -> Option<(u32, u32, Vec<[u8; 4]>, bool)> {
    texture_tile(header, page, 64)
}

/// The largest mip of a `.texture` that fits one tile and `max` texels a
/// side, decoded (as [`texture_small`]).
pub fn texture_tile(header: &[u8], page: &dyn Fn(usize) -> Option<Vec<u8>>, max: u32) -> Option<(u32, u32, Vec<[u8; 4]>, bool)> {
    let h = Msg::parse(header).ok()?;
    let (w, ht, mips, fmt) = (h.varint(1)? as u32, h.varint(2)? as u32, h.varint(3).unwrap_or(1) as u32, h.varint(4)? as u32);
    let pt = h.sub(12)?;
    let (tw, th) = (pt.varint(1)? as u32, pt.varint(2)? as u32);
    let first: Vec<u64> = pt.bytes(4).map(pb::varints).unwrap_or_default();
    let (bpb, block, srgb) = match fmt {
        34 => (16, true, true),
        33 | 26 | 35 => (16, true, false),
        13 | 17 => (if fmt == 13 { 8 } else { 16 }, true, true),
        12 | 27 | 16 => (if fmt == 16 { 16 } else { 8 }, true, false),
        6 => (4, false, true),
        5 => (4, false, false),
        _ => return None,
    };
    if fmt == 26 {
        return None;
    }
    // the largest mip that fits one tile, at most `max` px
    let mut lvl = 0;
    while lvl + 1 < mips && ((w >> lvl).max(1) > tw.min(max) || (ht >> lvl).max(1) > th.min(max)) {
        lvl += 1;
    }
    let (mw, mh) = ((w >> lvl).max(1), (ht >> lvl).max(1));
    let pg = page(*first.get(lvl as usize)? as usize)?;
    let pitch = if block { (tw.div_ceil(4) * bpb) as usize } else { (tw * 4) as usize };
    let rows = if block { mh.div_ceil(4) } else { mh } as usize;
    let rb = if block { (mw.div_ceil(4) * bpb) as usize } else { (mw * 4) as usize };
    let mut data = Vec::with_capacity(rows * rb);
    for r in 0..rows {
        data.extend_from_slice(pg.get(r * pitch..r * pitch + rb)?);
    }
    let (bw, bh) = if block { (mw.div_ceil(4) * 4, mh.div_ceil(4) * 4) } else { (mw, mh) };
    let mut words = vec![0u32; (bw * bh) as usize];
    let ok = match fmt {
        34 | 33 => texture2ddecoder::decode_bc7(&data, bw as usize, bh as usize, &mut words).is_ok(),
        13 | 12 => texture2ddecoder::decode_bc1(&data, bw as usize, bh as usize, &mut words).is_ok(),
        17 | 16 => texture2ddecoder::decode_bc3(&data, bw as usize, bh as usize, &mut words).is_ok(),
        27 => texture2ddecoder::decode_bc4(&data, bw as usize, bh as usize, &mut words).is_ok(),
        35 => texture2ddecoder::decode_bc5(&data, bw as usize, bh as usize, &mut words).is_ok(),
        _ => {
            for (i, c) in data.chunks_exact(4).enumerate() {
                words[i] = (c[3] as u32) << 24 | (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32;
            }
            true
        }
    };
    if !ok {
        return None;
    }
    let mut px = Vec::with_capacity((mw * mh) as usize);
    for y in 0..mh {
        for x in 0..mw {
            let v = words[(y * bw + x) as usize];
            px.push([(v >> 16) as u8, (v >> 8) as u8, v as u8, (v >> 24) as u8]);
        }
    }
    Some((mw, mh, px, srgb))
}

/// Decode `rw` x `rh` texels of one page (row pitch `pitch` bytes).
fn decode_page(pg: &[u8], pitch: usize, rw: u32, rh: u32, fmt: u32, block: bool, bpb: u32) -> Option<Vec<[u8; 4]>> {
    let rows = if block { rh.div_ceil(4) } else { rh } as usize;
    let rb = if block { (rw.div_ceil(4) * bpb) as usize } else { (rw * 4) as usize };
    let mut data = Vec::with_capacity(rows * rb);
    for r in 0..rows {
        data.extend_from_slice(pg.get(r * pitch..r * pitch + rb)?);
    }
    let (bw, bh) = if block { (rw.div_ceil(4) * 4, rh.div_ceil(4) * 4) } else { (rw, rh) };
    let mut words = vec![0u32; (bw * bh) as usize];
    let ok = match fmt {
        34 | 33 => texture2ddecoder::decode_bc7(&data, bw as usize, bh as usize, &mut words).is_ok(),
        13 | 12 => texture2ddecoder::decode_bc1(&data, bw as usize, bh as usize, &mut words).is_ok(),
        17 | 16 => texture2ddecoder::decode_bc3(&data, bw as usize, bh as usize, &mut words).is_ok(),
        27 => texture2ddecoder::decode_bc4(&data, bw as usize, bh as usize, &mut words).is_ok(),
        35 => texture2ddecoder::decode_bc5(&data, bw as usize, bh as usize, &mut words).is_ok(),
        _ => {
            for (i, c) in data.chunks_exact(4).enumerate() {
                words[i] = (c[3] as u32) << 24 | (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32;
            }
            true
        }
    };
    if !ok {
        return None;
    }
    let mut px = Vec::with_capacity((rw * rh) as usize);
    for y in 0..rh {
        for x in 0..rw {
            let v = words[(y * bw + x) as usize];
            px.push([(v >> 16) as u8, (v >> 8) as u8, v as u8, (v >> 24) as u8]);
        }
    }
    Some(px)
}

/// The largest mip of a `.texture` with both sides at most `max`, decoded
/// over all its tiles (docs/FORMATS.md: tiles row-major, one 64 KiB page
/// each, from the mip's first page): (width, height, RGBA, sRGB).
pub fn texture_mip(header: &[u8], page: &dyn Fn(usize) -> Option<Vec<u8>>, max: u32) -> Option<(u32, u32, Vec<[u8; 4]>, bool)> {
    let h = Msg::parse(header).ok()?;
    let (w, ht, mips, fmt) = (h.varint(1)? as u32, h.varint(2)? as u32, h.varint(3).unwrap_or(1) as u32, h.varint(4)? as u32);
    let pt = h.sub(12)?;
    let (tw, th) = (pt.varint(1)? as u32, pt.varint(2)? as u32);
    let first: Vec<u64> = pt.bytes(4).map(pb::varints).unwrap_or_default();
    let (bpb, block, srgb) = match fmt {
        34 => (16, true, true),
        33 | 35 => (16, true, false),
        13 | 17 => (if fmt == 13 { 8 } else { 16 }, true, true),
        12 | 27 | 16 => (if fmt == 16 { 16 } else { 8 }, true, false),
        6 => (4, false, true),
        5 => (4, false, false),
        _ => return None,
    };
    let mut lvl = 0;
    while lvl + 1 < mips && ((w >> lvl).max(1) > max || (ht >> lvl).max(1) > max) {
        lvl += 1;
    }
    let (mw, mh) = ((w >> lvl).max(1), (ht >> lvl).max(1));
    let pitch = if block { (tw.div_ceil(4) * bpb) as usize } else { (tw * 4) as usize };
    let (nx, ny) = (mw.div_ceil(tw), mh.div_ceil(th));
    let p0 = *first.get(lvl as usize)? as usize;
    let mut px = vec![[0u8; 4]; (mw * mh) as usize];
    for ty in 0..ny {
        for tx in 0..nx {
            let (rw, rh) = ((mw - tx * tw).min(tw), (mh - ty * th).min(th));
            let pg = page(p0 + (ty * nx + tx) as usize)?;
            let t = decode_page(&pg, pitch, rw, rh, fmt, block, bpb)?;
            for y in 0..rh {
                for x in 0..rw {
                    px[((ty * th + y) * mw + tx * tw + x) as usize] = t[(y * rw + x) as usize];
                }
            }
        }
    }
    Some((mw, mh, px, srgb))
}

/// Mean colour of a `.texture` from one small single-page mip of its
/// `.texturemips` (`page(i)` returns page i, 64 KiB). Linear for sRGB
/// formats; masks raw 0..1.
pub fn texture_mean(header: &[u8], page: &dyn Fn(usize) -> Option<Vec<u8>>) -> Option<[f32; 4]> {
    let (_, _, px, srgb) = texture_small(header, page)?;
    let mut acc = [0f64; 4];
    for c in &px {
        for k in 0..3 {
            acc[k] += if srgb { lin(c[k]) as f64 } else { c[k] as f64 / 255.0 };
        }
        acc[3] += c[3] as f64 / 255.0;
    }
    let n = px.len() as f64;
    Some(acc.map(|a| (a / n.max(1.0)) as f32))
}

pub fn class_of(name: &str, shader: &str) -> &'static str {
    let t = name.to_ascii_lowercase();
    let any = |w: &[&str]| w.iter().any(|x| t.contains(x));
    if any(&["curb", "kerb", "cordol", "rumble"]) {
        "kerb"
    } else if shader == "DynamicTrack" || any(&["asph", "tarmac", "road", "asfalt"]) {
        "road"
    } else if any(&["tree", "leaf", "leaves", "bush", "branch", "pine", "foliage", "forest", "birch", "cedar", "populus"]) {
        "tree"
    } else if any(&["grass", "terrain", "gravel", "sand", "dirt", "soil", "lawn", "ground"]) {
        "grass"
    } else if any(&["build", "wall", "house", "stand", "pit", "concrete", "brick", "roof", "tower", "garage"]) {
        "building"
    } else {
        "other"
    }
}

/// Estimated mean linear albedo of one material. `tex(path)` = mean colour
/// of a texture (None when unknown).
pub fn material_albedo(m: &Msg, tex: &dyn Fn(&str) -> Option<[f32; 4]>) -> Option<[f32; 3]> {
    m.str(1)?;
    let p1 = |n: &str| get_prop(m, n, 1).map(|v| v[0]);
    let p4 = |n: &str| get_prop(m, n, 4).map(|v| [v[0], v[1], v[2], v[3]]);
    let on = |n: &str| p1(n).map(|v| v != 0.0).unwrap_or(false);
    let map = |slot: &str| get_slot(m, slot).filter(|s| !s.is_empty()).and_then(|s| tex(&s));
    let colour = |prefix: &str, slot: &str, has: &str, modul: &str| -> [f32; 3] {
        let bc = p4(&format!("{prefix}Basecolor")).map(|c| [c[0], c[1], c[2]]);
        match (on(has), map(slot)) {
            (true, Some(t)) => {
                let k = if on(modul) { bc.unwrap_or([1.0; 3]) } else { [1.0; 3] };
                [t[0] * k[0], t[1] * k[1], t[2] * k[2]]
            }
            _ => bc.unwrap_or([0.5; 3]),
        }
    };
    if get_prop(m, "Base_HasBaseColorMap", 1).is_none() {
        // the simple shaders (DefaultMaterial, ...)
        return Some(colour("", "txDiffuse", "HasBaseColorMap", "Modulation_BaseColor"));
    }
    let base = colour("Base_", "Base_BaseColorMap", "Base_HasBaseColorMap", "Modulation_BaseBaseColor");
    if get_prop(m, "Splatmap_UseConstant", 1).is_none() || !(on("Red_Enable") || on("Green_Enable") || on("Blue_Enable")) {
        return Some(base);
    }
    let mut w: [f32; 3] = if on("Splatmap_UseConstant") {
        p4("Splatmap_Constant").map(|c| [c[0], c[1], c[2]]).unwrap_or([0.0; 3])
    } else {
        map("SpatMap").map(|s| [s[0], s[1], s[2]]).unwrap_or([0.0; 3])
    };
    for (i, l) in ["Red", "Green", "Blue"].iter().enumerate() {
        if !on(&format!("{l}_Enable")) {
            w[i] = 0.0;
        }
    }
    if !on("Splatmap_DisableRestrictBlendToTwoLayers") {
        let lo = w.iter().cloned().fold(f32::INFINITY, f32::min);
        if let Some(i) = w.iter().position(|x| *x == lo) {
            w[i] = 0.0;
        }
    }
    let wb = (1.0 - w[0] - w[1] - w[2]).clamp(0.0, 1.0);
    let sum = wb + w[0] + w[1] + w[2] + 0.001;
    let (wb, w) = (wb / sum, w.map(|x| x / sum));
    let layers: Vec<[f32; 3]> = ["Red", "Green", "Blue"].iter().map(|l| colour(&format!("{l}_"), &format!("{l}_BaseColorMap"), &format!("{l}_HasBaseColorMap"), &format!("Modulation_{l}BaseColor"))).collect();
    let mut a = [0f32; 3];
    for c in 0..3 {
        a[c] = if on("Splatmap_MultiplyBaseColor") {
            (1.0 + (base[c] - 1.0) * wb) * (0..3).map(|i| 1.0 + (layers[i][c] - 1.0) * w[i]).product::<f32>()
        } else {
            wb * base[c] + (0..3).map(|i| w[i] * layers[i][c]).sum::<f32>()
        };
        if on("Detail_Enable") && on("Detail_BaseColor") {
            let dm = p1("Detail_BaseColorMultiplier").unwrap_or(1.0);
            a[c] *= dm * base[c] + (1.0 - dm * base[c]) * wb;
        }
        a[c] = a[c].clamp(0.0, 1.0);
    }
    Some(a)
}

/// A material with a parent (f20 baseMaterialPath) takes the parent's
/// variables and slots except the ones its f21 / f22 override lists name
/// (Renderer.proto MaterialData; stock child materials keep stale copies of
/// everything else).
pub fn resolve(m: Msg, read: &dyn Fn(&str) -> Option<Vec<u8>>, depth: u32) -> Msg {
    let Some(parent) = m.str(20).filter(|p| !p.is_empty() && depth < 4) else { return m };
    let Some(pm) = read(&parent).and_then(|b| Msg::parse(&b).ok()) else { return m };
    let mut pm = resolve(pm, read, depth + 1);
    let names = |f: u32| -> Vec<String> { m.subs(f).iter().filter_map(|e| e.str(1)).collect() };
    let entry_name = |v: &crate::io::pb::Val| -> Option<String> {
        match v {
            crate::io::pb::Val::Bytes(b) => Msg::parse(b).ok().and_then(|s| s.str(1)),
            crate::io::pb::Val::Msg(s) => s.str(1),
            _ => None,
        }
    };
    for (field, list) in [(4u32, names(21)), (5u32, names(22))] {
        for n in list {
            let child = m.fields.iter().find(|f| f.num == field && entry_name(&f.val).as_deref() == Some(n.as_str())).cloned();
            pm.fields.retain(|f| !(f.num == field && entry_name(&f.val).as_deref() == Some(n.as_str())));
            if let Some(c) = child {
                pm.fields.push(c);
            }
        }
    }
    // the child's own shader and render state win
    pm.fields.retain(|f| !matches!(f.num, 1 | 2 | 20 | 21 | 22));
    for f in m.fields.iter().filter(|f| matches!(f.num, 1 | 2)) {
        pm.fields.push(f.clone());
    }
    pm
}

/// Materials the measurement leaves out: collision / editor ones, blended
/// decals, glass, particles (not surfaces anyone compares).
pub fn skipped(name: &str, shader: &str, m: &Msg) -> bool {
    name.contains("physics") || shader.contains("Glass") || shader == "Particle" || shader == "IdealLine" || shader == "Skidmarks" || m.varint(2) == Some(2)
}

pub struct Report {
    /// class -> sorted luma values
    pub by_class: BTreeMap<&'static str, Vec<f32>>,
    pub rows: Vec<(String, String, &'static str, f32)>,
}

pub fn luma(a: [f32; 3]) -> f32 {
    0.2126 * a[0] + 0.7152 * a[1] + 0.0722 * a[2]
}

/// Measure a set of materials. `read(path)` = file bytes (header or
/// material), `page(path, i)` = page i of a `.texturemips`.
pub fn measure(materials: &[(String, Vec<u8>)], header: &dyn Fn(&str) -> Option<Vec<u8>>, page: &dyn Fn(&str, usize) -> Option<Vec<u8>>) -> Report {
    let mut cache: std::cell::RefCell<HashMap<String, Option<[f32; 4]>>> = Default::default();
    let tex = |p: &str| -> Option<[f32; 4]> {
        let k = p.to_lowercase();
        if let Some(v) = cache.borrow().get(&k) {
            return *v;
        }
        let v = header(p).and_then(|h| texture_mean(&h, &|i| page(&format!("{p}mips"), i)));
        cache.borrow_mut().insert(k, v);
        v
    };
    let mut rep = Report { by_class: BTreeMap::new(), rows: Vec::new() };
    for (path, b) in materials {
        let Ok(m) = Msg::parse(b) else { continue };
        let m = resolve(m, header, 0);
        let shader = m.str(1).unwrap_or_default();
        let name = path.rsplit('\\').next().unwrap_or(path).trim_end_matches(".material").to_string();
        if skipped(&name, &shader, &m) {
            continue;
        }
        if let Some(a) = material_albedo(&m, &tex) {
            let c = class_of(&name, &shader);
            rep.by_class.entry(c).or_default().push(luma(a));
            rep.rows.push((name, shader, c, luma(a)));
        }
    }
    let _ = &mut cache;
    for v in rep.by_class.values_mut() {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    }
    rep
}

pub fn quart(v: &[f32]) -> (f32, f32, f32) {
    let q = |p: f32| v[((v.len() - 1) as f32 * p).round() as usize];
    (q(0.25), q(0.5), q(0.75))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::material::{set_prop, set_slot};

    #[test]
    fn layered_sum_mode() {
        let mut m = Msg::default();
        m.set_str(1, "DynamicTrackMultilayer");
        for (k, v) in [("Base_HasBaseColorMap", 1.0), ("Modulation_BaseBaseColor", 1.0), ("Splatmap_UseConstant", 1.0), ("Red_Enable", 1.0), ("Red_HasBaseColorMap", 1.0),
            ("Splatmap_DisableRestrictBlendToTwoLayers", 1.0), ("Detail_Enable", 1.0), ("Detail_BaseColor", 1.0), ("Detail_BaseColorMultiplier", 2.0)] {
            set_prop(&mut m, k, &[v]);
        }
        set_prop(&mut m, "Base_Basecolor", &[1.0, 1.0, 1.0, 1.0]);
        set_prop(&mut m, "Splatmap_Constant", &[1.0, 0.0, 0.0, 0.0]);
        set_slot(&mut m, "Base_BaseColorMap", "b");
        set_slot(&mut m, "Red_BaseColorMap", "r");
        let tex = |p: &str| Some(if p == "b" { [0.4; 4] } else { [0.25; 4] });
        let a = material_albedo(&m, &tex).unwrap();
        // fully covered by Red: L x DetMul x Base = 0.25 x 2 x 0.4
        assert!((a[0] - 0.2).abs() < 0.01, "{a:?}");
        assert_eq!(class_of("asph4", "DynamicTrack"), "road");
        assert_eq!(class_of("curb-grass", "DefaultMaterial"), "kerb");
    }
}

/// `measure` over a track folder (`<dir>\materials\*.material`, textures
/// from the folder, else from `pkg`): what `aceconv albedo <track dir>`
/// prints, and what the track stage's brightness fit aims at.
pub fn measure_dir(dir: &std::path::Path, pkg: Option<&crate::io::kspkg::Pkg>) -> std::io::Result<Report> {
    let id = dir.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
    let prefix = format!("content\\tracks\\{id}\\").to_lowercase();
    let local = |p: &str| -> Option<std::path::PathBuf> { p.to_lowercase().strip_prefix(&prefix).map(|r| dir.join(r.replace('\\', "/"))) };
    let mut mats = Vec::new();
    for e in std::fs::read_dir(dir.join("materials"))?.flatten() {
        mats.push((format!("{prefix}materials\\{}", e.file_name().to_string_lossy()), std::fs::read(e.path())?));
    }
    mats.sort();
    let from_pkg = |p: &str| -> Option<Vec<u8>> { pkg.and_then(|g| g.find(p).and_then(|e| g.read(e).ok())) };
    let page_pkg = |p: &str, i: usize| -> Option<Vec<u8>> {
        let g = pkg?;
        let e = g.find(p)?;
        g.raw(e).and_then(|r| r.get(i * 65536..(i + 1) * 65536).map(|s| s.to_vec()))
    };
    let header = |p: &str| local(p).and_then(|f| std::fs::read(f).ok()).or_else(|| from_pkg(p));
    let page = |p: &str, i: usize| -> Option<Vec<u8>> {
        match local(p) {
            Some(f) => {
                use std::io::{Read, Seek, SeekFrom};
                let mut fh = std::fs::File::open(f).ok()?;
                fh.seek(SeekFrom::Start((i * 65536) as u64)).ok()?;
                let mut b = vec![0u8; 65536];
                fh.read_exact(&mut b).ok()?;
                Some(b)
            }
            None => page_pkg(p, i),
        }
    };
    Ok(measure(&mats, &header, &page))
}
