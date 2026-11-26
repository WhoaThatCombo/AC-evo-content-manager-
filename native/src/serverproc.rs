//! Running dedicated servers - ported from servers.py (profiles CRUD,
//! status, start, stop, log tail) and tools/start_vai_server.py (the
//! launch itself, now done directly from here: no launcher process).
//!
//! ⚠ REFUSE to start on a port already in use. A second server on an
//! occupied port spins retrying its sockets and reserved ~120 GB of virtual
//! memory on 2026-08-11, hanging the whole PC. A server needs ~30 s to bind,
//! so a launch claims its ports for a grace period before anything listens.
//! ⚠ ONE start at a time for every profile (two concurrent starts once
//! deployed into the same archive and launched two servers on one port).
//! ⚠ The exe is console-subsystem: CREATE_NO_WINDOW or a discarded stdout
//! makes it print "Start Server" then abort with 0xC0000409. It gets a NEW,
//! HIDDEN console and a real log file.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use serde_json::{json, Map, Value};

use crate::{autoshare, content, damage, detect, lobby, paths, procs, profiles, servers,
            splines, trackdeploy, trackimport, tracktables};

pub fn template() -> Map<String, Value> {
    match serde_json::from_str::<Value>(r#"{"name": "New server", "track_index": 18, "game_mode": "PRACTICE", "session_type": "UNRANKED", "ai": 0, "skill_min": 70, "skill_max": 95, "max_players": 90, "tcp_port": 9700, "http_port": 8080, "tod_hour": 13, "tod_minute": 0, "time_mult": 0, "weather": "CLEAR", "weather_behaviour": "STATIC", "grip": "OPTIMUM", "tuning": "TuningAllowed", "pi_min": 0.0, "pi_max": 0.0, "practice_duration": 9999, "cycle": false, "driver_password": "", "spectator_password": "", "admin_password": "", "cars": [], "allow_kunos": true, "mods": "", "entry_list_path": "", "results_path": "", "entry_list_url": "", "results_post_url": "", "tcp_internal_port": 0, "udp_internal_port": 0, "overtime_wait": 10, "max_wait_to_box": 10, "track_label": "", "custom_track": "", "penalties": false, "damage_pct": 100, "car_cut_tyres_out": 4, "warning_trigger_countdown": 3, "time_penalty_ms": 5000, "tod_year": 2024, "tod_month": 8, "tod_day": 15, "tod_second": 0, "no_lobby": false, "write_results": false, "export_json": false, "car_handicaps": "", "telemetry": true, "log": "vai_server.log"}"#) {
        Ok(Value::Object(m)) => m,
        _ => Map::new(),
    }
}

pub fn options() -> Value {
    json!({"game_mode": ["PRACTICE", "RACE_WEEKEND", "INSTANT_RACE", "HOTLAP", "HOTSTINT", "DRIFT", "FREEROAM", "RALLY", "SUPERPOLE", "TEST_DRIVE", "A_TO_B", "SRO_RACE"],
           "session_type": ["UNRANKED", "RANKED", "BOTH"],
           "weather": ["CLEAR", "SCATTERED_CLOUDS", "BROKEN_CLOUDS", "OVERCAST", "DAMP", "DRIZZLE", "RAIN", "HEAVY_RAIN", "CUSTOM"],
           "weather_behaviour": ["STATIC", "DYNAMIC"], "grip": ["OPTIMUM", "FAST", "GREEN"],
           "tuning": ["TuningAllowed", "TuningDenied"]})
}

// ------------------------------------------------------------- helpers --
fn sv(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

/// profile.get(k, default) as an int, the way the backend reads it
fn iv(v: &Value, k: &str, d: i64) -> i64 {
    match v.get(k) {
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).unwrap_or(d),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(d),
        Some(Value::Bool(b)) => *b as i64,
        _ => d,
    }
}

fn truthy(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
        _ => false,
    }
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn sane(v: Option<&Value>, d: i64, lo: i64, hi: i64) -> i64 {
    let x = match v {
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Some(Value::String(s)) => s.trim().parse().ok(),
        _ => None,
    };
    match x { Some(y) if (lo..=hi).contains(&y) => y, _ => d }
}

const LEGACY_LOG: &str = "vai_server.log";

/// A plain file name: no folder, drive or stream part, nothing like "..".
fn bare_log_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 120 && !n.contains(['/', '\\', ':']) && !n.chars().any(|c| c.is_control())
        && !n.trim_matches('.').is_empty() && Path::new(n).file_name().map(|f| f == n).unwrap_or(false)
}

fn default_log(id: &str) -> String {
    let safe: String = id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(48).collect();
    if safe.is_empty() { LEGACY_LOG.into() } else { format!("server_{safe}.log") }
}

/// The log file NAME a profile's server writes. ⚠ Every profile used to
/// default to one shared name, and a second server truncated the first
/// one's log; a profile still carrying that default gets its own.
pub fn log_name(profile: &Value) -> String {
    let n = sv(profile, "log");
    if bare_log_name(&n) && n != LEGACY_LOG { n } else { default_log(&sv(profile, "id")) }
}

/// <server>/serverConfig/<name>; anything but a bare file name falls back
/// to the shared legacy name (the "log" field must not point elsewhere).
pub fn server_log(name: &str) -> PathBuf {
    Path::new(&detect::effective_server_dir()).join("serverConfig").join(if bare_log_name(name) { name } else { LEGACY_LOG })
}

