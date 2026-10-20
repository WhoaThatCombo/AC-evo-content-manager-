//! Analogue needles (requirements car-animations-needles.md):
//! `analog_instruments.ini` -> `.caranalogicsystem`.
//!
//! # The convention (checked on the calibration twins)
//!
//! AC1 turns a needle node about its own local Z by the instrument angle;
//! the stock EVO needle bone sits at the same node frame and turns about
//! `Axis_Z` by the LUT degrees with the same sign. ks_mazda_miata's
//! ARROW_RPM node axes (rows) x (-0.707, -0.694, -0.135), z (0, -0.191,
//! 0.982) at (0.414, 0.777, 0.348) are exactly ks_mazda_mx5_na's ARROW_RPM
//! bone, and the AC1 rpm LUT (1000=24 | 6000=206 | 7000=237 | 8000=268) is the
//! stock LUT; `analog_speed_curve.lut` (220 km/h = 266) is the stock speed
//! LUT (220 = 267). So: one bone at the node, Axis_Z, the AC1 degrees.
//!
//! # From AC1
//!
//! Per section: OBJECT_NAME, LUT (inline) or ZERO + (value - MIN_VALUE) x
//! STEP; `analog_speed_curve.lut` / `analog_turbo_curve.lut` files replace
//! the speed / turbo mapping. Channels: RPM_INDICATOR rpm, SPEED_INDICATOR
//! speed_kmh, FUEL_INDICATOR fuel_liters, WATER_TEMP water_temperature_c,
//! OIL_TEMP oil_temperature_c, OIL_PRESSURE oil_pressure_bar,
//! TURBO_INDICATOR turbo_pressure_bar (only with a turbo: a boost gauge
//! without one is dropped), PLACE_HOLDER_n place_holder (AC1 parks these at
//! ZERO while running: LUT 0 -> 0, 1 -> ZERO, value 1 with the ignition on).
//!
//! Sampling ranges (ours): rpm 0 .. 1.15 x limiter, speed 0 .. 400 km/h,
//! fuel 0 .. MAX_FUEL, temperatures 0 .. 160 C, boost 0 .. 1.3 x MAX_BOOST,
//! oil pressure 0 .. 10 bar; 9 points per ZERO / STEP needle.

use std::collections::BTreeMap;

use crate::ac1::ini::{self, Ini};
use crate::car::animparts::Needle;
use crate::io::pb::{Msg, Val};

/// One convertible AC1 needle.
pub struct NeedleSpec {
    pub section: String,
    pub object: String,
    /// CarAnalogicSystemData field
    pub field: u32,
    /// (value, degrees)
    pub lut: Vec<(f64, f64)>,
    pub clock: bool,
    pub channel: String,
}

fn channel(sec: &str) -> Option<(u32, &'static str)> {
    let s = sec.to_uppercase();
    Some(if s.starts_with("RPM") {
        (1, "rpm")
    } else if s.starts_with("SPEED") {
        (3, "speed_kmh")
    } else if s.starts_with("FUEL") {
        (4, "fuel_liters")
    } else if s.starts_with("WATER") {
        (9, "water_temperature_c")
    } else if s.starts_with("OIL_TEMP") {
        (10, "oil_temperature_c")
    } else if s.starts_with("OIL_PRESS") {
        (7, "oil_pressure_bar")
    } else if s.starts_with("TURBO") || s.starts_with("BOOST") {
        (8, "turbo_pressure_bar")
    } else if s.starts_with("PLACE_HOLDER") {
        (12, "place_holder")
    } else {
        return None;
    })
}

