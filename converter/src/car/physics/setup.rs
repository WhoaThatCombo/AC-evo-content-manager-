//! The stock setup (`.carsetup`) and its limits (`.carsetuplimits`).
//!
//! - Every value of the setup lies inside its limit; every limit covers the
//!   car's own value.
//! - Items AC1's setup.ini adjusts are modifiable with AC1's range and step
//!   (units converted: AC1 differential locks, brake bias and brake power
//!   are percent; EVO's diff locks are 0..1). Everything else is fixed at
//!   the car's value (min = max), as on stock road cars.
//! - Ranges and steps are clamped to the game-wide envelope read off all 81
//!   stock `.carsetuplimits` (`ENVELOPE`, measured with examples/setupenv.rs).
//! - Ride height: the setup's `front/rear_target_height` (mm) is the ride
//!   height at the car.ini pickup points (PICKUP_FRONT/REAR_HEIGHT below
//!   the CoG) of the drawn AC1 model; the collar positions (mm) are the
//!   CarData collars. Stock setups are solved from the targets: their
//!   collars differ from CarData's (mx5_na 106 / 98 mm against 122 / 107).
//!   AC1 ROD_LENGTH_xx ranges become a target-height range.

use anyhow::Result;

use super::pbx::{self, f};
use super::Ctx;
use crate::ac1::ini::Ini;
use crate::io::pb::{Msg, Val};

/// (item, min, max, smallest step): the game-wide envelope of the 81 stock
/// `.carsetuplimits`, measured by examples/setupenv.rs
/// (tools/setupenv_stock.tsv): min / max over the limits in use (modifiable
/// ranges and fixed values), the smallest step of a modifiable limit.
const ENVELOPE: [(&str, f64, f64, f64); 17] = [
    ("abs", 0.0, 11.0, 1.0),
    ("arbs", 0.0, 130000.0, 1.0),
    ("bump_rate", 0.0, 10000.0, 100.0),
    ("camber", -6.0, 1.5, 0.01),
    ("damper", 0.0, 26500.0, 1.0),
    ("diff", 0.0, 1.0, 0.01),
    ("front_bias", 48.0, 90.0, 0.1),
    ("fuel", 0.0, 150.0, 1.0),
    ("preload", 0.0, 300.0, 1.0),
    ("pressure", 13.0, 45.0, 0.1),
    ("steer_ratio", -18.0, 20.0, 0.5),
    ("target_height", 30.0, 190.0, 0.5),
    ("tc", 0.0, 12.0, 1.0),
    ("toe", -0.5, 0.5, 0.01),
    ("torque_multiplier", 75.0, 110.0, 1.0),
    ("turbo", 0.0, 100.0, 10.0),
    ("wheel_rate", 0.0, 400000.0, 1.0),
];

fn env(item: &str) -> (f64, f64, f64) {
    ENVELOPE.iter().find(|e| e.0 == item).map(|e| (e.1, e.2, e.3)).unwrap_or((f64::MIN, f64::MAX, 0.0))
}

/// A SetupSettingLimits {1 step, 2 min, 3 max, 4 is_modifiable}.
fn lim(item: &str, value: f64, range: Option<(f64, f64, f64)>) -> Msg {
    let (emin, emax, estep) = env(item);
    let mut m = Msg::default();
    match range {
        Some((lo, hi, step)) => {
            let lo = lo.max(emin).min(value);
            let hi = hi.min(emax).max(value);
            f(&mut m, 1, step.max(estep));
            f(&mut m, 2, lo);
            f(&mut m, 3, hi);
            pbx::b(&mut m, 4, hi > lo);
        }
        None => {
            let _ = estep;
            f(&mut m, 1, if value.abs() < 10.0 { 0.01 } else { 1.0 });
            f(&mut m, 2, value);
            f(&mut m, 3, value);
        }
    }
    m
}

