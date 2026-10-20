//! `aceconv make-donor <content.kspkg> <out dir>`: the converter's template
//! car and template materials, built from the player's own game files.
//!
//! The converter clones a donor car (`ks_modded_car`) and grafts the AC1 car
//! onto it. The model is Kunos's SDK sample mod: the stock
//! Abarth 695 Biposto re-exported through the AC EVO editor and renamed. Its
//! layout is reproduced here path for path (`donor_layout.tsv`), every file
//! sourced from content.kspkg:
//!
//! * `copy` - a stock file verbatim. Most are byte-identical to the
//!   Kunos SDK sample's (curves, displays, fonts, the Abarth's textures,
//!   common tyre physics); a few slots take the stock file the sample was made from
//!   (disc textures from `road_drill_3_road_1`, the common windows dirt for
//!   the sample's own `INT_Windows_Dirt`).
//! * `ren` - a stock file with the id renamed, through a protobuf-aware
//!   rewrite (length prefixes re-encoded, so the 21-char stock id can become
//!   the 13-char donor id), plus the sample's deterministic edits (`post`):
//!   brand-neutral names, mod tyre compound ids 5001.., common assets
//!   pointed at the car-local copies, lower-cased data paths, the engine
//!   throttle map removed, perf-mode names, ...
//! * `gen` - small files written here: the `.moddedcarcontent`, the
//!   localisation, a neutral `modded_brand` badge/logo.
//!
//! Where the SDK sample holds the editor's own re-export (simplified meshes,
//! re-encoded textures, re-authored materials, its FMOD bank) we take the
//! stock Abarth file instead: functionally the same car, and the real
//! game data. The two FMOD banks are not reproduced (the converter always
//! writes its own; on a sound failure the stock Abarth banks stay referenced).
//!
//! `refmats\*.material` (the converter's templates for new sections) are
//! generated from stock materials of the same shader: texture slots cleared
//! and the switches that make a plain single-layer material set (see
//! `refmats`). `templates\...\ks_audi_r8_lms_gt4_evo_elec.carelectronics`
//! (the electronics template) is the stock file.

use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::io::kspkg::Pkg;

pub const DONOR_ZIP: &str = "ks_modded_car.zip";
/// bump when the builder's output changes: cached donors are remade
pub const BUILDER_VERSION: u32 = 1;
const CAR: &str = "content\\cars\\ks_modded_car\\";
const STOCK: &str = "content\\cars\\ks_abarth_695_biposto\\";
const LAYOUT: &str = include_str!("donor_layout.tsv");
const LOCAL: &str = include_str!("donor_local.tsv");
const ELEC: &str = "content\\cars\\ks_audi_r8_lms_gt4_evo\\data\\ks_audi_r8_lms_gt4_evo_elec.carelectronics";

// ------------------------------------------------------- schema-less tree --

/// A protobuf value whose length-delimited payloads are classified as a
/// sub-message, a printable string or raw bytes (the same rule as the
/// research tools, so renames land on strings and every length is rebuilt).
#[derive(Clone, Debug)]
enum N {
    V(u64),
    F64([u8; 8]),
    F32([u8; 4]),
    S(Vec<u8>),
    B(Vec<u8>),
    M(T),
}
type T = Vec<(u32, N)>;

enum Raw<'a> {
    V(u64),
    F64([u8; 8]),
    F32([u8; 4]),
    L(&'a [u8]),
}

fn scan(d: &[u8]) -> Option<Vec<(u32, Raw<'_>)>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < d.len() {
        let key = crate::io::pb::read_varint(d, &mut i).ok()?;
        let num = key >> 3;
        if num == 0 || num > 100_000 {
            return None;
        }
        let v = match key & 7 {
            0 => Raw::V(crate::io::pb::read_varint(d, &mut i).ok()?),
            1 => {
                let s = d.get(i..i + 8)?;
                i += 8;
                Raw::F64(s.try_into().unwrap())
            }
            5 => {
                let s = d.get(i..i + 4)?;
                i += 4;
                Raw::F32(s.try_into().unwrap())
            }
            2 => {
                let n = crate::io::pb::read_varint(d, &mut i).ok()? as usize;
                let s = d.get(i..i.checked_add(n)?)?;
                i += n;
                Raw::L(s)
            }
            _ => return None,
        };
        out.push((num as u32, v));
    }
    Some(out)
}

fn is_str(b: &[u8]) -> bool {
    !b.is_empty() && b.iter().all(|&c| (0x20..=0x7e).contains(&c) || c == b'\t' || c == b'\r' || c == b'\n')
}

