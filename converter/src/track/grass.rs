//! The grass carpet (requirement track-grass-carpet): the game's 3D grass
//! tufts on the AC1 grass ground near the track.
//!
//! Game facts (stock containers `ground_grass*.scene`, `ground_vegetation*.
//! scene`, loaded with the scene and listed in no row; measured with the
//! clean P9 density tool against the stock ideal lines and their side
//! widths): the carpet is ISMesh nodes of `low\grass\tracks_terrain_
//! vegetation` tufts (grass_2_b4 cull 150 m, grass_2_b3 cull 120 m), yaw
//! plus a tilt to the ground, scales xz 0.5..1.8 with y 0.55..0.75 of it.
//! Instances per m² of covered grass ground (4 m cells holding a tuft)
//! against the distance to the road edge, brands_hatch / imola / spa /
//! monza: 0..2 m 5.6 / 10.3 / 9.0 / 1.4, 5..10 m 4.3 / 6.9 / 7.2 / 1.1,
//! 10..20 m 3.5 / 6.0 / 5.8 / 0.9, 20..35 m 1.8 / 2.1 / 3.3 / 0.6, 35..50 m
//! 1.3 / 1.1 / 2.6 / 0.4, 50..75 m 0.8 / 0.55 / 1.6 / 0.24, 75..100 m 0.47 /
//! 0.24 / 0.75 / 0.29, 100..150 m 0.4 / 0.19 / 0.38 / 0.23, none beyond
//! 150..200 m; totals 0.84M / 1.2M / 1.33M / 0.26M.
//!
//! Ours:
//! - a 2 m raster over the drivable area + [`REACH`]: each cell is road
//!   (a drivable physics surface: road, kerb, pit, verge), grass (the grass
//!   kind, `classes::surface_kind`) or other, from the top-most physics
//!   surface; where no physics mesh lies, drawn grass (an opaque material
//!   on grass physics elsewhere, or named grass / lawn / terrain, not sand
//!   or gravel) counts as grass; distance to the road by a two-pass chamfer
//!   transform;
//! - cells within [`CORRIDOR`] of an AI line get none;
//! - density [`density`] (per m² of grass, the brands_hatch profile: the
//!   calibration twin); over twice [`BUDGET`] the reach shrinks (x0.7 steps,
//!   down to 40 m), then the rest is thinned evenly to the budget;
//! - per cell, the count is drawn (fraction by hash) and placed on jittered
//!   strata of the cell (sqrt(k) x sqrt(k), a hash-shuffled subset), so no
//!   rows show; every tuft is checked on the ground index: grass under it,
//!   slope under 57 degrees, no drivable ground within 0.45 m (cells near
//!   the road), and no drawn opaque geometry 0.3..30 m above it (barriers,
//!   buildings, stands);
//! - tufts: the two catalogue grass meshes closest in colour to the AC1
//!   grass (the one with the longer cull first); the second takes a third
//!   of the tufts within 35 m; yaw from the hash, tilted to the ground
//!   normal; scale xz 0.8..1.6, y 0.6..0.8 of it; 2 cm sunk;
//! - nodes per mesh and [`TILE`] m tile, at most [`NODE_CAP`] each, in the
//!   containers `decor_grass_<n>` of at most [`CONTAINER_CAP`] tufts
//!   (loaded with the scene, as stock);
//! - seeds: SplitMix64 of FNV-1a of (track id, cell); deterministic.

use rayon::prelude::*;
use std::collections::HashSet;

use super::classes::Kind;
use super::csptrees::{fnv, splitmix, uniform};
use super::ground::{Ground, Tri};
use super::scan::{Role, Scan};
use super::scene::Entity;
use super::vegcat::{Catalogue, Family};
use super::vegetation::{nodes, Inst};

