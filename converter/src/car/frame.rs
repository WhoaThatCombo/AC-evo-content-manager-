//! The car's frame of reference, shared by every car stage.
//!
//! # EVO car space (measured on stock cars)
//!
//! Right-handed, Y up, metres, like AC1. +X is the car's LEFT (WHEEL_LF sits
//! at +x), +Z is forward. The origin is on the ground (a wheel centre sits at
//! y = its tyre radius, a little less on stock cars: static deflection) and
//! halfway between the axles: on ks_mazda_mx5_na, ks_toyota_ae86_sprinter_trueno,
//! ks_porsche_992_gt3_rs and ks_porsche_935 the front and rear WHEEL_ bone
//! z values are +-wheelbase/2 within 3.5 cm.
//!
//! Kunos's own AC1 models are already nearly there: the AC1 ks_mazda_miata
//! kn5 puts WHEEL_LF at (0.705, 0.285, 1.168), exactly where the stock EVO
//! mx5_na's WHEEL_LF bone is.
//!
//! # The datum (ours)
//!
//! EVO = kn5 world + `datum`, one translation for the whole car:
//! - x: minus the mean x of the four kn5 `WHEEL_xx` nodes (cars are symmetric);
//! - z: minus the mean of the front and rear axle z (axle midpoint at 0);
//! - y: the mean tyre radius (tyres.ini, default compound) minus the mean
//!   wheel-node y, so the tyres touch y = 0.
//!
//! Without wheel nodes the AC1 physics positions are used instead (below).
//! `GRAPHICS_PITCH_ROTATION` (a fraction of a degree on Kunos cars) is not
//! applied.
//!
//! # AC1 physics in this space
//!
//! AC1 places the model relative to the physics body with car.ini
//! `GRAPHICS_OFFSET`: CoG frame = kn5 + GRAPHICS_OFFSET (checked on
//! ks_mazda_miata: kn5 front axle 1.168 - 0.075 = 1.093 = WHEELBASE x (1 -
//! CG_LOCATION)). In the CoG frame the wheel centres are (+-TRACK/2, BASEY,
//! +WHEELBASE x (1 - CG_LOCATION)) front and (.., -WHEELBASE x CG_LOCATION)
//! rear. `cog` and `physics_wheels` are those points in EVO space.

use std::collections::BTreeMap;

use crate::ac1::car::Car;
use crate::ac1::kn5::{Kn5, Node, NodeKind};

/// Row-major 4x4 (AC1 convention: row vectors, translation in 12..14).
pub type M4 = [f64; 16];

pub const CORNERS: [&str; 4] = ["LF", "RF", "LR", "RR"];

pub const I4: M4 = [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.];

/// tyres.ini sizes of one axle (default compound), metres.
#[derive(Clone, Copy, Debug)]
pub struct Tyre {
    pub radius: f64,
    pub width: f64,
    pub rim_radius: f64,
}

