//! CarData general block, colliders, body offset, brakes, steering and the
//! electronics (ABS / TC / EDL / ESP).
//!
//! Field meanings checked on stock twins: `general.totalMass`, `fuel`,
//! `maxFuel`, `tankPosition` (from the CoG), `pickupFront/RearHeight` and
//! the box colliders are car.ini / colliders.ini values unchanged on
//! ks_mazda_mx5_na; `bodyMeshOffset.position` is AC1's GRAPHICS_OFFSET
//! (mx5_na: (0, -0.5, -0.08) against AC1 (0, -0.47, -0.075)), i.e. the
//! mesh origin relative to the physics CoG. Brakes: `frontBias` is
//! FRONT_SHARE in percent (supra_mkiv 0.695 -> 69.5) and `totalTorque` is
//! MAX_TORQUE (supra 2800 -> 2800). Controls are car.ini [CONTROLS].

use anyhow::Result;

use super::pbx::{self, at, f};
use super::{ac1_v3, Ctx, KG_PER_LITRE};
use crate::io::pb::{Msg, Val};

/// Stock ranges (71 cars) the converter clamps to; outside them a note says so.
const BOX_X: (f64, f64) = (1.5, 2.06);
const BOX_Y: (f64, f64) = (0.55, 1.46);
const BOX_Z: (f64, f64) = (3.0, 5.0);
const FF_MULT: (f64, f64) = (0.7, 5.5);

pub fn clamp_note(c: &mut Ctx, what: &str, v: f64, r: (f64, f64)) -> f64 {
    let o = v.clamp(r.0, r.1);
    if (o - v).abs() > 1e-9 {
        c.note(format!("physics: {what} {v:.4} is outside the stock range {:.4} .. {:.4}; clamped", r.0, r.1));
    }
    o
}

pub fn general(c: &mut Ctx) -> Result<()> {
    let ci = c.ini("car.ini").clone();
    let mass = ci.num("BASIC", "TOTALMASS").filter(|m| *m > 100.0).unwrap_or(1200.0);
    let fuel = ci.num("FUEL", "FUEL").filter(|v| *v > 0.0).unwrap_or(30.0);
    let max_fuel = ci.num("FUEL", "MAX_FUEL").filter(|v| *v > 0.0).unwrap_or(fuel.max(50.0));
    let cg = c.ini("suspensions.ini").num("BASIC", "CG_LOCATION").filter(|v| *v > 0.05 && *v < 0.95).unwrap_or(0.5);
    let wb = c.ini("suspensions.ini").num("BASIC", "WHEELBASE").or(c.frame.wheelbase()).unwrap_or(2.5);
    c.d.mass = mass;
    c.d.fuel = fuel.min(max_fuel);
    c.d.max_fuel = max_fuel;
    c.d.mass_static = mass + c.d.fuel * KG_PER_LITRE;
    c.d.cg_front = cg;
    c.d.wheelbase = wb;
    let inertia = ac1_v3(&ci, "BASIC", "INERTIA").unwrap_or([1.8, 1.2, 4.2]);
    let bx = [clamp_note(c, "body box x (car.ini INERTIA)", inertia[0], BOX_X), clamp_note(c, "body box y", inertia[1], BOX_Y), clamp_note(c, "body box z", inertia[2], BOX_Z)];
    let tank = ac1_v3(&ci, "FUELTANK", "POSITION").unwrap_or([0.0, -0.1, -1.0]);
    let cog = c.frame.cog;
    let g = at(&mut c.cd, &[1])?;
    f(g, 1, mass);
    pbx::set_v3(g, 4, tank);
    f(g, 5, c.d.fuel);
    f(g, 6, max_fuel);
    f(g, 8, KG_PER_LITRE);
    pbx::set_v3(g, 9, bx);
    f(g, 10, ci.num("RIDE", "PICKUP_FRONT_HEIGHT").unwrap_or(-0.3));
    f(g, 11, ci.num("RIDE", "PICKUP_REAR_HEIGHT").unwrap_or(-0.3));
    pbx::fo(g, 13, ci.num("RULES", "MIN_HEIGHT").filter(|v| *v > 0.0));
    // the mesh origin (on the ground, between the axles) relative to the CoG
    let bmo = at(&mut c.cd, &[18])?;
    pbx::set_v3(bmo, 1, [-cog[0], -cog[1], -cog[2]]);
    pbx::set_v3(bmo, 2, [0.0, 0.0, 0.0]);
    pbx::set_v3(bmo, 3, [1.0, 1.0, 1.0]);
    // box colliders (from the CoG): colliders.ini, else one floor plate
    let col = c.ini("colliders.ini").clone();
    let mut boxes: Vec<([f64; 3], [f64; 3])> = col.with_prefix("COLLIDER_").filter_map(|(s, _)| Some((ac1_v3(&col, s, "CENTRE")?, ac1_v3(&col, s, "SIZE")?))).collect();
    if boxes.is_empty() {
        let tr = c.frame.track_width(true).unwrap_or(1.5);
        boxes.push(([0.0, 0.15 - cog[1], 0.0], [tr * 0.9, 0.1, wb * 0.95]));
        c.note("physics: no colliders.ini - one floor plate collider from the track and wheelbase");
    }
    c.cd.remove(9);
    for (centre, size) in boxes {
        let mut b = Msg::default();
        b.set(1, pbx::v3(centre));
        b.set(2, pbx::v3(size));
        c.cd.push(9, Val::Msg(b));
    }
    // no tuning-part performance modes (stock road cars carry none)
    c.cd.remove(30);
    Ok(())
}

