//! Engine: torque curve, coast, throttle, limiter, idle, inertia, turbos and
//! their boost controllers.
//!
//! - `powerCurve` is AC1's power.lut as it is (rpm -> N m): stock
//!   ks_mazda_mx5_na's HEADER_POWER_NA16 peaks at 135 N m / 5500 like AC1
//!   ks_mazda_miata's power.lut.
//! - `minimum`, `limiter`, `limiterCycles`, `inertia` are MINIMUM, LIMITER,
//!   LIMITER_HZ, INERTIA ([ENGINE_DATA]); `nominal_rpm` is [DAMAGE]
//!   RPM_THRESHOLD (mx5_na 7700 = AC1 7700).
//! - Turbos (TurboData) keep AC1's [TURBO_n] names: MAX_BOOST, WASTEGATE,
//!   LAG_UP / LAG_DN, REFERENCE_RPM -> rpm_Ref, GAMMA -> gamma_Rpm and
//!   gamma_Gas (quattro: AC1 1.8 / 1.03 -> EVO 1.8 / 1.04). The engine's
//!   `max_turbo_boost` is the boost cap.
//! - Boost controllers: EVO's controller output IS the boost target (stock
//!   turbos with a controller often have no max_boost at all); stage inputs
//!   are the game's enum (DynamicControllerStageData.Input: 18 RPMS, 2 Gas,
//!   6 Speed, 7 Gear, 17 Const, ...). AC1's ctrl_turbo<n>.ini controller
//!   caps the boost instead, so ours keeps AC1's MAX_BOOST as the wastegate:
//!   at full spool both give min(MAX_BOOST, controller).
//! - Coast (OURS): AC1 [COAST_REF] (TORQUE at RPM, NON_LINEARITY 0 linear ..
//!   1 quadratic) -> a negative torque curve from 0 to 1.2 x the limiter,
//!   as stock coast curves are; a coast .lut is used as it is (negated when
//!   positive).

use anyhow::Result;

use super::pbx::{self, at, f, i};
use super::Ctx;
use crate::io::pb::{Msg, Val};

use super::ctrl::Stage;

/// AC1 `ctrl_turbo<n>.ini` (absent -> no controller).
fn controller(c: &mut Ctx, n: usize) -> Vec<Stage> {
    super::ctrl::read(c, &format!("ctrl_turbo{n}.ini"), &format!("turbo {n}")).unwrap_or_default()
}

struct Turbo {
    max_boost: f64,
    wastegate: f64,
    lag_up: f64,
    lag_dn: f64,
    ref_rpm: f64,
    gamma: f64,
    adjustable: bool,
    stages: Vec<Stage>,
}

impl Turbo {
    /// Steady boost at full throttle and full lag settling.
    fn boost(&self, rpm: f64) -> f64 {
        let spool = if self.ref_rpm > 0.0 { (rpm / self.ref_rpm).max(0.0).powf(self.gamma.max(0.1)).min(1.0) } else { 1.0 };
        let mut target = self.max_boost;
        if !self.stages.is_empty() {
            let mut v = 0.0;
            for s in &self.stages {
                let x = match s.input {
                    18 => pbx::lerp_lut(&s.lut, rpm),
                    2 => pbx::lerp_lut(&s.lut, 1.0),
                    17 => s.lut.first().map(|p| p.1).unwrap_or(0.0),
                    _ => s.lut.iter().fold(0.0f64, |a, p| a.max(p.1)),
                };
                if s.add { v += x } else { v *= x }
            }
            target = target.min(v.max(0.0));
        }
        (target * spool).min(if self.wastegate > 0.0 { self.wastegate } else { f64::MAX }).max(0.0)
    }
}

