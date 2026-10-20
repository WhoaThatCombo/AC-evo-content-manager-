//! Pre-install validation of converted cars (a .kspkg) and tracks (a folder).
//!
//! Catches what makes the game crash or silently show nothing:
//! - a referenced `content\...` file that exists neither in the output nor in
//!   the game's content.kspkg (looked up case-insensitively, like the game);
//! - a mesh whose sections point past its indices, whose indices point past
//!   its vertices, with non-finite positions, or without a material;
//! - a `.texture` whose `.texturemips` is not the size its page table says;
//! - (tracks) a container holding entity kinds its Container entity does not
//!   declare - the game does not load those (a road that never draws);
//!   container files missing, or listed in the scene but not in the
//!   declaration rows (or the other way round);
//! - (tracks) ISMesh entities (3D vegetation, grass carpet): position /
//!   rotation / scale arrays of equal length, finite, positive scales, no
//!   more instances per node than the largest stock node, bounds (f13) that
//!   hold every instance (else the node culls away), a cull distance; the
//!   total against the largest stock track (`limits`).
//! - (tracks) lighting invariants of the static meshes: bounds that hold every
//!   vertex, finite unit normals, no back-to-back coincident triangle pairs
//!   (AC1 double-sided walls: lit/dark flicker), SMesh entity boxes that hold
//!   their mesh. `aceconv check-lighting <dir>...` runs only these.
//! - (cars) the rig, data-only (src/rigcheck.rs): needles on bones with
//!   geometry, car animation clips bound, driver pedal layers that move, a
//!   full-turn steer layer, no bodywork inside a tyre at rest, no
//!   wheel-carried bone hanging far off its wheel.
//! - (tracks) AI splines: every layout container's ideal line, pit lane and
//!   control points parse (`track::ai`) and have a sane point spacing;
//!   dynamic-track presets parse and share the `.track` frame.
//! - (tracks) per session (lines.rs): the `*track_limits_left*` /
//!   `*track_limits_right*` / `*ideal_line*` splines the game finds by name,
//!   the ideal line's aisplinedata at the layout path they give, spawn
//!   points upright and facing along the nearest line.

use anyhow::Result;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

pub mod batches;
pub mod limits;
pub mod rigcheck;
pub mod camcheck;
pub mod decl;
pub mod lines;

use crate::io::kspkg::Pkg;
use crate::io::pb::{Msg, Val};

#[derive(Default, Debug)]
pub struct Report {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub files: usize,
    pub refs: usize,
}

impl Report {
    fn err(&mut self, s: String) {
        if self.errors.len() < 200 {
            self.errors.push(s);
        }
    }
    fn warn(&mut self, s: String) {
        if self.warnings.len() < 200 {
            self.warnings.push(s);
        }
    }
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// The game's package index (lower-case paths), built once.
pub struct Game {
    paths: HashSet<String>,
    pkg: Pkg,
    idx: std::collections::HashMap<String, usize>,
}

impl Game {
    pub fn open(content: &Path) -> Result<Game> {
        let p = Pkg::open(content)?;
        let idx = p.entries.iter().enumerate().map(|(i, e)| (e.path.to_lowercase(), i)).collect();
        Ok(Game { paths: p.entries.iter().map(|e| e.path.to_lowercase()).collect(), pkg: p, idx })
    }
    /// A stock file's bytes (case-insensitive path).
    pub fn read(&self, p: &str) -> Option<Vec<u8>> {
        let i = *self.idx.get(&p.to_lowercase())?;
        self.pkg.read(&self.pkg.entries[i]).ok()
    }
    pub fn has(&self, p: &str) -> bool {
        self.paths.contains(&p.to_lowercase())
    }
    /// Every stock path (lower case) under a prefix.
    pub fn paths_with_prefix(&self, prefix: &str) -> Vec<String> {
        let pre = prefix.to_lowercase();
        self.paths.iter().filter(|p| p.starts_with(&pre)).cloned().collect()
    }
    /// The stock ids of a kind ("cars" | "tracks"): `content\<kind>\<id>\...`.
    pub fn ids(&self, kind: &str) -> HashSet<String> {
        let pre = format!("content\\{kind}\\");
        self.paths.iter().filter_map(|p| p.strip_prefix(&pre).and_then(|r| r.split_once('\\')).map(|(id, _)| id.to_string())).collect()
    }
}

const EXTS: [&str; 14] = [".texture", ".material", ".mesh", ".scene", ".track", ".animation", ".bank", ".curve", ".carparticleemitterdata_emitterspack", ".particleemitter", ".html", ".track_layout", ".driverfinalstate", ".preset"];

fn looks_like_path(b: &[u8]) -> Option<String> {
    if b.len() < 12 || b.len() > 400 || !b[..8].eq_ignore_ascii_case(b"content\\") {
        return None;
    }
    if !b.iter().all(|c| (32..127).contains(c)) {
        return None;
    }
    let s = String::from_utf8_lossy(b).into_owned();
    let low = s.to_lowercase();
    EXTS.iter().any(|e| low.ends_with(e)).then_some(s)
}

/// Every `content\...` path string in a protobuf file (recursive).
fn paths_in(b: &[u8], out: &mut Vec<String>, depth: usize) {
    let Ok(m) = Msg::parse(b) else { return };
    for f in &m.fields {
        let bytes = match &f.val {
            Val::Bytes(x) => x.as_slice(),
            _ => continue,
        };
        if let Some(p) = looks_like_path(bytes) {
            out.push(p);
        } else if depth < 12 && bytes.len() > 2 && bytes.len() < 8_000_000 {
            paths_in(bytes, out, depth + 1);
        }
    }
}

fn varints(b: &[u8]) -> Vec<u64> {
    let mut o = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match crate::io::pb::read_varint(b, &mut i) {
            Ok(v) => o.push(v),
            Err(_) => break,
        }
    }
    o
}

/// A .mesh: sections, indices and positions agree.
fn check_mesh(name: &str, b: &[u8], r: &mut Report) {
    let Ok(top) = Msg::parse(b) else { return r.err(format!("{name}: not a protobuf mesh")) };
    let mut bodies = top.subs(5);
    if bodies.is_empty() {
        return r.err(format!("{name}: no LOD body (f5)"));
    }
    for (li, body) in bodies.drain(..).enumerate() {
        let nv = body.bytes(5).map(|p| p.len() / 12).unwrap_or(0);
        if nv == 0 {
            r.err(format!("{name}: LOD{li} has no vertices"));
            continue;
        }
        if let Some(p) = body.bytes(5) {
            if p.chunks_exact(4).any(|c| !f32::from_le_bytes(c.try_into().unwrap()).is_finite()) {
                r.err(format!("{name}: LOD{li} has non-finite positions"));
            }
        }
        let idx = body.bytes(11).map(varints).unwrap_or_default();
        if let Some(m) = idx.iter().max() {
            if *m as usize >= nv {
                r.err(format!("{name}: LOD{li} index {m} >= {nv} vertices"));
            }
        }
        let secs = body.subs(4);
        if secs.is_empty() {
            r.err(format!("{name}: LOD{li} has no sections"));
        }
        for s in secs {
            let (start, count) = (s.varint(2).unwrap_or(0) as usize, s.varint(3).unwrap_or(0) as usize);
            if start + count > idx.len() {
                r.err(format!("{name}: section {:?} spans {}..{} of {} indices", s.str(1).unwrap_or_default(), start, start + count, idx.len()));
            }
            if s.str(4).map(|m| m.is_empty()).unwrap_or(true) {
                r.err(format!("{name}: section {:?} has no material", s.str(1).unwrap_or_default()));
            }
        }
    }
}

