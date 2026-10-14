//! preset_<code>_mech_<n> -> ks_<model>, from the live archive - ported
//! from acecm/carsmap.py. Cached in <data>/preset_map.json (shared with the
//! backend, same shape and CACHE_V) against the archive's mtime and size.

use std::fs::File;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use regex_lite::Regex;
use serde_json::{json, Map, Value};

use crate::{detect, kspkg, mods, paths};

const CACHE_V: i64 = 3;

fn cache_path() -> std::path::PathBuf {
    paths::data().join("preset_map.json")
}

pub fn kspkg_path() -> String {
    for d in [detect::effective_server_dir(), detect::find("game_dir", false)] {
        if d.is_empty() {
            continue;
        }
        let c = Path::new(&d).join("content.kspkg");
        if c.is_file() {
            return c.to_string_lossy().into_owned();
        }
    }
    String::new()
}

fn key(path: &str) -> String {
    match std::fs::metadata(path) {
        Ok(m) => {
            let ns = m.modified().ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_nanos()).unwrap_or(0);
            format!("{path}|{ns}|{}", m.len())
        }
        Err(_) => path.to_string(),
    }
}

fn varint(b: &[u8], i: &mut usize) -> Option<u64> {
    let (mut r, mut s) = (0u64, 0u32);
    loop {
        let x = *b.get(*i)?;
        *i += 1;
        r |= ((x & 0x7f) as u64) << s.min(63);
        s += 7;
        if x & 0x80 == 0 {
            return Some(r);
        }
    }
}

/// brand / display name / variant (fields 1-3) and the first visual preset
/// a .mechanicalcarpreset points at. The game names its own trims.
fn preset_head(blob: &[u8]) -> Value {
    let mut out: [Option<String>; 4] = Default::default();
    let mut visual = String::new();
    let mut i = 0;
    while i < blob.len() {
        let Some(k) = varint(blob, &mut i) else { break };
        let (fnum, wt) = (k >> 3, k & 7);
        match wt {
            2 => {
                let Some(ln) = varint(blob, &mut i) else { break };
                let end = i.saturating_add(ln as usize);
                if end > blob.len() {
                    // python slices past the end silently - same result
                    let v = &blob[i..];
                    i = end;
                    if (1..=3).contains(&fnum) {
                        if let Ok(s) = std::str::from_utf8(v) { out[fnum as usize] = Some(s.trim().to_string()); }
                    }
                    continue;
                }
                let v = &blob[i..end];
                i = end;
                if (1..=3).contains(&fnum) {
                    if let Ok(s) = std::str::from_utf8(v) {
                        out[fnum as usize] = Some(s.trim().to_string());
                    }
                } else if visual.is_empty() {
                    if let Ok(t) = std::str::from_utf8(v) {
                        if t.to_lowercase().ends_with(".visualcarpreset") {
                            let name = t.replace('\\', "/");
                            let name = name.rsplit('/').next().unwrap_or("");
                            visual = name.rsplit_once('.').map(|(a, _)| a).unwrap_or(name).to_lowercase();
                        }
                    }
                }
            }
            0 => { if varint(blob, &mut i).is_none() { break } }
            5 => i += 4,
            1 => i += 8,
            _ => break,
        }
        if out.iter().filter(|x| x.is_some()).count() >= 3 && !visual.is_empty() {
            break;
        }
    }
    let g = |n: usize| out[n].clone().unwrap_or_default();
    json!({"brand": g(1), "name": g(2), "variant": g(3), "visual": visual})
}

struct Re {
    preset: Regex,
    stem: Regex,
}

fn re() -> &'static Re {
    static R: std::sync::OnceLock<Re> = std::sync::OnceLock::new();
    R.get_or_init(|| Re {
        preset: Regex::new(r"(?i)preset_[a-z0-9]+_mech_\d+").unwrap(),
        stem: Regex::new(r"(?i)^[a-z0-9_]+_mech_\d+$").unwrap(),
    })
}

