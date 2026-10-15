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

/// app._base(): links in manifests point back at whatever host the joiner
/// reached us on.
fn base_url(h: &axum::http::HeaderMap) -> String {
    let host = h.get(header::HOST).and_then(|v| v.to_str().ok()).map(String::from)
        .unwrap_or_else(|| format!("127.0.0.1:{}", config::load().get("ui_port").and_then(|v| v.as_i64()).unwrap_or(8092)));
    format!("http://{host}")
}

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

/// Stream a file, honouring `Range: bytes=a-b` the way app._serve_content
/// does (a bad or unsatisfiable range is answered with the whole file).
/// ⚠ Ranges are what let a joiner pull one big file over several parallel
/// connections - one TCP stream on a real internet path is capped far
/// below either end's link.
async fn serve_file(path: &std::path::Path, range: &str, ctype: &'static str, filename: &str, sha: &str) -> Response {
    use futures_util::StreamExt;
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let Ok(mut f) = tokio::fs::File::open(path).await else {
        return json_resp(404, &json!({"error": "not a declared content file"}));
    };
    let size = f.metadata().await.map(|m| m.len()).unwrap_or(0);
    let (mut start, mut end, mut status) = (0u64, size.saturating_sub(1), 200u16);
    if let Some(r) = range.strip_prefix("bytes=") {
        let parsed = r.split_once('-').and_then(|(a, b)| {
            let a = if a.is_empty() { Some(0) } else { a.trim().parse::<i64>().ok() };
            let b = if b.is_empty() { Some(size as i64 - 1) } else { b.trim().parse::<i64>().ok() };
            Some((a?, b?.min(size as i64 - 1)))
        });
        match parsed {
            Some((a, b)) if 0 <= a && a <= b && (b as u64) < size => { start = a as u64; end = b as u64; status = 206; }
            _ => { start = 0; end = size.saturating_sub(1); }
        }
    }
    let length = if size == 0 { 0 } else { end - start + 1 };
    if f.seek(std::io::SeekFrom::Start(start)).await.is_err() {
        return json_resp(500, &json!({"error": "seek failed"}));
    }
    let stream = futures_util::stream::unfold((f, length), |(mut f, left)| async move {
        if left == 0 {
            return None;
        }
        let mut buf = vec![0u8; (left.min(1 << 22)) as usize];
        match f.read(&mut buf).await {
            Ok(0) | Err(_) => None,
            Ok(n) => { buf.truncate(n); Some((Ok::<_, std::io::Error>(axum::body::Bytes::from(buf)), (f, left - n as u64))) }
        }
    }).boxed();
    let mut b = Response::builder().status(status)
        .header(header::CONTENT_TYPE, ctype)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, length.to_string());
    if status == 206 {
        b = b.header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{size}"));
    }
    if !sha.is_empty() {
        b = b.header("X-ACECM-SHA256", sha);
    }
    b.header(header::CONTENT_DISPOSITION, format!("attachment; filename=\"{filename}\""))
        .body(Body::from_stream(stream)).unwrap_or_else(|_| json_resp(500, &json!({"error": "bad response"})))
}

/// Python bool() of a JSON value
fn bb_of(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Bool(b)) => *b,
        Some(Value::Null) | None => false,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

/// A server profile by id (ids compare as strings, as `str(x.get("id"))`).
fn find_profile_s(sid: &str) -> Option<Value> {
    crate::profiles::load().into_iter().find(|p| match p.get("id") {
        Some(Value::String(s)) => s == sid,
        Some(Value::Null) | None => sid == "None",
        Some(v) => v.to_string() == sid,
    })
}

/// One dragged-in file, streamed into its drop staging folder. ⚠ Exactly
/// Content-Length bytes: an HTTP/1.1 browser keeps the connection open and
/// never sends EOF, and a short upload must not be handed on as the file.
async fn drop_part(req: Request, query: &str) -> Response {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;
    let did = q_get(query, "id").unwrap_or_default();
    let name = q_get(query, "name").or_else(|| req.headers().get("X-Filename").and_then(|v| v.to_str().ok()).map(String::from)).unwrap_or_default();
    let size: u64 = req.headers().get(header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).unwrap_or(0);
    let (d2, n2) = (did.clone(), name.clone());
    let opened = tokio::task::spawn_blocking(move || crate::install::drop_part_open(&d2, &n2)).await;
    let (dest, f) = match opened {
        Ok(Ok(x)) => x,
        Ok(Err(v)) => return json_resp(400, &v),
        Err(e) => return json_resp(500, &json!({"ok": false, "error": e.to_string()})),
    };
    let mut f = tokio::fs::File::from_std(f);
    let mut stream = req.into_body().into_data_stream();
    let mut wrote: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else { break };
        wrote += chunk.len() as u64;
        if wrote > crate::install::DROP_LIMIT {
            drop(f);
            let _ = tokio::fs::remove_file(&dest).await;
            return json_resp(400, &json!({"ok": false, "error": "file is larger than 8 GB"}));
        }
        if let Err(e) = f.write_all(&chunk).await {
            return json_resp(400, &json!({"ok": false, "error": e.to_string()}));
        }
        if size != 0 && wrote >= size {
            break;
        }
    }
    let _ = f.flush().await;
    drop(f);
    if size != 0 && wrote != size {
        let _ = tokio::fs::remove_file(&dest).await;
        return json_resp(400, &json!({"ok": false, "error": format!("upload stopped early ({wrote} of {size} bytes)")}));
    }
    let shown = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    json_resp(200, &json!({"ok": true, "name": shown, "bytes": wrote, "size": if size != 0 { size } else { wrote }}))
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

