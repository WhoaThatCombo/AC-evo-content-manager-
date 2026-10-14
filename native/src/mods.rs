//! Car mods in the server and client mod folders - ported from the read
//! side of acecm/install.py (mods_dir, installed, audit, car_names,
//! car_models). Installing/removing stays in the backend for now.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::{config, detect};

fn home() -> PathBuf {
    std::env::var("USERPROFILE").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("."))
}

fn show(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

pub fn mods_dir() -> String {
    let cfg = config::load();
    let s = config::get_str(&cfg, "mods_dir");
    if !s.is_empty() {
        return s;
    }
    let d = detect::find("server_mods", false);
    if !d.is_empty() {
        return d;
    }
    show(&home().join("Saved Games").join("ACE-Server").join("mods"))
}

pub fn client_mods_dir() -> String {
    let cfg = config::load();
    let s = config::get_str(&cfg, "client_mods_dir");
    if !s.is_empty() {
        return s;
    }
    let d = detect::find("client_mods", false);
    if !d.is_empty() {
        return d;
    }
    show(&home().join("Saved Games").join("ACE").join("mods"))
}

/// os.path.splitext: the last dot, unless it only leads the name.
fn splitext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if name[..i].chars().any(|c| c != '.') => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

fn size_mb(p: &Path) -> Value {
    let b = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0) as f64;
    json!((b / 1048576.0 * 10.0).round() / 10.0)
}

/// Every car mod in one mod folder, with health.
pub fn installed(which: &str) -> Value {
    let d = if which == "server" { mods_dir() } else { client_mods_dir() };
    let dir = Path::new(&d);
    if !dir.is_dir() {
        return json!({"dir": d, "mods": [], "note": "mod folder does not exist yet"});
    }
    let mut names = BTreeSet::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let f = e.file_name().to_string_lossy().into_owned();
            let (base, ext) = splitext(&f);
            let ext = ext.to_ascii_lowercase();
            if ext == ".kspkg" || ext == ".json" {
                names.insert(base.to_string());
            }
        }
    }
    let mut out = vec![];
    let mut usable_n = 0;
    for base in &names {
        let pkg = dir.join(format!("{base}.kspkg"));
        let meta = dir.join(format!("{base}.json"));
        let (has_pkg, has_meta) = (pkg.exists(), meta.exists());
        let mut cars = vec![];
        let mut err = Value::Null;
        if has_meta {
            match std::fs::read(&meta).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) {
                Some(v) => {
                    for c in v.get("cars").and_then(|c| c.as_array()).cloned().unwrap_or_default() {
                        let name = c.get("name").cloned().unwrap_or(Value::Null);
                        let label = match c.get("display_name") {
                            Some(Value::String(s)) if !s.is_empty() => Value::String(s.clone()),
                            _ => name.clone(),
                        };
                        cars.push(json!({"id": name, "label": label,
                                         "pi": c.get("performance_indicator").cloned().unwrap_or(Value::Null)}));
                    }
                }
                None => err = json!("unreadable .json"),
            }
        }
        let usable = has_pkg && has_meta && err.is_null();
        usable_n += usable as usize;
        let why = if has_pkg && has_meta { Value::Null }
                  else if !has_pkg { json!("missing .kspkg - nothing to load") }
                  else { json!("missing .json - the car will NOT appear in the car list") };
        out.push(json!({
            "name": base, "kspkg": has_pkg, "json": has_meta,
            "size_mb": if has_pkg { size_mb(&pkg) } else { json!(0) },
            "cars": cars, "error": err, "usable": usable, "why": why,
        }));
    }
    let total = out.len();
    json!({"dir": d, "side": which, "mods": out, "total": total, "usable": usable_n})
}

fn mods_of(v: &Value) -> Vec<Value> {
    v.get("mods").and_then(|m| m.as_array()).cloned().unwrap_or_default()
}

fn by_name(v: &Value) -> HashMap<String, Value> {
    mods_of(v).into_iter()
        .filter_map(|m| Some((m.get("name")?.as_str()?.to_string(), m)))
        .collect()
}

