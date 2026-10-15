//! What is happening right now, and what is wrong - ported from overview.py
//! (and the dashboard's /api/state).
//!
//! ⚠ Runs on every dashboard load: each probe must be cheap and must not
//! fail the page - a broken mod folder costs its one line, not the answer.

use std::path::Path;

use serde_json::{json, Value};

use crate::{config, detect, profiles, serverproc};

/// Every server executable in the server folder, for diagnostics.
pub fn server_candidates() -> Vec<String> {
    let d = detect::effective_server_dir();
    if d.is_empty() {
        return vec![];
    }
    let Ok(rd) = std::fs::read_dir(&d) else { return vec![] };
    let mut v: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| { let l = n.to_lowercase(); l.starts_with("assettocorsaevoserver") && l.ends_with(".exe") })
        .collect();
    v.sort();
    v
}

/// Our helper tools: the configured copy, else the bundled one.
pub fn tools_dir() -> String {
    let cfg = config::load();
    let d = config::get_str(&cfg, "tools_dir");
    if !d.is_empty() && Path::new(&d).is_dir() {
        return d;
    }
    crate::bundle::tools_dir().to_string_lossy().into_owned()
}

fn server_exe() -> String {
    detect::find("server_exe", false)
}

/// GET /api/state
pub fn state() -> Value {
    let profs = profiles::load();
    let exe = server_exe();
    json!({
        "profiles": profs.len(),
        "running": profs.iter().take(1).any(|p| serverproc::status(p)["running"] == json!(true)),
        "backend": crate::backend::state(),
        "server_dir": detect::effective_server_dir(),
        "server_exe_ok": !exe.is_empty() && Path::new(&exe).exists(),
        "server_exe": exe,
        "server_exe_found": server_candidates(),
        "tools_ok": Path::new(&tools_dir()).is_dir(),
    })
}

/// ⚠ Worth knowing BEFORE launching: a dedicated server started on an
/// occupied port retries in a tight loop and has frozen this machine.
fn port_busy(port: u16) -> bool {
    !crate::procs::tcp_listen_pids(port).is_empty()
}

fn events() -> Vec<Value> {
    for name in ["events_practice.json", "events_race_weekend.json"] {
        let p = crate::content::catalog_path(name);
        if p.is_empty() {
            continue;
        }
        if let Some(ev) = std::fs::read(&p).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|d| d.get("events").and_then(|e| e.as_array()).cloned()) {
            return ev;
        }
    }
    vec![]
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

fn truthy_or(v: &Value, k: &str, d: Value) -> Value {
    match v.get(k) {
        None | Some(Value::Null) | Some(Value::Bool(false)) => d,
        Some(Value::Number(n)) if n.as_f64() == Some(0.0) => d,
        Some(Value::String(x)) if x.is_empty() => d,
        Some(x) => x.clone(),
    }
}

fn servers() -> Vec<Value> {
    let evs = events();
    profiles::load().iter().map(|p| {
        let st = serverproc::status(p);
        // ⚠ a profile stores an INDEX into events_*.json: a deployed custom
        // track wins, then track_label (a borrowed stock slot), then the event
        let custom = s(p, "custom_track");
        let mut track = if !custom.trim().is_empty() { custom.trim().to_string() } else { s(p, "track_label").trim().to_string() };
        let mut layout = String::new();
        if track.is_empty() {
            let idx = match p.get("track_index") { Some(Value::Number(n)) => n.as_i64().unwrap_or(0), Some(Value::String(x)) => x.parse().unwrap_or(-1), _ => 0 };
            let idx = if idx < 0 { idx + evs.len() as i64 } else { idx };
            if let Some(ev) = usize::try_from(idx).ok().and_then(|i| evs.get(i)) {
                track = s(ev, "track");
                layout = s(ev, "layout");
            }
        }
        let name = s(p, "name");
        json!({
            "id": p.get("id").cloned().unwrap_or(Value::Null),
            "name": if name.is_empty() { "(unnamed)".to_string() } else { name },
            "track": track,
            "custom_track": custom.trim(),
            "layout": layout,
            "port": truthy_or(p, "tcp_port", truthy_or(p, "port", json!(9700))),
            "http_port": truthy_or(p, "http_port", json!(8080)),
            "running": st["running"] == json!(true),
            "pid": st.get("pid").cloned().unwrap_or(Value::Null),
            "clients": st.get("clients").cloned().unwrap_or(Value::Null),
        })
    }).collect()
}