static SECRET: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static BACKEND_URL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static QUIT: std::sync::OnceLock<Box<dyn Fn() + Send + Sync>> = std::sync::OnceLock::new();

/// How the app quits (the window owner hands this in): Quit in the UI must
/// close the SHELL, which then stops the backend and the helpers.
pub fn set_quit(f: impl Fn() + Send + Sync + 'static) {
    let _ = QUIT.set(Box::new(f));
}

pub fn set_secret(s: &str) {
    let _ = SECRET.set(s.to_string());
}

pub fn set_backend(u: &str) {
    let _ = BACKEND_URL.set(u.to_string());
}

/// A GET to the Python backend, for the parts of an answer it still owns
/// (blocking: call inside spawn_blocking).
pub fn backend_get(path: &str) -> Value {
    let (Some(base), Some(sec)) = (BACKEND_URL.get(), SECRET.get()) else { return Value::Null };
    reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(10)).build().ok()
        .and_then(|c| c.get(format!("{base}{path}")).header("Host", "127.0.0.1")
            .header("X-ACECM-Peer", "127.0.0.1").header("X-ACECM-Proxy", sec).send().ok())
        .and_then(|r| r.text().ok()).and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null)
}

/// ⚠ The reverse bridge: the backend asks the shell to start/stop servers
/// (one owner of server processes). Only a loopback caller holding the
/// per-launch secret is answered.
async fn native_internal(peer: IpAddr, req: Request, path: &str) -> Response {
    let ok = auth::is_loopback(&peer) && SECRET.get().map(|s| {
        req.headers().get("x-acecm-proxy").and_then(|v| v.to_str().ok()) == Some(s.as_str())
    }).unwrap_or(false);
    if !ok {
        return json_resp(403, &json!({"ok": false, "error": "forbidden"}));
    }
    let bytes = to_bytes(req.into_body(), 16 << 20).await.unwrap_or_default();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(json!({}));
    let v = match path {
        "/api/_native/server/start" => {
            let p = body.get("profile").cloned().unwrap_or(json!({}));
            blocking(move || crate::serverproc::start(&p)).await
        }
        "/api/_native/server/stop" => {
            let id = body.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            blocking(move || crate::serverproc::stop(&id)).await
        }
        "/api/_native/server/state" => crate::serverproc::start_state(),
        "/api/_native/backend/start" => {
            let mode = body.get("mode").and_then(|v| v.as_str()).unwrap_or("proxy").to_string();
            blocking(move || crate::backend::start(&mode)).await
        }
        "/api/_native/backend/stop" => blocking(crate::backend::stop).await,
        "/api/_native/backend/state" => blocking(crate::backend::state).await,
        "/api/_native/lobby/fetch" => {
            let wait = body.get("wait").and_then(|v| v.as_bool()).unwrap_or(true);
            let t = body.get("timeout").and_then(|v| v.as_u64()).unwrap_or(30);
            blocking(move || crate::backend::lobby_fetch(wait, t)).await
        }
        "/api/_native/lobby/prefetch" => {
            let age = body.get("max_age").and_then(|v| v.as_u64()).unwrap_or(300);
            json!(crate::backend::lobby_prefetch(age))
        }
        "/api/_native/lobby/servers" => blocking(crate::backend::server_list).await,
        "/api/_native/lobby/state" => json!({"running": crate::backend::LOBBY.lock().unwrap().running}),
        "/api/_native/game/launch" => {
            let extra: Vec<String> = body.get("extra_args").and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
            blocking(move || crate::launch::launch_game(&extra)).await
        }
        "/api/_native/registry/forget" => { crate::registry::forget_public_sizes(); json!({"ok": true}) }
        "/api/_native/registry/pack" => {
            let folder = body.get("folder").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let p = blocking(move || json!(crate::registry::ensure_track_pack(&folder).map(|p| p.to_string_lossy().into_owned()))).await;
            json!({"path": p})
        }
        "/api/_native/progress" => blocking(crate::jobs::progress).await,
        "/api/_native/drive/status" => blocking(crate::drive::status).await,
        "/api/_native/drive/ensure_backend" => blocking(crate::drive::ensure_backend).await,
        _ => json!({"ok": false, "error": "not found"}),
    };
    json_resp(200, &v)
}