/// Lighting invariants of a track .mesh (what makes surfaces flicker or go
/// black in the game, none of which factory meshes break):
/// - top f6/f7 and LOD f17/f18 bounds hold every vertex (a box that misses its
///   geometry drops the mesh from culling / shadow-cascade sets in turns);
/// - normals are finite and unit length (a zero normal is NaN after the
///   shader's normalize);
/// - no back-to-back coincident triangle pairs (same material, opposite
///   winding on a 5 mm grid, real face normals opposite): each copy shadows
///   the other at zero depth distance, and the lit face flips between lit
///   and dark as the shadows update.
/// Returns the back-to-back pair count.
fn check_mesh_lighting(name: &str, b: &[u8], r: &mut Report) -> usize {
    let Ok(top) = Msg::parse(b) else { return 0 };
    let v3 = |m: Option<Msg>| m.map(|m| [m.f32(1).unwrap_or(0.0), m.f32(2).unwrap_or(0.0), m.f32(3).unwrap_or(0.0)]);
    let (lo, hi) = (v3(top.sub(6)), v3(top.sub(7)));
    let short = name.rsplit('\\').next().unwrap_or(name);
    let mut pairs = 0;
    for (li, body) in top.subs(5).iter().enumerate() {
        let Some(pb) = body.bytes(5) else { continue };
        let pos: Vec<f32> = pb.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect();
        let nv = pos.len() / 3;
        if nv == 0 {
            continue;
        }
        let (mut vlo, mut vhi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for p in pos.chunks_exact(3) {
            for k in 0..3 {
                vlo[k] = vlo[k].min(p[k]);
                vhi[k] = vhi[k].max(p[k]);
            }
        }
        let tol = |x: f32| 0.01 + x.abs() * 1e-5;
        let holds = |a: Option<[f32; 3]>, b: Option<[f32; 3]>| match (a, b) {
            (Some(a), Some(b)) => (0..3).all(|k| vlo[k] >= a[k] - tol(a[k]) && vhi[k] <= b[k] + tol(b[k])),
            _ => true,
        };
        if lo.is_none() || hi.is_none() {
            r.err(format!("{short}: no bounds (f6/f7)"));
        } else if !holds(lo, hi) {
            r.err(format!("{short}: LOD{li} vertices [{:.2?}..{:.2?}] outside the mesh bounds [{:.2?}..{:.2?}] - culls / shadows pop", vlo, vhi, lo.unwrap(), hi.unwrap()));
        }
        if !holds(v3(body.sub(17)), v3(body.sub(18))) {
            r.err(format!("{short}: LOD{li} vertices outside the LOD bounds (f17/f18)"));
        }
        if let Some(nb) = body.bytes(6) {
            let n: Vec<f32> = nb.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect();
            let (mut bad, mut zero) = (0, 0);
            for v in n.chunks_exact(3) {
                let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                if !l.is_finite() {
                    bad += 1;
                } else if l < 0.5 {
                    zero += 1;
                }
            }
            if bad > 0 {
                r.err(format!("{short}: LOD{li} {bad} non-finite normal(s)"));
            }
            if zero > 0 {
                r.warn(format!("{short}: LOD{li} {zero} zero-length normal(s) (NaN shading after normalize)"));
            }
        }
        // the colour stream (f13): 4 values per vertex, finite, 0 .. 1/255
        // (stock: the vertex AO is component 3 as ao / 255)
        if let Some(cb) = body.bytes(13) {
            if cb.len() != nv * 16 {
                r.err(format!("{short}: LOD{li} colour stream holds {} values for {nv} vertices (want 4 each)", cb.len() / 4));
            }
            let bad = cb.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).filter(|v| !v.is_finite() || *v < 0.0 || *v > 1.01 / 255.0).count();
            if bad > 0 {
                r.err(format!("{short}: LOD{li} {bad} colour-stream value(s) outside 0 .. 1/255 (vertex AO would read past 1)"));
            }
        }
        // back-to-back pairs per section material
        let idx = body.bytes(11).map(varints).unwrap_or_default();
        let q = |i: u64| -> (i64, i64, i64) { let i = i as usize * 3; ((pos[i] * 200.0).round() as i64, (pos[i + 1] * 200.0).round() as i64, (pos[i + 2] * 200.0).round() as i64) };
        // a pair also needs real face normals pointing opposite ways: on the
        // 5 mm grid a sliver or a millimetre triangle can flip its winding
        // while both faces point the same way (no sheet, no flicker)
        let fnorm = |t: &[u64]| -> Option<[f32; 3]> {
            let p = |i: u64| { let i = i as usize * 3; [pos[i], pos[i + 1], pos[i + 2]] };
            let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
            let (u, v) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
            let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
            let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            (l.is_finite() && l > 1e-20).then(|| [n[0] / l, n[1] / l, n[2] / l])
        };
        let mut seen: std::collections::HashMap<(String, [(i64, i64, i64); 3]), (bool, Option<[f32; 3]>)> = std::collections::HashMap::new();
        for s in body.subs(4) {
            let mat = s.str(4).unwrap_or_default();
            let (start, count) = (s.varint(2).unwrap_or(0) as usize, s.varint(3).unwrap_or(0) as usize);
            let Some(tris) = idx.get(start..start + count) else { continue };
            for t in tris.chunks_exact(3) {
                if t.iter().any(|&i| i as usize >= nv) {
                    continue;
                }
                let (a, b, c) = (q(t[0]), q(t[1]), q(t[2]));
                if a == b || b == c || a == c {
                    continue;
                }
                let fwd = [[a, b, c], [b, c, a], [c, a, b]].into_iter().min().unwrap();
                let rev = [[a, c, b], [c, b, a], [b, a, c]].into_iter().min().unwrap();
                let (k, w) = if fwd <= rev { (fwd, true) } else { (rev, false) };
                let n = fnorm(t);
                match seen.get(&(mat.clone(), k)) {
                    Some(&(w0, n0)) if w0 != w && matches!((n0, n), (Some(x), Some(y)) if x[0] * y[0] + x[1] * y[1] + x[2] * y[2] < -0.5) => {
                        pairs += 1;
                        seen.remove(&(mat.clone(), k));
                    }
                    Some(_) => {}
                    None => {
                        seen.insert((mat.clone(), k), (w, n));
                    }
                }
            }
        }
    }
    pairs
}

/// Only the lighting checks of a track folder, one mesh in memory at a time
/// (`aceconv check-lighting <track dir>`, for surveying installed tracks).
pub fn track_lighting(dir: &Path) -> Result<Report> {
    let mut r = Report::default();
    let mut b2b: Vec<(usize, String)> = Vec::new();
    let sm = dir.join("static_mesh");
    let mut names: Vec<_> = std::fs::read_dir(&sm).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    names.sort();
    for p in names.iter().filter(|p| p.extension().map(|e| e.eq_ignore_ascii_case("mesh")).unwrap_or(false)) {
        let b = std::fs::read(p)?;
        r.files += 1;
        let n = check_mesh_lighting(&p.file_name().unwrap().to_string_lossy(), &b, &mut r);
        if n > 0 {
            b2b.push((n, p.file_name().unwrap().to_string_lossy().into_owned()));
        }
    }
    if !b2b.is_empty() {
        b2b.sort_by(|a, b| b.0.cmp(&a.0));
        let total: usize = b2b.iter().map(|x| x.0).sum();
        r.warn(format!("{total} back-to-back coincident triangle pair(s) in {} mesh(es) (lit/dark flicker as shadows update): {}", b2b.len(),
            b2b.iter().take(6).map(|(n, f)| format!("{f} {n}")).collect::<Vec<_>>().join(", ")));
    }
    Ok(r)
}

