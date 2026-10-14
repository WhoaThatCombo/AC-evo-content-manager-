//! The game's own settings files - ported from the read side of
//! acecm/settings.py (state, discover, read, backups, export_bundle).
//! Files under Saved Games\ACE are raw protobuf whose EXTENSION is the
//! message name (video.videosettings -> VideoSettings).
//! Writing / importing / restoring stay in the backend for now.

use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use prost_reflect::SerializeOptions;
use serde_json::{json, Value};

use crate::{config, detect, procs, protos};

const FEATURED: &[(&str, &str)] = &[
    ("input_settings.inputsettings", "Force feedback & steering"),
    ("video.videosettings", "Graphics & display"),
    ("audio.audiosettings", "Audio"),
    ("input_devices.inputdeviceconfiguration", "Controller bindings"),
    ("input_keyboard.keyboardinputconfiguration", "Keyboard bindings"),
    ("vr.videosettings", "VR"),
];

pub fn settings_dir() -> String {
    let cfg = config::load();
    let a = config::get_str(&cfg, "ace_dir");
    if !a.is_empty() {
        return a;
    }
    let d = detect::find("ace_dir", false);
    if !d.is_empty() {
        return d;
    }
    let home = std::env::var("USERPROFILE").map(PathBuf::from).unwrap_or_default();
    home.join("Saved Games").join("ACE").to_string_lossy().into_owned()
}

/// lowercase top-level message name -> real name.
fn index() -> HashMap<String, String> {
    let mut out = HashMap::new();
    for m in protos::pool().all_messages() {
        if m.parent_message().is_none() {
            out.entry(m.name().to_lowercase()).or_insert_with(|| m.name().to_string());
        }
    }
    out
}

fn message_for(idx: &HashMap<String, String>, path: &Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    // os.path.splitext: the extension after the last dot (not a leading one)
    let ext = match name.rfind('.') {
        Some(i) if name[..i].chars().any(|c| c != '.') => &name[i + 1..],
        _ => "",
    };
    idx.get(&ext.to_lowercase()).cloned()
}

pub fn state() -> Value {
    json!({"dir": settings_dir(), "game_running": procs::game_running()})
}

fn walk(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            // ⚠ PRUNE mods/ (the multi-GB library), logs and crashdumps
            if dir == root {
                let n = e.file_name().to_string_lossy().to_lowercase();
                if n == "logs" || n == "crashdumps" || n == "mods" {
                    continue;
                }
            }
            walk(&p, root, out);
        } else {
            out.push(p);
        }
    }
}

/// Every settings file we can decode, featured ones first.
pub fn discover() -> Value {
    let d = settings_dir();
    let root = Path::new(&d);
    if !root.is_dir() {
        return json!({"dir": d, "files": [], "error": "settings folder not found"});
    }
    let idx = index();
    let mut paths = vec![];
    walk(root, root, &mut paths);
    let mut found: BTreeMap<String, Value> = BTreeMap::new();
    for p in paths {
        if !p.is_file() {
            continue;
        }
        let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().replace('\\', "/");
        let msg = message_for(&idx, &p);
        let decodable = msg.as_deref().map(protos::has).unwrap_or(false);
        let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        found.insert(rel.clone(), json!({"file": rel, "message": msg, "size": size, "decodable": decodable}));
    }
    let mut order = vec![];
    for (name, label) in FEATURED {
        if let Some(mut v) = found.remove(*name) {
            v["label"] = json!(label);
            order.push(v);
        }
    }
    order.extend(found.into_values());
    let n = order.iter().filter(|f| f["decodable"] == true).count();
    json!({"dir": d, "files": order, "decodable": n})
}

/// A file under the settings folder, or None if the path walks out.
fn safe_path(rel: &str) -> Option<PathBuf> {
    let rel = rel.replace('\\', "/");
    let parts: Vec<&str> = rel.trim_start_matches('/').split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    if parts.is_empty() || parts.contains(&"..") {
        return None;
    }
    let root = PathBuf::from(settings_dir());
    if root.as_os_str().is_empty() {
        return None;
    }
    let mut p = root.clone();
    for part in parts {
        let c = Path::new(part);
        if c.components().any(|x| !matches!(x, Component::Normal(_))) {
            return None;
        }
        p.push(part);
    }
    p.starts_with(&root).then_some(p)
}

/// Decode one settings file to plain JSON (proto3 JSON mapping, snake_case
/// names, every field printed - as json_format.MessageToDict did).
pub fn read(rel: &str) -> Value {
    let Some(path) = safe_path(rel).filter(|p| p.is_file()) else {
        return json!({"ok": false, "error": "not found"});
    };
    let idx = index();
    let msg = message_for(&idx, &path);
    let Some(name) = msg.clone().filter(|m| protos::has(m)) else {
        return json!({"ok": false, "error": format!("no schema for '{rel}'"), "message": msg});
    };
    let raw = match std::fs::read(&path) {
        Ok(r) => r,
        Err(e) => return json!({"ok": false, "error": format!("cannot parse: {e}")}),
    };
    let m = match protos::decode(&name, &raw) {
        Ok(m) => m,
        Err(e) => return json!({"ok": false, "error": format!("cannot parse: {e}")}),
    };
    let opts = SerializeOptions::new().use_proto_field_name(true).skip_default_fields(false);
    let mut ser = serde_json::Serializer::new(vec![]);
    let values = match m.serialize_with_options(&mut ser, &opts) {
        Ok(()) => serde_json::from_slice::<Value>(&ser.into_inner()).unwrap_or(Value::Null),
        Err(e) => return json!({"ok": false, "error": format!("cannot parse: {e}")}),
    };
    json!({"ok": true, "file": rel, "message": name, "values": values})
}

/// Every timestamped copy we have taken of one settings file.
pub fn backups(rel: &str) -> Value {
    let Some(path) = safe_path(rel) else {
        return json!({"ok": false, "file": rel, "backups": []});
    };
    let (d, base) = (path.parent().map(Path::to_path_buf).unwrap_or_default(),
                     path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    let mut names: Vec<String> = std::fs::read_dir(&d).map(|rd| {
        rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()
    }).unwrap_or_default();
    names.sort();
    names.reverse();
    let mut out = vec![];
    for f in names {
        if f.starts_with(&format!("{base}.")) && (f.ends_with(".bak") || f.ends_with(".bak_acecm")) {
            let m = std::fs::metadata(d.join(&f)).ok();
            out.push(json!({
                "name": f,
                "size": m.as_ref().map(|m| m.len()).unwrap_or(0),
                "mtime": m.and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs()).unwrap_or(0),
            }));
        }
    }
    json!({"ok": true, "file": rel, "backups": out})
}
