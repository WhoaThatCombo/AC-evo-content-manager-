//! Shift lights and dash LEDs (requirements car-lights-leds-display.md).
//!
//! # What the game wants (stock cars)
//!
//! - An LED is a VehicleLed material whose `LedMask` red channel picks the
//!   LED: stock masks hold 0, 10, 20, ... (of 255) for Led_01, Led_02, ...
//!   and 255 for "no LED" (common_assets led_1 `int_leds_f`, mx5_na
//!   `int_decals_f`); the LED emits its emissive map times the LED's value
//!   and colour from the `.carledsystem`.
//! - `.carledsystem` (CarLedSystemData): rule groups (rpm f1, engine limiter
//!   f3, fuel f4, light stages f19, ...) of instances {led, physics value,
//!   on / off time, colour, value}; `engine_limiter_rpm` f33; every one of
//!   the 69 stock files ends with `intensity_filters` f35 and a fixed32 f33
//!   (a constant 2.0), kept as they are.
//!
//! # From AC1 (`digital_instruments.ini`)
//!
//! - `[LED_n]` OBJECT_NAME / RPM_SWITCH / EMISSIVE / BLINK_SWITCH / BLINK_HZ;
//! - `[RPM_SERIE_n]` PREFIX + START_INDEX..END_INDEX meshes spread over
//!   RPM_START..RPM_END;
//! - `[FUEL_WARNING_n]` OBJECT_NAME / FUEL_SWITCH (litres; on below it).
//!
//! The limiter field is the car's own (engine.ini LIMITER), no shift light
//! is set past it, and a car without AC1 LEDs keeps none of the donor's
//! rules (every group emptied).

use std::collections::{BTreeMap, HashMap};

use anyhow::Result;

use crate::ac1::car::Car;
use crate::car::body::Plan;
use crate::car::fx;
use crate::car::geometry::Inst;
use crate::car::materials::{Hints, Kind};
use crate::files::Files;
use crate::io::material::{get_slot, set_prop, set_slot};
use crate::io::pb::{Field, Msg, Val};

#[derive(Clone, Debug)]
pub struct Led {
    /// 0-based LedLight index
    pub index: usize,
    pub node: String,
    pub what: LedKind,
    pub colour: [f32; 3],
}

#[derive(Clone, Debug)]
pub enum LedKind {
    /// on above rpm; Some(hz) = blinks at the limiter
    Rpm(f32, Option<f32>),
    /// on below litres
    Fuel(f32),
}

#[derive(Default)]
pub struct LedPlan {
    pub leds: Vec<Led>,
    pub limiter: f32,
    /// the donor's VehicleLed material, read before the body overwrites it
    pub template: Option<Msg>,
    pub notes: Vec<String>,
}

/// AC1 shift-light colours by position along a series (ours: green,
/// yellow, red thirds, like most AC1 and EVO shift lights).
fn serie_colour(i: usize, n: usize) -> [f32; 3] {
    let t = i as f32 / n.max(1) as f32;
    if t < 0.4 { [0.0, 1.0, 0.0] } else if t < 0.75 { [1.0, 0.8, 0.0] } else { [1.0, 0.0, 0.0] }
}

