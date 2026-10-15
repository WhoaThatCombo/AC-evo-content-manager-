//! Send a server the content it needs, from this install - ported from
//! push.py. The receiver is a (headless) ACECM taking uploads into a drop
//! staging folder; this decides WHAT to send and streams it there.
//!
//! ⚠ Uploads STREAM (a track is a gigabyte), with Content-Length set: the
//! receiver reads exactly that many bytes.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};

use crate::{autoshare, logs, mods, registry};

const CHUNK: usize = 4 * 1024 * 1024;

fn client(timeout: u64) -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder().timeout(Duration::from_secs(timeout)).user_agent("ACECM").build().unwrap()
}

fn with_token(rb: reqwest::blocking::RequestBuilder, token: &str) -> reqwest::blocking::RequestBuilder {
    if token.is_empty() { rb } else { rb.header("X-ACECM-Token", token) }
}

enum PostErr {
    Http(u16, String),
    Other(String),
}

fn post_json(base: &str, path: &str, token: &str, obj: &Value, timeout: u64) -> Result<Value, PostErr> {
    let r = with_token(client(timeout).post(format!("{}{path}", base.trim_end_matches('/'))), token)
        .header("Content-Type", "application/json").body(obj.to_string()).send().map_err(|e| PostErr::Other(e.to_string()))?;
    let code = r.status().as_u16();
    let t = r.text().unwrap_or_default();
    if code >= 400 {
        return Err(PostErr::Http(code, t));
    }
    Ok(serde_json::from_str(if t.is_empty() { "{}" } else { &t }).unwrap_or(json!({})))
}

/// Turn an HTTP failure into something the user can act on.
fn why(code: u16, body: &str, what: &str) -> String {
    if code == 401 || code == 403 {
        return format!("{what}: the server refused the upload - check the admin token, and that it was started with --headless");
    }
    if let Ok(v) = serde_json::from_str::<Value>(body) {
        if let Some(e) = v.get("error").and_then(|e| e.as_str()).filter(|e| !e.is_empty()) {
            return format!("{what}: {e}");
        }
    }
    format!("{what}: HTTP {code}")
}

/// A progress callback that can travel with a streamed request body.
pub type Tick = std::sync::Arc<dyn Fn(u64, u64) + Send + Sync>;

struct Counting {
    f: std::fs::File,
    sent: u64,
    total: u64,
    progress: Option<Tick>,
}

impl Read for Counting {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = buf.len().min(CHUNK);
        let got = self.f.read(&mut buf[..n])?;
        if got > 0 {
            self.sent += got as u64;
            if let Some(p) = &self.progress {
                p(self.sent, self.total);
            }
        }
        Ok(got)
    }
}

/// Stream one file into the remote's staging folder; returns the running
/// byte count.
fn send_file(base: &str, token: &str, did: &str, path: &Path, progress: Option<Tick>, sent: u64, total: u64) -> Result<u64, PostErr> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let size = std::fs::metadata(path).map(|m| m.len()).map_err(|e| PostErr::Other(e.to_string()))?;
    let url = format!("{}/api/drop/part?id={did}&name={}", base.trim_end_matches('/'), quote(&name));
    let body = Counting { f: std::fs::File::open(path).map_err(|e| PostErr::Other(e.to_string()))?, sent, total, progress };
    let r = with_token(client(3600).post(url), token).header("Content-Type", "application/octet-stream")
        .body(reqwest::blocking::Body::sized(body, size)).send().map_err(|e| PostErr::Other(e.to_string()))?;
    let code = r.status().as_u16();
    let t = r.text().unwrap_or_default();
    if code >= 400 {
        return Err(PostErr::Http(code, t));
    }
    Ok(sent + size)
}

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

