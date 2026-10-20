//! AI car data (`.aicardata`), the performance indicator and the
//! mechanical preset's spec sheet.
//!
//! Physics hints (AiPhysicsHints): models fitted on the 71 stock cars by
//! tools/aifit.py from the per-car table examples/aifit.rs writes
//! (tools/aifit_stock.tsv; fit report tools/aifit_summary.txt, per-car
//! residuals tools/aifit_fit.tsv; docs/FORMATS.md, "Car physics"). Inputs are
//! the `Derived` numbers (stats.rs reads the same ones off stock CarData):
//! - `aero_hint` (1) = AERO_A + AERO_B x CL.A rho / (2 m g), m with fuel:
//!   linear in the wings' downforce over weight; the slope is a speed
//!   squared, so the form reads "x at 30.9 m/s" (fitted on the 50 stock cars
//!   without CZ aero maps);
//! - lateral plateau, low speed, end (5.1) = LAT_A + LAT_B x mu_y, mu_y the
//!   default compound's static-load lateral mu averaged over the axles by
//!   weight; the high-speed end (7.1) is it times `aero_hint` (stock: median
//!   7.1 / 5.1 / aero_hint = 0.999); the plateau starts (4.1, 6.1) are the
//!   stock median start / end ratios;
//! - braking (11) = min(BRAKE_MU x mu_x, BRAKE_T x T / (r m g)), T the
//!   brake torque, r the mean tyre radius (median regression);
//! - every hint clamped to the stock min / max widened by 10 % of the stock
//!   span (OURS);
//! - tyre slips (4.2 .. 7.2): the stock median slip at the low-speed end on
//!   either side of the mu_y split that best separates the stock values, and
//!   the stock median ratios of the other three to it;
//! - top speed, the acceleration table (one point per 10 km/h and per shift,
//!   gear counted with R = 0 and N = 1 like stock) and the forward
//!   acceleration (stock values match the run's acceleration at 120 km/h)
//!   from check::run.
//!
//! PI (OURS): the average speed in km/h over a fixed synthetic circuit
//! (straights and corners of 15 .. 200 m radius, 4.0 km), divided by 10,
//! from the same model: corner speed from the lateral plateau, downforce
//! adding grip like weight (lat(v) = plateau x (1 + CL.A q / (m g))),
//! straights from the acceleration run, braking from the braking hint. The
//! scale is ours: the game's stock PIs are not in the content, so the
//! calibration to the game's numbers is a pending in-game reading.

use anyhow::Result;

use super::check::{self, RHO};
use super::pbx::{self, f, i};
use super::{Ctx, Derived, G};
use crate::io::pb::{Msg, Val};

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Hints {
    pub lat_low: f64,
    pub lat_high: f64,
    pub aero_hint: f64,
    pub braking: f64,
    pub forward: f64,
    pub top_kmh: f64,
    pub t_0_100: f64,
    pub slip: f64,
}

// Fitted by tools/aifit.py (tools/aifit_summary.txt), at the fit's precision.
/// aero_hint = AERO_A + AERO_B x CL.A rho / (2 m g)  (AERO_B in m^2/s^2)
const AERO_A: f64 = 0.9661846580562213;
const AERO_B: f64 = 953.254076311785;
const AERO_CLAMP: (f64, f64) = (0.8313454747200012, 1.5906376481056212);
/// low-speed lateral plateau end = LAT_A + LAT_B x mu_y
const LAT_A: f64 = -0.5324522251768861;
const LAT_B: f64 = 1.2523329716945453;
const LAT_CLAMP: (f64, f64) = (0.6489268004894256, 2.1296577394008636);
/// braking = min(BRAKE_MU x mu_x, BRAKE_T x T / (r m g))
const BRAKE_MU: f64 = 0.8945281017826807;
const BRAKE_T: f64 = 1.5323931245658833;
const BRAKE_CLAMP: (f64, f64) = (0.4012736439704895, 1.9354309678077697);
/// stock median plateau start / end (g): low speed, high speed
const LOW_START: f64 = 0.9936439966494767;
const HIGH_START: f64 = 0.9954129522668582;
/// stock median slip at the low-speed end at or below / above the mu_y split
const SLIP_SPLIT_MU: f64 = 1.230845746819114;
const SLIP_LOW_END: (f64, f64) = (1.7680517435073853, 1.1330105066299438);
/// stock median slips over the low-speed end's: low start, high start, high end
const SLIP_RATIO: [f64; 3] = [0.8708935561156503, 0.7637807288034041, 0.9167709543311205];

