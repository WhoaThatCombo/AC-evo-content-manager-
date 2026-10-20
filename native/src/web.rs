//! The UI's own files (acecm/web), served by the shell - ported from
//! app.Handler._static.
//!
//! ⚠ Served to anyone, like before: the page has to LOAD before it can ask a
//! remote viewer for a token. They carry no secrets; /api/ stays gated.

use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::paths;

fn web_dir() -> PathBuf {
    if let Ok(w) = std::env::var("ACECM_WEB") {
        return PathBuf::from(w);
    }
    crate::bundle::web_dir()
}

fn mime(p: &Path) -> &'static str {
    match p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase().as_str() {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Only plain relative names: no `..`, no drive or root, nothing that can
/// step outside the web folder.
fn safe_rel(rel: &str) -> Option<PathBuf> {
    let p = Path::new(rel);
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(s) => out.push(s),
            Component::CurDir => {}
            _ => return None,
        }
    }
    if out.as_os_str().is_empty() { None } else { Some(out) }
}

fn mtime(p: &Path) -> u64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Bust the WebView2 disk / V8 code cache whenever the UI files change: the
/// newest mtime of the scripts and stylesheets.
fn cache_tag(web: &Path) -> String {
    let mut newest = mtime(&web.join("style.css")).max(mtime(&web.join("redesign.css")));
    if let Ok(rd) = std::fs::read_dir(web.join("js")) {
        for e in rd.flatten() {
            if e.path().extension().and_then(|x| x.to_str()) == Some("js") {
                newest = newest.max(mtime(&e.path()));
            }
        }
    }
    // dev edition: rebuilt without a version bump, so the exe's own time too
    if crate::edition::is_dev() {
        if let Ok(exe) = std::env::current_exe() {
            newest = newest.max(mtime(&exe));
        }
    }
    newest.to_string()
}

fn theme() -> String {
    // ⚠ kept in config, not only localStorage: an update wipes the webview
    // profile. Injected before first paint so there is no flash of default.
    let c = paths::config();
    let t = c.get("ui_theme").and_then(|v| v.as_str()).unwrap_or("").trim();
    t.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(24).collect()
}

fn index_html(raw: String, tag: &str) -> String {
    let mut s = raw
        .replace(r#"href="/style.css""#, &format!(r#"href="/style.css?v={tag}""#))
        .replace(r#"href="/redesign.css""#, &format!(r#"href="/redesign.css?v={tag}""#));
    // every UI script (web/js/NN-*.js)
    let mut out = String::with_capacity(s.len() + 512);
    let needle = r#"src="/js/"#;
    while let Some(i) = s.find(needle) {
        let start = i + needle.len();
        let end = match s[start..].find('"') { Some(e) => start + e, None => break };
        let name = &s[start..end];
        out.push_str(&s[..end]);
        if name.ends_with(".js") && !name.contains('?') {
            out.push_str("?v=");
            out.push_str(tag);
        }
        s = s[end..].to_string();
    }
    out.push_str(&s);
    out.replace("__ACECM_THEME__", &theme())
}

pub fn serve(path: &str) -> Response {
    let rel = match path {
        "/" | "" => "index.html",
        "/live" | "/live.html" => "live.html",
        p => p.trim_start_matches('/'),
    };
    let web = web_dir();
    let Some(rel) = safe_rel(rel) else { return StatusCode::NOT_FOUND.into_response() };
    let full = web.join(&rel);
    let Ok(bytes) = std::fs::read(&full) else { return StatusCode::NOT_FOUND.into_response() };
    let body: Vec<u8> = if rel == Path::new("index.html") {
        index_html(String::from_utf8_lossy(&bytes).into_owned(), &cache_tag(&web)).into_bytes()
    } else {
        bytes
    };
    ([(header::CONTENT_TYPE, mime(&full)), (header::CACHE_CONTROL, "no-store")], body)
        .into_response()
}
