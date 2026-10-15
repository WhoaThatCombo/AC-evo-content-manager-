//! A second, live source of EVO servers: a public server directory - ported
//! from evoreg.py. A plain unauthenticated JSON endpoint, so it works with
//! the game closed. Its servers are usually modded / open-world ones and
//! each advertises the track it runs plus its own HTTP share.
//!
//! ⚠ Written from the endpoint's observed shape; we identify honestly.

use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

const BASES: [&str; 2] = ["https://registry.evoforge.fr/", "https://join.evoforge.fr/registry/"];
const TTL: f64 = 20.0;
// ⚠ short: this sits behind the Drive list the browser abandons at 12 s
const TIMEOUT: u64 = 3;
const FAIL_BACKOFF: f64 = 300.0;
const COLD_WAIT: f64 = 2.5;

struct Mem {
    at: f64,
    servers: Vec<Value>,
    base: String,
    failed_at: f64,
}

static MEM: Mutex<Mem> = Mutex::new(Mem { at: 0.0, servers: Vec::new(), base: String::new(), failed_at: 0.0 });
static REFRESHING: Mutex<bool> = Mutex::new(false);

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

pub fn ua() -> String {
    format!("ACECM/{} (+https://github.com/WhoaThatCombo/AC-evo-content-manager-)", env!("CARGO_PKG_VERSION"))
}

fn cache_path() -> std::path::PathBuf {
    crate::paths::data().join("evoforge_registry.json")
}

fn int(v: Option<&Value>) -> i64 {
    match v {
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).unwrap_or(0),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0),
        Some(Value::Bool(b)) => *b as i64,
        _ => 0,
    }
}

fn s(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(x) => x.to_string(),
    }
}

fn s_or(v: Option<&Value>) -> String {
    match v {
        Some(Value::Bool(false)) | Some(Value::Null) | None => String::new(),
        Some(Value::Number(n)) if n.as_f64() == Some(0.0) => String::new(),
        other => s(other),
    }
}

/// Their record -> the field names the rest of ACECM speaks.
fn norm(rec: &Value) -> Value {
    let ip = s_or(rec.get("ip")).trim().to_string();
    let port = int(rec.get("tcpPort"));
    let empty = json!({});
    let track = rec.get("track").filter(|t| t.is_object()).unwrap_or(&empty);
    let share = rec.get("share").filter(|t| t.is_object()).unwrap_or(&empty);
    let live = rec.get("live").filter(|t| t.is_object()).unwrap_or(&empty);
    let info = rec.get("info").filter(|t| t.is_object()).unwrap_or(&empty);
    let key = s_or(rec.get("key"));
    let tid = s_or(track.get("id"));
    json!({
        "server_id": if key.is_empty() { format!("{ip}:{port}") } else { key },
        "server_name": ({ let n = s_or(rec.get("name")); if n.is_empty() { "(unnamed)".to_string() } else { n } }),
        "server_ip": ip, "server_tcp_port": port, "server_udp_port": port,
        "track": if tid.is_empty() { s_or(track.get("name")) } else { tid },
        "layout": s_or(track.get("layout")),
        // ⚠ live.players is the LIST of drivers; the number is live.count
        "players": int(live.get("count")),
        "max_players": int(info.get("pits")),
        "game_mode": s_or(info.get("mode")).to_uppercase(),
        "source": "evoforge",
        "track_origin": s_or(track.get("origin")),
        "share_url": s_or(share.get("url")),
        "content": rec.get("content").filter(|c| c.as_array().map(|a| !a.is_empty()).unwrap_or(false)).cloned().unwrap_or(json!([])),
        "country": s_or(rec.get("country")),
        "page": s_or(rec.get("page")),
    })
}

fn get(base: &str) -> Result<Value, String> {
    let c = reqwest::blocking::Client::builder().timeout(Duration::from_secs(TIMEOUT)).user_agent(ua()).build().map_err(|e| e.to_string())?;
    let r = c.get(format!("{base}?a=list")).header("Accept", "application/json").send().map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("HTTP Error {}", r.status().as_u16()));
    }
    let t = r.text().map_err(|e| e.to_string())?;
    serde_json::from_str(&t).map_err(|e| e.to_string())
}

fn kick() {
    {
        let mut g = REFRESHING.lock().unwrap();
        if *g {
            return;
        }
        *g = true;
    }
    std::thread::spawn(|| {
        fetch(true, true);
        *REFRESHING.lock().unwrap() = false;
    });
}

/// The directory, memory-cached for TTL and disk-cached across runs.
/// ⚠ `block=false` on any UI path: never depend on someone else's uptime.
pub fn fetch(force: bool, block: bool) -> Value {
    let t = now();
    {
        let m = MEM.lock().unwrap();
        if !force && t - m.at < TTL && !m.servers.is_empty() {
            return json!({"ok": true, "servers": m.servers, "cached": true, "base": m.base});
        }
    }
    if !block {
        if t - MEM.lock().unwrap().failed_at > FAIL_BACKOFF {
            kick();
        }
        // with NOTHING yet, wait briefly for the refresh in flight
        let until = Instant::now() + Duration::from_secs_f64(COLD_WAIT);
        while Instant::now() < until && MEM.lock().unwrap().servers.is_empty() {
            std::thread::sleep(Duration::from_millis(100));
        }
        let m = MEM.lock().unwrap();
        return json!({"ok": true, "servers": m.servers, "cached": true, "stale": true, "base": m.base});
    }
    let mut last = String::new();
    for base in BASES {
        let d = match get(base) {
            Ok(d) => d,
            Err(e) => { last = e; continue }
        };
        if d.get("ok").map(|v| !matches!(v, Value::Bool(false) | Value::Null)).unwrap_or(false) && d.is_object() {
            let servers: Vec<Value> = d.get("servers").and_then(|v| v.as_array()).cloned().unwrap_or_default().iter()
                .filter(|s| !s.is_null() && !s_or(s.get("ip")).is_empty() && int(s.get("tcpPort")) != 0).map(norm).collect();
            {
                let mut m = MEM.lock().unwrap();
                m.at = t;
                m.servers = servers.clone();
                m.base = base.to_string();
            }
            let _ = std::fs::write(cache_path(), crate::profiles::py_dumps(&json!({"at": t, "servers": servers})));
            return json!({"ok": true, "servers": servers, "cached": false, "base": base});
        }
        last = "registry said no".into();
    }
    // offline: a stale directory still says which servers exist
    if let Some(d) = std::fs::read(cache_path()).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) {
        return json!({"ok": true, "servers": d.get("servers").cloned().unwrap_or(json!([])), "cached": true, "stale": true, "error": last});
    }
    MEM.lock().unwrap().failed_at = now();
    crate::logs::info(format!("server directory unavailable: {last}"));
    json!({"ok": false, "servers": [], "error": if last.is_empty() { "unavailable".to_string() } else { last }})
}
