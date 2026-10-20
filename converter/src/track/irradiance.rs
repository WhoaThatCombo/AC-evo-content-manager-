//! Irradiance volumes (requirement track-irradiance).
//!
//! Format (game facts, measured on stock laguna_seca, brands_hatch, monza,
//! imola and spa; shaders from the executable's DXIL):
//! - main scene: one "Macro Irradiance Volume" entity (f50.f135 empty;
//!   transform = centre and half extent of the probe grid; active-distance
//!   override 1000) and one "Irradiance Probe" entity per volume on a full
//!   rectangular grid (laguna 14 x 18 = 252, imola 32 x 20, spa 20 x 32);
//! - a volume's transform position is its grid corner (y = 0); its data
//!   (IrradianceVolumeActorData, f50.f120): f1 bounced-sky visibility
//!   texture, f2 groundInterval (4.5 m; spa 5.0), f3 / f4 / f5 the heights
//!   of the three texel levels above the ground (1.5 / 5.5 / 13.5; spa
//!   15.0), f6 256 ground heights (16 x 16 cells of groundInterval, x
//!   fastest), f8 collisionTolerance 1.5, f9 / f10 grid x / z, f11 the macro
//!   entity's guid, f12 the night irradiance texture. A volume spans 16
//!   ground cells (72 m; spa 80 m);
//! - textures `irradiance_volumes\iv_bounced_sky<guid>` and
//!   `iv_night_irradiance_<guid>` (the probe's guid in decimal), header {f1
//!   64, f2 3, f3 1, f4 30 (R11G11B10 float), f6 16, f8 1}, 12,288 bytes of
//!   texels x fastest: x = 4 x cell + k (k = SH coefficient), y = level, z =
//!   cell; cells run along world +X and +Z (checked by edge continuity
//!   against neighbouring volumes);
//! - the game's GI-update compute shader turns the visibility texel V_k
//!   (rgb) into SH coefficient k as ((V_k (1 + sun x Y_k(mirrored sun) x
//!   sky) x bouncedSkyIntensity + max(lum(V_k), 0.1) x skyIntensity) x sky
//!   + night); the track shaders evaluate E(N) = max(0, 0.886 sh0 -
//!   1.023 N.y sh1 + 1.023 N.z sh2 - 1.023 N.x sh3) / pi. So V is the L1
//!   SH projection of the light reaching the cell under a unit sky, with
//!   the basis (0.282, -0.489 y, 0.489 z, -0.489 x); the sun's bounce is
//!   added at run time from V. The format holds no negatives: the sky's
//!   own downward-pointing term is cut to 0, as stock (stock coefficient 1
//!   median 0);
//! - stock texel medians of coefficient 0 by level (G): laguna_seca 0.91 /
//!   1.03 / 1.05, brands_hatch 0.36 / 0.43 / 0.58; night textures mostly 0.
//!
//! Ours:
//! - the grid covers the drivable area plus [`AREA_MARGIN`], 16 cells of
//!   4.5 m per volume (the stock spacing), the cell growing in 0.5 m steps
//!   while more than [`MAX_VOLUMES`] (stock maximum, nurburgring) would be
//!   needed;
//! - ground heights: the highest drawn ground surface (a drawn physics
//!   mesh) under each cell centre, else the highest upward-facing opaque
//!   surface; empty cells take the nearest filled cell;
//! - texels: [`RAYS`] directions (a Fibonacci sphere turned by a hash of
//!   the texel) to [`REACH`] m; the sky has radiance 1, a hit surface
//!   reflects albedo x its own sky exposure (0.5 + 0.5 n.y) x 0.85, the
//!   back of a surface (a deck seen from below) albedo x 0.15; rays that
//!   leave the map below the horizon see a ground of albedo 0.2; an
//!   alpha-tested card (tree, bush, mesh fence) on the way passes a quarter
//!   of the ray and reflects the rest. A texel
//!   with 90 % of its rays on back faces is enclosed (inside a model) and
//!   takes the value of the level above (or its neighbours).
//!   [`SCALE`] maps the result onto stock's range (calibrated against
//!   laguna_seca: see FORMATS.md);
//! - night textures are zeros (no night lighting model yet).

