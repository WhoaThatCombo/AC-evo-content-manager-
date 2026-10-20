//! Where ACECM keeps things, and the few settings the shell itself needs.
//!
//! Mirrors acecm/config.py for an installed copy: everything lives under
//! %LOCALAPPDATA%\ACECM (config.json beside data\), never beside the exe.

use std::path::PathBuf;

pub fn root() -> PathBuf {
    if let Ok(r) = std::env::var("ACECM_ROOT") {
        return PathBuf::from(r);
    }
    let base = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    base.join(crate::edition::ROOT)
}

pub fn data() -> PathBuf {
    std::env::var("ACECM_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| root().join("data"))
}

/// The repo this shell was built from - where the Python backend lives
/// until the port is finished.
pub fn repo() -> PathBuf {
    if let Ok(r) = std::env::var("ACECM_REPO") {
        return PathBuf::from(r);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// config.json as JSON, or an empty object.
pub fn config() -> serde_json::Value {
    let path = root().join("config.json");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

pub struct Listen {
    pub host: String,
    pub port: u16,
}

/// The public port: the window, and other ACECMs asking us for content.
pub fn listen() -> Listen {
    let c = config();
    let port = c.get("ui_port").and_then(|v| v.as_u64()).unwrap_or(8092) as u16;
    let host = c
        .get("listen")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "0.0.0.0".to_string());
    Listen { host, port }
}