fn uuid4() -> String {
    let mut b = [0u8; 16];
    let _ = getrandom::fill(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

/// What this server is short of, as files on this machine.
pub fn plan(profile: &Value) -> (Vec<(String, Vec<PathBuf>)>, Vec<String>, Vec<String>) {
    let want = autoshare::needs(profile);
    let strs = |k: &str| want.get(k).and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect::<Vec<_>>()).unwrap_or_default();
    let src = mods::client_mods_dir();
    let (mut mods_, mut missing) = (Vec::new(), Vec::new());
    for name in strs("mods") {
        let here: Vec<PathBuf> = [".kspkg", ".json"].iter().map(|e| Path::new(&src).join(format!("{name}{e}"))).filter(|p| p.is_file()).collect();
        if here.iter().any(|p| p.to_string_lossy().ends_with(".kspkg")) {
            mods_.push((name, here));
        } else {
            missing.push(name);
        }
    }
    (mods_, strs("tracks"), missing)
}

/// Is there an ACECM there, and will it take our uploads?
pub fn check(base: &str, token: &str) -> Value {
    let base = base.trim().trim_end_matches('/');
    if !(base.starts_with("http://") || base.starts_with("https://")) {
        return json!({"ok": false, "error": "give the server's ACECM address, like http://100.x.y.z:8092"});
    }
    let r = match with_token(client(15).get(format!("{base}/api/auth")), token).send() {
        Ok(r) => r,
        Err(e) => return json!({"ok": false, "error": format!("no ACECM answering at {base} ({e})")}),
    };
    if r.status().as_u16() >= 400 {
        return json!({"ok": false, "error": format!("the server answered HTTP {}", r.status().as_u16())});
    }
    let got: Value = r.text().ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(json!({}));
    if got.get("authorised") == Some(&json!(true)) {
        return json!({"ok": true, "remote_admin": got.get("remote_admin")});
    }
    if got.get("remote_admin") != Some(&json!(true)) {
        return json!({"ok": false, "error": "that ACECM is not accepting remote administration - start it with --headless, or turn remote admin on in its Settings"});
    }
    json!({"ok": false, "error": "the server did not accept that admin token"})
}

/// Upload everything `profile` needs to the ACECM at `base`.
pub fn send(profile: &Value, base: &str, token: &str, overwrite: bool, progress: Option<Tick>) -> Value {
    let base = base.trim().trim_end_matches('/').to_string();
    if !(base.starts_with("http://") || base.starts_with("https://")) {
        return json!({"ok": false, "error": "give the server's ACECM address, like http://100.x.y.z:8092"});
    }
    // ⚠ the token BEFORE a gigabyte, or a wrong one surfaces as a socket abort
    let reach = check(&base, token);
    if reach.get("ok") != Some(&json!(true)) {
        return reach;
    }
    let (mods_, tracks, missing) = plan(profile);
    if mods_.is_empty() && tracks.is_empty() {
        return json!({"ok": false, "error": "nothing modded about this server - there is nothing to send"});
    }
    let mut files: Vec<PathBuf> = mods_.iter().flat_map(|(_, p)| p.iter().cloned()).collect();
    let mut packs = Vec::new();
    for folder in &tracks {
        match registry::ensure_track_pack(folder).filter(|p| p.is_file()) {
            Some(t) => packs.push(t),
            None => return json!({"ok": false, "error": format!("no files found for track {folder}")}),
        }
    }
    files.extend(packs.iter().cloned());
    let total: u64 = files.iter().map(|f| std::fs::metadata(f).map(|m| m.len()).unwrap_or(0)).sum();
    let mut sent = 0u64;
    let mut done = Vec::new();
    // ⚠ one drop per THING: a car pair is ingested together, a pack alone
    for (name, paths) in &mods_ {
        let did = uuid4();
        for p in paths {
            match send_file(&base, token, &did, p, progress.clone(), sent, total) {
                Ok(s) => sent = s,
                Err(PostErr::Http(c, b)) => return json!({"ok": false, "error": why(c, &b, name)}),
                Err(PostErr::Other(e)) => return json!({"ok": false, "error": format!("sending {name}: {e}")}),
            }
        }
        match post_json(&base, "/api/drop", token, &json!({"id": did, "overwrite": overwrite}), 600) {
            Ok(r) if r.get("ok") == Some(&json!(true)) => done.push(name.clone()),
            Ok(r) => return json!({"ok": false, "error": format!("{name}: {}", r.get("error").map(|e| e.to_string()).unwrap_or_default().trim_matches('"'))}),
            Err(PostErr::Http(c, b)) => return json!({"ok": false, "error": why(c, &b, name)}),
            Err(PostErr::Other(e)) => return json!({"ok": false, "error": format!("sending {name}: {e}")}),
        }
    }
    for (folder, tar) in tracks.iter().zip(packs.iter()) {
        let did = uuid4();
        let r = send_file(&base, token, &did, tar, progress.clone(), sent, total)
            .and_then(|s| { sent = s; post_json(&base, "/api/drop", token, &json!({"id": did, "overwrite": overwrite}), 3600) });
        match r {
            Ok(r) if r.get("ok") == Some(&json!(true)) => done.push(folder.clone()),
            Ok(r) => return json!({"ok": false, "error": format!("{folder}: {}", r.get("error").map(|e| e.to_string()).unwrap_or_default().trim_matches('"'))}),
            Err(PostErr::Http(c, b)) => return json!({"ok": false, "error": why(c, &b, folder)}),
            Err(PostErr::Other(e)) => return json!({"ok": false, "error": format!("sending {folder}: {e}")}),
        }
    }
    logs::info(format!("sent {} to {base}", done.join(", ")));
    json!({"ok": true, "sent": done, "bytes": sent, "missing_here": missing})
}

// ---------------------------------------------------------- server build --
// ⚠ never leaves this machine: the operator's own state (serverConfig holds
// their account file), logs, and ACECM's .bak safety copies
const SKIP_DIRS: [&str; 4] = ["serverconfig", "results", "logs", "crashdumps"];
const SKIP_EXT: [&str; 4] = [".log", ".old", ".tmp", ".part"];

fn server_skip(rel: &str) -> bool {
    let low = rel.replace('\\', "/").to_lowercase();
    let head = low.split('/').next().unwrap_or("");
    SKIP_DIRS.contains(&head) || low.contains(".bak") || SKIP_EXT.iter().any(|e| low.ends_with(e))
}

pub fn server_build_files() -> (String, Vec<(String, u64)>) {
    let root = crate::detect::effective_server_dir();
    if root.is_empty() || !Path::new(&root).is_dir() {
        return (String::new(), Vec::new());
    }
    let mut out = Vec::new();
    fn walk(root: &Path, rel_dir: &str, out: &mut Vec<(String, u64)>) {
        let dir = if rel_dir.is_empty() { root.to_path_buf() } else { root.join(rel_dir) };
        let Ok(rd) = std::fs::read_dir(&dir) else { return };
        let mut dirs = Vec::new();
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            let rel = if rel_dir.is_empty() { n.clone() } else { format!("{rel_dir}\\{n}") };
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                if !server_skip(&rel) {
                    dirs.push(rel);
                }
            } else if !server_skip(&rel) {
                if let Ok(m) = e.metadata() {
                    out.push((rel, m.len()));
                }
            }
        }
        for d in dirs {
            walk(root, &d, out);
        }
    }
    walk(Path::new(&root), "", &mut out);
    (root, out)
}

