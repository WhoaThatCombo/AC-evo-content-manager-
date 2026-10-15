//! The joiner's side of content delivery - ported from contentsync.py:
//! find the ACECM beside a game server, compare its manifest with what is
//! here (by SHA-256), and download only what is missing.
//!
//! ⚠ The dedicated server can never be the source: its archive holds a
//! track's logic only, the ~1 GB of art lives in the HOST's game install.

use std::collections::{BTreeMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::{config, logs, registry, trackimport};

// ACECM picks a free port; these are where it lands in practice
pub const PORTS: [u16; 7] = [8092, 8093, 8730, 8080, 8091, 8094, 8095];
pub const TIMEOUT: f64 = 4.0;

pub fn tracks_dir() -> String {
    trackimport::tracks_dir()
}

fn sv(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(x) => x.to_string(),
    }
}

/// Absolute path inside root, or Err if it would escape.
/// ⚠ Every component must be a plain name: no `.` / `..` (a manifest path
/// "tracks/./x" made the WHOLE tracks folder one track's folder, so every
/// other track's files counted as stale and were deleted), no `:` (NTFS
/// alternate data streams, drive letters) and no NUL. Names come from
/// remote hosts and shares.
pub fn under(root: &str, parts: &[&str]) -> Result<PathBuf, String> {
    let root = std::path::absolute(root).map_err(|e| e.to_string())?;
    let mut dest = root.clone();
    for p in parts {
        for c in p.split(['/', '\\']) {
            if c.is_empty() {
                continue;
            }
            if c == "." || c == ".." || c.contains(':') || c.contains('\0') {
                return Err("bad path component".into());
            }
        }
        dest.push(p);
    }
    let dest = normalize(&dest);
    if !dest.starts_with(normalize(&root)) {
        return Err("path escapes content folder".into());
    }
    Ok(dest)
}

/// os.path.abspath's normalisation: drop `.`, resolve `..` lexically
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => { out.pop(); }
            std::path::Component::CurDir => {}
            c => out.push(c.as_os_str()),
        }
    }
    out
}

// ⚠ An ALLOWLIST of what content is made of: a hash proves the transfer,
// not that the file belongs there.
pub const CONTENT_EXTS: [&str; 12] = ["texture", "texturemips", "mesh", "material", "scene", "track",
    "aisplinedata", "trackcontrolpoints", "track_layout", "reference", "kspkg", "json"];
const CONTENT_EXTS_BIN: &str = "bin";

pub fn content_allowed(rel: &str) -> bool {
    let name = rel.replace('\\', "/").rsplit('/').next().unwrap_or("").to_string();
    let Some((_, ext)) = name.rsplit_once('.') else { return false };
    let ext = ext.to_lowercase();
    !ext.is_empty() && (CONTENT_EXTS.contains(&ext.as_str()) || ext == CONTENT_EXTS_BIN)
}

fn exts_sorted() -> String {
    let mut v: Vec<&str> = CONTENT_EXTS.iter().copied().chain([CONTENT_EXTS_BIN]).collect();
    v.sort();
    v.join(", ")
}

/// Where a manifest path belongs here. ⚠ Must match acecm_sync.destination.
pub fn destination(rel: &str) -> Result<PathBuf, String> {
    let rel = rel.replace('\\', "/");
    let rel = rel.trim_start_matches('/');
    let parts: Vec<&str> = rel.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    if parts.is_empty() || parts.contains(&"..") {
        return Err("bad content path".into());
    }
    let last = parts[parts.len() - 1];
    if !content_allowed(last) {
        return Err(format!("refusing '{last}': content is only {}", exts_sorted()));
    }
    if parts[0] == "mods" {
        if parts.len() != 2 {
            return Err("bad mod path".into());
        }
        return under(&crate::mods::client_mods_dir(), &[parts[1]]);
    }
    if parts[0] == "tracks" {
        if parts.len() < 2 {
            return Err("bad track path".into());
        }
        return under(&tracks_dir(), &parts[1..]);
    }
    under(&tracks_dir(), &[last])
}

// ------------------------------------------------------------------ http --
fn client(timeout: f64) -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder().timeout(Duration::from_secs_f64(timeout))
        .user_agent("ACECM").build().unwrap()
}

/// GET JSON, keeping the host's own words when it fails.
pub fn get_json(url: &str, timeout: f64) -> Result<Value, String> {
    let r = client(timeout).get(url).send().map_err(|e| format!("<urlopen error {e}>"))?;
    let code = r.status().as_u16();
    let body = r.text().map_err(|e| e.to_string())?;
    if code >= 400 {
        let detail = match serde_json::from_str::<Value>(&body) {
            Ok(v) => { let e = sv(&v, "error"); if e.is_empty() { sv(&v, "detail") } else { e } }
            Err(_) => body.trim().chars().take(300).collect(),
        };
        if !detail.is_empty() {
            return Err(format!("the host answered {code}: {detail}"));
        }
        return Err(format!("HTTP Error {code}"));
    }
    if body.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&body).map_err(|e| e.to_string())
}