pub fn profile_log(profile: &Value) -> PathBuf {
    server_log(&log_name(profile))
}

/// profiles.json exists but does not read as a list: saving "the list"
/// now would replace every profile with the one being saved.
fn profiles_unreadable() -> bool {
    let p = profiles::path();
    p.exists() && !std::fs::read(&p).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).map(|v| v.is_array()).unwrap_or(false)
}

const UNREADABLE: &str = "profiles.json could not be read - not saving over it (fix or move the file, then try again)";

// -------------------------------------------------------------- profiles --
/// Merge over the EXISTING profile (a partial save must not reset the rest),
/// sanitise the date (some years crash this exe), give new ones an id.
pub fn upsert(profile: &Value) -> Value {
    if profiles_unreadable() {
        return json!({"ok": false, "error": UNREADABLE});
    }
    let mut items = profiles::load();
    let id = sv(profile, "id");
    let current = items.iter().find(|p| !id.is_empty() && sv(p, "id") == id).cloned().unwrap_or(json!({}));
    let mut m = template();
    for src in [&current, profile] {
        if let Value::Object(o) = src {
            for (k, v) in o {
                m.insert(k.clone(), v.clone());
            }
        }
    }
    m.insert("tod_year".into(), json!(sane(m.get("tod_year"), 2024, 2020, 2035)));
    m.insert("tod_month".into(), json!(sane(m.get("tod_month"), 8, 1, 12)));
    m.insert("tod_day".into(), json!(sane(m.get("tod_day"), 15, 1, 31)));
    let mut p = Value::Object(m);
    let new = !truthy(p.get("id"));
    if new {
        p["id"] = json!(profiles::new_id());
    }
    // its own log: a path, or a name another profile already writes, is replaced
    let mine = log_name(&p);
    if !bare_log_name(&sv(&p, "log")) || items.iter().any(|o| o.get("id") != p.get("id") && log_name(o) == mine) {
        p["log"] = json!(default_log(&sv(&p, "id")));
    }
    if new {
        items.push(p.clone());
    } else {
        let mut found = false;
        for x in items.iter_mut() {
            if x.get("id") == p.get("id") {
                *x = p.clone();
                found = true;
            }
        }
        if !found {
            items.push(p.clone());
        }
    }
    let _ = profiles::save_all(&items);
    autoshare::publish(&p); // sharing follows the configuration
    p
}

pub fn delete(id: &str) {
    if profiles_unreadable() {
        return;
    }
    let items: Vec<Value> = profiles::load().into_iter().filter(|p| sv(p, "id") != id).collect();
    let _ = profiles::save_all(&items);
    autoshare::prune(&profiles::load());
}

// --------------------------------------------------------------- runtime --
fn runtime_path() -> PathBuf {
    paths::data().join("runtime.json")
}

pub fn runtime() -> Map<String, Value> {
    std::fs::read(runtime_path()).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.as_object().cloned()).unwrap_or_default()
}

/// ⚠ runtime.json is read-modify-written from the events thread (status)
/// and from starts/stops: one lock, or a freshly bound pid is lost.
static RT_LOCK: Mutex<()> = Mutex::new(());

fn rt_lock() -> std::sync::MutexGuard<'static, ()> {
    RT_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn save_runtime(r: &Map<String, Value>) {
    let _ = profiles::write_json_indent2(&runtime_path(), &Value::Object(r.clone()));
}

pub fn bind_pid(id: &str, rec: Value) {
    let _g = rt_lock();
    let mut r = runtime();
    r.insert(id.to_string(), rec);
    save_runtime(&r);
}

/// Drop a profile's record - only while it still names `pid` (when given),
/// so a record a new start just wrote survives.
fn forget_pid(id: &str, pid: Option<u32>) {
    let _g = rt_lock();
    let mut r = runtime();
    let cur = r.get(id).and_then(|v| v.get("pid")).and_then(|v| v.as_u64()).map(|v| v as u32);
    if cur.is_some() && (pid.is_none() || cur == pid) {
        r.remove(id);
        save_runtime(&r);
    }
}

pub fn server_pids() -> HashSet<u32> {
    procs::pids_named(&["AssettoCorsaEVOServer", "AssettoCorsaEVOServer.stock"]).into_iter().collect()
}

fn port_shared(profile: &Value) -> bool {
    let mine = iv(profile, "http_port", 8080);
    profiles::load().iter().any(|o| o.get("id") != profile.get("id") && iv(o, "http_port", 8080) == mine)
}

pub fn port_busy(port: i64) -> bool {
    std::net::TcpStream::connect_timeout(&std::net::SocketAddr::from(([127, 0, 0, 1], port as u16)),
                                         Duration::from_millis(400)).is_ok()
}