pub fn prepare(car: &Car, insts: &[Inst], files: &Files, id: &str, hints: &mut Hints) -> LedPlan {
    let mut p = LedPlan::default();
    p.limiter = car.ini("engine.ini").num("ENGINE_DATA", "LIMITER").unwrap_or(7000.0) as f32;
    p.template = files.get(&format!("content\\cars\\{id}\\materials\\INT_LED.material")).and_then(|b| Msg::parse(b).ok());
    let nodes: HashMap<String, bool> = insts.iter().map(|i| (i.node.to_lowercase(), i.cockpit)).collect();
    let di = car.ini("digital_instruments.ini");
    let mut want: Vec<(String, LedKind, [f32; 3])> = Vec::new();
    for (sec, kv) in di.sections.iter() {
        if sec.starts_with("LED_") {
            let Some(n) = kv.get("OBJECT_NAME") else { continue };
            let rpm = kv.get("RPM_SWITCH").and_then(|v| crate::ac1::ini::num_of(v)).unwrap_or(0.0) as f32;
            if rpm <= 0.0 {
                continue;
            }
            let blink = kv.get("BLINK_SWITCH").and_then(|v| crate::ac1::ini::num_of(v)).map(|v| v as f32);
            let hz = kv.get("BLINK_HZ").and_then(|v| crate::ac1::ini::num_of(v)).map(|v| v as f32).unwrap_or(8.0);
            let col = fx::hue(fx::rgb(kv.get("EMISSIVE")));
            want.push((n.to_lowercase(), LedKind::Rpm(rpm, blink.filter(|b| *b > 0.0).map(|_| hz)), if col == [0.0; 3] { [1.0, 0.0, 0.0] } else { col }));
        } else if sec.starts_with("RPM_SERIE_") {
            let prefix = kv.get("PREFIX").cloned().unwrap_or_default();
            let a = kv.get("START_INDEX").and_then(|v| v.trim().parse::<i64>().ok()).unwrap_or(0);
            let b = kv.get("END_INDEX").and_then(|v| v.trim().parse::<i64>().ok()).unwrap_or(-1);
            let r0 = kv.get("RPM_START").and_then(|v| crate::ac1::ini::num_of(v)).unwrap_or(0.0) as f32;
            let r1 = kv.get("RPM_END").and_then(|v| crate::ac1::ini::num_of(v)).unwrap_or(r0 as f64) as f32;
            let n = (b - a + 1).max(0) as usize;
            for k in 0..n {
                let rpm = if n > 1 { r0 + (r1 - r0) * k as f32 / (n - 1) as f32 } else { r0 };
                want.push((format!("{}{}", prefix, a + k as i64).to_lowercase(), LedKind::Rpm(rpm, None), serie_colour(k, n)));
            }
        } else if sec.starts_with("FUEL_WARNING_") {
            let Some(n) = kv.get("OBJECT_NAME") else { continue };
            let l = kv.get("FUEL_SWITCH").and_then(|v| crate::ac1::ini::num_of(v)).unwrap_or(5.0) as f32;
            want.push((n.to_lowercase(), LedKind::Fuel(l), [1.0, 0.6, 0.0]));
        }
    }
    for (node, what, colour) in want {
        match nodes.get(&node) {
            Some(true) => {}
            Some(false) => {
                p.notes.push(format!("leds: {node} is not in the cockpit - left as it is"));
                continue;
            }
            None => {
                p.notes.push(format!("leds: {node} is not a mesh of the kn5"));
                continue;
            }
        }
        if p.leds.iter().any(|l| l.node == node) {
            continue;
        }
        if p.leds.len() >= 26 {
            p.notes.push("leds: more than 26 LEDs - the rest left out".into());
            break;
        }
        let index = p.leds.len();
        hints.leds.insert(node.clone());
        hints.groups.insert(node.clone(), format!("led{index}"));
        p.leds.push(Led { index, node, what, colour });
    }
    p.notes.push(format!("leds: {} LED(s) ({} shift light(s)), limiter {:.0} rpm", p.leds.len(),
        p.leds.iter().filter(|l| matches!(l.what, LedKind::Rpm(..))).count(), p.limiter));
    p
}

fn instance(led: usize, value: f32, colour: [f32; 3], blink: Option<f32>, intensity: f32) -> Msg {
    let mut m = Msg::default();
    if led != 0 {
        m.set_varint(1, led as u64);
    }
    if value != 0.0 {
        m.set_f32(2, value);
    }
    match blink {
        Some(t) => {
            m.set_f32(6, t);
            m.set_f32(7, intensity);
            m.set_f32(8, t);
        }
        None => {
            m.set_f32(6, 1.0);
            m.set_f32(7, intensity);
        }
    }
    m.push(20, Val::Msg(fx::vec_msg(&colour)));
    m.push(21, Val::Msg(Msg::default()));
    m
}

