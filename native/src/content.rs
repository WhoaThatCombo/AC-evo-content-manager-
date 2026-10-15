//! Car and track inventory - ported from acecm/content.py (cars, tracks).
//!
//! cars   from the server's cars.json, split into Kunos presets
//!        (`<code>_mech_<n>`) and mods, plus cars only an installed mod
//!        declares; named from the mods' manifests and the archive.
//! tracks from events_practice.json - the index IS the launcher's
//!        EVENT_IDX - plus imported tracks (index 10000+).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use regex_lite::Regex;
use serde_json::{json, Map, Value};

use crate::{carsmap, detect, mods, paths, trackimport, viewer};

fn re(p: &'static str) -> &'static Regex {
    use std::sync::OnceLock;
    static C: OnceLock<Mutex<HashMap<&'static str, &'static Regex>>> = OnceLock::new();
    let m = C.get_or_init(|| Mutex::new(HashMap::new()));
    let mut g = m.lock().unwrap();
    *g.entry(p).or_insert_with(|| Box::leak(Box::new(Regex::new(p).unwrap())))
}

/// cars.json / events_*.json: the server folder, else the bundled copy.
pub fn catalog_path(name: &str) -> String {
    let n = Path::new(name).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let bundled = crate::bundle::tools_dir();
    for base in [PathBuf::from(detect::effective_server_dir()), bundled] {
        if base.as_os_str().is_empty() {
            continue;
        }
        let p = base.join(&n);
        if p.is_file() {
            return p.to_string_lossy().into_owned();
        }
    }
    String::new()
}

fn capitalize(w: &str) -> String {
    let mut c = w.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
        None => String::new(),
    }
}

/// ks_bmw_m4_gt3_mech_0 -> BMW M4 GT3.
pub fn pretty(name: &str) -> String {
    let s = re(r"_mech_\d+$").replace(name, "").into_owned();
    let s = re(r"^(ks|preset)_").replace(&s, "").into_owned();
    s.split('_').filter(|p| !p.is_empty()).map(|p| {
        let low = p.to_lowercase();
        if p.chars().count() <= 3 && p.chars().all(|c| c.is_alphabetic()) && low != "the" && low != "and" {
            p.to_uppercase()
        } else {
            capitalize(p)
        }
    }).collect::<Vec<_>>().join(" ")
}

fn first_word(s: &str) -> String {
    s.split(' ').next().unwrap_or("").to_string()
}

fn mech_n(id: &str) -> u64 {
    re(r"_mech_(\d+)$").captures(id).and_then(|c| c.get(1)).and_then(|m| m.as_str().parse().ok()).unwrap_or(0)
}

// ------------------------------------------------------------------ cache --
// Keyed on what the lists are derived from: the catalogues and the server
// archive by stat, the mod / imported-track folders by LISTING (an install
// or delete changes no file we already read).
fn stat_part(p: &str) -> String {
    std::fs::metadata(p).map(|m| {
        let ns = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos()).unwrap_or(0);
        format!("{ns}/{}", m.len())
    }).unwrap_or_else(|_| "-".into())
}

fn dir_part(d: &str) -> String {
    let mut v: Vec<String> = std::fs::read_dir(d).map(|rd| rd.flatten().map(|e| {
        let m = e.metadata().ok();
        format!("{}:{}:{}", e.file_name().to_string_lossy(),
                m.as_ref().map(|m| m.len()).unwrap_or(0),
                m.and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_nanos()).unwrap_or(0))
    }).collect()).unwrap_or_default();
    v.sort();
    v.join(",")
}

fn content_key() -> String {
    let srv = detect::effective_server_dir();
    let pkg = if srv.is_empty() { String::new() }
              else { Path::new(&srv).join("content.kspkg").to_string_lossy().into_owned() };
    [stat_part(&catalog_path("cars.json")), stat_part(&catalog_path("events_practice.json")),
     stat_part(&pkg), dir_part(&mods::mods_dir()), dir_part(&mods::client_mods_dir()),
     dir_part(&trackimport::tracks_dir()),
     // ⚠ the backend's forget_lists() deletes these on a content change
     stat_part(&paths::data().join("preset_map.json").to_string_lossy()),
     stat_part(&paths::data().join("viewer").join("index.json").to_string_lossy())]
        .join("|")
}

static CACHE: Mutex<Option<(String, Value, Value)>> = Mutex::new(None);

fn cached(which: usize) -> Value {
    let k = content_key();
    if let Some((ck, c, t)) = CACHE.lock().unwrap().as_ref() {
        if *ck == k {
            return if which == 0 { c.clone() } else { t.clone() };
        }
    }
    let (c, t) = (cars_uncached(), tracks_uncached());
    *CACHE.lock().unwrap() = Some((k, c.clone(), t.clone()));
    if which == 0 { c } else { t }
}