/// Live state for one profile, tied to ITS process where possible.
pub fn status(profile: &Value) -> Value {
    let mut st = json!({"running": false, "pid": null, "clients": null, "log_age": null});
    let id = sv(profile, "id");
    let rec = runtime().get(&id).cloned().unwrap_or(json!({}));
    let pid = rec.get("pid").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let http = { let h = iv(profile, "http_port", 8080); if h == 0 { 8080 } else { h } };
    let live = server_pids();
    // ⚠ a leftover pid is not enough - Windows reuses them
    if pid != 0 && live.contains(&pid) {
        st["running"] = json!(true);
        st["pid"] = json!(pid);
        watch(pid); // a server started by an earlier ACECM gets its watcher back
    } else if pid != 0 {
        // dead, or the pid now belongs to something else: not our server
        forget_pid(&id, Some(pid));
    } else if !port_shared(profile) {
        // ⚠ only when the listener IS a dedicated server (NVIDIA Broadcast
        // sits on 8080 and made a stopped server look running)
        let owners: HashSet<u32> = procs::tcp_listen_pids(http as u16).into_iter().collect();
        if let Some(hit) = owners.intersection(&live).next() {
            st["running"] = json!(true);
            st["pid"] = json!(hit);
        }
    } else {
        st["ambiguous"] = json!(true);
    }
    if st["running"] == true {
        let c = reqwest::blocking::Client::builder().no_proxy().timeout(Duration::from_millis(350)).build().ok()
            .and_then(|c| c.get(format!("http://127.0.0.1:{http}/")).send().ok())
            .and_then(|r| r.text().ok()).and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v.get("clients").cloned());
        st["clients"] = c.unwrap_or(Value::Null);
    }
    let log = profile_log(profile);
    if let Ok(m) = std::fs::metadata(&log) {
        let age = m.modified().ok().and_then(|t| SystemTime::now().duration_since(t).ok())
            .map(|d| (d.as_secs_f64() * 10.0).round() / 10.0).unwrap_or(0.0);
        st["log_age"] = json!(age);
        st["log"] = json!(log.to_string_lossy());
    }
    st
}

pub fn port_clashes() -> Value {
    let mut out = Map::new();
    for key in ["tcp_port", "http_port"] {
        let mut seen: Vec<(i64, Vec<String>)> = vec![];
        for p in profiles::load() {
            let port = iv(&p, key, 0);
            let name = { let n = sv(&p, "name"); if n.is_empty() { "?".into() } else { n } };
            match seen.iter_mut().find(|(x, _)| *x == port) {
                Some(e) => e.1.push(name),
                None => seen.push((port, vec![name])),
            }
        }
        for (port, names) in seen {
            if port != 0 && names.len() > 1 {
                let e = out.entry(port.to_string()).or_insert(json!([]));
                for n in names {
                    if !e.as_array().unwrap().contains(&json!(n)) {
                        e.as_array_mut().unwrap().push(json!(n));
                    }
                }
            }
        }
    }
    Value::Object(out)
}

// ----------------------------------------------------------------- start --
static START_LOCK: Mutex<()> = Mutex::new(());
pub static STARTING: Mutex<Option<HashMap<String, Value>>> = Mutex::new(None);
pub static LAST_START: Mutex<Option<HashMap<String, Value>>> = Mutex::new(None);
/// port -> (claimed at, pid once launched)
static LAUNCHING: Mutex<Option<HashMap<i64, (Instant, u32)>>> = Mutex::new(None);
const STARTUP_GRACE: u64 = 45;

pub fn start_state() -> Value {
    let s = STARTING.lock().unwrap().clone().unwrap_or_default();
    let l = LAST_START.lock().unwrap().clone().unwrap_or_default();
    json!({"starting": s, "last_start": l})
}

/// One start at a time; see START_LOCK.
pub fn start(profile: &Value) -> Value {
    let id = sv(profile, "id");
    // ⚠ a panic in an earlier start poisons the lock: that is not "busy"
    let _g = match START_LOCK.try_lock() {
        Ok(g) => g,
        Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => {
            let who = STARTING.lock().unwrap().as_ref().and_then(|m| m.values().next().cloned())
                .map(|v| sv(&v, "name")).filter(|n| !n.is_empty()).unwrap_or_else(|| "another server".into());
            return json!({"ok": false, "busy": true, "error": format!("{who} is already starting - wait for it to finish")});
        }
    };
    STARTING.lock().unwrap().get_or_insert_with(HashMap::new)
        .insert(id.clone(), json!({"at": now_secs(), "name": sv(profile, "name")}));
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| start_inner(profile.clone())))
        .unwrap_or_else(|p| {
            let msg = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "panic".into());
            json!({"ok": false, "error": format!("the start failed unexpectedly: {msg}")})
        });
    STARTING.lock().unwrap().get_or_insert_with(HashMap::new).remove(&id);
    LAST_START.lock().unwrap().get_or_insert_with(HashMap::new).insert(id, json!({
        "at": now_secs(), "ok": r.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
        "error": sv(&r, "error")}));
    r
}

/// Track names the SERVER's own archive can resolve.
fn server_track_names() -> Vec<String> {
    let pkg = trackdeploy::server_kspkg();
    let Some(e) = crate::kspkg::find_entry(&pkg, r"system\tracks.table").ok().flatten() else { return vec![] };
    let Ok(mut f) = std::fs::File::open(&pkg) else { return vec![] };
    let Ok(blob) = crate::kspkg::read_entry(&mut f, &e) else { return vec![] };
    tracktables::registered_names(&blob).map(|s| s.into_iter().collect()).unwrap_or_default()
}