/// SMesh entity f13 xz box against the mesh bounds (plain translation only).
fn check_smesh_boxes(files: &BTreeMap<String, Vec<u8>>, r: &mut Report) {
    let by_low: std::collections::HashMap<String, &Vec<u8>> = files.iter().map(|(k, v)| (k.to_lowercase(), v)).collect();
    for (k, b) in files.iter().filter(|(k, _)| k.to_lowercase().ends_with(".scene")) {
        for e in entities(b) {
            if entity_kind(&e).as_deref() != Some("SMesh") {
                continue;
            }
            let Some(path) = e.sub(50).and_then(|d| d.sub(100)).and_then(|d| d.str(1)) else { continue };
            let Some(mesh) = by_low.get(&path.to_lowercase()).and_then(|m| Msg::parse(m).ok()) else { continue };
            let tr = e.sub(4);
            let rotated = tr.as_ref().and_then(|t| t.sub(2)).map(|q| !q.fields.is_empty()).unwrap_or(false);
            let scaled = tr.as_ref().and_then(|t| t.sub(3)).map(|s| [1, 2, 3].iter().any(|&n| (s.f32(n).unwrap_or(1.0) - 1.0).abs() > 1e-6)).unwrap_or(false);
            if rotated || scaled {
                continue;
            }
            let t = tr.as_ref().and_then(|t| t.sub(1)).map(|p| [p.f32(1).unwrap_or(0.0), p.f32(3).unwrap_or(0.0)]).unwrap_or([0.0; 2]);
            let (Some(lo), Some(hi)) = (mesh.sub(6), mesh.sub(7)) else { continue };
            let Some(b13) = e.sub(13) else { continue };
            let (Some(emin), Some(emax)) = (b13.sub(1), b13.sub(2)) else { continue };
            let m = [lo.f32(1).unwrap_or(0.0) + t[0], lo.f32(3).unwrap_or(0.0) + t[1], hi.f32(1).unwrap_or(0.0) + t[0], hi.f32(3).unwrap_or(0.0) + t[1]];
            let bx = [emin.f32(1).unwrap_or(0.0), emin.f32(2).unwrap_or(0.0), emax.f32(1).unwrap_or(0.0), emax.f32(2).unwrap_or(0.0)];
            if m[0] < bx[0] - 0.01 || m[1] < bx[1] - 0.01 || m[2] > bx[2] + 0.01 || m[3] > bx[3] + 0.01 {
                r.err(format!("{}: SMesh {:?} box {:.1?} misses its mesh {:.1?} - culls / shadows pop", k.rsplit('\\').next().unwrap_or(""), e.str(1).unwrap_or_default(), bx, m));
            }
        }
    }
}

/// A .texture header against its .texturemips size.
fn check_texture(name: &str, header: &[u8], mips: Option<usize>, r: &mut Report) {
    let Ok(h) = Msg::parse(header) else { return r.err(format!("{name}: bad texture header")) };
    let (w, hh) = (h.varint(1).unwrap_or(0), h.varint(2).unwrap_or(0));
    if w == 0 || hh == 0 {
        r.err(format!("{name}: texture without size"));
    }
    let pages: u64 = h.sub(12).and_then(|pt| pt.bytes(5).map(varints)).map(|v| v.iter().sum()).unwrap_or(0);
    match mips {
        None => r.err(format!("{name}: no .texturemips")),
        Some(n) if pages > 0 && n as u64 != pages * 65536 => r.err(format!("{name}: .texturemips is {n} B, page table says {} B", pages * 65536)),
        // unpaged R11G11B10F volume (irradiance probes: 64 x 3 x 16, one mip)
        Some(n) if pages == 0 && h.varint(4) == Some(30) => {
            let (d, levels) = (h.varint(6).unwrap_or(1).max(1), h.varint(8).unwrap_or(1).max(1));
            let want: u64 = (0..levels).map(|l| (w >> l).max(1) * (hh >> l).max(1) * (d >> l).max(1) * 4).sum();
            if n as u64 != want {
                r.err(format!("{name}: .texturemips is {n} B, a {w}x{hh}x{d} R11G11B10 texture with {levels} mip(s) needs {want} B"));
            }
        }
        _ => {}
    }
}

/// Irradiance Probe entities: 256 ground heights, both probe textures present.
fn check_probes(scene: &[u8], files: &BTreeMap<String, Vec<u8>>, r: &mut Report) {
    let own: HashSet<String> = files.keys().map(|k| k.to_lowercase()).collect();
    let (mut probes, mut with_macro) = (0usize, 0usize);
    for e in entities(scene) {
        if entity_kind(&e).as_deref() != Some("Irradiance Probe") {
            continue;
        }
        let name = e.str(1).unwrap_or_else(|| "?".into());
        let Some(p) = e.sub(50).and_then(|x| x.sub(120)) else {
            r.err(format!("probe {name}: no f50.f120"));
            continue;
        };
        if p.bytes(6).map(|b| b.len()) != Some(1024) {
            r.err(format!("probe {name}: f6 is not 256 ground heights"));
        }
        for f in [1, 12] {
            match p.str(f) {
                Some(t) if own.contains(&t.to_lowercase()) && own.contains(&format!("{}mips", t.to_lowercase())) => {
                    // texels: R11G11B10 floats, never Inf / NaN (exponent 31)
                    let key = files.keys().find(|k| k.eq_ignore_ascii_case(&format!("{t}mips")));
                    if let Some(m) = key.and_then(|k| files.get(k)) {
                        let bad = m.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap()))
                            .filter(|u| (u >> 6) & 31 == 31 || (u >> 17) & 31 == 31 || (u >> 27) & 31 == 31).count();
                        if bad > 0 {
                            r.err(format!("probe {name}: {bad} texel(s) of {t} are Inf / NaN"));
                        }
                    }
                }
                Some(t) => r.err(format!("probe {name}: {t} (or its .texturemips) is missing")),
                None => r.err(format!("probe {name}: no texture in f{f}")),
            }
        }
        probes += 1;
        if p.bytes(11).is_some() || p.get(11).is_some() {
            with_macro += 1;
        }
    }
    // stock maximum (nurburgring 900)
    if probes > 900 {
        r.err(format!("{probes} irradiance volumes: more than the stock maximum of 900"));
    }
    if probes > 0 && with_macro < probes {
        r.warn(format!("{} of {probes} irradiance volumes name no macro volume (f11)", probes - with_macro));
    }
}

/// Resolve references; `own` = (lower-case path) -> exists in the output.
fn check_refs(files: &BTreeMap<String, Vec<u8>>, own_prefix: &str, game: &Game, r: &mut Report) {
    let own: HashSet<String> = files.keys().map(|k| k.to_lowercase()).collect();
    let mut seen: HashSet<String> = HashSet::new();
    for (name, b) in files {
        let low = name.to_lowercase();
        if low.ends_with(".texturemips") || low.ends_with(".bank") || low.ends_with(".png") || low.ends_with(".jpg") || low.ends_with(".html") || low.ends_with(".json") || low.ends_with(".ttf") || low.ends_with(".otf") || low.ends_with(".dynamictrackpresetcompressed") {
            continue;
        }
        let mut ps = Vec::new();
        paths_in(b, &mut ps, 0);
        for p in ps {
            let pl = p.to_lowercase();
            r.refs += 1;
            if !seen.insert(format!("{low}|{pl}")) {
                continue;
            }
            if pl.starts_with(own_prefix) {
                if !own.contains(&pl) && !game.has(&pl) {
                    r.err(format!("missing {p} (referenced by {name})"));
                }
            } else if !game.has(&pl) && !own.contains(&pl) {
                r.err(format!("missing game file {p} (referenced by {name})"));
            }
        }
    }
    for (name, b) in files {
        let low = name.to_lowercase();
        if low.ends_with(".mesh") {
            check_mesh(name, b, r);
        } else if low.ends_with(".texture") {
            let mips = files.get(&format!("{name}mips")).map(|m| m.len()).or_else(|| files.iter().find(|(k, _)| k.to_lowercase() == format!("{low}mips")).map(|(_, v)| v.len()));
            check_texture(name, b, mips, r);
        }
    }
}

