//! Measure stock content (READ ONLY) or one converted item.
//!
//! Per car (`content\cars\<id>\`) and per track (`content\tracks\<id>\`):
//! file counts and sizes by kind, materials and shaders, textures by
//! format, mesh vertices / triangles / sections / LODs / bones, animations,
//! the top-level entry counts of the car's system files (lighting, LEDs,
//! analogue gauges, actor), track scene entities by kind, ISMesh instances
//! (per node and per track), irradiance probes, dynamic-track presets
//! (tiles, rubber / grip histograms), AI splines.
//! Global facts: id rules, the texture header table, kspkg record ids, the
//! largest ISMesh node and track.

use anyhow::{Context, Result};
use rayon::prelude::*;
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use super::{fit, ranges, Fit, Metrics, Stock};
use crate::io::kspkg::{entry_id, Pkg};
use crate::io::pb::{self, Msg, Val};

/// Bumped when a metric's meaning changes.
pub const VERSION: u32 = 4;

fn ext_of(p: &str) -> String {
    let f = p.rsplit('\\').next().unwrap_or(p);
    f.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
}

fn add(m: &mut Metrics, k: &str, v: f64) {
    *m.entry(k.to_string()).or_insert(0.0) += v;
}

fn maxm(m: &mut Metrics, k: &str, v: f64) {
    let e = m.entry(k.to_string()).or_insert(v);
    if v > *e {
        *e = v;
    }
}

/// Kinds worth a count of their own.
const CAR_EXTS: [&str; 16] = ["mesh", "material", "texture", "animation", "curve", "carpart", "compatiblepart", "design", "rimmesh", "compatiblerim",
    "carfinalstate", "visualcarpreset", "mechanicalcarpreset", "html", "bank", "carsetup"];
const TRACK_EXTS: [&str; 10] = ["mesh", "material", "texture", "scene", "aisplinedata", "trackcontrolpoints", "dynamictrackpresetcompressed", "track_layout", "reference", "bank"];
/// Car files whose top-level repeated entries are counted (lamps, LEDs, gauges, actor slots).
const CAR_SYSTEMS: [&str; 5] = ["carlightingsystem", "carledsystem", "caranalogicsystem", "actor", "carexhaustbackfire"];

/// Mesh: f5 LOD bodies {f4 sections, f5 positions, f11 indices, f12 bones}.
fn mesh_metrics(b: &[u8], m: &mut Metrics, prefix: &str) {
    let Ok(top) = Msg::parse(b) else { return add(m, &format!("{prefix}unreadable"), 1.0) };
    let lods = top.subs(5);
    maxm(m, &format!("{prefix}lods_max"), lods.len() as f64);
    if let Some(l0) = lods.first() {
        let nv = l0.bytes(5).map(|p| p.len() / 12).unwrap_or(0);
        let ni = l0.bytes(11).map(|i| pb::varints(i).len()).unwrap_or(0);
        add(m, &format!("{prefix}vertices"), nv as f64);
        add(m, &format!("{prefix}tris"), (ni / 3) as f64);
        add(m, &format!("{prefix}sections"), l0.all(4).count() as f64);
        let bones = l0.all(12).count();
        add(m, &format!("{prefix}bones"), bones as f64);
        maxm(m, &format!("{prefix}bones_max"), bones as f64);
    }
}

fn texture_metrics(b: &[u8], mips: usize, m: &mut Metrics, prefix: &str) {
    let Ok(h) = Msg::parse(b) else { return add(m, &format!("{prefix}unreadable"), 1.0) };
    let fmt = h.varint(4).unwrap_or(0);
    add(m, &format!("{prefix}fmt.{fmt}"), 1.0);
    let dim = h.varint(1).unwrap_or(0).max(h.varint(2).unwrap_or(0));
    maxm(m, &format!("{prefix}max_dim"), dim as f64);
    add(m, &format!("{prefix}mips_mb"), mips as f64 / 1048576.0);
}

fn material_metrics(b: &[u8], m: &mut Metrics, shaders: &mut BTreeMap<String, usize>) {
    let Ok(x) = Msg::parse(b) else { return add(m, "material.unreadable", 1.0) };
    *shaders.entry(x.str(1).unwrap_or_default()).or_default() += 1;
    let used = x.subs(5).iter().filter(|s| s.sub(2).and_then(|t| t.str(2)).map(|p| !p.is_empty()).unwrap_or(false)).count();
    add(m, "material.slots_used", used as f64);
}

