//! The advertisement our lobby backend puts in the multiplayer list
//! (<data>/lobby.json) - ported from acecm/lobby.py. The backend process
//! re-reads this FILE on every list request, so whatever is true has to end
//! up on disk.
//! ⚠ Corrected against the server it is FOR (by its TCP port), never
//! "whichever server is running" - with two up that advertised (and
//! joined) the wrong one.

use std::io::Read;
use std::path::Path;

use base64::Engine;
use serde_json::{json, Map, Value};

use crate::{content, misc, paths, procs, profiles, servers, trackdeploy, trackimport};

pub fn path() -> std::path::PathBuf {
    paths::data().join("lobby.json")
}

const GAME_MODE_TYPE: &[(&str, i64)] = &[
    ("NONE", 0), ("RACE_WEEKEND", 1), ("SRO_RACE", 2), ("INSTANT_RACE", 3), ("SUPERPOLE", 4),
    ("FREEROAM", 5), ("DRIFT", 6), ("RALLY", 7), ("HOTSTINT", 8), ("HOTLAP", 9),
    ("PRACTICE", 10), ("TEST_DRIVE", 11), ("A_TO_B", 12),
];

fn mode_type(name: &str) -> i64 {
    let n = if name.trim().is_empty() { "PRACTICE".to_string() } else { name.trim().to_uppercase() };
    let key = n.replace("GAMEMODETYPE_", "");
    GAME_MODE_TYPE.iter().find(|(k, _)| *k == key).map(|(_, v)| *v).unwrap_or(10)
}

fn sv(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

/// python int(x or default) for the JSON values a profile carries
fn int_or(v: &Value, k: &str, d: i64) -> i64 {
    match v.get(k) {
        Some(Value::Number(n)) => { let x = n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).unwrap_or(0); if x == 0 { d } else { x } }
        Some(Value::String(s)) if !s.is_empty() => s.trim().parse().unwrap_or(d),
        Some(Value::Bool(true)) => 1,
        _ => d,
    }
}

pub fn event(profile: &Value) -> Map<String, Value> {
    let custom = sv(profile, "custom_track").trim().to_string();
    let mut m = Map::new();
    if !custom.is_empty() {
        let mut layout = String::new();
        if let Some(folder) = trackimport::track_map(false).get(&custom).and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
            if let Ok(t) = trackdeploy::read_track_folder(&Path::new(&trackimport::tracks_dir()).join(folder), None) {
                layout = t.layout;
            }
        }
        m.insert("track".into(), json!(custom));
        m.insert("event_name".into(), json!(if layout.is_empty() { custom.clone() } else { format!("{layout} Race") }));
        m.insert("layout".into(), json!(layout));
        m.insert("track_index".into(), json!(int_or(profile, "track_index", 0)));
        return m;
    }
    let idx = int_or(profile, "track_index", 0);
    let tracks = content::tracks().get("tracks").and_then(|t| t.as_array()).cloned().unwrap_or_default();
    let ev = tracks.iter().find(|t| t.get("index").and_then(|v| v.as_i64()) == Some(idx)).cloned()
        .or_else(|| if idx >= 0 { tracks.get(idx as usize).cloned() } else { None }).unwrap_or(json!({}));
    let label = sv(profile, "track_label").trim().to_string();
    let name = [label, sv(&ev, "name"), sv(&ev, "event_name")].into_iter().find(|s| !s.is_empty()).unwrap_or_default();
    m.insert("track".into(), json!(sv(&ev, "track")));
    m.insert("layout".into(), json!(sv(&ev, "layout")));
    m.insert("event_name".into(), json!(name));
    m.insert("track_index".into(), json!(idx));
    m
}

fn truthy(v: Option<&Value>) -> bool {
    match v {
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
        _ => false,
    }
}

pub fn from_profile(profile: &Value) -> Value {
    let hour = int_or(profile, "tod_hour", 13);
    let minute = int_or(profile, "tod_minute", 0);
    let id = sv(profile, "id");
    let name = sv(profile, "name");
    let gm = sv(profile, "game_mode");
    let mut out = json!({
        "server_id": if id.is_empty() { "local-0000-0000-0000-000000000001".into() } else { id },
        "server_name": if name.is_empty() { "ACECM server".into() } else { name },
        "tcp_port": int_or(profile, "tcp_port", 9700),
        "udp_port": int_or(profile, "tcp_port", 9700),
        "http_port": int_or(profile, "http_port", 8080),
        "max_players": int_or(profile, "max_players", 90),
        // ⚠ BOOLEAN flags, never the passwords: the row goes to every client
        "driver_password": truthy(profile.get("driver_password")),
        "spectator_password": truthy(profile.get("spectator_password")),
        "time_of_day": format!("{hour:02}:{minute:02}"),
        "game_mode": if gm.is_empty() { "PRACTICE".into() } else { gm.clone() },
        "game_mode_type": mode_type(&gm),
        "cars": servers::allowed_car_ids(profile),
        "lan_ip": misc::lan_ipv4(),
        "loopback": "127.0.0.1",
    });
    if let Some(o) = out.as_object_mut() {
        o.extend(event(profile));
    }
    out
}

