//! Stock track measurements behind constants of the track stage
//! (`aceconv measure <what> [content.kspkg]`). Each prints a table and a
//! summary line; the outputs are committed under `stockstats\measure_*.txt`
//! and quoted where the constant is defined.
//!
//! - `phys`: triangles per collision-only stock mesh (the files under
//!   `static_mesh\physics\`), for meshes.rs MAX_PHYS_TRIS.
//! - `frames`: every stock dynamic-track preset's frame against the box of
//!   its non-empty cells and of the layout's AI lines, for dyntrack.rs
//!   `frame_for`.
//! - `pigment`: spatial smoothness of stock pigment maps (the luma
//!   autocorrelation length, in metres), for pigment.rs.

use anyhow::{anyhow, Result};
use std::collections::BTreeMap;

use crate::io::kspkg::Pkg;
use crate::io::pb::{self, Msg};

fn tracks(pkg: &Pkg) -> BTreeMap<String, Vec<usize>> {
    let mut out: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, e) in pkg.entries.iter().enumerate() {
        let l = e.path.to_lowercase();
        if let Some(r) = l.strip_prefix("content\\tracks\\") {
            if let Some((t, _)) = r.split_once('\\') {
                // not stock here: road_atlanta (a local track-slot install);
                // not circuits: the shared / garage folders
                if !matches!(t, "common_assets" | "common_phsx" | "interns" | "road_atlanta") {
                    out.entry(t.to_string()).or_default().push(i);
                }
            }
        }
    }
    out
}