fn entity_kind(e: &Msg) -> Option<String> {
    e.fields.iter().find(|f| f.num == 3).and_then(|f| match &f.val { Val::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()), _ => None })
}

/// ISMesh instance count of one entity (f50.f105.f7.f2 = positions).
fn ismesh_instances(e: &Msg) -> usize {
    e.sub(50).and_then(|x| x.sub(105)).and_then(|d| d.sub(7)).and_then(|t| t.bytes(2).map(|b| b.len() / 12)).unwrap_or(0)
}

/// Facts collected while measuring, merged into `Stock::facts`.
#[derive(Default)]
struct Facts {
    ismesh_node_max: (usize, String),
    ismesh_track_max: (usize, String),
}

/// Vegetation instances by class (trees, impostors, bushes, grass): per
/// class (cull distance, instances) per node and instances per mesh.
#[derive(Default)]
struct VegAcc {
    nodes: std::collections::BTreeMap<&'static str, Vec<(f64, usize)>>,
    meshes: std::collections::BTreeMap<&'static str, std::collections::BTreeMap<String, usize>>,
}

fn scene_metrics(b: &[u8], m: &mut Metrics, track: &str, facts: &mut Facts, probe_xz: &mut Vec<[f32; 2]>, veg: &mut VegAcc) {
    let Ok(s) = Msg::parse(b) else { return add(m, "scene.unreadable", 1.0) };
    for e in s.subs(2) {
        let k = entity_kind(&e).unwrap_or_else(|| "(none)".into());
        add(m, &format!("entity.{k}"), 1.0);
        if k == "ISMesh" {
            let n = ismesh_instances(&e);
            add(m, "ismesh.instances", n as f64);
            maxm(m, "ismesh.node_max", n as f64);
            if n > facts.ismesh_node_max.0 {
                facts.ismesh_node_max = (n, format!("{track}: {}", e.str(1).unwrap_or_default()));
            }
            let mesh = e.sub(50).and_then(|x| x.sub(105)).and_then(|d| d.str(1)).unwrap_or_default();
            if let Some(c) = crate::track::vegcat::stats_class(&mesh) {
                let cull = e.sub(13).and_then(|b| b.f32(3)).unwrap_or(0.0) as f64;
                veg.nodes.entry(c).or_default().push((cull, n));
                *veg.meshes.entry(c).or_default().entry(mesh.to_lowercase()).or_default() += n;
            }
        }
        if k == "Surface Def" {
            // SurfaceDefinitionData entries: count, grip range of the surfaces
            // (category 1), the largest sin height and damping
            for d in e.sub(50).and_then(|a| a.sub(109)).map(|x| x.subs(1)).unwrap_or_default() {
                add(m, "surface.entries", 1.0);
                if d.varint(2) == Some(1) {
                    let g = d.f32(3).unwrap_or(0.0) as f64;
                    maxm(m, "surface.grip_max", g);
                    let e = m.entry("surface.grip_min".to_string()).or_insert(g);
                    if g < *e {
                        *e = g;
                    }
                    maxm(m, "surface.sin_height_max", d.f32(10).unwrap_or(0.0) as f64);
                    maxm(m, "surface.damping_max", d.f32(12).unwrap_or(0.0) as f64);
                }
            }
        }
        if k == "Start Pos" {
            // packed types: 3 grid, 4 pit, 5 box
            let types = e.sub(50).and_then(|a| a.sub(104)).and_then(|d| d.bytes(6).map(pb::varints)).unwrap_or_default();
            for t in types {
                match t {
                    3 => add(m, "spawn.grid_or_hotlap", 1.0),
                    4 => add(m, "spawn.pit", 1.0),
                    5 => add(m, "spawn.box", 1.0),
                    _ => {}
                }
            }
        }
        if k == "Irradiance Probe" {
            if let Some(p) = e.sub(4).and_then(|t| t.sub(1)) {
                probe_xz.push([p.f32(1).unwrap_or(0.0), p.f32(3).unwrap_or(0.0)]);
            }
            if let Some(gi) = e.sub(50).and_then(|a| a.sub(120)).and_then(|d| d.f32(2)) {
                maxm(m, "irradiance.ground_interval", gi as f64);
            }
        }
    }
    if let Some(p) = s.sub(3).and_then(|g| g.sub(4)) {
        m.insert("pigment.block".into(), 1.0);
        if p.str(2).map(|t| !t.is_empty()).unwrap_or(false) {
            m.insert("pigment.texture".into(), 1.0);
        }
    }
}

