//! The lighting bakes (requirements track-vertex-ao, track-irradiance,
//! track-pigment): per-vertex ambient occlusion in the static meshes,
//! irradiance volumes, and the terrain pigment map.
//!
//! They run on the converted output: the written `.mesh` files (world
//! space, identity transforms) and the materials' estimated albedo. That
//! keeps the bakes apart from the mesh stage: they read the meshes back,
//! cast rays against them and rewrite each mesh's colour stream.
//!
//! Occluders (ours): every drawn mesh whose material is opaque (not alpha
//! tested, not blended, not a decal); AC1 tree cards are alpha tested and
//! so are not AO occluders (the vegetation stage adds canopies through
//! [`Inputs::extra_occluders`]). For the probes alone, alpha-tested meshes
//! (tree and bush cards, mesh fences) are a second, partial occluder set
//! ([`Tile::foliage`]): stock probes are darkest under tree canopies.
//!
//! Huge maps (ours): the bakes run per square tile of [`TILE`] metres, the
//! ray tree holding the tile plus a [`MARGIN`] wide border, so memory
//! follows the densest tile rather than the whole map. A track whose
//! drawn geometry fits one tile (every stock-sized circuit) is one tile.
//!
//! Everything is deterministic: sample directions come from fixed point
//! sets rotated by a hash of the sample position, never from a random
//! generator, and the work is split so thread scheduling cannot change a
//! value.

use anyhow::{Context, Result};
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::bvh::Bvh;
use crate::io::pb::{self, Msg, Val};

/// Tile edge for huge maps (ours).
pub const TILE: f32 = 1024.0;
/// Border loaded around a tile: the longest ray (probe sky rays, 250 m)
/// plus half a probe volume (ours).
pub const MARGIN: f32 = 300.0;
/// Below this many occluder triangles the whole track is one tile.
pub const ONE_TILE_TRIS: usize = 6_000_000;

/// Payload bit: the triangle belongs to drawn ground (a drawn physics
/// surface: road, kerb, grass, sand, terrain).
pub const GROUND_BIT: u32 = 1 << 31;
const PALETTE_MASK: u32 = 0xffff;

#[derive(Clone, Copy, Debug)]
pub struct Switches {
    pub ao: bool,
    pub probes: bool,
    pub pigment: bool,
}

impl Default for Switches {
    fn default() -> Self {
        Switches { ao: true, probes: true, pigment: true }
    }
}

impl Switches {
    /// `ACECONV_BAKES` = comma list of `ao`, `probes`, `pigment` (or
    /// `none`); unset = all on.
    pub fn from_env() -> Switches {
        match std::env::var("ACECONV_BAKES") {
            Ok(v) => {
                let v = v.to_lowercase();
                let has = |k: &str| v.split(',').any(|x| x.trim() == k || x.trim() == "all");
                Switches { ao: has("ao"), probes: has("probes"), pigment: has("pigment") }
            }
            Err(_) => Switches::default(),
        }
    }
}

/// What the bakes know about one written material.
#[derive(Clone, Copy, Debug)]
pub struct MatInfo {
    /// estimated mean linear albedo
    pub albedo: [f32; 3],
    /// opaque: occludes rays
    pub opaque: bool,
    /// alpha-tested (AC1 tree / bush cards, mesh fences): a partial
    /// occluder for the probe rays only
    pub foliage: bool,
    /// emissive / blinking / start-light variants keep the neutral AO
    pub emissive: bool,
    /// the EVO shader (report statistics)
    pub shader: &'static str,
}

/// The shader names the report groups by.
pub fn shader_tag(s: &str) -> &'static str {
    match s {
        "DynamicTrack" => "DynamicTrack",
        "DynamicTrackMultilayer" => "DynamicTrackMultilayer",
        "DefaultMaterial" => "DefaultMaterial",
        "Particle" => "Particle",
        "Semaphore" => "Semaphore",
        _ => "other",
    }
}

/// One written mesh.
#[derive(Clone, Debug)]
pub struct MeshRef {
    /// the `.mesh` file
    pub file: PathBuf,
    pub lo: [f32; 3],
    pub hi: [f32; 3],
    /// drawn (not collision-only)
    pub drawn: bool,
    /// a drawn physics surface (road, grass, terrain...)
    pub ground: bool,
    /// drivable (track surfaces) - the area probes and the pigment cover
    pub drivable: bool,
}