/// The actor's audio slots that name the car's own bank: the bank and its strings bank are in the
/// package, parse, and hold the slot's event.
fn check_audio(files: &BTreeMap<String, Vec<u8>>, id: &str, r: &mut Report) {
    use crate::sound::bankread as br;
    let get = |p: &str| files.iter().find(|(k, _)| k.eq_ignore_ascii_case(p)).map(|(_, v)| v);
    let Some(actor) = get(&format!("content\\cars\\{id}\\{id}.actor")) else { return };
    let Ok(a) = Msg::parse(actor) else { return };
    let Some(car) = a.sub(103) else { return };
    let own = format!("content\\cars\\{id}\\").to_lowercase();
    type Loaded = Option<(Vec<(String, crate::sound::bankwrite::prj::Guid)>, br::Bank)>;
    let mut cache: BTreeMap<String, Loaded> = BTreeMap::new();
    for s in car.subs(9) {
        let (Some(ev), Some(bank)) = (s.str(1), s.str(6)) else { continue };
        if !bank.to_lowercase().starts_with(&own) {
            continue;
        }
        let entry = cache.entry(bank.to_lowercase()).or_insert_with(|| {
            let sb = format!("{}.strings.bank", bank.strip_suffix(".bank")?);
            let names = br::strings(get(&sb)?).ok()?;
            let bk = br::read_meta(get(&bank)?).ok()?;
            Some((names, bk))
        });
        match entry {
            None => r.err(format!("audio slot {ev}: {bank} or its .strings.bank is missing or does not parse")),
            Some((names, bk)) => match names.iter().find(|(p, _)| p.eq_ignore_ascii_case(&ev)) {
                None => r.err(format!("audio slot {ev}: not in the strings bank of {bank}")),
                Some((_, g)) if !bk.events.contains_key(g) => r.err(format!("audio slot {ev}: not in {bank}")),
                _ => {}
            },
        }
    }
}

/// A converted car package.
pub fn car(kspkg: &Path, game: &Game) -> Result<Report> {
    let mut r = Report::default();
    let files: BTreeMap<String, Vec<u8>> = crate::io::kspkg::read(kspkg, "")?.into_iter().collect();
    r.files = files.len();
    let id = files.keys().find_map(|k| k.strip_prefix("content\\cars\\").and_then(|x| x.split('\\').next()).map(String::from)).unwrap_or_default();
    if id.is_empty() {
        r.err("no content\\cars\\<id>\\ in the package".into());
        return Ok(r);
    }
    if !files.keys().any(|k| k.eq_ignore_ascii_case(&format!("content\\cars\\{id}\\{id}.actor"))) {
        r.err(format!("no {id}.actor"));
    }
    check_refs(&files, &format!("content\\cars\\{id}\\").to_lowercase(), game, &mut r);
    check_audio(&files, &id, &mut r);
    check_brand(&files, &id, game, &mut r);
    // batch names / shaders the car renderer looks up (validate::batches)
    {
        let low: std::collections::HashMap<String, &Vec<u8>> = files.iter().map(|(k, v)| (k.to_lowercase(), v)).collect();
        let get = |p: &str| low.get(&p.to_lowercase()).map(|v| (*v).clone()).or_else(|| game.read(p));
        let own: Vec<String> = files.keys().cloned().collect();
        batches::check(&id, &own, &get, &mut r);
    }
    // rig: needles, car / driver clips, tyres at rest, wheel-carried bones
    let rig = crate::validate::rigcheck::check(&files, &id);
    for w in rig.warnings {
        r.warn(w);
    }
    for e in rig.errors {
        r.err(e);
    }
    let cams = crate::validate::camcheck::check(&files, &id);
    for w in cams.warnings {
        r.warn(w);
    }
    // the game re-applies saved per-car cameras over the actor's (car::usersettings)
    if let (Some(dir), Some(actor)) = (crate::car::usersettings::saved_dir(), files.iter().find(|(k, _)| k.eq_ignore_ascii_case(&format!("content\\cars\\{id}\\{id}.actor"))).map(|(_, v)| v)) {
        if let Ok(b) = std::fs::read(dir.join(crate::car::usersettings::CAMERAS)) {
            let stale = crate::car::usersettings::stale_cameras(&b, &id, actor);
            if !stale.is_empty() {
                r.warn(format!("cameras: {} (your Saved Games, not this package) holds older cameras for {id}, which the game uses instead of the actor's: {} - with the game closed, `aceconv reset-car-settings {id}`", crate::car::usersettings::CAMERAS, stale.join("; ")));
            }
        }
    }
    for e in cams.errors {
        r.err(e);
    }
    let (pe, pw) = crate::car::physics::stats::validate(&files, None);
    for w in pw {
        r.warn(w);
    }
    for e in pe {
        r.err(e);
    }
    // drivetrain: enum vs driven wheels / diffs, and vs the AC1 source when
    // the sidecar `<id>.json` records it
    let ac1 = std::fs::read(kspkg.with_extension("json")).ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| v["cars"][0]["aceconv_ac1_traction"].as_str().map(String::from));
    if let Some(cd) = crate::car::physics::stats::cardata_of(files.keys().map(|s| s.as_str())).and_then(|p| Msg::parse(&files[&p]).ok()) {
        let (de, dw) = crate::car::physics::dtcheck::check(&cd, ac1.as_deref());
        for w in dw {
            r.warn(w);
        }
        for e in de {
            r.err(e);
        }
    }
    Ok(r)
}

/// The car's brand (car::brand): the `.moddedcarcontent` brand string is a
/// stock brand (its branding folder is in the game; written as the stock
/// name, or car select shows a second tile), or a complete car-local brand:
/// the UI's id and loc key of the name agree, `badge` and `logo` textures
/// (+ mips) at the package ROOT `uiresources\branding\oem\<id>\` (the path
/// the UI fetches; car-local copies are never served) are valid, and the
/// car's `en.cars.loc` names `brand_<id>`. The presets carry the same
/// string. The per-car descriptions the UI requests,
/// `uiresources\localization\cars\<car>\en.loc` and `<mech preset>.en.loc`,
/// are present.
fn check_brand(files: &BTreeMap<String, Vec<u8>>, id: &str, game: &Game, r: &mut Report) {
    use crate::car::brand::{loc_rows, ui_id, ui_loc_key, Brands};
    let low: std::collections::HashMap<String, &Vec<u8>> = files.iter().map(|(k, v)| (k.to_lowercase(), v)).collect();
    let base = format!("content\\cars\\{id}\\").to_lowercase();
    let Some(mc) = low.get(&format!("{base}{id}.moddedcarcontent")) else {
        return r.err(format!("brand: no {id}.moddedcarcontent"));
    };
    let Some(name) = Msg::parse(mc).ok().and_then(|m| m.str(10)).filter(|s| !s.is_empty()) else {
        return r.err("brand: the .moddedcarcontent has no brand (f10)".into());
    };
    let key = ui_id(&name);
    let local = format!("{base}uiresources\\branding\\");
    if let Some(p) = low.keys().find(|p| p.starts_with(&local)) {
        r.err(format!("brand: {p} is car-local branding, which the UI never serves (it fetches the root uiresources\\branding\\oem\\<key>\\)"));
    }
    let locdir = format!("uiresources\\localization\\cars\\{id}\\").to_lowercase();
    match low.get(&format!("{locdir}en.loc")) {
        None => r.err(format!("brand: no {locdir}en.loc (the car description the UI requests)")),
        Some(b) if std::str::from_utf8(b).map(|t| t.trim().is_empty()).unwrap_or(true) => r.err(format!("brand: {locdir}en.loc is empty or not UTF-8")),
        _ => {}
    }
    for p in low.keys() {
        if !(p.starts_with(&base) && p.ends_with(".mechanicalcarpreset")) {
            continue;
        }
        let stem = p.rsplit('\\').next().unwrap_or(p).trim_end_matches(".mechanicalcarpreset");
        if !low.contains_key(&format!("{locdir}{stem}.en.loc")) {
            r.err(format!("brand: no {locdir}{stem}.en.loc (the preset description the UI requests)"));
        }
    }
    for (p, b) in files {
        let pl = p.to_lowercase();
        let num = if pl.ends_with(".mechanicalcarpreset") || pl.ends_with(".visualcarpreset") { 1 } else { continue };
        if let Some(s) = Msg::parse(b).ok().and_then(|m| m.str(num)) {
            if s != name {
                r.err(format!("brand: {p} says \"{s}\", the car \"{name}\""));
            }
        }
    }
    if game.has(&format!("uiresources\\branding\\oem\\{key}\\badge.texture")) {
        let stock = Brands::from_game(game);
        if let Some(b) = stock.list.iter().find(|b| b.key == key) {
            if b.name != name {
                r.warn(format!("brand: \"{name}\" is the stock brand {key}, whose cars say \"{}\" (car select shows two tiles)", b.name));
            }
        }
        return;
    }
    if format!("brand_{key}") != ui_loc_key(&name) {
        r.err(format!("brand: \"{name}\" gives the UI id {key} but the loc key {} (use the key as the name)", ui_loc_key(&name)));
    }
    let dir = format!("uiresources\\branding\\oem\\{key}\\");
    for t in ["badge", "logo"] {
        let p = format!("{dir}{t}.texture");
        match low.get(&p) {
            None => r.err(format!("brand: brand {key} has no {p} in the package")),
            Some(h) => {
                let mips = low.get(&format!("{p}mips")).map(|m| m.len());
                let n = r.errors.len();
                check_texture(&p, h, mips, r);
                if r.errors.len() == n && mips.unwrap_or(0) == 0 {
                    r.err(format!("brand: {p}: empty mips"));
                }
            }
        }
    }
    let loc = low.get(&format!("{base}uiresources\\localization\\en.cars.loc")).map(|b| loc_rows(b)).unwrap_or_default();
    match loc.get(&format!("brand_{key}")) {
        Some(v) if !v.is_empty() => {}
        _ => r.err(format!("brand: brand {key} has no brand_{key} in the car's en.cars.loc")),
    }
}