/// printable bytes parse as a message only by accident unless they hold a
/// length-delimited field
fn msgish(v: &[u8]) -> bool {
    if !is_str(v) {
        return true;
    }
    scan(v).map(|f| f.iter().any(|(_, r)| matches!(r, Raw::L(_)))).unwrap_or(false)
}

fn parse(d: &[u8], depth: u32) -> Option<T> {
    let mut out = Vec::new();
    for (num, r) in scan(d)? {
        out.push((num, match r {
            Raw::V(v) => N::V(v),
            Raw::F64(b) => N::F64(b),
            Raw::F32(b) => N::F32(b),
            Raw::L(v) => {
                let t = if depth < 40 && !v.is_empty() && msgish(v) { parse(v, depth + 1) } else { None };
                match t {
                    Some(t) => N::M(t),
                    None if is_str(v) => N::S(v.to_vec()),
                    None => N::B(v.to_vec()),
                }
            }
        }));
    }
    Some(out)
}

fn encode(t: &T) -> Vec<u8> {
    use crate::io::pb::write_varint as wv;
    let mut out = Vec::new();
    for (num, v) in t {
        let n = *num as u64;
        match v {
            N::V(x) => {
                wv(&mut out, n << 3);
                wv(&mut out, *x);
            }
            N::F64(b) => {
                wv(&mut out, n << 3 | 1);
                out.extend_from_slice(b);
            }
            N::F32(b) => {
                wv(&mut out, n << 3 | 5);
                out.extend_from_slice(b);
            }
            N::S(b) | N::B(b) => {
                wv(&mut out, n << 3 | 2);
                wv(&mut out, b.len() as u64);
                out.extend_from_slice(b);
            }
            N::M(m) => {
                let b = encode(m);
                wv(&mut out, n << 3 | 2);
                wv(&mut out, b.len() as u64);
                out.extend_from_slice(&b);
            }
        }
    }
    out
}

fn map_strings(t: &mut T, f: &dyn Fn(&[u8]) -> Vec<u8>) {
    for (_, v) in t.iter_mut() {
        match v {
            N::M(m) => map_strings(m, f),
            N::S(b) | N::B(b) => *b = f(b),
            _ => {}
        }
    }
}

fn set(t: &mut T, num: u32, val: N) {
    match t.iter_mut().find(|(n, _)| *n == num) {
        Some(x) => x.1 = val,
        None => t.push((num, val)),
    }
}

fn drop_field(t: &mut T, num: u32) {
    t.retain(|(n, _)| *n != num);
}

fn subs(t: &mut T, num: u32) -> Vec<&mut T> {
    t.iter_mut().filter(|(n, _)| *n == num).filter_map(|(_, v)| if let N::M(m) = v { Some(m) } else { None }).collect()
}

fn s(x: &str) -> N {
    N::S(x.as_bytes().to_vec())
}

// --------------------------------------------------------------- renaming --

struct Renamer {
    local: regex::bytes::Regex,
    local_map: HashMap<String, String>,
    textures: regex::bytes::Regex,
    presets: regex::bytes::Regex,
    data: regex::bytes::Regex,
}

impl Renamer {
    fn new() -> Renamer {
        let mut local_map = HashMap::new();
        for l in LOCAL.lines().filter(|l| !l.is_empty()) {
            let (a, b) = l.split_once('\t').unwrap();
            local_map.insert(a.to_lowercase(), b.to_string());
        }
        let mut keys: Vec<&String> = local_map.keys().collect();
        keys.sort_by(|a, b| b.len().cmp(&a.len()).then(a.cmp(b)));
        let alt = keys.iter().map(|k| regex::escape(k)).collect::<Vec<_>>().join("|");
        Renamer {
            local: regex::bytes::Regex::new(&format!("(?i-u){alt}")).unwrap(),
            local_map,
            textures: regex::bytes::Regex::new(r"(?i-u)content\\cars\\ks_abarth_695_biposto\\textures\\").unwrap(),
            presets: regex::bytes::Regex::new(r"(?-u)(preset|design|rim|tyres|rimdesign)_695b").unwrap(),
            data: regex::bytes::Regex::new(r"(?i-u)content\\cars\\ks_modded_car\\data\\[\x21-\x7e]*").unwrap(),
        }
    }