pub struct Inputs<'a> {
    pub out: &'a Path,
    pub track: &'a str,
    pub meshes: Vec<MeshRef>,
    /// material archive path (lower case) -> info
    pub materials: HashMap<String, MatInfo>,
    /// world xz box of the drivable area (union of the layouts)
    pub drivable: ([f32; 2], [f32; 2]),
    /// extra occluder triangles (vegetation canopies), with their albedo
    pub extra_occluders: Vec<([[f32; 3]; 3], [f32; 3])>,
    /// extra partial occluders for the probes (vegetation crowns, passed
    /// like alpha-tested cards), with their albedo
    pub extra_foliage: Vec<([[f32; 3]; 3], [f32; 3])>,
}

/// LOD0 geometry of a mesh file.
pub struct Geo {
    pub pos: Vec<[f32; 3]>,
    pub nrm: Vec<[f32; 3]>,
    pub idx: Vec<u32>,
    pub material: String,
}

fn f32s(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

fn v3s(b: Option<&[u8]>) -> Vec<[f32; 3]> {
    b.map(|b| f32s(b).chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect()).unwrap_or_default()
}

/// Read LOD0 of a mesh file.
pub fn read_geo(file: &Path) -> Result<Geo> {
    let b = std::fs::read(file).with_context(|| format!("read {}", file.display()))?;
    let top = Msg::parse(&b)?;
    let l0 = top.sub(5).unwrap_or_default();
    let material = l0.sub(4).and_then(|s| s.str(4)).unwrap_or_default();
    let idx: Vec<u32> = l0.bytes(11).map(pb::varints).unwrap_or_default().into_iter().map(|v| v as u32).collect();
    Ok(Geo { pos: v3s(l0.bytes(5)), nrm: v3s(l0.bytes(6)), idx, material })
}

fn boxes_meet(alo: [f32; 2], ahi: [f32; 2], blo: [f32; 2], bhi: [f32; 2]) -> bool {
    alo[0] <= bhi[0] && blo[0] <= ahi[0] && alo[1] <= bhi[1] && blo[1] <= ahi[1]
}

/// One tile of work: its core rectangle (half open) and the ray tree of
/// everything within MARGIN of it.
pub struct Tile {
    pub lo: [f32; 2],
    pub hi: [f32; 2],
    pub bvh: Bvh,
    /// alpha-tested geometry (partial occluders for the probes)
    pub foliage: Bvh,
    /// palette index -> albedo
    pub palette: Vec<[f32; 3]>,
    /// highest geometry in the tile (downward rays start above it)
    pub top: f32,
}

impl Tile {
    pub fn contains(&self, x: f32, z: f32) -> bool {
        x >= self.lo[0] && x < self.hi[0] && z >= self.lo[1] && z < self.hi[1]
    }
    pub fn albedo(&self, payload: u32) -> [f32; 3] {
        self.palette.get((payload & PALETTE_MASK) as usize).copied().unwrap_or([0.2; 3])
    }
}

/// Every bake's result, for the report.
#[derive(Default)]
pub struct Report {
    pub lines: Vec<String>,
}

/// Run the bakes that `sw` turns on. Probes and the pigment block are
/// added to `scene`; meshes are rewritten in place.
pub fn run(inp: &Inputs, sw: Switches, scene: &mut super::scene::SceneBuilder, graphics: &mut Msg) -> Result<Report> {
    let mut rep = Report::default();
    if !(sw.ao || sw.probes || sw.pigment) {
        rep.lines.push("bakes: off".into());
        return Ok(rep);
    }
    let t0 = std::time::Instant::now();
    // ---- palette of material albedos
    let mut palette: Vec<[f32; 3]> = Vec::new();
    let mut pal_of: HashMap<String, (u32, MatInfo)> = HashMap::new();
    let mut keys: Vec<&String> = inp.materials.keys().collect();
    keys.sort();
    for k in keys {
        let m = inp.materials[k];
        pal_of.insert(k.clone(), (palette.len().min(PALETTE_MASK as usize) as u32, m));
        palette.push(m.albedo);
    }
    // extra occluders share one palette entry per albedo
    let mut by_albedo: HashMap<[u32; 3], u32> = HashMap::new();
    let mut entry = |a: [f32; 3], palette: &mut Vec<[f32; 3]>| -> u32 {
        *by_albedo.entry(a.map(f32::to_bits)).or_insert_with(|| {
            palette.push(a);
            (palette.len() - 1).min(PALETTE_MASK as usize) as u32
        })
    };
    let extra_pal: Vec<u32> = inp.extra_occluders.iter().map(|(_, a)| entry(*a, &mut palette)).collect();
    let foliage_pal: Vec<u32> = inp.extra_foliage.iter().map(|(_, a)| entry(*a, &mut palette)).collect();
    // ---- every drawn mesh's material (and triangle count)
    let read: Vec<(Option<(u32, MatInfo)>, usize)> = inp.meshes.par_iter().map(|m| {
        if !m.drawn {
            return (None, 0);
        }
        let Ok(b) = std::fs::read(&m.file) else { return (None, 0) };
        let Ok(top) = Msg::parse(&b) else { return (None, 0) };
        let l0 = top.sub(5).unwrap_or_default();
        let path = l0.sub(4).and_then(|s| s.str(4)).unwrap_or_default().to_lowercase();
        let tris = l0.bytes(11).map(|i| pb::varints(i).len() / 3).unwrap_or(0);
        (Some(pal_of.get(&path).copied().unwrap_or((u32::MAX, MatInfo { albedo: [0.2; 3], opaque: true, foliage: false, emissive: false, shader: "other" }))), tris)
    }).collect();
    let mats: Vec<Option<(u32, MatInfo)>> = read.iter().map(|x| x.0).collect();
    // ---- tiles
    let occ: Vec<usize> = (0..inp.meshes.len()).filter(|&i| mats[i].map(|x| x.1.opaque).unwrap_or(false)).collect();
    let fol: Vec<usize> = if sw.probes { (0..inp.meshes.len()).filter(|&i| mats[i].map(|x| !x.1.opaque && x.1.foliage).unwrap_or(false)).collect() } else { Vec::new() };
    let (mut glo, mut ghi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
    let mut ytop = f32::NEG_INFINITY;
    for m in inp.meshes.iter().filter(|m| m.drawn) {
        glo = [glo[0].min(m.lo[0]), glo[1].min(m.lo[2])];
        ghi = [ghi[0].max(m.hi[0]), ghi[1].max(m.hi[2])];
        ytop = ytop.max(m.hi[1]);
    }
    if !glo[0].is_finite() {
        rep.lines.push("bakes: no drawn meshes".into());
        return Ok(rep);
    }
    let tri_count: usize = occ.iter().chain(&fol).map(|&i| read[i].1).sum::<usize>() + inp.extra_occluders.len() + inp.extra_foliage.len();
    let tiles: Vec<([f32; 2], [f32; 2])> = if tri_count <= ONE_TILE_TRIS {
        vec![([glo[0] - 1.0, glo[1] - 1.0], [ghi[0] + 1.0, ghi[1] + 1.0])]
    } else {
        let nx = ((ghi[0] - glo[0]) / TILE).ceil().max(1.0) as usize;
        let nz = ((ghi[1] - glo[1]) / TILE).ceil().max(1.0) as usize;
        let mut v = Vec::new();
        for j in 0..nz {
            for i in 0..nx {
                let lo = [glo[0] + i as f32 * TILE, glo[1] + j as f32 * TILE];
                let hi = [if i + 1 == nx { ghi[0] + 1.0 } else { lo[0] + TILE }, if j + 1 == nz { ghi[1] + 1.0 } else { lo[1] + TILE }];
                v.push((lo, hi));
            }
        }
        v
    };
    rep.lines.push(format!("  bakes: {} occluder + {} foliage meshes, {} + {} canopy triangles, {} triangles, {} tile(s)", occ.len(), fol.len(), inp.extra_occluders.len(), inp.extra_foliage.len(), tri_count, tiles.len()));

    // ---- plans that do not need rays
    let drawn_tris: usize = read.iter().map(|x| x.1).sum();
    let mut ao = if sw.ao { Some(super::ao::Ao::new(inp.meshes.len(), drawn_tris)) } else { None };
    let mut probes = if sw.probes { super::irradiance::plan(inp, glo, ghi) } else { None };
    let mut pig = if sw.pigment { super::pigment::plan(inp, glo, ghi) } else { None };
    let mut tree_bytes = 0usize;
    let mut tree_tris = 0usize;
    let single = tiles.len() == 1;
    for (ti, (lo, hi)) in tiles.iter().enumerate() {
        // a tile no drawn mesh reaches has nothing to bake
        if !single && !inp.meshes.iter().any(|m| m.drawn && boxes_meet([m.lo[0], m.lo[2]], [m.hi[0], m.hi[2]], *lo, *hi)) {
            continue;
        }
        let elo = [lo[0] - MARGIN, lo[1] - MARGIN];
        let ehi = [hi[0] + MARGIN, hi[1] + MARGIN];
        // occluders of this tile (+ border)
        let gather = |list: &[usize]| -> (Vec<[[f32; 3]; 3]>, Vec<u32>) {
            let parts: Vec<(Vec<[[f32; 3]; 3]>, Vec<u32>)> = list.par_iter().filter(|&&i| {
                let m = &inp.meshes[i];
                single || boxes_meet([m.lo[0], m.lo[2]], [m.hi[0], m.hi[2]], elo, ehi)
            }).map(|&i| {
                let m = &inp.meshes[i];
                let Ok(g) = read_geo(&m.file) else { return (Vec::new(), Vec::new()) };
                let pal = mats[i].map(|x| x.0).unwrap_or(u32::MAX);
                let pay = (if pal == u32::MAX { PALETTE_MASK } else { pal }) | if m.ground { GROUND_BIT } else { 0 };
                let mut t = Vec::with_capacity(g.idx.len() / 3);
                for c in g.idx.chunks_exact(3) {
                    let (Some(a), Some(b), Some(d)) = (g.pos.get(c[0] as usize), g.pos.get(c[1] as usize), g.pos.get(c[2] as usize)) else { continue };
                    if !single {
                        let cx = (a[0] + b[0] + d[0]) / 3.0;
                        let cz = (a[2] + b[2] + d[2]) / 3.0;
                        if cx < elo[0] || cx > ehi[0] || cz < elo[1] || cz > ehi[1] {
                            continue;
                        }
                    }
                    t.push([*a, *b, *d]);
                }
                let n = t.len();
                (t, vec![pay; n])
            }).collect();
            let mut tris: Vec<[[f32; 3]; 3]> = Vec::new();
            let mut pay: Vec<u32> = Vec::new();
            for (t, p) in parts {
                tris.extend(t);
                pay.extend(p);
            }
            (tris, pay)
        };
        let (mut tris, mut pay) = gather(&occ);
        for (k, (t, _)) in inp.extra_occluders.iter().enumerate() {
            let c = [(t[0][0] + t[1][0] + t[2][0]) / 3.0, (t[0][2] + t[1][2] + t[2][2]) / 3.0];
            if single || (c[0] >= elo[0] && c[0] <= ehi[0] && c[1] >= elo[1] && c[1] <= ehi[1]) {
                tris.push(*t);
                pay.push(extra_pal[k]);
            }
        }
        let bvh = Bvh::build(tris, pay);
        let (mut ft, mut fp) = gather(&fol);
        if sw.probes {
            for (k, (t, _)) in inp.extra_foliage.iter().enumerate() {
                let c = [(t[0][0] + t[1][0] + t[2][0]) / 3.0, (t[0][2] + t[1][2] + t[2][2]) / 3.0];
                if single || (c[0] >= elo[0] && c[0] <= ehi[0] && c[1] >= elo[1] && c[1] <= ehi[1]) {
                    ft.push(*t);
                    fp.push(foliage_pal[k]);
                }
            }
        }
        let foliage = Bvh::build(ft, fp);
        let tile = Tile { lo: *lo, hi: *hi, bvh, foliage, palette: palette.clone(), top: ytop + 10.0 };
        tree_bytes = tree_bytes.max(tile.bvh.bytes() + tile.foliage.bytes());
        tree_tris = tree_tris.max(tile.bvh.len() + tile.foliage.len());
        if let Some(a) = ao.as_mut() {
            a.bake_tile(inp, &mats, &tile)?;
        }
        if let Some(p) = probes.as_mut() {
            p.bake_tile(&tile);
        }
        if let Some(p) = pig.as_mut() {
            p.bake_tile(&tile);
        }
        if tiles.len() > 1 {
            eprintln!("  bakes: tile {}/{} ({} triangles) {:.0} s", ti + 1, tiles.len(), tile.bvh.len(), t0.elapsed().as_secs_f64());
        }
    }
    rep.lines.push(format!("  bakes: largest ray tree {} triangles, {:.0} MB", tree_tris, tree_bytes as f64 / 1048576.0));
    if let Some(a) = ao {
        rep.lines.extend(a.finish(inp, &mats)?);
    }
    if let Some(p) = probes {
        rep.lines.extend(p.finish(inp, scene)?);
    }
    if let Some(p) = pig {
        rep.lines.extend(p.finish(inp, graphics)?);
    }
    rep.lines.push(format!("  bakes: {:.1} s", t0.elapsed().as_secs_f64()));
    Ok(rep)
}

/// A deterministic hash of a position (sample rotations).
pub fn hash3(p: [f32; 3]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for v in p {
        let q = (v * 256.0).round() as i64;
        for b in q.to_le_bytes() {
            h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3);
        }
    }
    // final avalanche (splitmix)
    h ^= h >> 30;
    h = h.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94d0_49bb_1331_11eb);
    h ^ (h >> 31)
}

/// Uniform [0, 1) from a hash.
pub fn unit_of(h: u64) -> f32 {
    ((h >> 40) as f32) / (1u64 << 24) as f32
}

pub fn put_f32s(m: &mut Msg, f: u32, v: &[f32]) {
    m.set(f, Val::Bytes(pb::f32_bytes(v)));
}