use anyhow::Result;
use rayon::prelude::*;

use super::bakes::{hash3, unit_of, Inputs, Tile, GROUND_BIT};
use super::scene::{Entity, SceneBuilder};
use crate::io::pb::{Msg, Val};

pub const MAX_VOLUMES: usize = 900;
pub const AREA_MARGIN: f32 = 120.0;
pub const GROUND_INTERVAL: f32 = 4.5;
pub const ELEVATIONS: [f32; 3] = [1.5, 5.5, 13.5];
pub const COLLISION_TOLERANCE: f32 = 1.5;
pub const RAYS: usize = 48;
pub const REACH: f32 = 250.0;
/// Visibility -> stock texel scale (ours, calibrated).
pub const SCALE: f32 = 0.6;
const TEXELS: usize = 16 * 3 * 16;
/// Light leaving the back of a surface (ours: the bounce from the ground).
const BACK_LIGHT: f32 = 0.15;
/// Share of a ray an alpha-tested card lets through (ours), and how many
/// cards one ray meets at most.
pub const FOLIAGE_PASS: f32 = 0.25;

/// Calibration knobs (ACECONV_PROBE_SCALE / ACECONV_FOLIAGE_PASS override
/// the constants; for re-calibrating against stock only).
fn knob(name: &str, v: f32) -> f32 {
    std::env::var(name).ok().and_then(|x| x.parse().ok()).unwrap_or(v)
}
const FOLIAGE_LAYERS: usize = 6;

pub struct Volume {
    pub gx: u32,
    pub gz: u32,
    /// corner (x, z)
    pub origin: [f32; 2],
    pub heights: Option<Vec<f32>>,
    /// per texel (cell x, level, cell z): 4 coefficients x rgb; None = inside geometry
    pub texels: Option<Vec<Option<[[f32; 3]; 4]>>>,
}

pub struct Probes {
    pub gi: f32,
    pub nx: u32,
    pub nz: u32,
    pub origin: [f32; 2],
    pub volumes: Vec<Volume>,
    rays: usize,
    inside: usize,
    no_ground: usize,
}

/// Lay out the grid (None when there is nothing to cover).
pub fn plan(inp: &Inputs, glo: [f32; 2], ghi: [f32; 2]) -> Option<Probes> {
    let (dlo, dhi) = inp.drivable;
    if !(dlo[0].is_finite() && dhi[0] > dlo[0] && dhi[1] > dlo[1]) {
        return None;
    }
    let lo = [(dlo[0] - AREA_MARGIN).max(glo[0]), (dlo[1] - AREA_MARGIN).max(glo[1])];
    let hi = [(dhi[0] + AREA_MARGIN).min(ghi[0]), (dhi[1] + AREA_MARGIN).min(ghi[1])];
    let (w, h) = ((hi[0] - lo[0]).max(1.0), (hi[1] - lo[1]).max(1.0));
    let mut gi = GROUND_INTERVAL;
    let (mut nx, mut nz);
    loop {
        let s = gi * 16.0;
        nx = (w / s).ceil().max(1.0) as u32;
        nz = (h / s).ceil().max(1.0) as u32;
        if (nx * nz) as usize <= MAX_VOLUMES {
            break;
        }
        gi += 0.5;
    }
    let s = gi * 16.0;
    let c = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5];
    // whole metres, as stock corners are
    let origin = [(c[0] - nx as f32 * s * 0.5).round(), (c[1] - nz as f32 * s * 0.5).round()];
    let mut volumes = Vec::new();
    for gz in 0..nz {
        for gx in 0..nx {
            volumes.push(Volume { gx, gz, origin: [origin[0] + gx as f32 * s, origin[1] + gz as f32 * s], heights: None, texels: None });
        }
    }
    Some(Probes { gi, nx, nz, origin, volumes, rays: RAYS, inside: 0, no_ground: 0 })
}