/// Problems worth acting on, most urgent first - each says what to DO.
fn attention(srv: &[Value], be: &Value) -> Vec<Value> {
    let mut items = Vec::new();
    if let Some(rb) = crate::updater::last_rollback() {
        items.push(json!({"level": "warn", "what": rb["what"], "do": rb["do"]}));
    }
    let exe = server_exe();
    if exe.is_empty() || !Path::new(&exe).exists() {
        let found = server_candidates();
        items.push(json!({"level": "bad", "what": "Dedicated server executable not found",
            "do": if found.is_empty() { "Install the dedicated server, or set server_dir in Settings".to_string() }
                  else { format!("Found in that folder: {} - pick one in Settings", found.join(", ")) }}));
    }
    if !Path::new(&tools_dir()).is_dir() {
        items.push(json!({"level": "warn", "what": "Backend tools folder missing", "do": "Set tools_dir in Settings"}));
    }
    if be.get("have_cert").map(|v| !matches!(v, Value::Null | Value::Bool(false))) != Some(true) {
        items.push(json!({"level": "warn", "what": "No TLS certificate",
            "do": "The client will not connect to our own lobby without one - generate it on the Backend page"}));
    }
    // ⚠ two profiles on one port cannot be told apart
    if let Value::Object(cl) = serverproc::port_clashes() {
        for (port, names) in cl {
            let names: Vec<String> = names.as_array().cloned().unwrap_or_default().iter().filter_map(|n| n.as_str().map(String::from)).collect();
            items.push(json!({"level": "warn", "what": format!("{} profiles share port {port}: {}", names.len(), names.join(", ")),
                "do": "Give each its own port - while they share one, ACECM cannot tell which is running, and stopping one may stop the other"}));
        }
    }
    for sv in srv {
        let port = sv["port"].as_u64().or_else(|| sv["port"].as_str().and_then(|x| x.parse().ok())).unwrap_or(0);
        if sv["running"] != json!(true) && port > 0 && port < 65536 && port_busy(port as u16) {
            items.push(json!({"level": "bad",
                "what": format!("Port {} is already in use, and “{}” wants it", port, s(sv, "name")),
                "do": "Stop whatever holds it, or change this profile's port. Starting anyway can lock the whole machine up"}));
        }
    }
    let broken = crate::mods::audit().get("rows").and_then(|r| r.as_array())
        .map(|r| r.iter().filter(|x| x.get("problem").map(|p| !matches!(p, Value::Null | Value::Bool(false)) && p != &json!("")).unwrap_or(false)).count())
        .unwrap_or(0);
    if broken > 0 {
        items.push(json!({"level": "warn", "what": format!("{broken} car mod(s) are not installed cleanly"),
            "do": "See Content - a half-installed mod loads for you and not for joiners"}));
    }
    // hosting something nobody can download
    let reg = crate::registry::load();
    let published: Vec<String> = reg.iter().flat_map(|e| e.get("required_tracks").and_then(|t| t.as_array()).cloned().unwrap_or_default())
        .filter_map(|t| t.as_str().map(String::from)).collect();
    let known = crate::trackimport::track_map(false);
    // ⚠ only CUSTOM content: a base-game track is on every player's disk
    let stock: std::collections::HashSet<String> = crate::content::tracks().get("tracks").and_then(|t| t.as_array()).cloned().unwrap_or_default()
        .iter().filter(|t| t.get("mod").map(|m| matches!(m, Value::Null | Value::Bool(false))).unwrap_or(true))
        .map(|t| s(t, "track").trim().to_lowercase()).collect();
    let hosting = srv.iter().any(|x| x["running"] == json!(true) && !s(x, "track").is_empty()
        && (!s(x, "custom_track").is_empty() || !stock.contains(&s(x, "track").trim().to_lowercase())));
    if hosting && published.is_empty() && !known.is_empty() {
        items.push(json!({"level": "warn", "what": "A server is running but no content is published",
            "do": "Players missing the track cannot download it from you - deploy it from Content to publish automatically"}));
    }
    if !published.is_empty() {
        items.push(json!({"level": "info", "what": "Remote players fetch content from this ACECM, not from the game port",
            "do": "They need TCP 8092 (this window) plus the game's TCP+UDP port (usually 9700). Forward both if they are not on your LAN, and allow 8092 in Windows Firewall."}));
    }
    items
}

/// GET /api/overview
pub fn overview() -> Value {
    let be = crate::backend::state();
    let srv = servers();
    let running: Vec<&Value> = srv.iter().filter(|x| x["running"] == json!(true)).collect();
    let players: i64 = running.iter().map(|x| x["clients"].as_i64().unwrap_or(0)).sum();
    let tracks = crate::trackimport::track_map(false).len();
    let cars = crate::contentsync::installed_cars().len();
    let shared: usize = crate::registry::load().iter().map(|e| {
        let n = |k: &str| e.get(k).and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
        n("required_tracks") + n("required_mods")
    }).sum();
    json!({
        "servers": srv,
        "running": running.len(),
        "players": players,
        "tracks": tracks,
        "cars": cars,
        "shared": shared,
        "backend": be,
        "attention": attention(&srv, &be),
        "server_dir": detect::effective_server_dir(),
    })
}