/// A pasted share link, host:port or bare IP -> (host, port, base).
pub fn parse_target(raw: &str) -> (String, Option<u16>, String) {
    let raw = raw.trim();
    if raw.is_empty() {
        return (String::new(), None, String::new());
    }
    if raw.contains("://") {
        let Ok(u) = reqwest::Url::parse(raw) else { return (String::new(), None, String::new()) };
        let host = u.host_str().unwrap_or("").trim_matches(['[', ']']).to_string();
        if host.is_empty() {
            return (String::new(), None, String::new());
        }
        let port = u.port();
        let scheme = if u.scheme().is_empty() { "http" } else { u.scheme() };
        // netloc as written (urlparse keeps it verbatim)
        let after = &raw[raw.find("://").unwrap() + 3..];
        let netloc = after.split(['/', '?', '#']).next().unwrap_or("").to_string();
        let netloc = if netloc.is_empty() { match port { Some(p) => format!("{host}:{p}"), None => host.clone() } } else { netloc };
        return (host, port, format!("{scheme}://{netloc}"));
    }
    if raw.matches(':').count() == 1 {
        let (left, right) = raw.rsplit_once(':').unwrap();
        if !right.is_empty() && right.chars().all(|c| c.is_ascii_digit()) && !left.is_empty() {
            if let Ok(p) = right.parse::<u16>() {
                return (left.to_string(), Some(p), String::new());
            }
        }
    }
    (raw.to_string(), None, String::new())
}

fn servers_of(got: &Value) -> Option<Vec<Value>> {
    match got {
        Value::Object(_) => got.get("servers").and_then(|s| s.as_array()).cloned(),
        Value::Array(a) => Some(a.clone()),
        _ => None,
    }
}

/// Find the ACECM sharing content beside a game server. A host that is not
/// running ACECM is the normal case, not an error.
pub fn discover(raw_host: &str, port: Option<u16>, ports: &[u16], timeout: f64) -> Value {
    let (mut host, mut parsed_port, mut given_base) = parse_target(raw_host);
    if port.is_some() {
        parsed_port = port;
    }
    if host.is_empty() {
        return json!({"ok": false, "error": "no host given"});
    }
    // ⚠ our OWN server goes over loopback (no NAT hairpinning)
    if our_ips().contains(&host) {
        host = "127.0.0.1".into();
        given_base = String::new();
    }
    if !given_base.is_empty() {
        return match get_json(&format!("{given_base}/api/registry/list"), timeout) {
            Ok(got) => match servers_of(&got) {
                Some(s) => json!({"ok": true, "base": given_base, "port": parsed_port, "servers": s}),
                None => json!({"ok": false, "tried": [parsed_port], "error": format!("no ACECM at {given_base}")}),
            },
            Err(e) => json!({"ok": false, "tried": [parsed_port],
                "error": format!("no ACECM at {given_base} ({e}). The host must be running ACECM and TCP {} must be reachable.", parsed_port.unwrap_or(8092))}),
        };
    }
    let probe: Vec<u16> = match parsed_port { Some(p) => vec![p], None => ports.to_vec() };
    let mut tried = Vec::new();
    for p in probe {
        if p == 0 {
            continue;
        }
        let base = format!("http://{host}:{p}");
        tried.push(p);
        let Ok(got) = get_json(&format!("{base}/api/registry/list"), timeout) else { continue };
        if let Some(s) = servers_of(&got) {
            return json!({"ok": true, "base": base, "port": p, "servers": s});
        }
    }
    let t: Vec<String> = tried.iter().map(|p| p.to_string()).collect();
    json!({"ok": false, "tried": tried,
        "error": format!("no ACECM answering on {host} (tried TCP {}). Content is NOT the game port: the host must run ACECM and allow inbound TCP 8092 (Windows Firewall + router port-forward if you are not on the same LAN). Game join still needs TCP+UDP 9700.",
            if t.is_empty() { "none".to_string() } else { t.join(", ") })})
}

/// netutil.our_ips(): every address that means "this machine".
pub fn our_ips() -> HashSet<String> {
    let mut out: HashSet<String> = ["127.0.0.1", "localhost", "::1"].iter().map(|s| s.to_string()).collect();
    let lan = crate::misc::lan_ipv4();
    if !lan.is_empty() {
        out.insert(lan);
    }
    out
}

