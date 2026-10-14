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
