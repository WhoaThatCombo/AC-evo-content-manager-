//! Track textures: every image a converted material uses, encoded once in
//! the format stock tracks use for that slot.
//!
//! Formats (measured over the stock track materials of content.kspkg, the
//! `.texture` header of every filled slot; road_atlanta left out):
//!
//! | use | stock majority | here | cap (ours, stock sizes) |
//! |---|---|---|---|
//! | base colour (`txDiffuse`, `Base_BaseColorMap`) | BC7 sRGB (34) | BC7 sRGB | 4096 (stock: 2048 typical, 4096 on terrain / road bases) |
//! | layer colour (`Red_BaseColorMap`, ...) | BC1 sRGB (13) | BC1 sRGB | 2048 |
//! | normal (`NormalMap`, `Base_NormalMap`) | BC6H (26) | BC6H | 2048 (stock 1024 typical) |
//! | roughness (`RoughnessMap`) | BC4 (27) | BC4 | 1024 (stock 512 typical) |
//! | splat mask (`SpatMap`) | BC1 / BC7 linear (12 / 33) | BC7 linear | 2048 |
//! | flipbook (`txDiffuse` of Particle) | BC7 sRGB (34) | BC7 sRGB | 1024 |
//!
//! Sizes become powers of two (nearest per axis, then the cap). Alpha is
//! kept only where a transparent material uses the image. The brightness
//! gain (look.rs) is baked into base colour textures, in linear light, so
//! identical pixels at one gain are written once.

use anyhow::Result;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::scan::Scan;
use crate::io::texture::{self, Fmt, Image};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Albedo { alpha: bool },
    Layer,
    Normal,
    Rough,
    Splat,
    Flipbook,
}

impl Kind {
    fn fmt(self) -> (Fmt, bool) {
        match self {
            Kind::Albedo { .. } | Kind::Flipbook => (Fmt::Bc7, true),
            Kind::Layer => (Fmt::Bc1D, true),
            Kind::Normal => (Fmt::Bc6h, false),
            Kind::Rough => (Fmt::Bc4, false),
            Kind::Splat => (Fmt::Bc7Linear, false),
        }
    }
    fn cap(self) -> u32 {
        match self {
            Kind::Rough | Kind::Flipbook => 1024,
            Kind::Albedo { .. } => 4096,
            _ => 2048,
        }
    }
    fn suffix(self) -> &'static str {
        match self {
            Kind::Albedo { .. } | Kind::Layer => "",
            Kind::Normal => "_nm",
            Kind::Rough => "_r",
            Kind::Splat => "_mask",
            Kind::Flipbook => "_flip",
        }
    }
}

/// Where a texture's pixels come from.
#[derive(Clone, Debug)]
pub enum Src {
    /// an AC1 texture as it is
    Ac1(usize),
    /// AC1 multilayer base: txDiffuse with the baked channels and the
    /// coverage compensation folded in (look.rs, materials.rs)
    Multi(Box<MultiBake>),
    /// roughness from an AC1 `txMaps` (R specular level, G glossiness)
    Rough { maps: usize, spec: f32, exp: f32 },
    /// the SpatMap: mask channels picked into R, G, B
    Splat { mask: usize, chans: Vec<usize> },
    /// a 2-frame flipbook: [lit | dark] side by side
    Flipbook { tex: usize },
}

#[derive(Clone, Debug)]
pub struct MultiBake {
    pub diffuse: Option<usize>,
    pub mask: Option<usize>,
    /// kept channels: (mask channel, mean linear colour of its detail, mean
    /// luma of the layer as written - normalised)
    pub kept: Vec<(usize, [f32; 3], f32)>,
    /// baked channels: (mask channel, linear colour it adds)
    pub baked: Vec<(usize, [f32; 3])>,
    pub magic: f32,
    /// the material's Detail_BaseColorMultiplier
    pub detail_mult: f32,
    /// the uncovered part shows the base (look::Multi::remainder)
    pub remainder: bool,
}

#[derive(Clone, Debug)]
pub struct Job {
    pub src: Src,
    pub kind: Kind,
    pub gain: f32,
    pub name: String,
}