/// Which of these IPs have ACECM sharing content on the UI port.
pub fn tag_hosts(ips: &[String], timeout: f64) -> Value {
    let port = config::load().get("ui_port").and_then(|v| v.as_u64()).unwrap_or(8092) as u16;
    let mut seen = HashSet::new();
    let mut targets = Vec::new();
    for raw in ips {
        let (host, p, _) = parse_target(raw);
        if host.is_empty() || !seen.insert(host.clone()) {
            continue;
        }
        targets.push((host, p.unwrap_or(port)));
        if targets.len() >= 24 {
            break;
        }
    }
    let out = Arc::new(Mutex::new(Map::new()));
    let work = Arc::new(Mutex::new(targets));
    let n = work.lock().unwrap().len().min(8);
    let hs: Vec<_> = (0..n).map(|_| {
        let (out, work) = (out.clone(), work.clone());
        std::thread::spawn(move || loop {
            let Some((host, p)) = work.lock().unwrap().pop() else { break };
            let miss = json!({"acecm": false, "hosted": false, "port": p});
            let addr = format!("{host}:{p}");
            let ok = std::net::ToSocketAddrs::to_socket_addrs(&addr.as_str()).ok().and_then(|mut a| a.next())
                .map(|a| std::net::TcpStream::connect_timeout(&a, Duration::from_secs_f64(timeout)).is_ok()).unwrap_or(false);
            let info = if !ok { miss } else {
                match get_json(&format!("http://{host}:{p}/api/registry/list"), timeout).ok().and_then(|g| g.get("servers").and_then(|s| s.as_array()).cloned()) {
                    None => miss,
                    Some(servers) => {
                        let hosted = servers.iter().any(|e| e.get("required_tracks").map(|t| t.as_array().map(|a| !a.is_empty()).unwrap_or(false)).unwrap_or(false)
                            || e.get("required_mods").map(|t| t.as_array().map(|a| !a.is_empty()).unwrap_or(false)).unwrap_or(false));
                        let items: usize = servers.iter().map(|e| e.get("required_tracks").and_then(|t| t.as_array()).map(|a| a.len()).unwrap_or(0)
                            + e.get("required_mods").and_then(|t| t.as_array()).map(|a| a.len()).unwrap_or(0)).sum();
                        json!({"acecm": true, "hosted": hosted, "port": p, "items": items})
                    }
                }
            };
            out.lock().unwrap().insert(host, info);
        })
    }).collect();
    for h in hs {
        let _ = h.join();
    }
    let out = std::mem::take(&mut *out.lock().unwrap());
    json!({"ok": true, "port": port, "hosts": out})
}

/// What a host should send a joining player.
pub fn share_info() -> Value {
    let cfg = config::load();
    let port = cfg.get("ui_port").and_then(|v| v.as_i64()).unwrap_or(8092);
    let lan = crate::misc::lan_ipv4();
    let mut pubs = Vec::new();
    for e in registry::load() {
        if !e.get("public").map(|p| p.as_bool().unwrap_or(true)).unwrap_or(true) {
            continue;
        }
        pubs.push(json!({"id": e.get("id"), "name": e.get("name"),
            "tracks": e.get("required_tracks").filter(|v| !v.is_null()).cloned().unwrap_or(json!([])),
            "mods": e.get("required_mods").filter(|v| !v.is_null()).cloned().unwrap_or(json!([]))}));
    }
    let wan = crate::misc::public_ipv4();
    let listen = { let l = sv(&Value::Object(cfg.clone()), "listen"); if l.is_empty() { "0.0.0.0".to_string() } else { l } };
    json!({
        "ok": true, "port": port, "listen": listen, "lan_ip": lan,
        "lan_url": if lan.is_empty() { String::new() } else { format!("http://{lan}:{port}") },
        "public_ip": wan,
        "public_url": if wan.is_empty() { String::new() } else { format!("http://{wan}:{port}") },
        "localhost_url": format!("http://127.0.0.1:{port}"),
        "published": pubs,
        "hint": format!("Players paste the URL into Server browser → Fetch from ACECM. They need TCP {port} (this app) and the game's TCP+UDP port (usually 9700). Same LAN: firewall on {port}. Different network: forward both."),
        "public_note": if !wan.is_empty() { format!("Anyone outside your network also needs TCP {port} forwarded to this PC on your router - the address alone is not enough.") }
                       else { "Could not work out your public address (no internet, or the lookup was blocked).".to_string() },
    })
}

/// Ask a set of hosts whether they can supply content, in parallel.
/// ⚠ Only servers you are MISSING content for, capped, two ports: not a
/// port scan of other people's machines.
pub fn scan(servers: &[Value], limit: usize, workers: usize) -> Value {
    let mut seen = HashSet::new();
    let mut targets = Vec::new();
    for s in servers {
        let ip = sv(s, "server_ip").trim().to_string();
        if ip.is_empty() || !seen.insert(ip) {
            continue;
        }
        targets.push(s.clone());
        if targets.len() >= limit {
            break;
        }
    }
    let probed = targets.len();
    let results: Arc<Mutex<Vec<(usize, Value)>>> = Arc::new(Mutex::new(Vec::new()));
    let work = Arc::new(Mutex::new(targets.into_iter().enumerate().collect::<Vec<_>>()));
    let hs: Vec<_> = (0..workers.min(probed.max(1))).map(|_| {
        let (results, work) = (results.clone(), work.clone());
        std::thread::spawn(move || loop {
            let Some((i, s)) = work.lock().unwrap().pop() else { break };
            let d = discover(&sv(&s, "server_ip"), None, &[8092, 8093], 1.5);
            if d.get("ok") != Some(&json!(true)) {
                continue;
            }
            let entries: Vec<Value> = d.get("servers").and_then(|v| v.as_array()).cloned().unwrap_or_default().iter().map(|e| json!({
                "id": e.get("id"), "name": e.get("name"),
                "tracks": e.get("required_tracks").filter(|v| !v.is_null()).cloned().unwrap_or(json!([])),
                "mods": e.get("required_mods").filter(|v| !v.is_null()).cloned().unwrap_or(json!([])),
                "bytes": e.get("content_bytes").filter(|v| !v.is_null()).cloned().unwrap_or(json!(0))})).collect();
            if entries.is_empty() {
                continue;
            }
            results.lock().unwrap().push((i, json!({"server": s.get("server_name"), "ip": s.get("server_ip"),
                "port": s.get("server_tcp_port"), "base": d.get("base"), "entries": entries})));
        })
    }).collect();
    for h in hs {
        let _ = h.join();
    }
    let mut r = std::mem::take(&mut *results.lock().unwrap());
    r.sort_by_key(|(i, _)| *i);
    json!({"ok": true, "probed": probed, "hosts": r.into_iter().map(|(_, v)| v).collect::<Vec<_>>()})
}

