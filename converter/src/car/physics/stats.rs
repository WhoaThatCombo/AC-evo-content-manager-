//! Physics numbers read back from a car package (stock or converted): the
//! `Derived` model of a CarData and the files it names, the `phys.*`
//! metrics stockstats compares, and the data-level checks the validator
//! runs (`check-car`).

use std::collections::BTreeMap;

use super::check;
use super::pbx::{read_curve, rf, rf_at};
use super::{Derived, G, KG_PER_LITRE};
use crate::io::pb::{Msg, Val};

fn strs(m: &Msg, num: u32) -> Vec<String> {
    m.all(num).filter_map(|v| match v { Val::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()), _ => None }).collect()
}

/// The model of one car from its CarData; `get` reads any package or game path.
pub fn derive(cd: &Msg, get: &dyn Fn(&str) -> Option<Vec<u8>>) -> Derived {
    let mut d = Derived::default();
    let gen = cd.sub(1).unwrap_or_default();
    d.mass = rf(&gen, 1).unwrap_or(1200.0);
    d.fuel = rf(&gen, 5).unwrap_or(30.0);
    d.max_fuel = rf(&gen, 6).unwrap_or(d.fuel);
    d.mass_static = d.mass + d.fuel * KG_PER_LITRE;
    let s = cd.sub(2).unwrap_or_default();
    d.wheelbase = rf(&s, 2).unwrap_or(2.5);
    d.cg_front = rf(&s, 3).unwrap_or(0.5);
    d.track = [rf(&s, 52).unwrap_or(1.5), rf(&s, 53).unwrap_or(1.5)];
    // engine: the .carengine wins over the inline copy (the game loads the file)
    let e = cd.str(20).and_then(|p| get(&p)).and_then(|b| Msg::parse(&b).ok()).or_else(|| cd.sub(4)).unwrap_or_default();
    d.torque = e.str(2).and_then(|p| get(&p)).map(|b| read_curve(&b)).unwrap_or_default();
    d.idle = rf(&e, 5).unwrap_or(900.0);
    d.limiter = rf(&e, 6).unwrap_or(7000.0);
    let mut turbos = e.subs(24);
    if turbos.is_empty() {
        turbos = strs(&e, 29).iter().filter_map(|p| get(p)).filter_map(|b| Msg::parse(&b).ok()).collect();
    }
    let ctrls = e.subs(25);
    let mut boost = Vec::new();
    let mut r = 0.0;
    while r <= d.limiter + 1000.0 {
        let mut b = 0.0;
        for (k, t) in turbos.iter().enumerate() {
            let (mb, refr, gam, wg) = (rf(t, 1).unwrap_or(0.0), rf(t, 4).unwrap_or(0.0), rf(t, 5).unwrap_or(1.0), rf(t, 7).unwrap_or(0.0));
            let spool = if refr > 0.0 { (r / refr).powf(gam.max(0.1)).min(1.0) } else { 1.0 };
            let stages = ctrls.get(k).map(|c| c.subs(2)).unwrap_or_default();
            let target = if stages.is_empty() {
                mb
            } else {
                stages.iter().map(|st| {
                    let l = st.str(3).and_then(|p| get(&p)).map(|b| read_curve(&b)).unwrap_or_default();
                    match st.varint(1).unwrap_or(0) {
                        18 => check::lut(&l, r),
                        2 => check::lut(&l, 1.0),
                        17 => rf(st, 8).or_else(|| l.first().map(|p| p.1)).unwrap_or(0.0),
                        _ => l.iter().fold(0.0f64, |a, p| a.max(p.1)),
                    }
                }).sum::<f64>()
            };
            let mut x = target.max(0.0) * spool;
            if wg > 0.0 {
                x = x.min(wg);
            }
            b += x;
        }
        boost.push((r, b));
        r += 250.0;
    }
    d.boost = boost;
    let gb = cd.str(24).and_then(|p| get(&p)).and_then(|b| Msg::parse(&b).ok()).or_else(|| cd.sub(23)).unwrap_or_default();
    for g in gb.subs(2) {
        let name = g.str(1).unwrap_or_default();
        let ratio = rf(&g, 2).unwrap_or(0.0);
        if name == "R" || ratio < 0.0 {
            d.reverse = ratio.abs();
        } else if name != "N" && ratio > 0.0 {
            d.gears.push(ratio);
        }
    }
    d.final_drive = rf(&gb, 3).unwrap_or(4.0);
    d.shift_up_rpm = rf_at(&gb, &[13, 1]).unwrap_or(d.limiter * super::drivetrain::AUTO_UP_OF_LIMITER);
    d.shift_down_rpm = rf_at(&gb, &[13, 5]).unwrap_or(d.limiter * super::drivetrain::AUTO_DOWN_OF_LIMITER);
    d.traction = match cd.sub(3).and_then(|x| x.varint(2)).unwrap_or(0) { 1 => 1, 0 => 0, _ => 2 };
    d.efficiency = match d.traction { 1 => 0.88, 2 => 0.82, _ => 0.85 };
    // tyres (first compound of each axle)
    let fz = [d.mass_static * G * d.cg_front / 2.0, d.mass_static * G * (1.0 - d.cg_front) / 2.0];
    for (a, num) in [(0usize, 10u32), (1, 11)] {
        let Some(t) = cd.str(num).and_then(|p| get(&p)).and_then(|b| Msg::parse(&b).ok()) else { continue };
        let td = t.sub(3).unwrap_or_default();
        let md = t.sub(4).unwrap_or_default();
        d.tyre_radius[a] = rf(&td, 2).unwrap_or(0.3);
        d.tyre_rate[a] = rf(&td, 3).unwrap_or(280000.0);
        let fz0 = rf(&md, 5).unwrap_or(4000.0);
        d.mu_y[a] = rf(&md, 1).unwrap_or(1.2) * (fz[a] / fz0).powf(rf(&md, 3).unwrap_or(0.85) - 1.0);
        d.mu_x[a] = rf(&md, 2).unwrap_or(1.2) * (fz[a] / fz0).powf(rf(&md, 4).unwrap_or(0.85) - 1.0);
    }
    let b = cd.sub(5).unwrap_or_default();
    d.brake_torque = rf(&b, 1).unwrap_or(2000.0);
    d.front_bias = rf(&b, 2).unwrap_or(65.0);
    // aero: wings at their angle
    for w in cd.sub(12).map(|a| a.subs(3)).unwrap_or_default() {
        let area = rf(&w, 3).unwrap_or(1.0) * rf(&w, 4).unwrap_or(1.0);
        let ang = rf(&w, 13).unwrap_or(0.0);
        let at = |n: u32| w.str(n).and_then(|p| get(&p)).map(|b| check::lut(&read_curve(&b), ang)).unwrap_or(0.0);
        d.cl_area += at(7) * rf(&w, 12).unwrap_or(0.0) * area;
        d.cd_area += at(8) * rf(&w, 11).unwrap_or(0.0) * area;
    }
    for (a, num, geo) in [(0usize, 8u32, 40u32), (1, 9, 41)] {
        let co = s.sub(num).unwrap_or_default();
        d.spring[a] = rf(&co, 1).unwrap_or(40000.0);
        d.collar[a] = rf(&co, 5).unwrap_or(0.0);
        d.bump_up[a] = rf_at(&co, &[3, 1]).unwrap_or(0.05);
        d.bump_dn[a] = rf_at(&co, &[4, 1]).unwrap_or(0.05);
        let hub = rf_at(&s, &[geo, 1, 3]).unwrap_or(40.0);
        let load = fz[a];
        d.sag[a] = (load - hub * G).max(load * 0.5) / d.spring[a].max(1000.0);
        d.rest_x[a] = d.sag[a] - d.collar[a];
    }
    let k = cd.sub(8).unwrap_or_default();
    d.steer_lock = rf(&k, 2).unwrap_or(450.0);
    d.steer_ratio = rf(&k, 3).unwrap_or(14.0);
    d
}

