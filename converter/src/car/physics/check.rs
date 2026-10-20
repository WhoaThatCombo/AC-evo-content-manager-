//! Longitudinal model (OURS) shared by the AI hints, the PI and the
//! data-level checks: full-throttle run from standstill, the speed at the
//! limiter in every gear, and the gearing checks.
//!
//! Point mass with: crank torque x (1 + boost) through gear x final x the
//! drivetrain efficiency; traction capped by the driven axle's static load x
//! its longitudinal mu; aero drag 0.5 rho CD.A v^2 (rho 1.22); rolling
//! resistance 0.012 m g; upshift at the auto-shifter rpm (default: the stock
//! median ratio to the limiter, drivetrain.rs),
//! no drive during the shift time. On the stock fleet its terminal speed is
//! within -8.4 % .. +16.0 % (p10 .. p90) of the shipped AI top speed, median
//! -1.9 % (tools/aifit.py over the 50 stock cars whose drag is in wings).

use super::{Derived, G};

pub const RHO: f64 = 1.22;
pub const RR: f64 = 0.012;

pub fn lut(l: &[(f64, f64)], x: f64) -> f64 {
    crate::ac1::ini::lut_at(l, x).unwrap_or(0.0)
}

/// Full-throttle crank torque with boost.
pub fn torque(d: &Derived, rpm: f64) -> f64 {
    lut(&d.torque, rpm) * (1.0 + lut(&d.boost, rpm).max(0.0))
}

/// Peak crank power (W) and its rpm, up to the limiter.
pub fn peak_power(d: &Derived) -> (f64, f64) {
    let mut best = (0.0, 0.0);
    let mut r = d.idle.max(500.0);
    while r <= d.limiter {
        let p = torque(d, r) * r * std::f64::consts::TAU / 60.0;
        if p > best.0 {
            best = (p, r);
        }
        r += 50.0;
    }
    best
}

pub fn peak_torque(d: &Derived) -> (f64, f64) {
    let mut best = (0.0, 0.0);
    let mut r = d.idle.max(500.0);
    while r <= d.limiter {
        let t = torque(d, r);
        if t > best.0 {
            best = (t, r);
        }
        r += 50.0;
    }
    best
}

fn driven_radius(d: &Derived) -> f64 {
    match d.traction {
        1 => d.tyre_radius[0],
        2 => (d.tyre_radius[0] + d.tyre_radius[1]) / 2.0,
        _ => d.tyre_radius[1],
    }
    .max(0.15)
}

/// Road speed (m/s) at `rpm` in a gear of `ratio`.
pub fn speed_at(d: &Derived, ratio: f64, rpm: f64) -> f64 {
    rpm * std::f64::consts::TAU / 60.0 * driven_radius(d) / (ratio * d.final_drive).max(1e-6)
}