/// Why this profile's custom track would fail, or "" if fine. ⚠ A name the
/// server cannot resolve is a crash ("Trying to load a message with an
/// empty path"), not an error - check first and say what is wrong.
fn custom_track_problem(profile: &Value) -> String {
    let name = sv(profile, "custom_track").trim().to_string();
    if name.is_empty() {
        return String::new();
    }
    let known = server_track_names();
    if known.is_empty() || known.contains(&name) {
        return String::new();
    }
    let first = name.split_whitespace().next().unwrap_or("").to_lowercase();
    let close = known.iter().find(|k| k.to_lowercase().contains(&first));
    format!("the server does not have a track called '{name}' - it would start and then die loading an empty path. Deploy it from Content first{}",
            close.map(|c| format!(" (its archive calls it '{c}')")).unwrap_or_default())
}

/// The event JSON for a deployed track, or "" for the stock index.
/// ⚠ driven by custom_track, never by the cosmetic track_label.
fn custom_event(profile: &Value) -> Option<Value> {
    let name = sv(profile, "custom_track").trim().to_string();
    if name.is_empty() {
        return None;
    }
    let folder = trackimport::track_map(false).get(&name).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let layout = if folder.is_empty() { String::new() } else {
        trackdeploy::read_track_folder(&Path::new(&trackimport::tracks_dir()).join(&folder), None)
            .map(|t| t.layout).unwrap_or_default()
    };
    // ⚠ refuse rather than guess: a wrong layout resolves no containers
    if layout.is_empty() {
        return None;
    }
    Some(json!({"track": name, "layout": layout, "event_name": format!("{layout} Race")}))
}

fn enum_(prefix: &str, v: &str) -> String {
    let v = v.trim();
    if v.starts_with(prefix) { v.to_string() } else { format!("{prefix}{}", v.to_uppercase()) }
}

fn encode_blob(v: &Value) -> String {
    use std::io::Write;
    let j = profiles::py_dumps(v);
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    let _ = z.write_all(j.as_bytes());
    let comp = z.finish().unwrap_or_default();
    let mut raw = (j.len() as u32).to_be_bytes().to_vec();
    raw.extend(comp);
    base64::engine::general_purpose::STANDARD.encode(raw)
}

/// The two blobs the exe is launched with (start_vai_server.main).
pub fn build_blobs(profile: &Value) -> Result<(Value, Value), String> {
    let game_mode = { let g = sv(profile, "game_mode"); if g.is_empty() { "PRACTICE".into() } else { g } };
    let events_file = if game_mode.to_uppercase().contains("RACE") { "events_race_weekend.json" } else { "events_practice.json" };
    let evs = std::fs::read(content::catalog_path(events_file)).ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.get("events").and_then(|e| e.as_array()).cloned())
        .ok_or_else(|| format!("missing {events_file}"))?;
    let mut idx = iv(profile, "track_index", 0);
    if idx < 0 || idx as usize >= evs.len() {
        idx = 0; // a stale index falls back visibly, it must not stop the server
    }
    if evs.is_empty() {
        return Err(format!("{events_file} lists no events"));
    }
    let mut ev = evs[idx as usize].clone();
    if let Some(c) = custom_event(profile) {
        if let (Some(e), Value::Object(w)) = (ev.as_object_mut(), c) {
            e.extend(w);
        }
    }
    let port = iv(profile, "tcp_port", 9700);
    let http = iv(profile, "http_port", 8080);
    let mut cars = servers::allowed_car_ids(profile);
    if cars.is_empty() {
        cars = std::fs::read(content::catalog_path("cars.json")).ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| v.get("cars").and_then(|c| c.as_array()).cloned()).unwrap_or_default()
            .iter().filter_map(|c| c.get("name").and_then(|v| v.as_str()).map(String::from))
            .filter(|n| n.contains("_mech_")).collect();
    }
    let mut handicap: HashMap<String, (i64, i64)> = HashMap::new();
    for item in sv(profile, "car_handicaps").split(',').filter(|s| !s.is_empty()) {
        let bits: Vec<&str> = item.split(':').collect();
        if let Some(c) = bits.first().map(|c| c.trim()).filter(|c| !c.is_empty()) {
            let g = |i: usize| bits.get(i).filter(|b| !b.is_empty()).and_then(|b| b.parse().ok()).unwrap_or(0);
            handicap.insert(c.to_string(), (g(1), g(2)));
        }
    }
    let f = |k: &str, d: f64| profile.get(k).and_then(|v| v.as_f64()).unwrap_or(d);
    let cfg = json!({
        "server_tcp_listener_port": port, "server_udp_listener_port": port,
        "server_tcp_internal_port": ({ let x = iv(profile, "tcp_internal_port", 0); if x == 0 { port } else { x } }),
        "server_udp_internal_port": ({ let x = iv(profile, "udp_internal_port", 0); if x == 0 { port } else { x } }),
        "server_http_port": http,
        "server_name": ({ let n = sv(profile, "name"); if n.is_empty() { "ACECM server".into() } else { n } }),
        "max_players": iv(profile, "max_players", 90),
        "cycle": truthy(profile.get("cycle")),
        "allowed_cars_list_full": cars.iter().map(|c| { let h = handicap.get(c).copied().unwrap_or((0, 0));
            json!({"car_name": c, "ballast": h.0, "restrictor": h.1}) }).collect::<Vec<_>>(),
        "driver_password": sv(profile, "driver_password"),
        "spectator_password": sv(profile, "spectator_password"),
        "admin_password": sv(profile, "admin_password"),
        "type": enum_("MultiplayerServerListSessionType_", &{ let s = sv(profile, "session_type"); if s.is_empty() { "UNRANKED".into() } else { s } }),
        "tuning_type": ({ let t = sv(profile, "tuning"); if t.is_empty() { "TuningAllowed".into() } else { t } }),
        "pi_min": f("pi_min", 0.0), "pi_max": f("pi_max", 0.0),
        "entry_list_path": sv(profile, "entry_list_path"), "results_path": sv(profile, "results_path"),
        "entry_list_server_url": sv(profile, "entry_list_url"), "results_post_url": sv(profile, "results_post_url"),
    });
    let tl = match ev.get("track_length") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) if n.is_f64() => { let x = n.as_f64().unwrap(); if x.fract() == 0.0 { format!("{x:.1}") } else { n.to_string() } }
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Null) | None => "None".into(),
        Some(o) => o.to_string(),
    };
    let tod = |k: &str, d: i64| iv(profile, k, d);
    let season = json!({
        "game_type": enum_("GameModeType_", &game_mode),
        "event": {"track": ev.get("track").cloned().unwrap_or(json!("")), "layout": ev.get("layout").cloned().unwrap_or(json!("")),
                  "event_name": ev.get("event_name").cloned().unwrap_or(json!("")), "track_length": tl},
        "export_json": truthy(profile.get("export_json")),
        "game_config": {
            "practice_duration": tod("practice_duration", 9999),
            "practice_time_of_day": {"year": sane(profile.get("tod_year"), 2024, 2020, 2035),
                                     "month": sane(profile.get("tod_month"), 8, 1, 12),
                                     "day": sane(profile.get("tod_day"), 15, 1, 31),
                                     "hour": tod("tod_hour", 13), "minute": tod("tod_minute", 0),
                                     "second": tod("tod_second", 0), "time_multiplier": tod("time_mult", 0)},
            "practice_overtime_waiting_next_session": tod("overtime_wait", 10),
            "practice_max_wait_to_box": tod("max_wait_to_box", 10),
            // spelling is the exe's
            "enable_custom_penalities": truthy(profile.get("penalties")),
            "car_cut_tyres_out": tod("car_cut_tyres_out", 4),
            "warning_trigger_countdown": tod("warning_trigger_countdown", 3),
            "time_penalty_ms": tod("time_penalty_ms", 5000),
        },
        "weather_type": enum_("GameModeSelectionWeatherType_", &{ let w = sv(profile, "weather"); if w.is_empty() { "CLEAR".into() } else { w } }),
        "weather_behaviour": enum_("GameModeSelectionWeatherBehaviour_", &{ let w = sv(profile, "weather_behaviour"); if w.is_empty() { "STATIC".into() } else { w } }),
        "initial_grip": enum_("InitialGrip_", &{ let g = sv(profile, "grip"); if g.is_empty() { "OPTIMUM".into() } else { g } }),
    });
    Ok((cfg, season))
}