/// Height of the ground under (x, z): the highest drawn ground surface,
/// else the highest upward-facing opaque surface.
pub fn ground_at(tile: &Tile, x: f32, z: f32) -> Option<f32> {
    let mut y = tile.top;
    let mut any: Option<f32> = None;
    for _ in 0..24 {
        let Some(h) = tile.bvh.hit([x, y, z], [0.0, -1.0, 0.0], 0.0, 1.0e5) else { break };
        let hy = y - h.t;
        if !h.back && h.normal[1] > 0.2 {
            if h.payload & GROUND_BIT != 0 {
                return Some(hy);
            }
            any.get_or_insert(hy);
        }
        y = hy - 0.01;
    }
    any
}

/// Fibonacci sphere direction i of n.
fn fib(i: usize, n: usize) -> [f32; 3] {
    let y = 1.0 - (i as f32 + 0.5) / n as f32 * 2.0;
    let r = (1.0 - y * y).max(0.0).sqrt();
    let phi = i as f32 * 2.399_963_1;
    [r * phi.cos(), y, r * phi.sin()]
}

const Y0: f32 = 0.282_095;
const Y1: f32 = 0.488_603;

/// Visibility SH (4 coefficients x rgb) at a point; None when inside geometry.
pub fn texel_at(tile: &Tile, p: [f32; 3], rays: usize) -> Option<[[f32; 3]; 4]> {
    let rot = unit_of(hash3(p)) * std::f32::consts::TAU;
    let (s, c) = rot.sin_cos();
    let w = 4.0 * std::f32::consts::PI / rays as f32;
    let mut v = [[0f32; 3]; 4];
    let mut backs = 0;
    let fpass = knob("ACECONV_FOLIAGE_PASS", FOLIAGE_PASS);
    for i in 0..rays {
        let f = fib(i, rays);
        let d = [f[0] * c - f[2] * s, f[1], f[0] * s + f[2] * c];
        let solid = tile.bvh.hit(p, d, 0.05, REACH);
        // foliage cards in front of the solid hit: each lets FOLIAGE_PASS
        // through and reflects the rest (lit like a surface facing up half way)
        let end = solid.map(|h| h.t).unwrap_or(REACH);
        let (mut pass, mut lit) = (1.0f32, [0f32; 3]);
        let mut tmin = 0.05;
        for _ in 0..FOLIAGE_LAYERS {
            let Some(f) = tile.foliage.hit(p, d, tmin, end) else { break };
            let a = tile.albedo(f.payload);
            let e = 0.6 * 0.85 * (1.0 - fpass) * pass;
            for ch in 0..3 {
                lit[ch] += a[ch] * e;
            }
            pass *= fpass;
            tmin = f.t + 0.01;
        }
        let l: [f32; 3] = match solid {
            None if d[1] > 0.0 => [1.0; 3],
            None => [0.2 * 0.85; 3],
            Some(h) if h.back => {
                // the unlit side of a surface (a bridge deck seen from
                // below): only the bounce from below reaches it
                backs += 1;
                let a = tile.albedo(h.payload);
                [a[0] * BACK_LIGHT, a[1] * BACK_LIGHT, a[2] * BACK_LIGHT]
            }
            Some(h) => {
                let a = tile.albedo(h.payload);
                let e = (0.5 + 0.5 * h.normal[1]) * 0.85;
                [a[0] * e, a[1] * e, a[2] * e]
            }
        };
        let l = [l[0] * pass + lit[0], l[1] * pass + lit[1], l[2] * pass + lit[2]];
        let basis = [Y0, -Y1 * d[1], Y1 * d[2], -Y1 * d[0]];
        for k in 0..4 {
            for ch in 0..3 {
                v[k][ch] += l[ch] * basis[k] * w;
            }
        }
    }
    // enclosed (inside a closed model): no car is ever there
    if backs * 10 >= rays * 9 {
        return None;
    }
    Some(v)
}

