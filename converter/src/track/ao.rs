//! Per-vertex ambient occlusion in the static meshes (requirement
//! track-vertex-ao).
//!
//! Format (game facts, measured on 2,600 stock track meshes of laguna_seca,
//! brands_hatch, monza, imola and spa with `aostock`): the LOD bodies'
//! 4-float colour stream (f13) carries the AO in its fourth component as
//! ao / 255 (no AO = 1/255); materials read it with
//! `Vertex_UseVertexAmbientOcclusion` = 1, `Vertex_AmbientOcclusionChannel`
//! = 3, `Vertex_AmbientOcclusionIntensity` 250..255 (the shader takes
//! saturate(intensity x map AO x (offset + c[channel]))). The first three
//! components hold other per-vertex data (start-light lamp classes, road
//! variation); they are kept. Stock AO x 255, median [p10]: road
//! (DynamicTrack) 0.94 [0.90], kerbs / grass (DynamicTrackMultilayer) 0.93
//! [0.66], fences 0.67 [0.48], buildings / walls (Uber*) 0.60 [0.22].
//!
//! Method (ours): cosine-weighted hemisphere rays from every vertex (2 cm
//! off the surface along its normal) against the bake's ray tree of opaque
//! geometry; a hit at distance t occludes by (1 - t / RANGE), except a
//! parallel surface less than 25 cm above the vertex's tangent plane (an
//! AC1 overlay layer: tyre marks, grass borders, painted kerbs over the
//! road; rays pass it). The ray count
//! per vertex follows a total budget (RAYS_TOTAL, 8..32 per vertex), so a
//! 26M-triangle map costs about what a circuit does. Rays are a Hammersley
//! set rotated by a hash of the vertex position: deterministic.
//! Values stop at [`FLOOR`]. LOD levels take the value of the LOD0 vertex
//! at the same position.

use anyhow::Result;
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use std::collections::BTreeMap;

use super::bakes::{hash3, read_geo, unit_of, Inputs, MatInfo, Tile};
use crate::io::pb::{self, Msg, Val};

/// Occlusion range in metres (ours, calibrated on stock: see the module notes).
pub const RANGE: f32 = 8.0;
/// Total AO ray budget per track.
pub const RAYS_TOTAL: usize = 30_000_000;
pub const RAYS_MIN: usize = 8;
pub const RAYS_MAX: usize = 32;
const OFFSET: f32 = 0.02;
/// Parallel surfaces closer than this (along the normal) are layers of the
/// same surface (ours).
const LAYER: f32 = 0.25;
/// Occlusion strength (ours, calibrated on stock: road medians stay near
/// the stock 0.94 while wall bases darken).
const STRENGTH: f32 = 1.0;
/// Lowest AO written (ours): a vertex inside a barrier or under a guard
/// rail is hidden, but a black value there smears into the visible
/// triangles it shares.
pub const FLOOR: f32 = 0.15;