/// THE "content changed" hook (content.forget_lists): drop the cached
/// inventories and the files a stat key cannot see through.
pub fn forget_lists() {
    *CACHE.lock().unwrap() = None;
    for f in [paths::data().join("track_map.json"), paths::data().join("viewer").join("index.json"), paths::data().join("preset_map.json")] {
        let _ = std::fs::remove_file(f);
    }
    crate::carsmap::forget();
}

/// Real car model names harvested from the server log ("on car <model>,").
/// The two id spaces are NOT mapped to each other here.
pub fn models_seen() -> Value {
    let log = crate::serverproc::server_log("");
    let Ok(b) = std::fs::read(&log) else { return json!({"models": [], "note": "no server log yet"}) };
    let txt = String::from_utf8_lossy(&b);
    let mut seen = std::collections::BTreeSet::new();
    for (i, _) in txt.match_indices("on car ") {
        let rest = &txt[i + 7..];
        let name: String = rest.chars().take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_').collect();
        if !name.is_empty() && rest[name.len()..].starts_with(',') {
            seen.insert(name);
        }
    }
    let total = seen.len();
    json!({"models": seen.into_iter().map(|m| json!({"id": m, "label": pretty(&m)})).collect::<Vec<_>>(), "total": total})
}

pub fn cars() -> Value {
    cached(0)
}

pub fn tracks() -> Value {
    cached(1)
}

// ------------------------------------------------------------------- cars --
fn cars_uncached() -> Value {
    let path = catalog_path("cars.json");
    let raw = match std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.get("cars").and_then(|c| c.as_array()).cloned())
    {
        Some(r) => r,
        None => return json!({"error": "cannot read cars.json", "cars": []}),
    };
    let declared = mods::car_names();
    let table = carsmap::table(false);
    let presets = table.get("presets").and_then(|p| p.as_object()).cloned().unwrap_or_default();
    let variants = table.get("variants").and_then(|p| p.as_object()).cloned().unwrap_or_default();
    let variant_of = |id: &str| -> String {
        variants.get(id).and_then(|v| v.get("variant")).and_then(|v| v.as_str()).unwrap_or("").to_string()
    };
    let viewer_ids: HashSet<String> = viewer::index(false).get("cars").and_then(|c| c.as_array())
        .map(|a| a.iter().filter_map(|c| c.get("id")?.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let mod_models = mods::car_models();
    let guess_model = |name: &str| -> String {
        if let Some(h) = mod_models.get(name).and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
            return h.to_string();
        }
        if viewer_ids.contains(name) {
            return name.to_string();
        }
        let code = re(r"^preset_|_mech_\d+$").replace_all(name, "").into_owned();
        for cand in [format!("ks_{code}"), code.clone()] {
            if viewer_ids.contains(&cand) {
                return cand;
            }
        }
        if code.is_empty() {
            return String::new();
        }
        let hits: Vec<&String> = viewer_ids.iter().filter(|k| k.contains(&code)).collect();
        if hits.len() == 1 { hits[0].clone() } else { String::new() }
    };
    let preset_model = |id: &str| -> String {
        presets.get(id).or_else(|| presets.get(&id.to_lowercase()))
            .and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(String::from)
            .unwrap_or_else(|| guess_model(id))
    };
    let mech = re(r"^.+_mech_\d+$");
    let mut out: Vec<Map<String, Value>> = vec![];
    for c in &raw {
        let name = c.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let is_kunos = mech.is_match(&name);
        let model = preset_model(&name);
        let label = if let Some(l) = declared.get(&name) {
            l.as_str().unwrap_or("").to_string()
        } else if !model.is_empty() {
            pretty(&model)
        } else {
            pretty(&name)
        };
        let mut row = Map::new();
        row.insert("id".into(), json!(name));
        row.insert("model".into(), json!(model));
        row.insert("label".into(), json!(label));
        row.insert("base_label".into(), json!(label));
        row.insert("variant".into(), json!(variant_of(&name)));
        row.insert("named".into(), json!(declared.contains_key(&name) || !model.is_empty()));
        row.insert("brand".into(), json!(first_word(&label)));
        row.insert("kunos".into(), json!(is_kunos));
        row.insert("mod".into(), json!(!is_kunos));
        out.push(row);
    }
    // cars shipped only by an installed mod are not in cars.json at all
    let known: HashSet<String> = out.iter().filter_map(|c| c["id"].as_str().map(String::from)).collect();
    for (cid, label) in &declared {
        if known.contains(cid) {
            continue;
        }
        let l = label.as_str().unwrap_or("").to_string();
        let mut row = Map::new();
        row.insert("id".into(), json!(cid));
        row.insert("model".into(), json!(preset_model(cid)));
        row.insert("label".into(), json!(l));
        row.insert("brand".into(), json!(first_word(&l)));
        row.insert("kunos".into(), json!(false));
        row.insert("mod".into(), json!(true));
        row.insert("named".into(), json!(true));
        row.insert("from_mod".into(), json!(true));
        out.push(row);
    }
    // disambiguate only rows that actually share a car
    let key_of = |c: &Map<String, Value>| -> String {
        match c.get("model").and_then(|v| v.as_str()) {
            Some(m) if !m.is_empty() => m.to_string(),
            _ => c["id"].as_str().unwrap_or("").to_string(),
        }
    };
    let mut per: HashMap<String, usize> = HashMap::new();
    for c in &out {
        *per.entry(key_of(c)).or_insert(0) += 1;
    }
    for c in out.iter_mut() {
        let var = c.get("variant").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if var.is_empty() || per.get(&key_of(c)).copied().unwrap_or(0) < 2 {
            c.insert("variant".into(), json!(""));
            continue;
        }
        let base = match c.get("base_label").and_then(|v| v.as_str()) {
            Some(b) if !b.is_empty() => b.to_string(),
            _ => c["label"].as_str().unwrap_or("").to_string(),
        };
        if !base.to_lowercase().ends_with(&var.to_lowercase()) {
            c.insert("label".into(), json!(format!("{base} · {var}")));
        }
    }
    // base name, then the preset number: variants stay in the game's order
    let base_of = |c: &Map<String, Value>| -> String {
        match c.get("base_label").and_then(|v| v.as_str()) {
            Some(b) if !b.is_empty() => b.to_lowercase(),
            _ => c["label"].as_str().unwrap_or("").to_lowercase(),
        }
    };
    out.sort_by(|a, b| {
        let ka = (a["kunos"] != true, base_of(a), mech_n(a["id"].as_str().unwrap_or("")));
        let kb = (b["kunos"] != true, base_of(b), mech_n(b["id"].as_str().unwrap_or("")));
        ka.cmp(&kb)
    });
    let count = |k: &str| out.iter().filter(|c| c.get(k) == Some(&json!(true))).count();
    let (k, m, f) = (count("kunos"), count("mod"), count("from_mod"));
    let total = out.len();
    json!({"cars": out, "total": total, "kunos": k, "mods": m, "from_mods": f})
}

