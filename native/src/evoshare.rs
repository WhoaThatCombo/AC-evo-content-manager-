//! Fetch content from a directory server's own share - ported from
//! evoshare.py. Unauthenticated:
//!
//!     GET <base>evoforge.json     manifest {content[], max, active, ...}
//!     GET <base>index/<file>      gzipped json index of the pack
//!     GET <base>content/<file>    the .kspkg itself, Range-resumable
//!
//! The index carries absolute byte spans, so only what is missing is pulled.
//! Entry bytes are ordinary kspkg payload: decrypt each at offset 0, never a
//! .texturemips.
//! ⚠ The key is the `file` name, never the `id`. ⚠ Other paths are relayed
//! to the game server (502 / status JSON does not mean the share is down).
//! Be a good guest: one stream at a time.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use crate::{contentsync, kspkg, logs};

const TIMEOUT: u64 = 8;
const GAP_TOLERANCE: u64 = 1 << 20;
const CHUNK: usize = 1 << 20;

static STATE: Mutex<Option<Map<String, Value>>> = Mutex::new(None);

fn sv(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(x) => x.to_string(),
    }
}

fn url(base: &str, parts: &[&str]) -> String {
    let mut b = base.trim().to_string();
    if !b.ends_with('/') {
        b.push('/');
    }
    b + &parts.iter().map(|p| p.trim_start_matches('/')).collect::<Vec<_>>().join("/")
}

fn get(u: &str, rng: Option<(u64, u64)>, timeout: u64) -> Result<reqwest::blocking::Response, String> {
    let c = reqwest::blocking::Client::builder().timeout(Duration::from_secs(timeout)).user_agent(crate::evoreg::ua()).build().map_err(|e| e.to_string())?;
    let mut rb = c.get(u).header("Accept", "*/*");
    if let Some((a, b)) = rng {
        rb = rb.header("Range", format!("bytes={a}-{b}"));
    }
    let r = rb.send().map_err(|e| format!("<urlopen error {e}>"))?;
    if r.status().as_u16() >= 400 {
        return Err(format!("HTTP Error {}: {}", r.status().as_u16(), r.status().canonical_reason().unwrap_or("")));
    }
    Ok(r)
}

/// What this share publishes.
pub fn manifest(base: &str) -> Value {
    let d = match get(&url(base, &["evoforge.json"]), None, TIMEOUT).and_then(|r| r.text().map_err(|e| e.to_string()))
        .and_then(|t| serde_json::from_str::<Value>(&t).map_err(|e| e.to_string())) {
        Ok(d) => d,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let has = d.get("content").map(|c| match c { Value::Array(a) => !a.is_empty(), Value::Null => false, _ => true }).unwrap_or(false);
    if !d.is_object() || !has {
        return json!({"ok": false, "error": "not a content share"});
    }
    let mut out = Map::new();
    out.insert("ok".into(), json!(true));
    out.extend(d.as_object().cloned().unwrap_or_default());
    Value::Object(out)
}

/// The pack's entry table (served gzipped).
pub fn index(base: &str, fname: &str) -> Value {
    let mut raw = Vec::new();
    match get(&url(base, &["index", fname]), None, TIMEOUT) {
        Ok(mut r) => { if let Err(e) = r.read_to_end(&mut raw) { return json!({"ok": false, "error": e.to_string()}) } }
        Err(e) => return json!({"ok": false, "error": e}),
    }
    if raw.starts_with(&[0x1f, 0x8b]) {
        let mut out = Vec::new();
        if let Err(e) = flate2::read::GzDecoder::new(&raw[..]).read_to_end(&mut out) {
            return json!({"ok": false, "error": format!("bad gzip: {e}")});
        }
        raw = out;
    }
    let d: Value = match serde_json::from_slice(&raw) {
        Ok(d) => d,
        Err(e) => return json!({"ok": false, "error": format!("bad index: {e}")}),
    };
    let mut out = Map::new();
    out.insert("ok".into(), json!(true));
    out.extend(d.as_object().cloned().unwrap_or_default());
    Value::Object(out)
}

/// Pack path (content\tracks\X\...) -> the path destination() understands.
fn rel(n: &str) -> String {
    let p = n.replace('\\', "/");
    let p = p.trim_start_matches('/');
    if p.to_lowercase().starts_with("content/") { p[8..].to_string() } else { p.to_string() }
}

/// The mechanical preset names inside a car pack (the .json descriptor
/// needs them, or the car never appears in the list).
fn car_presets(idx: &Value) -> Vec<String> {
    let mut out: std::collections::BTreeSet<String> = Default::default();
    for e in idx.get("entries").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
        let n = sv(&e, "n").replace('\\', "/");
        if n.to_lowercase().ends_with(".mechanicalcarpreset") {
            let leaf = n.rsplit('/').next().unwrap_or("");
            out.insert(leaf.rsplit_once('.').map(|(a, _)| a).unwrap_or(leaf).to_string());
        }
    }
    out.into_iter().collect()
}

