//! Driving assists - ported from assists.read (writing stays in the backend).
//!
//! ⚠ damage_rate is stored OFFSET BY ONE: shown% = (damage_rate + 1) * 100.
//! ⚠ RacingSetting: 0 = OFF, 1 = ON, 2 = FREE; manual_ignition is inverted
//! (the UI calls it "Automatic ignition").

use std::path::PathBuf;

use prost_reflect::{DynamicMessage, ReflectMessage, Value as PV};
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

/// Set a numeric field whatever its declared kind (enum, int or float).
fn set_num(m: &mut DynamicMessage, name: &str, v: f64) -> bool {
    use prost_reflect::Kind;
    let Some(fd) = m.descriptor().get_field_by_name(name) else { return false };
    let pv = match fd.kind() {
        Kind::Enum(_) => PV::EnumNumber(v as i32),
        Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => PV::I32(v as i32),
        Kind::Uint32 | Kind::Fixed32 => PV::U32(v as u32),
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => PV::I64(v as i64),
        Kind::Uint64 | Kind::Fixed64 => PV::U64(v as u64),
        Kind::Float => PV::F32(v as f32),
        Kind::Double => PV::F64(v),
        Kind::Bool => PV::Bool(v != 0.0),
        _ => return false,
    };
    m.set_field(&fd, pv);
    true
}

fn py_float(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok().filter(|f| f.is_finite()),
        Value::Bool(b) => Some(*b as i32 as f64),
        _ => None,
    }
}

/// Apply only the keys present in `values`; everything else is preserved.
/// ⚠ Changing anything makes it a CUSTOM preset, as the game's own page does.
pub fn write(values: &Value) -> Value {
    use prost::Message;
    let d = dir_path();
    if !d.is_dir() {
        return json!({"ok": false, "error": "no AssistSettings folder - run the game once"});
    }
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let mut changed: Vec<String> = Vec::new();
    let custom = d.join("AssistPresetListType_CUSTOM.racingsettingsassistpreset");
    let current = d.join("current.assistpresetsave");
    for (path, wrap) in [(custom, false), (current, true)] {
        if !path.is_file() {
            continue;
        }
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let raw = std::fs::read(&path).unwrap_or_default();
        let mut msg = match protos::decode(if wrap { "AssistPresetSave" } else { MSG }, &raw) {
            Ok(m) => m,
            Err(e) => return json!({"ok": false, "error": format!("unreadable {name}: {e}")}),
        };
        {
            let p: &mut DynamicMessage = if wrap {
                match msg.get_field_by_name_mut("preset") { Some(PV::Message(m)) => m, _ => continue }
            } else { &mut msg };
            for (fname, _label, inv) in TOGGLES {
                if let Some(v) = values.get(*fname) {
                    let s = match v { Value::String(s) => s.to_lowercase(), Value::Bool(b) => if *b { "true".into() } else { "false".into() }, v => v.to_string() };
                    let n = match flip(&s, *inv).as_str() { "off" => Some(0.0), "on" => Some(1.0), "free" => Some(2.0), _ => None };
                    if let Some(n) = n {
                        set_num(p, fname, n);
                        changed.push(fname.to_string());
                    }
                }
            }
            if let Some(v) = values.get("stability_control").and_then(py_float) {
                set_num(p, "stability_control", (v / 100.0).clamp(0.0, 1.0));
                changed.push("stability_control".into());
            }
            if let Some(v) = values.get("damage_pct").and_then(py_float) {
                set_num(p, "damage_rate", v.clamp(0.0, 100.0) / 100.0 - 1.0);
                changed.push("damage_pct".into());
            }
            set_num(p, "type", 4.0);
        }
        let blob = msg.encode_to_vec();
        if let Err(e) = crate::safefs::copy_atomic(&path, std::path::Path::new(&format!("{}.{stamp}.bak", path.display()))) {
            return json!({"ok": false, "error": format!("could not back the preset up first ({e}) - nothing was changed")});
        }
        if let Err(e) = crate::safefs::write_atomic(&path, &blob) {
            return json!({"ok": false, "error": e.to_string()});
        }
    }
    if changed.is_empty() {
        return json!({"ok": false, "error": "nothing to change"});
    }
    changed.sort();
    changed.dedup();
    crate::logs::info(format!("assists updated: {}", changed.join(", ")));
    let mut r = read();
    r["saved"] = json!(changed);
    r
}
