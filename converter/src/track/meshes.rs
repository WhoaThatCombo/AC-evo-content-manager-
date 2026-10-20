//! Static meshes and collision meshes (the track-base version: correct
//! geometry, collision and lighting invariants; the track-look stage
//! refines grouping and materials).
//!
//! `.mesh` layout (ACECM `protos\Mesh.proto.txt`, stock track meshes):
//! MeshData f2 isVisible, f3 isRenderable, f5 LOD bodies, f6 / f7 bounds,
//! f8 castFarShadow, f15 irradianceVolumeGroundLevel (stock road meshes),
//! f20 staticLod (empty on stock collision meshes), f100 useFullPrecisionUvs;
//! LOD body f1 castShadows, f4 batches {f1 name, f2 start, f3 count, f4
//! material}, f5 positions, f6 normals, f7 uv0, f8 tangents (xyz + w =
//! +-1), f11 indices (packed), f13 colours (4 per vertex), f14..f16 uv1..3,
//! f17 / f18 LOD bounds. Every stock LOD body carries f13..f16. f13 holds
//! values of at most 1/255; its fourth component is the vertex AO as
//! ao / 255 (track::ao), so 1/255 is its neutral value; the others are 0
//! unless a stage uses them (start-light lamp classes).
//! Stock collision-only meshes use the material `editor/default.material`.
//!
//! What this stage guarantees (requirements track-meshes, game evidence):
//! - every drawn, active AC1 mesh is drawn; every physics mesh collides,
//!   invisible ones too (SMesh physics_only); `0`-prefixed and non-physics
//!   meshes do not collide;
//! - AC1 far-LOD copies (lodIn > 0) are not drawn over their near copy;
//! - world-space vertices with identity entity transforms;
//! - normals finite and unit length: an invalid AC1 normal takes the
//!   normal of the largest triangle using the vertex;
//! - tangents orthogonal to the normal, w = +-1 from the UV orientation;
//! - UVs rebased by whole tiles when an instance sits far from the origin
//!   (mean |uv| > 16), and full-precision UVs flagged past 32;
//! - AC1 back-to-back sheets (coincident triangles of opposite winding,
//!   5 mm grid, any materials, real face normals opposite - a sliver or a
//!   millimetre triangle can flip its winding on the grid without being a
//!   sheet): same material and UVs -> one triangle on a two-sided clone of
//!   the material; otherwise both kept, each pushed about 1 cm along its
//!   own normal, at the first of 10 / 7 / 13 / 4 mm where it lands on no
//!   other triangle (AC1 rails carry a second layer exactly 1 cm away).
//!   Stock tracks have none; they flicker in EVO's shadows;
//! - blended / depth-read-only AC1 materials (decals) lifted 1.5 cm along
//!   their normals so they do not z-fight the surface below;
//! - bounds (mesh and LOD) hold every vertex; the SMesh entity box holds
//!   its mesh;
//! - shadows: AC1 castShadows kept; meshes 6 m or taller also cast in the
//!   far cascade (ours; stock flags about a quarter of its 4 m+ meshes);
//!   giant flat backdrops (thinner than 0.5 m, wider than 100 m, taller
//!   than 15 m) never cast.
//!
//! Grouping (ours): instances are merged by (role, surface, material,
//! two-sided, cast shadows, layouts) inside 256 m tiles; an instance larger
//! than a tile is cut by triangle centroid into at most 8 x 8 tiles (the
//! tile doubles for huge terrain). A mesh holds at most 50k
//! triangles (collision-only meshes 8k; stock maxima 53.7k / 6.7k); bigger
//! groups are split in order.

use anyhow::Result;
use rustc_hash::FxHashMap;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use super::classes::{surface_kind, Kind};
use super::scan::{MeshInst, Role, Scan};
use crate::io::pb::{self, Msg, Val};

pub const TILE: f32 = 256.0;
/// per-mesh triangle caps, calibrated on stock: drawn meshes at most 53.7k
/// (p99 19.9k), measured on 1,200 stock track meshes
const MAX_TRIS: usize = 50_000;
/// Collision-only meshes: the stock p99 rounded down to 100. `aceconv
/// measure phys` (stockstats/measure_phys.txt): 727 collision-only stock
/// meshes (`static_mesh/physics/`, 8 of 17 circuits have them; road_atlanta
/// excluded), p50 96, p90 3550, p99 5652, max 6907 triangles (spa).
const MAX_PHYS_TRIS: usize = 5_600;
pub const FAR_SHADOW_HEIGHT: f32 = 6.0;
/// LOD chains (ours, calibrated on stock: about 3 % of stock track meshes
/// carry 2..4 levels, level 1 from 50..200 m (median 200) at 12..50 % of the
/// triangles): drawn decor meshes of LOD_MIN_TRIS or more get these
/// (triangle ratio, lodIn metres) levels, meshoptimizer-simplified.
pub const LOD_MIN_TRIS: usize = 8_000;
pub const LODS: [(f32, f32); 2] = [(0.25, 200.0), (0.08, 500.0)];
/// Push distances for back-to-back sheets with different UVs, tried in order
/// until the pushed triangle lands on no other triangle (an AC1 model may
/// carry a second layer exactly 1 cm away: Imola's `GR_HI` / `gr3_LOW`
/// rails). Ours, about 1 cm as the requirement asks.
const PUSHES: [f32; 4] = [0.01, 0.007, 0.013, 0.004];
const DECAL_LIFT: f32 = 0.015;
pub const COLLIDER_MATERIAL: &str = "editor/default.material";
/// The colour stream's AO component with no occlusion (stock: AO / 255).
pub const NEUTRAL_AO: f32 = 1.0 / 255.0;