impl Default for Tyre {
    fn default() -> Self {
        Tyre { radius: 0.31, width: 0.205, rim_radius: 0.2 }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Frame {
    /// added to every AC1 (kn5 world) position to land in EVO car space
    pub datum: [f64; 3],
    /// wheel centres in EVO car space, by corner (LF, RF, LR, RR): the kn5
    /// `WHEEL_xx` nodes moved by the datum (AC1 physics positions for a
    /// missing node)
    pub wheel_centres: BTreeMap<String, [f64; 3]>,
    /// the wheel-frame transform of each corner (AC1 `WHEEL_<c>` world, datum applied)
    pub wheel_frames: BTreeMap<String, M4>,
    /// every kn5 node's world matrix, by node name (AC1 space, NO datum:
    /// use `evo_m`); the first node of a name wins
    pub node_world: BTreeMap<String, M4>,
    /// the AC1 physics CoG in EVO space
    pub cog: [f64; 3],
    /// the AC1 physics wheel centres (suspensions.ini) in EVO space
    pub physics_wheels: BTreeMap<String, [f64; 3]>,
    /// front, rear
    pub tyres: [Tyre; 2],
    /// car.ini GRAPHICS_OFFSET
    pub graphics_offset: [f64; 3],
    pub notes: Vec<String>,
}

pub fn mul(a: &M4, b: &M4) -> M4 {
    let mut o = [0f64; 16];
    for r in 0..4 {
        for c in 0..4 {
            o[r * 4 + c] = (0..4).map(|k| a[r * 4 + k] * b[k * 4 + c]).sum();
        }
    }
    o
}

pub fn m64(m: &[f32; 16]) -> M4 {
    m.map(|x| x as f64)
}

/// Point through a row-vector matrix.
pub fn xform(m: &M4, p: [f64; 3]) -> [f64; 3] {
    [
        p[0] * m[0] + p[1] * m[4] + p[2] * m[8] + m[12],
        p[0] * m[1] + p[1] * m[5] + p[2] * m[9] + m[13],
        p[0] * m[2] + p[1] * m[6] + p[2] * m[10] + m[14],
    ]
}

/// Direction through the 3x3 part.
pub fn xdir(m: &M4, d: [f64; 3]) -> [f64; 3] {
    [d[0] * m[0] + d[1] * m[4] + d[2] * m[8], d[0] * m[1] + d[1] * m[5] + d[2] * m[9], d[0] * m[2] + d[1] * m[6] + d[2] * m[10]]
}

/// Affine inverse (row-vector, translation in 12..14).
pub fn inv(m: &M4) -> Option<M4> {
    let a = [[m[0], m[1], m[2]], [m[4], m[5], m[6]], [m[8], m[9], m[10]]];
    let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1]) - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0]) + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
    if !det.is_finite() || det.abs() < 1e-12 {
        return None;
    }
    let c = |r: usize, k: usize| -> f64 {
        let (r1, r2) = ((r + 1) % 3, (r + 2) % 3);
        let (k1, k2) = ((k + 1) % 3, (k + 2) % 3);
        a[r1][k1] * a[r2][k2] - a[r1][k2] * a[r2][k1]
    };
    let mut o = [0f64; 16];
    for i in 0..3 {
        for j in 0..3 {
            o[i * 4 + j] = c(j, i) / det;
        }
    }
    let t = [m[12], m[13], m[14]];
    for j in 0..3 {
        o[12 + j] = -(t[0] * o[j] + t[1] * o[4 + j] + t[2] * o[8 + j]);
    }
    o[15] = 1.0;
    Some(o)
}

pub fn translation(t: [f64; 3]) -> M4 {
    let mut m = I4;
    m[12] = t[0];
    m[13] = t[1];
    m[14] = t[2];
    m
}

/// The corner a node name belongs to (`WHEEL_LF`, `SUSP_RR`, `rim_lf`...):
/// a trailing LF / RF / LR / RR token.
pub fn corner_of(name: &str) -> Option<&'static str> {
    let up = name.to_ascii_uppercase();
    let tok = up.rsplit(|c: char| c == '_' || c == ' ' || c == '-').next()?;
    CORNERS.iter().copied().find(|c| *c == tok)
}

/// tyres.ini of the default compound: (front, rear).
pub fn tyre_sizes(car: &Car) -> [Tyre; 2] {
    let t = car.ini("tyres.ini");
    let idx = t.num("COMPOUND_DEFAULT", "INDEX").unwrap_or(0.0) as i64;
    let sec = |axle: &str| if idx <= 0 { axle.to_string() } else { format!("{axle}_{idx}") };
    let one = |axle: &str| -> Tyre {
        let s = if t.has(&sec(axle)) { sec(axle) } else { axle.to_string() };
        let d = Tyre::default();
        let r = t.num(&s, "RADIUS").filter(|v| *v > 0.1 && *v < 1.0).unwrap_or(d.radius);
        Tyre {
            radius: r,
            width: t.num(&s, "WIDTH").filter(|v| *v > 0.05 && *v < 1.0).unwrap_or(d.width),
            rim_radius: t.num(&s, "RIM_RADIUS").filter(|v| *v > 0.05 && *v < r).unwrap_or((r - 0.1).max(0.1)),
        }
    };
    [one("FRONT"), one("REAR")]
}

