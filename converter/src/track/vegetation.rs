//! 3D vegetation (requirement track-vegetation): AC1 card trees and bushes
//! (and CSP tree lists) become instances of the game's own 3D plants.
//!
//! Game facts (stock scenes, measured with the clean P9 tools):
//! - plants are ISMesh entities (f50.f105 {f1 mesh, f7 {f2 positions, f3
//!   rotations (Euler degrees, yaw only for trees: (0, y, 0) or (180, y,
//!   180)), f4 scales} as packed f32 x 3, f10 {f5 "", f6 ""}}), in the main
//!   scene, with f13 {xz box of the instances, f3 cull distance = the
//!   mesh's lodOut} and f14 = 100000; instance positions are relative to
//!   the entity transform (ours sits at the origin);
//! - stock tracks hold 220..2,000 3D trees and 700..35,000 bushes within
//!   150 m of the road, and 1,500..36,000 cross impostors out to 1,000 m and
//!   more (cull 2,500..5,500 m); tree scales 0.9..2.9, bushes 0.8..2.8.
//!
//! Ours:
//! - **Cards.** An AC1 material is foliage when its shader is ksTree /
//!   ksGrass, or it is alpha tested / blended and its name or diffuse
//!   texture names a plant ([`card_class`]); bark / trunk materials are
//!   tree parts. Every placed instance of such a material is cut into
//!   fragments (triangles joined by shared or coincident vertices).
//! - **Clustering.** Fragments join (union-find over an 8 m grid) when their
//!   xz centres are closer than 0.35 x their mean width + 0.75 m, their
//!   height ranges are less than 2.5 m apart and the lower one is at least
//!   a quarter of the other's height or floats 1 m above its foot:
//!   crossed cards, a crown above its trunk and stacked crown pieces
//!   become one tree, the grass cards around its foot do not. A fragment longer
//!   than 2.5 x its height and 12 m is a forest wall: trees every
//!   0.6 x height (3..10 m) along it. A cluster wider than 1.3 x its height
//!   and 10 m (merged neighbours) is re-cut on the same spacing.
//! - **Ground.** A plant stands on the highest physics or upward-facing
//!   drawn surface at most 1.5 m above the cluster's lowest point (so never
//!   on a trunk or a crown), 0.12 m sunk; one whose footprint (trunk radius
//!   0.8 m, bushes a quarter of their width) touches drivable ground
//!   within [`ROAD_NEAR`] of an AI line, or a wall, is pushed away from it
//!   in 1 m steps (up to 8 m) or dropped.
//! - **Species** ([`pick`]): the catalogue entries of the plant's family
//!   (tree from 4.5 m, bush from 1.0 m; lower cards are grass, replaced by
//!   the carpet) scored on height (log ratio), crown width / height,
//!   leaf colour (chromaticity, lightness), climate (vegcat::climate_of of
//!   the track's latitude against the species' affinity), conifer / palm /
//!   dead hints from the AC1 names. The species most plants would pick
//!   form the track's palette ([`palette_size`]: 4 trees, 2 bushes, 3
//!   impostors, as stock tracks use few); each plant then takes one of the
//!   best of the palette (within 0.35 of the best score, up to 4) drawn by
//!   a position hash for variety. Scale =
//!   AC1 height / species height (0.5..2.5), crown x / z within +-25 % of
//!   it to follow the AC1 width, yaw from the hash.
//! - **Far plants.** Beyond [`IMPOSTOR_FROM`] of every AI line a tree is a
//!   camera-facing impostor (the catalogue's billboards and species cards,
//!   zero rotation and uniform scale as stock places them), as stock
//!   forests are.
//! - **Replacement.** Every card mesh that produced a plant is not drawn;
//!   meshes with flat (ground) fragments stay drawn.
//! - **Caps.** At most [`PLANT_CAP`] plants (the nearest to the AI lines
//!   kept); nodes per species and [`TILE`] m tile, at most [`NODE_CAP`]
//!   instances each.
//! - **Canopies for the bakes.** Each plant gives an octagonal bipyramid
//!   crown, 0.45 x its width x [`CANOPY_SCALE`] in radius: the full crown is a partial
//!   foliage occluder for the probes, the lower half of a core of half the
//!   crown radii a solid occluder for AO and probes (an open shell, so the
//!   probe texels inside a crown are not taken for enclosed ones).
//! - Deterministic: fixed orders, hashes of positions, no random generator.

use rayon::prelude::*;
use rustc_hash::FxHashMap;
use std::collections::{BTreeMap, HashMap};

use super::bakes::hash3;
use super::classes::Kind;
use super::csptrees;
use super::ground::{Ground, Tri};
use super::scan::{Role, Scan};
use super::scene::Entity;
use super::vegcat::{Catalogue, Family};
use crate::io::pb::{self, Msg, Val};

/// Trees become impostors beyond this distance from every AI line (ours:
/// stock 3D trees stand within 150 m of the road, most within 50 m, and
/// make 6..20 % of the trees; impostors start 20 m out).
pub const IMPOSTOR_FROM: f32 = 60.0;
/// Most plants per track (ours: above the stock maximum of ~37k trees +
/// impostors, nurburgring).
pub const PLANT_CAP: usize = 60_000;
/// Node tile edge (ours).
pub const TILE: f32 = 1024.0;
/// Most instances in one tree / bush node (ours).
pub const NODE_CAP: usize = 20_000;
/// Card clusters lower than this are grass (ours).
pub const GRASS_MAX: f32 = 1.0;
/// Card clusters from this height are trees, below it bushes (ours).
pub const TREE_MIN: f32 = 4.5;
const SINK: f32 = 0.12;
/// Drivable ground farther than this from every AI line does not keep
/// plants away (ours).
pub const ROAD_NEAR: f32 = 30.0;
/// Tallest plant drawn (ours: stock tree heights reach p90 20..24 m;
/// taller AC1 cards are forest walls on a slope).
pub const MAX_HEIGHT: f32 = 30.0;

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub trees: bool,
    pub grass: bool,
}