impl Probes {
    pub fn bake_tile(&mut self, tile: &Tile) {
        let s = self.gi * 16.0;
        let gi = self.gi;
        let rays = self.rays;
        let mine: Vec<usize> = (0..self.volumes.len()).filter(|&i| {
            let v = &self.volumes[i];
            tile.contains(v.origin[0] + s * 0.5, v.origin[1] + s * 0.5)
        }).collect();
        // ground heights
        let hs: Vec<(usize, Option<Vec<f32>>)> = mine.par_iter().map(|&i| {
            let o = self.volumes[i].origin;
            let raw: Vec<Option<f32>> = (0..256).map(|c| ground_at(tile, o[0] + ((c % 16) as f32 + 0.5) * gi, o[1] + ((c / 16) as f32 + 0.5) * gi)).collect();
            (i, fill_cells(&raw))
        }).collect();
        for (i, h) in hs {
            if h.is_none() {
                self.no_ground += 1;
            }
            self.volumes[i].heights = h;
        }
        // volumes without any ground take the nearest volume of this tile that has some
        let with: Vec<(u32, u32, f32)> = mine.iter().filter_map(|&i| {
            let v = &self.volumes[i];
            v.heights.as_ref().map(|h| (v.gx, v.gz, median(h)))
        }).collect();
        for &i in &mine {
            if self.volumes[i].heights.is_none() {
                let (gx, gz) = (self.volumes[i].gx as i64, self.volumes[i].gz as i64);
                let y = with.iter().min_by_key(|w| (w.0 as i64 - gx).pow(2) + (w.1 as i64 - gz).pow(2)).map(|w| w.2).unwrap_or(0.0);
                self.volumes[i].heights = Some(vec![y; 256]);
            }
        }
        // texels
        let tx: Vec<(usize, Vec<Option<[[f32; 3]; 4]>>)> = mine.par_iter().map(|&i| {
            let v = &self.volumes[i];
            let h = v.heights.as_ref().unwrap();
            let t: Vec<Option<[[f32; 3]; 4]>> = (0..TEXELS).map(|k| {
                let (cx, lv, cz) = (k % 16, (k / 16) % 3, k / 48);
                let p = [v.origin[0] + (cx as f32 + 0.5) * gi, h[cz * 16 + cx] + ELEVATIONS[lv], v.origin[1] + (cz as f32 + 0.5) * gi];
                texel_at(tile, p, rays)
            }).collect();
            (i, t)
        }).collect();
        for (i, t) in tx {
            self.inside += t.iter().filter(|x| x.is_none()).count();
            self.volumes[i].texels = Some(t);
        }
    }