fn fix_hint(name: &str, s: Option<&Value>, c: Option<&Value>) -> Value {
    let b = |m: Option<&Value>, k: &str| m.and_then(|m| m.get(k)).and_then(|v| v.as_bool()).unwrap_or(false);
    let srv = Path::new(&mods_dir()).join(format!("{name}.json"));
    let cli = Path::new(&client_mods_dir()).join(format!("{name}.json"));
    if c.is_some() && b(c, "kspkg") && !b(c, "json") && s.is_some() && b(s, "json") {
        return json!({"what": "copy the .json from the server to the client",
                      "src": show(&srv), "dst": show(&cli)});
    }
    if s.is_some() && b(s, "kspkg") && !b(s, "json") && c.is_some() && b(c, "json") {
        return json!({"what": "copy the .json from the client to the server",
                      "src": show(&cli), "dst": show(&srv)});
    }
    Value::Null
}

/// Compare both sides and name what is actually wrong.
pub fn audit() -> Value {
    let srv = installed("server");
    let cli = installed("client");
    let (bs, bc) = (by_name(&srv), by_name(&cli));
    let names: BTreeSet<&String> = bs.keys().chain(bc.keys()).collect();
    let mut rows = vec![];
    let mut problems = 0;
    for name in names {
        let (s, c) = (bs.get(name), bc.get(name));
        let mut issues: Vec<String> = vec![];
        for (side, m) in [("server", s), ("client", c)] {
            if let Some(w) = m.and_then(|m| m.get("why")).and_then(|w| w.as_str()) {
                issues.push(format!("{side}: {w}"));
            }
        }
        if s.is_some() && c.is_none() {
            issues.push("only on the server - you cannot load this car".into());
        }
        if c.is_some() && s.is_none() {
            issues.push("only on the client - the server cannot host it".into());
        }
        let ok_of = |m: Option<&Value>| m.and_then(|m| m.get("usable")).and_then(|v| v.as_bool()).unwrap_or(false);
        let first = s.or(c);
        problems += !issues.is_empty() as usize;
        rows.push(json!({
            "name": name, "server": s.is_some(), "client": c.is_some(),
            "server_ok": ok_of(s), "client_ok": ok_of(c),
            "cars": first.and_then(|m| m.get("cars")).cloned().unwrap_or(json!([])),
            "size_mb": first.and_then(|m| m.get("size_mb")).cloned().unwrap_or(json!(0)),
            "ok": issues.is_empty(),
            "issues": issues,
            "fix": fix_hint(name, s, c),
        }));
    }
    let total = rows.len();
    json!({"server_dir": srv["dir"], "client_dir": cli["dir"], "mods": rows,
           "total": total, "problems": problems})
}

/// (packaged names, both listings): a mod is real when it has a package on
/// EITHER side - a manifest left behind after deleting a car contributes
/// nothing, but a package on one side and the manifest on the other counts.
fn listings() -> (HashSet<String>, Vec<Vec<Value>>) {
    let l = vec![mods_of(&installed("server")), mods_of(&installed("client"))];
    let mut packaged = HashSet::new();
    for side in &l {
        for m in side {
            if m.get("kspkg").and_then(|v| v.as_bool()).unwrap_or(false) {
                if let Some(n) = m.get("name").and_then(|v| v.as_str()) {
                    packaged.insert(n.to_string());
                }
            }
        }
    }
    (packaged, l)
}

/// preset id -> display name, from installed mod manifests.
pub fn car_names() -> Map<String, Value> {
    let (packaged, l) = listings();
    let mut names = Map::new();
    for side in &l {
        for m in side {
            let Some(n) = m.get("name").and_then(|v| v.as_str()) else { continue };
            if !packaged.contains(n) {
                continue;
            }
            for c in m.get("cars").and_then(|c| c.as_array()).into_iter().flatten() {
                if let Some(id) = c.get("id").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
                    if !names.contains_key(id) {
                        names.insert(id.to_string(), c.get("label").cloned().unwrap_or(Value::Null));
                    }
                }
            }
        }
    }
    names
}

/// preset id -> the mod folder (== car id) that ships it.
pub fn car_models() -> Map<String, Value> {
    let (packaged, l) = listings();
    let mut out = Map::new();
    for side in &l {
        for m in side {
            let Some(n) = m.get("name").and_then(|v| v.as_str()) else { continue };
            if !packaged.contains(n) {
                continue;
            }
            for c in m.get("cars").and_then(|c| c.as_array()).into_iter().flatten() {
                if let Some(id) = c.get("id").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
                    out.entry(id.to_string()).or_insert(json!(n));
                }
            }
        }
    }
    out
}
