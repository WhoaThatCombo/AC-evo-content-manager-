//! Small file-backed reads: favourites, server track folders, LAN address.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::{json, Value};

use crate::{detect, paths};

/// Servers the user pinned (drive.favourites). Writes stay in the backend.
pub fn favourites() -> Value {
    let items: Vec<Value> = std::fs::read(paths::data().join("favourites.json")).ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.as_array().cloned()).unwrap_or_default();
    let keep: Vec<Value> = items.into_iter().filter(|f| {
        match f.get("ip") {
            Some(Value::String(s)) => !s.is_empty(),
            Some(Value::Null) | None => false,
            Some(Value::Bool(b)) => *b,
            Some(Value::Number(n)) => n.as_f64() != Some(0.0),
            Some(Value::Array(a)) => !a.is_empty(),
            Some(Value::Object(o)) => !o.is_empty(),
        }
    }).collect();
    json!({"ok": true, "favourites": keep})
}

/// Track folders on the server and whether they have the loose AI splines
/// (install.tracks_installed). ⚠ The server's content.kspkg ships neither
/// spline file, so a track without them hosts fine but runs no AI.
pub fn tracks_installed() -> Value {
    let root = Path::new(&detect::effective_server_dir()).join("content").join("tracks");
    let rs = root.to_string_lossy().into_owned();
    if !root.is_dir() {
        return json!({"root": rs, "tracks": [], "note": "no content/tracks folder"});
    }
    let mut names: Vec<String> = std::fs::read_dir(&root).map(|rd| {
        rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()
    }).unwrap_or_default();
    names.sort();
    let mut out = vec![];
    for track in names {
        let ldir = root.join(&track).join("layouts");
        if !ldir.is_dir() {
            continue;
        }
        let mut layouts: BTreeMap<String, BTreeSet<&str>> = BTreeMap::new();
        for e in std::fs::read_dir(&ldir).into_iter().flatten().flatten() {
            let f = e.file_name().to_string_lossy().into_owned();
            if !f.ends_with(".aisplinedata") {
                continue;
            }
            let name = f.split('.').next().unwrap_or("").to_string();
            let kind = if f.contains(".ideal_line.") { "ideal_line" }
                       else if f.contains(".pitlane.") { "pitlane" } else { "other" };
            layouts.entry(name).or_default().insert(kind);
        }
        let ls: Vec<Value> = layouts.iter().map(|(k, v)| json!({
            "layout": k, "ideal_line": v.contains("ideal_line"), "pitlane": v.contains("pitlane"),
            "ai_ready": v.contains("ideal_line") && v.contains("pitlane"),
        })).collect();
        out.push(json!({"track": track, "layouts": ls}));
    }
    json!({"root": rs, "tracks": out})
}

/// Best-effort primary IPv4 on a real interface (netutil.lan_ipv4): a UDP
/// "connect" sends nothing, it only asks the routing table for a source IP.
pub fn lan_ipv4() -> String {
    if let Ok(s) = std::net::UdpSocket::bind("0.0.0.0:0") {
        if s.connect("1.1.1.1:80").is_ok() {
            if let Ok(a) = s.local_addr() {
                let ip = a.ip().to_string();
                if !ip.starts_with("127.") && ip != "0.0.0.0" {
                    return ip;
                }
            }
        }
    }
    String::new()
}