#[derive(Default)]
pub struct Textures {
    jobs: Vec<Job>,
    paths: Vec<String>,
    index: HashMap<String, usize>,
    names: HashSet<String>,
    failed: HashSet<usize>,
    means: HashMap<(usize, bool), [f32; 3]>,
    pub count: usize,
    pub bytes: usize,
    pub notes: Vec<String>,
    /// archive path -> mean linear colour of what was written (colour
    /// kinds) or mean value (masks), for the look report
    pub written_mean: HashMap<String, [f32; 3]>,
}

/// sRGB byte -> linear.
pub fn lin(v: u8) -> f32 {
    let c = v as f32 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// linear -> sRGB byte.
pub fn srgb(v: f32) -> u8 {
    let c = v.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0 + 0.5) as u8
}

fn lut() -> &'static [f32; 256] {
    static L: std::sync::OnceLock<[f32; 256]> = std::sync::OnceLock::new();
    L.get_or_init(|| {
        let mut t = [0f32; 256];
        for (i, v) in t.iter_mut().enumerate() {
            *v = lin(i as u8);
        }
        t
    })
}

pub fn pot(v: u32, cap: u32) -> u32 {
    let mut p = 1u32;
    while p * 2 <= v.max(1) {
        p *= 2;
    }
    // nearest power of two
    if p * 2 <= cap * 2 && (v - p) as f32 > (p * 2 - v) as f32 {
        p *= 2;
    }
    p.clamp(1, cap)
}

/// Bilinear resample (2x box steps first for big reductions).
pub fn resample(img: &Image, w: u32, h: u32) -> Image {
    let mut cur = img.clone();
    while cur.w >= w * 2 && cur.h >= h * 2 {
        cur = texture::half(&cur, true);
    }
    if (cur.w, cur.h) == (w, h) {
        return cur;
    }
    let mut px = vec![0u8; (w * h * 4) as usize];
    let (sx, sy) = (cur.w as f32 / w as f32, cur.h as f32 / h as f32);
    for y in 0..h {
        let fy = ((y as f32 + 0.5) * sy - 0.5).max(0.0);
        let (y0, ty) = (fy as u32, fy.fract());
        let y1 = (y0 + 1).min(cur.h - 1);
        for x in 0..w {
            let fx = ((x as f32 + 0.5) * sx - 0.5).max(0.0);
            let (x0, tx) = (fx as u32, fx.fract());
            let x1 = (x0 + 1).min(cur.w - 1);
            let at = |xx: u32, yy: u32, c: usize| cur.px[((yy.min(cur.h - 1) * cur.w + xx.min(cur.w - 1)) * 4) as usize + c] as f32;
            for c in 0..4 {
                let v = at(x0, y0, c) * (1.0 - tx) * (1.0 - ty) + at(x1, y0, c) * tx * (1.0 - ty) + at(x0, y1, c) * (1.0 - tx) * ty + at(x1, y1, c) * tx * ty;
                px[((y * w + x) * 4) as usize + c] = (v + 0.5).clamp(0.0, 255.0) as u8;
            }
        }
    }
    Image { w, h, px }
}

fn sized(img: Image, cap: u32) -> Image {
    let (w, h) = (pot(img.w, cap), pot(img.h, cap));
    if (w, h) != (img.w, img.h) { resample(&img, w, h) } else { img }
}

/// Mean colour of an image (linear for sRGB images), alpha-weighted.
fn image_mean(img: &Image, srgb_: bool) -> [f32; 3] {
    let l = lut();
    let (mut acc, mut w) = ([0f64; 3], 0f64);
    for p in img.px.chunks_exact(4).step_by(7) {
        let a = p[3] as f64 / 255.0;
        for c in 0..3 {
            acc[c] += a * if srgb_ { l[p[c] as usize] as f64 } else { p[c] as f64 / 255.0 };
        }
        w += a;
    }
    acc.map(|x| (x / w.max(1e-6)) as f32)
}