impl Options {
    /// `ACECONV_VEGETATION` = comma list of `trees`, `grass` (or `none`);
    /// unset = both.
    pub fn from_env() -> Options {
        match std::env::var("ACECONV_VEGETATION") {
            Ok(v) => {
                let v = v.to_lowercase();
                let has = |k: &str| v.split(',').any(|x| x.trim() == k || x.trim() == "all");
                Options { trees: has("trees"), grass: has("grass") }
            }
            Err(_) => Options { trees: true, grass: true },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardKind {
    Foliage,
    Trunk,
}

const PLANT_WORDS: [&str; 30] = [
    "tree", "leaf", "leaves", "branch", "foliage", "forest", "pine", "fir", "spruce", "palm", "bush", "shrub", "hedge", "oak", "birch",
    "cypress", "conifer", "vegeta", "plant", "bb_tree", "treeline", "treewall", "bushwall", "albero", "alberi", "cedar", "poplar", "willow", "maple", "beech",
];
const NOT_PLANT: [&str; 30] = [
    "fence", "net", "grid", "sign", "people", "crowd", "spectat", "flag", "banner", "glass", "decal", "shadow", "wire", "rail", "logo",
    "sponsor", "text", "light", "lamp", "grail", "cable", "crane", "chain", "metal", "gstand", "structure", "tyre", "tire", "brake", "window",
];
const TRUNK_WORDS: [&str; 5] = ["bark", "trunk", "tronc", "stem", "corteccia"];

/// Is this AC1 material a plant card (or a tree's trunk)?
pub fn card_class(scan: &Scan, g: usize) -> Option<CardKind> {
    let m = scan.material(g);
    let tex = m.texture("txDiffuse").unwrap_or("").to_lowercase();
    let name = m.name.to_lowercase();
    let both = format!("{name} {tex}");
    if NOT_PLANT.iter().any(|w| both.contains(w)) {
        return None;
    }
    let shader = m.shader.to_lowercase();
    if TRUNK_WORDS.iter().any(|w| both.contains(w)) {
        return Some(CardKind::Trunk);
    }
    let cutout = m.alpha_tested || m.blend_mode != 0;
    if shader == "kstree" || shader == "ksgrass" || (cutout && (PLANT_WORDS.iter().any(|w| both.contains(w)) || both.contains("grass"))) {
        return Some(CardKind::Foliage);
    }
    None
}

/// Nearest-point queries against the AI lines (xz), on a 50 m grid.
pub struct LineIndex {
    cell: f32,
    cells: FxHashMap<(i32, i32), Vec<[f32; 2]>>,
}

impl LineIndex {
    pub fn new(lines: &[Vec<[f32; 3]>]) -> LineIndex {
        let cell = 50.0;
        let mut cells: FxHashMap<(i32, i32), Vec<[f32; 2]>> = FxHashMap::default();
        for l in lines {
            for p in l {
                cells.entry(((p[0] / cell).floor() as i32, (p[2] / cell).floor() as i32)).or_default().push([p[0], p[2]]);
            }
        }
        LineIndex { cell, cells }
    }
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }
    /// Distance in xz to the nearest line point (searched up to `reach`;
    /// `reach` when nothing is that close).
    pub fn dist(&self, x: f32, z: f32, reach: f32) -> f32 {
        let (cx, cz) = ((x / self.cell).floor() as i32, (z / self.cell).floor() as i32);
        let r = (reach / self.cell).ceil() as i32;
        let mut best = reach * reach;
        for ring in 0..=r {
            // stop once the ring is further than the best found
            if ring > 0 && ((ring - 1) as f32 * self.cell).powi(2) > best {
                break;
            }
            for dz in -ring..=ring {
                for dx in -ring..=ring {
                    if dx.abs() != ring && dz.abs() != ring {
                        continue;
                    }
                    if let Some(v) = self.cells.get(&(cx + dx, cz + dz)) {
                        for p in v {
                            let d = (p[0] - x).powi(2) + (p[1] - z).powi(2);
                            if d < best {
                                best = d;
                            }
                        }
                    }
                }
            }
        }
        best.sqrt()
    }
}

/// One instance of a plant.
#[derive(Clone, Copy, Debug)]
pub struct Plant {
    pub species: usize,
    pub pos: [f32; 3],
    pub yaw: f32,
    pub scale: [f32; 3],
    /// drawn height and crown width (metres)
    pub height: f32,
    pub width: f32,
    /// from a CSP list (else an AC1 card)
    pub csp: bool,
    /// distance to the nearest AI line
    pub dist: f32,
}

#[derive(Default)]
pub struct Plan {
    /// per scan mesh: not drawn (a replaced card)
    pub drop: Vec<bool>,
    pub plants: Vec<Plant>,
    pub report: Vec<String>,
    pub notes: Vec<String>,
}

/// Bins of a forest strip's height profile along its axis.
const PROFILE: usize = 16;

/// A connected piece of card geometry.
#[derive(Clone, Debug)]
struct Fragment {
    mesh: usize,
    material: usize,
    kind: CardKind,
    lo: [f32; 3],
    hi: [f32; 3],
    /// area-weighted centroid (xz)
    c: [f32; 2],
    area: f32,
    /// principal xz direction and extent along it
    axis: [f32; 2],
    len: f32,
    /// long fragments: (bottom, top) per bin along the axis
    prof: Option<Box<[[f32; 2]; PROFILE]>>,
}

impl Fragment {
    fn width(&self) -> f32 {
        (self.hi[0] - self.lo[0]).max(self.hi[2] - self.lo[2])
    }
    fn height(&self) -> f32 {
        self.hi[1] - self.lo[1]
    }
    fn strip(&self) -> bool {
        self.kind == CardKind::Foliage && self.height() >= 2.0 && self.len > (2.5 * self.height()).max(12.0)
    }
    /// a sheet lying on the ground (forest floor, grass sheet), not a plant
    fn flat(&self) -> bool {
        self.width() > 2.0 && self.height() < (0.1 * self.width()).max(0.5)
    }
}

fn find(p: &mut [u32], mut i: u32) -> u32 {
    while p[i as usize] != i {
        p[i as usize] = p[p[i as usize] as usize];
        i = p[i as usize];
    }
    i
}

fn union(p: &mut [u32], a: u32, b: u32) {
    let (ra, rb) = (find(p, a), find(p, b));
    if ra != rb {
        // the smaller index wins: deterministic roots
        let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
        p[hi as usize] = lo;
    }
}

/// Fragments of one placed card mesh.
fn fragments(scan: &Scan, mi: usize, kind: CardKind) -> Vec<Fragment> {
    let m = &scan.meshes[mi];
    let pos = scan.positions(m);
    let tris = scan.triangles(m);
    if tris.is_empty() {
        return Vec::new();
    }
    let n = pos.len();
    let mut par: Vec<u32> = (0..n as u32).collect();
    let mut first: FxHashMap<(i32, i32, i32), u32> = FxHashMap::default();
    for (i, p) in pos.iter().enumerate() {
        let q = ((p[0] * 20.0).round() as i32, (p[1] * 20.0).round() as i32, (p[2] * 20.0).round() as i32);
        let f = *first.entry(q).or_insert(i as u32);
        union(&mut par, f, i as u32);
    }
    for t in &tris {
        union(&mut par, t[0], t[1]);
        union(&mut par, t[0], t[2]);
    }
    // per root: bbox, area centroid, xz moments
    struct Acc {
        lo: [f32; 3],
        hi: [f32; 3],
        cx: f64,
        cz: f64,
        a: f64,
        s: [f64; 6],
    }
    let mut acc: BTreeMap<u32, Acc> = BTreeMap::new();
    for t in &tris {
        let r = find(&mut par, t[0]);
        let (a, b, c) = (pos[t[0] as usize], pos[t[1] as usize], pos[t[2] as usize]);
        let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let cr = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
        let area = 0.5 * (cr[0] * cr[0] + cr[1] * cr[1] + cr[2] * cr[2]).sqrt() as f64;
        let e = acc.entry(r).or_insert(Acc { lo: [f32::INFINITY; 3], hi: [f32::NEG_INFINITY; 3], cx: 0.0, cz: 0.0, a: 0.0, s: [0.0; 6] });
        for v in [a, b, c] {
            for k in 0..3 {
                e.lo[k] = e.lo[k].min(v[k]);
                e.hi[k] = e.hi[k].max(v[k]);
            }
            let (x, z) = (v[0] as f64, v[2] as f64);
            e.s[0] += 1.0;
            e.s[1] += x;
            e.s[2] += z;
            e.s[3] += x * x;
            e.s[4] += z * z;
            e.s[5] += x * z;
        }
        let w = area.max(1e-6);
        e.cx += w * (a[0] + b[0] + c[0]) as f64 / 3.0;
        e.cz += w * (a[2] + b[2] + c[2]) as f64 / 3.0;
        e.a += w;
    }
    // a planar strip runs along one of its xz bbox diagonals: the one whose
    // sign matches the x-z covariance of its vertices
    let mut out: Vec<(u32, Fragment)> = acc.into_iter().filter(|(_, e)| e.lo[0].is_finite()).map(|(root, e)| {
        let (ex, ez) = (e.hi[0] - e.lo[0], e.hi[2] - e.lo[2]);
        let n = e.s[0].max(1.0);
        let cov = e.s[5] / n - (e.s[1] / n) * (e.s[2] / n);
        let len = (ex * ex + ez * ez).sqrt();
        let axis = if len > 1e-3 { [ex / len, if cov >= 0.0 { ez / len } else { -ez / len }] } else { [1.0, 0.0] };
        (root, Fragment { mesh: mi, material: m.material, kind, lo: e.lo, hi: e.hi, c: [(e.cx / e.a) as f32, (e.cz / e.a) as f32], area: e.a as f32, axis, len, prof: None })
    }).collect();
    // height profiles of the long ones (forest walls on a slope)
    let long: FxHashMap<u32, usize> = out.iter().enumerate().filter(|(_, (_, f))| f.len > 12.0 && f.height() >= 2.0).map(|(i, (r, _))| (*r, i)).collect();
    if !long.is_empty() {
        let mut profs: FxHashMap<usize, [[f32; 2]; PROFILE]> = FxHashMap::default();
        for t in &tris {
            let r = find(&mut par, t[0]);
            let Some(&i) = long.get(&r) else { continue };
            let f = &out[i].1;
            // the axis runs from the corner its direction starts at
            let start = [if f.axis[0] >= 0.0 { f.lo[0] } else { f.hi[0] }, if f.axis[1] >= 0.0 { f.lo[2] } else { f.hi[2] }];
            let pr = profs.entry(i).or_insert([[f32::INFINITY, f32::NEG_INFINITY]; PROFILE]);
            for &vi in t {
                let v = pos[vi as usize];
                let u = ((v[0] - start[0]) * f.axis[0] + (v[2] - start[1]) * f.axis[1]) / f.len.max(1e-3);
                let b = ((u * PROFILE as f32) as isize).clamp(0, PROFILE as isize - 1) as usize;
                pr[b][0] = pr[b][0].min(v[1]);
                pr[b][1] = pr[b][1].max(v[1]);
            }
        }
        for (i, p) in profs {
            out[i].1.prof = Some(Box::new(p));
        }
    }
    out.into_iter().map(|(_, f)| f).collect()
}

/// A plant to place: from a card cluster or a CSP entry.
#[derive(Clone, Debug)]
struct Seed {
    x: f32,
    z: f32,
    bottom: f32,
    top: f32,
    width: f32,
    colour: [f32; 3],
    hints: String,
    csp: bool,
    yaw: Option<f32>,
}

/// What the species pick knows about one AC1 plant.
#[derive(Clone, Debug)]
pub struct Feat {
    pub height: f32,
    pub ratio: f32,
    pub colour: [f32; 3],
    pub conifer: Option<bool>,
    pub palm: bool,
    pub dead: bool,
}

impl Feat {
    fn from_hints(height: f32, width: f32, colour: [f32; 3], hints: &str) -> Feat {
        let h = hints.to_lowercase();
        let any = |w: &[&str]| w.iter().any(|x| h.contains(x));
        let conifer = if any(&["pine", "fir", "spruce", "conifer", "cedar", "larch", "abete", "pino", "cypress", "thuja"]) {
            Some(true)
        } else if any(&["oak", "birch", "beech", "maple", "poplar", "plane", "willow", "palm", "acacia", "olive", "deciduous", "leaf", "leaves"]) {
            Some(false)
        } else {
            None
        };
        Feat { height, ratio: width / height.max(0.1), colour, conifer, palm: any(&["palm"]), dead: any(&["dead", "dry_tree", "deadtree"]) }
    }
}

fn chroma(c: [f32; 3]) -> ([f32; 3], f32) {
    let s = (c[0] + c[1] + c[2]).max(1e-4);
    ([c[0] / s, c[1] / s, c[2] / s], s / 3.0)
}

/// Score a species for a plant (lower is better).
pub fn score(f: &Feat, s: &super::vegcat::Species, climate: [f32; 4]) -> f32 {
    let mut v = 0.6 * (f.height / s.height).ln().abs().min(2.0);
    v += 1.0 * (f.ratio - s.ratio()).abs().min(1.5);
    let (ca, la) = chroma(f.colour);
    let (cb, lb) = chroma(s.colour);
    v += 2.0 * ((ca[0] - cb[0]).abs() + (ca[1] - cb[1]).abs() + (ca[2] - cb[2]).abs()) + 0.4 * (la.max(1e-3) / lb.max(1e-3)).ln().abs().min(2.0);
    let cw: f32 = climate.iter().sum::<f32>().max(1e-3);
    let fit = (0..4).map(|i| climate[i] * s.climate[i]).sum::<f32>() / cw;
    v += 1.5 * (1.0 - fit);
    if let Some(c) = f.conifer {
        if c != s.conifer {
            v += 1.5;
        }
    }
    if f.palm != s.palm {
        v += 3.0;
    }
    if f.dead != s.dead {
        v += 3.0;
    }
    v
}

/// Species per family a track uses (ours: stock tracks use a median of 3
/// tree, 1 bush and 3 impostor species, p90 8 / 4 / 5).
pub fn palette_size(f: Family) -> usize {
    match f {
        Family::Tree => 4,
        Family::Bush => 2,
        Family::Impostor => 3,
        Family::Grass => 2,
    }
}

/// The species for a plant of family `fam`: one of the best few (within
/// `allowed` when given), by hash.
pub fn pick(cat: &Catalogue, fam: Family, f: &Feat, climate: [f32; 4], h: u64) -> Option<usize> {
    pick_in(cat, fam, f, climate, h, None)
}

pub fn pick_in(cat: &Catalogue, fam: Family, f: &Feat, climate: [f32; 4], h: u64, allowed: Option<&[usize]>) -> Option<usize> {
    let mut sc: Vec<(f32, usize)> = cat.of(fam).filter(|(i, _)| allowed.map(|a| a.contains(i)).unwrap_or(true)).map(|(i, s)| (score(f, s, climate), i)).collect();
    if sc.is_empty() {
        return None;
    }
    sc.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap().then(a.1.cmp(&b.1)));
    let best = sc[0].0;
    let n = sc.iter().take(4).filter(|x| x.0 <= best + 0.35).count().max(1);
    Some(sc[(h % n as u64) as usize].1)
}

pub struct Inputs<'a> {
    pub scan: &'a Scan<'a>,
    pub cat: &'a Catalogue,
    /// physics ground of the first layout
    pub ground: &'a Ground,
    pub kinds: &'a [Kind],
    /// every layout's AI line points
    pub lines: &'a LineIndex,
    pub climate: [f32; 4],
    pub track_id: &'a str,
    /// the carpet replaces low grass cards
    pub grass_on: bool,
}