    pub fn finish(self, inp: &Inputs, b: &mut SceneBuilder) -> Result<Vec<String>> {
        let dir = inp.out.join("irradiance_volumes");
        std::fs::create_dir_all(&dir)?;
        let s = self.gi * 16.0;
        // ---- macro volume: centre and half extent of the grid
        let all_h: Vec<f32> = self.volumes.iter().filter_map(|v| v.heights.as_ref()).flatten().copied().collect();
        let (ylo, yhi) = all_h.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |a, &y| (a.0.min(y), a.1.max(y)));
        let (ylo, yhi) = if ylo.is_finite() { (ylo, yhi) } else { (0.0, 0.0) };
        let mut mac = Entity::new("gi_macro_volume", "Macro Irradiance Volume", 135, Msg::default());
        mac.position = [self.origin[0] + self.nx as f32 * s * 0.5, (ylo + yhi) * 0.5, self.origin[1] + self.nz as f32 * s * 0.5];
        mac.scale = [self.nx as f32 * s * 0.5, ((yhi - ylo) * 0.5 + 100.0).max(200.0), self.nz as f32 * s * 0.5];
        mac.distance_override = Some(1000.0);
        let macro_id = b.add(None, mac);
        // ---- per volume
        let mut texel_vals: Vec<f32> = Vec::new();
        let open = open_texel();
        let mut files: Vec<(String, Vec<u8>)> = Vec::new();
        let scale = knob("ACECONV_PROBE_SCALE", SCALE);
        for v in &self.volumes {
            let name = format!("gi_volume_{}_{}", v.gx, v.gz);
            let id = b.ids.of("Irradiance Probe", &format!("/{name}"));
            let rel_sky = format!("content\\tracks\\{}\\irradiance_volumes\\iv_bounced_sky{id}.texture", inp.track);
            let rel_night = format!("content\\tracks\\{}\\irradiance_volumes\\iv_night_irradiance_{id}.texture", inp.track);
            let texels = fill_texels(v.texels.as_deref(), &open);
            let mut mips = Vec::with_capacity(TEXELS * 4 * 4);
            for z in 0..16 {
                for lv in 0..3 {
                    for cx in 0..16 {
                        let t = &texels[z * 48 + lv * 16 + cx];
                        for k in 0..4 {
                            let c = [0, 1, 2].map(|ch| (t[k][ch] * scale).max(0.0));
                            if k == 0 {
                                texel_vals.push(c[1]);
                            }
                            mips.extend_from_slice(&pack_r11g11b10(c).to_le_bytes());
                        }
                    }
                }
            }
            files.push((format!("iv_bounced_sky{id}.texture"), volume_header()));
            files.push((format!("iv_bounced_sky{id}.texturemips"), mips));
            files.push((format!("iv_night_irradiance_{id}.texture"), volume_header()));
            files.push((format!("iv_night_irradiance_{id}.texturemips"), vec![0u8; TEXELS * 4 * 4]));
            let heights = v.heights.clone().unwrap_or_else(|| vec![(ylo + yhi) * 0.5; 256]);
            let mut d = Msg::default();
            d.set_str(1, &rel_sky);
            d.set_f32(2, self.gi);
            d.set_f32(3, ELEVATIONS[0]);
            d.set_f32(4, ELEVATIONS[1]);
            d.set_f32(5, ELEVATIONS[2]);
            d.push(6, Val::Bytes(crate::io::pb::f32_bytes(&heights)));
            d.set_f32(8, COLLISION_TOLERANCE);
            if v.gx != 0 {
                d.set_varint(9, v.gx as u64);
            }
            if v.gz != 0 {
                d.set_varint(10, v.gz as u64);
            }
            d.push(11, Val::F64(macro_id.to_le_bytes()));
            d.set_str(12, &rel_night);
            let mut e = Entity::new(&name, "Irradiance Probe", 120, d);
            e.position = [v.origin[0], 0.0, v.origin[1]];
            e.distance_override = Some(1000.0);
            b.main.entities.push((id, e));
        }
        files.par_iter().try_for_each(|(n, bytes)| std::fs::write(dir.join(n), bytes))?;
        texel_vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let q = |f: f32| texel_vals.get(((texel_vals.len().max(1) - 1) as f32 * f) as usize).copied().unwrap_or(0.0);
        let area = self.nx as f32 * s * self.nz as f32 * s / 1e6;
        Ok(vec![
            format!("  probes: {} x {} volumes of {:.0} m (ground cell {:.1} m), {:.2} km2 ({:.0} per km2), {} rays per texel; {} texels inside geometry, {} volumes without ground",
                self.nx, self.nz, s, self.gi, area, self.volumes.len() as f32 / area.max(1e-6), self.rays, self.inside, self.no_ground),
            format!("  probes: coefficient 0 (G) p10 {:.3} p50 {:.3} p90 {:.3} max {:.3}", q(0.1), q(0.5), q(0.9), q(1.0)),
        ])
    }
}

fn median(v: &[f32]) -> f32 {
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    s[s.len() / 2]
}