pub struct Ao {
    /// per mesh, per LOD0 vertex: AO x 255 (None = not baked: neutral)
    per_mesh: Vec<Option<Vec<u8>>>,
    rays: usize,
    vertices: usize,
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn norm(a: [f32; 3]) -> Option<[f32; 3]> {
    let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    (l.is_finite() && l > 1e-12).then(|| [a[0] / l, a[1] / l, a[2] / l])
}

/// Van der Corput radical inverse (base 2).
pub fn radical2(mut i: u32) -> f32 {
    i = i.rotate_right(16);
    i = ((i & 0x5555_5555) << 1) | ((i & 0xaaaa_aaaa) >> 1);
    i = ((i & 0x3333_3333) << 2) | ((i & 0xcccc_cccc) >> 2);
    i = ((i & 0x0f0f_0f0f) << 4) | ((i & 0xf0f0_f0f0) >> 4);
    i = ((i & 0x00ff_00ff) << 8) | ((i & 0xff00_ff00) >> 8);
    i as f32 / 4_294_967_296.0
}

/// AO (0..1) of one point with unit normal `n`, `rays` samples.
pub fn ao_at(tile: &Tile, p: [f32; 3], n: [f32; 3], rays: usize) -> f32 {
    let t = norm(cross(if n[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] }, n)).unwrap_or([1.0, 0.0, 0.0]);
    let b = cross(n, t);
    let h = hash3(p);
    let (r1, r2) = (unit_of(h), unit_of(h.rotate_left(24)));
    let o = [p[0] + n[0] * OFFSET, p[1] + n[1] * OFFSET, p[2] + n[2] * OFFSET];
    let mut occ = 0f32;
    for i in 0..rays {
        let u1 = ((i as f32 + 0.5) / rays as f32 + r1).fract();
        let u2 = (radical2(i as u32) + r2).fract();
        let r = u1.sqrt();
        let phi = std::f32::consts::TAU * u2;
        let (s, c) = phi.sin_cos();
        let z = (1.0 - u1).max(0.0).sqrt();
        let d = [t[0] * r * c + b[0] * r * s + n[0] * z, t[1] * r * c + b[1] * r * s + n[1] * z, t[2] * r * c + b[2] * r * s + n[2] * z];
        // a surface a few centimetres above and parallel (an AC1 overlay:
        // tyre marks, kerb paint) is a layer of the same surface, not an
        // occluder: the ray goes on past it
        let mut tmin = 0.0;
        for _ in 0..8 {
            let Some(hit) = tile.bvh.hit(o, d, tmin, RANGE) else { break };
            let par = (hit.normal[0] * n[0] + hit.normal[1] * n[1] + hit.normal[2] * n[2]).abs();
            // height of the hit above the vertex's tangent plane: a grazing
            // ray meets a layer 3 cm up only after 20 cm or more
            let above = hit.t * (d[0] * n[0] + d[1] * n[1] + d[2] * n[2]);
            if above < LAYER && par > 0.85 {
                tmin = hit.t + 1e-4;
                continue;
            }
            occ += 1.0 - hit.t / RANGE;
            break;
        }
    }
    (1.0 - STRENGTH * occ / rays.max(1) as f32).clamp(FLOOR, 1.0)
}

impl Ao {
    /// `meshes` written meshes, `drawn_tris` their triangle total (sets the
    /// rays per vertex).
    pub fn new(meshes: usize, drawn_tris: usize) -> Ao {
        // about 0.8 vertices per triangle on converted tracks
        let verts = (drawn_tris as f64 * 0.8).max(1.0);
        let rays = ((RAYS_TOTAL as f64 / verts) as usize).clamp(RAYS_MIN, RAYS_MAX);
        Ao { per_mesh: vec![None; meshes], rays, vertices: 0 }
    }

    pub fn bake_tile(&mut self, inp: &Inputs, mats: &[Option<(u32, MatInfo)>], tile: &Tile) -> Result<()> {
        let rays = self.rays;
        let todo: Vec<usize> = (0..inp.meshes.len()).filter(|&i| {
            let m = &inp.meshes[i];
            m.drawn && !mats[i].map(|x| x.1.emissive).unwrap_or(false)
                && m.lo[0] < tile.hi[0] && m.hi[0] >= tile.lo[0] && m.lo[2] < tile.hi[1] && m.hi[2] >= tile.lo[1]
        }).collect();
        let done: Vec<(usize, usize, Vec<(u32, u8)>)> = todo.par_iter().map(|&i| {
            let Ok(g) = read_geo(&inp.meshes[i].file) else { return (i, 0, Vec::new()) };
            let v: Vec<(u32, u8)> = g.pos.iter().enumerate().filter(|(_, p)| tile.contains(p[0], p[2])).map(|(k, p)| {
                let n = g.nrm.get(k).and_then(|n| norm(*n)).unwrap_or([0.0, 1.0, 0.0]);
                (k as u32, (ao_at(tile, *p, n, rays) * 255.0).round() as u8)
            }).collect();
            (i, g.pos.len(), v)
        }).collect();
        for (i, n, v) in done {
            if v.is_empty() {
                continue;
            }
            let slot = self.per_mesh[i].get_or_insert_with(|| vec![255u8; n]);
            for (k, a) in v {
                if let Some(x) = slot.get_mut(k as usize) {
                    *x = a;
                }
                self.vertices += 1;
            }
        }
        Ok(())
    }