pub fn write(profile: &Value) -> Value {
    let blob = from_profile(profile);
    let _ = std::fs::create_dir_all(paths::data());
    let _ = profiles::write_json_indent2(&path(), &blob);
    blob
}

/// One base64 + (4-byte BE length) + zlib JSON blob from a command line.
fn decode_blob(cmd: &str, flag: &str) -> Value {
    let Some(i) = cmd.find(flag) else { return json!({}) };
    let raw = cmd[i + flag.len()..].split(' ').next().unwrap_or("").trim().trim_matches('"');
    let Ok(data) = base64::engine::general_purpose::STANDARD.decode(raw) else { return json!({}) };
    if data.len() < 4 {
        return json!({});
    }
    let mut s = String::new();
    if flate2::read::ZlibDecoder::new(&data[4..]).read_to_string(&mut s).is_err() {
        return json!({});
    }
    serde_json::from_str(&s).unwrap_or(json!({}))
}

fn server_cmdlines() -> Vec<String> {
    procs::pids_named_prefix(&["assettocorsaevoserver"]).into_iter()
        .map(procs::cmdline).filter(|c| !c.is_empty()).map(|c| c.trim().to_string()).collect()
}

fn server_cmdline(want_port: i64) -> String {
    let lines = server_cmdlines();
    if lines.is_empty() {
        return String::new();
    }
    if want_port != 0 {
        return lines.into_iter().find(|ln| {
            decode_blob(ln, "-serverconfig=").get("server_tcp_listener_port")
                .and_then(|v| v.as_i64()).unwrap_or(0) == want_port
        }).unwrap_or_default();
    }
    lines[0].clone()
}

/// What the live dedicated server was ACTUALLY started with (its whole
/// configuration rides on the command line).
pub fn running_config(want_port: i64) -> Map<String, Value> {
    let cmd = server_cmdline(want_port);
    let mut out = Map::new();
    if cmd.is_empty() {
        return out;
    }
    let cfg = decode_blob(&cmd, "-serverconfig=");
    let season = decode_blob(&cmd, "-seasondefinition=");
    let ev = season.get("event").cloned().unwrap_or(json!({}));
    if truthy(cfg.get("server_name")) {
        out.insert("server_name".into(), cfg["server_name"].clone());
    }
    if truthy(cfg.get("max_players")) {
        out.insert("max_players".into(), json!(int_or(&cfg, "max_players", 0)));
    }
    for (k, n) in [("server_tcp_listener_port", "tcp_port"), ("server_udp_listener_port", "udp_port"),
                   ("server_http_port", "http_port")] {
        if truthy(cfg.get(k)) {
            out.insert(n.into(), json!(int_or(&cfg, k, 0)));
        }
    }
    for k in ["track", "layout", "event_name"] {
        if truthy(ev.get(k)) {
            out.insert(k.into(), ev[k].clone());
        }
    }
    let cars: Vec<Value> = cfg.get("allowed_cars_list_full").and_then(|v| v.as_array()).cloned().unwrap_or_default()
        .into_iter().filter_map(|c| c.get("car_name").filter(|n| truthy(Some(n))).cloned()).collect();
    if !cars.is_empty() {
        out.insert("cars".into(), json!(cars));
    }
    if !out.is_empty() {
        out.insert("live".into(), json!(true));
    }
    out
}

fn stored() -> Map<String, Value> {
    std::fs::read(path()).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.as_object().cloned()).unwrap_or_default()
}

fn adv_port(cur: &Map<String, Value>) -> i64 {
    let v = Value::Object(cur.clone());
    let p = int_or(&v, "tcp_port", 0);
    if p != 0 { p } else { int_or(&v, "server_tcp_port", 0) }
}

/// The advertisement, corrected against the server it is for.
pub fn read() -> Value {
    let mut blob = stored();
    let live = running_config(adv_port(&blob));
    blob.extend(live);
    Value::Object(blob)
}

/// Write the corrected advertisement back; {} when nothing changed.
pub fn refresh(want_port: i64) -> Value {
    let cur = stored();
    let port = if want_port != 0 { want_port } else { adv_port(&cur) };
    let live = running_config(port);
    if live.is_empty() {
        return json!({});
    }
    let mut merged = cur.clone();
    merged.extend(live);
    if merged == cur {
        return json!({});
    }
    let v = Value::Object(merged);
    let _ = profiles::write_json_indent2(&path(), &v);
    v
}