impl Textures {
    /// Ask for a texture; returns its archive path (the file is written by
    /// `write_all`, after which `ok(path)` says whether it made it).
    pub fn want(&mut self, scan: &Scan, id: &str, job: Job) -> String {
        let key = format!("{:?}|{:?}|{:.3}", job.src, job.kind, job.gain);
        if let Some(&i) = self.index.get(&key) {
            return self.paths[i].clone();
        }
        let _ = scan;
        let base = super::materials::safe(job.name.rsplit_once('.').map(|x| x.0).unwrap_or(&job.name));
        let mut stem = format!("{base}{}", job.kind.suffix());
        if (job.gain - 1.0).abs() > 0.004 {
            stem += &format!("_g{:03}", (job.gain * 100.0).round() as i32);
        }
        while !self.names.insert(stem.clone()) {
            stem.push('_');
        }
        let path = format!("content\\tracks\\{id}\\textures\\{stem}.texture");
        self.jobs.push(job);
        self.paths.push(path.clone());
        self.index.insert(key, self.jobs.len() - 1);
        path
    }

    /// Did the texture at `path` get written?
    pub fn ok(&self, path: &str) -> bool {
        self.paths.iter().position(|p| p == path).map(|i| !self.failed.contains(&i)).unwrap_or(false)
    }

    /// Mean linear colour of an AC1 texture (`texel0`: its first texel, what
    /// AC1 samples for a detail layer with mult 0).
    pub fn mean(&mut self, scan: &Scan, t: usize, texel0: bool) -> [f32; 3] {
        if let Some(m) = self.means.get(&(t, texel0)) {
            return *m;
        }
        let m = texture::decode_any(&scan.texture_bytes(t)).map(|img| {
            let l = lut();
            if texel0 {
                return [l[img.px[0] as usize], l[img.px[1] as usize], l[img.px[2] as usize]];
            }
            // a small copy is plenty for a mean
            let s = if img.w > 256 { texture::resize_to(&img, 256, true) } else { img };
            let mut acc = [0f64; 3];
            for p in s.px.chunks_exact(4) {
                for c in 0..3 {
                    acc[c] += l[p[c] as usize] as f64;
                }
            }
            let n = (s.px.len() / 4).max(1) as f64;
            [(acc[0] / n) as f32, (acc[1] / n) as f32, (acc[2] / n) as f32]
        }).unwrap_or([0.5; 3]);
        self.means.insert((t, texel0), m);
        m
    }

    /// Mean of each channel of a mask texture (raw 0..1, no colour space).
    pub fn mask_mean(&mut self, scan: &Scan, t: usize) -> [f32; 4] {
        texture::decode_any(&scan.texture_bytes(t)).map(|img| {
            let s = if img.w > 256 { texture::resize_to(&img, 256, false) } else { img };
            let mut acc = [0f64; 4];
            for p in s.px.chunks_exact(4) {
                for c in 0..4 {
                    acc[c] += p[c] as f64 / 255.0;
                }
            }
            let n = (s.px.len() / 4).max(1) as f64;
            acc.map(|a| (a / n) as f32)
        }).unwrap_or([1.0, 0.0, 0.0, 0.0])
    }

    /// Mean of every planned texture as `aceconv albedo` reads it back
    /// (stats/albedo.rs `texture_mean`: the largest mip of at most 64
    /// texels, decoded): each texture is rendered at its full size (the
    /// brightness gain clips bright texels there, before any mip), halved
    /// down to that mip, encoded in its own format and decoded again;
    /// nothing is written. Archive path -> mean (linear for colour).
    pub fn probe_means(&self, scan: &Scan) -> HashMap<String, [f32; 4]> {
        let ids: Vec<usize> = (0..self.jobs.len()).collect();
        ids.par_iter().filter_map(|&i| {
            let job = &self.jobs[i];
            let (fmt, srgb_) = job.kind.fmt();
            let mut img = self.render(scan, job, None).ok()?;
            while img.w > 64 || img.h > 64 {
                img = texture::half(&img, srgb_);
            }
            let enc = texture::encode(&img, fmt, srgb_, &job.name);
            let page = |k: usize| enc.mips.get(k * 65536..(k + 1) * 65536).map(|s| s.to_vec());
            let m = crate::stats::albedo::texture_mean(&enc.header, &page).unwrap_or_else(|| {
                let m = image_mean(&img, srgb_);
                [m[0], m[1], m[2], 1.0]
            });
            Some((self.paths[i].clone(), m))
        }).collect()
    }