/// An AC1 setup.ini item's (min, max, step) times `scale`, when present.
fn ac1_range(s: &Ini, sec: &str, scale: f64) -> Option<(f64, f64, f64)> {
    let lo = s.num(sec, "MIN")?;
    let hi = s.num(sec, "MAX")?;
    let step = s.num(sec, "STEP").filter(|v| *v > 0.0).unwrap_or(1.0);
    (hi > lo).then(|| (lo * scale, hi * scale, step * scale))
}

fn msg(fields: Vec<(u32, Val)>) -> Msg {
    let mut m = Msg::default();
    for (n, v) in fields {
        m.push(n, v);
    }
    m
}

fn fv(x: f64) -> Val {
    Val::F32((x as f32).to_le_bytes())
}

fn mv(m: Msg) -> Val {
    Val::Msg(m)
}

pub fn write(c: &mut Ctx) -> Result<()> {
    let s = c.ini("setup.ini").clone();
    let si = c.ini("suspensions.ini").clone();
    let d = c.d.clone();
    let corners = ["LF", "RF", "LR", "RR"];
    let ax = |k: usize| if k < 2 { 0 } else { 1 };
    let arb = [si.num("ARB", "FRONT").unwrap_or(0.0), si.num("ARB", "REAR").unwrap_or(0.0)];
    // the diff the drivetrain stage made `differential_data` (the rear diff
    // of the four-wheel types), so a setup load never undoes it
    let diff = c.d.diff;
    let damp = |a: usize, key: &str, dflt: f64| si.num(if a == 0 { "FRONT" } else { "REAR" }, key).filter(|v| *v > 0.0).unwrap_or(dflt);
    let pressure = c.cd.str(10).and_then(|p| c.read_any(&p)).and_then(|b| Msg::parse(&b).ok()).and_then(|m| pbx::rf(&m, 11)).unwrap_or(30.0);
    let camber = |a: usize| si.num(if a == 0 { "FRONT" } else { "REAR" }, "STATIC_CAMBER").unwrap_or(0.0);
    let toe_lin = |a: usize| si.num(if a == 0 { "FRONT" } else { "REAR" }, "TOE_OUT").unwrap_or(0.0);
    let bump_rate = |a: usize| si.num(if a == 0 { "FRONT" } else { "REAR" }, "BUMP_STOP_RATE").unwrap_or(50000.0) * 0.01;
    let turbo_default = c.ini("engine.ini").num("ENGINE_DATA", "DEFAULT_TURBO_ADJUSTMENT").unwrap_or(100.0);
    // ---- values
    let mut set = Msg::default();
    set.push(1, mv(msg(vec![
        (1, fv(arb[0])),
        (1, fv(arb[1])),
        (2, fv(d.steer_ratio)),
        (3, mv(msg(vec![(1, fv(d.front_bias)), (2, fv(100.0))]))),
        (4, mv(msg(vec![(1, fv(diff[0])), (2, fv(diff[1])), (3, fv(diff[2]))]))),
    ])));
    for k in 0..4 {
        let a = ax(k);
        set.push(2, mv(msg(vec![
            (1, fv(d.spring[a])),
            (2, mv(msg(vec![(1, fv(d.bump_up[a])), (2, fv(bump_rate(a)))]))),
            (3, mv(msg(vec![(1, fv(d.bump_dn[a])), (2, fv(bump_rate(a)))]))),
        ])));
    }
    for k in 0..4 {
        let a = ax(k);
        let sb = damp(a, "DAMP_BUMP", 3000.0);
        let sr = damp(a, "DAMP_REBOUND", 6000.0);
        set.push(3, mv(msg(vec![(1, fv(sb)), (2, fv(damp(a, "DAMP_FAST_BUMP", sb * 0.5))), (3, fv(sr)), (4, fv(damp(a, "DAMP_FAST_REBOUND", sr * 0.5)))])));
    }
    for k in 0..4 {
        let a = ax(k);
        let mut al = msg(vec![(1, fv(pressure)), (2, fv(camber(a))), (3, fv(0.0)), (5, fv(camber(a))), (6, fv(toe_lin(a)))]);
        if d.default_compound > 0 {
            al.push(7, fv(d.default_compound as f64));
        }
        set.push(4, mv(al));
    }
    let mut el = Msg::default();
    if d.has_tc {
        el.push(1, fv(if d.tc_on { 1.0 } else { 0.0 }));
    }
    el.push(3, fv(if d.abs_on { 1.0 } else { 0.0 }));
    if d.turbo_adjustable {
        el.push(7, fv(turbo_default));
    }
    set.push(5, mv(el));
    let mut aero = Msg::default();
    for k in 0..4 {
        aero.push(1, fv(d.collar[ax(k)] * 1000.0));
    }
    aero.push(2, fv(d.target_height_mm[0]));
    aero.push(3, fv(d.target_height_mm[1]));
    set.push(6, mv(aero));
    set.push(7, mv(msg(vec![(1, fv(d.fuel))])));
    // ---- limits
    let mut li = Msg::default();
    let ab = |sec: &str| ac1_range(&s, sec, 1.0).filter(|r| r.1 > 100.0);
    li.push(1, mv(msg(vec![
        (1, mv(lim("arbs", arb[0], ab("ARB_FRONT")))),
        (1, mv(lim("arbs", arb[1], ab("ARB_REAR")))),
        (2, mv(lim("steer_ratio", d.steer_ratio, None))),
        (3, mv(msg(vec![
            (1, mv(lim("front_bias", d.front_bias, ac1_range(&s, "FRONT_BIAS", 1.0)))),
            (2, mv(lim("torque_multiplier", 100.0, ac1_range(&s, "BRAKE_POWER_MULT", 1.0)))),
        ]))),
        (4, mv(msg(vec![
            (1, mv(lim("diff", diff[0], ac1_range(&s, "DIFF_POWER", 0.01)))),
            (2, mv(lim("diff", diff[1], ac1_range(&s, "DIFF_COAST", 0.01)))),
            (3, mv(lim("preload", diff[2], ac1_range(&s, "DIFF_PRELOAD", 1.0)))),
        ]))),
    ])));
    for (k, cn) in corners.iter().enumerate() {
        let a = ax(k);
        let mut sl = msg(vec![
            (1, mv(lim("wheel_rate", d.spring[a], ac1_range(&s, &format!("SPRING_RATE_{cn}"), 1.0).filter(|r| r.1 > 1000.0)))),
            (2, mv(msg(vec![(1, mv(lim("bump_range", d.bump_up[a], None))), (2, mv(lim("bump_rate", bump_rate(a), None)))]))),
            (3, mv(msg(vec![(1, mv(lim("bump_range", d.bump_dn[a], None))), (2, mv(lim("bump_rate", bump_rate(a), None)))]))),
        ]);
        sl.push(4, mv(msg(vec![(1, fv(1.0))])));
        sl.push(5, mv(msg(vec![(1, fv(1.0))])));
        li.push(2, mv(sl));
    }
    for (k, cn) in corners.iter().enumerate() {
        let a = ax(k);
        let sb = damp(a, "DAMP_BUMP", 3000.0);
        let sr = damp(a, "DAMP_REBOUND", 6000.0);
        let r = |key: &str| ac1_range(&s, &format!("{key}_{cn}"), 1.0).filter(|r| r.1 > 100.0);
        li.push(3, mv(msg(vec![
            (1, mv(lim("damper", sb, r("DAMP_BUMP")))),
            (2, mv(lim("damper", damp(a, "DAMP_FAST_BUMP", sb * 0.5), r("DAMP_FAST_BUMP")))),
            (3, mv(lim("damper", sr, r("DAMP_REBOUND")))),
            (4, mv(lim("damper", damp(a, "DAMP_FAST_REBOUND", sr * 0.5), r("DAMP_FAST_REBOUND")))),
        ])));
    }
    for (k, cn) in corners.iter().enumerate() {
        let a = ax(k);
        let cam = ac1_range(&s, &format!("CAMBER_{cn}"), 1.0).map(|(lo, hi, st)| {
            // AC1 camber clicks of 1 over a few degrees are tenths of a degree in the game's UI
            (lo, hi, if st >= 1.0 && hi - lo <= 10.0 { 0.1 } else { st })
        });
        let toe = s.has(&format!("TOE_OUT_{cn}")).then_some((-0.2, 0.2, 0.01));
        let mut al = msg(vec![
            (1, mv(lim("pressure", pressure, ac1_range(&s, &format!("PRESSURE_{cn}"), 1.0)))),
            (2, mv(lim("camber", camber(a), cam))),
            (3, mv(lim("toe", 0.0, toe))),
            (4, mv(Msg::default())),
            (5, mv(lim("camber", camber(a), None))),
            (6, mv(lim("toe_lin", toe_lin(a), None))),
        ]);
        let n = d.compounds.max(1) as f64 - 1.0;
        al.push(7, mv(lim("compound", d.default_compound as f64, (n > 0.0).then_some((0.0, n, 1.0)))));
        li.push(4, mv(al));
    }
    let mut el = Msg::default();
    el.push(1, mv(lim("tc", if d.tc_on { 1.0 } else { 0.0 }, d.has_tc.then_some((0.0, d.tc_levels as f64, 1.0)))));
    el.push(2, mv(lim("tc", 0.0, None)));
    el.push(3, mv(lim("abs", if d.abs_on { 1.0 } else { 0.0 }, d.has_abs.then_some((0.0, 1.0, 1.0)))));
    el.push(4, mv(lim("engine_map", 0.0, None)));
    el.push(5, mv(msg(vec![(1, fv(1.0)), (3, fv(99.0)), (4, Val::Varint(1))])));
    el.push(6, mv(lim("esc", 0.0, None)));
    let tl = s.with_prefix("TURBO_").next().and_then(|(sec, _)| ac1_range(&s, sec, 1.0));
    el.push(7, mv(lim("turbo", if d.turbo_adjustable { turbo_default } else { 0.0 }, if d.turbo_adjustable { tl.or(Some((0.0, 100.0, 1.0))) } else { None })));
    li.push(5, mv(el));
    let mut aero = Msg::default();
    for k in 0..4 {
        aero.push(1, mv(lim("collar", d.collar[ax(k)] * 1000.0, None)));
    }
    for (a, cns) in [(0usize, ["LF", "RF"]), (1, ["LR", "RR"])] {
        let rod = cns.iter().find_map(|cn| ac1_range(&s, &format!("ROD_LENGTH_{cn}"), 1000.0));
        let t = d.target_height_mm[a];
        let r = rod.map(|(lo, hi, st)| (t - (hi - lo) / 2.0, t + (hi - lo) / 2.0, st.abs().max(0.5)));
        aero.push(2 + a as u32, mv(lim("target_height", t, r)));
    }
    aero.push(4, mv(Msg::default()));
    aero.push(5, mv(Msg::default()));
    li.push(6, mv(aero));
    li.push(7, mv(msg(vec![(1, mv(lim("fuel", d.fuel, Some((1.0, d.max_fuel, 1.0)))))])));
    // write over the donor's files (CarData names them)
    for (num, new) in [(19u32, set), (16u32, li)] {
        let Some(p) = c.cd.str(num) else { continue };
        let mut out = new;
        if num == 19 {
            // keep the donor's final state name and version
            if let Some(old) = c.files.find_ci(&p).and_then(|k| c.files.get(&k)).and_then(|b| Msg::parse(b).ok()) {
                for k in [9u32, 10, 11] {
                    if let Some(v) = old.get(k) {
                        out.push(k, v.clone());
                    }
                }
            }
        }
        c.put(&p, out.encode());
    }
    // the AI and wet setups are the stock one
    for num in [190u32, 191] {
        c.cd.remove(num);
    }
    c.note(format!(
        "physics: setup: ride height targets {:.0} / {:.0} mm, collars {:.0} / {:.0} mm; modifiable: {}",
        d.target_height_mm[0], d.target_height_mm[1], d.collar[0] * 1000.0, d.collar[1] * 1000.0,
        s.names().filter(|n| !n.is_empty() && *n != "DISPLAY_METHOD" && *n != "GEARS").collect::<Vec<_>>().join(" ")
    ));
    Ok(())
}
