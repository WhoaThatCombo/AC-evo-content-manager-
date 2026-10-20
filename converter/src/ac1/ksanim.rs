//! AC1 `.ksanim` animation reader (public AC1 format, read off the AC1
//! install's own files):
//!
//! `i32 version, i32 node count`, then per node `i32 name length, name,
//! i32 frame count` and the frames:
//! - version 1: a row-major 4x4 matrix per frame (16 f32, row vectors,
//!   translation in 12..14, like the kn5 node matrices);
//! - version 2: rotation quaternion (x, y, z, w), translation (x, y, z),
//!   scale (x, y, z) per frame (10 f32).
//!
//! Frames are the node's LOCAL transform (relative to its kn5 parent).

use anyhow::{bail, Result};

use crate::car::frame::M4;

#[derive(Clone, Debug)]
pub struct Track {
    pub node: String,
    pub frames: Vec<M4>,
}

/// Quaternion + translation + scale -> a row-vector matrix (v' = v * M).
pub fn trs(q: [f64; 4], t: [f64; 3], s: [f64; 3]) -> M4 {
    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    let n = (x * x + y * y + z * z + w * w).sqrt();
    let (x, y, z, w) = if n > 1e-12 { (x / n, y / n, z / n, w / n) } else { (0.0, 0.0, 0.0, 1.0) };
    // rows = the images of the basis vectors (the transpose of the
    // column-vector rotation matrix)
    let r = [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y + z * w), 2.0 * (x * z - y * w)],
        [2.0 * (x * y - z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z + x * w)],
        [2.0 * (x * z + y * w), 2.0 * (y * z - x * w), 1.0 - 2.0 * (x * x + y * y)],
    ];
    [
        r[0][0] * s[0], r[0][1] * s[0], r[0][2] * s[0], 0.0,
        r[1][0] * s[1], r[1][1] * s[1], r[1][2] * s[1], 0.0,
        r[2][0] * s[2], r[2][1] * s[2], r[2][2] * s[2], 0.0,
        t[0], t[1], t[2], 1.0,
    ]
}

pub fn read(b: &[u8]) -> Result<Vec<Track>> {
    let mut o = 0usize;
    let i32_ = |o: &mut usize| -> Result<i32> {
        let Some(s) = b.get(*o..*o + 4) else { bail!("ksanim truncated at {o}") };
        *o += 4;
        Ok(i32::from_le_bytes(s.try_into().unwrap()))
    };
    let version = i32_(&mut o)?;
    if !(1..=2).contains(&version) {
        bail!("ksanim version {version}");
    }
    let count = i32_(&mut o)?;
    if !(0..=100_000).contains(&count) {
        bail!("ksanim node count {count}");
    }
    let mut out = Vec::with_capacity(count as usize);
    let f = |o: &mut usize| -> Result<f64> {
        let Some(s) = b.get(*o..*o + 4) else { bail!("ksanim truncated at {o}") };
        *o += 4;
        Ok(f32::from_le_bytes(s.try_into().unwrap()) as f64)
    };
    for _ in 0..count {
        let n = i32_(&mut o)?;
        if !(0..=4096).contains(&n) {
            bail!("ksanim name length {n}");
        }
        let Some(name) = b.get(o..o + n as usize) else { bail!("ksanim truncated in a name") };
        let node = String::from_utf8_lossy(name).into_owned();
        o += n as usize;
        let frames = i32_(&mut o)?;
        if !(0..=1_000_000).contains(&frames) {
            bail!("ksanim frame count {frames}");
        }
        let mut fr = Vec::with_capacity(frames as usize);
        for _ in 0..frames {
            if version == 1 {
                let mut m = [0f64; 16];
                for v in &mut m {
                    *v = f(&mut o)?;
                }
                fr.push(m);
            } else {
                let q = [f(&mut o)?, f(&mut o)?, f(&mut o)?, f(&mut o)?];
                let t = [f(&mut o)?, f(&mut o)?, f(&mut o)?];
                let s = [f(&mut o)?, f(&mut o)?, f(&mut o)?];
                fr.push(trs(q, t, s));
            }
        }
        out.push(Track { node, frames: fr });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quarter_turn_about_y() {
        // 90 degrees about +Y: +Z goes to +X (right-handed, row vectors)
        let h = std::f64::consts::FRAC_1_SQRT_2;
        let m = trs([0.0, h, 0.0, h], [1.0, 2.0, 3.0], [1.0; 3]);
        let p = crate::car::frame::xform(&m, [0.0, 0.0, 1.0]);
        assert!((p[0] - 2.0).abs() < 1e-9 && (p[1] - 2.0).abs() < 1e-9 && (p[2] - 3.0).abs() < 1e-9, "{p:?}");
    }

    #[test]
    fn parses_v2() {
        let mut b = Vec::new();
        for v in [2i32, 1, 4] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(b"NODE");
        b.extend_from_slice(&1i32.to_le_bytes());
        for v in [0f32, 0., 0., 1., 0.5, 0., 0., 1., 1., 1.] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        let t = read(&b).unwrap();
        assert_eq!(t[0].node, "NODE");
        assert_eq!(t[0].frames[0][12], 0.5);
    }
}