    fn render(&self, scan: &Scan, job: &Job, cap: Option<u32>) -> Result<Image> {
        let cap = cap.map(|c| c.min(job.kind.cap())).unwrap_or(job.kind.cap());
        let l = lut();
        let decode = |t: usize| texture::decode_any(&scan.texture_bytes(t));
        let mut img = match &job.src {
            Src::Ac1(t) => sized(decode(*t)?, cap),
            Src::Rough { maps, spec, exp } => {
                let mut img = sized(decode(*maps)?, cap);
                for p in img.px.chunks_exact_mut(4) {
                    let s = *spec * (p[0] as f32 / 255.0);
                    let e = *exp * (p[1] as f32 / 255.0).max(0.05);
                    let r = super::look::roughness_of(s, e);
                    p[0] = (r * 255.0 + 0.5) as u8;
                    p[1] = p[0];
                    p[2] = p[0];
                    p[3] = 255;
                }
                img
            }
            Src::Splat { mask, chans } => {
                let mut img = sized(decode(*mask)?, cap);
                for p in img.px.chunks_exact_mut(4) {
                    // channel 4 = inverse alpha (the MultiMap detail weight)
                    let src = [p[0], p[1], p[2], p[3], 255 - p[3]];
                    for k in 0..3 {
                        p[k] = chans.get(k).map(|&c| src[c]).unwrap_or(0);
                    }
                    p[3] = 255;
                }
                img
            }
            Src::Flipbook { tex } => {
                let one = sized(decode(*tex)?, cap / 2);
                let (w, h) = (one.w, one.h);
                let mut px = vec![0u8; (w * 2 * h * 4) as usize];
                for y in 0..h {
                    for x in 0..w {
                        let s = ((y * w + x) * 4) as usize;
                        let lit = ((y * w * 2 + x) * 4) as usize;
                        let dark = ((y * w * 2 + w + x) * 4) as usize;
                        px[lit..lit + 4].copy_from_slice(&one.px[s..s + 4]);
                        // the dark frame: the lamp body at 8 % (ours)
                        for c in 0..3 {
                            px[dark + c] = srgb(l[one.px[s + c] as usize] * 0.08);
                        }
                        px[dark + 3] = one.px[s + 3];
                    }
                }
                Image { w: w * 2, h, px }
            }
            Src::Multi(b) => {
                let base = match b.diffuse {
                    Some(t) => sized(decode(t)?, cap),
                    None => Image { w: 4, h: 4, px: vec![255; 64] },
                };
                let mask = match b.mask {
                    Some(t) => Some(resample(&decode(t)?, base.w, base.h)),
                    None => None,
                };
                let mut out = base.clone();
                for i in 0..(base.w * base.h) as usize {
                    // AC1: D x magic x (sum_kept m L + sum_baked m col + R), R the
                    // uncovered remainder of a detail lerp; EVO sum mode: where
                    // the layers cover, (sum m/S L) x DetailMul x D'; uncovered, D'
                    let m: [f32; 5] = match &mask {
                        Some(mk) => {
                            let a = mk.px[i * 4 + 3] as f32 / 255.0;
                            [mk.px[i * 4] as f32 / 255.0, mk.px[i * 4 + 1] as f32 / 255.0, mk.px[i * 4 + 2] as f32 / 255.0, a, 1.0 - a]
                        }
                        None => [1.0, 0.0, 0.0, 0.0, 0.0],
                    };
                    let mk: f32 = b.kept.iter().map(|(c, _, _)| m[*c]).sum();
                    let all: f32 = mk + b.baked.iter().map(|(c, _)| m[*c]).sum::<f32>();
                    let rem = if b.remainder { (1.0 - all).max(0.0) } else { 0.0 };
                    let wb = (1.0 - mk).clamp(0.0, 1.0);
                    let s = mk.max(1.0);
                    // what EVO's covered region multiplies D' by: the written
                    // (normalised) layers' level x the Detail multiplier
                    let evo: f32 = b.kept.iter().map(|(ch, _, n)| m[*ch] / s * n).sum::<f32>() * b.detail_mult;
                    for c in 0..3 {
                        let k: f32 = b.kept.iter().map(|(ch, mean, _)| m[*ch] * mean[c]).sum();
                        let bb: f32 = b.baked.iter().map(|(ch, col)| m[*ch] * col[c]).sum::<f32>() + rem;
                        let cover = if b.kept.is_empty() { 0.0 } else { b.magic * (k + bb) / evo.max(1e-3) };
                        let open = if b.baked.is_empty() && !b.remainder { 1.0 } else { b.magic * bb / wb.max(1e-3) };
                        let f = cover * (1.0 - wb) + open.min(8.0) * wb;
                        out.px[i * 4 + c] = srgb(l[base.px[i * 4 + c] as usize] * f.clamp(0.0, 8.0));
                    }
                }
                out
            }
        };
        if job.kind == Kind::Layer && (job.gain - 1.0).abs() > 0.004 {
            for p in img.px.chunks_exact_mut(4) {
                for c in 0..3 {
                    p[c] = srgb(l[p[c] as usize] * job.gain);
                }
            }
        }
        if let Kind::Albedo { alpha } = job.kind {
            if (job.gain - 1.0).abs() > 0.004 {
                for p in img.px.chunks_exact_mut(4) {
                    for c in 0..3 {
                        p[c] = srgb(l[p[c] as usize] * job.gain);
                    }
                }
            }
            if !alpha {
                for p in img.px.chunks_exact_mut(4) {
                    p[3] = 255;
                }
            }
        }
        if !matches!(job.kind, Kind::Albedo { alpha: true } | Kind::Flipbook) {
            for p in img.px.chunks_exact_mut(4) {
                p[3] = 255;
            }
        }
        Ok(img)
    }