fn pct(v: &[f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v[((v.len() - 1) as f64 * p).round() as usize]
}

/// Triangles of a `.mesh` (LOD 0).
fn mesh_tris(b: &[u8]) -> Option<usize> {
    let top = Msg::parse(b).ok()?;
    let l0 = top.subs(5).into_iter().next()?;
    Some(l0.bytes(11).map(|i| pb::varints(i).len() / 3).unwrap_or(0))
}

pub fn phys(pkg: &Pkg) -> Result<String> {
    let mut out = String::from("track\tcollision_meshes\ttris_p50\ttris_p99\ttris_max\n");
    let mut all: Vec<f64> = Vec::new();
    for (t, idx) in tracks(pkg) {
        let mut v: Vec<f64> = idx.iter().map(|&i| &pkg.entries[i]).filter(|e| {
            let l = e.path.to_lowercase();
            l.contains("\\static_mesh\\physics\\") && l.ends_with(".mesh")
        }).filter_map(|e| pkg.read(e).ok().and_then(|b| mesh_tris(&b))).map(|n| n as f64).collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        if v.is_empty() {
            out += &format!("{t}\t0\t-\t-\t-\n");
            continue;
        }
        out += &format!("{t}\t{}\t{:.0}\t{:.0}\t{:.0}\n", v.len(), pct(&v, 0.5), pct(&v, 0.99), v[v.len() - 1]);
        all.extend(v);
    }
    all.sort_by(|a, b| a.partial_cmp(b).unwrap());
    out += &format!("ALL collision-only stock meshes: n {} p50 {:.0} p90 {:.0} p99 {:.0} max {:.0}\n", all.len(), pct(&all, 0.5), pct(&all, 0.9), pct(&all, 0.99),
        all.last().copied().unwrap_or(f64::NAN));
    Ok(out)
}

pub fn frames(pkg: &Pkg) -> Result<String> {
    use crate::track::dyntrack::{self, CELLS, EMPTY_HEIGHT, TILES};
    let mut out = String::from("track\tpreset\textent\tcells_span_x\tcells_span_z\tai_span_x\tai_span_z\tcentre_off_ai_x\tcentre_off_ai_z\textent/ai_span\n");
    let mut big: Vec<(String, f64, f64)> = Vec::new();
    for (t, idx) in tracks(pkg) {
        // AI lines of the track (every layout): the drivable stand-in
        let (mut alo, mut ahi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
        for &i in &idx {
            let e = &pkg.entries[i];
            if !e.path.to_lowercase().ends_with(".aisplinedata") {
                continue;
            }
            let Ok(p) = pkg.read(e).map_err(|x| anyhow!("{x}")).and_then(|b| crate::track::ai::parse(&b)) else { continue };
            for q in &p.points {
                alo[0] = alo[0].min(q[0] as f64);
                ahi[0] = ahi[0].max(q[0] as f64);
                alo[1] = alo[1].min(q[2] as f64);
                ahi[1] = ahi[1].max(q[2] as f64);
            }
        }
        for &i in &idx {
            let e = &pkg.entries[i];
            if !e.path.to_lowercase().ends_with(".dynamictrackpresetcompressed") {
                continue;
            }
            let Ok(p) = pkg.read(e).map_err(|x| anyhow!("{x}")).and_then(|b| dyntrack::parse(&b)) else { continue };
            let ext = (p.max[0] - p.min[0]) as f64;
            let cell = ext / (TILES as usize * CELLS) as f64;
            let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
            for tl in &p.tiles {
                let (row, col) = ((tl.id / TILES) as usize, (tl.id % TILES) as usize);
                for (k, h) in tl.heights.iter().enumerate() {
                    if *h == EMPTY_HEIGHT {
                        continue;
                    }
                    let (cz, cx) = (row * CELLS + k / CELLS, col * CELLS + k % CELLS);
                    let (x, z) = (p.min[0] as f64 + (cx as f64 + 0.5) * cell, p.min[2] as f64 + (cz as f64 + 0.5) * cell);
                    lo[0] = lo[0].min(x);
                    hi[0] = hi[0].max(x);
                    lo[1] = lo[1].min(z);
                    hi[1] = hi[1].max(z);
                }
            }
            let name = e.path.rsplit('\\').next().unwrap_or("").to_string();
            let ai = [ahi[0] - alo[0], ahi[1] - alo[1]];
            let centre = [p.min[0] as f64 + ext / 2.0, p.min[2] as f64 + ext / 2.0];
            let off = [centre[0] - (alo[0] + ahi[0]) / 2.0, centre[1] - (alo[1] + ahi[1]) / 2.0];
            let ratio = ext / ai[0].max(ai[1]);
            out += &format!("{t}\t{name}\t{ext:.1}\t{:.0}\t{:.0}\t{:.0}\t{:.0}\t{:.0}\t{:.0}\t{ratio:.3}\n", hi[0] - lo[0], hi[1] - lo[1], ai[0], ai[1], off[0], off[1]);
            if ext > 6000.5 {
                big.push((format!("{t}/{name}"), ext, ai[0].max(ai[1])));
            }
        }
    }
    for (n, e, s) in &big {
        out += &format!("ABOVE 6000: {n} extent {e:.1} ai span {s:.0} extent - span {:.0} extent / span {:.3}\n", e - s, e / s);
    }
    Ok(out)
}

/// The pigment map of a stock track: (texture path, half extent x, z).
fn pigment_of(pkg: &Pkg, idx: &[usize], track: &str) -> Option<(String, f32, f32)> {
    let main = format!("content\\tracks\\{track}\\{track}.scene");
    let e = idx.iter().map(|&i| &pkg.entries[i]).find(|e| e.path.eq_ignore_ascii_case(&main))
        .or_else(|| idx.iter().map(|&i| &pkg.entries[i]).find(|e| e.path.to_lowercase().ends_with(".scene") && pkg.read(e).ok().and_then(|b| Msg::parse(&b).ok()).and_then(|s| s.sub(3)).and_then(|g| g.sub(4)).is_some()))?;
    let s = Msg::parse(&pkg.read(e).ok()?).ok()?;
    let p = s.sub(3)?.sub(4)?;
    let tex = p.str(2).filter(|t| !t.is_empty())?;
    let sc = p.sub(1)?.sub(3)?;
    Some((tex, sc.f32(1).unwrap_or(0.0), sc.f32(3).unwrap_or(0.0)))
}


pub fn pigment(pkg: &Pkg) -> Result<String> {
    use crate::track::pigment::corr_lag;
    let mut out = String::from("track\ttexture\tw\th\tm_per_texel\tlag_0.9_m\tlag_1/e_m (mip <= 512)\n");
    let (mut fine, mut coarse) = (Vec::new(), Vec::new());
    for (t, idx) in tracks(pkg) {
        let Some((tex, hx, hz)) = pigment_of(pkg, &idx, &t) else {
            out += &format!("{t}\t(no pigment map)\n");
            continue;
        };
        if !tex.to_lowercase().starts_with(&format!("content\\tracks\\{t}\\")) {
            out += &format!("{t}\t{tex}\t(shared tiling texture, world aligned: not a map of the track)\n");
            continue;
        }
        let Some(he) = pkg.find(&tex).and_then(|e| pkg.read(e).ok()) else { continue };
        let me = pkg.find(&format!("{tex}mips"));
        let page = |i: usize| -> Option<Vec<u8>> { me.and_then(|e| pkg.raw(e)).and_then(|r| r.get(i * 65536..(i + 1) * 65536).map(|s| s.to_vec())) };
        let luma = |max: u32| -> Option<(usize, usize, Vec<f32>)> {
            let (w, h, px, srgb) = super::albedo::texture_mip(&he, &page, max)?;
            let lin = |v: u8| {
                let c = v as f32 / 255.0;
                if !srgb { c } else if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
            };
            Some((w as usize, h as usize, px.iter().map(|c| 0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])).collect()))
        };
        let (Some((w, h, img)), Some((cw, ch, cimg))) = (luma(4096), luma(512)) else {
            out += &format!("{t}\t{tex}\t(not decodable)\n");
            continue;
        };
        let mpt = |w: usize, h: usize| ((2.0 * hx) as f64 / w as f64 + (2.0 * hz) as f64 / h as f64) / 2.0;
        let f = corr_lag(&img, w, h, 0.9, 64) * mpt(w, h);
        let c = corr_lag(&cimg, cw, ch, (-1.0f64).exp(), 256) * mpt(cw, ch);
        out += &format!("{t}\t{tex}\t{w}\t{h}\t{:.2}\t{f:.2}\t{c:.1}\n", mpt(w, h));
        if f.is_finite() {
            fine.push(f);
        }
        if c.is_finite() {
            coarse.push(c);
        }
    }
    for v in [&mut fine, &mut coarse] {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    }
    let line = |v: &[f64]| format!("n {} min {:.2} p25 {:.2} p50 {:.2} p75 {:.2} max {:.2}", v.len(), v.first().copied().unwrap_or(f64::NAN), pct(v, 0.25), pct(v, 0.5), pct(v, 0.75), v.last().copied().unwrap_or(f64::NAN));
    out += &format!("ALL stock pigments, lag where the autocorrelation falls to 0.9 (m): {}\n", line(&fine));
    out += &format!("ALL stock pigments, lag where it falls to 1/e (m): {}\n", line(&coarse));
    Ok(out)
}
