//! The terrain pigment map (requirement track-pigment).
//!
//! Format (game facts, stock brands_hatch, monza, imola, spa, laguna_seca):
//! the main scene's graphics settings carry f4 Pigment {f1 transform
//! (position = centre, scale = half extent; y is unused by the mapping),
//! f2 texture, f3 world_align, f4 world_align_scale_xz}. Stock pigments are
//! BC1 sRGB (13) or BC7 sRGB (34), 2048 or 4096 texels a side, full mip
//! chain; laguna_seca instead points at a shared tiling texture with
//! world_align. The mapping, found by laying stock road meshes over the
//! decoded brands_hatch pigment: u = (x - cx) / (2 hx) + 0.5 along +X, v
//! = (z - cz) / (2 hz) + 0.5 along +Z, texel row 0 at the low-z edge. The
//! game's Grass, CrossImpostor and some Subsurface materials read it
//! (`Pigment_Enable`); track surfaces do not.
//!
//! Ours: the drivable area plus [`MARGIN`] (inside the drawn geometry), the
//! long side [`SIZE`] texels; each texel is the estimated mean albedo of
//! the top-most upward-facing drawn ground surface (a drawn physics mesh)
//! under it, else of any upward-facing opaque surface; an unpainted texel
//! takes the colour of the nearest painted one (multi-source breadth-first
//! fill); then a Gaussian blur that brings the map's fine-scale smoothness
//! to the stock one (below). BC1 sRGB, as most stock pigments.
//!
//! Smoothness (ours, measured): the lag where a map's mean-removed luma
//! autocorrelation falls to 0.9 ([`corr_lag`]). `aceconv measure pigment`
//! (stockstats/measure_pigment.txt, the stock pigments that map their own
//! track; laguna_seca / nurburgring use a shared tiling texture): median
//! [`STOCK_LAG09_M`] m. For a field blurred by a Gaussian of sigma s the
//! autocorrelation is exp(-r^2 / 4 s^2), so its 0.9 lag is 2 s sqrt(ln
//! 1/0.9) = 0.649 s: the stock maps look blurred by s_stock = lag / 0.649,
//! ours (measured on the filled map) by s_ours, and the blur added is
//! sqrt(s_stock^2 - s_ours^2) (none when ours is already as smooth).

use anyhow::Result;
use rayon::prelude::*;

use super::bakes::{Inputs, Tile, GROUND_BIT};
use crate::io::pb::{Msg, Val};
use crate::io::texture::{self, Fmt, Image};

pub const MARGIN: f32 = 300.0;
pub const SIZE: u32 = 2048;
/// median over the 15 stock pigment maps of the 0.9 autocorrelation lag (m;
/// p25 0.77, p75 2.08; brands_hatch 0.86, monza 3.12)
pub const STOCK_LAG09_M: f64 = 1.09;
/// lag(0.9) / sigma of a Gaussian-blurred field: 2 sqrt(ln(1 / 0.9))
fn lag09_per_sigma() -> f64 {
    2.0 * (1.0f64 / 0.9).ln().sqrt()
}

pub struct Pigment {
    pub lo: [f32; 2],
    pub hi: [f32; 2],
    pub w: u32,
    pub h: u32,
    px: Vec<Option<[f32; 3]>>,
}

pub fn plan(inp: &Inputs, glo: [f32; 2], ghi: [f32; 2]) -> Option<Pigment> {
    let (dlo, dhi) = inp.drivable;
    if !(dlo[0].is_finite() && dhi[0] > dlo[0] && dhi[1] > dlo[1]) {
        return None;
    }
    let lo = [(dlo[0] - MARGIN).max(glo[0]), (dlo[1] - MARGIN).max(glo[1])];
    let hi = [(dhi[0] + MARGIN).min(ghi[0]), (dhi[1] + MARGIN).min(ghi[1])];
    let (ex, ez) = ((hi[0] - lo[0]).max(1.0), (hi[1] - lo[1]).max(1.0));
    let short = |a: f32, b: f32| -> u32 { ((SIZE as f32 * a / b).max(1.0) as u32).next_power_of_two().clamp(256, SIZE) };
    let (w, h) = if ex >= ez { (SIZE, short(ez, ex)) } else { (short(ex, ez), SIZE) };
    Some(Pigment { lo, hi, w, h, px: vec![None; (w * h) as usize] })
}

/// Albedo of the top-most upward-facing ground surface under (x, z).
fn colour_at(tile: &Tile, x: f32, z: f32) -> Option<[f32; 3]> {
    let mut y = tile.top;
    let mut any: Option<[f32; 3]> = None;
    for _ in 0..16 {
        let Some(h) = tile.bvh.hit([x, y, z], [0.0, -1.0, 0.0], 0.0, 1.0e5) else { break };
        if !h.back && h.normal[1] > 0.2 {
            if h.payload & GROUND_BIT != 0 {
                return Some(tile.albedo(h.payload));
            }
            any.get_or_insert(tile.albedo(h.payload));
        }
        y -= h.t + 0.01;
    }
    any
}