    /// Encode and write every wanted texture into `<out>\textures\`.
    pub fn write_all(&mut self, scan: &Scan, out: &Path) -> Result<()> {
        std::fs::create_dir_all(out.join("textures"))?;
        let ids: Vec<usize> = (0..self.jobs.len()).collect();
        // one image per worker at a time (a 4096 base map with its mips and
        // a resampled mask is a few hundred MB)
        for chunk in ids.chunks(rayon::current_num_threads().clamp(1, 4)) {
            let done: Vec<(usize, Result<(texture::Encoded, [f32; 3])>)> = chunk.par_iter().map(|&i| {
                let job = &self.jobs[i];
                let r = self.render(scan, job, None).map(|img| {
                    let (fmt, srgb) = job.kind.fmt();
                    (texture::encode(&img, fmt, srgb, &job.name), image_mean(&img, srgb))
                });
                (i, r)
            }).collect();
            for (i, r) in done {
                match r {
                    Ok((e, mean)) => {
                        self.written_mean.insert(self.paths[i].clone(), mean);
                        let stem = self.paths[i].rsplit('\\').next().unwrap().trim_end_matches(".texture").to_string();
                        std::fs::write(out.join("textures").join(format!("{stem}.texture")), &e.header)?;
                        std::fs::write(out.join("textures").join(format!("{stem}.texturemips")), &e.mips)?;
                        self.bytes += e.mips.len() + e.header.len();
                        self.count += 1;
                    }
                    Err(e) => {
                        self.failed.insert(i);
                        self.notes.push(format!("texture {}: {e:#} (slot left empty)", self.jobs[i].name));
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_colour() {
        assert_eq!(pot(1000, 2048), 1024);
        assert_eq!(pot(700, 2048), 512);
        assert_eq!(pot(4096, 2048), 2048);
        for v in [0u8, 1, 50, 128, 200, 255] {
            assert_eq!(srgb(lin(v)), v);
        }
        let img = Image { w: 3, h: 5, px: vec![200; 60] };
        let r = resample(&img, 4, 4);
        assert!(r.px.iter().all(|&v| v == 200));
    }
}