/// Material colour (mean of the opaque texels of txDiffuse), per material.
fn material_colours(scan: &Scan, mats: &[usize]) -> HashMap<usize, [f32; 3]> {
    mats.par_iter().map(|&g| {
        let c = scan.material_texture(g, "txDiffuse").and_then(|t| {
            let img = crate::io::texture::decode_any(&scan.texture_bytes(t)).ok()?;
            let s = if img.w > 64 { crate::io::texture::resize_to(&img, 64, true) } else { img };
            let px: Vec<[u8; 4]> = s.px.chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]]).collect();
            super::vegcat::opaque_mean(&px, true)
        }).unwrap_or([0.08, 0.11, 0.05]);
        (g, c)
    }).collect()
}

/// Plan the plants (cards found, clustered, placed; CSP lists added).
pub fn plan(inp: &Inputs) -> Plan {
    let scan = inp.scan;
    let mut out = Plan { drop: vec![false; scan.meshes.len()], ..Default::default() };
    let t0 = std::time::Instant::now();
    // ---- card meshes
    let class: HashMap<usize, Option<CardKind>> = scan.meshes.iter().map(|m| m.material).collect::<std::collections::BTreeSet<_>>().into_iter().map(|g| (g, card_class(scan, g))).collect();
    let cards: Vec<(usize, CardKind)> = scan.meshes.iter().enumerate().filter_map(|(i, m)| {
        if !matches!(m.role, Role::Visual) {
            return None;
        }
        class[&m.material].map(|k| (i, k))
    }).collect();
    // far LOD copies are not drawn anyway: only drop them
    let (near, far): (Vec<(usize, CardKind)>, Vec<(usize, CardKind)>) = cards.iter().partition(|(i, _)| scan.mesh(&scan.meshes[*i]).lod_in <= 0.0);
    let frs: Vec<Fragment> = near.par_iter().flat_map_iter(|&(i, k)| fragments(scan, i, k)).collect();
    let mats: Vec<usize> = { let mut v: Vec<usize> = frs.iter().map(|f| f.material).collect(); v.sort(); v.dedup(); v };
    let colours = material_colours(scan, &mats);
    // dev: ACECONV_VEG_FRAGS=<file> lists every fragment
    if let Ok(f) = std::env::var("ACECONV_VEG_FRAGS") {
        let mut rows = vec!["mesh\tmaterial\tlo_x\tlo_y\tlo_z\thi_x\thi_y\thi_z\tarea\tlen\tstrip\tflat".to_string()];
        for fr in &frs {
            rows.push(format!("{}\t{}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{}\t{}", fr.mesh, scan.material(fr.material).name,
                fr.lo[0], fr.lo[1], fr.lo[2], fr.hi[0], fr.hi[1], fr.hi[2], fr.area, fr.len, fr.strip() as u8, fr.flat() as u8));
        }
        let _ = std::fs::write(f, rows.join("\n"));
    }
    let hint_of = |g: usize| -> String {
        let m = scan.material(g);
        format!("{} {} {}", m.name, m.texture("txDiffuse").unwrap_or(""), m.shader).to_lowercase()
    };
    // ---- cluster fragments
    let nf = frs.len();
    let mut par: Vec<u32> = (0..nf as u32).collect();
    let cell = 8.0f32;
    let mut grid: FxHashMap<(i32, i32), Vec<u32>> = FxHashMap::default();
    for (i, f) in frs.iter().enumerate() {
        if f.strip() || f.flat() {
            continue;
        }
        grid.entry(((f.c[0] / cell).floor() as i32, (f.c[1] / cell).floor() as i32)).or_default().push(i as u32);
    }
    let mut keys: Vec<(i32, i32)> = grid.keys().copied().collect();
    keys.sort();
    for k in &keys {
        for &i in &grid[k] {
            let a = &frs[i as usize];
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let Some(v) = grid.get(&(k.0 + dx, k.1 + dz)) else { continue };
                    for &j in v {
                        if j <= i {
                            continue;
                        }
                        let b = &frs[j as usize];
                        let d = ((a.c[0] - b.c[0]).powi(2) + (a.c[1] - b.c[1]).powi(2)).sqrt();
                        let reach = 0.35 * (a.width() + b.width()) * 0.5 + 0.75;
                        let gap = (a.lo[1] - b.hi[1]).max(b.lo[1] - a.hi[1]);
                        // a much lower piece joins only when it floats (a
                        // crown part); grass and bush cards at a trunk's
                        // foot stay apart, so a field of them cannot chain
                        // the trees around it into one
                        let (small, big) = if a.height() < b.height() { (a, b) } else { (b, a) };
                        let fits = small.height() >= 0.25 * big.height() || small.lo[1] > big.lo[1] + 1.0;
                        if d < reach && gap < 2.5 && fits {
                            union(&mut par, i, j);
                        }
                    }
                }
            }
        }
    }
    let mut clusters: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for i in 0..nf as u32 {
        let f = &frs[i as usize];
        if f.strip() || f.flat() {
            continue;
        }
        clusters.entry(find(&mut par, i)).or_default().push(i);
    }
    // ---- seeds
    let mut seeds: Vec<(Seed, Vec<usize>)> = Vec::new();
    let spacing = |h: f32| (0.6 * h).clamp(3.0, 10.0);
    let mut strips = 0usize;
    for (i, f) in frs.iter().enumerate() {
        if !f.strip() {
            continue;
        }
        strips += 1;
        let s = spacing(f.height());
        let n = (f.len / s).floor().max(1.0) as usize;
        let mid = [(f.lo[0] + f.hi[0]) * 0.5, (f.lo[2] + f.hi[2]) * 0.5];
        let ax = f.axis;
        for k in 0..n {
            let u = (k as f32 + 0.5) / n as f32;
            let (x, z) = (mid[0] + ax[0] * (u - 0.5) * f.len, mid[1] + ax[1] * (u - 0.5) * f.len);
            // the wall's own bottom and top at this point
            let (bottom, top) = f.prof.as_ref().and_then(|p| {
                let b = ((u * PROFILE as f32) as usize).min(PROFILE - 1);
                (p[b][0].is_finite()).then_some((p[b][0], p[b][1]))
            }).unwrap_or((f.lo[1], f.hi[1]));
            // a wall tells nothing of crown shapes: a neutral width / height
            seeds.push((Seed { x, z, bottom, top, width: 0.6 * (top - bottom), colour: colours[&f.material], hints: hint_of(f.material), csp: false, yaw: None }, vec![i]));
        }
    }
    for members in clusters.values() {
        let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        let (mut cx, mut cz, mut a) = (0f64, 0f64, 0f64);
        let mut col = [0f64; 3];
        let mut best_mat = (0f32, members[0] as usize);
        let mut foliage = false;
        for &m in members {
            let f = &frs[m as usize];
            for k in 0..3 {
                lo[k] = lo[k].min(f.lo[k]);
                hi[k] = hi[k].max(f.hi[k]);
            }
            let w = f.area as f64;
            cx += w * f.c[0] as f64;
            cz += w * f.c[1] as f64;
            a += w;
            if f.kind == CardKind::Foliage {
                foliage = true;
                let c = colours[&f.material];
                for k in 0..3 {
                    col[k] += w * c[k] as f64;
                }
                if f.area > best_mat.0 {
                    best_mat = (f.area, m as usize);
                }
            }
        }
        let colour = if foliage {
            let s: f64 = members.iter().filter(|&&m| frs[m as usize].kind == CardKind::Foliage).map(|&m| frs[m as usize].area as f64).sum();
            [0, 1, 2].map(|k| (col[k] / s.max(1e-6)) as f32)
        } else {
            [0.08, 0.11, 0.05]
        };
        let hints = hint_of(frs[best_mat.1].material);
        let h = hi[1] - lo[1];
        let w = (hi[0] - lo[0]).max(hi[2] - lo[2]);
        let ms: Vec<usize> = members.iter().map(|&m| m as usize).collect();
        if w > (1.3 * h).max(10.0) {
            // merged neighbours (a dense stand): re-cut on the spacing
            let s = spacing(h);
            let mut cells: BTreeMap<(i32, i32), ([f32; 2], f32, f32)> = BTreeMap::new();
            for &m in &ms {
                let f = &frs[m];
                let k = ((f.c[0] / s).floor() as i32, (f.c[1] / s).floor() as i32);
                let e = cells.entry(k).or_insert(([f.c[0], f.c[1]], f.lo[1], f.hi[1]));
                e.1 = e.1.min(f.lo[1]);
                e.2 = e.2.max(f.hi[1]);
            }
            for (c, b, t) in cells.into_values() {
                seeds.push((Seed { x: c[0], z: c[1], bottom: b, top: t, width: 0.6 * (t - b), colour, hints: hints.clone(), csp: false, yaw: None }, ms.clone()));
            }
        } else {
            seeds.push((Seed { x: (cx / a) as f32, z: (cz / a) as f32, bottom: lo[1], top: hi[1], width: w, colour, hints, csp: false, yaw: None }, ms));
        }
    }
    let card_seeds = seeds.len();
    // ---- CSP lists: their discs clear the card plants
    let lists = csptrees::read(scan, inp.track_id);
    out.notes.extend(lists.notes.iter().cloned());
    if !lists.trees.is_empty() {
        let mut disc: FxHashMap<(i32, i32), Vec<([f32; 2], f32)>> = FxHashMap::default();
        for t in &lists.trees {
            let r = (t.width * 0.5).max(2.0);
            disc.entry(((t.pos[0] / 16.0).floor() as i32, (t.pos[2] / 16.0).floor() as i32)).or_default().push(([t.pos[0], t.pos[2]], r));
        }
        let cleared = |x: f32, z: f32| -> bool {
            let (cx, cz) = ((x / 16.0).floor() as i32, (z / 16.0).floor() as i32);
            (-1..=1).any(|dz| (-1..=1).any(|dx| disc.get(&(cx + dx, cz + dz)).map(|v| v.iter().any(|(p, r)| (p[0] - x).powi(2) + (p[1] - z).powi(2) < r * r)).unwrap_or(false)))
        };
        let before = seeds.len();
        seeds.retain(|(s, _)| !cleared(s.x, s.z));
        out.report.push(format!("  vegetation: {} CSP list(s), {} trees; they clear {} card plants", lists.lists, lists.trees.len(), before - seeds.len()));
        for t in &lists.trees {
            seeds.push((Seed { x: t.pos[0], z: t.pos[2], bottom: t.pos[1], top: t.pos[1] + t.height, width: t.width, colour: t.colour, hints: t.model.clone(), csp: true, yaw: Some(t.yaw) }, Vec::new()));
        }
    }
    // ---- drawn ground near the plants (the soil outside the physics area)
    let occupied: std::collections::HashSet<(i32, i32)> = seeds.iter().map(|(s, _)| ((s.x / 32.0).floor() as i32, (s.z / 32.0).floor() as i32)).collect();
    let vis_tris: Vec<Tri> = scan.meshes.par_iter().enumerate().filter(|(i, m)| {
        matches!(m.role, Role::Visual) && class[&m.material].is_none() && !super::materials::transparent(scan, m.material) && scan.mesh(m).lod_in <= 0.0 && *i < usize::MAX
    }).flat_map_iter(|(_, m)| {
        let p = scan.positions(m);
        let occ = &occupied;
        scan.triangles(m).into_iter().filter_map(move |t| {
            let (a, b, c) = (p[t[0] as usize], p[t[1] as usize], p[t[2] as usize]);
            let n = [(b[1] - a[1]) * (c[2] - a[2]) - (b[2] - a[2]) * (c[1] - a[1]), (b[2] - a[2]) * (c[0] - a[0]) - (b[0] - a[0]) * (c[2] - a[2]), (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])];
            let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if l < 1e-8 || (n[1] / l).abs() < 0.5 {
                return None;
            }
            let cx = (a[0] + b[0] + c[0]) / 3.0;
            let cz = (a[2] + b[2] + c[2]) / 3.0;
            occ.contains(&((cx / 32.0).floor() as i32, (cz / 32.0).floor() as i32)).then_some(Tri { v: [a, b, c], surface: 0 })
        }).collect::<Vec<_>>()
    }).collect();
    let vis = Ground::from_triangles(vis_tris, vec![false]);
    // ---- place
    let g = inp.ground;
    let kinds = inp.kinds;
    let wall_hits = std::sync::atomic::AtomicUsize::new(0);
    let bad = |x: f32, y: f32, z: f32| -> bool {
        g.below(x, y, z).map(|h| {
            let w = kinds.get(h.surface) == Some(&Kind::Wall);
            if w && !h.drivable {
                wall_hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            // drivable ground far from every AI line is terrain an AC1
            // track left on the default ROAD key (glensheil), not a road
            (h.drivable && inp.lines.dist(x, z, ROAD_NEAR + 1.0) <= ROAD_NEAR) || w
        }).unwrap_or(false)
    };
    let soil_at = |x: f32, look: f32, z: f32| -> Option<f32> {
        let a = g.below(x, look, z).map(|h| h.y);
        let b = vis.below(x, look, z).map(|h| h.y);
        match (a, b) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        }
    };
    struct Placed {
        seed: usize,
        pos: [f32; 3],
        height: f32,
        width: f32,
    }
    let placed: Vec<Result<Placed, bool>> = seeds.par_iter().enumerate().map(|(si, (s, _))| {
        let look = if s.csp { s.bottom + 3.0 } else { s.bottom + 1.5 };
        let (mut x, mut z) = (s.x, s.z);
        let mut soil = soil_at(x, look, z).unwrap_or(s.bottom);
        let floated = !s.csp && s.bottom - soil > 2.0;
        let height = if s.csp { s.top - s.bottom } else if floated { s.top - s.bottom } else { s.top - soil };
        if height < 0.3 {
            return Err(false);
        }
        let height = height.min(MAX_HEIGHT);
        // footprint off the road and walls
        let r = if height >= TREE_MIN { 0.8 } else { (s.width * 0.25).clamp(0.3, 0.8) };
        let ring = |x: f32, z: f32, y: f32| -> Option<[f32; 2]> {
            let mut away = [0f32; 2];
            let mut hit = false;
            for k in 0..7 {
                let (ox, oz) = if k == 0 { (0.0, 0.0) } else { let a = k as f32 * std::f32::consts::TAU / 6.0; (a.cos() * r, a.sin() * r) };
                if bad(x + ox, y + 1.5, z + oz) {
                    hit = true;
                    away[0] -= ox;
                    away[1] -= oz;
                }
            }
            hit.then_some(away)
        };
        let mut ok = false;
        for _ in 0..9 {
            match ring(x, z, soil) {
                None => {
                    ok = true;
                    break;
                }
                Some(a) => {
                    let mut d = [a[0], a[1]];
                    let l = (d[0] * d[0] + d[1] * d[1]).sqrt();
                    if l < 1e-4 {
                        // centred on the road: away from... the nearest free side
                        let h = hash3([x, 0.0, z]);
                        let ang = super::bakes::unit_of(h) * std::f32::consts::TAU;
                        d = [ang.cos(), ang.sin()];
                    } else {
                        d = [d[0] / l, d[1] / l];
                    }
                    x += d[0];
                    z += d[1];
                    soil = soil_at(x, soil + 2.0, z).unwrap_or(soil);
                }
            }
        }
        if !ok {
            return Err(true);
        }
        Ok(Placed { seed: si, pos: [x, soil, z], height, width: s.width.max(0.5) })
    }).collect();
    let dropped_road = placed.iter().filter(|p| matches!(p, Err(true))).count();
    let too_low = placed.iter().filter(|p| matches!(p, Err(false))).count();
    // dev: ACECONV_VEG_DEBUG=<file> lists the seeds that found no place
    if let Ok(f) = std::env::var("ACECONV_VEG_DEBUG") {
        let mut rows = vec!["x\tz\tbottom\ttop\tsurface\thints".to_string()];
        for (p, (s, _)) in placed.iter().zip(&seeds) {
            if p.is_err() {
                let k = g.below(s.x, s.bottom + 1.5, s.z).map(|h| scan.surfaces[h.surface].key.clone()).unwrap_or_default();
                rows.push(format!("{:.1}\t{:.1}\t{:.1}\t{:.1}\t{k}\t{}", s.x, s.z, s.bottom, s.top, s.hints));
            }
        }
        let _ = std::fs::write(f, rows.join("\n"));
    }
    // ---- species and instances: first the family and the best species of
    // every plant; the most wanted species per family form the track's
    // palette; then each plant picks within the palette
    let mut used_meshes: Vec<bool> = vec![false; scan.meshes.len()];
    let mut grass_cards = 0usize;
    let mut no_species = 0usize;
    struct Want {
        p: Placed,
        fam: Family,
        f: Feat,
        h: u64,
        dist: f32,
    }
    let mut wants: Vec<Want> = Vec::new();
    for p in placed.into_iter().flatten() {
        let (s, members) = &seeds[p.seed];
        let fam = if p.height < GRASS_MAX && !s.csp {
            grass_cards += 1;
            if inp.grass_on {
                for &m in members {
                    used_meshes[frs[m].mesh] = true;
                }
            }
            continue;
        } else if p.height < TREE_MIN {
            Family::Bush
        } else {
            Family::Tree
        };
        let dist = inp.lines.dist(p.pos[0], p.pos[2], 1000.0);
        let f = Feat::from_hints(p.height, p.width, s.colour, &s.hints);
        // palms and dead trees have no impostor of their own: they stay 3D
        let fam = if fam == Family::Tree && dist > IMPOSTOR_FROM && !f.palm && !f.dead && inp.cat.count(Family::Impostor) > 0 { Family::Impostor } else { fam };
        let h = hash3([p.pos[0], 0.0, p.pos[2]]);
        wants.push(Want { p, fam, f, h, dist });
    }
    let mut demand: BTreeMap<(Family, usize), usize> = BTreeMap::new();
    for w in &wants {
        if let Some(sp) = pick(inp.cat, w.fam, &w.f, inp.climate, 0) {
            *demand.entry((w.fam, sp)).or_default() += 1;
        }
    }
    let mut palette: HashMap<Family, Vec<usize>> = HashMap::new();
    for fam in [Family::Tree, Family::Bush, Family::Impostor] {
        let mut v: Vec<(usize, usize)> = demand.iter().filter(|((f, _), _)| *f == fam).map(|((_, sp), n)| (*sp, *n)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        palette.insert(fam, v.iter().take(palette_size(fam)).map(|x| x.0).collect());
    }
    let mut plants: Vec<Plant> = Vec::new();
    for w in wants {
        let Want { p, fam, f, h, dist } = w;
        let (s, members) = &seeds[p.seed];
        let Some(sp) = pick_in(inp.cat, fam, &f, inp.climate, h, palette.get(&fam).map(|v| v.as_slice())) else {
            no_species += 1;
            continue;
        };
        let spc = &inp.cat.species[sp];
        let k = (p.height / spc.height).clamp(0.5, 2.5);
        let wk = (p.width / spc.width / k).clamp(0.8, 1.25);
        let j = |b: u64| 1.0 + ((super::bakes::unit_of(h.rotate_left(b as u32)) - 0.5) * 0.08);
        let card = spc.family == Family::Impostor;
        let yaw = if card { 0.0 } else { s.yaw.unwrap_or_else(|| super::bakes::unit_of(h.rotate_left(17)) * 360.0 - 180.0) };
        let scale = if card { [k; 3] } else { [k * wk * j(5), k, k * wk * j(11)] };
        let wk = if card { 1.0 } else { wk };
        plants.push(Plant {
            species: sp, pos: [p.pos[0], p.pos[1] - SINK, p.pos[2]], yaw, scale,
            height: spc.height * k, width: spc.width * k * wk, csp: s.csp, dist,
        });
        for &m in members {
            used_meshes[frs[m].mesh] = true;
        }
    }
    // ---- drop the replaced cards (meshes with flat fragments stay)
    let mut flat_mesh = vec![false; scan.meshes.len()];
    for f in &frs {
        if f.flat() {
            flat_mesh[f.mesh] = true;
        }
    }
    let mut dropped = 0usize;
    for (i, u) in used_meshes.iter().enumerate() {
        if *u && !flat_mesh[i] {
            out.drop[i] = true;
            dropped += 1;
        }
    }
    // far LOD copies of a dropped card (same model node family): drop when
    // their material is a card material and a near copy of it was dropped
    let dropped_mats: std::collections::HashSet<usize> = (0..scan.meshes.len()).filter(|&i| out.drop[i]).map(|i| scan.meshes[i].material).collect();
    for (i, _) in &far {
        if dropped_mats.contains(&scan.meshes[*i].material) {
            out.drop[*i] = true;
        }
    }
    // ---- cap
    let total = plants.len();
    if plants.len() > PLANT_CAP {
        plants.sort_by(|a, b| a.dist.partial_cmp(&b.dist).unwrap().then(a.pos[0].partial_cmp(&b.pos[0]).unwrap()).then(a.pos[2].partial_cmp(&b.pos[2]).unwrap()));
        plants.truncate(PLANT_CAP);
        out.notes.push(format!("vegetation: {total} plants, the {PLANT_CAP} nearest the track kept"));
    }
    let count = |f: Family| plants.iter().filter(|p| inp.cat.species[p.species].family == f).count();
    out.report.push(format!("  vegetation: {} card meshes ({} far copies), {} fragments ({} forest strips), {} card plants; {} trees, {} bushes, {} impostors; {} card meshes not drawn; {} grass cards{}; {} too low, {} could not be moved off the road ({} wall samples), {} without a species; {:.1} s",
        near.len(), far.len(), nf, strips, card_seeds, count(Family::Tree), count(Family::Bush), count(Family::Impostor), dropped, grass_cards,
        if inp.grass_on { " (left to the carpet)" } else { " kept" }, too_low, dropped_road, wall_hits.load(std::sync::atomic::Ordering::Relaxed), no_species, t0.elapsed().as_secs_f64()));
    let mut mix: BTreeMap<&str, usize> = BTreeMap::new();
    for p in &plants {
        *mix.entry(inp.cat.species[p.species].name.as_str()).or_default() += 1;
    }
    let mut mixv: Vec<(&str, usize)> = mix.into_iter().collect();
    mixv.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    out.report.push(format!("  vegetation: species mix {}", mixv.iter().take(12).map(|(n, c)| format!("{n} {c}")).collect::<Vec<_>>().join(", ")));
    out.plants = plants;
    out
}

