//! The player's garage - ported from livery._saved_cars_dir / garage (read
//! side). Each saved car is a CarFinalStateWithConsumable record decoded
//! with the game's own schema; its multilayer_states map sits several levels
//! down, so it is found by walking rather than by a fixed path.

use std::path::Path;

use prost_reflect::{DynamicMessage, ReflectMessage, Value as PV};
use serde_json::{json, Map, Value};

use crate::{detect, protos};

const RECORD: &str = "CarFinalStateWithConsumable";
const EXT: &str = ".carfinalstatewithconsumable";

/// <Saved Games>\ACE\ProfileData\<profile>\OpenData\SavedCars, or "".
pub fn saved_cars_dir() -> String {
    let root = Path::new(&detect::saved_games()).join("ACE").join("ProfileData");
    let Ok(rd) = std::fs::read_dir(&root) else { return String::new() };
    let mut names: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    for n in names {
        let d = root.join(&n).join("OpenData").join("SavedCars");
        if d.is_dir() {
            return d.to_string_lossy().into_owned();
        }
    }
    String::new()
}

/// Every sub-message, depth-first (set fields only, like ListFields).
fn walk<'a>(m: &'a DynamicMessage, depth: usize, out: &mut Vec<DynamicMessage>) {
    if depth > 6 {
        return;
    }
    out.push(m.clone());
    for (_f, v) in m.fields() {
        match v {
            PV::Message(sub) => walk(sub, depth + 1, out),
            PV::List(items) => for it in items { if let PV::Message(s) = it { walk(s, depth + 1, out) } },
            PV::Map(mp) => for it in mp.values() { if let PV::Message(s) = it { walk(s, depth + 1, out) } },
            _ => {}
        }
    }
}

fn slots(rec: &DynamicMessage) -> Map<String, Value> {
    let mut all = vec![];
    walk(rec, 0, &mut all);
    for holder in all {
        let Some(fd) = holder.descriptor().get_field_by_name("multilayer_states") else { continue };
        if !holder.has_field(&fd) {
            continue;
        }
        if let PV::Map(states) = holder.get_field(&fd).into_owned() {
            if states.is_empty() {
                continue;
            }
            let mut out = Map::new();
            for (k, st) in states {
                let key = match k { prost_reflect::MapKey::String(s) => s, other => format!("{other:?}") };
                out.insert(key, json!(current_color(&st)));
            }
            return out;
        }
    }
    Map::new()
}

fn current_color(st: &PV) -> String {
    let PV::Message(m) = st else { return String::new() };
    let Some(ch) = m.get_field_by_name("primary_channel_state") else { return String::new() };
    let PV::Message(ch) = ch.into_owned() else { return String::new() };
    match ch.get_field_by_name("oem_color_path").map(|v| v.into_owned()) {
        Some(PV::String(s)) => s,
        _ => String::new(),
    }
}

/// <model>_<uuid> -> (model, uuid)
fn split_name(stem: &str) -> (String, String) {
    // the uuid is the last underscore-free token, shaped 8-4-4-4-12 hex
    if let Some((model, inst)) = stem.rsplit_once('_') {
        let g: Vec<&str> = inst.split('-').collect();
        let shape = [8, 4, 4, 4, 12];
        if g.len() == 5 && g.iter().zip(shape)
            .all(|(s, n)| s.len() == n && s.chars().all(|c| c.is_ascii_hexdigit()))
        {
            return (model.to_string(), inst.to_string());
        }
    }
    (stem.to_string(), String::new())
}

/// Every owned car: model, instance id, and the colour of each slot.
pub fn garage() -> Vec<Value> {
    let d = saved_cars_dir();
    let mut out = vec![];
    if d.is_empty() {
        return out;
    }
    let mut names: Vec<String> = std::fs::read_dir(&d).map(|rd| {
        rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()
    }).unwrap_or_default();
    names.sort();
    for name in names {
        let Some(stem) = name.strip_suffix(EXT) else { continue };
        let path = Path::new(&d).join(&name);
        let (model, inst) = split_name(stem);
        let mut entry = json!({"model": model, "instance": inst,
                               "file": path.to_string_lossy(), "slots": {}});
        match std::fs::read(&path).map_err(|e| e.to_string()).and_then(|raw| protos::decode(RECORD, &raw)) {
            Ok(rec) => entry["slots"] = Value::Object(slots(&rec)),
            Err(e) => entry["error"] = json!(e),
        }
        out.push(entry);
    }
    out
}