pub fn specs(ai: &Ini, data: &BTreeMap<String, String>, notes: &mut Vec<String>) -> Vec<NeedleSpec> {
    let eng = ini::parse(data.get("engine.ini").map(|s| s.as_str()).unwrap_or(""));
    let car = ini::parse(data.get("car.ini").map(|s| s.as_str()).unwrap_or(""));
    let limiter = eng.num("ENGINE_DATA", "LIMITER").unwrap_or(8000.0).max(1000.0);
    let turbo = eng.names().any(|n| n.starts_with("TURBO_"));
    let boost = eng.with_prefix("TURBO_").filter_map(|(_, kv)| kv.get("MAX_BOOST").and_then(|v| ini::num_of(v))).fold(0.0f64, f64::max).max(1.0);
    let tank = car.num("FUEL", "MAX_FUEL").unwrap_or(60.0).max(10.0);
    let mut out = Vec::new();
    for (sec, kv) in ai.sections.iter() {
        let Some((field, ch)) = channel(sec) else { continue };
        let Some(obj) = kv.get("OBJECT_NAME").map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) else { continue };
        if field == 8 && !turbo {
            notes.push(format!("needle [{sec}] {obj}: a boost gauge on a car without a turbo - left out"));
            continue;
        }
        let num = |k: &str, d: f64| kv.get(k).and_then(|v| ini::num_of(v)).unwrap_or(d);
        let (zero, step, min) = (num("ZERO", 0.0), num("STEP", 1.0), num("MIN_VALUE", 0.0));
        let file_lut = match field {
            3 => data.get("analog_speed_curve.lut"),
            8 => data.get("analog_turbo_curve.lut"),
            _ => None,
        }.map(|t| ini::lut(t)).filter(|l| l.len() >= 2);
        let inline = kv.get("LUT").map(|t| ini::lut(t)).filter(|l| l.len() >= 2);
        let lut: Vec<(f64, f64)> = if field == 12 {
            vec![(0.0, 0.0), (1.0, zero)]
        } else if let Some(l) = inline.or(file_lut) {
            l
        } else {
            let hi = match field {
                1 => limiter * 1.15,
                3 => 400.0,
                4 => tank,
                7 => 10.0,
                8 => boost * 1.3,
                _ => 160.0,
            };
            let lo = min.min(hi - 1.0);
            (0..=8).map(|k| {
                let v = lo + (hi - lo) * k as f64 / 8.0;
                (v, zero + (v - min) * step)
            }).collect()
        };
        out.push(NeedleSpec { section: sec.clone(), object: obj, field, lut, clock: false, channel: ch.to_string() });
    }
    out
}

/// The `.caranalogicsystem` bytes (fields in number order, like stock).
pub fn analogic(needles: &[Needle]) -> Vec<u8> {
    let mut by: BTreeMap<u32, Vec<Msg>> = BTreeMap::new();
    for n in needles {
        let mut i = Msg::default();
        i.set_str(1, &n.bone);
        i.set_varint(2, 2); // Axis_Z
        for (v, d) in &n.lut {
            let mut l = Msg::default();
            if *v != 0.0 {
                l.set_f32(1, *v as f32);
            }
            if *d != 0.0 {
                l.set_f32(2, *d as f32);
            }
            i.push(3, Val::Msg(l));
        }
        if n.field == 12 {
            i.set_f32(7, 1.0);
        }
        i.set_f32(30, 10.0);
        i.set_f32(31, 10.0);
        by.entry(n.field).or_default().push(i);
    }
    let mut m = Msg::default();
    for (f, v) in by {
        for i in v {
            m.push(f, Val::Msg(i));
        }
    }
    m.encode()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn miata_needles() {
        let ai = ini::parse("[RPM_INDICATOR]\nZERO=0\nSTEP=0.034\nOBJECT_NAME=ARROW_RPM\nLUT=(0=0|1000=24|8000=268)\n[SPEED_INDICATOR]\nZERO=0\nSTEP=1.2\nOBJECT_NAME=ARROW_SPEED\n[TURBO_INDICATOR]\nOBJECT_NAME=ARROW_TURBO\n[PLACE_HOLDER_0]\nOBJECT_NAME=ARROW_OIL\nZERO=53\nSTEP=-1.77\n");
        let mut data = BTreeMap::new();
        data.insert("analog_speed_curve.lut".to_string(), "0|2\n220|266\n".to_string());
        data.insert("engine.ini".to_string(), "[ENGINE_DATA]\nLIMITER=7200\n".to_string());
        let mut notes = Vec::new();
        let s = specs(&ai, &data, &mut notes);
        assert_eq!(s.len(), 3, "no turbo -> the boost gauge goes");
        assert_eq!(s[0].lut[2], (8000.0, 268.0));
        assert_eq!(s[1].lut[1], (220.0, 266.0));
        assert_eq!(s[2].lut, vec![(0.0, 0.0), (1.0, 53.0)]);
        let n = Needle { field: 1, bone: "AC1_ARROW_RPM_I".into(), lut: s[0].lut.clone(), clock: false, what: String::new() };
        let m = Msg::parse(&analogic(&[n])).unwrap();
        let i = m.sub(1).unwrap();
        assert_eq!(i.str(1).as_deref(), Some("AC1_ARROW_RPM_I"));
        assert_eq!(i.varint(2), Some(2));
        assert_eq!(i.subs(3).len(), 3);
    }
}