/// `phys.*` metrics of one car.
pub fn metrics(d: &Derived) -> BTreeMap<String, f64> {
    let mut m = BTreeMap::new();
    let mut put = |k: &str, v: f64| {
        if v.is_finite() {
            m.insert(format!("phys.{k}"), v);
        }
    };
    let (pw, pr) = check::peak_power(d);
    let (tq, _) = check::peak_torque(d);
    put("mass", d.mass);
    put("cg_front", d.cg_front);
    put("wheelbase", d.wheelbase);
    put("track_front", d.track[0]);
    put("track_rear", d.track[1]);
    put("power_kw", pw / 1000.0);
    put("power_rpm", pr);
    put("torque_nm", tq);
    put("kw_per_tonne", pw / d.mass);
    put("idle", d.idle);
    put("limiter", d.limiter);
    put("gears", d.gears.len() as f64);
    put("final", d.final_drive);
    if let (Some(a), Some(b)) = (d.gears.first(), d.gears.last()) {
        put("ratio_first", *a);
        put("ratio_top", *b);
        put("kmh_first_at_limiter", check::speed_at(d, *a, d.limiter) * 3.6);
        put("kmh_top_at_limiter", check::speed_at(d, *b, d.limiter) * 3.6);
    }
    put("tyre_radius_front", d.tyre_radius[0]);
    put("tyre_radius_rear", d.tyre_radius[1]);
    put("mu_y_front", d.mu_y[0]);
    put("mu_y_rear", d.mu_y[1]);
    put("spring_front", d.spring[0]);
    put("spring_rear", d.spring[1]);
    put("bump_front", d.bump_up[0]);
    put("bump_rear", d.bump_up[1]);
    put("brake_torque", d.brake_torque);
    put("front_bias", d.front_bias);
    put("steer_lock", d.steer_lock);
    put("wheel_lock_deg", d.steer_lock / d.steer_ratio.abs().max(1.0));
    put("cd_area", d.cd_area);
    put("cl_area", d.cl_area);
    let (pi, h, _) = super::ai::perf_indicator(d);
    put("top_kmh", h.top_kmh);
    put("lat_g", h.lat_low);
    put("pi", pi);
    m
}

