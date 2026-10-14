//! API routes answered in Rust. Anything not listed here is passed to the
//! Python backend (proxy.rs) - this table grows as modules are ported.
//!
//! Every ported route goes through the same gates as app.Handler: admin
//! (loopback or token) and the browser-forgery check, before it runs.

use std::net::IpAddr;

use axum::body::{to_bytes, Body};
use axum::extract::Request;
use axum::http::{header, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{json, Value};

use crate::{auth, config, detect};

pub fn json_resp(code: u16, v: &Value) -> Response {
    (
        StatusCode::from_u16(code).unwrap_or(StatusCode::OK),
        [(header::CONTENT_TYPE, "application/json"), (header::CACHE_CONTROL, "no-store")],
        serde_json::to_vec(v).unwrap_or_default(),
    )
        .into_response()
}

fn q_flag(query: &str, key: &str) -> bool {
    q_get(query, key).as_deref() == Some("1")
}

/// First value of `key` in a query string, percent-decoded.
pub fn q_get(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        (pct(k) == key).then(|| pct(v))
    })
}

fn pct(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                match u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16) {
                    Ok(x) => { out.push(x); i += 2; }
                    Err(_) => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn png(path: &std::path::Path) -> Response {
    match std::fs::read(path) {
        // cheap to regenerate, and a stale render after a repaint confuses
        Ok(b) => ([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "max-age=300")], b)
            .into_response(),
        Err(_) => json_resp(404, &json!({"error": "no image"})),
    }
}

/// List-row pictures. ⚠ Partial: a request that would MAKE one (render a
/// car, decode a cover) is handed back to the backend that owns that job.
async fn thumb(peer: IpAddr, req: Request, path: &str, query: &str) -> Result<Response, Request> {
    let cfg = config::load();
    let h = req.headers().clone();
    if !auth::admin_ok(&cfg, &peer, &h, query) {
        let (c, v) = auth::deny_remote(&cfg);
        return Ok(json_resp(c, &v));
    }
    if !auth::browser_ok(false, &peer, &h, query) {
        let (c, v) = auth::deny_browser();
        return Ok(json_resp(c, &v));
    }
    if path == "/api/thumb/car" {
        if q_flag(query, "big") || q_flag(query, "make") || q_flag(query, "force") {
            return Err(req);
        }
        let id = q_get(query, "id").unwrap_or_default();
        let preset = q_get(query, "preset").unwrap_or_default();
        let hit = tokio::task::spawn_blocking(move || crate::thumbs::list_car(&id, &preset))
            .await.ok().flatten();
        return Ok(match hit {
            Some(p) => png(&p),
            None => json_resp(404, &json!({"error": "no image"})),
        });
    }
    let folder = q_get(query, "folder").unwrap_or_default();
    let p = tokio::task::spawn_blocking(move || {
        crate::thumbs::track_path(&crate::thumbs::resolve_folder(&folder))
    }).await.ok();
    match p {
        Some(p) if p.is_file() => Ok(png(&p)),
        _ => Err(req), // not cached yet: the backend extracts or decodes it
    }
}

/// Run file-system work off the async threads.
async fn blocking<F: FnOnce() -> Value + Send + 'static>(f: F) -> Value {
    tokio::task::spawn_blocking(f).await.unwrap_or_else(|e| json!({"error": e.to_string()}))
}

/// Is this route ported? (method, path) - checked before reading the body.
fn ported(m: &Method, path: &str) -> bool {
    match *m {
        Method::GET => matches!(path, "/api/config" | "/api/detect" | "/api/auth"
            | "/api/version" | "/api/logs" | "/api/logs/files"
            | "/api/mods" | "/api/mods/audit" | "/api/cars/map" | "/api/viewer/cars"
            | "/api/cars" | "/api/tracks"),
        Method::POST => matches!(path, "/api/config"),
        _ => false,
    }
}