// ----------------------------------------------------------------- tracks --
fn tracks_uncached() -> Value {
    let path = catalog_path("events_practice.json");
    let evs = match std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.get("events").and_then(|e| e.as_array()).cloned())
    {
        Some(e) => e,
        None => return json!({"error": "cannot read events_practice.json", "tracks": []}),
    };
    let s = |v: &Value, k: &str, d: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or(d).to_string();
    let lay = re(r"^layout_");
    let mut out = vec![];
    let mut seen = HashSet::new();
    for (i, e) in evs.iter().enumerate() {
        let track = s(e, "track", "?");
        let layout = s(e, "layout", "?");
        let name = match e.get("event_name").and_then(|v| v.as_str()) {
            Some(n) if !n.is_empty() => n.to_string(),
            _ => pretty(&s(e, "track", "")),
        };
        let label = format!("{} - {}", pretty(&s(e, "track", "")),
                            pretty(&lay.replace(&s(e, "layout", ""), "")));
        seen.insert(track.to_lowercase());
        out.push(json!({"index": i, "track": track, "layout": layout, "name": name,
                        "label": label, "length_m": e.get("track_length").cloned().unwrap_or(Value::Null),
                        "mod": false}));
    }
    for (n, t) in trackimport::importable().iter().enumerate() {
        let name = match t.get("display_name").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
            Some(d) => d.trim().to_string(),
            None => t.get("folder").and_then(|v| v.as_str()).unwrap_or("").trim().to_string(),
        };
        if name.is_empty() || seen.contains(&name.to_lowercase()) {
            continue;
        }
        if t.get("ok") != Some(&json!(true)) {
            continue;
        }
        let layout = t.get("layout").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let label = if layout.is_empty() { name.clone() }
                    else { format!("{name} - {}", pretty(&lay.replace(&layout, ""))) };
        out.push(json!({"index": 10000 + n, "track": name, "layout": layout, "name": name,
                        "label": label, "length_m": null, "mod": true, "custom_track": name,
                        "folder": t.get("folder").cloned().unwrap_or(json!(""))}));
        seen.insert(name.to_lowercase());
    }
    let total = out.len();
    json!({"tracks": out, "total": total})
}
