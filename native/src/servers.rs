//! Server profiles - ported from acecm/servers.py.
//! Part 1: the car policy (two independent axes: stock cars on/off, and
//! mods all / none / picked). Process management follows.

use std::collections::HashSet;

use serde_json::{json, Value};

use crate::{content, mods};

fn catalog() -> Vec<Value> {
    content::cars().get("cars").and_then(|c| c.as_array()).cloned().unwrap_or_default()
}

/// A car id is a mod unless the catalogue says it is stock (unknown ids
/// count as mods, like the backend).
fn looks_mod(cid: &str, cat: &[Value], names: &serde_json::Map<String, Value>) -> bool {
    if let Some(hit) = cat.iter().find(|c| c.get("id").and_then(|v| v.as_str()) == Some(cid)) {
        return hit.get("mod").and_then(|v| v.as_bool()).unwrap_or(false);
    }
    let _ = names.contains_key(cid);
    true
}

fn chosen(profile: &Value) -> Vec<String> {
    profile.get("cars").and_then(|c| c.as_array())
        .map(|a| a.iter().filter_map(|c| c.as_str().filter(|s| !s.is_empty()).map(String::from)).collect())
        .unwrap_or_default()
}

pub fn infer_allow_kunos(profile: &Value, cat: &[Value], names: &serde_json::Map<String, Value>) -> bool {
    if let Some(v) = profile.get("allow_kunos") {
        if !v.is_null() {
            return match v {
                Value::Bool(b) => *b,
                Value::Number(n) => n.as_f64() != Some(0.0),
                Value::String(s) => !s.is_empty(),
                Value::Array(a) => !a.is_empty(),
                Value::Object(o) => !o.is_empty(),
                Value::Null => false,
            };
        }
    }
    let c = chosen(profile);
    c.is_empty() || c.iter().all(|x| looks_mod(x, cat, names))
}

pub fn infer_mods(profile: &Value, cat: &[Value], names: &serde_json::Map<String, Value>) -> String {
    let want = profile.get("mods").and_then(|v| v.as_str()).unwrap_or("").trim().to_lowercase();
    if ["all", "none", "pick"].contains(&want.as_str()) {
        return want;
    }
    if chosen(profile).iter().any(|c| looks_mod(c, cat, names)) {
        return "pick".into();
    }
    if infer_allow_kunos(profile, cat, names) { "all".into() } else { "none".into() }
}

/// What the dedicated server should actually accept. ⚠ Never empty: an
/// empty allow-list accepts nobody and fails joins with "wrong car".
pub fn allowed_car_ids(profile: &Value) -> Vec<String> {
    let cat = catalog();
    let names = mods::car_names();
    let kunos: Vec<String> = cat.iter().filter(|c| c.get("kunos") == Some(&json!(true)))
        .filter_map(|c| c.get("id").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(String::from)).collect();
    let mods_all: Vec<String> = names.keys().cloned().collect();
    let ch = chosen(profile);
    let allow_kunos = infer_allow_kunos(profile, &cat, &names);
    let m = infer_mods(profile, &cat, &names);
    let mut seen = HashSet::new();
    let mut out = vec![];
    let mut add = |c: &str, out: &mut Vec<String>| {
        if !c.is_empty() && seen.insert(c.to_string()) {
            out.push(c.to_string());
        }
    };
    if allow_kunos {
        for c in &kunos { add(c, &mut out) }
    } else {
        for c in &ch { if !looks_mod(c, &cat, &names) { add(c, &mut out) } }
    }
    if m == "all" {
        for c in &mods_all { add(c, &mut out) }
    } else if m == "pick" {
        for c in &ch { if looks_mod(c, &cat, &names) { add(c, &mut out) } }
    }
    if out.is_empty() {
        for c in &kunos { add(c, &mut out) }
    }
    out
}

pub fn car_policy(profile: &Value) -> Value {
    let cat = catalog();
    let names = mods::car_names();
    let allow_kunos = infer_allow_kunos(profile, &cat, &names);
    let m = infer_mods(profile, &cat, &names);
    let picked = chosen(profile);
    let extras: Vec<String> = picked.iter().filter(|c| looks_mod(c, &cat, &names)).cloned().collect();
    let (kind, label) = if allow_kunos && m == "all" {
        ("all", "all cars".to_string())
    } else if allow_kunos && m == "none" {
        ("kunos_only", "all Kunos, no mods".to_string())
    } else if allow_kunos {
        ("kunos_plus", format!("all Kunos + {} mod{}", extras.len(), if extras.len() == 1 { "" } else { "s" }))
    } else if m == "all" {
        ("mods_only", "mods only".to_string())
    } else if picked.is_empty() {
        ("kunos_only", "all Kunos, no mods".to_string())
    } else {
        ("only", format!("{}{}", picked.len(), if picked.len() == 1 { " car only" } else { " cars only" }))
    };
    json!({"kind": kind, "label": label, "allow_kunos": allow_kunos, "mods": m,
           "extras": extras, "count": allowed_car_ids(profile).len()})
}
