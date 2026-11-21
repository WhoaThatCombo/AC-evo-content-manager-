//! Settings - ported from acecm/config.py (DEFAULTS, load, save, public_cfg).
//!
//! ⚠ config.json is SHARED with the Python backend while the port is in
//! progress. Python re-reads it when its mtime changes (config.refresh), so a
//! save here is seen there on its next request.

use serde_json::{json, Map, Value};
use std::path::PathBuf;

use crate::paths;

pub fn defaults() -> Map<String, Value> {
    let v = json!({
        "ui_theme": "",
        "direct_launch_refused": false,
        "remote_admin": false,
        "fast_boot": false,
        "startup_scene": "menu",
        "hud_mph": false,
        "pit_btn_kind": "",
        "pit_btn_id": "",
        "pit_btn_mask": 0,
        "pit_btn_label": "",
        "pit_key": "ctrl+y",
        "server_dir": "",
        "server_exe": "",
        "tools_dir": "",
        "game_exe": "",
        // the EVO install folder (content.kspkg); empty = detected
        "game_dir": "",
        "ace_dir": "",
        "mods_dir": "",
        "client_mods_dir": "",
        "steam_appid": "3058630",
        "update_repo": "",
        "update_url": "",
        "update_token": "",
        "ui_port": crate::edition::PORT,
        "listen": "0.0.0.0",
        "telemetry_port": 8091,
        "backend_port": 448,
        // Convert page settings (convert.rs): ac1_root, out_dir, threads, memory_gb
        "convert": {},
        // the evoview tab's last Drive choices (evodrive.rs, 14d-evoview.js)
        "evoview": {},
        "backend_listen": "127.0.0.1",
        "auto_proxy": true,
        "default_tcp_port": 9700,
        "default_http_port": 8080
    });
    match v {
        Value::Object(m) => m,
        _ => unreachable!(),
    }
}

pub fn path() -> PathBuf {
    // ⚠ follows ACECM_DATA when that is pinned, like config.config_path
    if std::env::var("ACECM_DATA").is_ok() {
        return paths::data().join("config.json");
    }
    paths::root().join("config.json")
}

/// The file's settings: a JSON object (a UTF-8 BOM from Notepad allowed).
fn parse(b: &[u8]) -> Result<Map<String, Value>, String> {
    let b = b.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(b);
    match serde_json::from_slice::<Value>(b) {
        Ok(Value::Object(m)) => Ok(m),
        Ok(_) => Err("not a JSON object".into()),
        Err(e) => Err(e.to_string()),
    }
}

/// None when config.json is missing or fine; why it is unusable otherwise.
pub fn problem() -> Option<String> {
    problem_at(&path())
}

fn problem_at(p: &std::path::Path) -> Option<String> {
    if let Some(Value::Object(_)) = crate::jsoncache::read(p) {
        return None;
    }
    match std::fs::read(p) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => Some(format!("cannot read {}: {e}", p.display())),
        Ok(b) => parse(&b).err().map(|e| format!("{} does not parse ({e})", p.display())),
    }
}

/// DEFAULTS, overlaid with the file (unknown keys in the file are kept).
/// ⚠ An unparseable file runs on defaults but is never saved over (save).
pub fn load() -> Map<String, Value> {
    load_at(&path())
}

fn load_at(p: &std::path::Path) -> Map<String, Value> {
    let mut cfg = defaults();
    let file = match crate::jsoncache::read(p) {
        Some(Value::Object(m)) => Some(m),
        // the slow path only for a file the cache could not take (a BOM)
        _ => std::fs::read(p).ok().and_then(|b| parse(&b).ok()),
    };
    for (k, v) in file.unwrap_or_default() {
        cfg.insert(k, v);
    }
    cfg
}

/// Keep the unusable file as config.json.bad-<time> (once per content).
fn keep_bad_copy(p: &std::path::Path) -> Option<PathBuf> {
    let b = std::fs::read(p).ok()?;
    let name = p.file_name()?.to_string_lossy().into_owned();
    let dir = p.parent()?;
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if n.starts_with(&format!("{name}.bad-")) && std::fs::read(e.path()).ok().as_deref() == Some(&b[..]) {
            return Some(e.path());
        }
    }
    let bad = dir.join(format!("{name}.bad-{}", chrono::Local::now().format("%Y%m%d-%H%M%S")));
    crate::safefs::write_atomic(&bad, &b).ok()?;
    Some(bad)
}

