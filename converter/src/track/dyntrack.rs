//! `content\tracks\<id>\dynamic_track\<layout>.dynamictrackpresetcompressed`
//! (format decoded from the stock presets; see docs/FORMATS.md):
//!
//! 1. frame min {x, y, z} (f32 fields 1..3, zeros left out);
//! 2. frame max {x, y, z};
//! 3. repeated tile {f1: zlib of 64 x 64 cells x [rubber, grip], f2: tile id};
//! 4. repeated tile, same order {f1: repeated {f1: height f32} x 4096, f2: tile id};
//! 5. packed varint tile ids, ascending.
//!
//! The frame is a square of side E (min.y == max.y); 256 x 256 tiles over
//! it, tile id = row * 256 + col (row along +z, col along +x from min),
//! cell index in a tile = (z % 64) * 64 + (x % 64).

use anyhow::{anyhow, bail, Context, Result};
use std::io::Read;

use crate::io::pb::{self, Msg, Val};

pub const TILES: u32 = 256;
pub const CELLS: usize = 64;
/// rubber, grip of a cell with no road (every stock preset)
pub const EMPTY_CELL: [u8; 2] = [0, 57];
/// height of a cell with no road
pub const EMPTY_HEIGHT: f32 = -9_999_999.0;

#[derive(Clone, Debug)]
pub struct Tile {
    pub id: u32,
    /// 64 x 64 x [rubber, grip]
    pub cells: Vec<u8>,
    /// 64 x 64 road heights
    pub heights: Vec<f32>,
}

#[derive(Clone, Debug, Default)]
pub struct Preset {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub tiles: Vec<Tile>,
}

fn vec3(m: Option<Msg>) -> [f32; 3] {
    let m = m.unwrap_or_default();
    [m.f32(1).unwrap_or(0.0), m.f32(2).unwrap_or(0.0), m.f32(3).unwrap_or(0.0)]
}

/// Parse and check a preset: every tile inflates to 8192 bytes, carries
/// 4096 heights, and the three id lists agree and ascend.
pub fn parse(b: &[u8]) -> Result<Preset> {
    let m = Msg::parse(b).context("not a protobuf")?;
    let mut p = Preset { min: vec3(m.sub(1)), max: vec3(m.sub(2)), tiles: Vec::new() };
    let cells = m.subs(3);
    let heights = m.subs(4);
    let ids: Vec<u64> = m.all(5).flat_map(|v| match v {
        Val::Bytes(b) => pb::varints(b),
        Val::Varint(x) => vec![*x],
        _ => vec![],
    }).collect();
    if cells.len() != heights.len() || cells.len() != ids.len() {
        bail!("{} cell tiles, {} height tiles, {} ids", cells.len(), heights.len(), ids.len());
    }
    if ids.windows(2).any(|w| w[0] >= w[1]) {
        bail!("tile ids are not ascending");
    }
    for (i, (c, h)) in cells.iter().zip(&heights).enumerate() {
        let id = c.varint(2).unwrap_or(0);
        if id != ids[i] || h.varint(2).unwrap_or(0) != id {
            bail!("tile {i}: ids {} / {} / {} disagree", id, h.varint(2).unwrap_or(0), ids[i]);
        }
        if id >= (TILES * TILES) as u64 {
            bail!("tile id {id} outside the 256 x 256 grid");
        }
        let z = c.bytes(1).ok_or_else(|| anyhow!("tile {id}: no cell data"))?;
        let mut raw = Vec::with_capacity(CELLS * CELLS * 2);
        flate2::read::ZlibDecoder::new(z).read_to_end(&mut raw).with_context(|| format!("tile {id}: zlib"))?;
        if raw.len() != CELLS * CELLS * 2 {
            bail!("tile {id}: {} bytes of cells, want 8192", raw.len());
        }
        let hs: Vec<f32> = h.subs(1).iter().map(|x| x.f32(1).unwrap_or(0.0)).collect();
        if hs.len() != CELLS * CELLS {
            bail!("tile {id}: {} heights, want 4096", hs.len());
        }
        p.tiles.push(Tile { id: id as u32, cells: raw, heights: hs });
    }
    Ok(p)
}