/// The batch checks (`batches`) on a stock car straight out of content.kspkg:
/// the calibration of those rules.
pub fn stock_car(id: &str, game: &Game) -> Report {
    let mut r = Report::default();
    let own = game.paths_with_prefix(&format!("content\\cars\\{id}\\"));
    r.files = own.len();
    batches::check(id, &own, &|p: &str| game.read(p), &mut r);
    if let Some(cd) = crate::car::physics::stats::cardata_of(own.iter().map(|s| s.as_str())).and_then(|p| game.read(&p)).and_then(|b| Msg::parse(&b).ok()) {
        let (de, dw) = crate::car::physics::dtcheck::check(&cd, None);
        for w in dw {
            r.warn(w);
        }
        for e in de {
            r.err(e);
        }
    }
    r
}

fn entity_kind(e: &Msg) -> Option<String> {
    e.fields.iter().find(|f| f.num == 3).and_then(|f| match &f.val { Val::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()), _ => None })
}

fn entities(scene: &[u8]) -> Vec<Msg> {
    Msg::parse(scene).map(|m| m.subs(2)).unwrap_or_default()
}

/// Kinds the game must load from a container: undeclared ones are errors.
const CRITICAL: [&str; 7] = ["SMesh", "ISMesh", "Start Pos", "Zone", "Light", "InstancedLight", "Spline"];

use limits::{ISMESH_NODE_MAX, ISMESH_TRACK_MAX};

/// One ISMesh entity: arrays, values, bounds. Returns its instance count.
fn check_ismesh(e: &Msg, file: &str, r: &mut Report) -> usize {
    let name = e.str(1).unwrap_or_default();
    let w = format!("{file}: ISMesh {name}");
    let Some(d) = e.sub(50).and_then(|x| x.sub(105)) else {
        r.err(format!("{w} has no instance data (f50.f105)"));
        return 0;
    };
    if d.str(1).map(|p| p.is_empty()).unwrap_or(true) {
        r.err(format!("{w} has no mesh"));
    }
    let Some(t) = d.sub(7) else {
        r.err(format!("{w} has no transforms (f7)"));
        return 0;
    };
    let arr = |k: u32| -> Vec<f32> { t.bytes(k).map(|b| b.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect()).unwrap_or_default() };
    let (pos, rot, scl) = (arr(2), arr(3), arr(4));
    if pos.is_empty() || pos.len() % 3 != 0 || rot.len() != pos.len() || scl.len() != pos.len() {
        r.err(format!("{w}: transform arrays of {} / {} / {} floats", pos.len(), rot.len(), scl.len()));
        return 0;
    }
    let n = pos.len() / 3;
    if n > ISMESH_NODE_MAX {
        r.err(format!("{w}: {n} instances in one node (the largest stock node holds {ISMESH_NODE_MAX})"));
    }
    if pos.iter().chain(&rot).any(|v| !v.is_finite()) || pos.iter().any(|v| v.abs() > 1e6) {
        r.err(format!("{w}: non-finite or far-out positions / rotations"));
    }
    if scl.iter().any(|v| !v.is_finite() || *v <= 0.0 || *v > 1000.0) {
        r.err(format!("{w}: scales must be finite and in (0, 1000]"));
    }
    match e.sub(13) {
        Some(b) => {
            let lo = b.sub(1).map(|m| [m.f32(1).unwrap_or(0.0), m.f32(2).unwrap_or(0.0)]);
            let hi = b.sub(2).map(|m| [m.f32(1).unwrap_or(0.0), m.f32(2).unwrap_or(0.0)]);
            if !(b.f32(3).unwrap_or(0.0) > 0.0) {
                r.err(format!("{w}: no cull distance"));
            }
            if let (Some(lo), Some(hi)) = (lo, hi) {
                let out = pos.chunks_exact(3).filter(|p| p[0] < lo[0] - 0.01 || p[0] > hi[0] + 0.01 || p[2] < lo[1] - 0.01 || p[2] > hi[1] + 0.01).count();
                if out > 0 {
                    r.err(format!("{w}: {out} instances outside the node bounds"));
                }
            } else {
                r.err(format!("{w}: bounds without min / max"));
            }
        }
        None => r.err(format!("{w}: no bounds (f13)")),
    }
    n
}