pub fn brakes_steering(c: &mut Ctx) -> Result<()> {
    let bi = c.ini("brakes.ini").clone();
    let torque = bi.num("DATA", "MAX_TORQUE").filter(|v| *v > 0.0).unwrap_or(2000.0);
    let share = bi.num("DATA", "FRONT_SHARE").filter(|v| *v > 0.0 && *v < 1.0).unwrap_or(0.65);
    c.d.brake_torque = torque;
    c.d.front_bias = share * 100.0;
    let b = at(&mut c.cd, &[5])?;
    f(b, 1, torque);
    f(b, 2, share * 100.0);
    f(b, 3, bi.num("DATA", "HANDBRAKE_TORQUE").unwrap_or(torque * 0.4));
    pbx::b(b, 4, bi.num("DATA", "COCKPIT_ADJUSTABLE").unwrap_or(0.0) > 0.0);
    f(b, 5, bi.num("DATA", "ADJUST_STEP").filter(|v| *v > 0.0).unwrap_or(0.5));
    // AC1 has no electronic brake balance: empty controllers, EBB off
    for n in [8u32, 9, 14] {
        b.set(n, Val::Msg(Msg::default()));
    }
    b.remove(16);
    b.remove(10);
    b.remove(11);
    b.remove(15);
    let ci = c.ini("car.ini").clone();
    let lock = ci.num("CONTROLS", "STEER_LOCK").filter(|v| *v > 30.0).unwrap_or(450.0);
    let ratio = ci.num("CONTROLS", "STEER_RATIO").filter(|v| v.abs() > 1.0).unwrap_or(14.0);
    c.d.steer_lock = lock;
    c.d.steer_ratio = ratio;
    let ff = clamp_note(c, "FFMULT", ci.num("CONTROLS", "FFMULT").unwrap_or(2.0), FF_MULT);
    let k = at(&mut c.cd, &[8])?;
    f(k, 1, ff);
    f(k, 2, lock);
    f(k, 3, ratio);
    f(k, 4, ci.num("CONTROLS", "LINEAR_STEER_ROD_RATIO").unwrap_or(0.002));
    f(k, 5, ci.num("CONTROLS", "STEER_ASSIST").unwrap_or(1.0));
    Ok(())
}

/// Levels of an AC1 electronics LUT (`CURVE=abs.lut`: index -> slip ratio),
/// else the single SLIP_RATIO_LIMIT.
fn levels(c: &Ctx, sec: &str, ini: &crate::ac1::ini::Ini) -> Vec<f64> {
    let l: Vec<f64> = ini.get(sec, "CURVE").filter(|v| !v.trim().is_empty()).map(|v| c.lut(v)).unwrap_or_default().into_iter().map(|(_, y)| y).filter(|y| *y > 0.0 && *y < 1.0).collect();
    if !l.is_empty() {
        return l;
    }
    vec![ini.num(sec, "SLIP_RATIO_LIMIT").filter(|v| *v > 0.0 && *v < 1.0).unwrap_or(0.12)]
}