pub fn server_build_plan() -> Value {
    let (root, files) = server_build_files();
    json!({"ok": !files.is_empty(), "root": root, "files": files.len(), "bytes": files.iter().map(|f| f.1).sum::<u64>()})
}

/// Ship this machine's dedicated-server build to a remote ACECM, as one zip.
pub fn send_server_build(base: &str, token: &str, progress: Option<Tick>) -> Value {
    let reach = check(base, token);
    if reach.get("ok") != Some(&json!(true)) {
        return reach;
    }
    let (root, files) = server_build_files();
    if files.is_empty() {
        return json!({"ok": false, "error": "no dedicated server files here to send"});
    }
    let total: u64 = files.iter().map(|f| f.1).sum();
    let tmp = std::env::temp_dir().join("acecm_server_build.zip");
    let packed = (|| -> Result<(), String> {
        let mut z = zip::ZipWriter::new(std::io::BufWriter::new(std::fs::File::create(&tmp).map_err(|e| e.to_string())?));
        let opt = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated)
            .compression_level(Some(1)).large_file(true);
        let mut n = 0u64;
        for (rel, sz) in &files {
            z.start_file(rel.replace('\\', "/"), opt).map_err(|e| e.to_string())?;
            let mut f = std::fs::File::open(Path::new(&root).join(rel)).map_err(|e| e.to_string())?;
            std::io::copy(&mut f, &mut z).map_err(|e| e.to_string())?;
            n += sz;
            if let Some(p) = &progress {
                // packing is half the wait
                p(n / 2, total);
            }
        }
        z.finish().map_err(|e| e.to_string())?;
        Ok(())
    })();
    if let Err(e) = packed {
        let _ = std::fs::remove_file(&tmp);
        return json!({"ok": false, "error": format!("could not pack the build: {e}")});
    }
    let did = uuid4();
    let zsize = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
    let outer = progress.clone();
    let tick: Tick = std::sync::Arc::new(move |sent: u64, _t: u64| {
        if let Some(p) = &outer {
            p(total / 2 + (sent as f64 / zsize.max(1) as f64 * total as f64 / 2.0) as u64, total);
        }
    });
    let r = send_file(base, token, &did, &tmp, Some(tick), 0, zsize)
        .and_then(|_| post_json(base, "/api/server_build", token, &json!({"id": did}), 3600));
    let _ = std::fs::remove_file(&tmp);
    let r = match r {
        Ok(r) => r,
        Err(PostErr::Http(c, b)) => return json!({"ok": false, "error": why(c, &b, "server build")}),
        Err(PostErr::Other(e)) => return json!({"ok": false, "error": format!("sending the server build: {e}")}),
    };
    if r.get("ok") != Some(&json!(true)) {
        let e = r.get("error").and_then(|e| e.as_str()).filter(|e| !e.is_empty()).unwrap_or("the server refused it");
        return json!({"ok": false, "error": e});
    }
    logs::info(format!("sent a {}-file server build to {base}", files.len()));
    json!({"ok": true, "files": files.len(), "bytes": zsize, "applied": r.get("applied"), "root": r.get("root")})
}