/// The servers running a track this machine cannot load.
pub fn needs(servers: &[Value]) -> Vec<Value> {
    let known = trackimport::track_map(false);
    servers.iter().filter(|s| { let t = sv(s, "track").trim().to_string(); !t.is_empty() && !known.contains_key(&t) }).cloned().collect()
}

fn manifest_track_folders(man: &Value) -> HashSet<String> {
    let mut out = HashSet::new();
    for f in man.get("files").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
        let p = sv(&f, "path").replace('\\', "/");
        let parts: Vec<&str> = p.split('/').collect();
        if parts.len() >= 3 && parts[0] == "tracks" && !parts[1].is_empty() {
            out.insert(parts[1].to_string());
        }
    }
    out
}

/// Track folders whose FILES are here but the game cannot load (the table
/// write failed, e.g. with the game open).
fn unregistered_tracks(man: &Value) -> Vec<String> {
    let folders = manifest_track_folders(man);
    if folders.is_empty() {
        return Vec::new();
    }
    let here: HashSet<String> = trackimport::installed_tracks().into_iter().collect();
    let known: HashSet<String> = trackimport::track_map(false).values().filter_map(|v| v.as_str().map(String::from)).collect();
    let mut v: Vec<String> = folders.into_iter().filter(|f| here.contains(f) && !known.contains(f)).collect();
    v.sort();
    v
}

/// Local files the host no longer has, inside the manifest's TRACK folders
/// only (never an empty manifest read as "delete everything").
fn stale_files(man: &Value) -> Vec<String> {
    let files = man.get("files").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    if files.is_empty() {
        return Vec::new();
    }
    let (mut keep, mut folders) = (HashSet::new(), Vec::<String>::new());
    for f in &files {
        let rel = sv(f, "path").replace('\\', "/");
        let parts: Vec<&str> = rel.trim_start_matches('/').split('/').filter(|p| !p.is_empty()).collect();
        if parts.len() >= 3 && parts[0] == "tracks" {
            if !folders.contains(&parts[1].to_string()) {
                folders.push(parts[1].to_string());
            }
            keep.insert(parts.join("/").to_lowercase());
        }
    }
    let td = tracks_dir();
    // ⚠ strip against the SAME normalised root under() returns: a relative
    // or ..-containing setting made every file fail the match and be deleted
    let tdn = std::path::absolute(&td).map(|p| normalize(&p)).unwrap_or_else(|_| PathBuf::from(&td));
    let mut out = Vec::new();
    for folder in folders {
        if crate::install::safe_folder(&folder) != folder {
            continue;
        }
        let Ok(root) = under(&td, &[&folder]) else { continue };
        if !root.is_dir() {
            continue;
        }
        let mut stack = vec![root];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    stack.push(p);
                } else {
                    let Ok(rel) = p.strip_prefix(&tdn) else { continue };
                    let rel = rel.to_string_lossy().replace('\\', "/");
                    if !keep.contains(&format!("tracks/{rel}").to_lowercase()) {
                        out.push(p.to_string_lossy().into_owned());
                    }
                }
            }
        }
    }
    out
}

/// Delete files stale_files() named. Never anything outside the tracks folder.
pub fn remove_stale(paths: &[String]) -> Value {
    let root = normalize(&std::path::absolute(tracks_dir()).unwrap_or_default());
    let (mut gone, mut failed) = (Vec::new(), Vec::new());
    for p in paths {
        let ap = normalize(&std::path::absolute(p).unwrap_or_default());
        if !ap.starts_with(&root) {
            continue;
        }
        match std::fs::remove_file(&ap) {
            Ok(()) => gone.push(p.clone()),
            Err(e) => {
                logs::info(format!("could not remove stale {p}: {e}"));
                failed.push(p.clone());
            }
        }
    }
    if !gone.is_empty() {
        logs::info(format!("removed {} file(s) the host no longer has", gone.len()));
    }
    json!({"removed": gone, "failed": failed})
}

/// Only fetch share files from the ACECM we already discovered.
fn registry_file_url(base: &str, server_id: &str, rel: &str) -> Result<String, String> {
    let u = reqwest::Url::parse(base).map_err(|_| "bad content host".to_string())?;
    if !matches!(u.scheme(), "http" | "https") || u.host_str().is_none() || !u.username().is_empty() {
        return Err("bad content host".into());
    }
    let host = u.host_str().unwrap_or("").to_lowercase();
    if host.is_empty() || host == "0.0.0.0" || host == "[::]" {
        return Err("bad content host".into());
    }
    let netloc = &base[base.find("://").map(|i| i + 3).unwrap_or(0)..];
    let netloc = netloc.split(['/', '?', '#']).next().unwrap_or("");
    Ok(format!("{}://{netloc}/api/registry/file?id={}&path={}", u.scheme(), qenc(server_id), qenc(rel)))
}