pub fn hints(d: &Derived) -> (Hints, check::Run) {
    let r = check::run(d);
    let muy = d.mu_y[0] * d.cg_front + d.mu_y[1] * (1.0 - d.cg_front);
    let mux = d.mu_x[0] * d.cg_front + d.mu_x[1] * (1.0 - d.cg_front);
    let lat_low = (LAT_A + LAT_B * muy).clamp(LAT_CLAMP.0, LAT_CLAMP.1);
    let s = d.cl_area * 0.5 * RHO / (d.mass_static * G);
    let aero_hint = (AERO_A + AERO_B * s).clamp(AERO_CLAMP.0, AERO_CLAMP.1);
    let radius = (d.tyre_radius[0] + d.tyre_radius[1]) / 2.0;
    let braking = (BRAKE_MU * mux).min(BRAKE_T * d.brake_torque / (radius * d.mass_static * G)).clamp(BRAKE_CLAMP.0, BRAKE_CLAMP.1);
    let slip = if muy <= SLIP_SPLIT_MU { SLIP_LOW_END.0 } else { SLIP_LOW_END.1 };
    let h = Hints { lat_low, lat_high: lat_low * aero_hint, aero_hint, braking, forward: r.a_at_120.max(0.0), top_kmh: r.top_speed * 3.6, t_0_100: r.t_0_100, slip };
    (h, r)
}

/// The synthetic circuit: (length m, corner radius m; 0 = straight).
const CIRCUIT: [(f64, f64); 12] = [
    (700.0, 0.0), (40.0, 25.0), (300.0, 0.0), (120.0, 60.0), (450.0, 0.0), (190.0, 120.0),
    (250.0, 0.0), (50.0, 15.0), (600.0, 0.0), (210.0, 200.0), (350.0, 0.0), (80.0, 40.0),
];

/// Lap time (s) on the synthetic circuit.
pub fn lap_time(d: &Derived, h: &Hints, run: &check::Run) -> f64 {
    let ds = 2.0;
    let mut vmax: Vec<f64> = Vec::new();
    for (len, rad) in CIRCUIT {
        let n = (len / ds).ceil() as usize;
        let v = if rad > 0.0 {
            // v^2 / R = g lat(v), lat(v) = lat_low (1 + CL.A q / (m g)):
            // downforce adds grip like weight does
            let k = d.cl_area.max(0.0) * 0.5 * RHO / (d.mass_static * G);
            let den = 1.0 / (rad * G * h.lat_low) - k;
            if den > 1e-6 { (1.0 / den).sqrt() } else { 120.0 }
        } else {
            f64::MAX
        };
        vmax.extend(std::iter::repeat(v.min(h.top_kmh / 3.6 + 0.01)).take(n));
    }
    // acceleration available at speed v: from the run's table
    let acc = |v: f64| -> f64 {
        let s = &run.samples;
        let a = s.windows(2).find(|w| v >= w[0].v && v <= w[1].v).map(|w| w[0].a_g.max(w[1].a_g)).unwrap_or(0.0);
        (a.max(0.0) * G).max(0.05)
    };
    let brk = |v: f64| -> f64 { h.braking * G * (1.0 + d.cl_area.max(0.0) * 0.5 * RHO * v * v / (d.mass_static * G)) };
    let n = vmax.len();
    let mut v = vec![0.0; n];
    // two laps forward so the start speed is the flying-lap one
    let mut cur = vmax[n - 1].min(30.0);
    for _ in 0..2 {
        for k in 0..n {
            cur = (cur * cur + 2.0 * acc(cur) * ds).sqrt().min(vmax[k]);
            v[k] = cur;
        }
    }
    for k in (0..n).rev() {
        let next = if k + 1 < n { v[k + 1] } else { v[0] };
        v[k] = v[k].min((next * next + 2.0 * brk(next) * ds).sqrt());
    }
    v.iter().map(|x| ds / x.max(1.0)).sum()
}

pub fn perf_indicator(d: &Derived) -> (f64, Hints, check::Run) {
    let (h, r) = hints(d);
    let len: f64 = CIRCUIT.iter().map(|s| s.0).sum();
    let t = lap_time(d, &h, &r);
    (((len / t) * 3.6 / 10.0 * 10.0).round() / 10.0, h, r)
}

fn acc(g: f64, slip: f64) -> Val {
    let mut m = Msg::default();
    f(&mut m, 1, g);
    f(&mut m, 2, slip);
    Val::Msg(m)
}