/// What a triangle of an instance became.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Fate {
    Keep,
    Drop,
    TwoSided,
    /// pushed along its face normal by `PUSHES[i]`
    Push(u8),
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum GroupRole {
    /// drawn only
    Visual,
    /// drawn and colliding with surface
    Drawn(usize),
    /// collision only
    Collider(usize),
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
struct GroupKey {
    role: GroupRole,
    material: Option<usize>,
    two_sided: bool,
    /// material variant (look.rs: CSP emissive / blink, start lights); 0 = none
    variant: u16,
    cast: bool,
    layouts: u64,
    /// (scale level, x, z): tiles of TILE * 2^level metres
    tile: (u8, i32, i32),
}

/// The plan: which triangle goes where.
pub struct Plan {
    groups: BTreeMap<GroupKey, Vec<(usize, Vec<u32>)>>,
    fates: HashMap<usize, Vec<Fate>>,
    /// (global material, two-sided, variant) the drawn meshes need
    pub used_materials: HashSet<(usize, bool, u16)>,
    pub far_lod_dropped: usize,
    pub back_to_back_merged: usize,
    pub back_to_back_pushed: usize,
    /// pushes that had to take a shorter / longer distance than 1 cm
    pub push_retries: usize,
    /// drawn slivers (under 1 mm wide, two vertices in one 5 mm cell) dropped
    pub slivers: usize,
    pub backdrops: usize,
}

fn quant(p: [f32; 3]) -> (i64, i64, i64) {
    ((p[0] * 200.0).round() as i64, (p[1] * 200.0).round() as i64, (p[2] * 200.0).round() as i64)
}

fn tri_key(a: (i64, i64, i64), b: (i64, i64, i64), c: (i64, i64, i64)) -> Option<(u64, bool)> {
    if a == b || b == c || a == c {
        return None;
    }
    let fwd = [[a, b, c], [b, c, a], [c, a, b]].into_iter().min().unwrap();
    let rev = [[a, c, b], [c, b, a], [b, a, c]].into_iter().min().unwrap();
    let (k, w) = if fwd <= rev { (fwd, true) } else { (rev, false) };
    // the vertex set (both windings) as one hash
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in k {
        for v in [p.0, p.1, p.2] {
            for b in v.to_le_bytes() {
                h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3);
            }
        }
    }
    Some((h, w))
}

/// Unit face normal of a triangle (None when degenerate).
fn face_normal(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> Option<[f32; 3]> {
    unit(cross(sub(b, a), sub(c, a)))
}

fn bbox(p: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for v in p {
        for k in 0..3 {
            lo[k] = lo[k].min(v[k]);
            hi[k] = hi[k].max(v[k]);
        }
    }
    (lo, hi)
}

fn tile_of(x: f32, z: f32, level: u8) -> (u8, i32, i32) {
    let t = TILE * (1u32 << level) as f32;
    (level, (x / t).floor() as i32, (z / t).floor() as i32)
}

fn is_backdrop(lo: [f32; 3], hi: [f32; 3]) -> bool {
    let (ex, ey, ez) = (hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]);
    ex.min(ez) < 0.5 && ex.max(ez) > 100.0 && ey > 15.0
}

pub fn drawn_role(m: &MeshInst) -> bool {
    !matches!(m.role, Role::Physics { drawn: false, .. })
}

/// Positions, triangles and UVs of a few instances at a time.
struct InstCache<'a> {
    scan: &'a Scan<'a>,
    map: HashMap<usize, (Vec<[f32; 3]>, Vec<[u32; 3]>, Vec<[f32; 2]>)>,
}

impl<'a> InstCache<'a> {
    fn get(&mut self, mi: usize) -> &(Vec<[f32; 3]>, Vec<[u32; 3]>, Vec<[f32; 2]>) {
        if self.map.len() > 64 && !self.map.contains_key(&mi) {
            self.map.clear();
        }
        let scan = self.scan;
        self.map.entry(mi).or_insert_with(|| {
            let m = &scan.meshes[mi];
            let mesh = scan.mesh(m);
            (scan.positions(m), scan.triangles(m), (0..mesh.vertex_count).map(|v| mesh.uv(v)).collect())
        })
    }
    fn tri(&mut self, mi: usize, ti: usize) -> [[f32; 3]; 3] {
        let (p, t, _) = self.get(mi);
        let t = t[ti];
        [p[t[0] as usize], p[t[1] as usize], p[t[2] as usize]]
    }
}

/// Decide the fate and group of every triangle. `variants[m]` is the
/// material variant of scan mesh m (look.rs; empty = none).
pub fn plan(scan: &Scan, variants: &[u16]) -> Plan {
    plan_except(scan, variants, &[])
}