/// urllib.parse.urlencode's quote_plus
fn qenc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// urllib.parse.quote (safe "/")
fn quote(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' | b'/' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// What joining that server would cost THIS machine - compared by SHA-256,
/// so content you already have is never downloaded again.
pub fn plan(base: &str, server_id: &str) -> Value {
    // ⚠ generous: the host hashes a cold gigabyte the first time
    let man = match get_json(&format!("{base}/api/registry/manifest?id={server_id}"), 600.0) {
        Ok(m) => m,
        Err(e) => return json!({"ok": false, "error": format!("manifest unreadable: {e}")}),
    };
    if man.get("ok") != Some(&json!(true)) {
        let e = sv(&man, "error");
        return json!({"ok": false, "error": if e.is_empty() { "no manifest".into() } else { e }});
    }
    let (mut need, mut have) = (Vec::new(), 0usize);
    let mut bytes: u64 = 0;
    for f in man.get("files").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
        let path = sv(&f, "path");
        let (Ok(dest), Ok(url)) = (destination(&path), registry_file_url(base, server_id, &path)) else { continue };
        let size = f.get("size").and_then(|v| v.as_u64());
        let sha = sv(&f, "sha256");
        let same = dest.is_file() && std::fs::metadata(&dest).map(|m| Some(m.len()) == size).unwrap_or(false)
            && (sha.is_empty() || registry::file_digest(&dest).as_deref() == Some(sha.as_str()));
        if same {
            have += 1;
        } else {
            bytes += size.unwrap_or(0);
            let mut e = f.as_object().cloned().unwrap_or_default();
            e.insert("dest".into(), json!(dest.to_string_lossy()));
            e.insert("url".into(), json!(url));
            need.push(Value::Object(e));
        }
    }
    registry::flush_cache();
    json!({"ok": true, "server": man.get("server").cloned().unwrap_or(json!({})),
           "need": need, "have": have,
           "needs_register": unregistered_tracks(&man),
           "bytes": bytes,
           "stale": stale_files(&man),
           "missing_on_host": man.get("missing_locally").filter(|v| !v.is_null()).cloned().unwrap_or(json!([]))})
}

// ------------------------------------------------------------- downloads --
// ⚠ tuned for "saturate a gigabit link": the ceiling is per connection
pub const RANGED_MIN: u64 = 8 * 1024 * 1024;
pub const RANGED_PARTS: u64 = 16;
const READ_CHUNK: usize = 4 * 1024 * 1024;

pub type Progress<'a> = &'a (dyn Fn(u64, u64) + Sync);

/// Pull `size` bytes of `url` into `tmp` over several parallel ranges.
fn ranged_to_file(url: &str, size: u64, tmp: &Path, progress: Option<Progress>, parts: u64) -> Result<(), String> {
    std::fs::File::create(tmp).and_then(|f| f.set_len(size)).map_err(|e| e.to_string())?;
    let step = size.div_ceil(parts);
    let mut bounds = Vec::new();
    for i in 0..parts {
        let a = i * step;
        let b = (a + step).min(size).saturating_sub(1);
        if a <= b && a < size {
            bounds.push((a, b));
        }
    }
    let got: Mutex<BTreeMap<u64, u64>> = Mutex::new(BTreeMap::new());
    let res: Vec<Result<(), String>> = std::thread::scope(|sc| {
        let hs: Vec<_> = bounds.iter().map(|&(a, b)| {
            let got = &got;
            sc.spawn(move || -> Result<(), String> {
                let r = client(600.0).get(url).header("Range", format!("bytes={a}-{b}")).header("Accept-Encoding", "identity")
                    .send().map_err(|e| e.to_string())?;
                if r.status().as_u16() != 206 {
                    return Err("host did not honour Range".into());
                }
                let mut r = r;
                let mut f = std::fs::OpenOptions::new().write(true).open(tmp).map_err(|e| e.to_string())?;
                use std::io::Seek;
                f.seek(std::io::SeekFrom::Start(a)).map_err(|e| e.to_string())?;
                let mut pos = a;
                let mut buf = vec![0u8; READ_CHUNK.min((b - a + 1) as usize)];
                while pos <= b {
                    let want = ((b - pos + 1) as usize).min(buf.len());
                    let n = r.read(&mut buf[..want]).map_err(|e| e.to_string())?;
                    if n == 0 {
                        // ⚠ a range that ends early is a hole of zeros in the file
                        return Err("the host closed a range early".into());
                    }
                    f.write_all(&buf[..n]).map_err(|e| e.to_string())?;
                    pos += n as u64;
                    let mut g = got.lock().unwrap();
                    g.insert(a, pos - a);
                    if let Some(p) = progress {
                        p(g.values().sum(), size);
                    }
                }
                Ok(())
            })
        }).collect();
        hs.into_iter().map(|h| h.join().unwrap_or_else(|_| Err("range worker panicked".into()))).collect()
    });
    if let Some(Err(e)) = res.into_iter().find(|r| r.is_err()) {
        let _ = std::fs::remove_file(tmp);
        return Err(e);
    }
    Ok(())
}

