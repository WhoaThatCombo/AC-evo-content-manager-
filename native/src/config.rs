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

/// DEFAULTS, overlaid with the file (unknown keys in the file are kept).
pub fn load() -> Map<String, Value> {
    let mut cfg = defaults();
    if let Some(Value::Object(file)) = crate::jsoncache::read(&path()) {
        for (k, v) in file {
            cfg.insert(k, v);
        }
    }
    cfg
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
    Value::Object(out)
}

/// Persist a settings change - only keys we know about.
pub fn save(patch: &Value) -> std::io::Result<Value> {
    let mut cfg = load();
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
    write(&cfg)?;
    Ok(public_cfg(&cfg))
}

fn write(cfg: &Map<String, Value>) -> std::io::Result<()> {
    let p = path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    // write-then-rename: a reader (the Python backend) never sees half a file
    let tmp = p.with_extension("json.part");
    std::fs::write(&tmp, serde_json::to_string_pretty(&Value::Object(cfg.clone()))?)?;
    let r = std::fs::rename(&tmp, &p);
    crate::jsoncache::forget(&p);
    r
}