/// Encode a preset in the stock field order (tiles sorted by id).
pub fn encode(p: &Preset) -> Result<Vec<u8>> {
    use std::io::Write;
    let v3 = |v: [f32; 3]| {
        let mut m = Msg::default();
        for (i, x) in v.iter().enumerate() {
            if *x != 0.0 {
                m.set_f32(i as u32 + 1, *x);
            }
        }
        m
    };
    let mut tiles: Vec<&Tile> = p.tiles.iter().collect();
    tiles.sort_by_key(|t| t.id);
    let mut m = Msg::default();
    m.push(1, Val::Msg(v3(p.min)));
    m.push(2, Val::Msg(v3(p.max)));
    for t in &tiles {
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(&t.cells)?;
        let mut c = Msg::default();
        c.push(1, Val::Bytes(z.finish()?));
        if t.id != 0 {
            c.push(2, Val::Varint(t.id as u64));
        }
        m.push(3, Val::Msg(c));
    }
    for t in &tiles {
        let mut h = Msg::default();
        for x in &t.heights {
            let mut e = Msg::default();
            if *x != 0.0 {
                e.set_f32(1, *x);
            }
            h.push(1, Val::Msg(e));
        }
        if t.id != 0 {
            h.push(2, Val::Varint(t.id as u64));
        }
        m.push(4, Val::Msg(h));
    }
    let ids: Vec<u32> = tiles.iter().map(|t| t.id).collect();
    m.push(5, Val::Bytes(pb::varint_bytes(&ids)));
    Ok(m.encode())
}

// ---- building a preset (ours) ------------------------------------------
//
// - Frame: centred on the drivable ground's xz box (y: the middle of the
//   ground's height range, rounded); side 6000 m whenever the drivable
//   box fits in it, as every stock circuit but one (see [`frame_for`]).
// - Only ground within 40 m of the ideal or pit line is written (8 m
//   blocks; stock covers about 30 m beside the line), which also bounds
//   the work on huge flat areas.
// - Road cells: every road / kerb / pit / concrete triangle of the layout
//   rasterised at the cell centres (the highest surface wins where two
//   overlap); verge cells: grass / sand / verge / turf ground within 12 m
//   of a road cell (stock writes 15..30 m of verge beside the road).
// - Rubber on road cells only, from the ideal line: full within 1.2 m of
//   it, falling linearly to 0 at 2.8 m (on roads narrower than 11.2 m the
//   band shrinks to a quarter of the width a side), times a level of 140..255 along
//   the lap that grows with braking (deceleration / 10 m/s^2) and
//   cornering load (v^2 / R / 15 m/s^2), smoothed over 20 m, times a
//   per-cell 0.8..1.0 jitter.
// - Grip: road 180 +- 8 (smooth 40 m noise) +- 2, pit 176, concrete 172,
//   kerbs 200, verge 130 +- 15; clamped to 64..255 (stock: road cells
//   64..255, medians over all cells 118..189; verge 86 on brands_hatch,
//   130..160 on monza).

use super::ai::Line;
use super::classes::Kind;
use super::ground::Ground;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub centre: [f32; 3],
    pub extent: f32,
}

impl Frame {
    pub fn min(&self) -> [f32; 3] {
        [self.centre[0] - self.extent / 2.0, self.centre[1], self.centre[2] - self.extent / 2.0]
    }
    pub fn cell(&self) -> f32 {
        self.extent / (TILES as usize * CELLS) as f32
    }
}

/// Stock frame side: 31 of the 36 stock presets (every circuit but nurburgring, whose 5 presets are 6800 m) use
/// 6000 m (`aceconv measure frames`, stockstats/measure_frames.txt).
pub const STOCK_EXTENT: f32 = 6000.0;
/// Above 6000 m (ours, from the one stock example, nurburgring: frame 6800
/// m over a 6109 x 6375 m box of non-empty cells, AI lines 6085 x 6340 m):
/// the side is the drivable span times the nurburgring ratio frame / cell
/// span, rounded up to a multiple of 100 m as the stock side is.
pub const BIG_FACTOR: (f64, f64) = (6800.0, 6375.0);