/// Seconds since a launch claimed `port`, while the claim holds: for the
/// grace period, and after it for as long as that launch's server lives
/// (a slow server can take longer than the grace period to bind).
fn claimed(port: i64) -> Option<u64> {
    let (at, pid) = LAUNCHING.lock().unwrap().as_ref().and_then(|m| m.get(&port)).copied()?;
    let s = at.elapsed().as_secs();
    (s < STARTUP_GRACE || (pid != 0 && server_pids().contains(&pid))).then_some(s)
}

fn release(ports: &[i64]) {
    if let Some(m) = LAUNCHING.lock().unwrap().as_mut() {
        for p in ports {
            m.remove(p);
        }
    }
}

fn start_inner(mut profile: Value) -> Value {
    let id = sv(&profile, "id");
    let mut bad = custom_track_problem(&profile);
    if !bad.is_empty() {
        // a Kunos update wipes the rows we wrote; put them back and re-check
        let _ = trackdeploy::redeclare_tracks(false);
        bad = custom_track_problem(&profile);
        if !bad.is_empty() {
            // imported here but never hosted: deploy it under the profile's name
            let want = sv(&profile, "custom_track").trim().to_lowercase();
            if let Some(m) = trackimport::importable().into_iter().find(|t| t.get("ok") == Some(&json!(true))
                && sv(t, "display_name").trim().to_lowercase() == want)
            {
                let _ = trackdeploy::deploy_native(Path::new(&sv(&m, "path")), false, Some(&sv(&m, "folder")),
                                                   Some(&sv(&profile, "custom_track")));
            }
            bad = custom_track_problem(&profile);
        }
        if !bad.is_empty() {
            return json!({"ok": false, "error": bad});
        }
    }
    let _ = autoshare::sync(&profile, true);
    let _ = splines::ship_for_profile(&profile);

    let tcp = iv(&profile, "tcp_port", 9700);
    let mut http = iv(&profile, "http_port", 8080);
    for port in [tcp, http] {
        if let Some(ago) = claimed(port) {
            return json!({"ok": false, "error": if ago < STARTUP_GRACE {
                format!("a server is already starting on port {port} ({ago}s ago) - wait for it to come up")
            } else {
                format!("a server ACECM started is still running on port {port} - stop it first")
            }});
        }
    }
    let rt = runtime();
    let live = server_pids();
    // ⚠ this profile's own server first: a slow one has not bound its ports yet
    let own = rt.get(&id).and_then(|r| r.get("pid")).and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    if recorded_pid_usable(own, &live) {
        return json!({"ok": false, "error": format!("this server is already running (pid {own}) - stop it first")});
    }
    for other in profiles::load() {
        if sv(&other, "id") == id {
            continue;
        }
        let pid = rt.get(&sv(&other, "id")).and_then(|r| r.get("pid")).and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        if pid != 0 && live.contains(&pid) && (iv(&other, "tcp_port", 0) == tcp || iv(&other, "http_port", 0) == http) {
            return json!({"ok": false, "error": format!("'{}' is already running on those ports (pid {pid}) - stop it first", sv(&other, "name"))});
        }
    }
    // the HTTP port is only the status page: when a NON-server owns it, move
    if port_busy(http) {
        let owners: HashSet<u32> = procs::tcp_listen_pids(http as u16).into_iter().collect();
        if !owners.is_empty() && owners.is_disjoint(&live) {
            let taken: HashSet<i64> = profiles::load().iter().map(|o| iv(o, "http_port", 0)).collect();
            if let Some(c) = (http + 1..http + 60).find(|c| !taken.contains(c) && *c != tcp && !port_busy(*c)) {
                profile["http_port"] = json!(c);
                http = c;
                let saved = upsert(&profile);
                if saved.get("id").is_some() {
                    profile = saved;
                }
            }
        }
    }
    for (label, port) in [("game", tcp), ("HTTP", http)] {
        if port_busy(port) {
            return json!({"ok": false, "error": format!("{label} port {port} is already in use - stop whatever is on it first (a server left running on these ports will spin and eat all memory)")});
        }
    }
    // ⚠ damage is global to the install: say so when this start cannot have its own
    let want_damage = profile.get("damage_pct").and_then(|v| v.as_f64()).map(|v| v as i64).unwrap_or(100);
    let mut warning = String::new();
    if live.is_empty() {
        let r = damage::apply_for(&profile);
        if r["ok"] != true {
            warning = format!("damage could not be set to {want_damage}%: {} - the server uses the archive's current setting",
                              r["error"].as_str().unwrap_or("unknown error"));
        }
    } else {
        let cur = damage::status()["damage"].as_i64();
        if cur != Some(want_damage) {
            warning = format!("another server is running, so damage stays at {} instead of {want_damage}% (it is one setting for every server)",
                              cur.map(|c| format!("{c}%")).unwrap_or_else(|| "its current setting".into()));
        }
    }
    if !warning.is_empty() {
        crate::logs::warn(format!("server start: {warning}"));
    }
    let exe = detect::find("server_exe", false);
    if exe.is_empty() || !Path::new(&exe).is_file() {
        return json!({"ok": false, "error": "dedicated server executable not found - set server_dir in Settings to the folder that holds AssettoCorsaEVOServer.exe"});
    }
    let (cfg, season) = match build_blobs(&profile) {
        Ok(b) => b,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    // claim the ports BEFORE launching: nothing listens for ~30 s
    LAUNCHING.lock().unwrap().get_or_insert_with(HashMap::new).extend([(tcp, (Instant::now(), 0)), (http, (Instant::now(), 0))]);
    let mut args = vec![format!("-serverconfig={}", encode_blob(&cfg)), format!("-seasondefinition={}", encode_blob(&season))];
    if truthy(profile.get("no_lobby")) {
        args.push("-no_lobby=true".into());
    }
    if truthy(profile.get("write_results")) {
        args.push("-write_server_results=true".into());
    }
    let srv = detect::effective_server_dir();
    let log = profile_log(&profile);
    if let Some(d) = log.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let pid = match launch_hidden_console(Path::new(&exe), &args, Path::new(&srv), &log) {
        Ok(p) => p,
        Err(e) => {
            release(&[tcp, http]);
            return json!({"ok": false, "error": format!("could not start the server: {e}")});
        }
    };
    if let Some(m) = LAUNCHING.lock().unwrap().as_mut() {
        for p in [tcp, http] {
            if let Some(e) = m.get_mut(&p) {
                e.1 = pid;
            }
        }
    }
    // a CRT abort (0xC0000409) shows up within ~3 s - wait long enough to see it
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(6) {
        if !procs::alive(pid) {
            release(&[tcp, http]);
            let detail = std::fs::read(&log).map(|b| String::from_utf8_lossy(&b).trim().to_string()).unwrap_or_default();
            let tail: String = detail.chars().rev().take(800).collect::<Vec<_>>().into_iter().rev().collect();
            return json!({"ok": false, "error": "the dedicated server died right after start", "detail": tail});
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    bind_pid(&id, json!({"pid": pid, "started": now_secs(), "http_port": http}));
    lobby::write(&profile);
    watch(pid);
    let mut out = json!({"ok": true, "pid": pid});
    if !warning.is_empty() {
        out["warning"] = json!(warning);
    }
    out
}

static WATCHED: Mutex<Option<HashSet<u32>>> = Mutex::new(None);

/// One memory watcher per live server pid.
fn watch(pid: u32) {
    if !WATCHED.lock().unwrap().get_or_insert_with(HashSet::new).insert(pid) {
        return;
    }
    // ⚠ a server we may not open (started elevated) stays in the set, or
    // every status poll would spawn a thread that exits at once
    if procs::Held::open(pid).is_none() {
        return;
    }
    std::thread::spawn(move || {
        watch_memory(pid);
        if let Some(s) = WATCHED.lock().unwrap().as_mut() {
            s.remove(&pid);
        }
    });
}

/// Commit limit in bytes: ACECM_SERVER_MEM_LIMIT_GB, else 12 GB, but at
/// most 3/4 of the RAM (never under 6 GB) - past that the PC is paging.
fn mem_limit(ram: Option<u64>, env: Option<f64>) -> u64 {
    const GB: u64 = 1 << 30;
    if let Some(g) = env.filter(|g| *g > 0.0) {
        return (g * GB as f64) as u64;
    }
    let cap = ram.map(|r| (r / 4 * 3).max(6 * GB)).unwrap_or(12 * GB);
    (12 * GB).min(cap)
}

/// ⚠ The backstop for every runaway cause we have not thought of: a healthy
/// server sits at 1-2 GB. Measured as COMMIT, not working set (the runaway
/// reserved ~120 GB; a working set is capped by RAM and stops growing), on
/// a held handle so the pid cannot be reused under us.
fn watch_memory(pid: u32) {
    let Some(h) = procs::Held::open(pid) else { return };
    let env = std::env::var("ACECM_SERVER_MEM_LIMIT_GB").ok().and_then(|v| v.parse::<f64>().ok());
    let limit = mem_limit(procs::physical_ram(), env);
    loop {
        std::thread::sleep(Duration::from_secs(5));
        if !h.alive() {
            return;
        }
        let Some(c) = h.commit() else { return };
        if c > limit {
            procs::kill(pid);
            let gb = |b: u64| b as f64 / (1u64 << 30) as f64;
            crate::logs::warn(format!("KILLED runaway server pid {pid}: {:.1} GB committed, over the {:.1} GB limit", gb(c), gb(limit)));
            return;
        }
    }
}

/// CreateProcessW with a NEW HIDDEN console and stdout/stderr on `log`.
#[cfg(windows)]
pub fn launch_hidden_console(exe: &Path, args: &[String], cwd: &Path, log: &Path) -> Result<u32, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL,
                                                   FILE_SHARE_READ, FILE_SHARE_WRITE};
    use windows_sys::Win32::System::Threading::{CreateProcessW, CREATE_NEW_CONSOLE, CREATE_UNICODE_ENVIRONMENT,
                                                PROCESS_INFORMATION, STARTF_USESHOWWINDOW, STARTF_USESTDHANDLES, STARTUPINFOW};
    let w = |s: &std::ffi::OsStr| -> Vec<u16> { s.encode_wide().chain(Some(0)).collect() };
    unsafe {
        let sa = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                                       lpSecurityDescriptor: std::ptr::null_mut(), bInheritHandle: 1 };
        let lp = w(log.as_os_str());
        let h = CreateFileW(lp.as_ptr(), GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE, &sa,
                            CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, std::ptr::null_mut());
        if h == INVALID_HANDLE_VALUE {
            return Err(format!("cannot open the server log {}", log.display()));
        }
        let mut cmd = format!("\"{}\"", exe.display());
        for a in args {
            cmd.push(' ');
            cmd.push_str(a); // base64 and -flag=value: no spaces or quotes
        }
        let mut cmdw: Vec<u16> = cmd.encode_utf16().chain(Some(0)).collect();
        let mut si: STARTUPINFOW = std::mem::zeroed();
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        si.dwFlags = STARTF_USESHOWWINDOW | STARTF_USESTDHANDLES;
        si.wShowWindow = 0; // SW_HIDE
        si.hStdOutput = h;
        si.hStdError = h;
        si.hStdInput = std::ptr::null_mut();
        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
        let cw = w(cwd.as_os_str());
        let exew = w(exe.as_os_str());
        let ok = CreateProcessW(exew.as_ptr(), cmdw.as_mut_ptr(), std::ptr::null(), std::ptr::null(), 1,
                                CREATE_NEW_CONSOLE | CREATE_UNICODE_ENVIRONMENT, std::ptr::null(),
                                cw.as_ptr(), &si, &mut pi);
        CloseHandle(h);
        if ok == 0 {
            return Err(format!("CreateProcessW failed ({})", std::io::Error::last_os_error()));
        }
        CloseHandle(pi.hThread);
        CloseHandle(pi.hProcess);
        procs::invalidate_snapshot();
        Ok(pi.dwProcessId)
    }
}