/// A converted track folder (as it will be installed).
pub fn track(dir: &Path, id: &str, game: &Game) -> Result<Report> {
    let mut r = Report::default();
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    fn walk(base: &Path, d: &Path, prefix: &str, out: &mut BTreeMap<String, Vec<u8>>) -> std::io::Result<()> {
        for e in std::fs::read_dir(d)?.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(base, &p, prefix, out)?;
            } else {
                let rel = p.strip_prefix(base).unwrap().to_string_lossy().replace('/', "\\");
                out.insert(format!("{prefix}{rel}"), std::fs::read(&p)?);
            }
        }
        Ok(())
    }
    let prefix = format!("content\\tracks\\{id}\\");
    walk(dir, dir, &prefix, &mut files)?;
    r.files = files.len();
    let scene_path = format!("{prefix}{id}.scene");
    let Some(scene) = files.get(&scene_path).cloned() else {
        r.err(format!("no {id}.scene"));
        return Ok(r);
    };
    if !files.contains_key(&format!("{prefix}{id}.track")) {
        r.err(format!("no {id}.track"));
    }
    check_refs(&files, &prefix.to_lowercase(), game, &mut r);
    check_probes(&scene, &files, &mut r);
    // containers: declared types vs what each holds
    let mut declared: Vec<String> = Vec::new();
    // every Container entity of the main scene, own or shared: path -> kinds
    let mut entity_kinds: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for e in entities(&scene) {
        if entity_kind(&e).as_deref() != Some("Container") {
            continue;
        }
        let types: Vec<String> = e.fields.iter().filter(|f| f.num == 3).skip(1).filter_map(|f| match &f.val { Val::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()), _ => None }).collect();
        let Some(path) = e.sub(50).and_then(|p| p.sub(121)).and_then(|c| c.str(1)) else { continue };
        let pl = path.to_lowercase();
        entity_kinds.insert(pl.clone(), types.clone());
        if !pl.starts_with(&prefix.to_lowercase()) {
            continue; // common assets (camera sequence)
        }
        declared.push(pl.clone());
        let Some((_, body)) = files.iter().find(|(k, _)| k.to_lowercase() == pl) else {
            r.err(format!("container {path} is in the scene but not in the folder"));
            continue;
        };
        let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
        for x in entities(body) {
            if let Some(k) = entity_kind(&x) {
                *kinds.entry(k).or_default() += 1;
            }
        }
        for (k, n) in kinds {
            if types.iter().any(|t| *t == k) {
                continue;
            }
            let msg = format!("{} holds {n} {k} entit{} but declares {:?} - the game won't load them", path.rsplit('\\').next().unwrap_or(""), if n == 1 { "y" } else { "ies" }, types);
            if CRITICAL.contains(&k.as_str()) { r.err(msg) } else { r.warn(msg) }
        }
    }
    // declaration rows vs scene containers
    if let Some(d) = files.get(&format!("{prefix}ui\\track_decl.json")) {
        let v: serde_json::Value = serde_json::from_slice(d).unwrap_or_default();
        let rows: Vec<Vec<u8>> = v["container_rows"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).filter_map(|h| (0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).ok()).collect::<Option<Vec<u8>>>()).collect()).unwrap_or_default();
        if rows.is_empty() {
            r.err("track_decl.json has no container rows".into());
        }
        let tv_re = regex::Regex::new(r"(?i)^tv\d+_cameras").unwrap();
        let mut any_row: HashSet<String> = HashSet::new();
        // containers listed by a Race row (f5 is_instant_race) / a Hotstint row (f7 is_hot_stint)
        let (mut race, mut hotstint): (HashSet<String>, HashSet<String>) = Default::default();
        let mut race_rows = 0;
        for (i, row) in rows.iter().enumerate() {
            let Ok(m) = Msg::parse(row) else { r.err(format!("container row {i} unreadable")); continue };
            let body = m.sub(8).unwrap_or_default();
            let listed: Vec<String> = body.fields.iter().filter(|f| f.num == 11).filter_map(|f| match &f.val { Val::Bytes(x) => Some(String::from_utf8_lossy(x).to_lowercase()), _ => None }).collect();
            any_row.extend(listed.iter().cloned());
            if body.varint(5) == Some(1) {
                race_rows += 1;
                race.extend(listed.iter().cloned());
            }
            if body.varint(7) == Some(1) {
                hotstint.extend(listed.iter().cloned());
            }
            for l in &listed {
                if l.starts_with(&prefix.to_lowercase()) && !files.keys().any(|k| k.to_lowercase() == *l) {
                    r.err(format!("row {i} lists {l}, which is not in the folder"));
                }
                // the game enables only the main-scene Container entities whose
                // path a row lists (log "[WorldGrid] Enabled Container <guid>"):
                // a shared file without one is never loaded either
                if !entity_kinds.contains_key(l) {
                    r.err(format!("row {i} lists {l}, which the scene has no Container entity for - the game never loads it"));
                } else if !l.starts_with(&prefix.to_lowercase()) && !game.has(l) {
                    r.err(format!("row {i} lists {l}, which is neither in the folder nor in the game"));
                }
            }
        }
        if race_rows == 0 {
            r.err("track_decl.json has no Race row (f5 is_instant_race)".into());
        }
        for d in &declared {
            // stock lists TV cameras only in the No Game Mode row, no fmod container in any row,
            // hotlap spawns only in the Hotstint rows; every other container is in a layout's Race row
            let file = d.rsplit('\\').next().unwrap_or("");
            let other = tv_re.is_match(file) || file.starts_with("fmod_") || d.contains("\\containers\\physics_") || d.contains("\\containers\\decor_");
            if other {
                continue;
            }
            if file.starts_with("spawnpoints_hotlap") {
                if !hotstint.contains(d) {
                    r.err(format!("hotlap container {d} is in no Hotstint row - hotlap mode has no start"));
                }
            } else if !race.contains(d) {
                r.err(format!("scene container {d} is in no Race row - the game won't load it in that mode"));
            }
        }
        for d in declared.iter().filter(|d| tv_re.is_match(d.rsplit('\\').next().unwrap_or(""))) {
            if !any_row.contains(d) {
                r.err(format!("TV camera container {d} is in no declaration row (stock: the No Game Mode row)"));
            }
        }
        // the game's lookup per (layout, mode): rows, names, spawns (decl.rs)
        let track_name = v["tracks_table_row"].as_str().and_then(|h| (0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).ok()).collect::<Option<Vec<u8>>>())
            .and_then(|b| Msg::parse(&b).ok()).and_then(|m| m.sub(2)).and_then(|d| d.str(1)).unwrap_or_default();
        if track_name.is_empty() {
            r.err("track_decl.json: tracks_table_row has no name (f2.f1)".into());
        }
        // the name is the rows' key (f10): another track with it shares (and
        // an installer removing "its" rows by name deletes) the other's rows
        if let Some(t) = game.read("system\\tracks.table").and_then(|b| Msg::parse(&b).ok()) {
            let own = format!("content\\tracks\\{id}").to_lowercase();
            for line in t.sub(2).map(|x| x.subs(3)).unwrap_or_default() {
                let Some(d) = line.sub(2) else { continue };
                if d.str(1).as_deref() == Some(track_name.as_str()) && d.str(3).map(|f| f.to_lowercase() != own).unwrap_or(false) {
                    r.err(format!("track name {track_name:?} is already the game's {} - rows would mix; pick another name", d.str(3).unwrap_or_default()));
                }
            }
        }
        let parsed: Vec<decl::Row> = rows.iter().filter_map(|b| Msg::parse(b).ok()).filter_map(|m| decl::row(&m)).collect();
        if let Some(o) = parsed.iter().find(|x| x.track != track_name) {
            r.err(format!("row {:?} names track {:?}, the track row {:?}", o.event, o.track, track_name));
        }
        // the pit menu (camera mode 10) plays the `camera_sequence` cameras of
        // the session's containers (base row + mode row): none and it shows
        // the world from the origin, under the ground. Stock: a track's own
        // camera_sequence_practice in the Race / Time Attack rows, or the
        // shared car-relative sequence in No Game Mode.
        for layout in parsed.iter().map(|x| x.layout.clone()).collect::<std::collections::BTreeSet<_>>() {
            let base: Vec<&String> = parsed.iter().filter(|x| x.layout == layout && x.base).flat_map(|x| x.containers.iter()).collect();
            for (mode, is) in [("Race", (|x: &decl::Row| x.race) as fn(&decl::Row) -> bool), ("Time Attack", |x: &decl::Row| x.time_attack)] {
                let Some(row) = parsed.iter().find(|x| x.layout == layout && is(x)) else { continue };
                let cams = base.iter().copied().chain(row.containers.iter()).any(|c| entity_kinds.get(&c.to_lowercase()).map(|k| k.iter().any(|t| t == "camera_sequence")).unwrap_or(false));
                if !cams {
                    r.err(format!("layout {layout:?} / {mode}: no loaded container declares camera_sequence - the pit menu has no camera (it looks out of the world origin)"));
                }
            }
        }
        let lower: BTreeMap<String, &Vec<u8>> = files.iter().map(|(k, b)| (k.to_lowercase(), b)).collect();
        let read = |p: &str| -> Option<Vec<u8>> { lower.get(&p.to_lowercase()).map(|b| b.to_vec()).or_else(|| game.read(p)) };
        let _ = decl::check(&track_name, &parsed, &read, true, &mut r);
        // per session: the track-limit / ideal-line splines the game looks
        // up, spawns upright and facing along the track (lines.rs)
        let _ = lines::check(&track_name, &parsed, &scene, &read, true, &mut r);
        // the UI images the menus ask for, placed by the installer
        let ui_files: Vec<(String, String)> = v["ui_files"].as_array().map(|a| a.iter().filter_map(|x| Some((x["from"].as_str()?.to_string(), x["to"].as_str()?.to_string()))).collect()).unwrap_or_default();
        for l in v["layouts"].as_array().into_iter().flatten() {
            let Some(name) = l["name"].as_str() else { continue };
            for want in [format!("uiresources\\images\\tracks\\{}.texture", crate::track::declare::ui_track_id(&track_name, name)),
                         format!("uiresources\\images\\trackmaps\\{}.svg", crate::track::declare::ui_track_id(&track_name, name)),
                         format!("uiresources\\images\\trackmaps\\{}.texture", crate::track::declare::ui_map_id(&track_name, name))] {
                if !ui_files.iter().any(|(_, t)| t.eq_ignore_ascii_case(&want)) {
                    r.warn(format!("layout {name:?}: no {want} in ui_files - the menu shows an empty panel"));
                }
            }
        }
        for (from, _) in &ui_files {
            if !lower.contains_key(&format!("{prefix}{from}").to_lowercase()) {
                r.err(format!("ui_files lists {from}, which is not in the folder"));
            }
        }
    } else {
        r.err("no ui\\track_decl.json".into());
    }
    // ISMesh entities (main scene + containers)
    let mut instances = 0usize;
    for (k, b) in files.iter().filter(|(k, _)| k.to_lowercase().ends_with(".scene")) {
        let short = k.rsplit('\\').next().unwrap_or("").to_string();
        for e in entities(b) {
            if entity_kind(&e).as_deref() == Some("ISMesh") {
                instances += check_ismesh(&e, &short, &mut r);
            }
        }
    }
    if instances > ISMESH_TRACK_MAX {
        r.warn(format!("{instances} ISMesh instances: more than the largest stock track ({ISMESH_TRACK_MAX})"));
    }
    check_dynamic_track(&files, &prefix, id, &mut r);
    check_albedo(dir, &files, game, &mut r);
    check_ai_splines(&files, &prefix, &mut r);
    // lighting invariants of the static meshes + entity boxes
    let mut b2b: Vec<(usize, String)> = Vec::new();
    for (k, b) in files.iter().filter(|(k, _)| k.to_lowercase().ends_with(".mesh") && k.to_lowercase().contains("\\static_mesh\\")) {
        let n = check_mesh_lighting(k, b, &mut r);
        if n > 0 {
            b2b.push((n, k.rsplit('\\').next().unwrap_or("").to_string()));
        }
    }
    if !b2b.is_empty() {
        b2b.sort_by(|a, b| b.0.cmp(&a.0));
        let total: usize = b2b.iter().map(|x| x.0).sum();
        r.warn(format!("{total} back-to-back coincident triangle pair(s) in {} mesh(es) (lit/dark flicker as shadows update): {}", b2b.len(),
            b2b.iter().take(6).map(|(n, f)| format!("{f} {n}")).collect::<Vec<_>>().join(", ")));
    }
    check_smesh_boxes(&files, &mut r);
    // ⚠ the game needs somewhere to put the car: with no Start Pos at all it
    // throws "No starting position!" and crashes while loading (ek_momiji
    // converted with its layout's model files missing)
    let starts: usize = files.iter().filter(|(k, _)| k.to_lowercase().contains(r"\containers\spawnpoints_")).map(|(_, b)| entities(b).iter().filter(|e| entity_kind(e).as_deref() == Some("Start Pos")).count()).sum();
    if starts == 0 {
        r.err("no spawn points (grid, pit or hotlap) - the game crashes loading a track it can't place the car on".into());
    }
    // practice / hotlap sessions start in the pits: none = out_of_range crash
    // creating the car
    let pits: usize = files.iter().filter(|(k, _)| k.to_lowercase().contains(r"\containers\spawnpoints_pitlane")).map(|(_, b)| entities(b).iter().filter(|e| entity_kind(e).as_deref() == Some("Start Pos")).count()).sum();
    if starts > 0 && pits == 0 {
        r.err("no pit spawn points - practice starts in the pits and the game crashes creating the car".into());
    }
    // a drivable surface must exist
    let smesh_keys: usize = files.iter().filter(|(k, _)| k.to_lowercase().contains("\\containers\\layout_")).map(|(_, b)| entities(b).iter().filter(|e| entity_kind(e).as_deref() == Some("SMesh")).count()).sum();
    if smesh_keys == 0 {
        r.err("the layout container has no collision meshes - nothing to drive on".into());
    }
    Ok(r)
}