pub const CELL: f32 = 2.0;
/// Grass reaches this far from the road (ours; stock none past 150..200 m).
pub const REACH: f32 = 150.0;
/// No tufts this close to an AI line (ours).
pub const CORRIDOR: f32 = 3.5;
/// Most tufts per track (ours: the stock spa / imola totals).
pub const BUDGET: usize = 1_300_000;
/// Node tile edge and cap (ours; the largest stock node holds 1.13M).
pub const TILE: f32 = 512.0;
pub const NODE_CAP: usize = 250_000;
pub const CONTAINER: &str = "decor_grass";
/// Most tufts per container (ours: about 11 MB of scene each).
pub const CONTAINER_CAP: usize = 300_000;

/// Split the nodes into containers of at most CONTAINER_CAP tufts (whole
/// nodes, in order).
pub fn containers(nodes: Vec<Entity>) -> Vec<Vec<Entity>> {
    let mut out: Vec<Vec<Entity>> = Vec::new();
    let mut n = 0usize;
    for e in nodes {
        let k = e.actor.1.sub(7).and_then(|t| t.bytes(2).map(|b| b.len() / 12)).unwrap_or(0);
        if out.is_empty() || n + k > CONTAINER_CAP {
            out.push(Vec::new());
            n = 0;
        }
        n += k;
        out.last_mut().unwrap().push(e);
    }
    out
}

/// Tufts per m² of grass at `d` metres from the road edge (ours, the
/// brands_hatch profile).
pub fn density(d: f32) -> f32 {
    const P: [(f32, f32); 10] = [(0.0, 5.5), (2.0, 5.0), (5.0, 4.5), (10.0, 3.8), (20.0, 2.6), (35.0, 1.6), (50.0, 1.1), (75.0, 0.6), (100.0, 0.4), (150.0, 0.0)];
    if d <= 0.0 {
        return P[0].1;
    }
    for w in P.windows(2) {
        if d <= w[1].0 {
            let t = (d - w[0].0) / (w[1].0 - w[0].0);
            return w[0].1 + (w[1].1 - w[0].1) * t;
        }
    }
    0.0
}

pub struct Inputs<'a> {
    pub scan: &'a Scan<'a>,
    pub cat: &'a Catalogue,
    pub ground: &'a Ground,
    pub kinds: &'a [Kind],
    /// AI line points (every layout)
    pub lines: &'a [Vec<[f32; 3]>],
    pub track_id: &'a str,
    /// scan meshes not drawn (replaced cards): not cover
    pub dropped: &'a [bool],
}

#[derive(Default)]
pub struct Carpet {
    pub entities: Vec<Entity>,
    pub report: Vec<String>,
    pub notes: Vec<String>,
    pub instances: usize,
}

const ROAD: u8 = 1;
const GRASS: u8 = 2;
const OTHER: u8 = 3;
/// grass drawn where no physics mesh lies (AC1 terrain outside the physics)
const VGRASS: u8 = 4;

fn is_grass(c: u8) -> bool {
    c == GRASS || c == VGRASS
}

/// A drawn AC1 material that shows grass (not sand / gravel / dirt).
fn grass_material(scan: &Scan, g: usize, cls: &super::classes::MatClass) -> bool {
    if super::materials::transparent(scan, g) || super::vegetation::card_class(scan, g).is_some() {
        return false;
    }
    match cls.kind {
        Some(k) => k == Kind::Grass,
        None => {
            let m = scan.material(g);
            let t = format!("{} {}", m.name, m.texture("txDiffuse").unwrap_or("")).to_uppercase();
            ["GRASS", "ERBA", "LAWN", "TERRAIN", "MEADOW", "FIELD"].iter().any(|w| t.contains(w))
                && !["SAND", "GRAVEL", "GHIAIA", "SABBIA", "DIRT", "MUD", "ASPH", "ROAD"].iter().any(|w| t.contains(w))
        }
    }
}