pub fn get_str(cfg: &Map<String, Value>, key: &str) -> String {
    cfg.get(key).and_then(|v| v.as_str()).unwrap_or("").trim().to_string()
}

/// Settings for the UI. Never echo the GitHub token.
pub fn public_cfg(cfg: &Map<String, Value>) -> Value {
    let mut out = cfg.clone();
    let set = !get_str(cfg, "update_token").is_empty();
    out.insert("update_token".into(), json!(""));
    out.insert("update_token_set".into(), json!(set));
    if let Some(why) = problem() {
        out.insert("config_error".into(), json!(format!("{why} - ACECM is running on default settings and will not save over the file until it is fixed (or deleted)")));
    }
    Value::Object(out)
}

/// Persist a settings change - only keys we know about.
/// ⚠ Refused while config.json exists but does not parse: load() runs on
/// defaults then, and saving "defaults + this change" (several savers run
/// on their own) wiped every path and port the user had set.
pub fn save(patch: &Value) -> std::io::Result<Value> {
    save_at(&path(), patch)
}

fn save_at(p: &std::path::Path, patch: &Value) -> std::io::Result<Value> {
    if let Some(why) = problem_at(p) {
        let kept = keep_bad_copy(p).map(|b| format!("; a copy was kept as {}", b.display())).unwrap_or_default();
        return Err(std::io::Error::other(format!("{why} - not saving over it{kept}. Fix the file (or delete it to start from defaults), then try again")));
    }
    let mut cfg = load_at(p);
    let known = defaults();
    if let Value::Object(p) = patch {
        for (k, v) in p {
            if !known.contains_key(k) {
                continue;
            }
            // an empty token from the form means "leave the stored one"
            if k == "update_token" && v.as_str().map(|s| s.trim().is_empty()).unwrap_or(true) {
                continue;
            }
            cfg.insert(k.clone(), v.clone());
        }
    }
    write(p, &cfg)?;
    Ok(public_cfg(&cfg))
}

fn write(p: &std::path::Path, cfg: &Map<String, Value>) -> std::io::Result<()> {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    // temp + fsync + rename: a reader never sees half a file, two writers
    // never share one temp name, and a power cut leaves the old file
    let r = crate::safefs::write_atomic(p, serde_json::to_string_pretty(&Value::Object(cfg.clone()))?.as_bytes());
    crate::jsoncache::forget(p);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_broken_file_is_kept_not_overwritten() {
        let dir = crate::safefs::test_dir().join(format!("cfg_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.json");
        // missing: saves fine
        assert!(problem_at(&p).is_none());
        save_at(&p, &json!({"server_dir": "D:/srv"})).unwrap();
        assert_eq!(get_str(&load_at(&p), "server_dir"), "D:/srv");
        // a hand-edit typo: loads as defaults, every save is refused, the file survives
        let broken = b"{\"server_dir\": \"D:/srv\", \"game_dir\": \"E:/evo\",}".to_vec();
        std::fs::write(&p, &broken).unwrap();
        assert!(problem_at(&p).is_some());
        assert_eq!(get_str(&load_at(&p), "server_dir"), "");
        let e = save_at(&p, &json!({"remote_admin": true})).unwrap_err().to_string();
        assert!(e.contains("not saving"), "{e}");
        assert_eq!(std::fs::read(&p).unwrap(), broken);
        let bads: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("config.json.bad-")).collect();
        assert_eq!(bads.len(), 1);
        assert_eq!(std::fs::read(bads[0].path()).unwrap(), broken);
        // a second refusal does not pile up copies
        let _ = save_at(&p, &json!({"remote_admin": true}));
        assert_eq!(std::fs::read_dir(&dir).unwrap().flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("config.json.bad-")).count(), 1);
        // a Notepad BOM is fine
        std::fs::write(&p, b"\xEF\xBB\xBF{\"server_dir\": \"X:/s\"}").unwrap();
        assert!(problem_at(&p).is_none());
        assert_eq!(get_str(&load_at(&p), "server_dir"), "X:/s");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
