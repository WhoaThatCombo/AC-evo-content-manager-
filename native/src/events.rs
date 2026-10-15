//! One live snapshot of everything the UI shows as "in progress" - ported
//! from events.py. Streamed to every page over /api/events (SSE), readable
//! once from /api/events/snapshot.
//!
//! ⚠ BUILT BY ONE BACKGROUND THREAD, READ WITHOUT WAITING: a stuck probe
//! only makes the snapshot stale (see `age`) and is logged by name.

use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

const FAST: f64 = 0.8;  // drive + progress: the Join checklist and bar
const SLOW: f64 = 3.0;  // servers + proxy: human timescale
const WARN: f64 = 2.0;

struct State {
    snap: Option<Value>,
    slow_at: Option<Instant>,
    servers: Value,
    proxy: Value,
    lobby: Value,
}

static STATE: Mutex<State> = Mutex::new(State { snap: None, slow_at: None, servers: Value::Null, proxy: Value::Null, lobby: Value::Null });
static STARTED: Mutex<bool> = Mutex::new(false);
static WAKE: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn sv(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(x) => x.to_string(),
    }
}

fn timed(name: &str, f: impl FnOnce() -> Value) -> Value {
    let t = Instant::now();
    let v = f();
    let took = t.elapsed().as_secs_f64();
    if took > WARN {
        crate::logs::warn(format!("events: {name} took {took:.1}s"));
    }
    v
}

fn servers() -> Value {
    let st = crate::serverproc::start_state();
    let empty = Map::new();
    let starting = st.get("starting").and_then(|v| v.as_object()).unwrap_or(&empty).clone();
    let last = st.get("last_start").and_then(|v| v.as_object()).unwrap_or(&empty).clone();
    let t = now();
    let mut out = Vec::new();
    for p in crate::profiles::load() {
        let s = crate::serverproc::status(&p);
        let id = sv(&p, "id");
        let l = last.get(&id).cloned().unwrap_or(json!({}));
        let recent_fail = l.get("ok") != Some(&json!(true)) && t - l.get("at").and_then(|v| v.as_f64()).unwrap_or(0.0) < 120.0;
        out.push(json!({"id": p.get("id"), "name": sv(&p, "name"),
            "starting": starting.contains_key(&id),
            "start_error": if recent_fail { sv(&l, "error") } else { String::new() },
            "running": s.get("running").map(|v| v.as_bool().unwrap_or(false)).unwrap_or(false),
            "tcp_port": p.get("tcp_port").and_then(|v| v.as_i64()).unwrap_or(0),
            "clients": s.get("clients").cloned().unwrap_or(Value::Null)}));
    }
    json!(out)
}

/// Is the lobby proxy up, and is it serving OUR servers?
fn proxy() -> Value {
    let js = crate::backend::join_state();
    if !js.get("control").map(|v| !matches!(v, Value::Null | Value::Bool(false))).unwrap_or(false) {
        return json!({"up": false, "ours": false});
    }
    let theirs = sv(&js, "lobby");
    let norm = |p: &str| std::path::absolute(p).map(|a| a.to_string_lossy().to_lowercase()).unwrap_or_default();
    let ours = !theirs.is_empty() && norm(&theirs) == norm(&crate::lobby::path().to_string_lossy());
    json!({"up": true, "ours": ours, "client": js.get("client_connected").map(|v| v.as_bool().unwrap_or(false)).unwrap_or(false)})
}

fn lobby() -> Value {
    json!({"at": crate::backend::cached_at(&crate::paths::data().join("server_list.json")),
           "running": crate::backend::LOBBY.lock().unwrap().running})
}

fn build() -> Value {
    let slow_due = STATE.lock().unwrap().slow_at.map(|t| t.elapsed().as_secs_f64() >= SLOW).unwrap_or(true);
    if slow_due {
        let (s, p, l) = (timed("servers", servers), timed("proxy", proxy), timed("lobby", lobby));
        let mut st = STATE.lock().unwrap();
        st.servers = s;
        st.proxy = p;
        st.lobby = l;
        st.slow_at = Some(Instant::now());
    }
    let mut d = timed("drive", crate::drive::status);
    if let Some(m) = d.as_object_mut() {
        for k in ["pick", "wrote", "launch", "single_player"] {
            m.remove(k);
        }
    }
    let progress = timed("progress", crate::jobs::progress);
    let compress = timed("compress", crate::compress::status);
    let evoshare = timed("evoshare", crate::evoshare::status);
    let thumbs = timed("thumbs", crate::thumbs::job);
    let covers = timed("covers", crate::thumbs::cover_job);
    let mut st = STATE.lock().unwrap();
    let snap = json!({"ok": true, "at": now(), "drive": d, "progress": progress, "compress": compress,
        "evoshare": evoshare, "thumbs": thumbs, "covers": covers,
        "servers": if st.servers.is_null() { json!([]) } else { st.servers.clone() },
        "proxy": if st.proxy.is_null() { json!({}) } else { st.proxy.clone() },
        "lobby": if st.lobby.is_null() { json!({}) } else { st.lobby.clone() }});
    st.snap = Some(snap.clone());
    snap
}

/// Start the builder thread once.
pub fn start() {
    {
        let mut g = STARTED.lock().unwrap();
        if *g {
            return;
        }
        *g = true;
    }
    std::thread::Builder::new().name("events".into()).spawn(|| loop {
        let _ = std::panic::catch_unwind(build);
        // woken early by kick() after something starts
        let (m, cv) = &WAKE;
        let g = m.lock().unwrap();
        let (mut g, _) = cv.wait_timeout_while(g, Duration::from_secs_f64(FAST), |k| !*k).unwrap();
        *g = false;
    }).ok();
}

/// Rebuild now rather than at the next tick.
pub fn kick() {
    let (m, cv) = &WAKE;
    *m.lock().unwrap() = true;
    cv.notify_all();
}

/// The latest snapshot - never waits on a probe (except the very first).
pub fn snapshot(fresh: bool) -> Value {
    start();
    let cur = STATE.lock().unwrap().snap.clone();
    let snap = match cur {
        Some(s) if !fresh => s,
        _ => build(),
    };
    let mut out = snap.clone();
    let t = now();
    let at = snap.get("at").and_then(|v| v.as_f64()).unwrap_or(t);
    out["age"] = json!(((t - at) * 10.0).round_ties_even() / 10.0);
    if let Some(d) = out.get_mut("drive").and_then(|v| v.as_object_mut()) {
        d.insert("now".into(), json!(t));
    }
    out
}

/// The comparison key: the snapshot without its clocks.
pub fn key(s: &Value) -> String {
    let mut k = s.clone();
    k["at"] = json!(0);
    k["age"] = json!(0);
    if let Some(d) = k.get_mut("drive").and_then(|v| v.as_object_mut()) {
        d.insert("now".into(), json!(0));
    }
    k.to_string()
}