    /// stock id -> donor id in one string (or raw blob); `lower`: lower-case
    /// data\ paths (the sample does, except in its generated finalstate)
    fn ren(&self, b: &[u8], lower: bool) -> Vec<u8> {
        // sound stays on the stock Abarth banks: the converter writes its own
        if b.starts_with(b"event:/") || b.starts_with(b"content\\sfx\\") {
            return b.to_vec();
        }
        // Kunos's actor names its own driver animations "abarth_965_..."
        let mut v = replace(b, b"abarth_965_biposto_driver_", b"abarth_695_biposto_driver_");
        v = self.local.replace_all(&v, |c: &regex::bytes::Captures| {
            let k = String::from_utf8_lossy(&c[0]).to_lowercase();
            self.local_map[&k].as_bytes().to_vec()
        }).into_owned();
        v = self.textures.replace_all(&v, &b"content\\cars\\ks_modded_car\\texture\\"[..]).into_owned();
        v = replace(&v, b"ks_abarth_695_biposto", b"ks_modded_car");
        v = replace(&v, b"abarth_695_biposto", b"modded_car");
        v = self.presets.replace_all(&v, &b"${1}_modded_car"[..]).into_owned();
        if !lower {
            return v;
        }
        // the sample lower-cases every data\ reference (texture paths keep case)
        self.data.replace_all(&v, |c: &regex::bytes::Captures| c[0].to_ascii_lowercase()).into_owned()
    }
}

fn replace(h: &[u8], a: &[u8], b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(h.len());
    let mut i = 0;
    while let Some(p) = memchr::memmem::find(&h[i..], a) {
        out.extend_from_slice(&h[i..i + p]);
        out.extend_from_slice(b);
        i += p + a.len();
    }
    out.extend_from_slice(&h[i..]);
    out
}

/// "695 Biposto Visual 1" / "Abarth Paint Rally Beige" -> "modded_car ..."
fn neutral_name(b: &[u8]) -> Vec<u8> {
    for p in [&b"695 Biposto "[..], b"Abarth "] {
        if let Some(rest) = b.strip_prefix(p) {
            let mut v = b"modded_car ".to_vec();
            v.extend(rest.to_ascii_lowercase());
            return v;
        }
    }
    b.to_vec()
}

fn rename_strings(t: &mut T, num: u32, f: impl Fn(&[u8]) -> Vec<u8>) {
    for (n, v) in t.iter_mut() {
        if *n == num {
            if let N::S(b) = v {
                *b = f(b);
            }
        }
    }
}

/// The Kunos SDK sample's deterministic edits.
fn post(rel: &str, t: &mut T) {
    let low = rel.to_lowercase();
    if low.ends_with(".mechanicalcarpreset") || low.ends_with(".visualcarpreset") {
        set(t, 1, s("modded_brand"));
        rename_strings(t, 2, neutral_name);
        if low.ends_with(".visualcarpreset") {
            drop_field(t, 19);
        } else {
            // default compound = the first mod compound; no stock perf stats
            set(t, 7, N::V(5001));
            set(t, 19, N::B(Vec::new()));
        }
    }
    if low.ends_with(".oemmultilayercolor") {
        rename_strings(t, 2, neutral_name);
    }
    if low.ends_with(".design") || low.ends_with(".rimmesh") {
        set(t, 102, s("modded_brand"));
    }
    if low.ends_with(".rimmesh") {
        rename_strings(t, 11, |b| replace(b, b" Biposto", b""));
    }
    if low.ends_with(".design") {
        for sec in subs(t, 3) {
            if sec.iter().any(|(n, v)| *n == 1 && matches!(v, N::S(b) if b == b"EXT_CALIPER")) {
                for c in subs(sec, 6) {
                    for f in subs(c, 1) {
                        drop_field(f, 12);
                    }
                }
            }
        }
    }
    if low.ends_with(".compatibletyres") {
        // mod compounds are numbered 5001.. in list order, each with the
        // car-local tyre textures; the trailing f1 string goes last
        let mut ids: Vec<(u64, u64)> = Vec::new();
        for e in subs(t, 2) {
            for (n, v) in e.iter_mut() {
                if let (1, N::V(x)) = (*n, &*v) {
                    let id = 5001 + ids.len() as u64;
                    ids.push((*x, id));
                    *v = N::V(id);
                }
            }
        }
        for e in subs(t, 3) {
            for (n, v) in e.iter_mut() {
                if let (1, N::V(x)) = (*n, &*v) {
                    if let Some((_, id)) = ids.iter().find(|(o, _)| o == x) {
                        *v = N::V(*id);
                    }
                }
            }
            set(e, 2, s(&format!("{CAR}texture\\tyre\\tyre_road_1.tyretextures")));
        }
        let f1: Vec<(u32, N)> = t.iter().filter(|(n, _)| *n == 1).cloned().collect();
        drop_field(t, 1);
        t.extend(f1);
    }
    if low == "data/cardata.car" {
        for c in subs(t, 1) {
            set(c, 2, s("Modded Car"));
        }
        set(t, 202, s("Modded Car"));
        for (m, name) in subs(t, 30).into_iter().zip(["SPORT", "NORMAL"]) {
            set(m, 11, s(name));
        }
        for m in subs(t, 4) {
            drop_field(m, 4);
        }
        for x in subs(t, 25) {
            for m in subs(x, 4) {
                drop_field(m, 4);
            }
        }
    }
    if low.ends_with(".carengine") {
        // no throttle response map: the converter's throttle is linear
        drop_field(t, 4);
    }
    if low.ends_with(".clutch") {
        for m in subs(t, 4) {
            drop_field(m, 4);
        }
    }
    if low.ends_with(".actor") {
        for a in subs(t, 103) {
            drop_field(a, 510);
        }
    }
}