// ------------------------------------------------------------------ stop --
fn denied_msg(pids: &[u32]) -> String {
    let ids = pids.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(", ");
    format!("could not stop the server (PID {ids}). It was most likely started by an ACECM running as administrator, so this one has no rights over it. Restart ACECM as administrator and stop it again, or end AssettoCorsaEVOServer.exe in Task Manager (also as administrator).")
}

fn recorded_pid_usable(pid: u32, live: &HashSet<u32>) -> bool {
    pid != 0 && live.contains(&pid)
}

fn pid_on_port(port: i64) -> Option<u32> {
    let live = server_pids();
    procs::tcp_listen_pids(port as u16).into_iter().find(|p| live.contains(p))
}

/// Stop ONE profile's server, or every server when no id is given.
/// ⚠ Identify the process first: a per-server Stop used to kill them all.
pub fn stop(profile_id: &str) -> Value {
    if profile_id.is_empty() {
        let pids: Vec<u32> = server_pids().into_iter().collect();
        let stopped: Vec<u32> = pids.iter().copied().filter(|p| procs::kill(*p)).collect();
        let stuck: Vec<u32> = pids.iter().copied().filter(|p| !stopped.contains(p)).collect();
        if let Some(m) = LAUNCHING.lock().unwrap().as_mut() { m.clear(); }
        if !stuck.is_empty() {
            return json!({"ok": false, "stopped": stopped, "scope": "all", "error": denied_msg(&stuck)});
        }
        return json!({"ok": true, "stopped": stopped, "scope": "all"});
    }
    let Some(prof) = profiles::load().into_iter().find(|p| sv(p, "id") == profile_id) else {
        return json!({"ok": false, "error": "no such profile"});
    };
    let rec = runtime().get(profile_id).cloned().unwrap_or(json!({}));
    let mut pid = rec.get("pid").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    // ⚠ the recorded pid only while it IS a server: Windows reuses pids
    if !recorded_pid_usable(pid, &server_pids()) {
        pid = pid_on_port(iv(&prof, "http_port", 8080)).unwrap_or(0);
    }
    if pid == 0 {
        return json!({"ok": false, "error": "could not tell which process is this server - it may have been started outside ACECM. Use Stop all, or close it from Task Manager"});
    }
    if !procs::kill(pid) {
        return json!({"ok": false, "stopped": [], "scope": prof.get("name").cloned().unwrap_or(Value::Null), "error": denied_msg(&[pid])});
    }
    forget_pid(profile_id, None);
    release(&[iv(&prof, "tcp_port", 9700), iv(&prof, "http_port", 8080)]);
    json!({"ok": true, "stopped": [pid], "scope": prof.get("name").cloned().unwrap_or(Value::Null)})
}