/// Empty ground cells take the nearest filled one (None: no cell filled).
fn fill_cells(raw: &[Option<f32>]) -> Option<Vec<f32>> {
    let have: Vec<(usize, f32)> = raw.iter().enumerate().filter_map(|(i, h)| h.map(|y| (i, y))).collect();
    if have.is_empty() {
        return None;
    }
    Some((0..256).map(|c| raw[c].unwrap_or_else(|| {
        let (x, z) = ((c % 16) as i32, (c / 16) as i32);
        have.iter().min_by_key(|(i, _)| ((*i % 16) as i32 - x).pow(2) + ((*i / 16) as i32 - z).pow(2)).unwrap().1
    })).collect())
}

/// The texel of a fully open cell (unit sky above, ground of albedo 0.2 below).
fn open_texel() -> [[f32; 3]; 4] {
    let up = Y0 * 2.0 * std::f32::consts::PI;
    let down = 0.2 * 0.85 * Y0 * 2.0 * std::f32::consts::PI;
    let v = up + down;
    // sky above: coefficient 1 is negative (cut to 0); the ground below adds
    let c1 = (-Y1 * std::f32::consts::PI + 0.2 * 0.85 * Y1 * std::f32::consts::PI).max(0.0);
    [[v; 3], [c1; 3], [0.0; 3], [0.0; 3]]
}

/// Texels inside geometry: the level above in the same column, else the
/// mean of the valid texels at that level, else an open cell.
fn fill_texels(t: Option<&[Option<[[f32; 3]; 4]>]>, open: &[[f32; 3]; 4]) -> Vec<[[f32; 3]; 4]> {
    let Some(t) = t else { return vec![*open; TEXELS] };
    let mut level_mean = [*open; 3];
    for lv in 0..3 {
        let vals: Vec<&[[f32; 3]; 4]> = (0..TEXELS).filter(|k| (k / 16) % 3 == lv).filter_map(|k| t[k].as_ref()).collect();
        if !vals.is_empty() {
            let mut m = [[0f32; 3]; 4];
            for v in &vals {
                for k in 0..4 {
                    for c in 0..3 {
                        m[k][c] += v[k][c] / vals.len() as f32;
                    }
                }
            }
            level_mean[lv] = m;
        }
    }
    (0..TEXELS).map(|k| {
        let (cx, lv, cz) = (k % 16, (k / 16) % 3, k / 48);
        if let Some(v) = t[k] {
            return v;
        }
        for up in lv + 1..3 {
            if let Some(v) = t[cz * 48 + up * 16 + cx] {
                return v;
            }
        }
        level_mean[lv]
    }).collect()
}

/// `.texture` header of a probe volume: 64 x 3 x 16 R11G11B10F, one mip.
pub fn volume_header() -> Vec<u8> {
    let mut h = Msg::default();
    h.set_varint(1, 64);
    h.set_varint(2, 3);
    h.set_varint(3, 1);
    h.set_varint(4, 30);
    h.set_varint(6, 16);
    h.set_varint(8, 1);
    h.encode()
}

/// An unsigned small float: 5-bit exponent, `mbits` mantissa (no sign).
fn pack_uf(v: f32, mbits: u32) -> u32 {
    if !(v > 0.0) {
        return 0;
    }
    let one = 1u32 << mbits;
    let max = (2.0 - 1.0 / one as f32) * 32768.0;
    let v = v.min(max);
    let mut e = v.log2().floor() as i32;
    if e < -14 {
        // denormal: value = m / 2^mbits * 2^-14 (m = 2^mbits rolls into exponent 1)
        return (v / 2f32.powi(-14) * one as f32).round() as u32;
    }
    let mut m = ((v / 2f32.powi(e) - 1.0) * one as f32).round() as u32;
    if m >= one {
        m = 0;
        e += 1;
    }
    if e > 15 {
        return (30 << mbits) | (one - 1);
    }
    (((e + 15) as u32) << mbits) | m
}

