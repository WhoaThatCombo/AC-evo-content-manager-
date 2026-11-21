//! Who may call an API route - ported from acecm/auth.py and the gates at
//! the top of app.Handler.do_GET / _do_post.
//!
//! ⚠ Loopback stays unauthenticated (the desktop app must not need a token
//! to manage itself); anyone else needs the admin token, and only when
//! remote administration is on. ⚠ Separately, a WEB PAGE in the user's
//! browser can reach loopback too, so writes must come from our own origin
//! and loopback requests must use our own host names (DNS rebinding).
//!
//! ⚠ Remote administration is plain HTTP on the same port as the public
//! content share, and the admin token is equivalent to running code on this
//! PC (upload a file, point server_exe at it, start it). So the token is
//! only ever taken from a header or the SameSite cookie - never from the URL
//! - and remote admin should only be used over a LAN you trust, or through an
//! SSH tunnel / VPN to 127.0.0.1 (where no token is needed at all).

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
    "/api/registry/acemod",
    "/live",
    "/live.html",
];

fn token_file() -> std::path::PathBuf {
    paths::data().join("admin_token.txt")
}

/// The stored admin token (no new one is made by reading it).
pub fn token() -> String {
    std::fs::read_to_string(token_file()).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// The admin token, made on first use (secrets.token_urlsafe(24)). Kept in
/// a file so config.json stays something a user can paste into a report.
pub fn token_create() -> String {
    let got = token();
    if !got.is_empty() {
        return got;
    }
    use base64::Engine;
    let mut b = [0u8; 24];
    let _ = getrandom::fill(&mut b);
    let new = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b);
    let _ = std::fs::create_dir_all(paths::data());
    if let Err(e) = std::fs::write(token_file(), format!("{new}\n")) {
        crate::logs::warn(format!("could not save the admin token: {e}"));
    }
    new
}

/// Throw the current token away and issue another.
pub fn rotate() -> String {
    let _ = std::fs::remove_file(token_file());
    token_create()
}

pub fn enabled(cfg: &Map<String, Value>) -> bool {
    cfg.get("remote_admin").and_then(|v| v.as_bool()).unwrap_or(false)
}

/// The token a request carries: header, bearer, then cookie. ⚠ Never the
/// URL query: URLs end up in browser history, logs and pasted links, and
/// this token is as good as running code on the PC. (The web UI still takes
/// `?token=` from a pasted link once, client-side, and sends it as a header.)
pub fn presented(h: &HeaderMap, _query: &str) -> String {
    let hv = |n: &str| h.get(n).and_then(|v| v.to_str().ok()).unwrap_or("").trim().to_string();
    let got = hv("x-acecm-token");
    if !got.is_empty() {
        return got;
    }
    let a = hv("authorization");
    if a.to_ascii_lowercase().starts_with("bearer ") {
        return a[7..].trim().to_string();
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

pub fn admin_ok(cfg: &Map<String, Value>, peer: &IpAddr, h: &HeaderMap, query: &str) -> bool {
    is_loopback(peer) || token_ok(cfg, h, query)
}

/// Refuse requests a web PAGE made without the user meaning to.
///
/// ⚠ Applies to GETs too: several GET routes take raw paths (a cross-site
/// `<img src=".../api/mods/scan?path=\\host\x">` would make Windows hand the
/// user's NTLM hash to that host), so a cross-site page gets nothing.
/// ⚠ The loopback Host-name (DNS rebinding) check is skipped only when the
/// request carries a token that actually VERIFIES - carrying any token is not
/// enough, or `?token=x` would turn the rebinding check off.
pub fn browser_ok(cfg: &Map<String, Value>, _write: bool, peer: &IpAddr, h: &HeaderMap, query: &str) -> bool {
    let hv = |n: &str| h.get(n).and_then(|v| v.to_str().ok()).unwrap_or("").trim().to_ascii_lowercase();
    let host = hv("host");
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
    if is_loopback(peer) && !host.is_empty() && !token_ok(cfg, h, query) {
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
                      X-ACECM-Token, an Authorization: Bearer header or the acecm_token cookie"}));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn hdrs(kv: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in kv {
            h.insert(*k, v.parse().unwrap());
        }
        h
    }

    const LO: IpAddr = IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);

    #[test]
    fn loopback_own_ui_passes() {
        let cfg = Map::new();
        assert!(browser_ok(&cfg, false, &LO, &hdrs(&[("host", "127.0.0.1:8092"), ("sec-fetch-site", "same-origin")]), ""));
        assert!(browser_ok(&cfg, true, &LO, &hdrs(&[("host", "localhost:8092"), ("origin", "http://localhost:8092")]), ""));
        // a non-browser client (curl, the app itself)
        assert!(browser_ok(&cfg, true, &LO, &hdrs(&[("host", "127.0.0.1:8092")]), ""));
    }

    #[test]
    fn rebinding_not_bypassed_by_any_token() {
        // remote admin off: no token can verify, so the Host check always applies
        let cfg = Map::new();
        for h in [hdrs(&[("host", "evil.example:8092"), ("x-acecm-token", "x")]),
                  hdrs(&[("host", "evil.example:8092"), ("authorization", "Bearer x")]),
                  hdrs(&[("host", "evil.example:8092")])] {
            assert!(!browser_ok(&cfg, false, &LO, &h, "token=x"));
            assert!(!browser_ok(&cfg, true, &LO, &h, "token=x"));
        }
        // remote admin on, wrong token: still refused
        let mut on = Map::new();
        on.insert("remote_admin".into(), json!(true));
        let h = hdrs(&[("host", "evil.example:8092"), ("x-acecm-token", "definitely-not-the-token")]);
        assert!(!browser_ok(&on, false, &LO, &h, ""));
    }

    #[test]
    fn cross_site_gets_refused() {
        let cfg = Map::new();
        // <img src="http://127.0.0.1:8092/api/mods/scan?path=\\host\x">
        assert!(!browser_ok(&cfg, false, &LO, &hdrs(&[("host", "127.0.0.1:8092"), ("sec-fetch-site", "cross-site")]), ""));
        assert!(!browser_ok(&cfg, false, &LO, &hdrs(&[("host", "127.0.0.1:8092"), ("origin", "http://evil.example")]), ""));
        assert!(!browser_ok(&cfg, false, &LO, &hdrs(&[("host", "127.0.0.1:8092"), ("origin", "null")]), ""));
    }

    #[test]
    fn token_never_from_query() {
        assert_eq!(presented(&hdrs(&[]), "token=abc"), "");
        assert_eq!(presented(&hdrs(&[("x-acecm-token", "abc")]), ""), "abc");
        assert_eq!(presented(&hdrs(&[("authorization", "Bearer abc")]), ""), "abc");
        assert_eq!(presented(&hdrs(&[("cookie", "a=b; acecm_token=abc")]), ""), "abc");
    }
}