pub fn frame_for(lo: [f32; 3], hi: [f32; 3]) -> Frame {
    let span = (hi[0] - lo[0]).max(hi[2] - lo[2]);
    let extent = if span <= STOCK_EXTENT { STOCK_EXTENT } else { ((span as f64 * BIG_FACTOR.0 / BIG_FACTOR.1 / 100.0).ceil() * 100.0) as f32 };
    Frame { centre: [((lo[0] + hi[0]) / 2.0).round(), ((lo[1] + hi[1]) / 2.0).round(), ((lo[2] + hi[2]) / 2.0).round()], extent }
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub tiles: usize,
    pub road_cells: usize,
    pub verge_cells: usize,
    /// share of non-empty cells without rubber
    pub rubber_zero: f32,
    pub rubber_max: u8,
    pub grip_road: [u8; 3],
}

const EMPTY: u8 = 0;
const ROAD: u8 = 1;
const KERB: u8 = 2;
const PIT: u8 = 3;
const CONCRETE: u8 = 4;
const VERGE: u8 = 5;

struct Buf {
    cls: Vec<u8>,
    h: Vec<f32>,
}

fn hash2(x: i64, z: i64, seed: u64) -> f32 {
    let mut h = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (z as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F) ^ seed;
    h ^= h >> 31;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 29;
    (h >> 40) as f32 / (1u64 << 24) as f32
}

/// Smooth value noise in -1..1 over a lattice of `scale` metres.
fn noise(x: f32, z: f32, scale: f32, seed: u64) -> f32 {
    let (fx, fz) = (x / scale, z / scale);
    let (ix, iz) = (fx.floor() as i64, fz.floor() as i64);
    let (tx, tz) = (fx - ix as f32, fz - iz as f32);
    let s = |t: f32| t * t * (3.0 - 2.0 * t);
    let (a, b, c, d) = (hash2(ix, iz, seed), hash2(ix + 1, iz, seed), hash2(ix, iz + 1, seed), hash2(ix + 1, iz + 1, seed));
    let top = a + (b - a) * s(tx);
    let bot = c + (d - c) * s(tx);
    (top + (bot - top) * s(tz)) * 2.0 - 1.0
}

/// Blocks of BLOCK metres within REACH of a line (the cells a preset may hold).
const BLOCK: f32 = 8.0;
const REACH: f32 = 40.0;

fn line_mask(lines: &[&Line]) -> HashSet<(i32, i32)> {
    let mut m = HashSet::new();
    let r = (REACH / BLOCK).ceil() as i32;
    for l in lines {
        let mut last: Option<[f32; 3]> = None;
        for s in &l.samples {
            if let Some(q) = last {
                if (q[0] - s.pos[0]).powi(2) + (q[2] - s.pos[2]).powi(2) < (BLOCK * 0.5).powi(2) {
                    continue;
                }
            }
            last = Some(s.pos);
            let (bx, bz) = ((s.pos[0] / BLOCK).floor() as i32, (s.pos[2] / BLOCK).floor() as i32);
            for dz in -r..=r {
                for dx in -r..=r {
                    if (dx * dx + dz * dz) as f32 <= ((REACH / BLOCK) + 1.0).powi(2) {
                        m.insert((bx + dx, bz + dz));
                    }
                }
            }
        }
    }
    m
}

