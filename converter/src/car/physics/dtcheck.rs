//! Drivetrain read-back and checks (CarData f3, DrivetrainData).
//!
//! `summary` decodes a CarData's drivetrain the way the game wires it
//! (drivetrain.rs): the traction type, the driven wheels, every differential
//! and the AWD clutches. `check` is what check-car enforces; it passes on
//! every stock car (`check-car-stock`).

use crate::io::pb::Msg;
use serde_json::{json, Value};

use super::drivetrain::{evo_traction, TRACTION_NAMES};
use super::pbx::rf;

const DIFF_TYPES: [&str; 6] = ["LSD", "Spool", "Torsen", "EpicyclicTorsen", "EpicyclicLSD", "TorqueVectoring"];

fn diff_json(m: Option<Msg>) -> Value {
    let Some(m) = m else { return Value::Null };
    if m.fields.is_empty() {
        return Value::Null;
    }
    let ty = m.varint(4).unwrap_or(0) as usize;
    json!({
        "type": DIFF_TYPES.get(ty).copied().unwrap_or("?"),
        "power": rf(&m, 1), "coast": rf(&m, 2), "preload": rf(&m, 3), "front_share": rf(&m, 13),
        "tbr_power": rf(&m, 11), "tbr_coast": rf(&m, 12),
    })
}

/// Clutch positions (0 LF, 1 RF, 2 LR, 3 RR) of `awd_clutches`.
pub fn clutch_positions(dt: &Msg) -> Vec<i64> {
    dt.subs(45).iter().map(|c| c.varint(1).unwrap_or(0) as i64).collect()
}

/// The wheels the engine drives: LF, RF, LR, RR.
pub fn driven(tt: u64, clutches: &[i64]) -> [bool; 4] {
    match tt {
        0 => [false, false, true, true],
        1 => [true, true, false, false],
        2 => [true; 4],
        3 => [clutches.contains(&0), clutches.contains(&1), true, true],
        4 => [true, true, clutches.contains(&2), clutches.contains(&3)],
        _ => [false; 4],
    }
}

pub fn summary(cd: &Msg) -> Value {
    let dt = cd.sub(3).unwrap_or_default();
    let tt = dt.varint(2).unwrap_or(0);
    let w = dt.sub(15).unwrap_or_default();
    let cl = clutch_positions(&dt);
    let ctl = |n: u32| dt.sub(n).map(|m| m.subs(2).len()).unwrap_or(0);
    let clutches: Vec<Value> = dt.subs(45).iter().map(|c| json!({
        "position": c.varint(1).unwrap_or(0), "preload": rf(c, 2),
        "stages": c.sub(3).map(|m| m.subs(2).len()).unwrap_or(0),
    })).collect();
    json!({
        "traction": TRACTION_NAMES.get(tt as usize).copied().unwrap_or("?"),
        "traction_enum": tt,
        "driven": driven(tt, &cl),
        "diff": diff_json(dt.sub(39)),
        "front": diff_json(w.sub(1)), "rear": diff_json(w.sub(2)), "centre": diff_json(w.sub(3)),
        "clutches": clutches,
        "lock_controller_stages": {"rear": ctl(5), "front_share": ctl(6), "centre": ctl(7), "front": ctl(8)},
    })
}

