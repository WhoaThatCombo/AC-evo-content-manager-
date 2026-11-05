//! The evoview tab's Replays page: the game's own replays (Saved Games\ACE\
//! Replay), what is in each, and playback in evoview.
//!
//! The game names a replay `ACEVO_<yymmdd>-<hhmmss>_<kind>_<car>_<track>.rpy`
//! (kind R race, T everything else), which is enough to list it at once.
//! What is inside - the track folder evoview loads, the cars, how much motion
//! was captured - comes from `evoview --replay-info`, one process for a batch
//! of files, and is kept in data/replay_info.json keyed by path, size and
//! modification time (the big ones run to 700 MB; each is read once).

use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::{evodrive, paths, viewer};

/// The game's two replay folders: kept replays, and the recent ones it
/// writes by itself after each session.
fn dirs() -> Vec<(&'static str, PathBuf)> {
    let ace = crate::detect::find("ace_dir", false);
    if ace.is_empty() {
        return vec![];
    }
    let r = Path::new(&ace).join("Replay");
    vec![("saved", r.join("Saved")), ("recent", r.join("Temp"))]
}

/// A path the page sent, accepted only when it is a .rpy file inside one of
/// the replay folders (the API is reachable from the LAN).
fn checked(p: &str) -> Option<PathBuf> {
    let p = PathBuf::from(p);
    if p.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("rpy")) != Some(true) {
        return None;
    }
    let c = std::fs::canonicalize(&p).ok()?;
    dirs().iter().any(|(_, d)| std::fs::canonicalize(d).map_or(false, |d| c.starts_with(d))).then_some(p)
}

/// What the file name says: date, session kind, car, track name.
fn from_name(stem: &str) -> Value {
    let Some(rest) = stem.strip_prefix("ACEVO_") else { return json!({}) };
    let mut it = rest.splitn(3, '_');
    let (Some(stamp), Some(kind), Some(tail)) = (it.next(), it.next(), it.next()) else { return json!({}) };
    // 260422-124837 -> 2026-04-22 12:48
    let date = (stamp.len() == 13 && stamp.as_bytes()[6] == b'-').then(|| {
        format!("20{}-{}-{} {}:{}", &stamp[0..2], &stamp[2..4], &stamp[4..6], &stamp[7..9], &stamp[9..11])
    });
    // the track name is the last '_' field (names have spaces, never '_');
    // the car id is everything before it
    let (car, track) = tail.rsplit_once('_').unwrap_or((tail, ""));
    json!({"date": date, "kind": if kind == "R" { "Race" } else { "Practice" },
           "car": car, "track_name": track})
}

static CACHE: Mutex<Option<Map<String, Value>>> = Mutex::new(None);

fn cache_path() -> PathBuf {
    paths::data().join("replay_info.json")
}

fn key(p: &Path) -> Option<String> {
    let m = std::fs::metadata(p).ok()?;
    let t = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    Some(format!("{}|{}|{}", p.display(), m.len(), t))
}

fn with_cache<R>(f: impl FnOnce(&mut Map<String, Value>) -> R) -> R {
    let mut g = CACHE.lock().unwrap();
    let c = g.get_or_insert_with(|| {
        std::fs::read(cache_path()).ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default()
    });
    f(c)
}

/// GET /api/evoview/replays: every replay, newest first, with what its name
/// says and whatever --replay-info has already told us about it.
pub fn list() -> Value {
    let mut out = vec![];
    for (kind, d) in dirs() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()).map(|x| x.eq_ignore_ascii_case("rpy")) != Some(true) {
                continue;
            }
            let Ok(m) = e.metadata() else { continue };
            let mtime = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |t| t.as_secs());
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
            let info = key(&p).and_then(|k| with_cache(|c| c.get(&k).cloned()));
            out.push(json!({"path": p.display().to_string(), "name": stem, "folder": kind,
                            "size": m.len(), "mtime": mtime, "parsed": from_name(&stem), "info": info}));
        }
    }
    out.sort_by_key(|v| std::cmp::Reverse(v["mtime"].as_u64().unwrap_or(0)));
    let pkg = viewer::package();
    let tracks = pkg.as_deref().map(|p| evodrive::tracks(Path::new(p))).unwrap_or_default();
    let dirs: Vec<String> = dirs().iter().map(|(_, d)| d.display().to_string()).collect();
    json!({"ok": true, "replays": out, "dirs": dirs, "exe": viewer::viewer_exe(),
           "tracks": tracks.iter().map(|t| json!({"folder": t["folder"], "name": t["name"]})).collect::<Vec<_>>()})
}