/// Every cell centre of triangle `t` inside the mask blocks, with its height.
fn tri_cells(t: &super::ground::Tri, mn: [f32; 3], cell: f32, ncell: i64, mask: &HashSet<(i32, i32)>, mut f: impl FnMut(i64, i64, f32)) {
    let [a, b, c] = t.v;
    let d = (b[2] - c[2]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[2] - c[2]);
    if d.abs() < 1e-9 {
        return;
    }
    let (lx, hx) = (a[0].min(b[0]).min(c[0]), a[0].max(b[0]).max(c[0]));
    let (lz, hz) = (a[2].min(b[2]).min(c[2]), a[2].max(b[2]).max(c[2]));
    let (bx0, bx1) = ((lx / BLOCK).floor() as i32, (hx / BLOCK).floor() as i32);
    let (bz0, bz1) = ((lz / BLOCK).floor() as i32, (hz / BLOCK).floor() as i32);
    let mut blocks: Vec<(i32, i32)> = if (bx1 - bx0 + 1) as i64 * (bz1 - bz0 + 1) as i64 > mask.len() as i64 {
        mask.iter().copied().filter(|&(x, z)| x >= bx0 && x <= bx1 && z >= bz0 && z <= bz1).collect()
    } else {
        (bz0..=bz1).flat_map(|z| (bx0..=bx1).map(move |x| (x, z))).filter(|k| mask.contains(k)).collect()
    };
    blocks.sort();
    for (bx, bz) in blocks {
        let (ex0, ex1) = ((bx as f32 * BLOCK).max(lx), ((bx + 1) as f32 * BLOCK).min(hx));
        let (ez0, ez1) = ((bz as f32 * BLOCK).max(lz), ((bz + 1) as f32 * BLOCK).min(hz));
        let x0 = ((ex0 - mn[0]) / cell - 0.5).ceil().max(0.0) as i64;
        let x1 = ((ex1 - mn[0]) / cell - 0.5).floor().min(ncell as f32 - 1.0) as i64;
        let z0 = ((ez0 - mn[2]) / cell - 0.5).ceil().max(0.0) as i64;
        let z1 = ((ez1 - mn[2]) / cell - 0.5).floor().min(ncell as f32 - 1.0) as i64;
        for cz in z0..=z1 {
            let z = mn[2] + (cz as f32 + 0.5) * cell;
            if z < bz as f32 * BLOCK || z >= (bz + 1) as f32 * BLOCK {
                continue;
            }
            for cx in x0..=x1 {
                let x = mn[0] + (cx as f32 + 0.5) * cell;
                if x < bx as f32 * BLOCK || x >= (bx + 1) as f32 * BLOCK {
                    continue;
                }
                let l1 = ((b[2] - c[2]) * (x - c[0]) + (c[0] - b[0]) * (z - c[2])) / d;
                let l2 = ((c[2] - a[2]) * (x - c[0]) + (a[0] - c[0]) * (z - c[2])) / d;
                let l3 = 1.0 - l1 - l2;
                if l1 >= -1e-6 && l2 >= -1e-6 && l3 >= -1e-6 {
                    f(cx, cz, l1 * a[1] + l2 * b[1] + l3 * c[1]);
                }
            }
        }
    }
}

fn put_cell(tiles: &mut HashMap<u32, Buf>, cx: i64, cz: i64, y: f32, c: u8, only_empty: bool) {
    let id = (cz / CELLS as i64) as u32 * TILES + (cx / CELLS as i64) as u32;
    let k = (cz as usize % CELLS) * CELLS + cx as usize % CELLS;
    let t = tiles.entry(id).or_insert_with(|| Buf { cls: vec![EMPTY; CELLS * CELLS], h: vec![EMPTY_HEIGHT; CELLS * CELLS] });
    if only_empty {
        if t.cls[k] == EMPTY {
            t.cls[k] = c;
            t.h[k] = y;
        }
    } else if t.cls[k] == EMPTY || t.cls[k] == VERGE || y > t.h[k] {
        t.cls[k] = c;
        t.h[k] = y;
    }
}

