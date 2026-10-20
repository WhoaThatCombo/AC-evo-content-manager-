//! Small helpers over `io::pb` for the physics stages: nested field access,
//! scalar setters, vectors and `.curve` files.

use anyhow::Result;

use crate::io::pb::{Msg, Val};

/// The sub-message at a field path (each step: the first field of that
/// number, created when absent).
pub fn at<'a>(m: &'a mut Msg, path: &[u32]) -> Result<&'a mut Msg> {
    let mut cur = m;
    for &n in path {
        cur = cur.msg_mut(n)?;
    }
    Ok(cur)
}

/// Set a float field (f32 on the wire, like every EVO physics float).
pub fn f(m: &mut Msg, num: u32, v: f64) {
    m.set_f32(num, v as f32);
}

/// Set a float field, or drop it when `v` is None.
pub fn fo(m: &mut Msg, num: u32, v: Option<f64>) {
    match v {
        Some(x) => f(m, num, x),
        None => m.remove(num),
    }
}

pub fn i(m: &mut Msg, num: u32, v: i64) {
    m.set_varint(num, v as u64);
}

pub fn b(m: &mut Msg, num: u32, v: bool) {
    if v {
        m.set_varint(num, 1);
    } else {
        m.remove(num);
    }
}

/// A Vector3Data {1 x, 2 y, 3 z}.
pub fn v3(p: [f64; 3]) -> Val {
    let mut m = Msg::default();
    for (k, x) in p.iter().enumerate() {
        m.set_f32(k as u32 + 1, *x as f32);
    }
    Val::Msg(m)
}

pub fn set_v3(m: &mut Msg, num: u32, p: [f64; 3]) {
    m.set(num, v3(p));
}

/// Read a float field (f32 or varint).
pub fn rf(m: &Msg, num: u32) -> Option<f64> {
    m.f32(num).map(|x| x as f64).or_else(|| m.varint(num).map(|x| x as f64))
}

/// Read a float at a field path.
pub fn rf_at(m: &Msg, path: &[u32]) -> Option<f64> {
    let (last, head) = path.split_last()?;
    let mut cur = m.clone();
    for &n in head {
        cur = cur.sub(n)?;
    }
    rf(&cur, *last)
}

/// `.curve` files: CurveData {1 point {1 x, 3 {}, 4 {}, 10 y}} (the y sits
/// in field 10 of the point, as in every stock curve; 3 / 4 are the empty
/// tangent messages stock power curves carry).
pub fn curve(points: &[(f64, f64)]) -> Vec<u8> {
    let mut c = Msg::default();
    for &(x, y) in points {
        let mut p = Msg::default();
        p.set_f32(1, x as f32);
        p.push(3, Val::Bytes(Vec::new()));
        p.push(4, Val::Bytes(Vec::new()));
        p.set_f32(10, y as f32);
        c.push(1, Val::Msg(p));
    }
    c.encode()
}

/// A `.curve` file's points (x, y), sorted by x.
pub fn read_curve(d: &[u8]) -> Vec<(f64, f64)> {
    let Ok(m) = Msg::parse(d) else { return vec![] };
    let mut out: Vec<(f64, f64)> = m.subs(1).iter().map(|p| (rf(p, 1).unwrap_or(0.0), rf(p, 10).or_else(|| rf(p, 2)).unwrap_or(0.0))).collect();
    out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// Linear interpolation, clamped at the ends.
pub fn lerp_lut(l: &[(f64, f64)], x: f64) -> f64 {
    crate::ac1::ini::lut_at(l, x).unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    #[test]
    fn curve_round_trip() {
        let pts = vec![(0.0, 50.0), (1000.0, 70.0), (7000.0, 110.5)];
        let b = super::curve(&pts);
        assert_eq!(super::read_curve(&b), pts);
    }
}