/// The `.carledsystem` from the donor's: every group emptied, ours filled.
pub fn led_system(tmpl: &Msg, p: &LedPlan) -> Msg {
    let lim = p.limiter.max(1000.0);
    let mut groups: BTreeMap<u32, Vec<Msg>> = BTreeMap::new();
    for l in &p.leds {
        match l.what {
            LedKind::Rpm(r, blink) => {
                // never past the limiter
                groups.entry(1).or_default().push(instance(l.index, r.min(lim * 0.995), l.colour, None, 1.0));
                if let Some(hz) = blink {
                    groups.entry(3).or_default().push(instance(l.index, 0.0, l.colour, Some(0.5 / hz.max(1.0)), 1.0));
                }
            }
            LedKind::Fuel(lt) => groups.entry(4).or_default().push(instance(l.index, lt, l.colour, None, 0.5)),
        }
    }
    let mut out = Msg::default();
    let mut limiter_done = false;
    for f in &tmpl.fields {
        match (&f.val, f.num) {
            // the limiter (varint); the trailing fixed32 f33 is kept below
            (Val::Varint(_), 33) => {
                out.set_varint(33, lim.round() as u64);
                limiter_done = true;
            }
            (Val::F32(_), 33) | (_, 35) => out.fields.push(f.clone()),
            (Val::Bytes(_), n) | (Val::Msg(_), n) => {
                let mut g = Msg::default();
                // repeated groups (light stages, gearbox, performances) keep their count
                if let Some(v) = groups.remove(&n) {
                    for i in v {
                        g.push(1, Val::Msg(i));
                    }
                }
                out.fields.push(Field { num: n, val: Val::Msg(g) });
            }
            _ => out.fields.push(f.clone()),
        }
    }
    for (n, v) in groups {
        let mut g = Msg::default();
        for i in v {
            g.push(1, Val::Msg(i));
        }
        let at = out.fields.iter().position(|x| x.num == 35).unwrap_or(out.fields.len());
        out.fields.insert(at, Field { num: n, val: Val::Msg(g) });
    }
    if !limiter_done {
        let at = out.fields.iter().position(|x| x.num == 35).unwrap_or(out.fields.len());
        out.fields.insert(at, Field { num: 33, val: Val::Varint(lim.round() as u64) });
    }
    out
}