impl Pigment {
    fn texel_xz(&self, i: u32, j: u32) -> (f32, f32) {
        (self.lo[0] + (i as f32 + 0.5) / self.w as f32 * (self.hi[0] - self.lo[0]), self.lo[1] + (j as f32 + 0.5) / self.h as f32 * (self.hi[1] - self.lo[1]))
    }

    pub fn bake_tile(&mut self, tile: &Tile) {
        let w = self.w;
        let rows: Vec<(u32, Vec<Option<Option<[f32; 3]>>>)> = (0..self.h).into_par_iter().map(|j| {
            (j, (0..w).map(|i| {
                let (x, z) = self.texel_xz(i, j);
                tile.contains(x, z).then(|| colour_at(tile, x, z))
            }).collect())
        }).collect();
        for (j, r) in rows {
            for (i, c) in r.into_iter().enumerate() {
                if let Some(c) = c {
                    self.px[(j * w) as usize + i] = c;
                }
            }
        }
    }

    pub fn finish(self, inp: &Inputs, graphics: &mut Msg) -> Result<Vec<String>> {
        let (w, h) = (self.w as usize, self.h as usize);
        let painted = self.px.iter().filter(|p| p.is_some()).count();
        let mut img = fill_nearest(&self.px, w, h).unwrap_or_else(|| vec![[0.15, 0.17, 0.1]; w * h]);
        // blur to the stock smoothness
        let m_per_texel = (((self.hi[0] - self.lo[0]) / w as f32 + (self.hi[1] - self.lo[1]) / h as f32) * 0.5) as f64;
        let luma: Vec<f32> = img.iter().map(|c| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]).collect();
        let ours = corr_lag(&luma, w, h, 0.9, 64);
        let s_stock = STOCK_LAG09_M / m_per_texel.max(1e-3) / lag09_per_sigma();
        let s_ours = if ours.is_finite() { ours / lag09_per_sigma() } else { s_stock };
        let sigma = (s_stock * s_stock - s_ours * s_ours).max(0.0).sqrt();
        if sigma > 0.05 {
            img = gaussian(&img, w, h, sigma as f32);
        }
        let mut px = Vec::with_capacity(w * h * 4);
        let mut mean = [0f64; 3];
        for p in &img {
            for c in 0..3 {
                let s = super::textures::srgb(p[c].clamp(0.0, 1.0));
                mean[c] += s as f64;
                px.push(s);
            }
            px.push(255);
        }
        let name = "pigment_map";
        let enc = texture::encode(&Image { w: self.w, h: self.h, px }, Fmt::Bc1D, true, &format!("{name}.png"));
        let dir = inp.out.join("pigment");
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join(format!("{name}.texture")), &enc.header)?;
        std::fs::write(dir.join(format!("{name}.texturemips")), &enc.mips)?;
        let path = format!("content\\tracks\\{}\\pigment\\{name}.texture", inp.track);
        let c = [(self.lo[0] + self.hi[0]) * 0.5, (self.lo[1] + self.hi[1]) * 0.5];
        let half = [(self.hi[0] - self.lo[0]) * 0.5, (self.hi[1] - self.lo[1]) * 0.5];
        let mut p = Msg::default();
        p.push(1, Val::Msg(super::scene::transform([c[0], 0.0, c[1]], [0.0; 3], [half[0], 1000.0, half[1]])));
        p.set_str(2, &path);
        p.push(4, Val::Msg(Msg::default()));
        graphics.set(4, Val::Msg(p));
        let n = (w * h) as f64;
        Ok(vec![format!("  pigment: {}x{} over {:.0} x {:.0} m, {:.0} % painted, mean sRGB ({:.0}, {:.0}, {:.0}); 0.9 lag {:.2} m before blur (stock median {STOCK_LAG09_M} m), Gaussian sigma {:.2} texels",
            w, h, half[0] * 2.0, half[1] * 2.0, painted as f64 / n * 100.0, mean[0] / n, mean[1] / n, mean[2] / n, ours * m_per_texel, sigma)])
    }
}