pub fn electronics(c: &mut Ctx) -> Result<()> {
    let ei = c.ini("electronics.ini").clone();
    let on = |s: &str, k: &str| ei.num(s, k).unwrap_or(0.0) > 0.0;
    c.d.has_abs = on("ABS", "PRESENT");
    c.d.abs_on = c.d.has_abs && on("ABS", "ACTIVE");
    c.d.has_tc = on("TRACTION_CONTROL", "PRESENT");
    c.d.tc_on = c.d.has_tc && on("TRACTION_CONTROL", "ACTIVE");
    let abs_levels = levels(c, "ABS", &ei);
    let tc_levels = levels(c, "TRACTION_CONTROL", &ei);
    c.d.tc_levels = if c.d.has_tc { tc_levels.len() } else { 0 };
    let e = at(&mut c.cd, &[7])?;
    // ABS (f2): stock cars keep the block as the game's assist even when
    // the real car has none; the setup decides whether it is available
    {
        let a = e.msg_mut(2)?;
        let tmpl = a.subs(1).get(1).cloned().unwrap_or_default();
        a.remove(1);
        let mut off = Msg::default();
        f(&mut off, 1, -1.0);
        f(&mut off, 2, -1.0);
        f(&mut off, 5, 0.7);
        a.push(1, Val::Msg(off));
        for s in &abs_levels {
            let mut m = tmpl.clone();
            f(&mut m, 1, s * 0.75);
            f(&mut m, 2, s * 1.25);
            if m.get(4).is_none() {
                f(&mut m, 4, 0.35);
            }
            if m.get(5).is_none() {
                f(&mut m, 5, 0.8);
            }
            a.push(1, Val::Msg(m));
        }
        a.fields.sort_by_key(|x| x.num);
        f(a, 2, ei.num("ABS", "RATE_HZ").filter(|v| *v > 1.0).unwrap_or(40.0));
        pbx::i(a, 3, 4);
    }
    // TC (f1): only when the AC1 car has it (stock road cars without TC carry no block)
    if c.d.has_tc {
        let t = e.msg_mut(1)?;
        let tmpl = t.subs(7).get(1).cloned().unwrap_or_default();
        t.remove(7);
        let mut off = Msg::default();
        f(&mut off, 1, -1.0);
        f(&mut off, 2, -1.0);
        t.push(7, Val::Msg(off));
        for s in &tc_levels {
            let mut m = tmpl.clone();
            f(&mut m, 1, *s);
            f(&mut m, 2, s + 0.05);
            t.push(7, Val::Msg(m));
        }
        t.fields.sort_by_key(|x| x.num);
        f(t, 2, ei.num("TRACTION_CONTROL", "RATE_HZ").filter(|v| *v > 1.0).unwrap_or(150.0));
        f(t, 3, ei.num("TRACTION_CONTROL", "MIN_SPEED_KMH").unwrap_or(20.0));
    } else {
        e.remove(1);
    }
    // EDL (f3): AC1 [EDL] 1:1
    if ei.num("EDL", "PRESENT").unwrap_or(0.0) > 0.0 {
        let d = e.msg_mut(3)?;
        pbx::b(d, 1, true);
        let g = |k: &str, dflt: f64| ei.num("EDL", k).unwrap_or(dflt);
        f(d, 2, g("BRAKE_TORQUE_POWER", 200.0));
        f(d, 3, g("BRAKE_TORQUE_COAST", 200.0));
        f(d, 4, g("DEAD_ZONE_COAST", 0.05));
        f(d, 5, g("DEAD_ZONE_POWER", 0.1));
        f(d, 6, g("MAX_SPIN_POWER", 0.2));
        f(d, 7, g("MAX_SPIN_COAST", 0.3));
        f(d, 8, g("MIN_SPEED_KMH", 20.0));
    } else {
        e.remove(3);
    }
    // ESP (f4): an empty block, like stock road cars without stability control
    e.set(4, Val::Msg(Msg::default()));
    c.note(format!("physics: electronics ABS {}{} TC {}{}", if c.d.has_abs { "yes" } else { "no (assist only)" }, if c.d.abs_on { ", on" } else { "" }, if c.d.has_tc { format!("{} levels", c.d.tc_levels) } else { "no".into() }, if c.d.tc_on { ", on" } else { "" }));
    Ok(())
}