    /// Write the AO into every baked mesh's colour stream; report the
    /// statistics per shader.
    pub fn finish(self, inp: &Inputs, mats: &[Option<(u32, MatInfo)>]) -> Result<Vec<String>> {
        let stats: Vec<(&'static str, Vec<u8>)> = self.per_mesh.par_iter().enumerate().filter_map(|(i, ao)| {
            let ao = ao.as_ref()?;
            write_ao(&inp.meshes[i].file, ao).ok()?;
            let step = (ao.len() / 400).max(1);
            Some((mats[i].map(|x| x.1.shader).unwrap_or("other"), ao.iter().step_by(step).copied().collect()))
        }).collect();
        let mut by: BTreeMap<&str, Vec<u8>> = BTreeMap::new();
        for (k, v) in stats {
            by.entry(k).or_default().extend(v);
        }
        let mut lines = vec![format!("  ao: {} vertices, {} rays each, range {} m", self.vertices, self.rays, RANGE)];
        for (k, mut v) in by {
            v.sort_unstable();
            let q = |f: f32| v[((v.len() - 1) as f32 * f) as usize] as f32 / 255.0;
            lines.push(format!("  ao {k:<24} p10 {:.2} p25 {:.2} p50 {:.2} p90 {:.2} (n {})", q(0.1), q(0.25), q(0.5), q(0.9), v.len()));
        }
        Ok(lines)
    }
}

fn key(p: [f32; 3]) -> [u32; 3] {
    [p[0].to_bits(), p[1].to_bits(), p[2].to_bits()]
}

/// Rewrite a mesh's colour streams: component 3 = AO / 255 (LOD levels by
/// matching positions), components 0..2 kept.
pub fn write_ao(file: &std::path::Path, ao: &[u8]) -> Result<()> {
    let b = std::fs::read(file)?;
    let mut top = Msg::parse(&b)?;
    let lods = top.idx(5);
    let mut by_pos: FxHashMap<[u32; 3], u8> = FxHashMap::default();
    for (li, &fi) in lods.iter().enumerate() {
        let lod = top.open(fi)?;
        let pos: Vec<[f32; 3]> = lod.bytes(5).map(|b| b.chunks_exact(12).map(|c| [0, 1, 2].map(|k| f32::from_le_bytes([c[4 * k], c[4 * k + 1], c[4 * k + 2], c[4 * k + 3]]))).collect()).unwrap_or_default();
        let n = pos.len();
        let mut col: Vec<f32> = lod.bytes(13).map(|b| b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()).unwrap_or_default();
        col.resize(n * 4, 0.0);
        for (v, p) in pos.iter().enumerate() {
            let a = if li == 0 {
                let a = ao.get(v).copied().unwrap_or(255);
                by_pos.insert(key(*p), a);
                a
            } else {
                by_pos.get(&key(*p)).copied().unwrap_or(255)
            };
            col[v * 4 + 3] = a as f32 / 255.0 / 255.0;
        }
        lod.set(13, Val::Bytes(pb::f32_bytes(&col)));
    }
    std::fs::write(file, top.encode())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::bvh::Bvh;

    fn tile(tris: Vec<[[f32; 3]; 3]>) -> Tile {
        let n = tris.len();
        Tile { lo: [-1e6, -1e6], hi: [1e6, 1e6], bvh: Bvh::build(tris, vec![0; n]), foliage: Bvh::build(Vec::new(), Vec::new()), palette: vec![[0.2; 3]], top: 100.0 }
    }

    #[test]
    fn open_ground_is_unoccluded_and_wall_base_is_dark() {
        // ground 100 x 100 facing up, a wall along x = 0 facing -x
        let g = 50.0;
        let mut t = vec![[[-g, 0.0, -g], [-g, 0.0, g], [g, 0.0, g]], [[-g, 0.0, -g], [g, 0.0, g], [g, 0.0, -g]]];
        t.push([[0.0, 0.0, -g], [0.0, 10.0, -g], [0.0, 10.0, g]]);
        t.push([[0.0, 0.0, -g], [0.0, 10.0, g], [0.0, 0.0, g]]);
        let tl = tile(t);
        let open = ao_at(&tl, [-30.0, 0.0, 0.0], [0.0, 1.0, 0.0], 32);
        let base = ao_at(&tl, [-0.1, 0.0, 0.0], [0.0, 1.0, 0.0], 32);
        assert!(open > 0.99, "{open}");
        assert!(base >= FLOOR);
        assert!(base < 0.75, "{base}");
        // deterministic
        assert_eq!(base, ao_at(&tl, [-0.1, 0.0, 0.0], [0.0, 1.0, 0.0], 32));
    }
}