/// After the body: LED materials (VehicleLed, one uniform LedMask each),
/// the LED system, the actor's LED batches.
pub fn write(files: &mut Files, id: &str, plan: &Plan, insts: &[Inst], lp: &LedPlan, written: &[(String, Kind, usize, bool, String)]) -> Result<Vec<String>> {
    let base = format!("content\\cars\\{id}");
    let mut notes = Vec::new();
    let mut batches = Vec::new();
    for s in plan.sections.iter().filter(|s| s.kind == Kind::Led) {
        let Some(led) = s.parts.iter().find_map(|(ii, _)| lp.leds.iter().find(|l| l.node == insts[*ii].node.to_lowercase())) else { continue };
        let Some((_, _, _, _, mat)) = written.iter().find(|w| w.0 == s.name) else { continue };
        let generic = files.get(mat).and_then(|b| Msg::parse(b).ok()).unwrap_or_default();
        let Some(mut m) = lp.template.clone() else {
            notes.push("leds: the donor has no INT_LED material - LEDs left generic".into());
            break;
        };
        // the AC1 diffuse is both the look and the emission
        let tex = get_slot(&generic, "Base_BaseColorMap");
        for slot in ["Base_NormalMap", "RoughnessMap", "txRoughness"] {
            set_slot(&mut m, slot, "");
        }
        set_prop(&mut m, "Base_HasNormalMap", &[0.0]);
        match &tex {
            Some(t) => {
                set_slot(&mut m, "Base_BaseColorMap", t);
                set_slot(&mut m, "Base_EmissiveMap", t);
                set_prop(&mut m, "Base_HasBaseColorMap", &[1.0]);
                set_prop(&mut m, "Base_HasEmissiveMap", &[1.0]);
            }
            None => {
                set_slot(&mut m, "Base_BaseColorMap", "");
                set_prop(&mut m, "Base_HasBaseColorMap", &[0.0]);
                let w = format!("{base}\\texture\\leds\\led_white.texture");
                if files.get(&w).is_none() {
                    fx::uniform_texture(files, &w, [255, 255, 255, 255], false);
                }
                set_slot(&mut m, "Base_EmissiveMap", &w);
                set_prop(&mut m, "Base_HasEmissiveMap", &[1.0]);
            }
        }
        let v = (led.index * 10).min(250) as u8;
        let mp = format!("{base}\\texture\\leds\\led_mask_{:02}.texture", led.index + 1);
        fx::uniform_texture(files, &mp, [v, v, v, 255], false);
        set_slot(&mut m, "LedMask", &mp);
        for k in 1..=26 {
            set_prop(&mut m, &format!("ColorLed{k:02}"), &[1.0, 1.0, 1.0, 0.0]);
        }
        files.put(mat, m.encode());
        batches.push(s.name.clone());
    }
    let ap = format!("{base}\\{id}.actor");
    let Some(mut actor) = files.get(&ap).and_then(|b| Msg::parse(b).ok()) else { return Ok(notes) };
    let car = actor.msg_mut(103)?;
    let path = car.str(102).unwrap_or_else(|| format!("{base}\\instruments\\{id}.carledsystem"));
    let tmpl = files.get(&path).and_then(|b| Msg::parse(b).ok()).unwrap_or_default();
    files.put(&path, led_system(&tmpl, lp).encode());
    car.set_str(102, &path);
    // LED batches: our LED sections (the clean-up emptied the donor's)
    {
        let lb = car.msg_mut(26)?;
        lb.remove(1);
        let rest: Vec<Field> = lb.fields.drain(..).collect();
        for b in &batches {
            lb.push(1, Val::Bytes(b.as_bytes().to_vec()));
        }
        lb.fields.extend(rest);
    }
    files.put(&ap, actor.encode());
    notes.push(format!("leds: {} LED material(s), LED system with {} rule(s), limiter {:.0}", batches.len(),
        lp.leds.iter().map(|l| if matches!(l.what, LedKind::Rpm(_, Some(_))) { 2 } else { 1 }).sum::<usize>(), lp.limiter));
    Ok(notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_leds_empties_every_group() {
        let mut t = Msg::default();
        let mut g = Msg::default();
        g.push(1, Val::Msg(instance(3, 4200.0, [0.0, 1.0, 0.0], None, 1.0)));
        t.push(1, Val::Msg(g));
        t.push(19, Val::Msg(Msg::default()));
        t.push(19, Val::Msg(Msg::default()));
        t.set_varint(33, 6000);
        t.push(35, Val::Bytes(vec![0, 0, 0x48, 0x42]));
        t.push(33, Val::F32(2.0f32.to_le_bytes()));
        let p = LedPlan { limiter: 7200.0, ..Default::default() };
        let m = led_system(&t, &p);
        assert!(m.sub(1).unwrap().subs(1).is_empty());
        assert_eq!(m.subs(19).len(), 2);
        assert_eq!(m.varint(33), Some(7200));
        assert!(matches!(m.fields.last().unwrap().val, Val::F32(_)));
    }

    #[test]
    fn shift_lights_stay_below_the_limiter() {
        let p = LedPlan { limiter: 7000.0, leds: vec![Led { index: 0, node: "a".into(), what: LedKind::Rpm(7500.0, Some(8.0)), colour: [1.0, 0.0, 0.0] }], ..Default::default() };
        let m = led_system(&Msg::default(), &p);
        let r = m.sub(1).unwrap().subs(1)[0].f32(2).unwrap();
        assert!(r < 7000.0);
        assert_eq!(m.sub(3).unwrap().subs(1).len(), 1);
    }
}