/// The CarData of a car's files (`data\*.car`, `cardata.car` first).
pub fn cardata_of<'a>(paths: impl Iterator<Item = &'a str>) -> Option<String> {
    let mut cars: Vec<&str> = paths.filter(|p| {
        let l = p.to_lowercase();
        l.ends_with(".car") && l.contains("\\data\\")
    }).collect();
    cars.sort_by_key(|p| (!p.to_lowercase().ends_with("cardata.car"), p.to_lowercase()));
    cars.first().map(|s| s.to_string())
}

/// Every check of the converted physics: (errors, warnings).
pub fn validate(files: &BTreeMap<String, Vec<u8>>, game: Option<&crate::io::kspkg::Pkg>) -> (Vec<String>, Vec<String>) {
    let (mut err, mut warn) = (Vec::new(), Vec::new());
    let lower: BTreeMap<String, &Vec<u8>> = files.iter().map(|(k, v)| (k.to_lowercase(), v)).collect();
    let get = |p: &str| -> Option<Vec<u8>> {
        lower.get(&p.to_lowercase()).map(|v| (*v).clone()).or_else(|| {
            let g = game?;
            g.find(p).and_then(|e| g.read(e).ok())
        })
    };
    let Some(cdp) = cardata_of(files.keys().map(|s| s.as_str())) else {
        err.push("physics: no data\\*.car (CarData)".into());
        return (err, warn);
    };
    let Ok(cd) = Msg::parse(files.get(&cdp).unwrap()) else {
        err.push(format!("physics: {cdp} does not parse"));
        return (err, warn);
    };
    // every physics file parses
    for (p, b) in files {
        let l = p.to_lowercase();
        let phys = [".car", ".carengine", ".gearbox", ".drivetrain", ".clutch", ".brakesystem", ".suspension", ".coilover", ".wing", ".turbo", ".tyre", ".carsetup", ".carsetuplimits", ".aicardata", ".curve", ".compatibletyres", ".mechanicalcarpreset"];
        if phys.iter().any(|e| l.ends_with(e)) && Msg::parse(b).is_err() {
            err.push(format!("physics: {p} does not parse as protobuf"));
        }
    }
    // inline copies equal their standalone files
    let enc = |v: &Val| match v { Val::Bytes(b) => Some(b.clone()), Val::Msg(m) => Some(m.encode()), _ => None };
    let mut pairs: Vec<(String, Option<Vec<u8>>, String)> = Vec::new();
    for (inl, path, what) in [(4u32, 20u32, "engine"), (3, 22, "drivetrain"), (23, 24, "gearbox"), (25, 26, "clutch"), (5, 27, "brakes")] {
        if let Some(p) = cd.str(path) {
            pairs.push((what.into(), cd.get(inl).and_then(enc), p));
        }
    }
    if let Some(s) = cd.sub(2) {
        for (inl, path, what) in [(8u32, 20u32, "front coilover"), (9, 21, "rear coilover"), (40, 22, "front geometry"), (41, 23, "rear geometry")] {
            if let Some(p) = s.str(path) {
                pairs.push((what.into(), s.get(inl).and_then(enc), p));
            }
        }
    }
    if let Some(a) = cd.sub(12) {
        let w: Vec<Option<Vec<u8>>> = a.all(3).map(|v| enc(v)).collect();
        for (k, p) in strs(&a, 10).into_iter().enumerate() {
            pairs.push((format!("wing {k}"), w.get(k).cloned().flatten(), p));
        }
    }
    for (what, inline, p) in pairs {
        match (inline, get(&p)) {
            (_, None) => err.push(format!("physics: {what} file {p} is missing")),
            (None, _) => warn.push(format!("physics: CarData has no inline {what}")),
            (Some(i), Some(f)) => {
                if Msg::parse(&i).map(|m| m.encode()).ok() != Msg::parse(&f).map(|m| m.encode()).ok() {
                    err.push(format!("physics: CarData's inline {what} differs from {p}"));
                }
            }
        }
    }
    let d = derive(&cd, &get);
    if d.torque.len() < 2 {
        err.push("physics: no torque curve".into());
        return (err, warn);
    }
    for p in check::gearing(&d).problems {
        err.push(format!("physics: {p}"));
    }
    for p in check::chassis(&d) {
        warn.push(format!("physics: {p}"));
    }
    // setup values inside their limits
    if let (Some(sp), Some(lp)) = (cd.str(19), cd.str(16)) {
        match (get(&sp).and_then(|b| Msg::parse(&b).ok()), get(&lp).and_then(|b| Msg::parse(&b).ok())) {
            (Some(s), Some(l)) => {
                for o in setup_outside(&s, &l, "") {
                    err.push(format!("physics: setup value outside its limit: {o}"));
                }
            }
            _ => err.push("physics: the stock setup or its limits are missing".into()),
        }
    }
    // stock ranges (warnings)
    for (k, v) in metrics(&d) {
        if let Some((lo, hi)) = STOCK.iter().find(|s| s.0 == k).map(|s| (s.1, s.2)) {
            if v < lo || v > hi {
                warn.push(format!("physics: {k} = {v:.3} is outside the stock range {lo} .. {hi}"));
            }
        }
    }
    (err, warn)
}