/// `plan`, leaving out the scan meshes `drop` marks (AC1 tree cards the
/// vegetation stage replaced; only visual meshes are ever dropped).
pub fn plan_except(scan: &Scan, variants: &[u16], dropped: &[bool]) -> Plan {
    let mut far_lod_dropped = 0;
    let skip: Vec<bool> = scan.meshes.iter().enumerate().map(|(i, m)| {
        if dropped.get(i).copied().unwrap_or(false) && matches!(m.role, Role::Visual) {
            return true;
        }
        let far = scan.mesh(m).lod_in > 0.0 && drawn_role(m) && !matches!(m.role, Role::Physics { .. });
        if far {
            far_lod_dropped += 1;
        }
        far
    }).collect();
    // ---- coincident triangles: every triangle keyed by its vertex set (5 mm
    // grid, both windings one key); drawn and collision-only triangles apart
    let mut keys: Vec<(u64, u32, u32, bool)> = Vec::new();
    let mut slivers: Vec<(usize, usize)> = Vec::new();
    let mut ntris: Vec<usize> = vec![0; scan.meshes.len()];
    let salt_of = |m: &MeshInst| if drawn_role(m) { 0 } else { 0x9e37_79b9_7f4a_7c15u64 };
    for (mi, m) in scan.meshes.iter().enumerate() {
        if skip[mi] {
            continue;
        }
        let salt = salt_of(m);
        let wp = scan.positions(m);
        let p: Vec<(i64, i64, i64)> = wp.iter().copied().map(quant).collect();
        let tris = scan.triangles(m);
        ntris[mi] = tris.len();
        for (ti, t) in tris.iter().enumerate() {
            // a triangle with two vertices in one 5 mm cell is keyed on a
            // 0.5 mm grid instead (a sliver sheet still pairs with its back)
            let fine = || {
                let q = |i: u32| { let v = wp[i as usize]; ((v[0] * 2000.0).round() as i64, (v[1] * 2000.0).round() as i64, (v[2] * 2000.0).round() as i64) };
                tri_key(q(t[0]), q(t[1]), q(t[2])).map(|(k, w)| (k ^ 0x5bd1_e995_0000_0001, w))
            };
            match tri_key(p[t[0] as usize], p[t[1] as usize], p[t[2] as usize]).or_else(fine) {
                Some((k, w)) => keys.push((k ^ salt, mi as u32, ti as u32, w)),
                None => {
                    // a sliver (two vertices in one 5 mm cell, under 1 mm
                    // wide): nothing to see, but a decal lift along two
                    // different vertex normals opens it into a flickering pair
                    let (a, b, c) = (wp[t[0] as usize], wp[t[1] as usize], wp[t[2] as usize]);
                    let longest = len(sub(b, a)).max(len(sub(c, b))).max(len(sub(a, c))).max(1e-9);
                    if len(cross(sub(b, a), sub(c, a))) / longest < 0.001 && drawn_role(m) {
                        slivers.push((mi, ti));
                    }
                }
            }
        }
    }
    keys.sort_unstable();
    let taken = |k: u64| keys.binary_search_by_key(&k, |x| x.0).is_ok();
    // per run of one key with both windings whose real faces point opposite
    // ways (a 5 mm grid can flip the winding of a sliver or a millimetre
    // triangle that is not a sheet at all): drawn, one material, same UVs ->
    // keep one triangle on a two-sided material, drop the rest; otherwise
    // push every copy about 1 cm along its own normal (opposite windings
    // part), at a distance where it lands on no other triangle
    let mut fates: HashMap<usize, Vec<Fate>> = HashMap::new();
    let (mut merged, mut pushed, mut retries) = (0usize, 0usize, 0usize);
    let mut cache = InstCache { scan, map: HashMap::new() };
    let mut i = 0;
    while i < keys.len() {
        let mut j = i + 1;
        while j < keys.len() && keys[j].0 == keys[i].0 {
            j += 1;
        }
        let run = &keys[i..j];
        i = j;
        if !(run.iter().any(|x| x.3) && run.iter().any(|x| !x.3)) {
            continue;
        }
        let normals: Vec<Option<[f32; 3]>> = run.iter().map(|x| {
            let t = cache.tri(x.1 as usize, x.2 as usize);
            face_normal(t[0], t[1], t[2])
        }).collect();
        let Some(n0) = normals[0] else { continue };
        if !normals.iter().any(|n| n.map(|n| dot(n, n0) < -0.5).unwrap_or(false)) {
            continue;
        }
        let first = run[0];
        let drawn = drawn_role(&scan.meshes[first.1 as usize]);
        let mat = scan.meshes[first.1 as usize].material;
        let var = |mi: u32| variants.get(mi as usize).copied().unwrap_or(0);
        let one_mat = run.iter().all(|x| scan.meshes[x.1 as usize].material == mat && var(x.1) == var(first.1));
        let mut same_uv = drawn && one_mat;
        if same_uv {
            let (pa, ta, ua) = cache.get(first.1 as usize).clone();
            let qa: Vec<(i64, i64, i64)> = ta[first.2 as usize].iter().map(|&v| quant(pa[v as usize])).collect();
            for x in &run[1..] {
                let (pb, tb, ub) = cache.get(x.1 as usize);
                let ok = ta[first.2 as usize].iter().zip(&qa).all(|(&va, qva)| {
                    tb[x.2 as usize].iter().find(|&&vb| quant(pb[vb as usize]) == *qva)
                        .map(|&vb| (ua[va as usize][0] - ub[vb as usize][0]).abs() < 1e-4 && (ua[va as usize][1] - ub[vb as usize][1]).abs() < 1e-4)
                        .unwrap_or(false)
                });
                if !ok {
                    same_uv = false;
                    break;
                }
            }
        }
        for (k, x) in run.iter().enumerate() {
            let mi = x.1 as usize;
            let f = match (same_uv, k) {
                (true, 0) => Fate::TwoSided,
                (true, _) => Fate::Drop,
                _ => {
                    // the first distance whose pushed copy hits no triangle
                    let t = cache.tri(mi, x.2 as usize);
                    let n = normals[k].unwrap_or(n0);
                    let salt = salt_of(&scan.meshes[mi]);
                    let pick = (0..PUSHES.len()).find(|&d| {
                        let q = t.map(|p| quant([p[0] + n[0] * PUSHES[d], p[1] + n[1] * PUSHES[d], p[2] + n[2] * PUSHES[d]]));
                        tri_key(q[0], q[1], q[2]).map(|(kk, _)| !taken(kk ^ salt)).unwrap_or(true)
                    }).unwrap_or(0);
                    if pick > 0 {
                        retries += 1;
                    }
                    Fate::Push(pick as u8)
                }
            };
            fates.entry(mi).or_insert_with(|| vec![Fate::Keep; ntris[mi]])[x.2 as usize] = f;
        }
        if same_uv { merged += 1 } else { pushed += 1 }
    }
    for &(mi, ti) in &slivers {
        fates.entry(mi).or_insert_with(|| vec![Fate::Keep; ntris[mi]])[ti] = Fate::Drop;
    }
    drop(cache);
    drop(keys);
    // ---- groups
    let mut groups: BTreeMap<GroupKey, Vec<(usize, Vec<u32>)>> = BTreeMap::new();
    let mut used = HashSet::new();
    let mut backdrops = 0;
    for (mi, m) in scan.meshes.iter().enumerate() {
        if skip[mi] {
            continue;
        }
        let mesh = scan.mesh(m);
        let pos = scan.positions(m);
        let tris = scan.triangles(m);
        let (lo, hi) = bbox(&pos);
        let backdrop = is_backdrop(lo, hi);
        if backdrop {
            backdrops += 1;
        }
        let role = match m.role {
            Role::Visual => GroupRole::Visual,
            Role::Physics { surface, drawn: true } => GroupRole::Drawn(surface),
            Role::Physics { surface, drawn: false } => GroupRole::Collider(surface),
        };
        let cast = mesh.cast_shadows && !backdrop;
        let extent = (hi[0] - lo[0]).max(hi[2] - lo[2]);
        let big = extent > TILE * 1.5;
        // a huge instance (terrain, an endless floor) is cut into at most
        // 8 x 8 tiles: the tile doubles until it fits
        let mut level = 0u8;
        while big && level < 16 && extent / (TILE * (1u32 << level) as f32) > 8.0 {
            level += 1;
        }
        let centre = tile_of((lo[0] + hi[0]) / 2.0, (lo[2] + hi[2]) / 2.0, 0);
        let fate = fates.get(&mi);
        let collider = matches!(role, GroupRole::Collider(_));
        let variant = if collider { 0 } else { variants.get(mi).copied().unwrap_or(0) };
        for (ti, t) in tris.iter().enumerate() {
            let f = fate.map(|v| v[ti]).unwrap_or(Fate::Keep);
            if f == Fate::Drop {
                continue;
            }
            let tile = if big {
                let c = [0, 1, 2].map(|k| (pos[t[0] as usize][k] + pos[t[1] as usize][k] + pos[t[2] as usize][k]) / 3.0);
                tile_of(c[0], c[2], level)
            } else {
                centre
            };
            let two = f == Fate::TwoSided;
            let material = if collider { None } else { Some(m.material) };
            if let Some(g) = material {
                used.insert((g, two, variant));
            }
            let key = GroupKey { role, material, two_sided: two, variant, cast, layouts: m.layouts, tile };
            let list = groups.entry(key).or_default();
            match list.last_mut() {
                Some((i, v)) if *i == mi => v.push(ti as u32),
                _ => list.push((mi, vec![ti as u32])),
            }
        }
    }
    Plan { groups, fates, used_materials: used, far_lod_dropped, back_to_back_merged: merged, back_to_back_pushed: pushed, push_retries: retries, backdrops, slivers: slivers.len() }
}