impl Frame {
    /// The frame of one AC1 car from its LOD0 kn5 and data.
    pub fn build(car: &Car, k: &Kn5) -> Frame {
        let mut f = Frame { tyres: tyre_sizes(car), ..Default::default() };
        // node worlds (first of a name wins)
        fn walk(n: &Node, parent: &M4, out: &mut BTreeMap<String, M4>) {
            let w = match &n.kind {
                NodeKind::Transform(m) => mul(&m64(m), parent),
                _ => *parent,
            };
            out.entry(n.name.clone()).or_insert(w);
            for c in &n.children {
                walk(c, &w, out);
            }
        }
        walk(&k.root, &I4, &mut f.node_world);
        // AC1 physics (CoG frame)
        let ci = car.ini("car.ini");
        let g = ci.nums("BASIC", "GRAPHICS_OFFSET");
        f.graphics_offset = [g.first().copied().unwrap_or(0.0), g.get(1).copied().unwrap_or(0.0), g.get(2).copied().unwrap_or(0.0)];
        let s = car.ini("suspensions.ini");
        let wb = s.num("BASIC", "WHEELBASE").filter(|v| *v > 0.5 && *v < 6.0);
        let cg = s.num("BASIC", "CG_LOCATION").filter(|v| *v > 0.0 && *v < 1.0).unwrap_or(0.5);
        let mut phys_cog: BTreeMap<String, [f64; 3]> = BTreeMap::new();
        if let Some(wb) = wb {
            for c in CORNERS {
                let front = c.ends_with('F');
                let ax = if front { "FRONT" } else { "REAR" };
                let tr = s.num(ax, "TRACK").filter(|v| *v > 0.5 && *v < 3.0).unwrap_or(1.5);
                let by = s.num(ax, "BASEY").unwrap_or(-0.1);
                let x = if c.starts_with('L') { tr / 2.0 } else { -tr / 2.0 };
                let z = if front { wb * (1.0 - cg) } else { -wb * cg };
                phys_cog.insert(c.to_string(), [x, by, z]);
            }
        }
        // kn5 wheel nodes
        let wheel_node = |c: &str| -> Option<M4> {
            f.node_world.iter().find(|(n, _)| n.eq_ignore_ascii_case(&format!("WHEEL_{c}"))).map(|(_, m)| *m).filter(|m| m.iter().all(|x| x.is_finite()))
        };
        let kn5_wheels: BTreeMap<String, M4> = CORNERS.iter().filter_map(|c| wheel_node(c).map(|m| (c.to_string(), m))).collect();
        // physics wheel centres in kn5 space (CoG frame minus the offset)
        let phys_kn5: BTreeMap<String, [f64; 3]> = phys_cog.iter().map(|(c, p)| (c.clone(), [p[0] - f.graphics_offset[0], p[1] - f.graphics_offset[1], p[2] - f.graphics_offset[2]])).collect();
        let src: BTreeMap<String, [f64; 3]> = CORNERS.iter().filter_map(|c| {
            kn5_wheels.get(*c).map(|m| [m[12], m[13], m[14]]).or_else(|| phys_kn5.get(*c).copied()).map(|p| (c.to_string(), p))
        }).collect();
        if kn5_wheels.len() < 4 {
            f.notes.push(format!("frame: {} of 4 WHEEL_ nodes in the kn5; the rest from suspensions.ini", kn5_wheels.len()));
        }
        if src.len() == 4 {
            let mean = |i: usize| src.values().map(|p| p[i]).sum::<f64>() / 4.0;
            let zf = (src["LF"][2] + src["RF"][2]) / 2.0;
            let zr = (src["LR"][2] + src["RR"][2]) / 2.0;
            let rmean = (f.tyres[0].radius + f.tyres[1].radius) / 2.0;
            f.datum = [-mean(0), rmean - mean(1), -(zf + zr) / 2.0];
        } else {
            f.notes.push("frame: no wheel positions at all (kn5 or suspensions.ini); datum 0".into());
        }
        let d = f.datum;
        for (c, p) in &src {
            f.wheel_centres.insert(c.clone(), [p[0] + d[0], p[1] + d[1], p[2] + d[2]]);
            let m = kn5_wheels.get(c).copied().unwrap_or_else(|| translation(*p));
            f.wheel_frames.insert(c.clone(), f.evo_m(&m));
        }
        for (c, p) in &phys_kn5 {
            f.physics_wheels.insert(c.clone(), [p[0] + d[0], p[1] + d[1], p[2] + d[2]]);
        }
        let go = f.graphics_offset;
        f.cog = [-go[0] + d[0], -go[1] + d[1], -go[2] + d[2]];
        // how far the model's wheels are from the physics ones (a mod whose
        // kn5 and suspensions.ini disagree shows here)
        for c in CORNERS {
            if let (Some(a), Some(b)) = (f.wheel_centres.get(c), f.physics_wheels.get(c)) {
                let dz = (a[2] - b[2]).abs();
                let dx = (a[0].abs() - b[0].abs()).abs();
                if dz > 0.05 || dx > 0.05 {
                    f.notes.push(format!("frame: WHEEL_{c} is {dx:.3} m (x) / {dz:.3} m (z) from the suspensions.ini position"));
                }
            }
        }
        f
    }