/// One R11G11B10 unsigned float texel -> rgb.
fn r11g11b10(u: u32) -> [f32; 3] {
    let f = |x: u32, mb: u32| {
        let e = (x >> mb) & 31;
        let mant = (x & ((1 << mb) - 1)) as f32 / (1u32 << mb) as f32;
        match e {
            0 => mant * 2f32.powi(-14),
            31 => f32::INFINITY,
            _ => (1.0 + mant) * 2f32.powi(e as i32 - 15),
        }
    };
    [f(u & 0x7ff, 6), f((u >> 11) & 0x7ff, 6), f(u >> 22, 5)]
}

/// Probe texels (bounced-sky volumes, 64 x 3 x 16, x = 4 x cell + SH
/// coefficient, y = level): coefficient 0 (G) per level, the directional
/// coefficients, non-finite texels.
#[derive(Default)]
struct ProbeTexels {
    sh0: [Vec<f64>; 3],
    dir: Vec<f64>,
    bad: usize,
    max: f64,
}

fn probe_texels(b: &[u8], t: &mut ProbeTexels) {
    if b.len() != 12288 {
        return;
    }
    for (i, c) in b.chunks_exact(4).enumerate() {
        let v = r11g11b10(u32::from_le_bytes([c[0], c[1], c[2], c[3]]));
        if v.iter().any(|x| !x.is_finite()) {
            t.bad += 1;
            continue;
        }
        t.max = t.max.max(v[0].max(v[1]).max(v[2]) as f64);
        let (x, y) = (i % 64, (i / 64) % 3);
        if x % 4 == 0 {
            t.sh0[y].push(v[1] as f64);
        } else if i % 3 == 0 {
            t.dir.push(v[1] as f64);
        }
    }
}

/// AO (colour stream component 3 x 255) of a mesh's LOD0, sampled.
fn mesh_ao(b: &[u8]) -> Vec<f64> {
    let Ok(top) = Msg::parse(b) else { return Vec::new() };
    let Some(l0) = top.sub(5) else { return Vec::new() };
    let Some(c) = l0.bytes(13) else { return Vec::new() };
    let n = c.len() / 16;
    let step = (n / 300).max(1);
    (0..n).step_by(step).map(|v| {
        let o = v * 16 + 12;
        f32::from_le_bytes([c[o], c[o + 1], c[o + 2], c[o + 3]]) as f64 * 255.0
    }).collect()
}

fn dyntrack_metrics(b: &[u8], m: &mut Metrics) {
    match crate::track::dyntrack::parse(b) {
        Ok(p) => {
            add(m, "dyntrack.presets", 1.0);
            maxm(m, "dyntrack.tiles_max", p.tiles.len() as f64);
            maxm(m, "dyntrack.extent", (p.max[0] - p.min[0]) as f64);
            let (mut zero, mut road, mut rmax) = (0usize, 0usize, 0u8);
            let mut grip: Vec<f64> = Vec::new();
            for t in &p.tiles {
                for (c, h) in t.cells.chunks_exact(2).zip(&t.heights) {
                    if *h <= -9_000_000.0 {
                        continue;
                    }
                    road += 1;
                    if c[0] == 0 {
                        zero += 1;
                    }
                    rmax = rmax.max(c[0]);
                    grip.push(c[1] as f64);
                }
            }
            maxm(m, "dyntrack.road_cells_max", road as f64);
            if road > 0 {
                maxm(m, "dyntrack.rubber_zero_share", zero as f64 / road as f64);
                maxm(m, "dyntrack.rubber_max", rmax as f64);
                if let Some(r) = super::Range::of(&grip) {
                    maxm(m, "dyntrack.grip_p10", r.p10);
                    maxm(m, "dyntrack.grip_p50", r.p50);
                    maxm(m, "dyntrack.grip_p90", r.p90);
                }
            }
        }
        Err(_) => add(m, "dyntrack.unreadable", 1.0),
    }
}

/// One item's files: (archive path, bytes or None for skipped pixel data, size).
type Item<'a> = Vec<(&'a str, Option<Vec<u8>>, usize)>;