/// Build the preset of one layout.
/// `lines[0]` is the ideal line (rubber); every line (ideal, pit lane)
/// bounds the cells: only ground within 40 m of one is written.
pub fn build(g: &Ground, kinds: &[Kind], lines: &[&Line], frame: &Frame) -> (Preset, Stats) {
    let line = lines[0];
    let mask = line_mask(lines);
    let mn = frame.min();
    let cell = frame.cell();
    let ncell = (TILES as usize * CELLS) as i64;
    let mut tiles: HashMap<u32, Buf> = HashMap::new();
    let class_of = |s: u16| match kinds.get(s as usize).copied().unwrap_or(Kind::Road) {
        Kind::Road => ROAD,
        Kind::Kerb | Kind::FlatKerb => KERB,
        Kind::Pit => PIT,
        Kind::Concrete => CONCRETE,
        Kind::Grass | Kind::Sand | Kind::Verge | Kind::Turf => VERGE,
        Kind::Wall => EMPTY,
    };
    // road cells, and a 2 m mask of everything within VERGE_M (12 m) of them
    const COARSE: f32 = 2.0;
    const VERGE_M: f32 = 12.0;
    let r = (VERGE_M / COARSE).ceil() as i32;
    let mut road_coarse: HashSet<(i32, i32)> = HashSet::new();
    for t in g.triangles() {
        let c = class_of(t.surface);
        if c == EMPTY || c == VERGE {
            continue;
        }
        tri_cells(t, mn, cell, ncell, &mask, |cx, cz, y| {
            put_cell(&mut tiles, cx, cz, y, c, false);
            let x = mn[0] + (cx as f32 + 0.5) * cell;
            let z = mn[2] + (cz as f32 + 0.5) * cell;
            road_coarse.insert(((x / COARSE).floor() as i32, (z / COARSE).floor() as i32));
        });
    }
    let mut near: HashSet<(i32, i32)> = HashSet::with_capacity(road_coarse.len() * 4);
    for &(x, z) in &road_coarse {
        for dz in -r..=r {
            for dx in -r..=r {
                if dx * dx + dz * dz <= r * r {
                    near.insert((x + dx, z + dz));
                }
            }
        }
    }
    // verge cells inside the mask
    for t in g.triangles() {
        if class_of(t.surface) != VERGE {
            continue;
        }
        let [a, b, c] = t.v;
        let (lx, hx) = (a[0].min(b[0]).min(c[0]), a[0].max(b[0]).max(c[0]));
        let (lz, hz) = (a[2].min(b[2]).min(c[2]), a[2].max(b[2]).max(c[2]));
        let (x0, x1, z0, z1) = ((lx / COARSE).floor() as i32, (hx / COARSE).floor() as i32, (lz / COARSE).floor() as i32, (hz / COARSE).floor() as i32);
        let any = if (x1 - x0 + 1) as i64 * (z1 - z0 + 1) as i64 > near.len() as i64 { true } else { (x0..=x1).any(|x| (z0..=z1).any(|z| near.contains(&(x, z)))) };
        if !any {
            continue;
        }
        tri_cells(t, mn, cell, ncell, &mask, |cx, cz, y| {
            let x = mn[0] + (cx as f32 + 0.5) * cell;
            let z = mn[2] + (cz as f32 + 0.5) * cell;
            if near.contains(&((x / COARSE).floor() as i32, (z / COARSE).floor() as i32)) {
                put_cell(&mut tiles, cx, cz, y, VERGE, true);
            }
        });
    }
    // rubber level along the line
    let n = line.samples.len();
    let mut level = vec![140.0f32; n];
    if n > 2 {
        let raw: Vec<f32> = (0..n).map(|i| {
            let s = &line.samples[i];
            let j = if i + 1 < n { i + 1 } else if line.closed { 0 } else { i };
            let ds = (line.samples[j].dist - s.dist).abs().max(0.1);
            let decel = ((s.speed - line.samples[j].speed) / ds * s.speed).max(0.0);
            let lat = s.speed * s.speed / s.radius.abs().max(1.0);
            (decel / 10.0).max(lat / 15.0).clamp(0.0, 1.0)
        }).collect();
        let step = (line.length / n as f32).max(0.1);
        let w = ((20.0 / step) as isize).max(1);
        for i in 0..n {
            let mut acc = 0.0;
            let mut cnt = 0.0;
            for d in -w..=w {
                let k = i as isize + d;
                let k = if line.closed { k.rem_euclid(n as isize) } else if k < 0 || k >= n as isize { continue } else { k } as usize;
                acc += raw[k];
                cnt += 1.0;
            }
            level[i] = 140.0 + 115.0 * (acc / cnt);
        }
    }
    // line samples in an 8 m hash
    const HB: f32 = 8.0;
    let mut hash: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
    for (i, s) in line.samples.iter().enumerate() {
        hash.entry(((s.pos[0] / HB).floor() as i32, (s.pos[2] / HB).floor() as i32)).or_default().push(i as u32);
    }
    let next = |i: usize, back: bool| -> Option<usize> {
        if back {
            if i > 0 { Some(i - 1) } else if line.closed && n > 1 { Some(n - 1) } else { None }
        } else if i + 1 < n {
            Some(i + 1)
        } else if line.closed && n > 1 {
            Some(0)
        } else {
            None
        }
    };
    let line_dist = |x: f32, z: f32, y: f32| -> Option<(f32, usize)> {
        let (hx, hz) = ((x / HB).floor() as i32, (z / HB).floor() as i32);
        let mut best: Option<(f32, usize)> = None;
        for dz in -1..=1 {
            for dx in -1..=1 {
                let Some(v) = hash.get(&(hx + dx, hz + dz)) else { continue };
                for &i in v {
                    let i = i as usize;
                    let a = line.samples[i].pos;
                    if (a[1] - y).abs() > 4.0 {
                        continue; // another level (a bridge)
                    }
                    for j in [next(i, false), next(i, true)].into_iter().flatten() {
                        let b = line.samples[j].pos;
                        let (ux, uz) = (b[0] - a[0], b[2] - a[2]);
                        let l2 = ux * ux + uz * uz;
                        let t = if l2 > 0.0 { (((x - a[0]) * ux + (z - a[2]) * uz) / l2).clamp(0.0, 1.0) } else { 0.0 };
                        let d = ((a[0] + ux * t - x).powi(2) + (a[2] + uz * t - z).powi(2)).sqrt();
                        if best.map(|b| d < b.0).unwrap_or(true) {
                            best = Some((d, i));
                        }
                    }
                }
            }
        }
        best
    };
    let mut ids: Vec<u32> = tiles.keys().copied().collect();
    ids.sort();
    let mut p = Preset { min: mn, max: [mn[0] + frame.extent, mn[1], mn[2] + frame.extent], tiles: Vec::with_capacity(ids.len()) };
    let mut st = Stats { tiles: ids.len(), ..Default::default() };
    let mut road_grip: Vec<u8> = Vec::new();
    let mut zero = 0usize;
    let mut filled = 0usize;
    for id in ids {
        let b = tiles.remove(&id).unwrap();
        let (row, col) = ((id / TILES) as i64, (id % TILES) as i64);
        let mut cells = vec![0u8; CELLS * CELLS * 2];
        for k in 0..CELLS * CELLS {
            let c = b.cls[k];
            if c == EMPTY {
                cells[2 * k..2 * k + 2].copy_from_slice(&EMPTY_CELL);
                continue;
            }
            filled += 1;
            let cx = col * CELLS as i64 + (k % CELLS) as i64;
            let cz = row * CELLS as i64 + (k / CELLS) as i64;
            let x = mn[0] + (cx as f32 + 0.5) * cell;
            let z = mn[2] + (cz as f32 + 0.5) * cell;
            let jitter = hash2(cx, cz, 7);
            let mut rubber = 0u8;
            if c == ROAD || c == PIT || c == CONCRETE {
                if let Some((d, i)) = line_dist(x, z, b.h[k]) {
                    // narrow roads: the band shrinks to a quarter of the width a side
                    let w = line.samples[i].sides[0] + line.samples[i].sides[1];
                    let outer = (0.25 * w).clamp(1.0, 2.8);
                    let inner = outer * 1.2 / 2.8;
                    let f = if d <= inner { 1.0 } else if d < outer { (outer - d) / (outer - inner) } else { 0.0 };
                    if f > 0.0 {
                        rubber = (level[i] * f * (0.8 + 0.2 * jitter)).round().clamp(0.0, 255.0) as u8;
                    }
                }
            }
            let nz = noise(x, z, 40.0, 11);
            let grip = match c {
                ROAD => 180.0 + 8.0 * nz + 4.0 * (jitter - 0.5),
                PIT => 176.0 + 4.0 * nz,
                CONCRETE => 172.0 + 4.0 * nz,
                KERB => 200.0,
                _ => 130.0 + 15.0 * nz,
            }
            .round()
            .clamp(64.0, 255.0) as u8;
            if c == VERGE {
                st.verge_cells += 1;
            } else {
                st.road_cells += 1;
                road_grip.push(grip);
            }
            if rubber == 0 {
                zero += 1;
            }
            st.rubber_max = st.rubber_max.max(rubber);
            cells[2 * k] = rubber;
            cells[2 * k + 1] = grip;
        }
        p.tiles.push(Tile { id, cells, heights: b.h });
    }
    road_grip.sort();
    if !road_grip.is_empty() {
        let q = |f: f32| road_grip[((road_grip.len() - 1) as f32 * f) as usize];
        st.grip_road = [q(0.1), q(0.5), q(0.9)];
    }
    st.rubber_zero = zero as f32 / filled.max(1) as f32;
    (p, st)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let mut cells = vec![0u8; 8192];
        for c in cells.chunks_exact_mut(2) {
            c.copy_from_slice(&EMPTY_CELL);
        }
        cells[100] = 250;
        let p = Preset {
            min: [-3000.0, 0.0, -3000.0],
            max: [3000.0, 0.0, 3000.0],
            tiles: vec![
                Tile { id: 0, cells: cells.clone(), heights: vec![EMPTY_HEIGHT; 4096] },
                Tile { id: 513, cells, heights: vec![12.5; 4096] },
            ],
        };
        let b = encode(&p).unwrap();
        let q = parse(&b).unwrap();
        assert_eq!(q.tiles.len(), 2);
        assert_eq!(q.tiles[1].id, 513);
        assert_eq!(q.tiles[0].cells[100], 250);
        assert_eq!(q.tiles[1].heights[7], 12.5);
        assert_eq!(q.min, p.min);
    }

    #[test]
    fn builds_a_road() {
        use crate::track::classes::Kind;
        use crate::track::ground::{Ground, Tri};
        // a 12 m road along x, grass beside it
        let mut t = Vec::new();
        let quad = |x0: f32, x1: f32, z0: f32, z1: f32, s: u16, t: &mut Vec<Tri>| {
            t.push(Tri { v: [[x0, 5.0, z0], [x1, 5.0, z0], [x1, 5.0, z1]], surface: s });
            t.push(Tri { v: [[x0, 5.0, z0], [x1, 5.0, z1], [x0, 5.0, z1]], surface: s });
        };
        quad(-200.0, 200.0, -6.0, 6.0, 0, &mut t);
        quad(-200.0, 200.0, 6.0, 40.0, 1, &mut t);
        let g = Ground::from_triangles(t, vec![true, true]);
        let kinds = [Kind::Road, Kind::Grass];
        let pts: Vec<[f32; 3]> = (0..200).map(|i| [-190.0 + i as f32 * 2.0, 5.3, 0.0]).collect();
        let line = crate::track::ai::build(&pts, false, 1.7, &g, &kinds, false, None);
        let f = frame_for([-200.0, 5.0, -6.0], [200.0, 5.0, 40.0]);
        assert_eq!(f.extent, 6000.0);
        // a box that fits keeps 6000; nurburgring's cell box gives its 6800
        assert_eq!(frame_for([0.0; 3], [5990.0, 0.0, 10.0]).extent, 6000.0);
        assert_eq!(frame_for([0.0; 3], [6109.0, 0.0, 6375.0]).extent, 6800.0);
        let (p, st) = build(&g, &kinds, &[&line], &f);
        assert!(st.road_cells > 0 && st.verge_cells > 0 && st.rubber_max > 100, "{st:?}");
        assert!(st.rubber_zero > 0.5 && st.rubber_zero < 0.95, "{}", st.rubber_zero);
        let q = parse(&encode(&p).unwrap()).unwrap();
        assert_eq!(q.tiles.len(), st.tiles);
        assert!(q.tiles.iter().all(|t| t.heights.iter().all(|h| *h == EMPTY_HEIGHT || (*h - 5.0).abs() < 1e-4)));
    }
}
