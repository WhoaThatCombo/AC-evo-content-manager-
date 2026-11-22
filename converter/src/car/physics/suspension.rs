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
use super::{Ctx, G};
use crate::io::pb::{Msg, Val};
use crate::source::{Axle, Geometry};

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

fn pts_msg(p: &[[f64; 3]]) -> Msg {
    let mut m = Msg::default();
    for (k, v) in p.iter().enumerate() {
        m.set(k as u32 + 1, pbx::v3(*v));
    }
    m
}

/// The geometry message of one axle.
fn geometry(c: &mut Ctx, a: &Axle, sec: &str) -> Msg {
    let mut g = Msg::default();
    let mut basic = Msg::default();
    let is_axle = matches!(a.geometry, Geometry::Axle { .. });
    f(&mut basic, 3, if is_axle { (a.hub_mass * 0.3).max(20.0) } else { a.hub_mass });
    f(&mut basic, 4, a.toe_out);
    f(&mut basic, 5, a.static_camber_deg);
    f(&mut basic, 6, a.rim_offset);
    g.set(1, Val::Msg(basic));
    match &a.geometry {
        Geometry::DoubleWishbone(p) => g.set(2, Val::Msg(pts_msg(p))),
        Geometry::Strut(p) => g.set(3, Val::Msg(pts_msg(p))),
        Geometry::Axle { links, attach, leaf_lat_k, torque_reaction } => {
            let mut m = Msg::default();
            f(&mut m, 1, *attach);
            for (cs, ax) in links {
                m.push(3, pbx::v3(*cs));
                m.push(4, pbx::v3(*ax));
            }
            i(&mut m, 2, links.len() as i64);
            f(&mut m, 5, a.hub_mass);
            f(&mut m, 6, *leaf_lat_k);
            f(&mut m, 7, *torque_reaction);
            m.fields.sort_by_key(|x| x.num);
            g.set(4, Val::Msg(m));
        }
        Geometry::MultiLink(j) => {
            let mut m = Msg::default();
            for (cs, ty) in j {
                m.push(1, pbx::v3(*cs));
                m.push(2, pbx::v3(*ty));
            }
            m.fields.sort_by_key(|x| x.num);
            g.set(5, Val::Msg(m));
        }
        Geometry::Unknown => c.note(format!("physics: [{sec}] {} without all its points", a.kind)),
    }
    g
}

pub fn write(c: &mut Ctx) -> Result<()> {
    let ci = c.ini("car.ini").clone();
    let m = c.d.mass_static;
    let share = [c.d.cg_front, 1.0 - c.d.cg_front];
    let cog = c.frame.cog;
    let axles = c.src.axles.clone();
    let mut coil: Vec<Msg> = Vec::new();
    let mut geos: Vec<Msg> = Vec::new();
    let mut base_y = [0.0; 2];
    let mut track = [0.0; 2];
    for (a, sec) in ["FRONT", "REAR"].iter().enumerate() {
        let ax = &axles[a];
        let k = ax.spring;
        let load = m * G * share[a] / 2.0;
        let sag = (load - ax.hub_mass * G).max(load * 0.5) / k;
        let x_ac1 = sag - ax.rod_length;
        let by = ax.base_y;
        base_y[a] = by;
        track[a] = ax.track;
        // where the source model draws the wheel, from the CoG
        let cs = if a == 0 { ["LF", "RF"] } else { ["LR", "RR"] };
        let wy: Vec<f64> = cs.iter().filter_map(|k| c.frame.wheel_centres.get(*k)).map(|p| p[1]).collect();
        let w_vis = if wy.is_empty() { by } else { wy.iter().sum::<f64>() / wy.len() as f64 } - cog[1];
        let tdef = load / c.d.tyre_rate[a].max(1e5);
        let x = w_vis - by - tdef;
        let collar = sag - x;
        let up = (ax.bump_stop_up - x_ac1).clamp(0.01, BUMP_MAX).max(x.abs() + 0.015);
        let dn = (ax.bump_stop_down + x_ac1).clamp(0.01, DROOP_MAX).max(x.abs() + 0.015);
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
        pbx::fo(&mut co, 2, ax.progressive);
        for (num, range, damping) in [(3u32, up, BUMP_DAMPING_UP), (4u32, dn, BUMP_DAMPING_DOWN)] {
            // damping only where the template's stop has one (stock cars omit it on some stops)
            let template_damped = co.sub(num).map(|b| b.get(6).is_some()).unwrap_or(false);
            let b = co.msg_mut(num)?;
            f(b, 1, range);
            f(b, 2, 0.01);
            f(b, 3, ax.bump_stop_rate * 0.01);
            f(b, 4, 1.5);
            f(b, 5, 0.02);
            pbx::fo(b, 6, template_damped.then_some(damping));
        }
        f(&mut co, 5, collar);
        {
            let d = co.msg_mut(6)?;
            let dm = &ax.dampers;
            {
                let fa = d.msg_mut(1)?;
                f(fa, 1, dm.fast_bump);
                f(fa, 2, dm.fast_rebound);
            }
            {
                let sl = d.msg_mut(2)?;
                f(sl, 1, dm.bump);
                f(sl, 2, dm.rebound);
            }
            f(d, 3, dm.fast_bump_threshold);
            f(d, 4, dm.fast_rebound_threshold);
            d.remove(13);
            d.remove(14);
        }
        co.remove(7);
        co.remove(8);
        co.fields.sort_by_key(|x| x.num);
        coil.push(co);
        geos.push(geometry(c, ax, sec));
        c.note(format!(
            "physics: {sec} {} k {k:.0} N/m, sag {:.3} m, rest x {:+.3} m, collar {collar:.3} m, bump {up:.3} / droop {dn:.3} m, ride target {:.0} mm",
            ax.kind, sag, x, c.d.target_height_mm[a]
        ));
    }
    c.d.track = track;
    let arb = [axles[0].arb, axles[1].arb];
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
    // flex bars (twist beams) are the donor's: the source has none
    for num in [24u32, 25] {
        let mut fb = Msg::default();
        fb.set(2, Val::Msg(Msg::default()));
        sp.set(num, Val::Msg(fb));
    }
    sp.remove(60);
    sp.remove(61);
    Ok(())
}