/// POST /api/evoview/replay_info {paths}: read the ones not cached yet
/// (`evoview --replay-info`, one process for them all) and return every
/// asked-for path's info.
pub fn info(body: &Value) -> Value {
    let asked: Vec<PathBuf> = body.get("paths").and_then(|v| v.as_array()).into_iter().flatten()
        .filter_map(|v| v.as_str()).filter_map(checked).take(16).collect();
    // one reader at a time: the page asks again when a big batch outlasts its
    // request timeout, and that second ask should find the first one's work
    static READING: Mutex<()> = Mutex::new(());
    let _one = READING.lock().unwrap_or_else(|e| e.into_inner());
    let todo: Vec<&PathBuf> = asked.iter()
        .filter(|p| key(p).map_or(false, |k| with_cache(|c| !c.contains_key(&k)))).collect();
    if !todo.is_empty() {
        let Some(exe) = viewer::viewer_exe() else {
            return json!({"ok": false, "error": "evoview.exe not found"});
        };
        let mut c = std::process::Command::new(&exe);
        c.arg("--replay-info").args(todo.iter().map(|p| p.as_os_str()))
            .stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        crate::procs::hide(&mut c);
        let out = match c.output() {
            Ok(o) => o,
            Err(e) => return json!({"ok": false, "error": format!("could not run evoview: {e}")}),
        };
        let text = String::from_utf8_lossy(&out.stdout);
        let mut got = 0;
        for line in text.lines() {
            let Some(j) = line.strip_prefix("REPLAYINFO ") else { continue };
            let Ok(v) = serde_json::from_str::<Value>(j) else { continue };
            let Some(p) = v.get("file").and_then(|f| f.as_str()).map(PathBuf::from) else { continue };
            if let Some(k) = key(&p) {
                with_cache(|c| c.insert(k, v));
                got += 1;
            }
        }
        if got == 0 {
            // an evoview without --replay-info prints its usage instead
            return json!({"ok": false, "error": "this evoview cannot read replay details yet (update evoview)"});
        }
        let snapshot = with_cache(|c| Value::Object(c.clone()));
        let _ = crate::safefs::write_atomic(&cache_path(), serde_json::to_vec_pretty(&snapshot).unwrap_or_default().as_slice());
    }
    let infos: Map<String, Value> = asked.iter().filter_map(|p| {
        let k = key(p)?;
        Some((p.display().to_string(), with_cache(|c| c.get(&k).cloned())?))
    }).collect();
    json!({"ok": true, "info": infos})
}

/// POST /api/evoview/replay_play {path, env, time, replace}: watch a replay in
/// evoview. `env` is the graphics page's knobs, filtered like a drive's.
pub fn play(body: &Value) -> Value {
    let Some(path) = body.get("path").and_then(|v| v.as_str()).and_then(checked) else {
        return json!({"ok": false, "error": "that is not a replay in the game's replay folders"});
    };
    let Some(exe) = viewer::viewer_exe() else {
        return json!({"ok": false, "error": "evoview.exe not found. Put it in the tools folder next to ACECM, or set viewer_exe in Settings."});
    };
    let Some(pkg) = viewer::package() else {
        return json!({"ok": false, "error": "game install not found - set the game folder in Settings"});
    };
    let mut args = vec!["--replay".to_string(), path.display().to_string(), "--pkg".to_string(), pkg];
    if let Some(t) = body.get("time").and_then(|v| v.as_f64()).filter(|t| *t > 0.0 && *t < 1.0e6) {
        args.push("--time".into());
        args.push(format!("{t:.1}"));
    }
    let mut skipped = vec![];
    let env = evodrive::env_from(body, Path::new(&exe), &mut skipped);
    let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("replay");
    let p = from_name(name);
    let what = match p["track_name"].as_str() {
        Some(t) if !t.is_empty() => format!("the replay at {t}"),
        _ => format!("replay {name}"),
    };
    evodrive::spawn(&exe, args, env, what, skipped, body.get("replace") == Some(&json!(true)))
}

/// POST /api/evoview/replay_folder {path?}: show a replay (or the folder) in Explorer.
pub fn reveal(body: &Value) -> Value {
    let target = body.get("path").and_then(|v| v.as_str()).and_then(checked);
    let dir = match dirs().into_iter().next() {
        Some((_, d)) => d,
        None => return json!({"ok": false, "error": "the game's Saved Games folder was not found"}),
    };
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    let mut cmd = std::process::Command::new(Path::new(&windir).join("explorer.exe"));
    match &target {
        Some(p) => cmd.arg(format!("/select,{}", p.display())),
        None => cmd.arg(&dir),
    };
    match cmd.spawn() {
        Ok(_) => json!({"ok": true}),
        Err(e) => json!({"ok": false, "error": e.to_string()}),
    }
}