fn int(v: Option<&Value>) -> u64 {
    match v {
        Some(Value::Number(n)) => n.as_u64().or_else(|| n.as_f64().map(|f| f as u64)).unwrap_or(0),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

/// A car mod installs as a WHOLE pack plus its .json beside it.
fn plan_car(base: &str, fname: &str) -> Value {
    let d = crate::mods::client_mods_dir();
    if d.is_empty() {
        return json!({"ok": false, "error": "no client mods folder"});
    }
    // ⚠ fname is the SHARE's: a "..\\..\\Startup\\x.cmd" wrote anywhere
    if crate::install::safe_base(fname) != fname || !fname.to_lowercase().ends_with(".kspkg") {
        return json!({"ok": false, "error": "not a car package name"});
    }
    let dest = match contentsync::under(&d, &[fname]) { Ok(p) => p, Err(e) => return json!({"ok": false, "error": e}) };
    let base_name = &fname[..fname.len() - 6];
    let side = Path::new(&d).join(format!("{base_name}.json"));
    let man = manifest(base);
    let mut want = 0;
    for c in man.get("content").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
        if sv(&c, "file") == fname {
            want = int(c.get("bytes"));
        }
    }
    let have = dest.is_file() && (want == 0 || std::fs::metadata(&dest).map(|m| m.len() == want).unwrap_or(false));
    let have_side = side.is_file();
    json!({"ok": true, "kind": "car", "dest": dest.to_string_lossy(), "side": side.to_string_lossy(),
           "bytes": if have { 0 } else { want }, "have": have, "have_side": have_side,
           "need": if have && have_side { json!([]) } else { json!([{"rel": fname, "s": want}]) }})
}

/// What of this pack is missing here (by size: the index hash is empty).
pub fn plan(base: &str, fname: &str, kind: &str) -> Value {
    if kind.to_lowercase() == "car" {
        return plan_car(base, fname);
    }
    let idx = index(base, fname);
    if idx.get("ok") != Some(&json!(true)) {
        return idx;
    }
    let entries = idx.get("entries").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let (mut need, mut have, mut skipped) = (Vec::new(), 0, 0);
    for e in &entries {
        let r = rel(&sv(e, "n"));
        let Ok(dest) = contentsync::destination(&r) else { skipped += 1; continue };
        let size = int(e.get("s"));
        if dest.is_file() && std::fs::metadata(&dest).map(|m| m.len() == size).unwrap_or(false) {
            have += 1;
            continue;
        }
        need.push(json!({"n": e.get("n"), "rel": r, "dest": dest.to_string_lossy(), "o": int(e.get("o")), "s": size}));
    }
    let bytes: u64 = need.iter().map(|x| int(x.get("s"))).sum();
    json!({"ok": true, "total": entries.len(), "have": have, "skipped": skipped, "need": need, "bytes": bytes,
           "pack_bytes": int(idx.get("bytes"))})
}

pub fn status() -> Value {
    let mut out = Map::new();
    out.insert("ok".into(), json!(true));
    match STATE.lock().unwrap().as_ref() {
        Some(s) => out.extend(s.clone()),
        None => { out.insert("active".into(), json!(false)); }
    }
    Value::Object(out)
}

fn set(kv: Value) {
    if let Value::Object(kv) = kv {
        let mut g = STATE.lock().unwrap();
        let m = g.get_or_insert_with(Map::new);
        for (k, v) in kv {
            m.insert(k, v);
        }
    }
}

fn st_get(k: &str) -> Value {
    STATE.lock().unwrap().as_ref().and_then(|m| m.get(k).cloned()).unwrap_or(Value::Null)
}

fn cancelled() -> bool {
    st_get("cancel") == json!(true)
}

/// Download what is missing, in a background thread.
pub fn start(base: &str, fname: &str, folder: &str, kind: &str) -> Value {
    let job = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
    {
        let mut g = STATE.lock().unwrap();
        if g.as_ref().and_then(|m| m.get("active")).and_then(|v| v.as_bool()).unwrap_or(false) {
            return json!({"ok": false, "error": "a download is already running"});
        }
        let m = json!({"active": true, "phase": "planning", "done": 0, "total": 0, "bytes": 0, "want": 0, "error": "",
                       "server": base, "file": fname, "kind": kind, "job": job});
        *g = m.as_object().cloned();
    }
    let (b, f, fo, k) = (base.to_string(), fname.to_string(), folder.to_string(), kind.to_string());
    std::thread::spawn(move || {
        // ⚠ a panic must not leave the job "active" (every later download
        // was refused as "already running" until a restart)
        let r = std::panic::catch_unwind(|| if k.to_lowercase() == "car" { run_car(&b, &f) } else { run(&b, &f, &fo) })
            .unwrap_or_else(|_| Err("the download stopped unexpectedly".into()));
        if let Err(e) = r {
            logs::warn(format!("evoshare download failed: {e}"));
            set(json!({"active": false, "phase": "error", "error": e}));
        }
    });
    json!({"ok": true, "started": true, "job": job})
}

pub fn cancel() -> Value {
    set(json!({"cancel": true}));
    json!({"ok": true})
}

/// Decrypt exactly as kspkg.read_entry does, then place the file.
fn write_entry(it: &Value, raw: &[u8]) -> Result<(), String> {
    let mut data = raw.to_vec();
    if !sv(it, "n").to_lowercase().ends_with(".texturemips") {
        kspkg::decrypt(&mut data, 0);
    }
    let dest = PathBuf::from(sv(it, "dest"));
    if let Some(p) = dest.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    let tmp = PathBuf::from(format!("{}.part", dest.display()));
    std::fs::write(&tmp, &data).map_err(|e| e.to_string())?;
    // rename replaces: deleting first lost the old file when the rename failed
    std::fs::rename(&tmp, &dest).map_err(|e| { let _ = std::fs::remove_file(&tmp); e.to_string() })
}

fn run(base: &str, fname: &str, folder: &str) -> Result<(), String> {
    let p = plan(base, fname, "track");
    if p.get("ok") != Some(&json!(true)) {
        let e = sv(&p, "error");
        set(json!({"active": false, "phase": "error", "error": if e.is_empty() { "no plan".to_string() } else { e }}));
        return Ok(());
    }
    let mut need: Vec<Value> = p["need"].as_array().cloned().unwrap_or_default();
    let targets: Vec<(PathBuf, u64)> = need.iter().map(|it| (PathBuf::from(sv(it, "dest")), int(it.get("s")))).collect();
    if let Some(why) = crate::install::room_for_all(&targets) {
        return Err(why);
    }
    if need.is_empty() {
        set(json!({"active": false, "phase": "done", "done": 0, "total": 0, "note": "you already have everything this share offers"}));
        return Ok(());
    }
    set(json!({"phase": "downloading", "total": need.len(), "want": p["bytes"]}));
    // coalesce into contiguous runs, in offset order
    need.sort_by_key(|x| int(x.get("o")));
    let mut runs: Vec<(u64, u64, Vec<Value>)> = Vec::new();
    for it in need.iter().cloned() {
        let (o, s) = (int(it.get("o")), int(it.get("s")));
        // entries are u32-sized in a kspkg; anything else is a broken index
        if s > u32::MAX as u64 || o.checked_add(s).is_none() {
            return Err("the share's index is damaged".into());
        }
        match runs.last_mut() {
            Some(r) if o.saturating_sub(r.1) <= GAP_TOLERANCE && o + 0 >= r.0 => { r.1 = r.1.max(o + s); r.2.push(it); }
            _ => runs.push((o, o + s, vec![it])),
        }
    }
    let u = url(base, &["content", fname]);
    let (mut got, mut done) = (0u64, 0u64);
    for (start, end, items) in runs {
        if cancelled() {
            set(json!({"active": false, "phase": "cancelled"}));
            return Ok(());
        }
        let mut r = get(&u, Some((start, end.saturating_sub(1))), 60)?;
        let mut buf: Vec<u8> = Vec::new();
        // the stream offset buf[0] stands for: written entries are dropped
        // from the front, so a run never holds more than one entry ahead
        let mut base = start;
        let mut chunk = vec![0u8; CHUNK];
        for it in items {
            if cancelled() {
                set(json!({"active": false, "phase": "cancelled"}));
                return Ok(());
            }
            let (o, s) = (int(it.get("o")), int(it.get("s")));
            if o < base {
                return Err("the share's index has overlapping entries".into());
            }
            let want_to = (o + s - base) as usize;
            while buf.len() < want_to {
                let n = r.read(&mut chunk).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            if buf.len() < want_to {
                return Err("the share closed the stream early".into());
            }
            let at = (o - base) as usize;
            write_entry(&it, &buf[at..at + s as usize])?;
            buf.drain(..at + s as usize);
            base = o + s;
            got += s;
            done += 1;
            set(json!({"done": done, "bytes": got}));
        }
    }
    // ⚠ on disk is not loadable: the client tables need the track, and that
    // edit is refused while the game runs - report what really happened
    let folder = if folder.is_empty() {
        need.iter().find_map(|it| { let r = sv(it, "rel"); let parts: Vec<&str> = r.split('/').collect(); (parts.len() >= 2 && parts[0] == "tracks").then(|| parts[1].to_string()) }).unwrap_or_default()
    } else { folder.to_string() };
    let reg = if folder.is_empty() { json!({"ok": true, "skipped": true}) } else { contentsync::register_downloaded_track(&folder) };
    let ok = reg.get("ok") == Some(&json!(true));
    let note = if ok { String::new() } else { let e = sv(&reg, "error"); if e.is_empty() { "could not add it to the track list".into() } else { e } };
    set(json!({"active": false, "phase": "done", "folder": folder, "registered": ok,
               "needs_close": reg.get("needs_close") == Some(&json!(true)), "note": note}));
    crate::install::after_content_change();
    Ok(())
}

/// A whole car pack (resumable), then the descriptor beside it.
fn run_car(base: &str, fname: &str) -> Result<(), String> {
    let p = plan_car(base, fname);
    if p.get("ok") != Some(&json!(true)) {
        let e = sv(&p, "error");
        set(json!({"active": false, "phase": "error", "error": if e.is_empty() { "no plan".to_string() } else { e }}));
        return Ok(());
    }
    let (dest, side) = (PathBuf::from(sv(&p, "dest")), PathBuf::from(sv(&p, "side")));
    let want = int(p.get("bytes"));
    if p["have"] == json!(true) && p["have_side"] == json!(true) {
        set(json!({"active": false, "phase": "done", "total": 0, "done": 0, "note": "you already have this car"}));
        return Ok(());
    }
    set(json!({"phase": "downloading", "total": 1, "want": want}));
    if p["have"] != json!(true) {
        if let Some(why) = crate::install::room_for(&dest, want) {
            return Err(why);
        }
        if let Some(d) = dest.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let tmp = PathBuf::from(format!("{}.part", dest.display()));
        // resume a part-finished file rather than starting the GB again
        let at = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
        // ⚠ a .part as big as the car (or bigger) is not resumable
        let at = if want > 0 && at >= want { 0 } else { at };
        let rng = if at > 0 && want > 0 { Some((at, want - 1)) } else { None };
        let mut r = get(&url(base, &["content", fname]), rng, 60)?;
        // ⚠ resume ONLY on a 206: a host that ignores Range sends the whole
        // file, which appended to the part made a corrupt car
        let at = if rng.is_some() && r.status().as_u16() != 206 { 0 } else { at };
        let mut f = std::fs::OpenOptions::new().create(true).write(true).append(at > 0).truncate(at == 0).open(&tmp).map_err(|e| e.to_string())?;
        let mut got = at;
        let mut chunk = vec![0u8; CHUNK];
        loop {
            if cancelled() {
                set(json!({"active": false, "phase": "cancelled"}));
                return Ok(());
            }
            let n = r.read(&mut chunk).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            if want > 0 && got + n as u64 > want {
                return Err("the car is larger than the share declared".into());
            }
            f.write_all(&chunk[..n]).map_err(|e| e.to_string())?;
            got += n as u64;
            set(json!({"bytes": got}));
        }
        drop(f);
        if want > 0 && got != want {
            return Err(format!("the car arrived incomplete ({got} of {want} bytes) - try again to resume"));
        }
        std::fs::rename(&tmp, &dest).map_err(|e| e.to_string())?;
        set(json!({"done": 1}));
    }
    // ⚠ the .json is what makes the car APPEAR
    let mut wrote = false;
    if !side.is_file() {
        let idx = index(base, fname);
        let presets = if idx.get("ok") == Some(&json!(true)) { car_presets(&idx) } else { Vec::new() };
        let mut label = String::new();
        for c in manifest(base).get("content").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
            if sv(&c, "file") == fname {
                label = { let n = sv(&c, "name"); if n.is_empty() { sv(&c, "id") } else { n } };
            }
        }
        if !presets.is_empty() {
            let cars: Vec<Value> = presets.iter().map(|n| json!({"name": n, "display_name": if label.is_empty() { n.clone() } else { label.clone() },
                "performance_indicator": 0, "property_1": 0, "property_2": 0, "property_3": 0})).collect();
            let mut s = crate::profiles::py_dumps_indent2(&json!({"cars": cars}));
            if cfg!(windows) {
                s = s.replace('\n', "\r\n");
            }
            std::fs::write(&side, s).map_err(|e| e.to_string())?;
            wrote = true;
        } else {
            logs::warn(format!("no mechanicalcarpreset in {fname} - cannot write the descriptor, the car will not be listed"));
        }
    }
    let note = if wrote || side.is_file() { String::new() } else { "downloaded, but no descriptor could be built - the car will not appear in the car list".to_string() };
    set(json!({"active": false, "phase": "done", "kind": "car", "dest": dest.to_string_lossy(), "wrote_descriptor": wrote, "note": note}));
    crate::install::after_content_change();
    Ok(())
}
