//! Who may call an API route - ported from acecm/auth.py and the gates at
//! the top of app.Handler.do_GET / _do_post.
//!
//! ⚠ Loopback stays unauthenticated (the desktop app must not need a token
//! to manage itself); anyone else needs the admin token, and only when
//! remote administration is on. ⚠ Separately, a WEB PAGE in the user's
//! browser can reach loopback too, so writes must come from our own origin
//! and loopback requests must use our own host names (DNS rebinding).

use std::net::IpAddr;

use axum::http::HeaderMap;
use serde_json::{json, Map, Value};

use crate::{config, paths};

pub fn is_loopback(ip: &IpAddr) -> bool {
    ip.to_canonical().is_loopback()
}

/// Routes anyone may GET with no token (the content share, the auth probe).
pub const SHARE_GET: &[&str] = &[
    "/api/auth",
    "/api/registry/list",
    "/api/registry/manifest",
    "/api/registry/file",
    "/api/registry/pack",
    "/live",
    "/live.html",
];

fn token_file() -> std::path::PathBuf {
    paths::data().join("admin_token.txt")
}

/// The stored admin token; never created here (the backend owns that).
pub fn token() -> String {
    std::fs::read_to_string(token_file()).map(|s| s.trim().to_string()).unwrap_or_default()
}

pub fn enabled(cfg: &Map<String, Value>) -> bool {
    cfg.get("remote_admin").and_then(|v| v.as_bool()).unwrap_or(false)
}

fn query_token(query: &str) -> Option<String> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == "token" && !v.is_empty()).then(|| v.to_string())
    })
}

/// The token a request carries: header, bearer, query, then cookie.
pub fn presented(h: &HeaderMap, query: &str) -> String {
    let hv = |n: &str| h.get(n).and_then(|v| v.to_str().ok()).unwrap_or("").trim().to_string();
    let got = hv("x-acecm-token");
    if !got.is_empty() {
        return got;
    }
    let a = hv("authorization");
    if a.to_ascii_lowercase().starts_with("bearer ") {
        return a[7..].trim().to_string();
    }
    if let Some(t) = query_token(query) {
        return t.trim().to_string();
    }
    for part in hv("cookie").split(';') {
        if let Some((k, v)) = part.trim().split_once('=') {
            if k == "acecm_token" && !v.is_empty() {
                return v.trim().to_string();
            }
        }
    }
    String::new()
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    // constant time for equal lengths, like hmac.compare_digest
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn token_ok(cfg: &Map<String, Value>, h: &HeaderMap, query: &str) -> bool {
    if !enabled(cfg) {
        return false;
    }
    let want = token();
    let got = presented(h, query);
    !want.is_empty() && !got.is_empty() && ct_eq(want.as_bytes(), got.as_bytes())
}

/// A token in a header or the query - never the ambient cookie.
fn explicit_token(h: &HeaderMap, query: &str) -> bool {
    let has = |n: &str| h.get(n).and_then(|v| v.to_str().ok()).map(|s| !s.trim().is_empty()).unwrap_or(false);
    has("x-acecm-token") || has("authorization") || query_token(query).is_some()
}

pub fn admin_ok(cfg: &Map<String, Value>, peer: &IpAddr, h: &HeaderMap, query: &str) -> bool {
    is_loopback(peer) || token_ok(cfg, h, query)
}

/// Refuse requests a web PAGE made without the user meaning to.
pub fn browser_ok(write: bool, peer: &IpAddr, h: &HeaderMap, query: &str) -> bool {
    let hv = |n: &str| h.get(n).and_then(|v| v.to_str().ok()).unwrap_or("").trim().to_ascii_lowercase();
    let host = hv("host");
    if write {
        if hv("sec-fetch-site") == "cross-site" {
            return false;
        }
        let origin = hv("origin");
        if !origin.is_empty() {
            if origin == "null" {
                return false;
            }
            // netloc of the origin must be exactly our Host
            let netloc = origin.split("://").nth(1).unwrap_or("").split('/').next().unwrap_or("");
            if netloc != host {
                return false;
            }
        }
    }
    if is_loopback(peer) && !host.is_empty() && !explicit_token(h, query) {
        let name = if host.starts_with('[') {
            format!("{}]", host.split(']').next().unwrap_or(""))
        } else {
            host.rsplit_once(':').map(|(n, _)| n.to_string()).unwrap_or(host.clone())
        };
        if !["127.0.0.1", "localhost", "[::1]"].contains(&name.as_str()) {
            return false;
        }
    }
    true
}

pub fn deny_remote(cfg: &Map<String, Value>) -> (u16, Value) {
    if enabled(cfg) {
        return (401, json!({
            "ok": false, "need_token": true,
            "error": "this ACECM is managed remotely - send the admin token as \
                      X-ACECM-Token, ?token=... or the acecm_token cookie"}));
    }
    (403, json!({"ok": false, "error": "that action is only allowed from this PC"}))
}

pub fn deny_browser() -> (u16, Value) {
    (403, json!({"ok": false,
                 "error": "refused: that request did not come from ACECM's own window"}))
}

#[allow(dead_code)]
pub fn cfg() -> Map<String, Value> {
    config::load()
}
