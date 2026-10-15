//! Log viewing for the Diagnostics page - ported from logs.tail / logs.files.
//! (The backend still WRITES acecm.log; the shell only reads it.)

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde_json::{json, Value};

use crate::paths;

pub fn log_dir() -> PathBuf {
    let d = paths::data().join("logs");
    let _ = std::fs::create_dir_all(&d);
    d
}

fn show(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Recent log lines. `which` is reduced to a bare file name, so it can
/// never point outside the log folder.
pub fn tail(lines: usize, which: &str) -> Value {
    let name = Path::new(which).file_name().map(|n| n.to_owned());
    let Some(name) = name else {
        return json!({"ok": false, "error": format!("no log at {}", show(&log_dir()))});
    };
    let p = log_dir().join(name);
    if !p.is_file() {
        return json!({"ok": false, "error": format!("no log at {}", show(&p))});
    }
    let bytes = match std::fs::read(&p) {
        Ok(b) => b,
        Err(e) => return json!({"ok": false, "error": e.to_string()}),
    };
    let text = String::from_utf8_lossy(&bytes);
    // text-mode readlines(): \r\n reads as \n, and a final line without a
    // newline still counts
    let all: Vec<&str> = text.split_inclusive('\n').collect();
    let start = all.len().saturating_sub(lines);
    let got: Vec<String> = all[start..]
        .iter()
        .map(|l| l.trim_end_matches('\n').trim_end_matches('\r').to_string())
        .collect();
    json!({"ok": true, "file": show(&p), "lines": got})
}

pub fn files() -> Value {
    let d = log_dir();
    let mut names: Vec<String> = std::fs::read_dir(&d)
        .map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    names.sort();
    let out: Vec<Value> = names
        .iter()
        .map(|n| {
            let m = std::fs::metadata(d.join(n)).ok();
            let size = m.as_ref().map(|m| m.len()).unwrap_or(0);
            let mtime = m
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            json!({"name": n, "size": size, "mtime": mtime})
        })
        .collect();
    json!({"dir": show(&d), "files": out})
}

// ------------------------------------------------------------- writing --
// ⚠ Same file and the SAME line format as the backend's logging, so one log
// tells the whole story while both halves run. Opened per line, appended and
// closed: the backend rotates acecm.log by renaming it, and a handle held
// open here would make that rename fail.
static WRITE: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn write(level: &str, msg: &str) {
    use std::io::Write;
    let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let line = format!("{stamp} {level:<7} {:<12} acecm: {msg}\n", "native");
    let _g = WRITE.lock();
    let p = log_dir().join("acecm.log");
    // no backend to rotate it any more: do it here, the same way
    if !crate::sidecar::running() {
        if std::fs::metadata(&p).map(|m| m.len() >= 4 << 20).unwrap_or(false) {
            let _ = std::fs::remove_file(log_dir().join("acecm.log.5"));
            for i in (1..5).rev() {
                let _ = std::fs::rename(log_dir().join(format!("acecm.log.{i}")), log_dir().join(format!("acecm.log.{}", i + 1)));
            }
            let _ = std::fs::rename(&p, log_dir().join("acecm.log.1"));
        }
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let _ = f.write_all(line.as_bytes());
    }
    eprint!("{line}");
}

pub fn info(msg: impl AsRef<str>) {
    write("INFO", msg.as_ref());
}

pub fn warn(msg: impl AsRef<str>) {
    write("WARNING", msg.as_ref());
}
