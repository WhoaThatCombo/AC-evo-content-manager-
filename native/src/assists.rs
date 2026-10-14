//! Driving assists - ported from assists.read (writing stays in the backend).
//!
//! ⚠ damage_rate is stored OFFSET BY ONE: shown% = (damage_rate + 1) * 100.
//! ⚠ RacingSetting: 0 = OFF, 1 = ON, 2 = FREE; manual_ignition is inverted
//! (the UI calls it "Automatic ignition").

use std::path::PathBuf;

use prost_reflect::{DynamicMessage, Value as PV};
use serde_json::{json, Value};

use crate::protos;

const MSG: &str = "RacingSettingsAssistPreset";

const TOGGLES: &[(&str, &str, bool)] = &[
    ("manual_ignition", "Automatic ignition", true),
    ("automatic_gearbox", "Automatic gearbox", false),
    ("automatic_pit_limiter", "Automatic pit limiter", false),
    ("automatic_clutch", "Automatic clutch", false),
    ("automatic_clutch_on_start", "Automatic clutch on start", false),
    ("standing_start_assist", "Standing start assist", false),
    ("automatic_blip", "Automatic throttle blip", false),
    ("automatic_lights", "Automatic headlights", false),
    ("automatic_wipers", "Automatic wipers", false),
    ("ideal_line", "Ideal line", false),
];

pub fn dir_path() -> PathBuf {
    let home = std::env::var("USERPROFILE").map(PathBuf::from).unwrap_or_default();
    home.join("Saved Games").join("ACE").join("AssistSettings")
}

fn num(m: &DynamicMessage, name: &str) -> f64 {
    match m.get_field_by_name(name).map(|v| v.into_owned()) {
        Some(PV::EnumNumber(n)) | Some(PV::I32(n)) => n as f64,
        Some(PV::U32(n)) => n as f64,
        Some(PV::I64(n)) => n as f64,
        Some(PV::U64(n)) => n as f64,
        Some(PV::F32(f)) => f as f64,
        Some(PV::F64(f)) => f,
        Some(PV::Bool(b)) => b as i32 as f64,
        _ => 0.0,
    }
}

fn flip(v: &str, inverted: bool) -> String {
    if !inverted {
        return v.to_string();
    }
    match v { "on" => "off", "off" => "on", other => other }.to_string()
}

pub fn read() -> Value {
    let d = dir_path();
    let ds = d.to_string_lossy().into_owned();
    if !d.is_dir() {
        return json!({"ok": false, "error": "no AssistSettings folder - run the game once", "dir": ds});
    }
    let path = d.join("AssistPresetListType_CUSTOM.racingsettingsassistpreset");
    // a missing file reads as the defaults, like ParseFromString never run
    let raw = std::fs::read(&path).unwrap_or_default();
    let p = match protos::decode(MSG, &raw) {
        Ok(p) => p,
        Err(e) => return json!({"ok": false, "error": format!("unreadable preset: {e}"), "dir": ds}),
    };
    let toggles: Vec<Value> = TOGGLES.iter().map(|(name, label, inv)| {
        let v = match num(&p, name) as i64 { 1 => "on", 2 => "free", _ => "off" };
        json!({"name": name, "label": label, "value": flip(v, *inv)})
    }).collect();
    // python round(): halves go to even
    let sc = (num(&p, "stability_control") * 100.0).round_ties_even() as i64;
    let dmg = ((num(&p, "damage_rate") + 1.0) * 100.0).round_ties_even() as i64;
    json!({"ok": true, "dir": ds, "toggles": toggles, "stability_control": sc, "damage_pct": dmg})
}