/// Material brightness against stock (`aceconv albedo`, the class rule of
/// stats/albedo.rs and the stock bounds of track::materials::STOCK_CLASS):
/// a material more than 20 % above its class's stock p95 is an error (the
/// fit holds every material to p05..p95), above p95 or far below p05 a
/// warning. And the DynamicTrack rubber layer: the shader adds the dynamic
/// track's rubber to the Blue weight, so an unused Blue layer must be dark
/// (a white one turns rubbered tarmac white in game).
fn check_albedo(dir: &Path, files: &BTreeMap<String, Vec<u8>>, game: &Game, r: &mut Report) {
    use crate::track::materials::STOCK_CLASS;
    if let Ok(rep) = crate::stats::albedo::measure_dir(dir, Some(&game.pkg)) {
        let (mut high, mut over, mut under) = (Vec::new(), Vec::new(), Vec::new());
        for (name, _, class, y) in &rep.rows {
            let Some(&(_, _, _, lo, hi)) = STOCK_CLASS.iter().find(|x| x.0 == *class) else { continue };
            if *y > hi * 1.2 {
                high.push(format!("{name} ({class}) {y:.3} > {:.3}", hi * 1.2));
            } else if *y > hi + 0.002 {
                over.push(format!("{name} ({class}) {y:.3}"));
            } else if *y < lo * 0.8 && *y > 0.0 {
                under.push(format!("{name} ({class}) {y:.3}"));
            }
        }
        if !high.is_empty() {
            r.err(format!("{} material(s) far brighter than any stock material of their class (stock p95 x 1.2): {}", high.len(), high.join(", ")));
        }
        if !over.is_empty() {
            r.warn(format!("{} material(s) above their class's stock p95: {}", over.len(), over.join(", ")));
        }
        if !under.is_empty() {
            r.warn(format!("{} material(s) well below their class's stock p05: {}", under.len(), under.join(", ")));
        }
    }
    let (white, flat, lost) = road_colour_problems(files);
    if !white.is_empty() {
        r.err(format!("{} DynamicTrackMultilayer material(s) with an unused, bright Blue layer - the shader adds the rubber to Blue, so rubbered surfaces show it (white roads in game): {}", white.len(), white.join(", ")));
    }
    if !flat.is_empty() {
        r.err(format!("{} DynamicTrack material(s) without Base_HasMap / Detail_HasMap: the shader draws them a flat Base_Basecolor (white roads in game): {}", flat.len(), flat.join(", ")));
    }
    if !lost.is_empty() {
        r.err(format!("{} DynamicTrack material(s) with track textures in slots that shader never samples (Base_BaseColorMap / txDiffuse / layers / SpatMap; the road texture and its painted lines are lost): {}", lost.len(), lost.join(", ")));
    }
}

/// The road colour checks of [`check_albedo`] on the written materials,
/// by the game's shaders (stats/albedo.rs dynamic_track_albedo):
/// (multilayer materials with a bright unused Blue layer, DynamicTrack
/// materials with a flat bright colour, DynamicTrack materials carrying the
/// track's own textures in slots the DynamicTrack shader does not read).
pub fn road_colour_problems(files: &BTreeMap<String, Vec<u8>>) -> (Vec<String>, Vec<String>, Vec<String>) {
    use crate::io::material::{get_prop, get_slot};
    let (mut white, mut flat, mut lost) = (Vec::new(), Vec::new(), Vec::new());
    let lum = |c: &[f32]| c.first().copied().unwrap_or(0.0) * 0.2126 + c.get(1).copied().unwrap_or(0.0) * 0.7152 + c.get(2).copied().unwrap_or(0.0) * 0.0722;
    for (k, b) in files.iter().filter(|(k, _)| k.to_lowercase().ends_with(".material")) {
        let Ok(m) = Msg::parse(b) else { continue };
        let name = k.rsplit('\\').next().unwrap_or("").trim_end_matches(".material").to_string();
        let on = |n: &str| get_prop(&m, n, 1).map(|v| v[0] != 0.0).unwrap_or(false);
        match m.str(1).as_deref() {
            Some("DynamicTrackMultilayer") => {
                if !on("Blue_Enable") && lum(&get_prop(&m, "Blue_Basecolor", 4).unwrap_or_default()) > 0.5 {
                    white.push(name);
                }
            }
            Some("DynamicTrack") => {
                if !on("Base_HasMap") && !on("Detail_HasMap") && lum(&get_prop(&m, "Base_Basecolor", 4).unwrap_or_default()) > 0.5 {
                    flat.push(name.clone());
                }
                let own = ["Base_BaseColorMap", "txDiffuse", "Red_BaseColorMap", "Green_BaseColorMap", "Blue_BaseColorMap", "SpatMap"]
                    .iter().any(|s| get_slot(&m, s).map(|p| !p.to_lowercase().contains("\\common_assets\\")).unwrap_or(false));
                if own {
                    lost.push(name);
                }
            }
            _ => {}
        }
    }
    (white, flat, lost)
}