fn digest_check(tmp: &Path, entry: &Value) -> Result<(), String> {
    let want = sv(entry, "sha256");
    let got = registry::file_digest(tmp).unwrap_or_default();
    if !want.is_empty() && got != want {
        let _ = std::fs::remove_file(tmp);
        return Err(format!("checksum mismatch for {} (got {}, expected {})", sv(entry, "path"),
            got.chars().take(12).collect::<String>(), want.chars().take(12).collect::<String>()));
    }
    Ok(())
}

fn entry_dest(entry: &Value) -> Result<PathBuf, String> {
    let d = sv(entry, "dest");
    let d = if d.is_empty() { destination(&sv(entry, "path"))? } else { PathBuf::from(d) };
    Ok(std::path::absolute(&d).unwrap_or(d))
}

/// Download one manifest entry to .part, verify it, then move it into place.
pub fn fetch(entry: &Value, progress: Option<Progress>) -> Result<PathBuf, String> {
    let dest = entry_dest(entry)?;
    if let Some(p) = dest.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    let tmp = PathBuf::from(format!("{}.part", dest.display()));
    let size = entry.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
    let url = sv(entry, "url");
    if !url.contains("/api/registry/file") {
        return Err("refusing a content URL that is not this host's share".into());
    }
    let mut r = client(600.0).get(&url).send().map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("HTTP Error {}", r.status().as_u16()));
    }
    let mut f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 1 << 20];
    let mut done = 0u64;
    let fail = |e: String| { let _ = std::fs::remove_file(&tmp); e };
    loop {
        let n = r.read(&mut buf).map_err(|e| fail(e.to_string()))?;
        if n == 0 {
            break;
        }
        done += n as u64;
        // ⚠ never more than the manifest declared (a hostile host could
        // otherwise stream until the disk is full)
        if size > 0 && done > size {
            drop(f);
            return Err(fail(format!("{} is larger than the host declared", sv(entry, "path"))));
        }
        f.write_all(&buf[..n]).map_err(|e| fail(e.to_string()))?;
        if let Some(p) = progress {
            p(done, size);
        }
    }
    drop(f);
    if size > 0 && done != size {
        return Err(fail(format!("{} arrived incomplete ({done} of {size} bytes)", sv(entry, "path"))));
    }
    digest_check(&tmp, entry)?;
    std::fs::rename(&tmp, &dest).map_err(|e| e.to_string())?;
    Ok(dest)
}

/// fetch(), as several parallel byte-range GETs; anything but a clean set
/// of 206s falls back to the plain fetch.
pub fn fetch_ranged(entry: &Value, progress: Option<Progress>) -> Result<PathBuf, String> {
    let dest = entry_dest(entry)?;
    if let Some(p) = dest.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    let size = entry.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
    let url = sv(entry, "url");
    if !url.contains("/api/registry/file") {
        return Err("refusing a content URL that is not this host's share".into());
    }
    if size < RANGED_MIN {
        return fetch(entry, progress);
    }
    let tmp = PathBuf::from(format!("{}.part", dest.display()));
    if ranged_to_file(&url, size, &tmp, progress, RANGED_PARTS).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return fetch(entry, progress);
    }
    digest_check(&tmp, entry)?;
    std::fs::rename(&tmp, &dest).map_err(|e| e.to_string())?;
    Ok(dest)
}

