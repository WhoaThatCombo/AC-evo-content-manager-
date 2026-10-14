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
    query.split('&').any(|kv| kv == format!("{key}=1"))
}

/// Is this route ported? (method, path) - checked before reading the body.
fn ported(m: &Method, path: &str) -> bool {
    match *m {
        Method::GET => matches!(path, "/api/config" | "/api/detect" | "/api/auth"),
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