const BINARY: [&str; 4] = [".texturemips", ".bank", ".ttf", ".otf"];
const TEXT: [&str; 5] = [".html", ".js", ".css", ".less", ".loc"];

fn transform(r: &Renamer, rel: &str, data: &[u8]) -> Vec<u8> {
    let low = rel.to_lowercase();
    if BINARY.iter().any(|e| low.ends_with(e)) {
        return data.to_vec();
    }
    if !TEXT.iter().any(|e| low.ends_with(e)) {
        if let Some(mut t) = parse(data, 0) {
            let lower = !low.ends_with(".carfinalstate");
            map_strings(&mut t, &|b| r.ren(b, lower));
            post(rel, &mut t);
            return encode(&t);
        }
    }
    r.ren(data, true)
}

/// Rename identifiers in one donor file for a clone: every `(from, to)` pair
/// replaced case-insensitively (longest `from` first) in every string of a
/// protobuf file, with the length prefixes rebuilt; text files are replaced
/// as text; binary payloads (`.texturemips`, banks, fonts) are left alone.
/// A file that is neither keeps its bytes unless every replacement keeps the
/// length.
pub fn rename_content(path: &str, data: &[u8], pairs: &[(&str, &str)]) -> Vec<u8> {
    let low = path.to_lowercase();
    if BINARY.iter().any(|e| low.ends_with(e)) || pairs.is_empty() {
        return data.to_vec();
    }
    let re = rename_regex(pairs);
    let map: HashMap<String, String> = pairs.iter().map(|(a, b)| (a.to_lowercase(), b.to_string())).collect();
    let f = |b: &[u8]| -> Vec<u8> {
        re.replace_all(b, |c: &regex::bytes::Captures| map[&String::from_utf8_lossy(&c[0]).to_lowercase()].as_bytes().to_vec()).into_owned()
    };
    if !TEXT.iter().any(|e| low.ends_with(e)) {
        if let Some(mut t) = parse(data, 0) {
            map_strings(&mut t, &f);
            return encode(&t);
        }
        if pairs.iter().all(|(a, b)| a.len() == b.len()) {
            return f(data);
        }
        return data.to_vec();
    }
    f(data)
}

/// `rename_content` for a path (backslashes kept).
pub fn rename_path(path: &str, pairs: &[(&str, &str)]) -> String {
    if pairs.is_empty() {
        return path.to_string();
    }
    let re = rename_regex(pairs);
    let map: HashMap<String, String> = pairs.iter().map(|(a, b)| (a.to_lowercase(), b.to_string())).collect();
    String::from_utf8_lossy(&re.replace_all(path.as_bytes(), |c: &regex::bytes::Captures| map[&String::from_utf8_lossy(&c[0]).to_lowercase()].as_bytes().to_vec())).into_owned()
}

fn rename_regex(pairs: &[(&str, &str)]) -> regex::bytes::Regex {
    let mut keys: Vec<&str> = pairs.iter().map(|(a, _)| *a).collect();
    keys.sort_by(|a, b| b.len().cmp(&a.len()).then(a.cmp(b)));
    regex::bytes::Regex::new(&format!("(?i-u){}", keys.iter().map(|k| regex::escape(k)).collect::<Vec<_>>().join("|"))).unwrap()
}

// -------------------------------------------------------------- generated --