/// Does a triangle list hold a back-to-back pair (the validator's rule: 5 mm
/// grid, opposite windings, real faces opposite)? A simplified LOD level can
/// fold a pushed sheet back onto itself.
fn has_back_to_back(pos: &[f32], idx: &[u32]) -> bool {
    let p = |i: u32| { let i = i as usize * 3; [pos[i], pos[i + 1], pos[i + 2]] };
    let mut seen: FxHashMap<u64, (bool, [f32; 3])> = FxHashMap::default();
    for t in idx.chunks_exact(3) {
        let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
        let Some((k, w)) = tri_key(quant(a), quant(b), quant(c)) else { continue };
        let Some(n) = face_normal(a, b, c) else { continue };
        match seen.get(&k) {
            Some(&(w0, n0)) if w0 != w && dot(n0, n) < -0.5 => return true,
            Some(_) => {}
            None => {
                seen.insert(k, (w, n));
            }
        }
    }
    false
}

/// World-space vertex data of one instance, fixed up.
struct InstData {
    pos: Vec<[f32; 3]>,
    nrm: Vec<[f32; 3]>,
    uv: Vec<[f32; 2]>,
    tan: Vec<[f32; 4]>,
    tris: Vec<[u32; 3]>,
    fixed_normals: usize,
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn len(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}
fn unit(a: [f32; 3]) -> Option<[f32; 3]> {
    let l = len(a);
    (l.is_finite() && l > 1e-20).then(|| [a[0] / l, a[1] / l, a[2] / l])
}

/// 3x3 inverse-transpose of a row-vector matrix (for normals).
fn normal_matrix(m: &[f32; 16]) -> [[f32; 3]; 3] {
    let a = [[m[0], m[1], m[2]], [m[4], m[5], m[6]], [m[8], m[9], m[10]]];
    let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1]) - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0]) + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
    if det.abs() < 1e-20 {
        return a;
    }
    let inv = [
        [(a[1][1] * a[2][2] - a[1][2] * a[2][1]) / det, (a[0][2] * a[2][1] - a[0][1] * a[2][2]) / det, (a[0][1] * a[1][2] - a[0][2] * a[1][1]) / det],
        [(a[1][2] * a[2][0] - a[1][0] * a[2][2]) / det, (a[0][0] * a[2][2] - a[0][2] * a[2][0]) / det, (a[0][2] * a[1][0] - a[0][0] * a[1][2]) / det],
        [(a[1][0] * a[2][1] - a[1][1] * a[2][0]) / det, (a[0][1] * a[2][0] - a[0][0] * a[2][1]) / det, (a[0][0] * a[1][1] - a[0][1] * a[1][0]) / det],
    ];
    // row-vector normal' = n * inv^T
    [[inv[0][0], inv[1][0], inv[2][0]], [inv[0][1], inv[1][1], inv[2][1]], [inv[0][2], inv[1][2], inv[2][2]]]
}