    pub fn wheelbase(&self) -> Option<f64> {
        let f = self.wheel_centres.get("LF")?;
        let r = self.wheel_centres.get("LR")?;
        Some((f[2] - r[2]).abs())
    }

    pub fn track_width(&self, front: bool) -> Option<f64> {
        let (l, r) = if front { ("LF", "RF") } else { ("LR", "RR") };
        Some((self.wheel_centres.get(l)?[0] - self.wheel_centres.get(r)?[0]).abs())
    }

    /// AC1 point -> EVO car space.
    pub fn evo(&self, p: [f64; 3]) -> [f64; 3] {
        [p[0] + self.datum[0], p[1] + self.datum[1], p[2] + self.datum[2]]
    }

    /// AC1 world matrix -> EVO car space (translation moved by the datum).
    pub fn evo_m(&self, m: &M4) -> M4 {
        let mut o = *m;
        o[12] += self.datum[0];
        o[13] += self.datum[1];
        o[14] += self.datum[2];
        o
    }

    /// A kn5 node's world matrix in EVO car space.
    pub fn node_evo(&self, name: &str) -> Option<M4> {
        self.node_world.get(name).map(|m| self.evo_m(m))
    }

    /// The tyre sizes of a corner's axle.
    pub fn tyre(&self, corner: &str) -> Tyre {
        if corner.ends_with('F') { self.tyres[0] } else { self.tyres[1] }
    }

    /// The rotation that takes car axes into the wheel frame of a corner:
    /// the inverse of the WHEEL node's own rotation when that is a small one
    /// (camber / toe, under 20 degrees); a mirrored or half-turned node keeps
    /// car axes (its geometry is already modelled in car axes).
    pub fn wheel_unrotate(&self, corner: &str) -> M4 {
        let Some(m) = self.wheel_frames.get(corner) else { return I4 };
        let mut r = *m;
        r[12] = 0.0;
        r[13] = 0.0;
        r[14] = 0.0;
        // diagonal near +1 = small rotation
        let small = r[0] > 0.94 && r[5] > 0.94 && r[10] > 0.94;
        if !small {
            return I4;
        }
        inv(&r).unwrap_or(I4)
    }

    /// A summary for tools and logs (`aceconv car-frame`).
    pub fn json(&self) -> serde_json::Value {
        let r = |v: f64| (v * 10000.0).round() / 10000.0;
        let v3 = |p: &[f64; 3]| serde_json::json!([r(p[0]), r(p[1]), r(p[2])]);
        let map = |m: &BTreeMap<String, [f64; 3]>| serde_json::Value::Object(m.iter().map(|(k, p)| (k.clone(), v3(p))).collect());
        serde_json::json!({
            "datum": v3(&self.datum),
            "wheel_centres": map(&self.wheel_centres),
            "physics_wheels": map(&self.physics_wheels),
            "cog": v3(&self.cog),
            "graphics_offset": v3(&self.graphics_offset),
            "wheelbase": self.wheelbase().map(r),
            "track_front": self.track_width(true).map(r),
            "track_rear": self.track_width(false).map(r),
            "tyres": self.tyres.iter().map(|t| serde_json::json!({"radius": r(t.radius), "width": r(t.width), "rim_radius": r(t.rim_radius)})).collect::<Vec<_>>(),
            "nodes": self.node_world.len(),
            "notes": self.notes,
        })
    }

    /// An EVO car-space point -> the wheel frame of a corner (relative to the
    /// wheel centre, car axes, camber / toe of the WHEEL node removed).
    pub fn to_wheel(&self, corner: &str, p: [f64; 3]) -> [f64; 3] {
        let c = self.wheel_centres.get(corner).copied().unwrap_or([0.0; 3]);
        let rel = [p[0] - c[0], p[1] - c[1], p[2] - c[2]];
        xdir(&self.wheel_unrotate(corner), rel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_and_inverse() {
        assert_eq!(corner_of("WHEEL_LF"), Some("LF"));
        assert_eq!(corner_of("rim blur rr"), Some("RR"));
        assert_eq!(corner_of("SUSP_LR"), Some("LR"));
        assert_eq!(corner_of("WHEELS"), None);
        let m: M4 = [0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 2.0, 3.0, 1.0];
        let i = inv(&m).unwrap();
        let p = xform(&i, xform(&m, [0.3, -0.2, 5.0]));
        assert!((p[0] - 0.3).abs() < 1e-9 && (p[1] + 0.2).abs() < 1e-9 && (p[2] - 5.0).abs() < 1e-9);
    }
}