fn moddedcarcontent() -> Vec<u8> {
    // display name, year, brand, then the four content tags the sample carries
    let mut t: T = vec![(4, s("Modded Car")), (5, N::V(2026)), (10, s("modded_brand"))];
    for tag in [0u8, 11, 21, 71] {
        t.push((30, N::M(vec![(1, N::B(vec![tag]))])));
    }
    encode(&t)
}

/// en.cars.loc: the stock Abarth's entries, renamed like the files.
fn cars_loc(stock_loc: &str) -> Vec<u8> {
    let mut rows: Vec<(String, String)> = vec![
        ("brand_modded_brand".into(), "Modded Brand".into()),
        ("design_1".into(), "Design 1".into()),
    ];
    for l in stock_loc.lines() {
        let Some((k, v)) = l.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim());
        let key = if let Some(rest) = k.strip_prefix("ks_abarth_695_biposto_") {
            format!("ks_modded_car_{}", rest.replace("695_biposto", "modded_car"))
        } else if let Some(rest) = k.strip_prefix("abarth_") {
            format!("modded_car_{rest}")
        } else if k == "ultraleggera_biposto_rim" {
            // kept under the stock key, like the sample (the renamed rim
            // "Ultraleggera Rim" then labels itself)
            k.to_string()
        } else {
            continue;
        };
        let val = if key == "ks_modded_car_modded_car_base_preset" { "Standard".to_string() } else { v.to_string() };
        rows.push((key, val));
    }
    rows.sort_by_key(|(k, _)| k.to_lowercase());
    rows.dedup_by(|a, b| a.0.eq_ignore_ascii_case(&b.0));
    rows.iter().map(|(k, v)| format!("{k}={v}\r\n")).collect::<String>().into_bytes()
}

/// A neutral brand mark: a light ring on transparent.
fn brand_mark(size: u32) -> crate::io::texture::Image {
    let mut px = vec![0u8; (size * size * 4) as usize];
    let c = size as f32 / 2.0;
    let (r1, r0) = (size as f32 * 0.46, size as f32 * 0.36);
    for y in 0..size {
        for x in 0..size {
            let d = ((x as f32 + 0.5 - c).powi(2) + (y as f32 + 0.5 - c).powi(2)).sqrt();
            // 1 px anti-aliased edges
            let a = ((r1 - d).clamp(0.0, 1.0)) * ((d - r0).clamp(0.0, 1.0));
            let i = ((y * size + x) * 4) as usize;
            px[i..i + 4].copy_from_slice(&[230, 230, 230, (a * 255.0).round() as u8]);
        }
    }
    crate::io::texture::Image { w: size, h: size, px }
}

// --------------------------------------------------------------- refmats --

fn set_switches(m: &mut crate::io::pb::Msg, props: &[(&str, &[f32])]) {
    for (n, v) in props {
        crate::io::material::set_prop(m, n, v);
    }
}

/// Every texture slot emptied, every "has map" flag and extra layer off, no
/// parent material: what is left is the stock shader's plain base layer.
pub(crate) fn bare(stock: &[u8]) -> Result<crate::io::pb::Msg> {
    let mut m = crate::io::pb::Msg::parse(stock)?;
    m.remove(20);
    m.remove(22);
    let names: Vec<String> = m.subs(5).iter().filter_map(|s| s.str(1)).collect();
    for n in names {
        crate::io::material::set_slot(&mut m, &n, "");
    }
    let props: Vec<String> = m.subs(4).iter().filter_map(|s| s.str(1)).collect();
    for n in props {
        let layer = ["Red_", "Green_", "Blue_", "Scuff_", "Detail_", "Dirt_"].iter().any(|l| n.starts_with(l)) && n.ends_with("_Enable");
        if (n.contains("Has") && n.ends_with("Map")) || layer || n == "Splatmap_UseConstant" {
            crate::io::material::set_prop(&mut m, &n, &[0.0]);
        }
    }
    Ok(m)
}

