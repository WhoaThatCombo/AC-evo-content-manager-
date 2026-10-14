//! The share list (<data>/registry.json) - ported from registry.load / save
//! / upsert / remove / autofill. Serving content to joiners (manifests,
//! files, packs, the public list with its size cache) stays in the backend
//! for now; its size cache is keyed on the entries themselves, so an edit
//! made here is noticed there.

use serde_json::{json, Map, Value};

use crate::{misc, paths, profiles};

fn path() -> std::path::PathBuf {
    paths::data().join("registry.json")
}

pub fn template() -> Map<String, Value> {
    match json!({"id": "", "name": "My EVO server", "description": "", "ip": "", "port": 9700,
                 "profile_id": "", "required_mods": [], "required_tracks": [], "public": true}) {
        Value::Object(m) => m,
        _ => unreachable!(),
    }
}

pub fn load() -> Vec<Value> {
    std::fs::read(path()).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.as_array().cloned()).unwrap_or_default()
}

pub fn save(items: &[Value]) -> std::io::Result<()> {
    crate::profiles::write_json_indent2(&path(), &Value::Array(items.to_vec()))
}

/// The game port an entry should advertise: its linked profile, else the
/// only profile when there is exactly one.
fn profile_port(profile_id: &str) -> i64 {
    let profs = profiles::load();
    if !profile_id.is_empty() {
        if let Some(p) = profs.iter().find(|p| p.get("id").and_then(|v| v.as_str()) == Some(profile_id)) {
            return p.get("tcp_port").and_then(|v| v.as_i64()).unwrap_or(0);
        }
    }
    if profs.len() == 1 {
        return profs[0].get("tcp_port").and_then(|v| v.as_i64()).unwrap_or(0);
    }
    0
}

/// Fill only what is blank: a host who typed an address or port meant it.
pub fn autofill(e: &mut Map<String, Value>) {
    if e.get("ip").and_then(|v| v.as_str()).map(|s| s.trim().is_empty()).unwrap_or(true) {
        e.insert("ip".into(), json!(misc::public_ipv4()));
    }
    let port = e.get("port").and_then(|v| v.as_i64()).unwrap_or(0);
    if port == 0 || port == 9700 {
        let pid = e.get("profile_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let p = profile_port(&pid);
        if p != 0 {
            e.insert("port".into(), json!(p));
        }
    }
}

pub fn upsert(entry: &Value) -> Value {
    let mut e = template();
    if let Value::Object(m) = entry {
        for (k, v) in m {
            e.insert(k.clone(), v.clone());
        }
    }
    autofill(&mut e);
    if e.get("id").and_then(|v| v.as_str()).unwrap_or("").is_empty() {
        e.insert("id".into(), json!(profiles::new_id()));
    }
    let id = e["id"].clone();
    let mut items: Vec<Value> = load().into_iter().filter(|x| x.get("id") != Some(&id)).collect();
    let v = Value::Object(e);
    items.push(v.clone());
    let _ = save(&items);
    v
}

pub fn remove(id: &str) {
    let items: Vec<Value> = load().into_iter().filter(|x| x.get("id").and_then(|v| v.as_str()) != Some(id)).collect();
    let _ = save(&items);
}
