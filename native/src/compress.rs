//! Shrink the mods folder on disk with transparent NTFS compression
//! (`compact /exe:LZX`, the WOF overlay) - ported from compress.py.
//! Measured on a real folder: 43.1 GB of mods stored in 11 GB.
//!
//! ⚠ Windows only. ⚠ WOF is for read-mostly data: writing a compressed file
//! rehydrates it, so this targets the mods folder and nothing else.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use serde_json::{json, Map, Value};

static STATE: Mutex<Option<Map<String, Value>>> = Mutex::new(None);
static CACHE: Mutex<Option<(Instant, String, Value)>> = Mutex::new(None);
static MEASURING: Mutex<bool> = Mutex::new(false);
const TTL: u64 = 90;

pub fn supported() -> bool {
    cfg!(windows)
}

fn mods_dir() -> String {
    let d = crate::mods::client_mods_dir();
    if !d.is_empty() && Path::new(&d).is_dir() { d } else { String::new() }
}

/// What the filesystem really spends (the logical size never changes).
#[cfg(windows)]
fn on_disk(p: &Path) -> Option<u64> {
    use windows_sys::Win32::Storage::FileSystem::GetCompressedFileSizeW;
    let w: Vec<u16> = p.as_os_str().to_string_lossy().encode_utf16().chain(Some(0)).collect();
    let mut high: u32 = 0;
    let low = unsafe { GetCompressedFileSizeW(w.as_ptr(), &mut high) };
    if low == u32::MAX && std::io::Error::last_os_error().raw_os_error().unwrap_or(0) != 0 {
        return None;
    }
    Some(((high as u64) << 32) | low as u64)
}

#[cfg(not(windows))]
fn on_disk(p: &Path) -> Option<u64> {
    std::fs::metadata(p).ok().map(|m| m.len())
}

fn walk(root: &str) -> Value {
    let (mut logical, mut disk, mut files) = (0u64, 0u64, 0u64);
    let mut stack = vec![PathBuf::from(root)];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                stack.push(e.path());
                continue;
            }
            let (Ok(md), Some(od)) = (e.metadata(), on_disk(&e.path())) else { continue };
            logical += md.len();
            disk += od;
            files += 1;
        }
    }
    json!({"ok": true, "supported": true, "path": root, "files": files, "logical": logical, "on_disk": disk,
           "ratio": if disk != 0 { logical as f64 / disk as f64 } else { 0.0 }, "saved": logical.saturating_sub(disk)})
}

fn with(mut v: Value, extra: &[(&str, bool)]) -> Value {
    for (k, b) in extra {
        v[*k] = json!(b);
    }
    v
}

/// Logical vs on-disk for the mods folder - cached (a 90k-file walk takes
/// seconds); `block=false` answers at once and measures behind it.
pub fn measure(force: bool, block: bool) -> Value {
    if !supported() {
        return json!({"ok": false, "supported": false, "error": "NTFS compression is Windows only"});
    }
    let root = mods_dir();
    if root.is_empty() {
        return json!({"ok": false, "supported": true, "error": "no mods folder yet"});
    }
    let cached = CACHE.lock().unwrap().as_ref().filter(|(_, r, _)| *r == root).map(|(t, _, v)| (t.elapsed().as_secs(), v.clone()));
    if !force {
        if let Some((age, v)) = &cached {
            if *age < TTL {
                return with(v.clone(), &[("cached", true)]);
            }
        }
    }
    if !block {
        let mut busy = MEASURING.lock().unwrap();
        if !*busy {
            *busy = true;
            let r = root.clone();
            std::thread::spawn(move || {
                let v = walk(&r);
                *CACHE.lock().unwrap() = Some((Instant::now(), r, v));
                *MEASURING.lock().unwrap() = false;
            });
        }
        return match cached {
            Some((_, v)) => with(v, &[("cached", true), ("stale", true)]),
            None => json!({"ok": false, "supported": true, "measuring": true, "path": root, "error": "measuring the mods folder…"}),
        };
    }
    let v = walk(&root);
    *CACHE.lock().unwrap() = Some((Instant::now(), root, v.clone()));
    v
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

/// Run compact over the mods folder in the background.
pub fn start(undo: bool) -> Value {
    if !supported() {
        return json!({"ok": false, "error": "NTFS compression is Windows only"});
    }
    let root = mods_dir();
    if root.is_empty() {
        return json!({"ok": false, "error": "no mods folder yet"});
    }
    {
        let mut g = STATE.lock().unwrap();
        if g.as_ref().and_then(|s| s.get("active")).and_then(|v| v.as_bool()).unwrap_or(false) {
            return json!({"ok": false, "error": "already running"});
        }
        let mut m = Map::new();
        m.insert("active".into(), json!(true));
        m.insert("phase".into(), json!("measuring"));
        m.insert("undo".into(), json!(undo));
        *g = Some(m);
    }
    std::thread::spawn(move || {
        let before = measure(true, true);
        set(json!({"phase": "working", "before": before}));
        // /c compress  /u uncompress  /s recurse  /i ignore errors  /f force
        let mut c = std::process::Command::new("compact");
        c.args([if undo { "/u" } else { "/c" }, "/s", "/i", "/f"]);
        if !undo {
            c.arg("/exe:LZX");
        }
        c.arg(Path::new(&root).join("*")).current_dir(&root);
        crate::procs::hide(&mut c);
        match c.output() {
            Ok(o) => {
                let text = String::from_utf8_lossy(&o.stdout).trim().to_string();
                let lines: Vec<&str> = text.lines().collect();
                let tail: Vec<&str> = lines[lines.len().saturating_sub(3)..].iter().map(|l| l.trim()).collect();
                let after = measure(true, true);
                set(json!({"active": false, "phase": "done", "before": before, "after": after, "summary": tail.join(" ")}));
            }
            Err(e) => {
                crate::logs::warn(format!("compact failed: {e}"));
                set(json!({"active": false, "phase": "error", "error": e.to_string()}));
            }
        }
    });
    json!({"ok": true, "started": true})
}