// ------------------------------------------------------------------- log --
/// The end of a server's log, or only what is new since `since` bytes. A
/// log that got SMALLER was rotated or restarted: say so (reset) and send
/// the tail rather than splicing one run onto another.
pub fn log_tail(profile: &Value, lines: usize, since: u64) -> Value {
    use std::io::{Read, Seek, SeekFrom};
    let path = profile_log(profile);
    let Ok(mut f) = std::fs::File::open(&path) else {
        return json!({"lines": [], "error": "no log yet", "size": 0});
    };
    let size = f.seek(SeekFrom::End(0)).unwrap_or(0);
    let reset = since > size;
    let mut raw = vec![];
    if since > 0 && !reset {
        if since == size {
            return json!({"lines": [], "size": size, "reset": false});
        }
        let _ = f.seek(SeekFrom::Start(since));
    } else {
        let _ = f.seek(SeekFrom::Start(size.saturating_sub(200_000)));
    }
    let _ = f.read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw);
    // python str.splitlines (\n, \r\n, \r), dropping the vAI time spam
    let mut keep: Vec<String> = text.split_inclusive(['\n', '\r']).collect::<Vec<_>>()
        .join("").replace("\r\n", "\n").replace('\r', "\n").lines()
        .filter(|l| !l.contains("ServerWorldTime")).map(String::from).collect();
    if since == 0 || reset {
        let n = keep.len().saturating_sub(lines);
        keep = keep.split_off(n);
    }
    json!({"lines": keep, "size": size, "reset": reset || since == 0})
}