const OFF: &[f32] = &[0.0];
const ON: &[f32] = &[1.0];
/// The switches that make a bared stock material one plain base layer
/// (used for `simple_refmat` and for the car stage's templates of other
/// shaders: VehicleRim, UberVehicleInteriorMaterial).
pub(crate) const SINGLE: &[(&str, &[f32])] = &[
    ("Red_Enable", OFF), ("Green_Enable", OFF), ("Blue_Enable", OFF), ("Scuff_Enable", OFF),
    ("Splatmap_UseConstant", OFF), ("Splatmap_Constant", &[0.0, 0.0, 0.0, 0.0]),
    ("Base_HasBaseColorMap", ON), ("Base_HasNormalMap", OFF), ("Base_HasRoughnessMap", OFF),
    ("Base_HasMetalnessMap", OFF), ("Base_HasAmbientOcclusionMap", OFF), ("Base_HasOpacityMap", OFF),
    ("Base_HasClearCoatMap", OFF), ("Base_HasClearCoatNormalMap", OFF), ("Base_HasClearCoatRoughnessMap", OFF),
    ("Base_HasAnisotropyMap", OFF), ("Base_HasHeightMap", OFF), ("Base_Triplanar", OFF),
    ("Base_Basecolor", &[1.0, 1.0, 1.0, 1.0]), ("Base_NormalScale", OFF), ("Base_ClearCoatNormalScale", ON),
    ("Modulation_BaseBaseColor", OFF),
];

/// A stock material bared to one plain base layer (`bare` + `SINGLE`).
pub(crate) fn plain(stock: &[u8]) -> Result<crate::io::pb::Msg> {
    let mut m = bare(stock)?;
    set_switches(&mut m, SINGLE);
    Ok(m)
}

/// The template materials the converter clones for new sections.
/// simple: one plain PBR layer, base colour map only (the converter sets
/// roughness, metalness, reflectance, clear coat, UV scale and the maps);
/// multimap: base = detail map, Red layer = the diffuse laid over it at
/// full strength (constant splat), its roughness / metalness replacing the
/// base's; led / emi: base colour map drives the emissive (LED atlas).
fn refmats(pkg: &Pkg, idx: &HashMap<String, usize>) -> Result<Vec<(&'static str, Vec<u8>)>> {
    let rd = |p: &str| -> Result<Vec<u8>> { read(pkg, idx, &format!("{STOCK}materials\\{p}")) };
    const OFF: &[f32] = &[0.0];
    const ON: &[f32] = &[1.0];
    let single = SINGLE;
    let mut simple = bare(&rd("ext_mech_blanking.material")?)?;
    set_switches(&mut simple, single);
    let mut multi = simple.clone();
    set_switches(&mut multi, &[
        ("Red_Enable", ON), ("Splatmap_UseConstant", ON), ("Splatmap_Constant", &[1.0, 0.0, 0.0, 0.0]),
        ("Red_HasBaseColorMap", ON), ("Red_HasNormalMap", OFF), ("Red_HasRoughnessMap", OFF),
        ("Red_HasMetalnessMap", OFF), ("Red_HasAmbientOcclusionMap", OFF), ("Red_HasOpacityMap", OFF),
        ("Red_HasClearCoatMap", OFF), ("Red_HasClearCoatNormalMap", OFF), ("Red_HasAnisotropyMap", OFF),
        ("Red_Basecolor", &[1.0, 1.0, 1.0]), ("Red_UVscale", &[1.0, 1.0]),
        ("RedOverrideMultiply_BaseColor", ON), ("RedOverrideReplace_BaseColor", OFF),
        ("RedOverrideReplace_Roughness", ON), ("RedOverrideReplace_Metalness", ON),
        ("RedOverrideReplace_Reflectance", ON), ("RedOverrideReplace_ClearCoat", OFF),
        ("RedOverrideReplace_AmbientOcclusion", OFF), ("RedOverrideMultiply_AmbientOcclusion", OFF),
    ]);
    let emissive: &[(&str, &[f32])] = &[
        ("Base_HasBaseColorMap", ON), ("Base_HasNormalMap", OFF), ("Base_NormalScale", OFF),
        ("Base_EmissiveIsBaseColorAndEmissiveMapMasks", ON), ("Base_EmissiveIntensity", ON),
        ("Base_Basecolor", &[0.0, 0.0, 0.0, 1.0]), ("Base_UVscale", &[1.0, 1.0]), ("Base_UVscaleNormal", &[1.0, 1.0]),
    ];
    let mut led = bare(&rd("int_decals.material")?)?;
    set_switches(&mut led, emissive);
    let mut emi = bare(&rd("int_elec_blanking.material")?)?;
    set_switches(&mut emi, emissive);
    Ok(vec![
        ("simple_refmat.material", simple.encode()),
        ("multimap_usedet_refmat.material", multi.encode()),
        ("led_refmat.material", led.encode()),
        ("emi_refmat.material", emi.encode()),
    ])
}

// ----------------------------------------------------------------- build --

fn index(pkg: &Pkg) -> HashMap<String, usize> {
    pkg.entries.iter().enumerate().map(|(i, e)| (e.path.to_lowercase(), i)).collect()
}

