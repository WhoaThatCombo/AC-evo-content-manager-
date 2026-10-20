//! Survey the stock EXT_DISC batches of content.kspkg: per car the disc
//! triangles, the LF disc's radial / axial extent around its DISC_LF bone,
//! the UV law (planar in the disc plane or not), and the rim / tyre radii.
//! Calibrates the generated fallback disc (src/car/disc.rs).
//!
//! cargo run --release --example discsurvey -- <content.kspkg> [car id filter]

use aceconv::car::geometry::bone_worlds;
use aceconv::io::kspkg::Pkg;
use aceconv::io::pb::Msg;
use std::collections::HashMap;

fn f32s(b: Option<&[u8]>) -> Vec<f32> {
    b.map(|b| b.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect()).unwrap_or_default()
}

fn radial_extent(mesh: &[u8]) -> Option<(f64, f64)> {
    let top = Msg::parse(mesh).ok()?;
    let lod = top.subs(5).into_iter().next()?;
    let p = f32s(lod.bytes(5));
    let r: Vec<f64> = p.chunks_exact(3).map(|c| ((c[1] * c[1] + c[2] * c[2]) as f64).sqrt()).collect();
    Some((r.iter().copied().fold(f64::INFINITY, f64::min), r.iter().copied().fold(0.0, f64::max)))
}

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let pkg = Pkg::open(std::path::Path::new(&a[1]))?;
    let filt = a.get(2).cloned().unwrap_or_default();
    let idx: HashMap<String, usize> = pkg.entries.iter().enumerate().map(|(i, e)| (e.path.to_lowercase(), i)).collect();
    let get = |p: &str| idx.get(&p.to_lowercase()).and_then(|&i| pkg.read(&pkg.entries[i]).ok());
    let mut ids: Vec<String> = pkg.entries.iter().filter_map(|e| {
        let l = e.path.to_lowercase();
        let r = l.strip_prefix("content\\cars\\")?;
        let (id, file) = r.split_once('\\')?;
        (file == format!("{id}.actor")).then(|| id.to_string())
    }).collect();
    ids.sort();
    ids.dedup();
    println!("car\tlod_tris\trmin\trmax\tax_lo\tax_hi\trim_r\ttyre_inner\ttyre_r\tuv\tuv_fit\tmaterial");
    for id in ids.iter().filter(|i| i.contains(&filt)) {
        let Some(ab) = get(&format!("content\\cars\\{id}\\{id}.actor")) else { continue };
        let Ok(act) = Msg::parse(&ab) else { continue };
        let Some(car) = act.sub(103) else { continue };
        let bm = car.sub(12).unwrap_or_default();
        let Some(body) = bm.str(1).and_then(|p| get(&p)) else { continue };
        let rim = bm.sub(6).and_then(|s| s.str(1)).and_then(|p| get(&p)).and_then(|b| radial_extent(&b));
        let tyre = bm.sub(2).and_then(|s| s.str(1)).and_then(|p| get(&p)).and_then(|b| radial_extent(&b));
        let Ok(top) = Msg::parse(&body) else { continue };
        let lods = top.subs(5);
        let mut lod_tris = Vec::new();
        let mut line = String::new();
        for (li, l) in lods.iter().enumerate() {
            let discs: Vec<Msg> = l.subs(4).into_iter().filter(|s| s.str(1).as_deref() == Some("EXT_DISC")).collect();
            lod_tris.push(format!("{}x{}", discs.len(), discs.iter().map(|s| s.varint(3).unwrap_or(0) / 3).sum::<u64>()));
            if li != 0 || discs.is_empty() {
                continue;
            }
            let pos = f32s(l.bytes(5));
            let uvs = f32s(l.bytes(7));
            let bi = f32s(l.bytes(10));
            let ix = l.bytes(11).map(aceconv::io::pb::varints).unwrap_or_default();
            let bones = l.subs(12);
            let names: Vec<String> = bones.iter().map(|b| b.str(1).unwrap_or_default()).collect();
            let worlds = bone_worlds(&bones);
            let Some(lf) = names.iter().position(|n| n == "DISC_LF") else { line = "no DISC_LF".into(); continue };
            let w = worlds[lf];
            let mut vs: Vec<usize> = Vec::new();
            for s in &discs {
                let (st, n) = (s.varint(2).unwrap_or(0) as usize, s.varint(3).unwrap_or(0) as usize);
                vs.extend(ix.get(st..st + n).unwrap_or(&[]).iter().map(|&i| i as usize));
            }
            vs.sort_unstable();
            vs.dedup();
            let vs: Vec<usize> = vs.into_iter().filter(|&v| bi.get(v * 4).map(|&b| b as usize == lf).unwrap_or(false)).collect();
            let (mut rmin, mut rmax, mut alo, mut ahi) = (f64::INFINITY, 0f64, f64::INFINITY, f64::NEG_INFINITY);
            let (mut ulo, mut vlo, mut uhi, mut vhi) = (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
            // planar UV fit: u, v ~ affine in (y, z) about the bone
            let mut rows: Vec<([f64; 3], [f64; 2])> = Vec::new();
            for &v in &vs {
                let p = [pos[v * 3] as f64 - w[12] as f64, pos[v * 3 + 1] as f64 - w[13] as f64, pos[v * 3 + 2] as f64 - w[14] as f64];
                let r = (p[1] * p[1] + p[2] * p[2]).sqrt();
                rmin = rmin.min(r);
                rmax = rmax.max(r);
                alo = alo.min(p[0]);
                ahi = ahi.max(p[0]);
                let (u, vv) = (uvs[v * 2], uvs[v * 2 + 1]);
                ulo = ulo.min(u);
                vlo = vlo.min(vv);
                uhi = uhi.max(u);
                vhi = vhi.max(vv);
                rows.push(([1.0, p[1], p[2]], [u as f64, vv as f64]));
            }
            let fit = |k: usize| -> (f64, [f64; 3]) {
                // normal equations 3x3
                let mut m = [[0f64; 3]; 3];
                let mut b = [0f64; 3];
                for (x, t) in &rows {
                    for i in 0..3 {
                        for j in 0..3 {
                            m[i][j] += x[i] * x[j];
                        }
                        b[i] += x[i] * t[k];
                    }
                }
                let det = |m: &[[f64; 3]; 3]| m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
                let d = det(&m);
                let mut c = [0f64; 3];
                for col in 0..3 {
                    let mut mm = m;
                    for r in 0..3 {
                        mm[r][col] = b[r];
                    }
                    c[col] = det(&mm) / d;
                }
                let res = rows.iter().map(|(x, t)| (t[k] - (c[0] + c[1] * x[1] + c[2] * x[2])).powi(2)).sum::<f64>() / rows.len().max(1) as f64;
                (res.sqrt(), c)
            };
            let (ru, cu) = fit(0);
            let (rv, cv) = fit(1);
            let mat = discs[0].str(4).unwrap_or_default();
            line = format!("{rmin:.3}\t{rmax:.3}\t{alo:.3}\t{ahi:.3}\t{}\t{}\t{}\tuv {ulo:.2},{vlo:.2}..{uhi:.2},{vhi:.2}\tres {ru:.3}/{rv:.3} u={:.2}+{:.2}y+{:.2}z v={:.2}+{:.2}y+{:.2}z n={}\t{mat}",
                rim.map(|r| format!("{:.3}", r.1)).unwrap_or("-".into()),
                tyre.map(|r| format!("{:.3}", r.0)).unwrap_or("-".into()),
                tyre.map(|r| format!("{:.3}", r.1)).unwrap_or("-".into()),
                cu[0], cu[1], cu[2], cv[0], cv[1], cv[2], vs.len());
        }
        println!("{id}\t{}\t{line}", lod_tris.join(","));
    }
    Ok(())
}