#[cfg(test)]
mod fix_tests {
    use super::*;

    #[test]
    fn log_field_is_a_bare_file_name() {
        for ok in ["server_a.log", "my server.log", "x"] {
            assert!(bare_log_name(ok), "{ok}");
        }
        for bad in ["", "..", ".", "../x.log", r"..\x.log", r"C:\Windows\x.log", "C:x.log", "a/b.log", "x.log:ads", "a\u{1}b"] {
            assert!(!bare_log_name(bad), "{bad}");
        }
    }

    #[test]
    fn each_profile_gets_its_own_log() {
        let a = json!({"id": "srv1a", "log": "vai_server.log"});
        let b = json!({"id": "srv2b"});
        assert_eq!(log_name(&a), "server_srv1a.log");
        assert_eq!(log_name(&b), "server_srv2b.log");
        assert_eq!(log_name(&json!({"id": "x", "log": "mine.log"})), "mine.log");
        assert_eq!(log_name(&json!({"id": "x", "log": r"C:\x.log"})), "server_x.log");
        assert_eq!(default_log(r"..\a"), "server_a.log");
    }

    #[test]
    fn a_reused_pid_is_not_a_server() {
        let live: HashSet<u32> = [100, 200].into_iter().collect();
        assert!(recorded_pid_usable(100, &live));
        // alive or not, a pid that is not a dedicated server is never killed by Stop
        assert!(!recorded_pid_usable(std::process::id(), &live));
        assert!(!recorded_pid_usable(0, &live));
    }

    #[test]
    fn memory_limit_follows_ram() {
        const GB: u64 = 1 << 30;
        assert_eq!(mem_limit(Some(64 * GB), None), 12 * GB);
        assert_eq!(mem_limit(Some(16 * GB), None), 12 * GB);
        assert_eq!(mem_limit(Some(8 * GB), None), 6 * GB);
        assert_eq!(mem_limit(None, None), 12 * GB);
        assert_eq!(mem_limit(Some(8 * GB), Some(20.0)), 20 * GB);
    }
}