/// Answer a ported route, or hand the request back for the backend.
pub async fn handle(peer: IpAddr, req: Request) -> Result<Response, Request> {
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();
    // ⚠ debug builds only: lets tools/diff_api.py ask the Python backend
    // the same question, to prove a ported route answers identically
    if cfg!(debug_assertions) && auth::is_loopback(&peer)
        && req.headers().contains_key("x-acecm-force-python")
    {
        return Err(req);
    }
    if req.method() == Method::GET && (path == "/api/thumb/car" || path == "/api/thumb/track") {
        return thumb(peer, req, &path, &query).await;
    }
    if !ported(req.method(), &path) {
        return Err(req);
    }
    let cfg = config::load();
    let write = req.method() == Method::POST;
    let h = req.headers().clone();
    let gated = write || !auth::SHARE_GET.contains(&path.as_str());
    if gated && !auth::admin_ok(&cfg, &peer, &h, &query) {
        let (c, v) = auth::deny_remote(&cfg);
        return Ok(json_resp(c, &v));
    }
    if gated && !auth::browser_ok(write, &peer, &h, &query) {
        let (c, v) = auth::deny_browser();
        return Ok(json_resp(c, &v));
    }
    let body: Value = if write {
        let bytes = to_bytes(req.into_body(), 16 << 20).await.unwrap_or_default();
        serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}))
    } else {
        drop(req);
        Value::Null
    };
    let out = match (write, path.as_str()) {
        (false, "/api/config") => json_resp(200, &config::public_cfg(&cfg)),
        (false, "/api/detect") => {
            let refresh = q_flag(&query, "refresh");
            let v = tokio::task::spawn_blocking(move || detect::all_paths(refresh))
                .await.unwrap_or_else(|e| json!({"error": e.to_string()}));
            json_resp(200, &v)
        }
        (false, "/api/auth") => json_resp(200, &json!({
            "ok": true,
            "remote_admin": auth::enabled(&cfg),
            "authorised": auth::admin_ok(&cfg, &peer, &h, &query),
            "local": auth::is_loopback(&peer),
        })),
        (false, "/api/version") => json_resp(200, &json!({
            "name": "Assetto Corsa EVO Content Manager",
            "version": env!("CARGO_PKG_VERSION"),
            // the installer/updater still live in the (source) backend
            "frozen": false,
            "data_dir": crate::paths::data().to_string_lossy(),
        })),
        (false, "/api/logs") => {
            let n = q_get(&query, "lines").and_then(|v| v.parse().ok()).unwrap_or(300);
            let f = q_get(&query, "file").unwrap_or_else(|| "acecm.log".into());
            json_resp(200, &crate::logs::tail(n, &f))
        }
        (false, "/api/logs/files") => json_resp(200, &crate::logs::files()),
        (false, "/api/mods") => {
            let side = q_get(&query, "side").unwrap_or_else(|| "server".into());
            json_resp(200, &blocking(move || crate::mods::installed(&side)).await)
        }
        (false, "/api/viewer/cars") => {
            let refresh = q_flag(&query, "refresh");
            json_resp(200, &blocking(move || crate::viewer::index(refresh)).await)
        }
        (false, "/api/cars") => json_resp(200, &blocking(crate::content::cars).await),
        (false, "/api/tracks") => json_resp(200, &blocking(crate::content::tracks).await),
        (false, "/api/mods/audit") => json_resp(200, &blocking(crate::mods::audit).await),
        (false, "/api/cars/map") => {
            let refresh = q_flag(&query, "refresh");
            let t = blocking(move || crate::carsmap::table(refresh)).await;
            let n = t.get("presets").and_then(|p| p.as_object()).map(|m| m.len()).unwrap_or(0);
            json_resp(200, &json!({
                "presets": t.get("presets").cloned().unwrap_or(Value::Null),
                "models": t.get("models").cloned().unwrap_or(Value::Null),
                "count": n,
                "error": t.get("error").cloned().unwrap_or(Value::Null),
            }))
        }
        (true, "/api/config") => match config::save(&body) {
            Ok(v) => json_resp(200, &v),
            Err(e) => json_resp(500, &json!({"error": format!("OSError: {e}")})),
        },
        _ => json_resp(404, &json!({"error": "not found"})),
    };
    Ok(out)
}

#[allow(dead_code)]
pub fn empty() -> Body {
    Body::empty()
}