/// Pull one tar for a whole track and unpack it. Err("404") when the host
/// has no pack endpoint, so the caller falls back to single files.
pub fn fetch_track_pack(base: &str, sid: &str, folder: &str, files: &[Value], progress: Option<Progress>) -> Result<PathBuf, String> {
    let folder = folder.replace('\\', "/").trim_matches('/').to_string();
    if folder.is_empty() || folder.contains('/') || folder == "." || folder == ".." {
        return Err("bad track folder".into());
    }
    let u = reqwest::Url::parse(base).map_err(|_| "bad content host".to_string())?;
    if !matches!(u.scheme(), "http" | "https") || u.host_str().is_none() || !u.username().is_empty() {
        return Err("bad content host".into());
    }
    let netloc = base[base.find("://").map(|i| i + 3).unwrap_or(0)..].split(['/', '?', '#']).next().unwrap_or("").to_string();
    let url = format!("{}://{netloc}/api/registry/pack?id={}&track={}", u.scheme(), quote(sid), quote(&folder));
    let dest_dir = under(&tracks_dir(), &[&folder])?;
    std::fs::create_dir_all(&dest_dir).map_err(|e| e.to_string())?;
    let tmp = PathBuf::from(format!("{}.tar.part", dest_dir.to_string_lossy().trim_end_matches(['\\', '/'])));
    // ⚠ one byte first: settles 404, the real total and the checksum without
    // committing to a gigabyte down one connection
    let pr = client(600.0).get(&url).header("Range", "bytes=0-0").send().map_err(|e| e.to_string())?;
    if pr.status().as_u16() == 404 {
        return Err("404".into());
    }
    if !pr.status().is_success() {
        return Err(format!("HTTP Error {}", pr.status().as_u16()));
    }
    let mut want = pr.headers().get("X-ACECM-SHA256").and_then(|v| v.to_str().ok()).unwrap_or("").trim().to_string();
    let mut total = 0u64;
    if pr.status().as_u16() == 206 {
        if let Some(cr) = pr.headers().get("Content-Range").and_then(|v| v.to_str().ok()) {
            total = cr.rsplit('/').next().and_then(|t| t.parse().ok()).unwrap_or(0);
        }
    }
    drop(pr);
    let mut ranged_ok = false;
    if total >= RANGED_MIN {
        match ranged_to_file(&url, total, &tmp, progress, RANGED_PARTS) {
            Ok(()) => ranged_ok = true,
            Err(e) => logs::info(format!("ranged pack {folder} fell back to one stream: {e}")),
        }
    }
    if !ranged_ok {
        let mut r = client(600.0).get(&url).send().map_err(|e| e.to_string())?;
        if r.status().as_u16() == 404 {
            return Err("404".into());
        }
        if !r.status().is_success() {
            return Err(format!("HTTP Error {}", r.status().as_u16()));
        }
        want = r.headers().get("X-ACECM-SHA256").and_then(|v| v.to_str().ok()).unwrap_or("").trim().to_string();
        let len = r.content_length().unwrap_or(0);
        let res = (|| -> Result<(), String> {
            let mut f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
            let mut buf = vec![0u8; READ_CHUNK];
            let mut done = 0u64;
            loop {
                let n = r.read(&mut buf).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                f.write_all(&buf[..n]).map_err(|e| e.to_string())?;
                done += n as u64;
                if let Some(p) = progress {
                    p(done, len);
                }
            }
            Ok(())
        })();
        if let Err(e) = res {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    }
    let got = registry::file_digest(&tmp).unwrap_or_default();
    if !want.is_empty() && got != want {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("pack checksum mismatch for {folder}"));
    }
    if let Some(bad) = crate::install::archive_budget(&tmp, &dest_dir) {
        let _ = std::fs::remove_file(&tmp);
        return Err(sv(&bad, "error"));
    }
    // ⚠ this path never goes through destination(): the same two guards -
    // containment and the content-type allowlist - are applied per member
    let ex = crate::install::extract_tar_safe(&tmp, &dest_dir, &folder);
    let _ = std::fs::remove_file(&tmp);
    ex?;
    if !files.is_empty() {
        let missing = files.iter().filter(|f| {
            let d = sv(f, "dest");
            let p = if d.is_empty() { destination(&sv(f, "path")).unwrap_or_default() } else { PathBuf::from(d) };
            !p.is_file()
        }).count();
        if missing > 0 {
            return Err(format!("pack for {folder} missed {missing} file(s)"));
        }
    }
    Ok(dest_dir)
}