fn read(pkg: &Pkg, idx: &HashMap<String, usize>, path: &str) -> Result<Vec<u8>> {
    let i = idx.get(&path.to_lowercase()).ok_or_else(|| anyhow!("content.kspkg has no {path} (game update?)"))?;
    pkg.read(&pkg.entries[*i])
}

pub struct Built {
    /// (archive path with backslashes, bytes), donor layout order
    pub files: Vec<(String, Vec<u8>)>,
    pub refmats: Vec<(&'static str, Vec<u8>)>,
    pub elec: Vec<u8>,
}

/// Build the donor + refmats in memory from `content`.
pub fn build(content: &Path) -> Result<Built> {
    let pkg = Pkg::open(content)?;
    let idx = index(&pkg);
    let r = Renamer::new();
    let mut files = Vec::new();
    for line in LAYOUT.lines().filter(|l| !l.is_empty()) {
        let mut it = line.split('\t');
        let (rel, kind, src) = (it.next().unwrap(), it.next().unwrap_or(""), it.next().unwrap_or(""));
        let data = match kind {
            "copy" => read(&pkg, &idx, src)?,
            "ren" if rel.ends_with(".tyretextures") => {
                // the local road_1 textures stand in for the actor's default
                // road_2 set: brand-neutral names, road_2's texture id (f4, last)
                let mut t = parse(&transform(&r, rel, &read(&pkg, &idx, src)?), 0).ok_or_else(|| anyhow!("{rel}: not a protobuf"))?;
                let road2 = parse(&read(&pkg, &idx, "content\\cars\\common_assets\\parts\\tyres\\textures\\road_2\\tyre_road_2.tyretextures")?, 0)
                    .ok_or_else(|| anyhow!("tyre_road_2.tyretextures: not a protobuf"))?;
                set(&mut t, 2, s("Generic Tyre"));
                set(&mut t, 102, s("modded_brand"));
                drop_field(&mut t, 4);
                t.extend(road2.into_iter().filter(|(n, _)| *n == 4));
                encode(&t)
            }
            "ren" => transform(&r, rel, &read(&pkg, &idx, src)?),
            "gen" => match rel {
                "ks_modded_car.moddedcarcontent" => moddedcarcontent(),
                "uiresources/localization/en.loc" => b"Modded car general description.\r\n".to_vec(),
                "uiresources/localization/setup.en.loc" => b"Modded car setup description.".to_vec(),
                "uiresources/localization/en.cars.loc" => {
                    let l = read(&pkg, &idx, "uiresources\\localization\\en.cars.loc")?;
                    cars_loc(&String::from_utf8_lossy(&l))
                }
                x if x.starts_with("uiresources/branding/oem/modded_brand/") => {
                    let (name, size) = if x.contains("/badge.") { ("badge.png", 1024) } else { ("logo.png", 128) };
                    let e = crate::io::texture::encode(&brand_mark(size), crate::io::texture::Fmt::Bc7, true, name);
                    if x.ends_with(".texturemips") { e.mips } else { e.header }
                }
                _ => bail!("no generator for {rel}"),
            },
            k => bail!("donor layout: unknown kind {k} for {rel}"),
        };
        files.push((format!("{CAR}{}", rel.replace('/', "\\")), data));
    }
    let refmats = refmats(&pkg, &idx)?;
    let elec = read(&pkg, &idx, ELEC)?;
    Ok(Built { files, refmats, elec })
}

/// Write `<out>\ks_modded_car.zip`, `<out>\refmats\*.material` and
/// `<out>\templates\...` (where the converter looks for them).
pub fn write(b: &Built, out: &Path) -> Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(out.join("refmats"))?;
    let zp = out.join(DONOR_ZIP);
    let tmp = zp.with_extension("zip.part");
    {
        let f = std::fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
        let mut z = zip::ZipWriter::new(std::io::BufWriter::new(f));
        let opt = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).compression_level(Some(1));
        for (p, d) in &b.files {
            z.start_file(p.replace('\\', "/"), opt)?;
            z.write_all(d)?;
        }
        z.finish()?.flush()?;
    }
    std::fs::rename(&tmp, &zp)?;
    for (n, d) in &b.refmats {
        std::fs::write(out.join("refmats").join(n), d)?;
    }
    let e = out.join("templates").join("ks_audi_r8_lms_gt4_evo_mod_mech_1").join("data");
    std::fs::create_dir_all(&e)?;
    std::fs::write(e.join("ks_audi_r8_lms_gt4_evo_elec.carelectronics"), &b.elec)?;
    Ok(())
}