/// One ISMesh instance.
#[derive(Clone, Copy, Debug)]
pub struct Inst {
    pub pos: [f32; 3],
    pub rot: [f32; 3],
    pub scale: [f32; 3],
}

/// An ISMesh entity over `inst` (world positions; entity at the origin).
/// `margin` widens the xz box (the mesh's own half width).
pub fn ismesh(name: &str, mesh: &str, inst: &[Inst], cull: f32, margin: f32) -> Entity {
    let mut p = Vec::with_capacity(inst.len() * 3);
    let mut r = Vec::with_capacity(inst.len() * 3);
    let mut s = Vec::with_capacity(inst.len() * 3);
    let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
    for i in inst {
        p.extend_from_slice(&i.pos);
        r.extend_from_slice(&i.rot);
        s.extend_from_slice(&i.scale);
        lo = [lo[0].min(i.pos[0]), lo[1].min(i.pos[2])];
        hi = [hi[0].max(i.pos[0]), hi[1].max(i.pos[2])];
    }
    let mut t = Msg::default();
    t.push(2, Val::Bytes(pb::f32_bytes(&p)));
    t.push(3, Val::Bytes(pb::f32_bytes(&r)));
    t.push(4, Val::Bytes(pb::f32_bytes(&s)));
    let mut d = Msg::default();
    d.set_str(1, mesh);
    d.push(7, Val::Msg(t));
    let mut f10 = Msg::default();
    f10.push(5, Val::Bytes(Vec::new()));
    f10.push(6, Val::Bytes(Vec::new()));
    d.push(10, Val::Msg(f10));
    let mut e = Entity::new(name, "ISMesh", 105, d);
    e.bounds = Some(([lo[0] - margin, lo[1] - margin], [hi[0] + margin, hi[1] + margin]));
    e.active_distance = cull;
    e.distance_override = Some(100000.0);
    e
}