/// R11G11B10 unsigned float: R bits 0..10, G 11..21, B 22..31.
pub fn pack_r11g11b10(c: [f32; 3]) -> u32 {
    pack_uf(c[0], 6) | (pack_uf(c[1], 6) << 11) | (pack_uf(c[2], 5) << 22)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unpack(u: u32, mbits: u32) -> f32 {
        let e = (u >> mbits) & 31;
        let m = u & ((1 << mbits) - 1);
        if e == 0 {
            m as f32 / (1 << mbits) as f32 * 2f32.powi(-14)
        } else {
            (1.0 + m as f32 / (1 << mbits) as f32) * 2f32.powi(e as i32 - 15)
        }
    }

    #[test]
    fn small_floats_round_trip() {
        for v in [0.0f32, 1e-6, 0.001, 0.02, 0.3, 0.898, 1.0, 1.453, 7.5, 100.0] {
            let p = pack_r11g11b10([v, v, v]);
            let r = unpack(p & 0x7ff, 6);
            let b = unpack(p >> 22, 5);
            let tol = v * 0.02 + 1e-6;
            assert!((r - v).abs() <= tol, "{v} -> {r}");
            assert!((b - v).abs() <= v * 0.04 + 1e-6, "{v} -> {b}");
        }
        assert_eq!(pack_r11g11b10([-1.0, f32::NAN, 0.0]), 0);
        assert_eq!(volume_header().len(), 12);
    }

    #[test]
    fn open_sky_is_bright_and_inside_is_none() {
        use crate::track::bvh::Bvh;
        let g = 500.0;
        let t = vec![[[-g, 0.0, -g], [-g, 0.0, g], [g, 0.0, g]], [[-g, 0.0, -g], [g, 0.0, g], [g, 0.0, -g]]];
        let tile = Tile { lo: [-g, -g], hi: [g, g], bvh: Bvh::build(t, vec![GROUND_BIT; 2]), foliage: Bvh::build(Vec::new(), Vec::new()), palette: vec![[0.2; 3]], top: 100.0 };
        let v = texel_at(&tile, [0.0, 1.5, 0.0], 48).unwrap();
        assert!(v[0][1] > 1.6 && v[0][1] < 2.2, "{v:?}");
        assert!(v[1][1] < 0.0);
        // under the plane: dim, not black
        let u = texel_at(&tile, [0.0, -1.0, 0.0], 48).unwrap();
        assert!(u[0][1] > 0.05 && u[0][1] < 0.5, "{u:?}");
        // inside a closed box: enclosed
        let b = 5.0;
        let mut bx = Vec::new();
        for (a, s) in [(0usize, 1.0f32), (0, -1.0), (1, 1.0), (1, -1.0), (2, 1.0), (2, -1.0)] {
            // a face of the box at axis a = s * b, facing outwards
            let (u, v) = ((a + 1) % 3, (a + 2) % 3);
            let mut q = [[0f32; 3]; 4];
            for (k, (du, dv)) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].iter().enumerate() {
                q[k][a] = s * b;
                q[k][u] = du * b;
                q[k][v] = dv * b;
            }
            if s > 0.0 { bx.push([q[0], q[1], q[2]]); bx.push([q[0], q[2], q[3]]); } else { bx.push([q[0], q[2], q[1]]); bx.push([q[0], q[3], q[2]]); }
        }
        let n = bx.len();
        let inside = Tile { lo: [-g, -g], hi: [g, g], bvh: Bvh::build(bx, vec![0; n]), foliage: Bvh::build(Vec::new(), Vec::new()), palette: vec![[0.2; 3]], top: 100.0 };
        assert!(texel_at(&inside, [0.3, 0.2, -0.1], 48).is_none());
        assert!(texel_at(&inside, [0.3, 20.0, -0.1], 48).is_some());
        assert_eq!(ground_at(&tile, 3.0, 4.0), Some(0.0));
    }
}