pub fn make(content: &Path, out: &Path) -> Result<()> {
    let b = build(content)?;
    write(&b, out)?;
    std::fs::write(out.join("stamp.txt"), stamp(content)?)?;
    Ok(())
}

// ------------------------------------------------------------ the cache --

/// Builder version + content.kspkg size and mtime: a game update (or a
/// builder change) remakes the donor.
pub fn stamp(content: &Path) -> Result<String> {
    let m = std::fs::metadata(content).with_context(|| format!("{}", content.display()))?;
    let t = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
    Ok(format!("v{BUILDER_VERSION} {} {t}\n", m.len()))
}

#[cfg(windows)]
pub(crate) fn free_bytes(p: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetDiskFreeSpaceExW(dir: *const u16, avail: *mut u64, total: *mut u64, free: *mut u64) -> i32;
    }
    let mut q = p;
    while !q.exists() {
        q = q.parent()?;
    }
    let w: Vec<u16> = q.as_os_str().encode_wide().chain(Some(0)).collect();
    let (mut a, mut t, mut f) = (0u64, 0u64, 0u64);
    (unsafe { GetDiskFreeSpaceExW(w.as_ptr(), &mut a, &mut t, &mut f) } != 0).then_some(a)
}

#[cfg(not(windows))]
pub(crate) fn free_bytes(_: &Path) -> Option<u64> {
    None
}

/// The generated donor's folder: ACECONV_DONOR_DIR, else
/// %LOCALAPPDATA%\aceconv\donor when that drive has room (~150 MB
/// written, 2 GB free required).
pub fn cache_dir() -> Result<PathBuf> {
    if let Some(d) = std::env::var_os("ACECONV_DONOR_DIR") {
        return Ok(PathBuf::from(d));
    }
    let d = PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or_else(|| anyhow!("LOCALAPPDATA is not set; set ACECONV_DONOR_DIR"))?)
        .join("aceconv").join("donor");
    if let Some(free) = free_bytes(&d) {
        if free < 2 << 30 {
            bail!("only {} MB free for the generated donor at {}; set ACECONV_DONOR_DIR to a folder on a roomier drive", free >> 20, d.display());
        }
    }
    Ok(d)
}

/// (donor zip, refmats dir), generating the donor from `content` on first
/// use and again whenever content.kspkg changes.
pub fn ensure(content: &Path) -> Result<(PathBuf, PathBuf)> {
    let dir = cache_dir()?;
    let want = stamp(content)?;
    let zp = dir.join(DONOR_ZIP);
    let fresh = std::fs::read_to_string(dir.join("stamp.txt")).map(|s| s == want).unwrap_or(false)
        && zp.is_file() && dir.join("refmats").join("simple_refmat.material").is_file();
    if !fresh {
        eprintln!("building the donor car from {} -> {}", content.display(), dir.display());
        // build beside the target, then swap in (another process may race us)
        let parent = dir.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)?;
        let tmp = parent.join(format!("donor.tmp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp)?;
        make(content, &tmp)?;
        let _ = std::fs::remove_dir_all(&dir);
        if std::fs::rename(&tmp, &dir).is_err() {
            let _ = std::fs::remove_dir_all(&tmp);
            if !std::fs::read_to_string(dir.join("stamp.txt")).map(|s| s == want).unwrap_or(false) {
                bail!("could not place the donor in {}", dir.display());
            }
        }
    }
    Ok((zp, dir.join("refmats")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_round_trips() {
        let t: T = vec![(1, s("content\\cars\\x")), (2, N::M(vec![(1, N::V(5)), (3, N::F32(1f32.to_le_bytes()))])), (7, N::B(vec![0, 1, 2]))];
        let b = encode(&t);
        assert_eq!(encode(&parse(&b, 0).unwrap()), b);
    }

    #[test]
    fn rename_changes_lengths() {
        let r = Renamer::new();
        let t: T = vec![(1, N::M(vec![(2, s("content\\cars\\ks_abarth_695_biposto\\data\\HEADER_POWER.curve"))]))];
        let mut p = parse(&encode(&t), 0).unwrap();
        map_strings(&mut p, &|b| r.ren(b, true));
        let out = encode(&p);
        assert!(memchr::memmem::find(&out, b"content\\cars\\ks_modded_car\\data\\header_power.curve").is_some());
        assert!(parse(&out, 0).is_some());
    }
}