/// The cull distance of a species' nodes: its mesh's lodOut, as stock
/// gives, capped at the stock instance-weighted medians (ours: trees 1200
/// m, impostors 3500 m, bushes 550 m; stock tree nodes run 850..1850,
/// impostors 1500..5500, bushes 350..800).
pub fn cull_of(s: &super::vegcat::Species) -> f32 {
    let c = if s.lod_out > 0.0 { s.lod_out } else { 1000.0 };
    match s.family {
        Family::Tree => c.min(1200.0),
        Family::Impostor => c.min(3500.0),
        Family::Bush => c.min(550.0),
        _ => c,
    }
}

/// Group instances into nodes by species and tile; returns the entities.
pub fn nodes(prefix: &str, cat: &Catalogue, items: &[(usize, Inst)], tile: f32, cap: usize) -> Vec<Entity> {
    let mut groups: BTreeMap<(usize, i32, i32), Vec<Inst>> = BTreeMap::new();
    for (sp, i) in items {
        groups.entry((*sp, (i.pos[0] / tile).floor() as i32, (i.pos[2] / tile).floor() as i32)).or_default().push(*i);
    }
    let mut out = Vec::new();
    for ((sp, tx, tz), v) in groups {
        let s = &cat.species[sp];
        let short: String = s.name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect();
        let smax = v.iter().map(|i| i.scale[0].max(i.scale[2])).fold(1.0f32, f32::max);
        for (k, chunk) in v.chunks(cap.max(1)).enumerate() {
            let name = if k == 0 { format!("{prefix}_{short}_{tx}_{tz}") } else { format!("{prefix}_{short}_{tx}_{tz}_{k}") };
            let cull = cull_of(s);
            out.push(ismesh(&name, &s.path, chunk, cull, s.width * 0.5 * smax));
        }
    }
    out
}

