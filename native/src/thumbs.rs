//! Cached car renders and track covers for the list rows - ported from the
//! serving side of acecm/thumbs.py. Rendering (evoview) and cover decoding
//! stay in the backend: a request that would MAKE a picture is passed there.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use serde_json::{Map, Value};

use crate::{carsmap, livery, paths, trackimport};

fn dir(which: &str) -> PathBuf {
    let d = paths::data().join("thumbs").join(which);
    let _ = std::fs::create_dir_all(&d);
    d
}

fn safe(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || "_.-".contains(c) { c } else { '_' }).collect()
}

/// Keyed by the LOOK (trim + livery), not just the model: a car's trims
/// share one model folder.
pub fn car_path(car_id: &str, big: bool, visual: &str, paint: &str) -> PathBuf {
    let mut tag = String::new();
    if !visual.is_empty() || !paint.is_empty() {
        let raw = format!("{visual}|{paint}").to_lowercase();
        let h = sha1_smol::Sha1::from(raw.as_bytes()).digest().to_string();
        tag = format!("@{}", &h[..10]);
    }
    dir("cars").join(format!("{}{}{}", safe(car_id), tag, if big { "@big.png" } else { ".png" }))
}

pub fn track_path(folder: &str) -> PathBuf {
    dir("tracks").join(format!("{}.png", safe(folder)))
}

static GARAGE: Mutex<Option<(String, Map<String, Value>)>> = Mutex::new(None);

/// model -> the EXT SKIN paint on it. Memoised on the SavedCars folder's
/// mtime (one page of the car list asks once per row).
fn garage_paints() -> Map<String, Value> {
    let d = livery::saved_cars_dir();
    let mt = if !d.is_empty() && Path::new(&d).is_dir() {
        std::fs::metadata(&d).ok().and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|x| x.as_nanos()).unwrap_or(0)
    } else { 0 };
    let key = format!("{d}|{mt}");
    if let Some((k, m)) = GARAGE.lock().unwrap().as_ref() {
        if *k == key {
            return m.clone();
        }
    }
    let mut out = Map::new();
    for c in livery::garage() {
        let model = c.get("model").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let skin = c.get("slots").and_then(|s| s.get("EXT SKIN")).and_then(|v| v.as_str())
            .unwrap_or("").trim().to_string();
        if !model.is_empty() && !skin.is_empty() && !out.contains_key(&model) {
            let leaf = skin.replace('\\', "/");
            out.insert(model, Value::String(leaf.rsplit('/').next().unwrap_or("").to_string()));
        }
    }
    *GARAGE.lock().unwrap() = Some((key, out.clone()));
    out
}

/// (model, visual preset, EXT SKIN paint) for a mechanical preset id.
pub fn preset_look(preset_id: &str) -> (String, String, String) {
    let t = carsmap::table(false);
    let mut model = t.get("presets").and_then(|p| p.get(preset_id)).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let visual = t.get("variants").and_then(|v| v.get(preset_id)).and_then(|v| v.get("visual"))
        .and_then(|v| v.as_str()).unwrap_or("").to_string();
    if model.is_empty() {
        model = preset_id.to_string();
    }
    let paint = garage_paints().get(&model).and_then(|v| v.as_str()).unwrap_or("").to_string();
    (model, visual, paint)
}

/// The cached render a list row shows, or None (no render is ever MADE here).
pub fn list_car(id: &str, preset: &str) -> Option<PathBuf> {
    let (mut cid, mut visual, mut paint) = (id.to_string(), String::new(), String::new());
    if !preset.is_empty() {
        let (m, v, p) = preset_look(preset);
        if cid.is_empty() {
            cid = m;
        }
        visual = v;
        paint = p;
    }
    if cid.is_empty() {
        return None;
    }
    let shot = car_path(&cid, false, &visual, &paint);
    if shot.is_file() {
        return Some(shot);
    }
    // the trim-and-livery render may not exist yet: fall back to the model
    if !visual.is_empty() || !paint.is_empty() {
        let plain = car_path(&cid, false, "", "");
        if plain.is_file() {
            return Some(plain);
        }
    }
    None
}

/// Folder id or the DISPLAY NAME some lists send ("Circuit Of The Americas").
pub fn resolve_folder(folder: &str) -> String {
    if folder.is_empty() {
        return String::new();
    }
    let m = trackimport::track_map(false);
    if m.values().any(|v| v.as_str() == Some(folder)) {
        return folder.to_string();
    }
    if let Some(hit) = m.get(folder).and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
        return hit.to_string();
    }
    let low = folder.trim().to_lowercase();
    for (name, f) in &m {
        if name.trim().to_lowercase() == low {
            return f.as_str().unwrap_or(folder).to_string();
        }
    }
    folder.to_string()
}