/// (errors, warnings). `ac1` is the AC1 [TRACTION] TYPE the car was
/// converted from, when known (the `<id>.json` `aceconv_ac1_traction`).
pub fn check(cd: &Msg, ac1: Option<&str>) -> (Vec<String>, Vec<String>) {
    let (mut err, mut warn) = (Vec::new(), Vec::new());
    let Some(dt) = cd.sub(3) else {
        err.push("drivetrain: CarData has no drivetrain (f3)".into());
        return (err, warn);
    };
    let tt = dt.varint(2).unwrap_or(0);
    if tt > 4 {
        err.push(format!("drivetrain: tractionType {tt} is not a TractionType (0..4)"));
        return (err, warn);
    }
    let name = TRACTION_NAMES[tt as usize];
    let w = dt.sub(15).unwrap_or_default();
    let cl = clutch_positions(&dt);
    let nonempty = |m: Option<Msg>| m.map(|m| !m.fields.is_empty()).unwrap_or(false);
    match tt {
        2 => {
            match w.sub(3).filter(|m| !m.fields.is_empty()) {
                None => err.push("drivetrain: FourWD without a centre differential (FourWD_differentials.centerDiff)".into()),
                Some(c) => {
                    let ty = c.varint(4).unwrap_or(0);
                    if ![1, 3, 4].contains(&ty) {
                        warn.push(format!("drivetrain: FourWD centre is {} (only the epicyclic types honour front_share)", DIFF_TYPES.get(ty as usize).unwrap_or(&"?")));
                    }
                    match rf(&c, 13) {
                        Some(s) if (0.0..=1.0).contains(&s) => {}
                        Some(s) => err.push(format!("drivetrain: centre front_share {s} outside 0..1")),
                        None if ty != 1 => err.push("drivetrain: FourWD centre has no front_share".into()),
                        None => {}
                    }
                }
            }
        }
        3 | 4 => {
            let need: [i64; 2] = if tt == 3 { [0, 1] } else { [2, 3] };
            let axle = if tt == 3 { "front" } else { "rear" };
            if !need.iter().all(|p| cl.contains(p)) {
                err.push(format!("drivetrain: {name} needs awd_clutches at positions {} / {} (the {axle} wheels); has {cl:?} - the {axle} axle is never driven", need[0], need[1]));
            }
            for c in dt.subs(45) {
                let has_ctl = c.sub(3).map(|m| !m.subs(2).is_empty()).unwrap_or(false);
                if !has_ctl && rf(&c, 2).unwrap_or(0.0) <= 0.0 {
                    warn.push(format!("drivetrain: AWD clutch {} has neither a controller nor a preload (it never locks)", c.varint(1).unwrap_or(0)));
                }
            }
        }
        _ => {
            if !cl.is_empty() {
                warn.push(format!("drivetrain: {name} car carries awd_clutches {cl:?} the game ignores"));
            }
            if nonempty(w.sub(3)) && rf(&w.sub(3).unwrap(), 13).is_some() && !nonempty(dt.sub(39)) {
                warn.push(format!("drivetrain: {name} car has a centre split but no differential_data"));
            }
        }
    }
    for c in clutch_positions(&dt) {
        if !(0..=3).contains(&c) {
            err.push(format!("drivetrain: AWD clutch position {c} is not a wheel (0..3)"));
        }
    }
    if let Some(a) = ac1 {
        let want = evo_traction(&a.to_uppercase());
        if want as u64 != tt {
            err.push(format!("drivetrain: the AC1 car is {a} (-> {}), the package says {name}", TRACTION_NAMES[want as usize]));
        }
    }
    (err, warn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::pb::Val;

    fn car(tt: u64, clutches: &[i64]) -> Msg {
        let mut dt = Msg::default();
        if tt > 0 {
            dt.set_varint(2, tt);
        }
        for p in clutches {
            let mut c = Msg::default();
            c.set_varint(1, *p as u64);
            c.set_f32(2, 100.0);
            dt.push(45, Val::Msg(c));
        }
        let mut cd = Msg::default();
        cd.set(3, Val::Msg(dt));
        cd
    }

    #[test]
    fn traction_rules() {
        assert!(check(&car(0, &[]), Some("RWD")).0.is_empty());
        assert!(!check(&car(0, &[]), Some("FWD")).0.is_empty());
        assert!(check(&car(3, &[0, 1]), Some("AWD2")).0.is_empty());
        assert!(!check(&car(3, &[]), Some("AWD2")).0.is_empty());
        assert!(!check(&car(2, &[]), Some("AWD")).0.is_empty(), "FourWD with no centre");
        assert_eq!(driven(3, &[0, 1]), [true; 4]);
        assert_eq!(driven(1, &[]), [true, true, false, false]);
    }
}