pub fn print(r: &Report, what: &str) {
    println!("{what}: {} files, {} references, {} errors, {} warnings", r.files, r.refs, r.errors.len(), r.warnings.len());
    for e in &r.errors {
        println!("  ERROR {e}");
    }
    for w in r.warnings.iter().take(20) {
        println!("  warn  {w}");
    }
}

/// layouts\layout_<L>.{ideal_line,pitlane}.aisplinedata and
/// .trackcontrolpoints of every layout container: present (the game logs
/// "no ideal line" and AI cars cannot drive without one) and readable.
fn check_ai_splines(files: &BTreeMap<String, Vec<u8>>, prefix: &str, r: &mut Report) {
    let pl = prefix.to_lowercase();
    let get = |rel: &str| files.iter().find(|(k, _)| k.to_lowercase() == format!("{pl}{rel}")).map(|(_, b)| b);
    let stems: Vec<String> = files.keys().filter_map(|k| k.to_lowercase().strip_prefix(&format!("{pl}containers\\")).and_then(|x| x.strip_suffix(".scene")).filter(|x| x.starts_with("layout_")).map(String::from)).collect();
    for stem in stems {
        for (kind, needed) in [("ideal_line", true), ("pitlane", false)] {
            let rel = format!("layouts\\{stem}.{kind}.aisplinedata");
            let Some(b) = get(&rel) else {
                r.warn(format!("{stem}: no {rel}{}", if needed { " - AI cars cannot drive, no track progress" } else { " - AI cars cannot pit" }));
                continue;
            };
            match crate::track::ai::parse(b) {
                Err(e) => r.err(format!("{rel}: {e:#}")),
                Ok(p) => {
                    let mut seg: Vec<f32> = p.points.windows(2).map(|w| ((w[0][0] - w[1][0]).powi(2) + (w[0][1] - w[1][1]).powi(2) + (w[0][2] - w[1][2]).powi(2)).sqrt()).collect();
                    seg.sort_by(|a, b| a.partial_cmp(b).unwrap());
                    let med = seg.get(seg.len() / 2).copied().unwrap_or(0.0);
                    // stock: 1.6..5.4 m per point on ideal lines, 2.2 on pit lanes
                    if !(0.3..=8.0).contains(&med) {
                        r.warn(format!("{rel}: {med:.2} m per point (stock 1.6..5.4)"));
                    }
                    if p.points.iter().flatten().any(|v| !v.is_finite()) {
                        r.err(format!("{rel}: non-finite points"));
                    }
                    if kind == "ideal_line" && p.control_points == 0 {
                        r.warn(format!("{rel}: no control points (f7)"));
                    }
                }
            }
        }
        let rel = format!("layouts\\{stem}.trackcontrolpoints");
        match get(&rel) {
            Some(b) => {
                if let Err(e) = crate::track::ai::parse_control_points(b) {
                    r.err(format!("{rel}: {e:#}"));
                }
            }
            None => r.warn(format!("{stem}: no {rel}")),
        }
    }
}

/// dynamic_track/<L>.dynamictrackpresetcompressed: parses (track::dyntrack),
/// one per layout container, and its frame is the .track f1.f8/f9 frame.
fn check_dynamic_track(files: &BTreeMap<String, Vec<u8>>, prefix: &str, id: &str, r: &mut Report) {
    let pl = prefix.to_lowercase();
    let frame = files.get(&format!("{prefix}{id}.track")).and_then(|b| Msg::parse(b).ok()).and_then(|t| t.sub(1)).map(|d| {
        let c = d.sub(8).map(|v| [v.f32(1).unwrap_or(0.0), v.f32(2).unwrap_or(0.0), v.f32(3).unwrap_or(0.0)]);
        (c, d.f32(9))
    });
    let layouts: Vec<String> = files.keys().filter_map(|k| k.to_lowercase().strip_prefix(&format!("{pl}containers\\layout_")).and_then(|x| x.strip_suffix(".scene")).map(String::from)).collect();
    // stock names a preset after its layout's display name (the rows' f14,
    // lower case: `gp`, `gp short`), which track_decl.json lists
    let names: Vec<String> = files.get(&format!("{prefix}ui\\track_decl.json")).and_then(|d| serde_json::from_slice::<serde_json::Value>(d).ok())
        .and_then(|v| v["layouts"].as_array().map(|a| a.iter().filter_map(|l| l["name"].as_str().map(|s| s.to_lowercase())).collect())).unwrap_or_default();
    let mut n = 0;
    for (k, b) in files {
        let kl = k.to_lowercase();
        let Some(layout) = kl.strip_prefix(&format!("{pl}dynamic_track\\")).and_then(|x| x.strip_suffix(".dynamictrackpresetcompressed")) else { continue };
        n += 1;
        let name = k.rsplit('\\').next().unwrap_or("");
        if !layouts.iter().any(|l| l == layout) && !names.iter().any(|n| n == layout) {
            r.warn(format!("{name}: no layout named {layout:?} in track_decl.json and no layout_{layout}.scene"));
        }
        let p = match crate::track::dyntrack::parse(b) {
            Ok(p) => p,
            Err(e) => {
                r.err(format!("{name}: {e:#}"));
                continue;
            }
        };
        if p.tiles.is_empty() {
            r.warn(format!("{name}: no tiles"));
        }
        let ext = p.max[0] - p.min[0];
        if (p.max[2] - p.min[2] - ext).abs() > 0.01 || p.min[1] != p.max[1] || !(ext > 0.0) {
            r.err(format!("{name}: frame {:?}..{:?} is not a square", p.min, p.max));
        }
        match frame {
            Some((c, Some(e))) => {
                let c = c.unwrap_or([0.0; 3]);
                let want = [c[0] - e / 2.0, c[1], c[2] - e / 2.0];
                if (ext - e).abs() > 0.01 || (0..3).any(|i| (p.min[i] - want[i]).abs() > 0.01) {
                    r.err(format!("{name}: frame min {:?} extent {ext} != .track f1.f8 {c:?} f9 {e}", p.min));
                }
            }
            _ => r.err(format!("{name}: {id}.track has no f1.f9 extent")),
        }
    }
    if n == 0 && frame.map(|f| f.0.is_some()).unwrap_or(false) {
        r.warn(format!("{id}.track carries a dynamic-track frame but there is no dynamic_track preset"));
    }
}

#[cfg(test)]
mod road_tests {
    use super::*;
    use crate::io::material::{set_prop, set_slot};

    #[test]
    fn white_dynamic_track_road_is_caught() {
        // what the converter wrote before: a DynamicTrack road with the AC1
        // texture on Base_BaseColorMap (never sampled) and Base_HasMap off
        let mut m = Msg::default();
        m.set_str(1, "DynamicTrack");
        for (k, v) in [("Base_HasMap", 0.0), ("Detail_HasMap", 0.0)] {
            set_prop(&mut m, k, &[v]);
        }
        set_prop(&mut m, "Base_Basecolor", &[1.0, 1.0, 1.0, 1.0]);
        set_slot(&mut m, "Base_BaseColorMap", "content\\tracks\\x\\textures\\asph.texture");
        let mut files = BTreeMap::new();
        files.insert("content\\tracks\\x\\materials\\asph4.material".to_string(), m.encode());
        let (white, flat, lost) = road_colour_problems(&files);
        assert!(white.is_empty());
        assert_eq!(flat, vec!["asph4".to_string()]);
        assert_eq!(lost, vec!["asph4".to_string()]);
        // the shader's colour for it: flat white
        let a = crate::stats::albedo::material_albedo(&m, &|_| Some([0.2; 4])).unwrap();
        assert_eq!(a, [1.0; 3]);
    }
}
