//! Whole-car steady-state limits through one quasi-static model, so a
//! source car and its conversion are compared with only the tyres
//! changing: the largest steady cornering acceleration and the largest
//! straight-line braking at a few speeds.
//!
//! The model (identical for both sides): rigid body on four tyres; aero
//! downforce per axle (source CL.A split by wing position); lateral load
//! transfer m ay h / track shared between the axles by roll stiffness
//! (springs + anti-roll bars, t^2 (k / 2 + k_arb), all transfer elastic,
//! no roll centres, no camber); both tyres of an axle at one slip angle;
//! braking at the car's fixed front share (tyre-limited),
//! longitudinal transfer m a h / L; drag, rolling resistance, thermal,
//! pressure and transient slip left out. It ranks tyre sets; it is not
//! a lap-time model, and its absolute g is not the game's.

use crate::source::SourceCar;

use super::G;

#[derive(Clone, Debug)]
pub struct Model {
    pub mass: f64,
    pub cg_front: f64,
    pub cg_height: f64,
    pub wheelbase: f64,
    pub track: [f64; 2],
    /// front share of the roll stiffness
    pub roll_front: f64,
    pub cl_area: [f64; 2],
    pub brake_front: f64,
    pub brake_torque: f64,
    pub radius: [f64; 2],
}

impl Model {
    pub fn from_source(s: &SourceCar) -> Model {
        let k = |a: usize| s.axles[a].track.powi(2) * (s.axles[a].spring / 2.0 + s.axles[a].arb);
        let r = s.compounds.get(s.default_compound).map(|c| [c.tyres[0].radius, c.tyres[1].radius]).unwrap_or([0.3, 0.3]);
        Model {
            mass: s.mass_static(),
            cg_front: s.cg_front,
            cg_height: s.cg_height.clamp(0.15, 1.0),
            wheelbase: s.wheelbase,
            track: [s.axles[0].track, s.axles[1].track],
            roll_front: k(0) / (k(0) + k(1)).max(1.0),
            cl_area: s.aero.cl_area,
            brake_front: s.brake_front,
            brake_torque: s.brake_torque,
            radius: r,
        }
    }

    fn aero(&self, v: f64) -> [f64; 2] {
        let q = 0.5 * 1.225 * v * v;
        [self.cl_area[0] * q, self.cl_area[1] * q]
    }
}

/// One axle's tyre: (fz, alpha, sr, speed) -> (Fy, Fx) of one wheel.
pub type Tyre<'a> = &'a dyn Fn(f64, f64, f64, f64) -> (f64, f64);

fn axle_lat_cap(t: Tyre, f_out: f64, f_in: f64, v: f64) -> f64 {
    let mut best = 0.0f64;
    let mut a = 0.3f64;
    while a < 25.0 {
        let r = a.to_radians();
        best = best.max(t(f_out, r, 0.0, v).0 + t(f_in.max(0.0), r, 0.0, v).0);
        a += 0.1;
    }
    best
}

fn wheel_brake_cap(t: Tyre, fz: f64, v: f64) -> f64 {
    let mut best = 0.0f64;
    let mut s = 0.003;
    while s < 0.6 {
        best = best.max(t(fz, 0.0, -s, v).1.abs());
        s *= 1.05;
    }
    best
}

fn bisect(ok: impl Fn(f64) -> bool) -> f64 {
    let (mut lo, mut hi) = (0.0, 5.0);
    for _ in 0..40 {
        let m = 0.5 * (lo + hi);
        if ok(m) { lo = m } else { hi = m }
    }
    lo
}

/// The largest steady cornering acceleration (g) at `v` m/s.
pub fn lateral_g(m: &Model, tyres: [Tyre; 2], v: f64) -> f64 {
    let aero = m.aero(v);
    let w = m.mass * G;
    let stat = [w * m.cg_front / 2.0 + aero[0] / 2.0, w * (1.0 - m.cg_front) / 2.0 + aero[1] / 2.0];
    bisect(|ay| {
        let mom = m.mass * ay * G * m.cg_height;
        (0..2).all(|a| {
            let share = if a == 0 { m.roll_front } else { 1.0 - m.roll_front };
            let d = mom * share / m.track[a];
            let need = m.mass * ay * G * if a == 0 { 1.0 - m.cg_front } else { m.cg_front };
            d < stat[a] * 1.999 && axle_lat_cap(tyres[a], stat[a] + d, stat[a] - d, v) >= need
        })
    })
}

/// The largest straight-line deceleration (g) at `v` m/s with the car's
/// fixed brake share, tyre-limited (the brake torque is not a cap: AC1's
/// MAX_TORQUE meaning per wheel or per car is not documented).
pub fn braking_g(m: &Model, tyres: [Tyre; 2], v: f64) -> f64 {
    let aero = m.aero(v);
    let w = m.mass * G;
    bisect(|a| {
        let dt = m.mass * a * G * m.cg_height / m.wheelbase;
        let fz = [(w * m.cg_front + dt + aero[0]) / 2.0, (w * (1.0 - m.cg_front) - dt + aero[1]) / 2.0];
        let total = m.mass * a * G;
        let need = [total * m.brake_front / 2.0, total * (1.0 - m.brake_front) / 2.0];
        fz[1] > 0.0 && (0..2).all(|k| wheel_brake_cap(tyres[k], fz[k], v) >= need[k])
    })
}

pub const SPEEDS_KMH: [f64; 3] = [60.0, 120.0, 200.0];

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Limits {
    /// per SPEEDS_KMH
    pub lateral_g: Vec<f64>,
    pub braking_g: Vec<f64>,
}

pub fn limits(m: &Model, tyres: [Tyre; 2]) -> Limits {
    let v = SPEEDS_KMH.map(|k| k / 3.6);
    Limits { lateral_g: v.iter().map(|&v| lateral_g(m, tyres, v)).collect(), braking_g: v.iter().map(|&v| braking_g(m, tyres, v)).collect() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn car() -> Model {
        Model { mass: 1000.0, cg_front: 0.5, cg_height: 0.45, wheelbase: 2.4, track: [1.4, 1.4], roll_front: 0.5, cl_area: [0.0; 2], brake_front: 0.5, brake_torque: 1e9, radius: [0.3; 2] }
    }

    #[test]
    fn constant_mu_gives_mu() {
        // a load-insensitive tyre of mu 1.0 (saturating): 1 g either way
        let t = |fz: f64, a: f64, sr: f64, _v: f64| ((a * 10.0).clamp(-1.0, 1.0) * fz, (sr * 10.0).clamp(-1.0, 1.0) * fz);
        let mut m = car();
        assert!((lateral_g(&m, [&t, &t], 20.0) - 1.0).abs() < 0.01);
        // 50 % front share: the unloaded rear locks first, 0.5 / (0.5 + h / L)
        assert!((braking_g(&m, [&t, &t], 20.0) - 0.5 / (0.5 + 0.45 / 2.4)).abs() < 0.01);
        // the ideal share at 1 g: 1 g
        m.brake_front = 0.5 + 0.45 / 2.4;
        assert!((braking_g(&m, [&t, &t], 20.0) - 1.0).abs() < 0.01);
    }

    #[test]
    fn load_sensitivity_costs_cornering() {
        let t = |fz: f64, a: f64, _sr: f64, _v: f64| ((a * 10.0).clamp(-1.0, 1.0) * fz * (fz / 2450.0).powf(-0.2), 0.0);
        assert!(lateral_g(&car(), [&t, &t], 20.0) < 0.99);
    }
}