fn find_profile(id: &str) -> Option<Value> {
    crate::profiles::load().into_iter().find(|p| p.get("id").and_then(|v| v.as_str()) == Some(id))
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
            | "/api/cars" | "/api/tracks"
            | "/api/gamesettings" | "/api/gamesettings/read" | "/api/gamesettings/backups"
            | "/api/assists" | "/api/drive/fav" | "/api/tracks/installed"
            | "/api/trackdeploy" | "/api/tracks/missing" | "/api/splines"
            | "/api/share/auto" | "/api/browser/local" | "/api/lobby"
            | "/api/profiles" | "/api/server/status" | "/api/server/log"
            | "/api/backend" | "/api/backend/log" | "/api/join/state" | "/api/browser/why"
            | "/api/browser" | "/api/drive/status"
            | "/api/registry/list" | "/api/registry/manifest" | "/api/registry/file" | "/api/registry/pack"
            | "/api/browser/status" | "/api/browser/discover" | "/api/browser/tag" | "/api/browser/needs"
            | "/api/browser/plan" | "/api/share" | "/api/progress" | "/api/drop/status" | "/api/mods/scan"
            | "/api/library" | "/api/library/clip" | "/api/library/export" | "/api/pack/preview"
            | "/api/registry" | "/api/gamesettings/export" | "/api/models"),
        Method::POST => matches!(path, "/api/config" | "/api/splines/ship"
            | "/api/trackdeploy/deploy" | "/api/trackdeploy/restore"
            | "/api/trackdeploy/redeclare" | "/api/trackdeploy/redeclare_client"
            | "/api/profiles/save" | "/api/profiles/delete" | "/api/server/start" | "/api/server/stop"
            | "/api/backend/start" | "/api/backend/stop" | "/api/backend/redirect" | "/api/join"
            | "/api/app/quit" | "/api/drive" | "/api/drive/cancel" | "/api/drive/capture"
            | "/api/drive/list" | "/api/backend/ensure" | "/api/game/launch"
            | "/api/browser/install" | "/api/browser/scan" | "/api/tracks/restore" | "/api/push/send"
            | "/api/push/plan" | "/api/push/server_build" | "/api/server_build" | "/api/mods/install"
            | "/api/drop" | "/api/drop/part" | "/api/library/remove" | "/api/library/export" | "/api/mods/fix"
            | "/api/mods/remove" | "/api/mods/sync" | "/api/pack/build" | "/api/drive/direct"
            | "/api/drive/fav/add" | "/api/drive/fav/remove"
            | "/api/assists" | "/api/gamesettings/write" | "/api/gamesettings/restore"
            | "/api/gamesettings/import" | "/api/gamesettings/restore_backup"
            | "/api/registry/save" | "/api/registry/delete" | "/api/share/track"
            | "/api/auth/rotate" | "/api/auth/remote"),
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
    if path.starts_with("/api/_native/") {
        return Ok(native_internal(peer, req, &path).await);
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
    // a drag-drop STREAMS the file: never read it as a JSON body
    if write && path == "/api/drop/part" {
        return Ok(drop_part(req, &query).await);
    }
    // ⚠ The body is read ONCE and kept, so a route that turns out to need
    // the backend (decided from the body) can still be passed on intact.
    let (parts, rbody) = req.into_parts();
    let bytes = if write { to_bytes(rbody, 16 << 20).await.unwrap_or_default() } else { Default::default() };
    let body: Value = if write {
        serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}))
    } else {
        Value::Null
    };
    let bs = |k: &str| body.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let bb = |k: &str| match body.get(k) {
        Some(Value::Bool(b)) => *b,
        Some(Value::Null) | None => false,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
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
        (false, "/api/gamesettings") => json_resp(200, &blocking(|| {
            let mut v = crate::gamesettings::state();
            if let (Some(a), Value::Object(b)) = (v.as_object_mut(), crate::gamesettings::discover()) {
                a.extend(b);
            }
            v
        }).await),
        (false, "/api/gamesettings/read") => {
            let f = q_get(&query, "file").unwrap_or_default();
            json_resp(200, &blocking(move || crate::gamesettings::read(&f)).await)
        }
        (false, "/api/gamesettings/backups") => {
            let f = q_get(&query, "file").unwrap_or_default();
            json_resp(200, &blocking(move || crate::gamesettings::backups(&f)).await)
        }
        (false, "/api/assists") => json_resp(200, &blocking(crate::assists::read).await),
        (false, "/api/drive/fav") => json_resp(200, &blocking(crate::misc::favourites).await),
        (false, "/api/tracks/installed") => json_resp(200, &blocking(crate::misc::tracks_installed).await),
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
        (false, "/api/trackdeploy") => {
            let dir = q_get(&query, "dir");
            json_resp(200, &blocking(move || {
                let mut v = crate::trackdeploy::state();
                if let (Some(a), Value::Object(b)) = (v.as_object_mut(), crate::trackdeploy::packages(dir.as_deref())) {
                    a.extend(b);
                }
                v["imported"] = json!(crate::trackimport::importable());
                v
            }).await)
        }
        (false, "/api/tracks/missing") => json_resp(200, &blocking(|| {
            let m = crate::trackdeploy::missing_from_client();
            json!({"ok": true, "count": m.len(), "missing": m})
        }).await),
        (false, "/api/share/auto") => json_resp(200, &blocking(crate::autoshare::overview).await),
        (false, "/api/browser/local") => {
            let refresh = q_flag(&query, "refresh");
            json_resp(200, &blocking(move || {
                let tm = crate::trackimport::track_map(refresh);
                let cars: Vec<String> = {
                    let mut v: Vec<String> = crate::mods::installed("client").get("mods").and_then(|m| m.as_array())
                        .map(|a| a.iter().filter_map(|m| m.get("name")?.as_str().filter(|s| !s.is_empty()).map(String::from)).collect())
                        .unwrap_or_default();
                    v.sort();
                    v
                };
                let mut ids: std::collections::BTreeSet<String> = crate::carsmap::table(false).get("presets")
                    .and_then(|p| p.as_object()).map(|o| o.keys().cloned().collect()).unwrap_or_default();
                for m in crate::mods::installed("client").get("mods").and_then(|m| m.as_array()).cloned().unwrap_or_default() {
                    for c in m.get("cars").and_then(|c| c.as_array()).cloned().unwrap_or_default() {
                        if let Some(id) = c.get("id").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) { ids.insert(id.into()); }
                    }
                }
                json!({"tracks": crate::trackimport::installed_tracks(), "cars": cars,
                       "car_ids": ids.into_iter().collect::<Vec<_>>(), "track_map": tm,
                       "tracks_dir": crate::trackimport::tracks_dir()})
            }).await)
        }
        (false, "/api/lobby") => json_resp(200, &blocking(|| json!({
            "path": crate::lobby::path().to_string_lossy(), "lobby": crate::lobby::read(),
            "lan_ip": crate::misc::lan_ipv4()})).await),
        (false, "/api/splines") => json_resp(200, &blocking(crate::splines::status).await),
        (true, "/api/splines/ship") => {
            let (all, f, l) = (bb("all"), bs("folder"), bs("layout"));
            json_resp(200, &blocking(move || if all { crate::splines::ship("", "") } else { crate::splines::ship(&f, &l) }).await)
        }
        (true, "/api/trackdeploy/deploy") => {
            // native deploy of an imported track folder is ours; a slot-borrow
            // package (manifest.json) or the legacy injector is the backend's
            let p = bs("path");
            let native = bb("native");
            if !native || p.is_empty() || std::path::Path::new(&p).join("manifest.json").is_file() {
                return Err(Request::from_parts(parts, Body::from(bytes)));
            }
            let dry = bb("dry_run");
            json_resp(200, &blocking(move || crate::trackdeploy::deploy_native(std::path::Path::new(&p), dry, None, None)).await)
        }
        (true, "/api/trackdeploy/restore") => json_resp(200, &blocking(crate::trackdeploy::restore).await),
        (true, "/api/trackdeploy/redeclare") => {
            let dry = bb("dry_run");
            json_resp(200, &blocking(move || crate::trackdeploy::redeclare_tracks(dry)).await)
        }
        (true, "/api/trackdeploy/redeclare_client") => {
            let dry = bb("dry_run");
            json_resp(200, &blocking(move || crate::trackdeploy::redeclare_client_tracks(dry)).await)
        }
        (false, "/api/profiles") => json_resp(200, &blocking(|| {
            let mut items = crate::profiles::load();
            for it in items.iter_mut() {
                it["car_policy"] = crate::servers::car_policy(it);
                if it.get("allow_kunos").is_none() {
                    let cat = crate::content::cars().get("cars").and_then(|c| c.as_array()).cloned().unwrap_or_default();
                    it["allow_kunos"] = json!(crate::servers::infer_allow_kunos(it, &cat, &crate::mods::car_names()));
                }
            }
            json!({"profiles": items, "template": crate::serverproc::template(),
                   "options": crate::serverproc::options(),
                   // telemetry is still the backend's
                   "telemetry": backend_get("/api/telemetry/status")})
        }).await),
        (false, "/api/server/status") => {
            let id = q_get(&query, "id").unwrap_or_default();
            match find_profile(&id) {
                Some(p) => json_resp(200, &blocking(move || crate::serverproc::status(&p)).await),
                None => json_resp(404, &json!({"error": "no such profile"})),
            }
        }
        (false, "/api/server/log") => {
            let id = q_get(&query, "id").unwrap_or_default();
            let since = q_get(&query, "since").and_then(|v| v.parse().ok()).unwrap_or(0);
            let p = find_profile(&id).unwrap_or(json!({}));
            json_resp(200, &blocking(move || crate::serverproc::log_tail(&p, 120, since)).await)
        }
        (true, "/api/profiles/save") => {
            let b = body.clone();
            json_resp(200, &blocking(move || crate::serverproc::upsert(&b)).await)
        }
        (true, "/api/profiles/delete") => {
            let id = bs("id");
            blocking(move || { crate::serverproc::delete(&id); Value::Null }).await;
            json_resp(200, &json!({"ok": true}))
        }
        (true, "/api/server/start") => {
            let Some(p) = find_profile(&bs("id")) else {
                return Ok(json_resp(404, &json!({"ok": false, "error": "no profile"})));
            };
            // ⚠ off the request: a start that deploys a track runs past the
            // page's 12 s limit. Answer within 8 s; a slower start finishes
            // in the background and reports through the live feed.
            let h = tokio::task::spawn_blocking(move || crate::serverproc::start(&p));
            match tokio::time::timeout(std::time::Duration::from_secs(8), h).await {
                Ok(Ok(v)) => json_resp(200, &v),
                _ => json_resp(200, &json!({"ok": true, "starting": true,
                    "note": "still starting (deploying the track) - it will show as live when it is up"})),
            }
        }
        (true, "/api/server/stop") => {
            let id = bs("id");
            json_resp(200, &blocking(move || crate::serverproc::stop(&id)).await)
        }
        (false, "/api/backend") => json_resp(200, &blocking(crate::backend::state).await),
        (false, "/api/backend/log") => {
            let mode = q_get(&query, "mode").unwrap_or_else(|| "proxy".into());
            json_resp(200, &crate::backend::log(&mode, 80))
        }
        (true, "/api/app/quit") => {
            // answer first, then go (the page shows "closed" on the reply)
            std::thread::spawn(|| {
                std::thread::sleep(std::time::Duration::from_millis(300));
                if let Some(q) = QUIT.get() { q() }
            });
            json_resp(200, &json!({"ok": true, "quitting": true}))
        }
        (false, "/api/browser") => json_resp(200, &blocking(crate::backend::server_list).await),
        (false, "/api/join/state") => json_resp(200, &blocking(crate::backend::join_state).await),
        (false, "/api/browser/why") => json_resp(200, &blocking(crate::backend::browser_chain).await),
        (true, "/api/backend/start") => {
            let mode = { let m = bs("mode"); if m.is_empty() { "proxy".to_string() } else { m } };
            json_resp(200, &blocking(move || crate::backend::start(&mode)).await)
        }
        (false, "/api/registry/list") => {
            let base = base_url(&h);
            json_resp(200, &blocking(move || crate::registry::public_list(&base)).await)
        }
        (false, "/api/registry/manifest") => {
            let (base, id) = (base_url(&h), q_get(&query, "id").unwrap_or_default());
            json_resp(200, &blocking(move || crate::registry::manifest(&id, &base, true)).await)
        }
        (false, "/api/registry/file") => {
            let (id, rel) = (q_get(&query, "id").unwrap_or_default(), q_get(&query, "path").unwrap_or_default());
            let full = tokio::task::spawn_blocking(move || crate::registry::resolve(&id, &rel)).await.ok().flatten();
            match full.filter(|p| p.is_file()) {
                Some(p) => {
                    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let rng = h.get(header::RANGE).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                    serve_file(&p, &rng, "application/octet-stream", &name, "").await
                }
                None => json_resp(404, &json!({"error": "not a declared content file"})),
            }
        }
        (false, "/api/registry/pack") => {
            let (id, folder) = (q_get(&query, "id").unwrap_or_default(), q_get(&query, "track").unwrap_or_default().trim().to_string());
            let (i2, f2) = (id.clone(), folder.clone());
            if !tokio::task::spawn_blocking(move || crate::registry::shares_track(&i2, &f2)).await.unwrap_or(false) {
                json_resp(404, &json!({"error": "track is not shared on that entry"}))
            } else {
                let f2 = folder.clone();
                match tokio::task::spawn_blocking(move || crate::registry::ensure_track_pack(&f2)).await.ok().flatten().filter(|p| p.is_file()) {
                    None => json_resp(404, &json!({"error": "no files for that track"})),
                    Some(p) => {
                        let sha = crate::registry::track_pack_sha(&folder);
                        let rng = h.get(header::RANGE).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                        serve_file(&p, &rng, "application/x-tar", &format!("{folder}.tar"), &sha).await
                    }
                }
            }
        }
        (false, "/api/registry") => json_resp(200, &json!({"servers": crate::registry::load(), "template": crate::registry::template()})),
        (false, "/api/gamesettings/export") => {
            let only: Vec<String> = query.split('&').filter_map(|kv| kv.split_once('=')).filter(|(k, _)| *k == "file")
                .map(|(_, v)| q_get(&format!("x={v}"), "x").unwrap_or_default()).filter(|v| !v.is_empty()).collect();
            json_resp(200, &blocking(move || crate::gamesettings::export_bundle(if only.is_empty() { None } else { Some(&only) })).await)
        }
        (false, "/api/models") => json_resp(200, &blocking(crate::content::models_seen).await),
        (true, "/api/assists") => { let b = body.clone(); json_resp(200, &blocking(move || crate::assists::write(&b)).await) }
        (true, "/api/gamesettings/write") => {
            let (f, v) = (bs("file"), body.get("values").filter(|v| !v.is_null()).cloned().unwrap_or(json!({})));
            json_resp(200, &blocking(move || crate::gamesettings::write(&f, &v)).await)
        }
        (true, "/api/gamesettings/restore") => { let f = bs("file"); json_resp(200, &blocking(move || crate::gamesettings::restore(&f)).await) }
        (true, "/api/gamesettings/restore_backup") => {
            let (f, n) = (bs("file"), bs("name"));
            json_resp(200, &blocking(move || crate::gamesettings::restore_backup(&f, &n)).await)
        }
        (true, "/api/gamesettings/import") => {
            let bundle = body.get("bundle").filter(|v| !v.is_null()).cloned().unwrap_or(json!({}));
            let only: Option<Vec<String>> = body.get("only").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect());
            let dev = bb("include_devices");
            json_resp(200, &blocking(move || crate::gamesettings::import_bundle(&bundle, only.as_deref(), dev)).await)
        }
        (true, "/api/registry/save") => { let b = body.clone(); json_resp(200, &blocking(move || crate::registry::upsert(&b)).await) }
        (true, "/api/registry/delete") => { let i = bs("id"); json_resp(200, &blocking(move || { crate::registry::remove(&i); json!({"ok": true}) }).await) }
        (true, "/api/share/track") => {
            let folder = { let f = bs("folder"); if f.is_empty() { bs("name") } else { f } };
            let (share, label) = (bb("share"), bs("label"));
            json_resp(200, &blocking(move || crate::autoshare::set_track(&folder, share, &label)).await)
        }
        (true, "/api/auth/rotate") => {
            if !auth::is_loopback(&peer) { let (c, v) = auth::deny_remote(&cfg); json_resp(c, &v) }
            else { json_resp(200, &json!({"ok": true, "token": auth::rotate()})) }
        }
        (true, "/api/auth/remote") => {
            if !auth::is_loopback(&peer) { let (c, v) = auth::deny_remote(&cfg); json_resp(c, &v) }
            else {
                let on = bb("on");
                let _ = config::save(&json!({"remote_admin": on}));
                json_resp(200, &json!({"ok": true, "remote_admin": on, "token": if on { auth::token_create() } else { String::new() }}))
            }
        }
        (false, "/api/browser/status") => json_resp(200, &crate::jobs::status()),
        (false, "/api/progress") => json_resp(200, &blocking(crate::jobs::progress).await),
        (false, "/api/drop/status") => {
            let mut m = serde_json::Map::new();
            m.insert("ok".into(), json!(true));
            m.extend(crate::install::ingest_state());
            json_resp(200, &Value::Object(m))
        }
        (false, "/api/browser/discover") => {
            // ⚠ two ports, briefly: seven at 4 s each looked like a hang
            let raw = q_get(&query, "host").unwrap_or_default();
            let port = q_get(&query, "port").and_then(|p| p.parse::<u16>().ok());
            json_resp(200, &blocking(move || {
                let (_h, explicit, given) = crate::contentsync::parse_target(&raw);
                let to = if !given.is_empty() || explicit.is_some() { 8.0 } else { 2.0 };
                crate::contentsync::discover(&raw, port, &[8092, 8093], to)
            }).await)
        }
        (false, "/api/browser/tag") => {
            let mut ips: Vec<String> = query.split('&').filter_map(|kv| kv.split_once('=')).filter(|(k, _)| *k == "ip")
                .map(|(_, v)| q_get(&format!("x={v}"), "x").unwrap_or_default()).filter(|v| !v.is_empty()).collect();
            if ips.is_empty() {
                ips = q_get(&query, "ips").unwrap_or_default().split(',').map(String::from).collect();
            }
            json_resp(200, &blocking(move || crate::contentsync::tag_hosts(&ips, 0.45)).await)
        }
        (false, "/api/browser/needs") => json_resp(200, &blocking(|| {
            let lst = crate::backend::server_list();
            let servers = lst.get("servers").and_then(|v| v.as_array()).cloned().unwrap_or_default();
            json!({"ok": lst.get("ok"), "cached": lst.get("cached").cloned().unwrap_or(json!(false)),
                   "captured_at": lst.get("captured_at"), "total": servers.len(),
                   "missing": crate::contentsync::needs(&servers)})
        }).await),
        (false, "/api/browser/plan") => {
            let (base, id) = (q_get(&query, "base").unwrap_or_default(), q_get(&query, "id").unwrap_or_default());
            json_resp(200, &blocking(move || crate::contentsync::plan(&base, &id)).await)
        }
        (false, "/api/share") => json_resp(200, &blocking(crate::contentsync::share_info).await),
        (false, "/api/mods/scan") => {
            let p = q_get(&query, "path").unwrap_or_default();
            json_resp(200, &blocking(move || crate::install::scan_source(&p)).await)
        }
        (false, "/api/library") => json_resp(200, &blocking(crate::install::library).await),
        (false, "/api/library/clip") => {
            let (k, n) = (q_get(&query, "kind").unwrap_or_default(), q_get(&query, "name").unwrap_or_default());
            json_resp(200, &blocking(move || crate::install::clip_item(&k, &n)).await)
        }
        (false, "/api/library/export") => {
            let (k, n) = (q_get(&query, "kind").unwrap_or_default(), q_get(&query, "name").unwrap_or_default());
            let r = blocking(move || crate::install::export_item(&k, &n, None)).await;
            let ok = r.get("ok") == Some(&json!(true));
            match r.get("path").and_then(|p| p.as_str()).map(std::path::PathBuf::from).filter(|p| p.is_file()) {
                Some(p) if ok => {
                    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let ctype = if name.to_lowercase().ends_with(".zip") { "application/zip" } else { "application/x-tar" };
                    serve_file(&p, "", ctype, &name, "").await
                }
                _ if !ok => json_resp(400, &r),
                _ => json_resp(500, &json!({"error": "export file missing"})),
            }
        }
        (false, "/api/pack/preview") => {
            let sid = q_get(&query, "id").unwrap_or_default();
            json_resp(200, &blocking(move || match find_profile_s(&sid) {
                Some(p) => crate::pack::preview(&p),
                None => json!({"ok": false, "error": "no such server"}),
            }).await)
        }
        (true, "/api/browser/install") => { let b = body.clone(); json_resp(200, &blocking(move || crate::jobs::install_content(&b)).await) }
        (true, "/api/browser/scan") => {
            let only = match body.get("only_missing") { None => true, v => bb_of(v) };
            json_resp(200, &blocking(move || {
                let lst = crate::backend::server_list();
                let servers = lst.get("servers").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                let pool = if only { crate::contentsync::needs(&servers) } else { servers };
                crate::contentsync::scan(&pool, 24, 6)
            }).await)
        }
        (true, "/api/tracks/restore") => json_resp(200, &blocking(crate::jobs::tracks_restore).await),
        (true, "/api/push/send") => { let b = body.clone(); json_resp(200, &blocking(move || crate::jobs::push_send(&b)).await) }
        (true, "/api/push/plan") => { let b = body.clone(); json_resp(200, &blocking(move || crate::jobs::push_plan(&b)).await) }
        (true, "/api/push/server_build") => { let b = body.clone(); json_resp(200, &blocking(move || crate::jobs::push_server_build(&b)).await) }
        (true, "/api/server_build") => { let id = bs("id"); json_resp(200, &blocking(move || crate::jobs::apply_server_build(&id)).await) }
        (true, "/api/mods/install") => {
            let (p, ow) = (bs("path"), bb("overwrite"));
            let only: Option<Vec<String>> = body.get("only").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect());
            json_resp(200, &blocking(move || crate::install::install(&p, only, ow)).await)
        }
        (true, "/api/drop") => {
            let (id, ow, path, cancel) = (bs("id"), bb("overwrite"), bs("path"), bb("cancel"));
            json_resp(200, &blocking(move || {
                if cancel && !id.is_empty() {
                    crate::install::drop_cleanup(&id);
                    return json!({"ok": true, "cancelled": true});
                }
                if !id.is_empty() { crate::install::ingest_staging(&id, ow) } else { crate::install::ingest(&path, ow) }
            }).await)
        }
        (true, "/api/library/remove") => { let (k, n) = (bs("kind"), bs("name")); json_resp(200, &blocking(move || crate::install::remove_item(&k, &n)).await) }
        (true, "/api/library/export") => {
            let (k, n, d) = (bs("kind"), bs("name"), bs("dest"));
            json_resp(200, &blocking(move || crate::install::export_item(&k, &n, if d.is_empty() { None } else { Some(&d) })).await)
        }
        (true, "/api/mods/fix") => { let n = bs("name"); json_resp(200, &blocking(move || crate::install::apply_fix(&n)).await) }
        (true, "/api/mods/remove") => { let n = bs("name"); json_resp(200, &blocking(move || crate::install::remove(&n, None)).await) }
        (true, "/api/mods/sync") => {
            let (d, f) = (bs("direction"), bb("force"));
            let names: Option<Vec<String>> = body.get("names").and_then(|v| v.as_array()).filter(|a| !a.is_empty()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect());
            json_resp(200, &blocking(move || crate::install::sync(if d.is_empty() { "to_server" } else { &d }, f, names)).await)
        }
        (true, "/api/pack/build") => {
            // ⚠ local only: it writes a multi-gigabyte file wherever it is told
            if !auth::is_loopback(&peer) {
                let (c, v) = auth::deny_remote(&cfg);
                json_resp(c, &v)
            } else {
                let sid = match body.get("id") { Some(Value::String(s)) => s.clone(), Some(Value::Null) | None => String::new(), Some(v) => v.to_string() };
                let dest = bs("dest");
                json_resp(200, &blocking(move || match find_profile_s(&sid) {
                    Some(p) => crate::pack::build(&p, if dest.is_empty() { None } else { Some(&dest) }, true),
                    None => json!({"ok": false, "error": "no such server"}),
                }).await)
            }
        }
        (true, "/api/drive/direct") => { let t = bs("target"); json_resp(200, &blocking(move || crate::drive::direct_lookup(&t)).await) }
        (true, "/api/drive/fav/add") => { let (t, n) = (bs("target"), bs("name")); json_resp(200, &blocking(move || crate::drive::favourite_add(&t, &n)).await) }
        (true, "/api/drive/fav/remove") => { let i = bs("id"); json_resp(200, &blocking(move || crate::drive::favourite_remove(&i)).await) }
        (false, "/api/drive/status") => json_resp(200, &blocking(crate::drive::status).await),
        (true, "/api/drive") => {
            let b = body.clone();
            json_resp(200, &blocking(move || crate::drive::start(&b)).await)
        }
        (true, "/api/drive/cancel") => json_resp(200, &crate::drive::cancel()),
        (true, "/api/drive/capture") => json_resp(200, &blocking(crate::drive::capture_list).await),
        (true, "/api/drive/list") => {
            let id = { let i = bs("id"); if i.is_empty() { bs("local_id") } else { i } };
            json_resp(200, &blocking(move || crate::drive::publish_local(&id)).await)
        }
        (true, "/api/game/launch") => json_resp(200, &blocking(|| crate::launch::launch_game(&[])).await),
        (true, "/api/backend/ensure") => json_resp(200, &blocking(crate::drive::ensure_backend).await),
        (true, "/api/backend/stop") => json_resp(200, &blocking(crate::backend::stop).await),
        (true, "/api/backend/redirect") => {
            let restore = bs("action").to_lowercase() == "restore";
            json_resp(200, &blocking(move || if restore { crate::backend::restore_redirect() } else { crate::backend::apply_redirect() }).await)
        }
        (true, "/api/join") => {
            let id = body.get("id").cloned().unwrap_or(Value::Null);
            let shape = { let s = bs("shape"); if s.is_empty() { "bare".to_string() } else { s } };
            json_resp(200, &blocking(move || crate::backend::join(&id, &shape, Value::Null, Value::Null, "")).await)
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