fn traction_limit(d: &Derived) -> f64 {
    let w = d.mass_static * G;
    match d.traction {
        1 => w * d.cg_front * d.mu_x[0],
        2 => w * (d.cg_front * d.mu_x[0] + (1.0 - d.cg_front) * d.mu_x[1]),
        _ => w * (1.0 - d.cg_front) * d.mu_x[1],
    }
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Sample {
    pub t: f64,
    pub v: f64,
    pub a_g: f64,
    /// 1-based forward gear
    pub gear: usize,
    pub rpm: f64,
    pub wheel_hp: f64,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Run {
    pub samples: Vec<Sample>,
    pub top_speed: f64,
    pub t_0_100: f64,
    pub a_at_120: f64,
    /// highest rpm reached in each gear
    pub max_rpm: Vec<f64>,
}

/// Net forward force in a gear at speed v (full throttle).
pub fn drive_force(d: &Derived, gear: usize, v: f64) -> (f64, f64) {
    let ratio = d.gears[gear] * d.final_drive;
    let r = driven_radius(d);
    let mut rpm = v / r * ratio * 60.0 / std::f64::consts::TAU;
    // clutch slip at launch: the engine holds a launch rpm
    let launch = (d.limiter * 0.55).max(d.idle);
    if gear == 0 && rpm < launch {
        rpm = launch;
    }
    let t = if rpm >= d.limiter { 0.0 } else { torque(d, rpm.max(d.idle)) };
    let f = (t * ratio / r * d.efficiency).min(traction_limit(d));
    (f, rpm)
}

pub fn resist(d: &Derived, v: f64) -> f64 {
    0.5 * RHO * d.cd_area.max(0.0) * v * v + RR * d.mass_static * G
}

/// Full-throttle run from standstill.
pub fn run(d: &Derived) -> Run {
    let mut out = Run { max_rpm: vec![0.0; d.gears.len()], ..Default::default() };
    if d.gears.is_empty() || d.torque.len() < 2 {
        return out;
    }
    let m = d.mass_static * 1.05; // rotating inertia
    let dt = 0.01;
    let (mut t, mut v, mut gear) = (0.0, 0.0, 0usize);
    let mut shift_left = 0.0;
    let mut next_sample = 0.0;
    let shift_time = 0.25;
    let up = if d.shift_up_rpm > d.idle { d.shift_up_rpm.min(d.limiter) } else { d.limiter * super::drivetrain::AUTO_UP_OF_LIMITER };
    out.samples.push(Sample::default());
    let mut a_prev = 1.0;
    while t < 150.0 {
        let (f, rpm) = drive_force(d, gear, v);
        out.max_rpm[gear] = out.max_rpm[gear].max(rpm.min(d.limiter));
        let fdrive = if shift_left > 0.0 { 0.0 } else { f };
        let a = (fdrive - resist(d, v)) / m;
        if shift_left <= 0.0 && gear + 1 < d.gears.len() && rpm >= up - 1.0 {
            gear += 1;
            shift_left = shift_time;
            out.samples.push(Sample { t, v, a_g: a / G, gear: gear + 1, rpm: drive_force(d, gear, v).1, wheel_hp: 0.0 });
        }
        shift_left -= dt;
        v = (v + a * dt).max(0.0);
        t += dt;
        if out.t_0_100 == 0.0 && v >= 100.0 / 3.6 {
            out.t_0_100 = t;
        }
        if out.a_at_120 == 0.0 && v >= 120.0 / 3.6 {
            out.a_at_120 = a / G;
        }
        if v >= next_sample + 10.0 / 3.6 {
            next_sample = v;
            out.samples.push(Sample { t, v, a_g: a / G, gear: gear + 1, rpm, wheel_hp: fdrive * v / 745.7 });
        }
        // stop when the gear in use can no longer accelerate the car
        let avail = (f - resist(d, v)) / m;
        if avail < 0.003 * G && a_prev < 0.003 * G && shift_left <= 0.0 && (gear + 1 == d.gears.len() || rpm < up - 50.0) {
            break;
        }
        a_prev = avail;
    }
    let (f, rpm) = drive_force(d, gear, v);
    out.samples.push(Sample { t, v, a_g: 0.0, gear: gear + 1, rpm, wheel_hp: f * v / 745.7 });
    out.top_speed = v;
    out
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct GearCheck {
    pub gear: usize,
    pub ratio: f64,
    pub kmh_at_limiter: f64,
    pub reaches_limiter: bool,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Gearing {
    pub gears: Vec<GearCheck>,
    pub top_kmh: f64,
    pub problems: Vec<String>,
}

/// Every gear but the last reaches the limiter at full throttle (unless the
/// car's terminal speed is below that gear's limiter speed); the top gear's
/// limiter speed is plausible (80 .. 600 km/h); the shift point is below
/// the limiter.
pub fn gearing(d: &Derived) -> Gearing {
    let mut g = Gearing::default();
    if d.gears.is_empty() {
        g.problems.push("no forward gears".into());
        return g;
    }
    let r = run(d);
    g.top_kmh = r.top_speed * 3.6;
    let n = d.gears.len();
    for (k, ratio) in d.gears.iter().enumerate() {
        let kmh = speed_at(d, *ratio, d.limiter) * 3.6;
        let reaches = r.max_rpm[k] >= d.limiter.min(d.shift_up_rpm.max(d.idle)) - 60.0;
        if k + 1 < n && !reaches && kmh < g.top_kmh * 0.97 {
            g.problems.push(format!("gear {} never reaches the limiter / shift point at full throttle (limiter at {kmh:.0} km/h)", k + 1));
        }
        g.gears.push(GearCheck { gear: k + 1, ratio: *ratio, kmh_at_limiter: kmh, reaches_limiter: reaches });
    }
    let top = g.gears.last().map(|x| x.kmh_at_limiter).unwrap_or(0.0);
    if !(80.0..=600.0).contains(&top) {
        g.problems.push(format!("top gear at the limiter is {top:.0} km/h"));
    }
    if d.shift_up_rpm > d.limiter + 1.0 {
        g.problems.push(format!("auto-shift rpm {:.0} above the limiter {:.0}", d.shift_up_rpm, d.limiter));
    }
    for w in d.gears.windows(2) {
        if w[1] >= w[0] {
            g.problems.push(format!("gear ratios not descending ({:.3} then {:.3})", w[0], w[1]));
        }
    }
    g
}

/// Suspension and steering sanity (the data-level checks).
pub fn chassis(d: &Derived) -> Vec<String> {
    let mut p = Vec::new();
    for (a, ax) in ["front", "rear"].iter().enumerate() {
        if d.bump_up[a] <= d.rest_x[a].max(0.0) || d.bump_dn[a] <= (-d.rest_x[a]).max(0.0) {
            p.push(format!("{ax}: the rest position is outside the bump stops"));
        }
        if !(0.005..=0.30).contains(&d.bump_up[a]) || !(0.005..=0.30).contains(&d.bump_dn[a]) {
            p.push(format!("{ax}: bump / droop travel {:.3} / {:.3} m", d.bump_up[a], d.bump_dn[a]));
        }
        if !(0.3..=2.5).contains(&(d.sag[a] / 0.1).abs()) && d.sag[a] > 0.25 {
            p.push(format!("{ax}: static sag {:.3} m", d.sag[a]));
        }
    }
    let wheel = d.steer_lock / d.steer_ratio.abs().max(1.0);
    if !(10.0..=70.0).contains(&wheel) {
        p.push(format!("road-wheel lock {wheel:.1} deg (steer lock {:.0} / ratio {:.1})", d.steer_lock, d.steer_ratio));
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mx5ish() -> Derived {
        Derived {
            mass: 1080.0,
            mass_static: 1098.0,
            cg_front: 0.515,
            torque: vec![(0.0, 50.0), (1000.0, 70.0), (3000.0, 122.0), (5500.0, 135.0), (7000.0, 110.0), (8000.0, 20.0)],
            boost: vec![],
            idle: 900.0,
            limiter: 7250.0,
            gears: vec![3.136, 1.888, 1.33, 1.0, 0.814],
            final_drive: 4.1,
            shift_up_rpm: 7150.0,
            traction: 0,
            efficiency: 0.85,
            tyre_radius: [0.2888, 0.2888],
            mu_x: [1.05, 1.05],
            mu_y: [1.02, 1.02],
            cd_area: 0.75,
            steer_lock: 504.0,
            steer_ratio: 15.0,
            ..Default::default()
        }
    }

    #[test]
    fn mx5_gearing() {
        let d = mx5ish();
        let g = gearing(&d);
        assert!(g.problems.is_empty(), "{:?}", g.problems);
        // stock mx5_na AI top speed 193 km/h
        assert!((170.0..215.0).contains(&g.top_kmh), "{}", g.top_kmh);
        // 1st at the limiter: 7250 rpm, r 0.2888, 3.136 x 4.1 -> 57 km/h
        assert!((g.gears[0].kmh_at_limiter - 61.0).abs() < 5.0, "{}", g.gears[0].kmh_at_limiter);
    }
}