/// Mechanical-preset ids encoded in a file name.
fn preset_ids(filename: &str) -> Vec<String> {
    let found: Vec<String> = re().preset.find_iter(filename).map(|m| m.as_str().to_lowercase()).collect();
    if !found.is_empty() {
        return found;
    }
    let stem = filename.rsplit_once('.').map(|(a, _)| a).unwrap_or(filename);
    if re().stem.is_match(stem) { vec![stem.to_lowercase()] } else { vec![] }
}

fn scan(path: &str) -> std::io::Result<Map<String, Value>> {
    let p = Path::new(path);
    let entries = kspkg::entries(p)?;
    let mut fh = File::open(p)?;
    let mut presets = Map::new();
    let mut variants = Map::new();
    let mut models = std::collections::BTreeMap::new();
    for e in &entries {
        let low = e.path.to_lowercase();
        let Some(rest) = low.strip_prefix("content\\cars\\") else { continue };
        let Some((model, _)) = rest.split_once('\\') else { continue };
        if model.is_empty() {
            continue;
        }
        let is_mech = low.ends_with(".mechanicalcarpreset");
        if is_mech || low.ends_with(".carfinalstate") {
            let base = kspkg::entry_name(&low);
            if is_mech {
                if let Ok(blob) = kspkg::read_entry(&mut fh, e) {
                    let head = preset_head(&blob);
                    if head["name"] != "" || head["variant"] != "" {
                        for pid in preset_ids(base) {
                            variants.insert(pid, head.clone());
                        }
                    }
                }
            }
            for pid in preset_ids(base) {
                presets.entry(pid).or_insert(json!(model));
            }
        }
        models.entry(model.to_string()).or_insert(());
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let mut out = Map::new();
    out.insert("kspkg".into(), json!(path));
    out.insert("built".into(), json!(now));
    out.insert("entries_seen".into(), json!(entries.len()));
    out.insert("presets".into(), Value::Object(presets));
    out.insert("variants".into(), Value::Object(variants));
    out.insert("models".into(), json!(models.keys().collect::<Vec<_>>()));
    Ok(out)
}

static MEM: Mutex<Option<(String, Value)>> = Mutex::new(None);

pub fn table(refresh: bool) -> Value {
    let path = kspkg_path();
    if path.is_empty() {
        return json!({"presets": {}, "models": [], "error": "content.kspkg not found"});
    }
    let k = key(&path);
    if !refresh {
        // ⚠ the backend's forget_lists() DELETES preset_map.json when content
        // changes (an install adds mod names). Our memory copy must not
        // outlive that, so a missing file is a miss even with a warm key.
        if let Some((mk, v)) = MEM.lock().unwrap().as_ref() {
            if *mk == k && cache_path().is_file() {
                return v.clone();
            }
        }
        if let Some(c) = std::fs::read(cache_path()).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) {
            if c.get("kspkg").and_then(|v| v.as_str()) == Some(&path)
                && c.get("key").and_then(|v| v.as_str()) == Some(&k)
                && c.get("v").and_then(|v| v.as_i64()) == Some(CACHE_V)
            {
                *MEM.lock().unwrap() = Some((k, c.clone()));
                return c;
            }
        }
    }
    let mut data = match scan(&path) {
        Ok(d) => d,
        Err(e) => return json!({"presets": {}, "models": [], "error": e.to_string()}),
    };
    data.insert("key".into(), json!(k));
    data.insert("v".into(), json!(CACHE_V));
    // mods declare their own ids and names
    data.insert("mod_names".into(), Value::Object(mods::car_names()));
    let v = Value::Object(data);
    let tmp = cache_path().with_extension("json.part");
    if std::fs::write(&tmp, serde_json::to_vec(&v).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&tmp, cache_path());
    }
    *MEM.lock().unwrap() = Some((k, v.clone()));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_from_names() {
        assert_eq!(preset_ids("preset_695b_mech_1.mechanicalcarpreset"), vec!["preset_695b_mech_1"]);
        assert_eq!(preset_ids("ks_foo_preset_r5gt_mech_1_preset_r5gt_visual_1.carfinalstate"),
                   vec!["preset_r5gt_mech_1"]);
        assert_eq!(preset_ids("ks_caterham_485_csr_mech_2.mechanicalcarpreset"),
                   vec!["ks_caterham_485_csr_mech_2"]);
        assert!(preset_ids("readme.txt").is_empty());
    }
}
