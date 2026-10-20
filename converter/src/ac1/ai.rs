//! AC1 AI lines (`ai\fast_lane.ai`, `ai\pit_lane.ai`).
//!
//! Layout, decoded from the Kunos tracks of the user's install (version 7,
//! the only one seen there):
//! - header: 4 x i32 (version, point count, lap time, sample count);
//! - points: per point 3 x f32 position, f32 distance along the line (m),
//!   i32 id (20 bytes);
//! - extra data: i32 count (= point count), then 18 x f32 per point: speed
//!   (m/s), gas, brake, an unused lateral-g slot, radius, distance to the
//!   left / right track edge, camber, direction, normal (3), length, forward
//!   (3), tag, grade;
//! - anything after that (grid caches written by the game) is ignored.

use anyhow::{bail, Result};

#[derive(Clone, Copy, Debug, Default)]
pub struct AiPoint {
    pub pos: [f32; 3],
    /// distance from the first point along the line, metres
    pub dist: f32,
    pub id: i32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AiExtra {
    pub speed: f32,
    pub gas: f32,
    pub brake: f32,
    pub radius: f32,
    /// distance from the line to the left / right track edge, metres
    pub side_left: f32,
    pub side_right: f32,
    pub camber: f32,
    pub direction: f32,
    pub normal: [f32; 3],
    pub length: f32,
    pub forward: [f32; 3],
    pub tag: f32,
    pub grade: f32,
}

#[derive(Clone, Debug, Default)]
pub struct AiLine {
    pub version: i32,
    pub points: Vec<AiPoint>,
    /// per point, when the file carries the extra block (else empty)
    pub extra: Vec<AiExtra>,
}

impl AiLine {
    /// Length of the line (the last point's distance; closes the loop when
    /// the ends are within 50 m of each other).
    pub fn length(&self) -> f32 {
        let (Some(a), Some(b)) = (self.points.first(), self.points.last()) else { return 0.0 };
        let gap = dist(a.pos, b.pos);
        b.dist + if gap < 50.0 { gap } else { 0.0 }
    }
}

pub fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

pub fn parse(b: &[u8]) -> Result<AiLine> {
    let i32_at = |o: usize| -> Option<i32> { b.get(o..o + 4).map(|s| i32::from_le_bytes(s.try_into().unwrap())) };
    let f32_at = |o: usize| -> f32 { f32::from_le_bytes(b[o..o + 4].try_into().unwrap()) };
    let (Some(version), Some(n)) = (i32_at(0), i32_at(4)) else { bail!("ai line too short") };
    if !(0..=10_000_000).contains(&n) {
        bail!("ai line: bad point count {n}");
    }
    let n = n as usize;
    let mut o = 16;
    if b.len() < o + n * 20 {
        bail!("ai line truncated ({} points declared)", n);
    }
    let mut points = Vec::with_capacity(n);
    for _ in 0..n {
        points.push(AiPoint { pos: [f32_at(o), f32_at(o + 4), f32_at(o + 8)], dist: f32_at(o + 12), id: i32_at(o + 16).unwrap() });
        o += 20;
    }
    let mut extra = Vec::new();
    if i32_at(o) == Some(n as i32) && b.len() >= o + 4 + n * 72 {
        o += 4;
        for _ in 0..n {
            let f: Vec<f32> = (0..18).map(|k| f32_at(o + k * 4)).collect();
            extra.push(AiExtra {
                speed: f[0], gas: f[1], brake: f[2], radius: f[4], side_left: f[5], side_right: f[6], camber: f[7], direction: f[8],
                normal: [f[9], f[10], f[11]], length: f[12], forward: [f[13], f[14], f[15]], tag: f[16], grade: f[17],
            });
            o += 72;
        }
    }
    if points.iter().any(|p| p.pos.iter().any(|v| !v.is_finite())) {
        bail!("ai line has non-finite points");
    }
    Ok(AiLine { version, points, extra })
}

#[cfg(test)]
mod tests {
    #[test]
    fn synthetic() {
        let mut b = Vec::new();
        for v in [7i32, 2, 0, 0] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        for (i, x) in [0.0f32, 5.0].iter().enumerate() {
            for v in [*x, 1.0, 2.0, *x] {
                b.extend_from_slice(&v.to_le_bytes());
            }
            b.extend_from_slice(&(i as i32).to_le_bytes());
        }
        b.extend_from_slice(&2i32.to_le_bytes());
        for i in 0..2 {
            for k in 0..18 {
                b.extend_from_slice(&((i * 100 + k) as f32).to_le_bytes());
            }
        }
        let l = super::parse(&b).unwrap();
        assert_eq!(l.points.len(), 2);
        assert_eq!(l.points[1].pos, [5.0, 1.0, 2.0]);
        assert_eq!(l.extra[1].side_left, 105.0);
        assert_eq!(l.extra[0].forward, [13.0, 14.0, 15.0]);
    }
}