pub fn write(c: &mut Ctx) -> Result<()> {
    let ei = c.ini("engine.ini").clone();
    let power_file = ei.get("HEADER", "POWER_CURVE").unwrap_or("power.lut").to_string();
    let mut torque = c.lut(&power_file);
    torque.retain(|p| p.0.is_finite() && p.1.is_finite());
    torque.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    if torque.len() < 2 {
        anyhow::bail!("engine.ini POWER_CURVE {power_file}: no usable torque curve");
    }
    let peak = torque.iter().fold(0.0f64, |a, p| a.max(p.1));
    let idle = ei.num("ENGINE_DATA", "MINIMUM").filter(|v| *v > 100.0).unwrap_or(900.0);
    let limiter = match ei.num("ENGINE_DATA", "LIMITER").filter(|v| *v > idle) {
        Some(l) => l,
        None => {
            let l = torque.iter().filter(|p| p.1 > 0.05 * peak).map(|p| p.0).fold(0.0, f64::max) - 200.0;
            c.note(format!("physics: engine.ini has no limiter; {l:.0} rpm (where the torque curve ends)"));
            l
        }
    };
    // turbos
    let mut turbos = Vec::new();
    for n in 0..8 {
        let s = format!("TURBO_{n}");
        if !ei.has(&s) {
            continue;
        }
        let mb = ei.num(&s, "MAX_BOOST").unwrap_or(0.0);
        let wg = ei.num(&s, "WASTEGATE").unwrap_or(mb);
        let stages = controller(c, n);
        turbos.push(Turbo {
            max_boost: mb,
            // with an AC1 controller the controller is the cap: keep MAX_BOOST as the wastegate
            wastegate: if stages.is_empty() { wg } else { mb.max(wg) },
            lag_up: ei.num(&s, "LAG_UP").unwrap_or(0.99),
            lag_dn: ei.num(&s, "LAG_DN").unwrap_or(0.99),
            ref_rpm: ei.num(&s, "REFERENCE_RPM").unwrap_or(3000.0),
            gamma: ei.num(&s, "GAMMA").unwrap_or(2.0),
            adjustable: ei.num(&s, "COCKPIT_ADJUSTABLE").unwrap_or(0.0) > 0.0,
            stages,
        });
    }
    c.d.torque = torque.clone();
    c.d.idle = idle;
    c.d.limiter = limiter;
    c.d.turbo = !turbos.is_empty();
    c.d.turbo_adjustable = turbos.iter().any(|t| t.adjustable);
    let mut boost = Vec::new();
    let mut r = 0.0;
    while r <= limiter + 1000.0 {
        boost.push((r, turbos.iter().map(|t| t.boost(r)).sum::<f64>()));
        r += 250.0;
    }
    c.d.boost = boost;
    // curves
    let e_cd = c.cd.sub(4).unwrap_or_default();
    let path_of = |num: u32, file: &str| e_cd.str(num).unwrap_or_else(|| format!("{}\\data\\{file}", c.base));
    let p_power = path_of(2, "header_power.curve");
    let p_coast = path_of(3, "coast.curve");
    let p_throttle = path_of(14, "throttle.curve");
    c.put(&p_power, pbx::curve(&torque));
    let coast = coast_curve(c, &ei, limiter);
    c.put(&p_coast, pbx::curve(&coast));
    let throttle: Vec<(f64, f64)> = c.data_text("throttle.lut").map(crate::ac1::ini::lut).filter(|l| l.len() >= 2).map(|l| {
        // AC1 throttle.lut: pedal % -> throttle % (0..100); a 0..1 table is scaled
        let s = if l.iter().all(|p| p.0 <= 1.0 && p.1 <= 1.0) { 100.0 } else { 1.0 };
        l.iter().map(|p| (p.0 * s, p.1 * s)).collect()
    }).unwrap_or_else(|| vec![(0.0, 0.0), (100.0, 100.0)]);
    c.put(&p_throttle, pbx::curve(&throttle));
    // EngineData (CarData f4; the .carengine is written from it)
    let base = c.base.clone();
    let id = c.id.clone();
    let mut ctrl_files: Vec<(String, Vec<u8>)> = Vec::new();
    let e = at(&mut c.cd, &[4])?;
    e.remove(51);
    f(e, 1, ei.num("ENGINE_DATA", "INERTIA").filter(|v| *v > 0.0).unwrap_or(0.12));
    e.set_str(2, &p_power);
    e.set_str(3, &p_coast);
    e.remove(4);
    i(e, 5, idle.round() as i64);
    i(e, 6, limiter.round() as i64);
    i(e, 7, ei.num("ENGINE_DATA", "LIMITER_HZ").filter(|v| *v > 0.0).unwrap_or(20.0).round() as i64);
    e.set_str(14, &p_throttle);
    f(e, 15, ei.num("DAMAGE", "RPM_THRESHOLD").filter(|v| *v > limiter).unwrap_or(limiter + 500.0));
    pbx::fo(e, 16, ei.num("DAMAGE", "TURBO_BOOST_THRESHOLD"));
    pbx::fo(e, 17, ei.num("DAMAGE", "TURBO_DAMAGE_K"));
    for n in [24u32, 25, 26, 29, 34] {
        e.remove(n);
    }
    let any_ctrl = turbos.iter().any(|t| !t.stages.is_empty());
    for (k, t) in turbos.iter().enumerate() {
        let mut m = Msg::default();
        f(&mut m, 1, t.max_boost);
        f(&mut m, 2, t.lag_up);
        f(&mut m, 3, t.lag_dn);
        f(&mut m, 4, t.ref_rpm);
        f(&mut m, 5, t.gamma);
        f(&mut m, 6, t.gamma);
        f(&mut m, 7, t.wastegate);
        pbx::b(&mut m, 8, t.adjustable);
        e.push(24, Val::Msg(m));
        e.push(29, Val::Bytes(format!("{base}\\data\\{id}{k}.turbo").into_bytes()));
        // controllers pair with turbos by index: when any turbo has one,
        // every turbo gets one (a constant MAX_BOOST stage for the others)
        if !any_ctrl {
            continue;
        }
        let konst = [Stage { input: 17, add: true, lut: vec![(0.0, t.max_boost)], filter: 0.99, up: None, down: None }];
        let stages: &[Stage] = if t.stages.is_empty() { &konst } else { &t.stages };
        let (dc, files) = super::ctrl::message(stages, &format!("{base}\\data\\ctrl_turbo{k}_controller"));
        ctrl_files.extend(files);
        e.push(25, Val::Msg(dc));
    }
    if let Some(cap) = turbos.iter().map(|t| if t.wastegate > 0.0 { t.max_boost.min(t.wastegate) } else { t.max_boost }).reduce(f64::max) {
        f(e, 34, cap);
    }
    e.fields.sort_by_key(|x| x.num);
    for (p, b) in ctrl_files {
        c.put(&p, b);
    }
    let (pk, at_rpm) = super::check::peak_power(&c.d);
    c.note(format!("physics: engine {:.0} N m peak, {:.0} kW at {at_rpm:.0} rpm, idle {idle:.0}, limiter {limiter:.0}, {} turbo(s)", peak, pk / 1000.0, turbos.len()));
    Ok(())
}