/// Put a downloaded track in the client's system tables.
pub fn register_downloaded_track(folder: &str) -> Value {
    if folder.is_empty() || crate::install::safe_folder(folder) != folder {
        return json!({"ok": false, "error": format!("not a track folder name: {folder:?}")});
    }
    let dest = Path::new(&tracks_dir()).join(folder);
    let meta = std::fs::read(dest.join("acecm_track.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).unwrap_or(json!({}));
    let r = crate::trackdeploy::register_client_track(folder, &meta);
    if r.get("ok") != Some(&json!(true)) {
        logs::warn(format!("could not register {folder} in client tables: {}", sv(&r, "error")));
    }
    r
}

fn entry_sid(e: &Value) -> String {
    let url = sv(e, "url");
    reqwest::Url::parse(&url).ok().and_then(|u| u.query_pairs().find(|(k, _)| k == "id").map(|(_, v)| v.into_owned())).unwrap_or_default()
}

fn entry_base(e: &Value) -> String {
    let url = sv(e, "url");
    let Some(i) = url.find("://") else { return String::new() };
    let netloc = url[i + 3..].split(['/', '?', '#']).next().unwrap_or("");
    if netloc.is_empty() { String::new() } else { format!("{}://{netloc}", &url[..i]) }
}

/// Install a plan: one tar per track, then leftover files in parallel.
/// `status` is the shared content job (state/detail/done/...).
pub fn install_files(need: &[Value], status: &Mutex<Map<String, Value>>, register: &[String]) -> Vec<String> {
    let set = |k: &str, v: Value| { status.lock().unwrap().insert(k.into(), v); };
    let mut tracks: Vec<(String, Vec<Value>)> = Vec::new();
    let mut rest = Vec::new();
    for e in need {
        let rel = sv(e, "path").replace('\\', "/");
        let parts: Vec<&str> = rel.split('/').collect();
        if parts.len() >= 3 && parts[0] == "tracks" && !parts[1].is_empty() {
            if crate::install::safe_folder(parts[1]) != parts[1] {
                logs::warn(format!("skipping {rel}: not a track folder name"));
                continue;
            }
            match tracks.iter_mut().find(|(f, _)| f == parts[1]) {
                Some((_, v)) => v.push(e.clone()),
                None => tracks.push((parts[1].to_string(), vec![e.clone()])),
            }
        } else {
            rest.push(e.clone());
        }
    }
    let mut moved: u64 = 0;
    let mut installed: Vec<String> = Vec::new();
    for (folder, files) in &tracks {
        let (sid, base) = (entry_sid(&files[0]), entry_base(&files[0]));
        let mut packed = false;
        if !sid.is_empty() && !base.is_empty() && files.len() >= 8 {
            set("detail", json!(format!("track pack {folder} ({} files)", files.len())));
            let base_n = moved;
            let tick = |done: u64, _size: u64| { status.lock().unwrap().insert("done".into(), json!(base_n + done)); };
            match fetch_track_pack(&base, &sid, folder, files, Some(&tick)) {
                Ok(_) => {
                    moved += files.iter().map(|f| f.get("size").and_then(|v| v.as_u64()).unwrap_or(0)).sum::<u64>();
                    set("done", json!(moved));
                    installed.extend(files.iter().map(|f| sv(f, "path")));
                    packed = true;
                }
                Err(e) if e == "404" => {}
                Err(e) => logs::warn(format!("track pack {folder} failed, per-file fallback: {e}")),
            }
        }
        if !packed {
            rest.extend(files.iter().cloned());
        }
    }
    if !rest.is_empty() {
        set("detail", json!(format!("{} leftover file(s)", rest.len())));
        let base = moved;
        let inflight: Mutex<BTreeMap<usize, u64>> = Mutex::new(BTreeMap::new());
        let completed: Mutex<(u64, usize, Vec<String>)> = Mutex::new((moved, 0, Vec::new()));
        let queue: Mutex<Vec<(usize, Value)>> = Mutex::new(rest.iter().cloned().enumerate().rev().collect());
        let workers = if rest.len() > 1 { 8 } else { 1 };
        let n = rest.len();
        let errors: Mutex<Vec<String>> = Mutex::new(Vec::new());
        std::thread::scope(|sc| {
            for _ in 0..workers {
                sc.spawn(|| loop {
                    let Some((i, entry)) = queue.lock().unwrap().pop() else { break };
                    let tick = |done: u64, _s: u64| {
                        let mut g = inflight.lock().unwrap();
                        g.insert(i, done);
                        let c = completed.lock().unwrap().0;
                        status.lock().unwrap().insert("done".into(), json!(c.max(base) + g.values().sum::<u64>()));
                    };
                    match fetch_ranged(&entry, Some(&tick)) {
                        Ok(_) => {
                            inflight.lock().unwrap().remove(&i);
                            let mut c = completed.lock().unwrap();
                            c.0 += entry.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
                            c.1 += 1;
                            c.2.push(sv(&entry, "path"));
                            let mut st = status.lock().unwrap();
                            st.insert("done".into(), json!(c.0));
                            st.insert("detail".into(), json!(format!("{}/{} {}", c.1, n, sv(&entry, "path"))));
                        }
                        Err(e) => {
                            inflight.lock().unwrap().remove(&i);
                            errors.lock().unwrap().push(e);
                        }
                    }
                });
            }
        });
        let c = completed.into_inner().unwrap();
        moved = c.0;
        let _ = moved;
        installed.extend(c.2);
        // ⚠ as_completed().result() re-raised the first failure: the job
        // fails rather than reporting a partial install as done
        if let Some(e) = errors.into_inner().unwrap().into_iter().next() {
            set("files", json!(installed));
            status.lock().unwrap().insert("_error".into(), json!(e));
            return installed;
        }
    }
    let mut warns = Vec::new();
    let mut folders: Vec<String> = tracks.iter().map(|(f, _)| f.clone()).chain(register.iter().cloned()).collect();
    folders.sort();
    folders.dedup();
    for folder in folders {
        set("detail", json!(format!("registering {folder} in client tables")));
        let r = register_downloaded_track(&folder);
        if r.get("ok") != Some(&json!(true)) {
            let e = sv(&r, "error");
            warns.push(if e.is_empty() { folder.clone() } else { e });
        }
    }
    if !warns.is_empty() {
        set("warning", json!(format!("downloaded, but could not add to the game's track list: {} - close Assetto Corsa EVO and press Get content again", warns.join("; "))));
    }
    set("files", json!(installed));
    installed
}

/// What this machine can load (the /api/browser/local answer).
pub fn installed_cars() -> Vec<String> {
    let mut v: Vec<String> = crate::mods::installed("client").get("mods").and_then(|m| m.as_array()).cloned().unwrap_or_default()
        .iter().filter_map(|m| m.get("name").and_then(|n| n.as_str()).filter(|n| !n.is_empty()).map(String::from)).collect();
    v.sort();
    v
}

#[cfg(test)]
mod under_tests {
    use super::under;

    #[test]
    fn only_plain_components() {
        let root = std::env::temp_dir().join("acecm_under_root");
        let r = root.to_string_lossy().to_string();
        assert!(under(&r, &["tracks", "monza", "a.texture"]).is_ok());
        assert!(under(&r, &["monza/layouts/x.bin"]).is_ok());
        for bad in [".", "..", "x/../..", "a:b.texture", "C:", "x/./y", "nul\0"] {
            assert!(under(&r, &[bad]).is_err(), "{bad:?} must be refused");
        }
        let abs = if cfg!(windows) { r"C:\Windows\x" } else { "/etc/x" };
        assert!(under(&r, &[abs]).is_err());
    }
}