/// The plants' ISMesh entities (main scene).
pub fn entities(plan: &Plan, cat: &Catalogue) -> Vec<Entity> {
    let items: Vec<(usize, Inst)> = plan.plants.iter().map(|p| (p.species, Inst { pos: p.pos, rot: [0.0, p.yaw, 0.0], scale: p.scale })).collect();
    nodes("veg", cat, &items, TILE, NODE_CAP)
}

/// Crown occluders for the bakes: (solid cores, foliage crowns), each a
/// triangle with its albedo.
pub type Occ = Vec<([[f32; 3]; 3], [f32; 3])>;

/// Crown proxy size factor (ours, calibrated: see FORMATS.md).
pub const CANOPY_SCALE: f32 = 1.4;

pub fn canopies(plan: &Plan, cat: &Catalogue) -> (Occ, Occ) {
    let mut solid = Vec::new();
    let mut fol = Vec::new();
    // an octagonal bipyramid: its footprint covers 90 % of the crown's disc
    let crown = |c: [f32; 3], r: [f32; 3], a: [f32; 3], lower_only: bool, out: &mut Occ| {
        let ring: Vec<[f32; 3]> = (0..8).map(|k| {
            let t = k as f32 * std::f32::consts::FRAC_PI_4;
            [c[0] + r[0] * t.cos(), c[1], c[2] + r[2] * t.sin()]
        }).collect();
        let top = [c[0], c[1] + r[1], c[2]];
        let bot = [c[0], c[1] - r[1], c[2]];
        for k in 0..8 {
            let (u, v) = (ring[k], ring[(k + 1) % 8]);
            // counter-clockwise seen from outside
            if !lower_only {
                out.push(([top, v, u], a));
            }
            out.push(([bot, u, v], a));
        }
    };
    // calibration knob (ACECONV_CANOPY_SCALE): crown radii x this
    let k = std::env::var("ACECONV_CANOPY_SCALE").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(CANOPY_SCALE);
    for p in &plan.plants {
        let s = &cat.species[p.species];
        let (cy, ry) = if s.conifer { (0.5, 0.45) } else { (0.62, 0.35) };
        let c = [p.pos[0], p.pos[1] + cy * p.height, p.pos[2]];
        let r = [0.45 * p.width * k, ry * p.height * k.min(1.3), 0.45 * p.width * k];
        crown(c, r, s.colour, false, &mut fol);
        // the solid core is only its lower half: a closed core would make
        // the probe texels inside a crown "enclosed" (irradiance.rs), and
        // the ground below sees only that half anyway
        crown(c, [r[0] * 0.5, r[1] * 0.5, r[2] * 0.5], s.colour, true, &mut solid);
    }
    (solid, fol)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::vegcat::Species;

    fn sp(name: &str, fam: Family, h: f32, w: f32, col: [f32; 3]) -> Species {
        Species { path: format!("x\\{name}.mesh"), name: name.into(), family: fam, height: h, width: w, lod_out: 1200.0, colour: col,
            conifer: name.contains("spruce"), palm: name.contains("palm"), dead: false, climate: super::super::vegcat::affinity(name) }
    }

    #[test]
    fn species_follow_shape_and_climate() {
        let cat = super::super::vegcat::from_species(vec![
            sp("beech", Family::Tree, 12.0, 9.0, [0.06, 0.1, 0.03]),
            sp("spruce", Family::Tree, 21.0, 7.0, [0.04, 0.07, 0.04]),
            sp("palm", Family::Tree, 15.0, 8.0, [0.08, 0.1, 0.04]),
        ]);
        let temperate = super::super::vegcat::climate_of(51.0, true);
        let tall_narrow = Feat::from_hints(20.0, 6.0, [0.04, 0.07, 0.04], "trees1 trees.dds");
        assert_eq!(cat.species[pick(&cat, Family::Tree, &tall_narrow, temperate, 0).unwrap()].name, "spruce");
        let round = Feat::from_hints(11.0, 9.0, [0.06, 0.1, 0.03], "leaves");
        assert_eq!(cat.species[pick(&cat, Family::Tree, &round, temperate, 0).unwrap()].name, "beech");
        let palm = Feat::from_hints(10.0, 9.0, [0.08, 0.1, 0.04], "curly_palm_chq.bin");
        assert_eq!(cat.species[pick(&cat, Family::Tree, &palm, super::super::vegcat::climate_of(3.0, true), 0).unwrap()].name, "palm");
    }

    #[test]
    fn line_index_nearest() {
        let li = LineIndex::new(&[vec![[0.0, 0.0, 0.0], [100.0, 0.0, 0.0]]]);
        assert!((li.dist(100.0, 30.0, 500.0) - 30.0).abs() < 1e-3);
        assert_eq!(li.dist(5000.0, 0.0, 500.0), 500.0);
    }

    #[test]
    fn ismesh_bounds_hold_instances() {
        let inst = [Inst { pos: [1.0, 2.0, 3.0], rot: [0.0; 3], scale: [1.0; 3] }, Inst { pos: [-5.0, 0.0, 9.0], rot: [0.0; 3], scale: [1.0; 3] }];
        let e = ismesh("t", "m.mesh", &inst, 150.0, 0.5);
        let (lo, hi) = e.bounds.unwrap();
        assert!(lo[0] <= -5.0 && hi[0] >= 1.0 && lo[1] <= 3.0 && hi[1] >= 9.0);
        assert_eq!(e.active_distance, 150.0);
    }
}