fn coast_curve(c: &mut Ctx, ei: &crate::ac1::ini::Ini, limiter: f64) -> Vec<(f64, f64)> {
    let mode = ei.get("HEADER", "COAST_CURVE").unwrap_or("FROM_COAST_REF").trim().to_uppercase();
    if mode != "FROM_COAST_REF" && mode != "FROM_COAST_DATA" {
        let file = ei.get("COAST_CURVE", "FILENAME").map(|s| s.to_string()).unwrap_or_else(|| mode.to_lowercase());
        let l = c.lut(&file);
        if l.len() >= 2 {
            let sign = if l.iter().map(|p| p.1).sum::<f64>() > 0.0 { -1.0 } else { 1.0 };
            return l.iter().map(|p| (p.0, p.1 * sign)).collect();
        }
    }
    let rpm = ei.num("COAST_REF", "RPM").filter(|v| *v > 0.0).unwrap_or(limiter);
    let tq = ei.num("COAST_REF", "TORQUE").filter(|v| *v > 0.0).unwrap_or(40.0);
    let nl = ei.num("COAST_REF", "NON_LINEARITY").unwrap_or(0.0).clamp(0.0, 1.0);
    let mut out = Vec::new();
    let top = (limiter * 1.2).max(rpm);
    let n = 12;
    for k in 0..=n {
        let r = top * k as f64 / n as f64;
        let x = r / rpm;
        out.push((r, -tq * ((1.0 - nl) * x + nl * x * x)));
    }
    out
}
