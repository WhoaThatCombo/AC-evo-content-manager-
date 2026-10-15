//! .aisplinedata readers - ported from tools/parse_spline.py and
//! parse_edges.py. Plain protobuf: field 1 holds the point run (each point's
//! field 1 wraps a vec3 of fixed32 floats), field 2 a parallel run of
//! per-point frames (f9 up, f10 tangent, f3/f4 left/right width).

use std::collections::HashMap;
use std::path::Path;

enum W {
    Var,
    Len(usize, usize),
    F32(f32),
    F64,
}

fn varint(d: &[u8], mut i: usize) -> Option<(u64, usize)> {
    let (mut r, mut s) = (0u64, 0u32);
    loop {
        let b = *d.get(i)?;
        i += 1;
        r |= ((b & 0x7f) as u64) << s;
        if b & 0x80 == 0 {
            return Some((r, i));
        }
        s += 7;
        if s > 63 {
            return None;
        }
    }
}

/// (field number, value) pairs between i and end; stops on bad wire types.
fn fields(d: &[u8], mut i: usize, end: usize) -> Vec<(u64, W)> {
    let mut out = Vec::new();
    while i < end {
        let Some((key, ni)) = varint(d, i) else { break };
        i = ni;
        let (f, wt) = (key >> 3, key & 7);
        let v = match wt {
            0 => {
                let Some((_, ni)) = varint(d, i) else { break };
                i = ni;
                W::Var
            }
            2 => {
                let Some((ln, ni)) = varint(d, i) else { break };
                let s = ni;
                i = ni + ln as usize;
                if i > d.len() {
                    break;
                }
                W::Len(s, i)
            }
            5 => {
                if i + 4 > d.len() {
                    break;
                }
                let v = f32::from_le_bytes(d[i..i + 4].try_into().unwrap());
                i += 4;
                W::F32(v)
            }
            1 => {
                if i + 8 > d.len() {
                    break;
                }
                i += 8;
                W::F64
            }
            _ => break,
        };
        out.push((f, v));
    }
    out
}

/// The ideal-line polyline (x, y, z).
pub fn points(path: &Path) -> Vec<[f64; 3]> {
    let Ok(d) = std::fs::read(path) else { return Vec::new() };
    let mut out = Vec::new();
    for (f, v) in fields(&d, 0, d.len()) {
        let (1, W::Len(s, e)) = (f, v) else { continue };
        for (pf, pv) in fields(&d, s, e) {
            let (1, W::Len(ps, pe)) = (pf, pv) else { continue };
            for (vf, vv) in fields(&d, ps, pe) {
                let (1, W::Len(vs, ve)) = (vf, vv) else { continue };
                let mut xyz = HashMap::new();
                for (cf, cv) in fields(&d, vs, ve) {
                    if let W::F32(x) = cv {
                        xyz.insert(cf, x as f64);
                    }
                }
                if xyz.len() >= 3 {
                    if let (Some(x), Some(y), Some(z)) = (xyz.get(&1), xyz.get(&2), xyz.get(&3)) {
                        out.push([*x, *y, *z]);
                    }
                }
            }
        }
        break;
    }
    out
}

struct Meta {
    up: Option<Vec<f64>>,
    tan: Option<Vec<f64>>,
    wl: Option<f64>,
    wr: Option<f64>,
}

fn meta(d: &[u8]) -> Vec<Meta> {
    let mut out = Vec::new();
    for (f, v) in fields(d, 0, d.len()) {
        let (2, W::Len(s, e)) = (f, v) else { continue };
        let mut m = Meta { up: None, tan: None, wl: None, wr: None };
        for (a, b) in fields(d, s, e) {
            match b {
                W::F32(x) => match a {
                    3 => m.wl = Some(x as f64),
                    4 => m.wr = Some(x as f64),
                    _ => {}
                },
                W::Len(bs, be) => {
                    let v: Vec<f64> = fields(d, bs, be).into_iter()
                        .filter_map(|(_, w)| if let W::F32(x) = w { Some(x as f64) } else { None }).collect();
                    match a {
                        9 => m.up = Some(v),
                        10 => m.tan = Some(v),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        out.push(m);
    }
    out
}

fn norm(v: &[f64]) -> Vec<f64> {
    let m = v.iter().map(|c| c * c).sum::<f64>().sqrt();
    if m != 0.0 { v.iter().map(|c| c / m).collect() } else { v.to_vec() }
}

fn cross(a: &[f64], b: &[f64]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// (left, right) track edges as (x, z).
pub fn edges(path: &Path) -> (Vec<[f64; 2]>, Vec<[f64; 2]>) {
    let pts = points(path);
    let Ok(d) = std::fs::read(path) else { return (Vec::new(), Vec::new()) };
    let md = meta(&d);
    let (mut left, mut right) = (Vec::new(), Vec::new());
    for (i, p) in pts.iter().enumerate() {
        let Some(m) = md.get(i) else { break };
        let (Some(up), Some(tan), Some(wl), Some(wr)) = (&m.up, &m.tan, m.wl, m.wr) else { continue };
        if up.len() < 3 || tan.len() < 3 {
            continue;
        }
        let lat = norm(&cross(&norm(up), &norm(tan)));
        left.push([p[0] + lat[0] * wl, p[2] + lat[2] * wl]);
        right.push([p[0] - lat[0] * wr, p[2] - lat[2] * wr]);
    }
    (left, right)
}
