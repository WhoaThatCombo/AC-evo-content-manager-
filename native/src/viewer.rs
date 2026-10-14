//! Which cars have geometry, across the base game and every installed mod -
//! ported from viewer.packages / viewer.index. Cached in
//! <data>/viewer/index.json (shared with the backend, same stamp format).
//! Extraction / the evoview launch stay in the backend for now.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde_json::{json, Map, Value};

use crate::{detect, kspkg, mods, paths};

fn root() -> PathBuf {
    let d = paths::data().join("viewer");
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn cars_root() -> PathBuf {
    root().join("cars")
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// The client's content.kspkg (fresh detection, like viewer.package).
pub fn package() -> Option<String> {
    let g = detect::steam_app_dir(detect::APPID_GAME);
    let g = if g.is_empty() { detect::find("game_dir", true) } else { g };
    if g.is_empty() {
        return None;
    }
    let p = Path::new(&g).join("content.kspkg");
    p.is_file().then(|| s(&p))
}

/// (mod name or "", package path): the base game, then each mod package
/// (client folder first, a name seen once).
pub fn packages() -> Vec<(String, String)> {
    let mut out = vec![];
    if let Some(b) = package() {
        out.push((String::new(), b));
    }
    let mut seen = HashSet::new();
    for d in [mods::client_mods_dir(), mods::mods_dir()] {
        let dir = Path::new(&d);
        if d.is_empty() || !dir.is_dir() {
            continue;
        }
        let mut names: Vec<String> = std::fs::read_dir(dir).map(|rd| {
            rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()
        }).unwrap_or_default();
        names.sort();
        for f in names {
            if !f.to_lowercase().ends_with(".kspkg") {
                continue;
            }
            if !seen.insert(f.to_lowercase()) {
                continue;
            }
            let stem = f.rsplit_once('.').map(|(a, _)| a.to_string()).unwrap_or(f.clone());
            out.push((stem, s(&dir.join(&f))));
        }
    }
    out
}

fn mtime_secs(m: &std::fs::Metadata) -> u64 {
    m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0)
}

fn stamp(path: &str) -> String {
    std::fs::metadata(path).map(|m| format!("{}:{}", m.len(), mtime_secs(&m))).unwrap_or_default()
}

pub fn pretty(car_id: &str) -> String {
    let s = car_id.strip_prefix("ks_").unwrap_or(car_id);
    s.split('_').filter(|w| !w.is_empty()).map(|w| {
        if w.chars().count() <= 3 && w.chars().all(|c| c.is_alphabetic()) {
            w.to_uppercase()
        } else {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
                None => String::new(),
            }
        }
    }).collect::<Vec<_>>().join(" ")
}

fn mark(cars: &mut [Value]) {
    // recomputed on every read: it changes whenever a car is viewed
    let root = cars_root();
    for c in cars.iter_mut() {
        let id = c.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        c["extracted"] = json!(root.join(&id).is_dir());
    }
}

/// Every car that has geometry, cached on the packages AND the mod folders.
pub fn index(refresh: bool) -> Value {
    let pkgs = packages();
    if pkgs.is_empty() {
        return json!({"error": "game install not found - set the game folder in Settings", "cars": []});
    }
    let cache = root().join("index.json");
    let mut parts: Vec<String> = pkgs.iter().map(|(_, p)| {
        let base = Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        format!("{base}:{}", stamp(p))
    }).collect();
    for d in [mods::client_mods_dir(), mods::mods_dir()] {
        if d.is_empty() || !Path::new(&d).is_dir() {
            continue;
        }
        if let Ok(m) = std::fs::metadata(&d) {
            let n = std::fs::read_dir(&d).map(|r| r.count()).unwrap_or(0);
            parts.push(format!("{d}:{}:{n}", mtime_secs(&m)));
        }
    }
    let st = parts.join("|");
    if !refresh {
        if let Some(got) = std::fs::read(&cache).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) {
            if got.get("stamp").and_then(|v| v.as_str()) == Some(&st) {
                let mut cars = got.get("cars").and_then(|c| c.as_array()).cloned().unwrap_or_default();
                mark(&mut cars);
                return json!({"cars": cars, "cached": true});
            }
        }
    }
    // car id -> (mod, pkg); first package wins, like setdefault
    let mut found: BTreeMap<String, (String, String)> = BTreeMap::new();
    let mut order: Vec<String> = vec![];
    let mut bad = vec![];
    for (m, pkg) in &pkgs {
        // ⚠ one unreadable package must not empty the whole list
        match kspkg::entries(Path::new(pkg)) {
            Ok(es) => {
                for e in es {
                    if e.size == 0 {
                        continue;
                    }
                    let low = e.path.to_lowercase();
                    if !low.starts_with("content\\cars\\") || !low.contains("\\meshes\\") || !low.ends_with(".mesh") {
                        continue;
                    }
                    let parts: Vec<&str> = e.path.split('\\').collect();
                    if parts.len() > 2 && !parts[2].to_lowercase().starts_with("common") {
                        let id = parts[2].to_string();
                        if !found.contains_key(&id) {
                            order.push(id.clone());
                            found.insert(id, (m.clone(), pkg.clone()));
                        }
                    }
                }
            }
            Err(_) => bad.push(Path::new(pkg).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
        }
    }
    // a mod's real name is declared against its PRESET id; take the label
    // from the mod's own manifest for the folder id we list cars by
    let mut labels: Map<String, Value> = mods::car_names();
    for which in ["client", "server"] {
        let inst = mods::installed(which);
        for m in inst.get("mods").and_then(|v| v.as_array()).into_iter().flatten() {
            let Some(name) = m.get("name").and_then(|v| v.as_str()) else { continue };
            for c in m.get("cars").and_then(|v| v.as_array()).into_iter().flatten() {
                if let Some(l) = c.get("label").filter(|l| l.as_str().map(|s| !s.is_empty()).unwrap_or(!l.is_null())) {
                    labels.entry(name.to_string()).or_insert(l.clone());
                    break;
                }
            }
        }
    }
    let mut cars: Vec<Value> = order.iter().map(|id| {
        let (m, pkg) = &found[id];
        let label = match labels.get(id) {
            Some(Value::String(s)) if !s.is_empty() => json!(s),
            Some(v) if !v.is_null() && !v.is_string() => v.clone(),
            _ => json!(pretty(id)),
        };
        json!({"id": id, "label": label, "mod": !m.is_empty(), "pkg": pkg})
    }).collect();
    cars.sort_by(|a, b| {
        let k = |c: &Value| (c["mod"].as_bool().unwrap_or(false), c["label"].as_str().unwrap_or("").to_string());
        k(a).cmp(&k(b))
    });
    let _ = std::fs::write(&cache, serde_json::to_vec(&json!({"stamp": st, "cars": cars})).unwrap_or_default());
    mark(&mut cars);
    json!({"cars": cars, "cached": false, "unreadable": bad})
}
