//! Suspension: geometry and hard points, springs, dampers, anti-roll bars,
//! bump stops, the collar (ride height) and the track / wheelbase / CoG.
//!
//! - Hard points are AC1's suspensions.ini points unchanged (relative to the
//!   wheel centre at the design position, x positive inboard): stock
//!   ks_mazda_mx5_na's double-wishbone points equal AC1 ks_mazda_miata's.
//!   DWB -> `dW_data`, STRUT -> `strut` (STRUT_CAR / STRUT_TYRE + the lower
//!   arm and steering points), AXLE -> `axle` (the [AXLE] J<n>_CAR / J<n>_AXLE
//!   links, ATTACH_REL_POS, TORQUE_REACTION, LEAF_SPRING_LAT_K), ML ->
//!   `multi_link_data` (JOINT<n>_CAR / JOINT<n>_TYRE).
//! - `springRate` is AC1's SPRING_RATE (a wheel rate on both sides: the
//!   setup calls it `wheel_rate`), dampers are DAMP_BUMP / DAMP_REBOUND
//!   (slow) and DAMP_FAST_* (fast) with the fast thresholds, anti-roll bars
//!   are [ARB] FRONT / REAR, base_y is BASEY: all equal on ae86 / supra.
//! - Bump stops (BumpStopData): Force at Reference with exponent Gamma;
//!   stock 350 N at 0.01 m is AC1 mx5's BUMP_STOP_RATE 35000 N/m (ae86 30000
//!   / 50000 -> 300 / 500), so Force = BUMP_STOP_RATE x 0.01 with the stock
//!   Reference 0.01 and Gamma 1.5. Damping (6) is written only when the
//!   template car's stop has it, at the stock median (`BUMP_DAMPING_*`).
//! - Ride height (OURS): with a collar position c the coilover rests at
//!   x = sag - c (compression from the design position), sag the static
//!   wheel load over the wheel rate. Over the stock fleet the body's offset
//!   from its design pose follows sag - c within about +-3 cm. The converted
//!   car must rest where the AC1 model is drawn: wheel centre at
//!   `wheel_centres.y - cog.y` from the CoG, minus the tyre's static
//!   deflection, so x = that - BASEY - deflection and c = sag - x. The setup
//!   additionally carries the ride height at the pickup points as the target
//!   (setup.rs), which is what the game's setup solves collars from.
//! - Travel (OURS): bump / droop room at rest = AC1's (BUMPSTOP_UP - x_ac1,
//!   BUMPSTOP_DN + x_ac1, x_ac1 = sag - ROD_LENGTH), capped at 0.09 / 0.05 m (stock p90 / p50),
//!   at least |x| + 1.5 cm so
//!   the rest pose is inside the stops whether the game measures the
//!   ranges from rest or from the design position.

use anyhow::Result;

use super::pbx::{self, at, f, i};
use super::{ac1_v3, Ctx, G};
use crate::ac1::ini::Ini;
use crate::io::pb::{Msg, Val};

/// Travel caps (OURS): the stock p90 of bumpStopUp Range and the p50 of
/// bumpStopDown Range (0.09 / 0.045 m). AC1 BUMPSTOP_DN is often a far hard
/// limit (0.16 m on ks_mazda_miata's rear) that lets the wheel hang into
/// the arch at full droop (evoview anim_tyre_in_body).
const BUMP_MAX: f64 = 0.09;
/// BumpStopData 6 `damping`: the stock median over the stops that carry it,
/// bump 400 (135 of 142 stock stops) / droop 500 (132 of 142), measured by
/// examples/bumpstops.rs (tools/bumpstops_stock.tsv).
const BUMP_DAMPING_UP: f64 = 400.0;
const BUMP_DAMPING_DOWN: f64 = 500.0;
const DROOP_MAX: f64 = 0.05;

const DWB: [(&str, u32); 8] = [("WBCAR_TOP_FRONT", 1), ("WBCAR_TOP_REAR", 2), ("WBCAR_BOTTOM_FRONT", 3), ("WBCAR_BOTTOM_REAR", 4), ("WBTYRE_TOP", 5), ("WBTYRE_BOTTOM", 6), ("WBCAR_STEER", 7), ("WBTYRE_STEER", 8)];
const STRUT: [(&str, u32); 7] = [("STRUT_CAR", 1), ("STRUT_TYRE", 2), ("WBCAR_BOTTOM_FRONT", 3), ("WBCAR_BOTTOM_REAR", 4), ("WBTYRE_BOTTOM", 5), ("WBCAR_STEER", 6), ("WBTYRE_STEER", 7)];

fn points(s: &Ini, sec: &str, keys: &[(&str, u32)]) -> Option<Msg> {
    let mut m = Msg::default();
    for (k, n) in keys {
        m.set(*n, pbx::v3(ac1_v3(s, sec, k)?));
    }
    Some(m)
}