/// Stock min / max of the metrics the validator warns on: `car_ranges` of
/// stockstats/stockstats.json (`aceconv stockstats`, 71 cars).
const STOCK: [(&str, f64, f64); 12] = [
    ("phys.mass", 605.0, 2155.0),
    ("phys.cg_front", 0.285, 0.69),
    ("phys.wheelbase", 2.035, 3.6),
    ("phys.track_front", 1.25, 1.76),
    ("phys.track_rear", 1.2, 1.77),
    ("phys.limiter", 6200.0, 18800.0),
    ("phys.idle", 500.0, 2500.0),
    ("phys.final", 1.0, 5.1),
    ("phys.spring_front", 14000.0, 300000.0),
    ("phys.spring_rear", 10000.0, 300000.0),
    ("phys.brake_torque", 1000.0, 5500.0),
    ("phys.steer_lock", 217.0, 630.0),
];

/// Setup values (by matching field paths) outside their limits.
fn setup_outside(s: &Msg, l: &Msg, path: &str) -> Vec<String> {
    let mut out = Vec::new();
    // repeated fields pair up in order
    let mut seen: BTreeMap<u32, usize> = BTreeMap::new();
    for fld in &s.fields {
        let k = *seen.entry(fld.num).and_modify(|x| *x += 1).or_insert(0);
        let lims: Vec<&Val> = l.all(fld.num).collect();
        let Some(lv) = lims.get(k) else { continue };
        let lm = match lv { Val::Bytes(b) => Msg::parse(b).ok(), Val::Msg(m) => Some(m.clone()), _ => None };
        let Some(lm) = lm else { continue };
        let p = format!("{path}.f{}[{k}]", fld.num);
        match &fld.val {
            Val::F32(b) => {
                let v = f32::from_le_bytes(*b) as f64;
                let (lo, hi) = (rf(&lm, 2).unwrap_or(0.0), rf(&lm, 3).unwrap_or(0.0));
                // an empty limit (no max) is a fixed or free item the game ignores
                if lm.get(3).is_some() && (v < lo - 1e-3 || v > hi + 1e-3) {
                    out.push(format!("{p} = {v} not in {lo} .. {hi}"));
                }
            }
            Val::Bytes(_) | Val::Msg(_) => {
                let sm = match &fld.val { Val::Bytes(b) => Msg::parse(b).ok(), Val::Msg(m) => Some(m.clone()), _ => None };
                if let Some(sm) = sm {
                    if sm.fields.iter().all(|x| matches!(x.val, Val::F32(_) | Val::Bytes(_) | Val::Msg(_))) && !sm.fields.is_empty() {
                        out.extend(setup_outside(&sm, &lm, &p));
                    }
                }
            }
            _ => {}
        }
    }
    out
}