/// Car metrics from its files (paths `content\cars\<id>\...`).
pub fn car_metrics(files: &Item) -> Metrics {
    let mut m = Metrics::new();
    let mut shaders = BTreeMap::new();
    let sizes: HashMap<String, usize> = files.iter().map(|(p, _, n)| (p.to_lowercase(), *n)).collect();
    for (p, data, size) in files {
        let e = ext_of(p);
        add(&mut m, "files", 1.0);
        add(&mut m, "mb", *size as f64 / 1048576.0);
        if CAR_EXTS.contains(&e.as_str()) {
            add(&mut m, &format!("ext.{e}"), 1.0);
        }
        let Some(b) = data else { continue };
        let low = p.to_lowercase();
        match e.as_str() {
            "mesh" => {
                mesh_metrics(b, &mut m, "mesh.");
                if low.ends_with("_exterior.mesh") {
                    mesh_metrics(b, &mut m, "exterior.");
                } else if low.ends_with("_interior.mesh") {
                    mesh_metrics(b, &mut m, "interior.");
                }
            }
            "texture" => texture_metrics(b, sizes.get(&format!("{low}mips")).copied().unwrap_or(0), &mut m, "tex."),
            "material" => material_metrics(b, &mut m, &mut shaders),
            x if CAR_SYSTEMS.contains(&x) => {
                if let Ok(t) = Msg::parse(b) {
                    let mut c: BTreeMap<u32, usize> = BTreeMap::new();
                    for f in &t.fields {
                        *c.entry(f.num).or_default() += 1;
                    }
                    for (n, k) in c {
                        add(&mut m, &format!("sys.{x}.f{n}"), k as f64);
                    }
                }
            }
            _ => {}
        }
    }
    m.insert("material.shaders".into(), shaders.len() as f64);
    m
}

/// The `phys.*` metrics (car::physics::stats); paths outside the item
/// (stock common tyres) are read from the package when there is one.
fn phys_metrics(files: &Item, pkg: Option<&Pkg>) -> Metrics {
    let map: HashMap<String, &Vec<u8>> = files.iter().filter_map(|(p, d, _)| d.as_ref().map(|d| (p.to_lowercase(), d))).collect();
    let get = |p: &str| -> Option<Vec<u8>> {
        map.get(&p.to_lowercase()).map(|d| (*d).clone()).or_else(|| {
            let g = pkg?;
            g.find(p).and_then(|e| g.read(e).ok())
        })
    };
    let Some(cdp) = crate::car::physics::stats::cardata_of(files.iter().map(|(p, _, _)| *p)) else { return Metrics::new() };
    let Some(cd) = get(&cdp).and_then(|b| Msg::parse(&b).ok()) else { return Metrics::new() };
    let d = crate::car::physics::stats::derive(&cd, &get);
    crate::car::physics::stats::metrics(&d).into_iter().collect()
}

/// AI spline metrics (largest ideal line / pit lane of the track):
/// length, metres per point, closed, and the ideal line's control points
/// per km.
fn ai_metrics(low: &str, b: &[u8], m: &mut Metrics) {
    let Ok(p) = crate::track::ai::parse(b) else { return };
    let n = p.points.len().max(2) as f64;
    let key = if low.ends_with(".ideal_line.aisplinedata") { "ai.ideal" } else if low.ends_with(".pitlane.aisplinedata") { "ai.pit" } else { return };
    if p.length > m.get(&format!("{key}_len_m")).copied().unwrap_or(0.0) {
        m.insert(format!("{key}_len_m"), p.length);
        m.insert(format!("{key}_m_per_point"), p.length / (n - 1.0));
        if key == "ai.ideal" {
            m.insert("ai.ideal_closed".into(), if p.closed { 1.0 } else { 0.0 });
            m.insert("ai.ideal_cp_per_km".into(), p.control_points as f64 / (p.length / 1000.0).max(0.1));
        }
    }
}