/// The geometry message of one axle; the kind name for notes.
fn geometry(c: &mut Ctx, s: &Ini, sec: &str) -> Msg {
    let kind = s.get(sec, "TYPE").unwrap_or("DWB").trim().to_uppercase();
    let mut g = Msg::default();
    let mut basic = Msg::default();
    let hub = s.num(sec, "HUB_MASS").unwrap_or(40.0);
    f(&mut basic, 3, if kind == "AXLE" { (hub * 0.3).max(20.0) } else { hub });
    f(&mut basic, 4, s.num(sec, "TOE_OUT").unwrap_or(0.0));
    f(&mut basic, 5, s.num(sec, "STATIC_CAMBER").unwrap_or(0.0));
    f(&mut basic, 6, s.num(sec, "RIM_OFFSET").unwrap_or(0.0));
    g.set(1, Val::Msg(basic));
    match kind.as_str() {
        "STRUT" => match points(s, sec, &STRUT) {
            Some(p) => g.set(3, Val::Msg(p)),
            None => c.note(format!("physics: [{sec}] STRUT without all its points")),
        },
        "AXLE" => {
            let mut a = Msg::default();
            let n = s.num("AXLE", "LINK_COUNT").unwrap_or(4.0) as usize;
            f(&mut a, 1, s.num("AXLE", "ATTACH_REL_POS").unwrap_or(0.5));
            let mut links = 0;
            for k in 0..n {
                if let (Some(cs), Some(ax)) = (ac1_v3(s, "AXLE", &format!("J{k}_CAR")), ac1_v3(s, "AXLE", &format!("J{k}_AXLE"))) {
                    a.push(3, pbx::v3(cs));
                    a.push(4, pbx::v3(ax));
                    links += 1;
                }
            }
            i(&mut a, 2, links);
            f(&mut a, 5, hub);
            f(&mut a, 6, s.num("AXLE", "LEAF_SPRING_LAT_K").unwrap_or(0.0));
            f(&mut a, 7, s.num("AXLE", "TORQUE_REACTION").unwrap_or(0.0));
            a.fields.sort_by_key(|x| x.num);
            g.set(4, Val::Msg(a));
        }
        "ML" => {
            let mut m = Msg::default();
            for k in 0..5 {
                if let (Some(cs), Some(ty)) = (ac1_v3(s, sec, &format!("JOINT{k}_CAR")), ac1_v3(s, sec, &format!("JOINT{k}_TYRE"))) {
                    m.push(1, pbx::v3(cs));
                    m.push(2, pbx::v3(ty));
                }
            }
            m.fields.sort_by_key(|x| x.num);
            if m.fields.len() >= 10 {
                g.set(5, Val::Msg(m));
            } else if let Some(p) = points(s, sec, &DWB) {
                c.note(format!("physics: [{sec}] ML joints incomplete; its double-wishbone points are used"));
                g.set(2, Val::Msg(p));
            } else {
                c.note(format!("physics: [{sec}] ML joints incomplete"));
            }
        }
        _ => match points(s, sec, &DWB) {
            Some(p) => g.set(2, Val::Msg(p)),
            None => c.note(format!("physics: [{sec}] {kind} without all its points")),
        },
    }
    g
}