fn mean_colour(scan: &Scan, mats: &[usize]) -> Option<[f32; 3]> {
    let v: Vec<[f32; 3]> = mats.par_iter().filter_map(|&g| {
        let t = scan.material_texture(g, "txDiffuse")?;
        let img = crate::io::texture::decode_any(&scan.texture_bytes(t)).ok()?;
        let s = if img.w > 64 { crate::io::texture::resize_to(&img, 64, true) } else { img };
        let px: Vec<[u8; 4]> = s.px.chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]]).collect();
        super::vegcat::opaque_mean(&px, true)
    }).collect();
    (!v.is_empty()).then(|| {
        let n = v.len() as f32;
        [0, 1, 2].map(|k| v.iter().map(|c| c[k]).sum::<f32>() / n)
    })
}

/// Seed the carpet.
pub fn build(inp: &Inputs) -> Carpet {
    let t0 = std::time::Instant::now();
    let mut out = Carpet::default();
    let g = inp.ground;
    let kinds = inp.kinds;
    // ---- tuft meshes
    let ac1_grass: Vec<usize> = {
        let mut v: Vec<usize> = inp.scan.meshes.iter().filter_map(|m| match m.role {
            Role::Physics { surface, drawn: true } if kinds.get(surface) == Some(&Kind::Grass) => Some(m.material),
            _ => None,
        }).collect();
        v.sort();
        v.dedup();
        v
    };
    // grey tufts take their colour from the pigment map (Pigment_Enable):
    // those first, the longer cull first; else the closest in colour
    let want = mean_colour(inp.scan, &ac1_grass).unwrap_or([0.08, 0.12, 0.05]);
    let chroma = |c: [f32; 3]| -> [f32; 3] { let s = (c[0] + c[1] + c[2]).max(1e-4); [c[0] / s, c[1] / s, c[2] / s] };
    let grey = |c: [f32; 3]| { let k = chroma(c); (k[0] - 1.0 / 3.0).abs() + (k[1] - 1.0 / 3.0).abs() + (k[2] - 1.0 / 3.0).abs() < 0.03 };
    let cdist = |c: [f32; 3]| -> f32 { let (a, b) = (chroma(want), chroma(c)); (0..3).map(|k| (a[k] - b[k]).abs()).sum::<f32>() };
    let mut tufts: Vec<(usize, i32, f32)> = inp.cat.of(Family::Grass).map(|(i, s)| (i, if grey(s.colour) { 0 } else { 1 }, if grey(s.colour) { -s.lod_out } else { cdist(s.colour) })).collect();
    tufts.sort_by(|a, b| a.1.cmp(&b.1).then(a.2.partial_cmp(&b.2).unwrap()).then(a.0.cmp(&b.0)));
    let Some(&(a, _, _)) = tufts.first() else {
        out.notes.push("grass carpet: no grass tufts in the catalogue".into());
        return out;
    };
    let b = tufts.get(1).map(|t| t.0);
    // ---- raster
    if g.is_empty() {
        return out;
    }
    let (dlo, dhi) = g.drivable_bounds();
    let (glo, ghi) = g.bounds();
    if !(dlo[0].is_finite() && dhi[0] > dlo[0]) {
        return out;
    }
    let lo = [(dlo[0] - REACH).max(glo[0]), (dlo[1] - REACH).max(glo[2])];
    let hi = [(dhi[0] + REACH).min(ghi[0]), (dhi[1] + REACH).min(ghi[2])];
    let mut cell = CELL;
    while ((hi[0] - lo[0]) / cell) as f64 * ((hi[1] - lo[1]) / cell) as f64 > 30.0e6 {
        cell *= 1.25;
    }
    let nx = ((hi[0] - lo[0]) / cell).ceil().max(1.0) as usize;
    let nz = ((hi[1] - lo[1]) / cell).ceil().max(1.0) as usize;
    let ytop = ghi[1] + 10.0;
    // drawn grass outside the physics meshes
    let scan = inp.scan;
    let classes = super::classes::material_classes(scan);
    let vg_tris: Vec<Tri> = scan.meshes.par_iter().enumerate().filter(|(i, m)| {
        matches!(m.role, Role::Visual) && !inp.dropped.get(*i).copied().unwrap_or(false) && scan.mesh(m).lod_in <= 0.0 && grass_material(scan, m.material, &classes[m.material])
    }).flat_map_iter(|(_, m)| {
        let p = scan.positions(m);
        scan.triangles(m).into_iter().filter_map(move |t| {
            let v = [p[t[0] as usize], p[t[1] as usize], p[t[2] as usize]];
            let cx = (v[0][0] + v[1][0] + v[2][0]) / 3.0;
            let cz = (v[0][2] + v[1][2] + v[2][2]) / 3.0;
            (cx >= lo[0] && cx <= hi[0] && cz >= lo[1] && cz <= hi[1]).then_some(Tri { v, surface: 0 })
        }).collect::<Vec<_>>()
    }).collect();
    let vg = Ground::from_triangles(vg_tris, vec![false]);
    let vgr = &vg;
    let ytop = ytop.max(vg.bounds().1[1] + 10.0);
    let cells: Vec<(u8, f32)> = (0..nz).into_par_iter().flat_map_iter(|j| {
        (0..nx).map(move |i| {
            let (x, z) = (lo[0] + (i as f32 + 0.5) * cell, lo[1] + (j as f32 + 0.5) * cell);
            match g.below(x, ytop, z) {
                None => match vgr.below(x, ytop, z) {
                    Some(h) if h.normal[1] >= 0.55 => (VGRASS, h.y),
                    _ => (0, 0.0),
                },
                Some(h) if h.drivable => (ROAD, h.y),
                Some(h) if kinds.get(h.surface) == Some(&Kind::Grass) => (GRASS, h.y),
                Some(h) => (OTHER, h.y),
            }
        }).collect::<Vec<_>>()
    }).collect();
    // ---- distance to the road (chamfer 1 / sqrt 2)
    let mut d = vec![f32::INFINITY; nx * nz];
    for (k, c) in cells.iter().enumerate() {
        if c.0 == ROAD {
            d[k] = 0.0;
        }
    }
    let (s1, s2) = (cell, cell * std::f32::consts::SQRT_2);
    for j in 0..nz {
        for i in 0..nx {
            let k = j * nx + i;
            let mut v = d[k];
            if i > 0 { v = v.min(d[k - 1] + s1); }
            if j > 0 {
                v = v.min(d[k - nx] + s1);
                if i > 0 { v = v.min(d[k - nx - 1] + s2); }
                if i + 1 < nx { v = v.min(d[k - nx + 1] + s2); }
            }
            d[k] = v;
        }
    }
    for j in (0..nz).rev() {
        for i in (0..nx).rev() {
            let k = j * nx + i;
            let mut v = d[k];
            if i + 1 < nx { v = v.min(d[k + 1] + s1); }
            if j + 1 < nz {
                v = v.min(d[k + nx] + s1);
                if i + 1 < nx { v = v.min(d[k + nx + 1] + s2); }
                if i > 0 { v = v.min(d[k + nx - 1] + s2); }
            }
            d[k] = v;
        }
    }
    // the cell centre lies half a cell from the edge it is measured to
    // ---- AI-line corridor
    let mut corridor = vec![false; nx * nz];
    let r = (CORRIDOR / cell).ceil() as i64;
    for l in inp.lines {
        for p in l {
            let (ci, cj) = (((p[0] - lo[0]) / cell).floor() as i64, ((p[2] - lo[1]) / cell).floor() as i64);
            for dj in -r..=r {
                for di in -r..=r {
                    let (i, j) = (ci + di, cj + dj);
                    if i < 0 || j < 0 || i >= nx as i64 || j >= nz as i64 {
                        continue;
                    }
                    let (x, z) = (lo[0] + (i as f32 + 0.5) * cell, lo[1] + (j as f32 + 0.5) * cell);
                    if (x - p[0]).powi(2) + (z - p[2]).powi(2) <= CORRIDOR * CORRIDOR {
                        corridor[j as usize * nx + i as usize] = true;
                    }
                }
            }
        }
    }
    // ---- expected count and thinning
    let area = cell * cell;
    let mut band_area = [0f64; 9];
    const BANDS: [f32; 10] = [0.0, 2.0, 5.0, 10.0, 20.0, 35.0, 50.0, 75.0, 100.0, 150.0];
    let band = |dd: f32| BANDS.windows(2).position(|w| dd >= w[0] && dd < w[1]);
    // over the budget, the reach shrinks first (the verge matters most),
    // then the rest is thinned evenly
    let expected_to = |reach: f32| -> f64 {
        (0..nx * nz).filter(|&k| is_grass(cells[k].0) && !corridor[k] && d[k] < reach).map(|k| (density((d[k] - cell * 0.5).max(0.0)) * area) as f64).sum()
    };
    let mut reach = REACH;
    let mut expected = expected_to(reach);
    while expected > 2.0 * BUDGET as f64 && reach > 40.0 {
        reach = (reach * 0.7).max(40.0);
        expected = expected_to(reach);
    }
    for k in 0..nx * nz {
        if is_grass(cells[k].0) && !corridor[k] && d[k] < reach {
            if let Some(bi) = band((d[k] - cell * 0.5).max(0.0)) {
                band_area[bi] += area as f64;
            }
        }
    }
    let thin = if expected > BUDGET as f64 { (BUDGET as f64 / expected) as f32 } else { 1.0 };
    // ---- cover: drawn opaque geometry over the grass band (16 m cells)
    let occ: HashSet<(i32, i32)> = (0..nx * nz).filter(|&k| is_grass(cells[k].0) && d[k] < reach).map(|k| {
        let (i, j) = (k % nx, k / nx);
        (((lo[0] + i as f32 * cell) / 16.0).floor() as i32, ((lo[1] + j as f32 * cell) / 16.0).floor() as i32)
    }).collect();
    let cover_tris: Vec<Tri> = scan.meshes.par_iter().enumerate().filter(|(i, m)| {
        matches!(m.role, Role::Visual) && !inp.dropped.get(*i).copied().unwrap_or(false) && !super::materials::transparent(scan, m.material) && scan.mesh(m).lod_in <= 0.0
    }).flat_map_iter(|(_, m)| {
        let p = scan.positions(m);
        let occ = &occ;
        scan.triangles(m).into_iter().filter_map(move |t| {
            let v = [p[t[0] as usize], p[t[1] as usize], p[t[2] as usize]];
            let cx = (v[0][0] + v[1][0] + v[2][0]) / 3.0;
            let cz = (v[0][2] + v[1][2] + v[2][2]) / 3.0;
            occ.contains(&((cx / 16.0).floor() as i32, (cz / 16.0).floor() as i32)).then_some(Tri { v, surface: 0 })
        }).collect::<Vec<_>>()
    }).collect();
    let cover = Ground::from_triangles(cover_tris, vec![false]);
    // ---- seed
    let seed0 = fnv(inp.track_id);
    let rows: Vec<Vec<(usize, Inst, f32)>> = (0..nz).into_par_iter().map(|j| {
        let mut v = Vec::new();
        for i in 0..nx {
            let k = j * nx + i;
            if !is_grass(cells[k].0) || corridor[k] || d[k] >= reach {
                continue;
            }
            let dd = (d[k] - cell * 0.5).max(0.0);
            let n = density(dd) * area * thin;
            let mut st = seed0 ^ (k as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
            splitmix(&mut st);
            let mut cnt = n.floor() as usize;
            if uniform(&mut st) < n - n.floor() {
                cnt += 1;
            }
            if cnt == 0 {
                continue;
            }
            let m = (cnt as f32).sqrt().ceil() as usize;
            let mut strata: Vec<usize> = (0..m * m).collect();
            for s in 0..cnt.min(strata.len()) {
                let r = s + (splitmix(&mut st) % (strata.len() - s) as u64) as usize;
                strata.swap(s, r);
            }
            let (x0, z0) = (lo[0] + i as f32 * cell, lo[1] + j as f32 * cell);
            for &sidx in strata.iter().take(cnt) {
                let (sx, sz) = (sidx % m, sidx / m);
                let x = x0 + (sx as f32 + uniform(&mut st)) / m as f32 * cell;
                let z = z0 + (sz as f32 + uniform(&mut st)) / m as f32 * cell;
                let yaw = uniform(&mut st) * std::f32::consts::TAU;
                let s = 0.8 + 0.8 * uniform(&mut st);
                let sy = s * (0.6 + 0.2 * uniform(&mut st));
                let pick_b = uniform(&mut st) < 0.33 && dd < 35.0;
                let h = if cells[k].0 == VGRASS {
                    if g.below(x, cells[k].1 + 2.0, z).is_some() {
                        continue;
                    }
                    let Some(h) = vg.below(x, cells[k].1 + 2.0, z) else { continue };
                    h
                } else {
                    let Some(h) = g.below(x, cells[k].1 + 2.0, z) else { continue };
                    if h.drivable || kinds.get(h.surface) != Some(&Kind::Grass) {
                        continue;
                    }
                    h
                };
                if h.normal[1] < 0.55 {
                    continue;
                }
                if dd < 3.0 * cell && (0..4).any(|q| {
                    let a = q as f32 * std::f32::consts::FRAC_PI_2;
                    g.below(x + 0.45 * a.cos(), h.y + 1.0, z + 0.45 * a.sin()).map(|n| n.drivable).unwrap_or(false)
                }) {
                    continue;
                }
                if let Some(c) = cover.below(x, h.y + 30.0, z) {
                    if c.y > h.y + 0.3 {
                        continue;
                    }
                }
                let n = h.normal;
                let f = [yaw.sin(), 0.0, yaw.cos()];
                let dot = f[0] * n[0] + f[1] * n[1] + f[2] * n[2];
                let fp = [f[0] - dot * n[0], f[1] - dot * n[1], f[2] - dot * n[2]];
                let rot = super::scene::euler_of(fp, n);
                let sp = if pick_b { b.unwrap_or(a) } else { a };
                v.push((sp, Inst { pos: [x, h.y - 0.02, z], rot, scale: [s, sy, s] }, dd));
            }
        }
        v
    }).collect();
    let mut items: Vec<(usize, Inst)> = Vec::new();
    let mut per_band = [0usize; 9];
    for row in rows {
        for (sp, i, dd) in row {
            if let Some(bi) = band(dd) {
                per_band[bi] += 1;
            }
            items.push((sp, i));
        }
    }
    out.instances = items.len();
    // coarse rasters (huge maps) take bigger tiles: about as many nodes
    out.entities = nodes("grass", inp.cat, &items, TILE * (cell / CELL), NODE_CAP);
    let road_cells = cells.iter().filter(|c| c.0 == ROAD).count();
    let grass_cells = cells.iter().filter(|c| c.0 == GRASS).count();
    let vgrass_cells = cells.iter().filter(|c| c.0 == VGRASS).count();
    out.report.push(format!("  grass: {} tufts in {} nodes ({} / {}), raster {}x{} of {:.1} m, {} road + {} grass + {} drawn-grass cells, reach {:.0} m, thinned x{:.2}; {:.1} s",
        out.instances, out.entities.len(), inp.cat.species[a].name, b.map(|i| inp.cat.species[i].name.as_str()).unwrap_or("-"), nx, nz, cell, road_cells, grass_cells, vgrass_cells, reach, thin, t0.elapsed().as_secs_f64()));
    let prof: Vec<String> = (0..9).filter(|&i| band_area[i] > 0.0).map(|i| format!("{}-{} m {:.2}", BANDS[i], BANDS[i + 1], per_band[i] as f64 / band_area[i])).collect();
    out.report.push(format!("  grass: tufts per m2 of grass by distance to the road: {}", prof.join(", ")));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn density_falls_off() {
        assert!(density(0.0) > density(10.0) && density(10.0) > density(60.0) && density(60.0) > density(120.0));
        assert_eq!(density(150.0), 0.0);
        assert_eq!(density(400.0), 0.0);
        assert!((density(1.0) - 5.25).abs() < 1e-4);
    }
}