fn rowmul(v: [f32; 3], m: &[[f32; 3]; 3]) -> [f32; 3] {
    [v[0] * m[0][0] + v[1] * m[1][0] + v[2] * m[2][0], v[0] * m[0][1] + v[1] * m[1][1] + v[2] * m[2][1], v[0] * m[0][2] + v[1] * m[1][2] + v[2] * m[2][2]]
}

fn inst_data(scan: &Scan, mi: usize, fates: Option<&Vec<Fate>>, decal: bool) -> InstData {
    let m = &scan.meshes[mi];
    let mesh = scan.mesh(m);
    let mut pos = scan.positions(m);
    let tris = scan.triangles(m);
    let nm = normal_matrix(&m.world);
    let tm = [[m.world[0], m.world[1], m.world[2]], [m.world[4], m.world[5], m.world[6]], [m.world[8], m.world[9], m.world[10]]];
    let nv = pos.len();
    // face normals / areas
    let face: Vec<([f32; 3], f32)> = tris.iter().map(|t| {
        let c = cross(sub(pos[t[1] as usize], pos[t[0] as usize]), sub(pos[t[2] as usize], pos[t[0] as usize]));
        let l = len(c);
        (if l > 1e-20 && l.is_finite() { [c[0] / l, c[1] / l, c[2] / l] } else { [0.0, 1.0, 0.0] }, l)
    }).collect();
    let mut best: Vec<(f32, [f32; 3])> = vec![(-1.0, [0.0, 1.0, 0.0]); nv];
    for (t, &(n, a)) in tris.iter().zip(&face) {
        for &v in t {
            if a > best[v as usize].0 {
                best[v as usize] = (a, n);
            }
        }
    }
    let mut fixed = 0;
    let mut nrm: Vec<[f32; 3]> = (0..nv).map(|v| {
        match unit(rowmul(mesh.normal(v), &nm)) {
            Some(n) if len(mesh.normal(v)) > 0.5 => n,
            _ => {
                fixed += 1;
                best[v].1
            }
        }
    }).collect();
    let mut uv: Vec<[f32; 2]> = (0..nv).map(|v| {
        let u = mesh.uv(v);
        [if u[0].is_finite() { u[0] } else { 0.0 }, if u[1].is_finite() { u[1] } else { 0.0 }]
    }).collect();
    if nv > 0 {
        let mean = [uv.iter().map(|u| u[0] as f64).sum::<f64>() / nv as f64, uv.iter().map(|u| u[1] as f64).sum::<f64>() / nv as f64];
        if mean[0].abs() > 16.0 || mean[1].abs() > 16.0 {
            let off = [mean[0].round() as f32, mean[1].round() as f32];
            for u in &mut uv {
                u[0] -= off[0];
                u[1] -= off[1];
            }
        }
    }
    // tangents: AC1 tangent, orthonormalised; w from the UV orientation
    let mut bit = vec![[0f32; 3]; nv];
    let mut tuv = vec![[0f32; 3]; nv];
    for t in &tris {
        let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
        let (e1, e2) = (sub(pos[b], pos[a]), sub(pos[c], pos[a]));
        let (du1, dv1, du2, dv2) = (uv[b][0] - uv[a][0], uv[b][1] - uv[a][1], uv[c][0] - uv[a][0], uv[c][1] - uv[a][1]);
        let r = du1 * dv2 - du2 * dv1;
        if r.abs() < 1e-20 || !r.is_finite() {
            continue;
        }
        let tu = [(e1[0] * dv2 - e2[0] * dv1) / r, (e1[1] * dv2 - e2[1] * dv1) / r, (e1[2] * dv2 - e2[2] * dv1) / r];
        let bv = [(e2[0] * du1 - e1[0] * du2) / r, (e2[1] * du1 - e1[1] * du2) / r, (e2[2] * du1 - e1[2] * du2) / r];
        for &v in &[a, b, c] {
            for k in 0..3 {
                tuv[v][k] += tu[k];
                bit[v][k] += bv[k];
            }
        }
    }
    let tan: Vec<[f32; 4]> = (0..nv).map(|v| {
        let n = nrm[v];
        let ortho = |t: [f32; 3]| unit(sub(t, [n[0] * dot(n, t), n[1] * dot(n, t), n[2] * dot(n, t)]));
        let t = ortho(rowmul(mesh.tangent(v), &tm)).or_else(|| ortho(tuv[v])).unwrap_or_else(|| {
            let a = if n[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
            unit(cross(n, a)).unwrap_or([1.0, 0.0, 0.0])
        });
        let w = if dot(cross(n, t), bit[v]) < 0.0 { -1.0 } else { 1.0 };
        [t[0], t[1], t[2], w]
    }).collect();
    // pushes (back-to-back sheets with different UVs): each pushed triangle
    // gets its own vertex copies (shared by pushed neighbours facing the same
    // way), moved about 1 cm (the plan's distance) along its face normal - a sheet whose two sides share
    // vertices could not part otherwise; the copy's normal faces its side
    let mut tris = tris;
    let mut tan = tan;
    if let Some(f) = fates {
        let mut copies: FxHashMap<(u32, (i8, i8, i8), u8), u32> = FxHashMap::default();
        for ti in 0..tris.len() {
            let Fate::Push(d) = f[ti] else { continue };
            let push = PUSHES[d as usize];
            let fnorm = face[ti].0;
            let side = ((fnorm[0] * 4.0).round() as i8, (fnorm[1] * 4.0).round() as i8, (fnorm[2] * 4.0).round() as i8);
            for c in 0..3 {
                let v = tris[ti][c];
                let nvx = *copies.entry((v, side, d)).or_insert_with(|| {
                    let i = v as usize;
                    let p = pos[i];
                    pos.push([p[0] + fnorm[0] * push, p[1] + fnorm[1] * push, p[2] + fnorm[2] * push]);
                    let n = nrm[i];
                    nrm.push(if dot(n, fnorm) >= 0.0 { n } else { [-n[0], -n[1], -n[2]] });
                    uv.push(uv[i]);
                    tan.push(tan[i]);
                    (pos.len() - 1) as u32
                });
                tris[ti][c] = nvx;
            }
        }
    }
    let nv = pos.len();
    if decal {
        for v in 0..nv {
            for k in 0..3 {
                pos[v][k] += nrm[v][k] * DECAL_LIFT;
            }
        }
    }
    for n in &mut nrm {
        if !n.iter().all(|c| c.is_finite()) {
            *n = [0.0, 1.0, 0.0];
        }
    }
    InstData { pos, nrm, uv, tan, tris, fixed_normals: fixed }
}

/// One mesh written.
#[derive(Clone, Debug)]
pub struct Written {
    pub name: String,
    pub path: String,
    pub role: GroupRole,
    pub layouts: u64,
    pub lo: [f32; 3],
    pub hi: [f32; 3],
    pub tris: usize,
    pub verts: usize,
    pub far_shadow: bool,
    /// material variant (look.rs), 0 = none
    pub variant: u16,
}

pub struct Output {
    pub meshes: Vec<Written>,
    pub fixed_normals: usize,
    /// simplified LOD levels written
    pub lod_levels: usize,
}

fn put_f32s(m: &mut Msg, f: u32, v: &[f32]) {
    m.push(f, Val::Bytes(pb::f32_bytes(v)));
}

fn v3msg(v: [f32; 3]) -> Msg {
    super::scene::v3(v)
}

/// Write every planned mesh into `<out>\static_mesh\` (collision-only in
/// `static_mesh\physics\`). `material_path(g, two_sided, variant)` gives
/// the written material of an AC1 material; `colour_of(variant)` the vertex
/// colour stream value of a variant (start lights carry their lamp class
/// there), else the neutral zeros.
pub fn write(scan: &Scan, plan: &Plan, id: &str, out: &Path, material_path: &dyn Fn(usize, bool, u16) -> Option<String>,
    colour_of: &dyn Fn(u16) -> Option<[f32; 4]>) -> Result<Output> {
    std::fs::create_dir_all(out.join("static_mesh").join("physics"))?;
    let kinds: Vec<Kind> = scan.surfaces.iter().map(surface_kind).collect();
    let mut written = Vec::new();
    let mut names: HashSet<String> = HashSet::new();
    let mut fixed_normals = 0;
    let mut lod_levels = 0;
    for (key, members) in &plan.groups {
        let cap = if matches!(key.role, GroupRole::Collider(_)) { MAX_PHYS_TRIS } else { MAX_TRIS };
        // split members into chunks of at most `cap` triangles
        let mut chunks: Vec<Vec<(usize, &[u32])>> = vec![Vec::new()];
        let mut n = 0;
        for (mi, tris) in members {
            for part in tris.chunks(cap) {
                if n + part.len() > cap && n > 0 {
                    chunks.push(Vec::new());
                    n = 0;
                }
                chunks.last_mut().unwrap().push((*mi, part));
                n += part.len();
            }
        }
        let base = match key.role {
            GroupRole::Visual => key.material.map(|g| scan.materials[g].name.clone()).unwrap_or_default(),
            GroupRole::Drawn(s) => format!("{}_{}", scan.surfaces[s].key, key.material.map(|g| scan.materials[g].name.as_str()).unwrap_or("")),
            GroupRole::Collider(s) => format!("collider_{}", scan.surfaces[s].key),
        };
        for chunk in chunks.iter().filter(|c| !c.is_empty()) {
            let mut stem = super::materials::safe(&if key.tile.0 == 0 { format!("{base}_{}_{}", key.tile.1, key.tile.2) } else { format!("{base}_l{}_{}_{}", key.tile.0, key.tile.1, key.tile.2) });
            if key.two_sided {
                stem += "_2s";
            }
            if key.variant != 0 {
                stem += &format!("_v{}", key.variant);
            }
            let mut k = 1;
            while !names.insert(stem.clone()) {
                stem = format!("{stem}_{k}");
                k += 1;
            }
            let (mut pos, mut nrm, mut uv, mut tan, mut idx): (Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>, Vec<u32>) = Default::default();
            for (mi, tris) in chunk {
                let m = &scan.meshes[*mi];
                let decal = key.material.map(|g| { let mt = scan.material(g); mt.blend_mode != 0 || mt.depth_mode != 0 }).unwrap_or(false) && matches!(m.role, Role::Visual);
                let d = inst_data(scan, *mi, plan.fates.get(mi), decal);
                fixed_normals += d.fixed_normals;
                let mut remap: FxHashMap<u32, u32> = FxHashMap::default();
                for &t in tris.iter() {
                    for &v in &d.tris[t as usize] {
                        let next = (pos.len() / 3) as u32;
                        let e = *remap.entry(v).or_insert_with(|| {
                            let v = v as usize;
                            pos.extend_from_slice(&d.pos[v]);
                            nrm.extend_from_slice(&d.nrm[v]);
                            uv.extend_from_slice(&d.uv[v]);
                            tan.extend_from_slice(&d.tan[v]);
                            next
                        });
                        idx.push(e);
                    }
                }
            }
            let nv = pos.len() / 3;
            if nv == 0 || idx.is_empty() {
                continue;
            }
            let pts: Vec<[f32; 3]> = pos.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
            let (lo, hi) = bbox(&pts);
            let uvmax = uv.iter().fold(0f32, |a, b| a.max(b.abs()));
            let drivable_ground = match key.role {
                GroupRole::Drawn(s) | GroupRole::Collider(s) => kinds[s].track() || kinds[s] == Kind::Verge,
                _ => false,
            };
            let far = key.cast && hi[1] - lo[1] >= FAR_SHADOW_HEIGHT && !matches!(key.role, GroupRole::Collider(_)) && !drivable_ground;
            let material = match key.material {
                Some(g) => material_path(g, key.two_sided, key.variant).unwrap_or_else(|| COLLIDER_MATERIAL.to_string()),
                None => COLLIDER_MATERIAL.to_string(),
            };
            // LOD bodies: the mesh itself, then (big drawn decor only) the
            // simplified levels of LODS
            let cast_lod = key.cast && !matches!(key.role, GroupRole::Collider(_));
            let colour = colour_of(key.variant);
            let body = |idx: &[u32], lod_in: f32| -> Msg {
                // compact the vertices a level uses
                let mut remap: FxHashMap<u32, u32> = FxHashMap::default();
                let mut order: Vec<u32> = Vec::new();
                let li: Vec<u32> = idx.iter().map(|&v| *remap.entry(v).or_insert_with(|| { order.push(v); (order.len() - 1) as u32 })).collect();
                let pick = |src: &[f32], n: usize| -> Vec<f32> { order.iter().flat_map(|&v| src[v as usize * n..v as usize * n + n].to_vec()).collect() };
                let p3 = pick(&pos, 3);
                let pts: Vec<[f32; 3]> = p3.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
                let (llo, lhi) = bbox(&pts);
                let n = order.len();
                let mut lod = Msg::default();
                if cast_lod {
                    lod.set_varint(1, 1);
                }
                if lod_in > 0.0 {
                    lod.set_f32(3, lod_in);
                }
                let mut batch = Msg::default();
                batch.set_str(1, &stem);
                batch.set_varint(3, li.len() as u64);
                batch.set_str(4, &material);
                lod.push(4, Val::Msg(batch));
                put_f32s(&mut lod, 5, &p3);
                put_f32s(&mut lod, 6, &pick(&nrm, 3));
                put_f32s(&mut lod, 7, &pick(&uv, 2));
                put_f32s(&mut lod, 8, &pick(&tan, 4));
                lod.push(11, Val::Bytes(pb::varint_bytes(&li)));
                // component 3 is the vertex AO (ao / 255; the bakes stage
                // fills it): neutral 1 / 255 until then
                let c = colour.unwrap_or([0.0; 4]);
                put_f32s(&mut lod, 13, &[c[0], c[1], c[2], NEUTRAL_AO].repeat(n));
                for f in [14, 15, 16] {
                    put_f32s(&mut lod, f, &vec![0.0; n * 2]);
                }
                lod.push(17, Val::Msg(v3msg(llo)));
                lod.push(18, Val::Msg(v3msg(lhi)));
                lod
            };
            let mut bodies = vec![body(&idx, 0.0)];
            let lod_ok = key.material.map(|g| !super::materials::transparent(scan, g)).unwrap_or(false) && !key.two_sided;
            if matches!(key.role, GroupRole::Visual) && lod_ok && idx.len() / 3 >= LOD_MIN_TRIS {
                let vb: Vec<u8> = pos.iter().flat_map(|f| f.to_le_bytes()).collect();
                if let Ok(adapter) = meshopt::VertexDataAdapter::new(&vb, 12, 0) {
                    let mut prev = idx.len();
                    for &(ratio, dist) in LODS.iter() {
                        let target = ((idx.len() as f32 * ratio) as usize / 3 * 3).max(3);
                        let s = meshopt::simplify(&idx, &adapter, target, 0.05, meshopt::SimplifyOptions::None, None);
                        // a level only when it really saves (ours: under 70 % of the last)
                        if s.len() >= 3 && (s.len() as f32) < prev as f32 * 0.7 && !has_back_to_back(&pos, &s) {
                            bodies.push(body(&s, dist));
                            prev = s.len();
                            lod_levels += 1;
                        }
                    }
                }
            }
            let mut top = Msg::default();
            top.set_varint(2, 1);
            top.set_varint(3, 1);
            for l in bodies {
                top.push(5, Val::Msg(l));
            }
            top.push(6, Val::Msg(v3msg(lo)));
            top.push(7, Val::Msg(v3msg(hi)));
            if far {
                top.set_varint(8, 1);
            }
            if drivable_ground && !matches!(key.role, GroupRole::Collider(_)) {
                top.set_varint(15, 1);
            }
            if matches!(key.role, GroupRole::Collider(_)) {
                top.push(20, Val::Msg(Msg::default()));
            }
            if uvmax > 32.0 {
                top.set_varint(100, 1);
            }
            let (dir, rel) = if matches!(key.role, GroupRole::Collider(_)) { ("static_mesh\\physics", out.join("static_mesh").join("physics")) } else { ("static_mesh", out.join("static_mesh")) };
            std::fs::write(rel.join(format!("{stem}.mesh")), top.encode())?;
            written.push(Written {
                name: stem.clone(), path: format!("content\\tracks\\{id}\\{dir}\\{stem}.mesh"), role: key.role, layouts: key.layouts,
                lo, hi, tris: idx.len() / 3, verts: nv, far_shadow: far, variant: key.variant,
            });
        }
    }
    Ok(Output { meshes: written, fixed_normals, lod_levels })
}

/// The SMesh entity of a written mesh (identity transform, world-space box).
pub fn entity(w: &Written, scan: &Scan) -> super::scene::Entity {
    let mut d = Msg::default();
    d.set_str(1, &w.path);
    match w.role {
        GroupRole::Visual => {}
        GroupRole::Drawn(s) => d.set_str(2, &scan.surfaces[s].key),
        GroupRole::Collider(s) => {
            d.set_str(2, &scan.surfaces[s].key);
            d.set_varint(3, 1);
        }
    }
    let mut col = Msg::default();
    col.push(5, Val::Msg(Msg::default()));
    col.push(6, Val::Msg(Msg::default()));
    d.push(4, Val::Msg(col));
    let mut e = super::scene::Entity::new(&w.name, "SMesh", 100, d);
    e.bounds = Some(([w.lo[0], w.lo[2]], [w.hi[0], w.hi[2]]));
    e
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ac1::kn5;

    #[test]
    fn back_to_back_keys() {
        let (a, b, c) = ((0, 0, 0), (200, 0, 0), (0, 0, 200));
        let (k1, w1) = tri_key(a, b, c).unwrap();
        let (k2, w2) = tri_key(a, c, b).unwrap();
        let (k3, w3) = tri_key(b, c, a).unwrap();
        assert_eq!(k1, k2);
        assert_ne!(w1, w2);
        assert_eq!((k1, w1), (k3, w3));
        assert!(tri_key(a, a, c).is_none());
    }

    #[test]
    fn normal_matrix_of_scale() {
        // non-uniform scale x2 on X: a 45-degree normal leans towards Y
        let mut m = kn5::IDENTITY;
        m[0] = 2.0;
        let n = unit(rowmul([1.0, 1.0, 0.0], &normal_matrix(&m))).unwrap();
        assert!(n[1] > n[0]);
        assert!(is_backdrop([0.0, 0.0, 0.0], [300.0, 40.0, 0.2]) && !is_backdrop([0.0; 3], [300.0, 4.0, 0.2]));
    }
}