pub fn write(c: &mut Ctx) -> Result<()> {
    let s = c.ini("suspensions.ini").clone();
    let ci = c.ini("car.ini").clone();
    let m = c.d.mass_static;
    let share = [c.d.cg_front, 1.0 - c.d.cg_front];
    let cog = c.frame.cog;
    let mut coil: Vec<Msg> = Vec::new();
    let mut geos: Vec<Msg> = Vec::new();
    let mut base_y = [0.0; 2];
    let mut track = [0.0; 2];
    for (a, sec) in ["FRONT", "REAR"].iter().enumerate() {
        let k = s.num(sec, "SPRING_RATE").filter(|v| *v > 1000.0).unwrap_or(40000.0);
        let hub = s.num(sec, "HUB_MASS").unwrap_or(40.0);
        let load = m * G * share[a] / 2.0;
        let sag = (load - hub * G).max(load * 0.5) / k;
        let rod = s.num(sec, "ROD_LENGTH").unwrap_or(0.0);
        let x_ac1 = sag - rod;
        let by = s.num(sec, "BASEY").unwrap_or(-0.2);
        base_y[a] = by;
        track[a] = s.num(sec, "TRACK").or(c.frame.track_width(a == 0)).unwrap_or(1.5);
        // where the AC1 model draws the wheel, from the CoG
        let cs = if a == 0 { ["LF", "RF"] } else { ["LR", "RR"] };
        let wy: Vec<f64> = cs.iter().filter_map(|k| c.frame.wheel_centres.get(*k)).map(|p| p[1]).collect();
        let w_vis = if wy.is_empty() { by } else { wy.iter().sum::<f64>() / wy.len() as f64 } - cog[1];
        let tdef = load / c.d.tyre_rate[a].max(1e5);
        let x = w_vis - by - tdef;
        let collar = sag - x;
        let up = (s.num(sec, "BUMPSTOP_UP").unwrap_or(0.08) - x_ac1).clamp(0.01, BUMP_MAX).max(x.abs() + 0.015);
        let dn = (s.num(sec, "BUMPSTOP_DN").unwrap_or(0.08) + x_ac1).clamp(0.01, DROOP_MAX).max(x.abs() + 0.015);
        c.d.spring[a] = k;
        c.d.sag[a] = sag;
        c.d.rest_x[a] = x;
        c.d.collar[a] = collar;
        c.d.bump_up[a] = up;
        c.d.bump_dn[a] = dn;
        let pickup = ci.num("RIDE", if a == 0 { "PICKUP_FRONT_HEIGHT" } else { "PICKUP_REAR_HEIGHT" }).unwrap_or(-0.3);
        c.d.target_height_mm[a] = ((cog[1] + pickup) * 1000.0).round();
        // CoiloverData
        let mut co = c.cd.sub(2).and_then(|x| x.sub(if a == 0 { 8 } else { 9 })).unwrap_or_default();
        f(&mut co, 1, k);
        pbx::fo(&mut co, 2, s.num(sec, "PROGRESSIVE_SPRING_RATE").filter(|v| *v > 0.0));
        let stop_rate = s.num(sec, "BUMP_STOP_RATE").filter(|v| *v > 0.0).unwrap_or(50000.0);
        for (num, range, damping) in [(3u32, up, BUMP_DAMPING_UP), (4u32, dn, BUMP_DAMPING_DOWN)] {
            // damping only where the template's stop has one (stock cars omit it on some stops)
            let template_damped = co.sub(num).map(|b| b.get(6).is_some()).unwrap_or(false);
            let b = co.msg_mut(num)?;
            f(b, 1, range);
            f(b, 2, 0.01);
            f(b, 3, stop_rate * 0.01);
            f(b, 4, 1.5);
            f(b, 5, 0.02);
            pbx::fo(b, 6, template_damped.then_some(damping));
        }
        f(&mut co, 5, collar);
        {
            let d = co.msg_mut(6)?;
            let g = |key: &str, dflt: f64| s.num(sec, key).filter(|v| *v > 0.0).unwrap_or(dflt);
            let slow_b = g("DAMP_BUMP", 3000.0);
            let slow_r = g("DAMP_REBOUND", 6000.0);
            {
                let fa = d.msg_mut(1)?;
                f(fa, 1, g("DAMP_FAST_BUMP", slow_b * 0.5));
                f(fa, 2, g("DAMP_FAST_REBOUND", slow_r * 0.5));
            }
            {
                let sl = d.msg_mut(2)?;
                f(sl, 1, slow_b);
                f(sl, 2, slow_r);
            }
            f(d, 3, g("DAMP_FAST_BUMPTHRESHOLD", 0.1));
            f(d, 4, g("DAMP_FAST_REBOUNDTHRESHOLD", 0.1));
            d.remove(13);
            d.remove(14);
        }
        co.remove(7);
        co.remove(8);
        co.fields.sort_by_key(|x| x.num);
        coil.push(co);
        geos.push(geometry(c, &s, sec));
        c.note(format!(
            "physics: {sec} {} k {k:.0} N/m, sag {:.3} m, rest x {:+.3} m, collar {collar:.3} m, bump {up:.3} / droop {dn:.3} m, ride target {:.0} mm",
            s.get(sec, "TYPE").unwrap_or("DWB").trim(), sag, x, c.d.target_height_mm[a]
        ));
    }
    c.d.track = track;
    let arb = [s.num("ARB", "FRONT").unwrap_or(0.0), s.num("ARB", "REAR").unwrap_or(0.0)];
    let sp = at(&mut c.cd, &[2])?;
    f(sp, 2, c.d.wheelbase);
    f(sp, 3, c.d.cg_front);
    f(sp, 50, base_y[0]);
    f(sp, 51, base_y[1]);
    f(sp, 52, track[0]);
    f(sp, 53, track[1]);
    sp.set(8, Val::Msg(coil[0].clone()));
    sp.set(9, Val::Msg(coil[1].clone()));
    sp.set(40, Val::Msg(geos[0].clone()));
    sp.set(41, Val::Msg(geos[1].clone()));
    sp.remove(17);
    for (num, v) in [(18u32, arb[0]), (19, arb[1])] {
        let a = sp.msg_mut(num)?;
        f(a, 1, v.max(0.0));
        a.set(2, Val::Msg(Msg::default()));
    }
    // flex bars (twist beams) are the donor's: AC1 has none
    for num in [24u32, 25] {
        let mut fb = Msg::default();
        fb.set(2, Val::Msg(Msg::default()));
        sp.set(num, Val::Msg(fb));
    }
    sp.remove(60);
    sp.remove(61);
    Ok(())
}