/// Every unpainted texel takes the colour of the nearest painted one
/// (breadth-first from all painted texels at once, 8-connected); None when
/// nothing is painted.
pub fn fill_nearest(px: &[Option<[f32; 3]>], w: usize, h: usize) -> Option<Vec<[f32; 3]>> {
    let mut out: Vec<Option<[f32; 3]>> = px.to_vec();
    let mut queue: std::collections::VecDeque<usize> = (0..w * h).filter(|&i| px[i].is_some()).collect();
    if queue.is_empty() {
        return None;
    }
    while let Some(i) = queue.pop_front() {
        let (x, y) = ((i % w) as isize, (i / w) as isize);
        let c = out[i];
        for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1), (-1, -1), (1, -1), (-1, 1), (1, 1)] {
            let (nx, ny) = (x + dx, y + dy);
            if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                continue;
            }
            let j = ny as usize * w + nx as usize;
            if out[j].is_none() {
                out[j] = c;
                queue.push_back(j);
            }
        }
    }
    Some(out.into_iter().map(|c| c.unwrap_or([0.0; 3])).collect())
}

/// The lag (texels, mean of the x and z directions, linear between whole
/// lags) where the mean-removed autocorrelation of `img` first falls to
/// `level`; NaN when it stays above it up to `max_lag`.
pub fn corr_lag(img: &[f32], w: usize, h: usize, level: f64, max_lag: usize) -> f64 {
    let n = (w * h) as f64;
    let mean = img.iter().map(|v| *v as f64).sum::<f64>() / n;
    let var = img.iter().map(|v| (*v as f64 - mean).powi(2)).sum::<f64>() / n;
    if var <= 1e-12 {
        return f64::NAN;
    }
    let r = |dx: usize, dz: usize| -> f64 {
        let s: f64 = (0..h - dz).into_par_iter().map(|y| {
            let mut a = 0.0;
            for x in 0..w - dx {
                a += (img[y * w + x] as f64 - mean) * (img[(y + dz) * w + x + dx] as f64 - mean);
            }
            a
        }).sum();
        s / ((h - dz) * (w - dx)).max(1) as f64 / var
    };
    let first = |along_x: bool| -> f64 {
        let mut prev = 1.0;
        for d in 1..=max_lag.min((if along_x { w } else { h }) / 2) {
            let c = if along_x { r(d, 0) } else { r(0, d) };
            if c <= level {
                return d as f64 - 1.0 + (prev - level) / (prev - c).max(1e-12);
            }
            prev = c;
        }
        f64::NAN
    };
    (first(true) + first(false)) / 2.0
}

/// Separable Gaussian blur (radius 3 sigma, clamped at the edges).
fn gaussian(img: &[[f32; 3]], w: usize, h: usize, sigma: f32) -> Vec<[f32; 3]> {
    let r = (3.0 * sigma).ceil().max(1.0) as isize;
    let k: Vec<f32> = (-r..=r).map(|d| (-(d * d) as f32 / (2.0 * sigma * sigma)).exp()).collect();
    let ks: f32 = k.iter().sum();
    let k: Vec<f32> = k.iter().map(|v| v / ks).collect();
    let pass = |src: &[[f32; 3]], horiz: bool| -> Vec<[f32; 3]> {
        let mut out = vec![[0f32; 3]; w * h];
        out.par_chunks_mut(w).enumerate().for_each(|(j, row)| {
            for (i, o) in row.iter_mut().enumerate() {
                let mut acc = [0f32; 3];
                for (t, d) in (-r..=r).enumerate() {
                    let (x, y) = if horiz { ((i as isize + d).clamp(0, w as isize - 1), j as isize) } else { (i as isize, (j as isize + d).clamp(0, h as isize - 1)) };
                    let p = src[y as usize * w + x as usize];
                    for c in 0..3 {
                        acc[c] += p[c] * k[t];
                    }
                }
                *o = acc;
            }
        });
        out
    };
    let a = pass(img, true);
    pass(&a, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_and_smoothness() {
        // two painted texels: every other texel takes the nearer one
        let mut px = vec![None; 8];
        px[0] = Some([1.0, 0.0, 0.0]);
        px[7] = Some([0.0, 1.0, 0.0]);
        let f = fill_nearest(&px, 8, 1).unwrap();
        assert_eq!(f[2], [1.0, 0.0, 0.0]);
        assert_eq!(f[5], [0.0, 1.0, 0.0]);
        assert!(fill_nearest(&[None; 4], 2, 2).is_none());
        // a blurred random field: the measured 0.9 lag gives back sigma
        let (w, h) = (512usize, 512usize);
        let mut seed = 12345u64;
        let noise: Vec<[f32; 3]> = (0..w * h).map(|_| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let v = (seed >> 40) as f32 / (1u64 << 24) as f32;
            [v; 3]
        }).collect();
        let b = gaussian(&noise, w, h, 4.0);
        // away from the clamped edges (they hold extra, less correlated variance)
        let m = 16;
        let l: Vec<f32> = (m..h - m).flat_map(|y| (m..w - m).map(move |x| (x, y))).map(|(x, y)| b[y * w + x][0]).collect();
        let s = corr_lag(&l, w - 2 * m, h - 2 * m, 0.9, 64) / lag09_per_sigma();
        assert!((s - 4.0).abs() < 0.4, "{s}");
    }
}