pub fn write(c: &mut Ctx) -> Result<f64> {
    let d = c.d.clone();
    let (pi, h, run) = perf_indicator(&d);
    let Some(path) = c.cd.str(29) else {
        c.note("physics: CarData names no .aicardata; AI hints not written");
        return Ok(pi);
    };
    let old = c.files.find_ci(&path).and_then(|k| c.files.get(&k).cloned()).and_then(|b| Msg::parse(&b).ok()).unwrap_or_default();
    let mut a = old.clone();
    {
        let g = a.msg_mut(1)?;
        i(g, 1, d.shift_up_rpm.round() as i64);
        i(g, 2, d.shift_down_rpm.round() as i64);
        i(g, 5, d.limiter.round() as i64);
    }
    let mut ph = Msg::default();
    f(&mut ph, 1, h.aero_hint);
    ph.push(4, acc(h.lat_low * LOW_START, h.slip * SLIP_RATIO[0]));
    ph.push(5, acc(h.lat_low, h.slip));
    ph.push(6, acc(h.lat_high * HIGH_START, h.slip * SLIP_RATIO[1]));
    ph.push(7, acc(h.lat_high, h.slip * SLIP_RATIO[2]));
    f(&mut ph, 8, h.top_kmh);
    let last = run.samples.last().cloned().unwrap_or_default();
    f(&mut ph, 9, last.wheel_hp);
    f(&mut ph, 10, h.forward);
    f(&mut ph, 11, h.braking);
    for s in &run.samples {
        let mut p = Msg::default();
        f(&mut p, 1, (s.t * 1000.0).round());
        f(&mut p, 2, s.v);
        f(&mut p, 3, s.a_g.max(0.0));
        // gear index as stock: R = 0, N = 1, 1st = 2
        if s.gear > 0 {
            i(&mut p, 4, s.gear as i64 + 1);
            i(&mut p, 5, s.rpm.round() as i64);
        }
        f(&mut p, 6, s.wheel_hp);
        ph.push(12, Val::Msg(p));
    }
    a.set(5, Val::Msg(ph));
    // acceleration curves are the vehicle-analysis output of the SDK; ours has none
    for k in [15u32, 16, 17, 18] {
        a.remove(k);
    }
    a.fields.sort_by_key(|x| x.num);
    c.put(&path, a.encode());
    specs(c, &d, &h)?;
    c.note(format!(
        "physics: AI lat {:.2} g (high {:.2}), braking {:.2} g, top {:.0} km/h, 0-100 {:.1} s, PI {pi:.1}",
        h.lat_low, h.lat_high, h.braking, h.top_kmh, h.t_0_100
    ));
    Ok(pi)
}

/// The mechanical preset's `car_specs` (CarSpecsData items {1 key, 3 units, 4 value}).
fn specs(c: &mut Ctx, d: &Derived, h: &Hints) -> Result<()> {
    let (pw, _) = check::peak_power(d);
    let (tq, _) = check::peak_torque(d);
    // ui_car.json "specs" values are strings like "115bhp"; ours when absent
    let bhp = specs_num(&c.car.ui_text, "bhp").unwrap_or(pw / 745.7);
    let torque = specs_num(&c.car.ui_text, "torque").unwrap_or(tq);
    let items: Vec<(i64, i64, f64)> = vec![
        (1, 72, bhp.round()),
        (2, 62, torque.round()),
        (11, 40, d.mass.round()),
        (9, 60, specs_num(&c.car.ui_text, "topspeed").unwrap_or(h.top_kmh).round()),
        (8, 26, specs_num(&c.car.ui_text, "acceleration").unwrap_or((h.t_0_100 * 10.0).round() / 10.0)),
        (10, 0, d.gears.len() as f64),
        (19, 3, d.max_fuel.round()),
    ];
    let pre = format!("{}\\presets\\", c.base).to_lowercase();
    let mp: Vec<String> = c.files.map.keys().filter(|k| k.to_lowercase().starts_with(&pre) && k.to_lowercase().ends_with(".mechanicalcarpreset")).cloned().collect();
    for p in mp {
        let mut m = Msg::parse(c.files.get(&p).unwrap())?;
        let mut cs = Msg::default();
        for (key, unit, v) in &items {
            let mut it = Msg::default();
            pbx::i(&mut it, 1, *key);
            if *unit != 0 {
                pbx::i(&mut it, 3, *unit);
            }
            f(&mut it, 4, *v);
            cs.push(1, Val::Msg(it));
        }
        m.set(19, Val::Msg(cs));
        c.files.put(&p, m.encode());
    }
    Ok(())
}

/// A number from ui_car.json `"specs": {"<key>": "115bhp"}`.
fn specs_num(text: &str, key: &str) -> Option<f64> {
    let re = regex::Regex::new(&format!(r#""{key}"\s*:\s*"([^"]*)""#)).ok()?;
    let v = re.captures(text)?.get(1)?.as_str().to_string();
    let n: String = v.chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit() || *c == '.' || *c == ',').collect();
    n.replace(',', ".").parse().ok().filter(|x: &f64| *x > 0.0)
}