/// Track metrics from its files (paths `content\tracks\<id>\...`).
fn track_metrics_inner(id: &str, files: &Item, facts: &mut Facts) -> Metrics {
    let mut m = Metrics::new();
    let mut shaders = BTreeMap::new();
    let mut probes: Vec<[f32; 2]> = Vec::new();
    let mut veg = VegAcc::default();
    let sizes: HashMap<String, usize> = files.iter().map(|(p, _, n)| (p.to_lowercase(), *n)).collect();
    // material -> shader (the AO statistics group meshes by it)
    let shader_of: HashMap<String, String> = files.iter().filter(|(p, _, _)| ext_of(p) == "material")
        .filter_map(|(p, d, _)| d.as_ref().and_then(|b| Msg::parse(b).ok()).map(|x| (p.to_lowercase(), x.str(1).unwrap_or_default()))).collect();
    let mut ao: BTreeMap<&'static str, Vec<f64>> = BTreeMap::new();
    let mut tex = ProbeTexels::default();
    for (p, data, size) in files {
        let e = ext_of(p);
        let low = p.to_lowercase();
        add(&mut m, "files", 1.0);
        add(&mut m, "mb", *size as f64 / 1048576.0);
        if TRACK_EXTS.contains(&e.as_str()) {
            add(&mut m, &format!("ext.{e}"), 1.0);
        }
        if low.contains("\\irradiance_volumes\\") && e == "texture" {
            add(&mut m, "irradiance.volume_textures", 1.0);
            continue;
        }
        if low.contains("\\irradiance_volumes\\iv_bounced_sky") && e == "texturemips" {
            if let Some(b) = data {
                probe_texels(b, &mut tex);
            }
            continue;
        }
        let Some(b) = data else { continue };
        match e.as_str() {
            "mesh" => {
                mesh_metrics(b, &mut m, "mesh.");
                // collision-only meshes (static_mesh\physics\): count and the largest
                if low.contains("\\static_mesh\\physics\\") {
                    add(&mut m, "mesh.phys_meshes", 1.0);
                    let tris = Msg::parse(b).ok().and_then(|t| t.sub(5)).and_then(|l| l.bytes(11).map(|i| pb::varints(i).len() / 3)).unwrap_or(0);
                    maxm(&mut m, "mesh.phys_tris_max", tris as f64);
                }
                let mat = Msg::parse(b).ok().and_then(|t| t.sub(5)).and_then(|l| l.sub(4)).and_then(|x| x.str(4)).unwrap_or_default().to_lowercase();
                let group = match shader_of.get(&mat).map(|s| s.as_str()) {
                    Some("DynamicTrack") => "dynamictrack",
                    Some("DynamicTrackMultilayer") => "multilayer",
                    Some("DefaultMaterial") => "default",
                    Some("UberMaterial") | Some("UberBiplanarMaterial") => "uber",
                    Some("Fence") => "fence",
                    _ => "",
                };
                if !group.is_empty() {
                    ao.entry(group).or_default().extend(mesh_ao(b));
                }
            }
            "texture" => texture_metrics(b, sizes.get(&format!("{low}mips")).copied().unwrap_or(0), &mut m, "tex."),
            "material" => material_metrics(b, &mut m, &mut shaders),
            "scene" => scene_metrics(b, &mut m, id, facts, &mut probes, &mut veg),
            "dynamictrackpresetcompressed" => dyntrack_metrics(b, &mut m),
            "aisplinedata" => {
                add(&mut m, "ai.spline_kb", b.len() as f64 / 1024.0);
                ai_metrics(&low, b, &mut m);
            }
            "trackcontrolpoints" => {
                if let Ok(n) = crate::track::ai::parse_control_points(b) {
                    maxm(&mut m, "ai.control_points_max", n as f64);
                }
            }
            "track" if low.ends_with(&format!("\\{id}.track")) => {
                if let Some(d) = Msg::parse(b).ok().and_then(|t| t.sub(1)) {
                    if let Some(e) = d.f32(9) {
                        m.insert("track.dyn_extent".into(), e as f64);
                    }
                }
            }
            _ => {}
        }
    }
    m.insert("material.shaders".into(), shaders.len() as f64);
    for (g, v) in &ao {
        if let Some(r) = super::Range::of(v) {
            m.insert(format!("ao.{g}.p10"), r.p10);
            m.insert(format!("ao.{g}.p50"), r.p50);
        }
    }
    for (l, v) in tex.sh0.iter().enumerate() {
        if let Some(r) = super::Range::of(v) {
            m.insert(format!("irradiance.sh0_level{l}.p10"), r.p10);
            m.insert(format!("irradiance.sh0_level{l}.p50"), r.p50);
            m.insert(format!("irradiance.sh0_level{l}.p90"), r.p90);
        }
    }
    if let Some(r) = super::Range::of(&tex.dir) {
        m.insert("irradiance.sh_dir.p90".into(), r.p90);
        m.insert("irradiance.texel_max".into(), tex.max);
        m.insert("irradiance.texel_nonfinite".into(), tex.bad as f64);
    }
    if probes.len() >= 2 {
        let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
        for p in &probes {
            for k in 0..2 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        let km2 = ((hi[0] - lo[0]) as f64 * (hi[1] - lo[1]) as f64 / 1e6).max(1e-6);
        m.insert("irradiance.probe_box_km2".into(), km2);
        m.insert("irradiance.probes_per_km2".into(), probes.len() as f64 / km2);
    }
    // vegetation: instances, per km2 of the probe box, the instance-weighted
    // median cull distance, species count and the largest species' share
    let km2 = m.get("irradiance.probe_box_km2").copied();
    for (c, nodes) in &veg.nodes {
        let total: usize = nodes.iter().map(|x| x.1).sum();
        if total == 0 {
            continue;
        }
        m.insert(format!("veg.{c}.instances"), total as f64);
        if let Some(k) = km2 {
            m.insert(format!("veg.{c}.per_km2"), total as f64 / k);
        }
        let mut v = nodes.clone();
        v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let mut acc = 0usize;
        let med = v.iter().find(|x| { acc += x.1; acc * 2 >= total }).map(|x| x.0).unwrap_or(0.0);
        m.insert(format!("veg.{c}.cull_p50"), med);
        if let Some(ms) = veg.meshes.get(c) {
            m.insert(format!("veg.{c}.species"), ms.len() as f64);
            m.insert(format!("veg.{c}.top_share"), *ms.values().max().unwrap_or(&0) as f64 / total as f64);
        }
    }
    let total = m.get("ismesh.instances").copied().unwrap_or(0.0) as usize;
    if total > facts.ismesh_track_max.0 {
        facts.ismesh_track_max = (total, id.to_string());
    }
    m
}

pub fn track_metrics(id: &str, files: &Item) -> Metrics {
    track_metrics_inner(id, files, &mut Facts::default())
}

/// Measure every stock car and track in `content`.
pub fn measure(content: &Path) -> Result<Stock> {
    let t0 = std::time::Instant::now();
    let pkg = Pkg::open(content)?;
    let meta = std::fs::metadata(content)?;
    let mtime = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
    // ---- kspkg record ids
    let (mut id_lower, mut id_exact_only) = (0usize, 0usize);
    for e in &pkg.entries {
        if e.id == entry_id(&e.path) {
            id_lower += 1;
        } else if e.id == raw_id(&e.path) {
            id_exact_only += 1;
        }
    }
    // ---- group by item
    let mut items: BTreeMap<(String, String), Vec<usize>> = BTreeMap::new();
    for (i, e) in pkg.entries.iter().enumerate() {
        let parts: Vec<&str> = e.path.splitn(4, '\\').collect();
        if parts.len() == 4 && parts[0].eq_ignore_ascii_case("content") && (parts[1].eq_ignore_ascii_case("cars") || parts[1].eq_ignore_ascii_case("tracks")) {
            items.entry((parts[1].to_lowercase(), parts[2].to_lowercase())).or_default().push(i);
        }
    }
    let skip_data = |p: &str| {
        let l = p.to_lowercase();
        (l.ends_with(".texturemips") && !l.contains("\\irradiance_volumes\\iv_bounced_sky")) || l.ends_with(".bank") || l.ends_with(".png") || l.ends_with(".jpg") || l.ends_with(".ttf") || l.ends_with(".otf")
            || (l.contains("\\irradiance_volumes\\") && l.ends_with(".texture"))
    };
    // items that are not stock content on this machine (a track-slot install
    // replaces a stock track's files) or not a circuit (interns: the garage /
    // showroom scenes): ACECONV_STOCK_EXCLUDE, comma-separated ids
    let exclude: Vec<String> = std::env::var("ACECONV_STOCK_EXCLUDE").unwrap_or_default().split(',').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect();
    let list: Vec<(&(String, String), &Vec<usize>)> = items.iter().filter(|((_, id), _)| id != "common_assets" && id != "common_phsx" && !exclude.contains(id)).collect();
    let done = std::sync::atomic::AtomicUsize::new(0);
    let results: Vec<(String, String, Metrics, Facts)> = list.par_iter().map(|((kind, id), idx)| {
        let mut idx = (*idx).clone();
        idx.sort_by_key(|&i| pkg.entries[i].offset());
        let mut files: Item = Vec::with_capacity(idx.len());
        for &i in &idx {
            let e = &pkg.entries[i];
            let data = if skip_data(&e.path) { None } else { pkg.read(e).ok() };
            files.push((e.path.as_str(), data, e.size));
        }
        let mut facts = Facts::default();
        let mut m = if kind == "cars" { car_metrics(&files) } else { track_metrics_inner(id, &files, &mut facts) };
        if kind == "cars" {
            m.extend(phys_metrics(&files, Some(&pkg)));
        }
        let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        eprintln!("[{n}/{}] {kind}\\{id}: {} files ({:.0} s)", list.len(), files.len(), t0.elapsed().as_secs_f64());
        (kind.clone(), id.clone(), m, facts)
    }).collect();
    let mut st = Stock { version: VERSION, content: (meta.len(), mtime), entries: pkg.entries.len(), ..Default::default() };
    let mut facts = Facts::default();
    for (kind, id, m, f) in results {
        if f.ismesh_node_max.0 > facts.ismesh_node_max.0 {
            facts.ismesh_node_max = f.ismesh_node_max;
        }
        if f.ismesh_track_max.0 > facts.ismesh_track_max.0 {
            facts.ismesh_track_max = f.ismesh_track_max;
        }
        if kind == "cars" { st.cars.insert(id, m) } else { st.tracks.insert(id, m) };
    }
    st.car_ranges = ranges(&st.cars);
    st.track_ranges = ranges(&st.tracks);
    // ---- texture header table (all stock .texture headers)
    let textures = texture_table(&pkg);
    let lens = |v: &BTreeMap<String, Metrics>| -> (usize, usize) { (v.keys().map(|k| k.len()).min().unwrap_or(0), v.keys().map(|k| k.len()).max().unwrap_or(0)) };
    let charset = |v: &BTreeMap<String, Metrics>| v.keys().all(|k| k.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_'));
    st.facts = json!({
        "ids": {
            "cars": st.cars.keys().collect::<Vec<_>>(), "car_len": lens(&st.cars), "cars_all_ks_prefix": st.cars.keys().all(|k| k.starts_with("ks_")),
            "cars_charset_a_z_0_9_underscore": charset(&st.cars),
            "tracks": st.tracks.keys().collect::<Vec<_>>(), "track_len": lens(&st.tracks), "tracks_charset_a_z_0_9_underscore": charset(&st.tracks),
        },
        "kspkg": { "records": pkg.entries.len(), "id_is_fnv1a64_of_lowercased_utf16_path": id_lower, "id_matches_only_unlowered_path": id_exact_only },
        "limits": {
            "ismesh_node_max": facts.ismesh_node_max.0, "ismesh_node_max_at": facts.ismesh_node_max.1,
            "ismesh_track_max": facts.ismesh_track_max.0, "ismesh_track_max_at": facts.ismesh_track_max.1,
        },
        "textures": textures,
        "excluded": exclude,
        "seconds": t0.elapsed().as_secs_f64(),
    });
    Ok(st)
}

/// FNV-1a-64 over the UTF-16LE path as written (no lower-casing).
fn raw_id(path: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for u in path.encode_utf16() {
        for c in u.to_le_bytes() {
            h = (h ^ c as u64).wrapping_mul(0x100000001b3);
        }
    }
    h
}

/// Per texture format: count, root field sets, import-settings and source
/// signatures (the most common five of each).
fn texture_table(pkg: &Pkg) -> serde_json::Value {
    type Hist = BTreeMap<String, usize>;
    let mut by: BTreeMap<u64, (usize, Hist, Hist, Hist)> = BTreeMap::new();
    for e in pkg.entries.iter().filter(|e| e.path.to_lowercase().ends_with(".texture")) {
        let Ok(b) = pkg.read(e) else { continue };
        let Ok(h) = Msg::parse(&b) else { continue };
        let fmt = h.varint(4).unwrap_or(0);
        let x = by.entry(fmt).or_default();
        x.0 += 1;
        let mut root: Vec<String> = h.fields.iter().map(|f| match (&f.val, f.num) {
            (Val::Varint(v), 6 | 7 | 8 | 13) => format!("f{}={v}", f.num),
            _ => format!("f{}", f.num),
        }).collect();
        root.dedup();
        *x.1.entry(root.join(" ")).or_default() += 1;
        let sig = |m: Option<Msg>, skip: &[u32]| -> String {
            m.map(|m| m.fields.iter().filter(|f| !skip.contains(&f.num)).map(|f| match &f.val {
                Val::Varint(v) => format!("f{}={v}", f.num),
                Val::F32(b) => format!("f{}={}", f.num, f32::from_le_bytes(*b)),
                _ => format!("f{}", f.num),
            }).collect::<Vec<_>>().join(" ")).unwrap_or_else(|| "-".into())
        };
        *x.2.entry(sig(h.sub(11), &[])).or_default() += 1;
        *x.3.entry(sig(h.sub(10), &[1, 2, 3])).or_default() += 1;
    }
    let top = |h: &Hist| -> Vec<(String, usize)> {
        let mut v: Vec<(String, usize)> = h.iter().map(|(k, n)| (k.clone(), *n)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        v.truncate(5);
        v
    };
    json!(by.iter().map(|(f, (n, r, i, s))| (f.to_string(), json!({"count": n, "root": top(r), "import": top(i), "source": top(s)}))).collect::<serde_json::Map<_, _>>())
}

/// Write `<out>\stockstats.json`.
pub fn write(st: &Stock, out_dir: &Path) -> Result<std::path::PathBuf> {
    std::fs::create_dir_all(out_dir)?;
    let p = out_dir.join("stockstats.json");
    std::fs::write(&p, serde_json::to_string_pretty(st)?).with_context(|| format!("write {}", p.display()))?;
    Ok(p)
}

/// Measure a converted car `.kspkg` or track folder and print every metric
/// outside (or at the edge of) the stock range. Returns the count outside.
pub fn compare(item: &Path, stock_json: &Path) -> Result<usize> {
    let st: Stock = serde_json::from_slice(&std::fs::read(stock_json).with_context(|| format!("read {}", stock_json.display()))?)?;
    let (m, rs, what) = if item.is_dir() {
        let id = item.file_name().map(|f| f.to_string_lossy().to_lowercase()).unwrap_or_default();
        let files = crate::slim::read_dir(item)?;
        let prefix = format!("content\\tracks\\{id}\\");
        let list: Vec<(String, Vec<u8>)> = files.into_iter().map(|(k, v)| (format!("{prefix}{k}"), v)).collect();
        let keep = |p: &str| { let l = p.to_lowercase(); !l.ends_with(".texturemips") || l.contains("\\irradiance_volumes\\iv_bounced_sky") };
        let item: Item = list.iter().map(|(p, d)| (p.as_str(), if keep(p) { Some(d.clone()) } else { None }, d.len())).collect();
        (track_metrics(&id, &item), &st.track_ranges, "track")
    } else {
        let list = crate::io::kspkg::read(item, "")?;
        let item: Item = list.iter().map(|(p, d)| (p.as_str(), if p.to_lowercase().ends_with(".texturemips") { None } else { Some(d.clone()) }, d.len())).collect();
        let mut m = car_metrics(&item);
        m.extend(phys_metrics(&item, None));
        (m, &st.car_ranges, "car")
    };
    let mut outside = 0;
    for (k, v) in &m {
        let Some(r) = rs.get(k) else { continue };
        match fit(r, *v) {
            Fit::Outside => {
                outside += 1;
                println!("OUTSIDE  {k} = {v:.3}  (stock {:.3} .. {:.3}, median {:.3}, n {})", r.min, r.max, r.p50, r.n);
            }
            Fit::Unusual => println!("unusual  {k} = {v:.3}  (stock p10 {:.3} .. p90 {:.3})", r.p10, r.p90),
            Fit::Typical => {}
        }
    }
    let missing: Vec<&String> = rs.iter().filter(|(k, r)| r.n * 10 >= 9 * if what == "car" { st.cars.len() } else { st.tracks.len() } && !m.contains_key(*k)).map(|(k, _)| k).collect();
    for k in missing {
        println!("missing  {k} (present in >= 90 % of stock {what}s)");
    }
    println!("{what}: {} metrics, {outside} outside the stock range", m.len());
    Ok(outside)
}
