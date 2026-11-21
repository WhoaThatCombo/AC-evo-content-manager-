//! The one long CONTENT job the whole app shows a bar for - Get content,
//! re-registering imported tracks, pushing content or a server build to a
//! remote ACECM - and the progress view over it. Ported from app.py's
//! `_INSTALL` / `_install_content` / `_progress` and the push routes.
//!
//! ⚠ ONE job at a time across all of those: they share the state and the
//! "a content job is already running" guard.

use std::path::Path;
use std::sync::{LazyLock, Mutex};

use serde_json::{json, Map, Value};

use crate::{contentsync, install, logs, push};

pub static JOB: LazyLock<Mutex<Map<String, Value>>> = LazyLock::new(|| Mutex::new(match json!(
    {"state": "idle", "detail": "", "done": 0, "total": 0, "files": []}) { Value::Object(m) => m, _ => unreachable!() }));

fn update(kv: Value) {
    if let Value::Object(kv) = kv {
        let mut g = JOB.lock().unwrap();
        for (k, v) in kv {
            g.insert(k, v);
        }
    }
}

fn running() -> bool {
    JOB.lock().unwrap().get("state").and_then(|v| v.as_str()) == Some("running")
}

/// Claim the one job slot: false when a job is already running.
/// ⚠ Test-and-set under ONE lock: "is it running?" then "begin" let two
/// requests both start.
fn begin(detail: &str, total: u64) -> bool {
    let mut g = JOB.lock().unwrap();
    if g.get("state").and_then(|v| v.as_str()) == Some("running") {
        return false;
    }
    // a new job starts clean: no leftover warning or error from the last one
    g.remove("warning");
    g.remove("_error");
    g.remove("removed");
    for (k, v) in [("state", json!("running")), ("detail", json!(detail)), ("done", json!(0)), ("total", json!(total)), ("files", json!([]))] {
        g.insert(k.into(), v);
    }
    true
}

const BUSY: &str = "a content job is already running";

/// Run a job body on its own thread; a panic ends the job as an error
/// instead of leaving it "running" for ever (every later job refused).
fn spawn_job(f: impl FnOnce() + Send + 'static) {
    std::thread::spawn(move || {
        if let Err(p) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
            let msg = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "panic".into());
            logs::warn(format!("content job panicked: {msg}"));
            update(json!({"state": "error", "detail": format!("the job failed unexpectedly: {msg}")}));
        }
    });
}

fn sv(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(x) => x.to_string(),
    }
}

pub fn status() -> Value {
    let mut g = JOB.lock().unwrap().clone();
    g.remove("_error");
    Value::Object(g)
}

/// Download everything a server needs that this machine lacks. Runs in a
/// thread and reports through status(): a track is a gigabyte.
pub fn install_content(body: &Value) -> Value {
    if running() {
        return json!({"ok": false, "error": "an install is already running"});
    }
    let base = sv(body, "base");
    // ⚠ a host publishes one entry PER THING: take every id offered
    let ids: Vec<String> = match body.get("ids").and_then(|v| v.as_array()).filter(|a| !a.is_empty()) {
        Some(a) => a.iter().map(|x| match x { Value::String(s) => s.clone(), v => v.to_string() }).collect(),
        None => { let i = sv(body, "id"); if i.is_empty() { Vec::new() } else { vec![i] } }
    };
    let (mut need, mut seen, mut errors, mut stale, mut reg) = (Vec::new(), std::collections::HashSet::new(), Vec::new(), Vec::<String>::new(), Vec::<String>::new());
    let mut acemods: Vec<Value> = Vec::new();
    for sid in &ids {
        let p = contentsync::plan(&base, sid);
        if p.get("ok") != Some(&json!(true)) {
            errors.push(format!("{sid}: {}", sv(&p, "error")));
            continue;
        }
        for e in p["need"].as_array().cloned().unwrap_or_default() {
            if seen.insert(sv(&e, "path")) {
                need.push(e);
            }
        }
        for a in p["acemods"].as_array().cloned().unwrap_or_default() {
            if !acemods.iter().any(|x| x["kind"] == a["kind"] && x["name"] == a["name"]) {
                acemods.push(a);
            }
        }
        for f in p["stale"].as_array().cloned().unwrap_or_default() {
            let f = f.as_str().unwrap_or("").to_string();
            if !stale.contains(&f) { stale.push(f); }
        }
        for f in p["needs_register"].as_array().cloned().unwrap_or_default() {
            let f = f.as_str().unwrap_or("").to_string();
            if !reg.contains(&f) { reg.push(f); }
        }
    }
    // ⚠ files here but the track not in the game's list is NOT "you already
    // have everything" - only the tables need writing
    if need.is_empty() && !reg.is_empty() {
        if !begin("adding to the game's track list", 0) {
            return json!({"ok": false, "error": BUSY});
        }
        let r2 = reg.clone();
        spawn_job(move || {
            contentsync::install_files(&[], &JOB, &r2);
            install::after_content_change();
            let warn = sv(&status(), "warning");
            update(json!({"state": if warn.is_empty() { "done" } else { "error" },
                          "detail": if warn.is_empty() { format!("{} track(s) added to the game", r2.len()) } else { warn }}));
        });
        return json!({"ok": true, "files": 0, "bytes": 0, "registering": reg});
    }
    if need.is_empty() && !stale.is_empty() {
        let got = contentsync::remove_stale(&stale);
        return json!({"ok": true, "files": 0, "bytes": 0, "removed": got["removed"].as_array().map(|a| a.len()).unwrap_or(0)});
    }
    if need.is_empty() {
        return json!({"ok": false, "error": if errors.is_empty() { "you already have everything this host offers".to_string() } else { errors.join("; ") }});
    }
    let bytes: u64 = need.iter().map(|e| e.get("size").and_then(|v| v.as_u64()).unwrap_or(0)).sum();
    let targets: Vec<(std::path::PathBuf, u64)> = need.iter().filter_map(|e| {
        let d = sv(e, "dest");
        let p = if d.is_empty() { contentsync::destination(&sv(e, "path")).ok()? } else { std::path::PathBuf::from(d) };
        Some((p, e.get("size").and_then(|v| v.as_u64()).unwrap_or(0)))
    }).collect();
    if let Some(why) = install::room_for_all(&targets) {
        return json!({"ok": false, "error": why});
    }
    if !begin("starting", bytes) {
        return json!({"ok": false, "error": BUSY});
    }
    let n = need.len();
    spawn_job(move || {
        contentsync::install_files_with(&need, &acemods, &JOB, &reg);
        let err = JOB.lock().unwrap().get("_error").and_then(|e| e.as_str()).map(String::from);
        if let Some(e) = err {
            logs::warn(format!("content install failed: {e}"));
            update(json!({"state": "error", "detail": e}));
            return;
        }
        // ⚠ AFTER the downloads: a failed install must not leave files deleted
        if !stale.is_empty() {
            let dropped = contentsync::remove_stale(&stale);
            let k = dropped["removed"].as_array().map(|a| a.len()).unwrap_or(0);
            if k > 0 {
                update(json!({"removed": k}));
            }
        }
        // ⚠ THE hook that makes downloaded content show up without a restart
        install::after_content_change();
        let extra = sv(&status(), "warning");
        update(json!({"state": "done", "detail": format!("{n} file(s) installed{}", if extra.is_empty() { String::new() } else { format!(" — {extra}") })}));
    });
    json!({"ok": true, "files": n, "bytes": bytes})
}

/// Re-register every imported track the client table has forgotten.
pub fn tracks_restore() -> Value {
    if running() {
        return json!({"ok": false, "error": "a content job is already running"});
    }
    let miss = crate::trackdeploy::missing_from_client();
    if miss.is_empty() {
        return json!({"ok": true, "count": 0, "note": "nothing to restore"});
    }
    if crate::procs::game_running() {
        // same answer the job would reach, without starting it
    }
    if !begin("restoring imported tracks", miss.len() as u64) {
        return json!({"ok": false, "error": BUSY});
    }
    let n = miss.len();
    spawn_job(move || {
        if crate::procs::game_running() {
            update(json!({"state": "error", "detail": "close the game first - restoring tracks writes content.kspkg, which is locked while it runs"}));
            return;
        }
        let (mut done, mut failed) = (Vec::new(), Vec::new());
        for (i, folder) in miss.iter().enumerate() {
            update(json!({"done": i, "total": n, "detail": format!("restoring {folder} ({}/{n})", i + 1)}));
            let r = crate::trackdeploy::register_client_track(folder, &json!({}));
            if r.get("ok") == Some(&json!(true)) { done.push(folder.clone()) } else { failed.push(json!({"folder": folder, "error": r.get("error")})) }
        }
        let _ = crate::trackimport::track_map(true);
        install::after_content_change();
        if failed.is_empty() {
            update(json!({"state": "done", "detail": format!("restored {} track(s)", done.len())}));
        } else {
            let s: String = failed.iter().map(|f| f.to_string()).collect::<Vec<_>>().join("; ").chars().take(200).collect();
            update(json!({"state": "error", "detail": if s.is_empty() { "restore failed".to_string() } else { s }}));
        }
    });
    json!({"ok": true, "started": true, "count": n})
}

fn tick_job() -> push::Tick {
    std::sync::Arc::new(|sent: u64, total: u64| update(json!({"done": sent, "total": total})))
}

/// Upload what a profile needs to a remote ACECM, in a thread.
pub fn push_send(body: &Value) -> Value {
    let id = sv(body, "id");
    let Some(prof) = crate::profiles::load().into_iter().find(|p| sv(p, "id") == id) else {
        return json!({"ok": false, "error": "no such profile"});
    };
    if running() {
        return json!({"ok": false, "error": "a content job is already running"});
    }
    let (base, tok) = (sv(body, "base"), sv(body, "token"));
    if !begin("sending", 0) {
        return json!({"ok": false, "error": BUSY});
    }
    spawn_job(move || {
        let r = push::send(&prof, &base, &tok, true, Some(tick_job()));
        if r.get("ok") == Some(&json!(true)) {
            let sent: Vec<String> = r["sent"].as_array().cloned().unwrap_or_default().iter().filter_map(|x| x.as_str().map(String::from)).collect();
            update(json!({"state": "done", "detail": format!("sent {}", sent.join(", "))}));
        } else {
            let e = sv(&r, "error");
            update(json!({"state": "error", "detail": if e.is_empty() { "failed".to_string() } else { e }}));
        }
    });
    json!({"ok": true, "started": true})
}

pub fn push_plan(body: &Value) -> Value {
    let id = sv(body, "id");
    let Some(prof) = crate::profiles::load().into_iter().find(|p| sv(p, "id") == id) else {
        return json!({"ok": false, "error": "no such profile"});
    };
    let (mods_, tracks, missing) = push::plan(&prof);
    json!({"ok": true, "mods": mods_.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(), "tracks": tracks, "missing": missing})
}

/// Ship this machine's server build to a remote ACECM, in a thread.
pub fn push_server_build(body: &Value) -> Value {
    if running() {
        return json!({"ok": false, "error": "a content job is already running"});
    }
    let (base, tok) = (sv(body, "base"), sv(body, "token"));
    let plan = push::server_build_plan();
    if !begin("packing the server build", plan.get("bytes").and_then(|v| v.as_u64()).unwrap_or(0)) {
        return json!({"ok": false, "error": BUSY});
    }
    spawn_job(move || {
        let tick: push::Tick = std::sync::Arc::new(|sent: u64, total: u64| {
            update(json!({"done": sent, "total": total,
                          "detail": if sent * 2 < total { "packing the server build" } else { "sending the server build" }}));
        });
        let r = push::send_server_build(&base, &tok, Some(tick));
        if r.get("ok") == Some(&json!(true)) {
            update(json!({"state": "done", "detail": format!("server build applied ({} file(s))", r.get("applied").map(|v| v.to_string()).unwrap_or("None".into()))}));
        } else {
            let e = sv(&r, "error");
            update(json!({"state": "error", "detail": if e.is_empty() { "failed".to_string() } else { e }}));
        }
    });
    let mut out = json!({"ok": true, "started": true});
    if let (Some(o), Some(p)) = (out.as_object_mut(), plan.as_object()) {
        for (k, v) in p {
            o.insert(k.clone(), v.clone());
        }
    }
    out
}

/// Unpack an uploaded dedicated-server build over this machine's copy.
/// ⚠ Refused while a server runs; every entry must land inside the folder.
pub fn apply_server_build(did: &str) -> Value {
    let root = crate::detect::effective_server_dir();
    if root.is_empty() || !Path::new(&root).is_dir() {
        return json!({"ok": false, "error": "no dedicated server folder on this machine to update"});
    }
    if let Some(pid) = crate::serverproc::server_pids().into_iter().next() {
        return json!({"ok": false, "error": format!("a dedicated server is running (pid {pid}) - stop it before updating the build")});
    }
    let Some(d) = install::drop_staging(did, false).filter(|d| d.is_dir()) else {
        return json!({"ok": false, "error": "nothing was uploaded"});
    };
    let zips: Vec<_> = std::fs::read_dir(&d).into_iter().flatten().flatten()
        .filter(|e| e.file_name().to_string_lossy().to_lowercase().ends_with(".zip")).map(|e| e.path()).collect();
    let Some(src) = zips.into_iter().next() else { return json!({"ok": false, "error": "the upload was not a build archive"}) };
    if let Some(bad) = install::archive_budget(&src, Path::new(&root)) {
        install::drop_cleanup(did);
        return bad;
    }
    let root_abs = std::path::absolute(&root).unwrap_or_else(|_| root.clone().into());
    // ⚠ unpack BESIDE the server folder (same volume), then move the files
    // in: unpacking in place left a half-updated build on any error
    let staging = match staging_beside(&root_abs) {
        Ok(s) => s,
        Err(e) => { install::drop_cleanup(did); return json!({"ok": false, "error": e}); }
    };
    let (mut wrote, mut skipped) = (0usize, Vec::new());
    let r = crate::archive::each_file(&src, |name, _sz, rd| {
        let dest = contentsync::under(&staging.to_string_lossy(), &name.split(['/', '\\']).filter(|p| !p.is_empty()).collect::<Vec<_>>());
        let Ok(dest) = dest else { skipped.push(name.to_string()); return Ok(true) };
        if let Some(p) = dest.parent() {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        let mut out = std::fs::File::create(&dest).map_err(|e| e.to_string())?;
        std::io::copy(rd, &mut out).map_err(|e| e.to_string())?;
        out.sync_all().map_err(|e| e.to_string())?;
        wrote += 1;
        Ok(true)
    });
    install::drop_cleanup(did);
    if let Err(e) = r {
        let _ = std::fs::remove_dir_all(&staging);
        logs::warn(format!("applying a server build: {e}"));
        return json!({"ok": false, "error": format!("could not unpack: {e} - the server folder was not changed")});
    }
    let moved = move_tree_in(&staging, &root_abs);
    let _ = std::fs::remove_dir_all(&staging);
    if let Err(e) = moved {
        logs::warn(format!("applying a server build: {e}"));
        return json!({"ok": false, "error": e});
    }
    if !skipped.is_empty() {
        logs::warn(format!("server build: refused {} path(s) outside the server folder: {:?}", skipped.len(), &skipped[..skipped.len().min(3)]));
    }
    logs::info(format!("applied a server build: {wrote} file(s) into {root}"));
    json!({"ok": true, "applied": wrote, "root": root, "refused": skipped.len()})
}

/// An empty folder next to `root` (same volume: the final moves are renames).
fn staging_beside(root: &Path) -> Result<std::path::PathBuf, String> {
    let mut b = [0u8; 4];
    let _ = getrandom::fill(&mut b);
    let tag: String = b.iter().map(|x| format!("{x:02x}")).collect();
    let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "server".into());
    let s = root.with_file_name(format!("{name}.acecm-build-{tag}"));
    std::fs::create_dir_all(&s).map_err(|e| format!("could not make a staging folder beside the server: {e}"))?;
    Ok(s)
}

fn files_under(dir: &Path, rel: &Path, out: &mut Vec<std::path::PathBuf>) -> std::io::Result<()> {
    for e in std::fs::read_dir(dir.join(rel))? {
        let e = e?;
        let r = rel.join(e.file_name());
        if e.file_type()?.is_dir() { files_under(dir, &r, out)? } else { out.push(r) }
    }
    Ok(())
}

/// Move every file of `staging` to the same place under `root`, all or
/// nothing: each replaced file is kept as <name>.acecm-prev until every
/// move worked, and put back if one fails.
fn move_tree_in(staging: &Path, root: &Path) -> Result<(), String> {
    let mut files = Vec::new();
    files_under(staging, Path::new(""), &mut files).map_err(|e| format!("could not read the unpacked build: {e}"))?;
    let mut done: Vec<(std::path::PathBuf, Option<std::path::PathBuf>)> = Vec::new();
    let mut fail = None;
    for rel in &files {
        let dest = root.join(rel);
        let prev = dest.with_file_name(format!("{}.acecm-prev", dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
        let r = (|| -> std::io::Result<Option<std::path::PathBuf>> {
            if let Some(p) = dest.parent() {
                std::fs::create_dir_all(p)?;
            }
            let had = dest.exists();
            if had {
                let _ = std::fs::remove_file(&prev);
                std::fs::rename(&dest, &prev)?;
            }
            if let Err(e) = std::fs::rename(staging.join(rel), &dest) {
                if had {
                    let _ = std::fs::rename(&prev, &dest);
                }
                return Err(e);
            }
            Ok(had.then(|| prev.clone()))
        })();
        match r {
            Ok(p) => done.push((dest, p)),
            Err(e) => { fail = Some(format!("could not put {} in place ({e}) - the previous build was put back", rel.display())); break; }
        }
    }
    if let Some(e) = fail {
        for (dest, prev) in done.into_iter().rev() {
            let _ = std::fs::remove_file(&dest);
            if let Some(p) = prev {
                let _ = std::fs::rename(&p, &dest);
            }
        }
        return Err(e);
    }
    for (_, prev) in done {
        if let Some(p) = prev {
            let _ = std::fs::remove_file(p);
        }
    }
    Ok(())
}

/// Whatever long content job is running, for the bar the whole app shows.
/// ⚠ Ingest FIRST: a pack downloads then unpacks, and the unpack is what
/// is moving.
pub fn progress() -> Value {
    if let Some(what) = crate::acemod::export_progress() {
        return json!({"ok": true, "active": true, "phase": "export", "what": what, "done": 0, "total": 0});
    }
    let ing = install::ingest_state();
    if ing.get("state").and_then(|v| v.as_str()) == Some("running") {
        let what = sv(&Value::Object(ing.clone()), "detail");
        return json!({"ok": true, "active": true, "phase": "install", "what": if what.is_empty() { "Installing".to_string() } else { what },
                      "done": ing.get("done").cloned().unwrap_or(json!(0)), "total": ing.get("total").cloned().unwrap_or(json!(0))});
    }
    let job = status();
    if sv(&job, "state") == "running" {
        let what = sv(&job, "detail");
        return json!({"ok": true, "active": true, "phase": "download", "what": if what.is_empty() { "Downloading".to_string() } else { what },
                      "done": job.get("done").cloned().unwrap_or(json!(0)), "total": job.get("total").cloned().unwrap_or(json!(0))});
    }
    // a fetch from a directory server's share: same bar
    let ev = crate::evoshare::status();
    let want = ev.get("want").and_then(|v| v.as_u64()).unwrap_or(0);
    if ev.get("active") == Some(&json!(true)) && want > 0 {
        let file = { let f = sv(&ev, "file"); if f.is_empty() { "content".to_string() } else { f } };
        let server = { let s = sv(&ev, "server"); if s.is_empty() { "the host".to_string() } else { s } };
        return json!({"ok": true, "active": true, "phase": "download", "what": format!("Downloading {file} from {server}"),
                      "done": ev.get("bytes").cloned().unwrap_or(json!(0)), "total": want});
    }
    let st = { let s = sv(&job, "state"); if s.is_empty() { "idle".to_string() } else { s } };
    json!({"ok": true, "active": false, "state": st, "what": sv(&job, "detail")})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_build_moves_in_whole() {
        let base = crate::safefs::test_dir().join(format!("jobs_build_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("server");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("a.exe"), b"old a").unwrap();
        std::fs::write(root.join("keep.txt"), b"mine").unwrap();
        let st = staging_beside(&root).unwrap();
        std::fs::create_dir_all(st.join("sub")).unwrap();
        std::fs::write(st.join("a.exe"), b"new a").unwrap();
        std::fs::write(st.join("sub").join("b.bin"), b"new b").unwrap();
        move_tree_in(&st, &root).unwrap();
        assert_eq!(std::fs::read(root.join("a.exe")).unwrap(), b"new a");
        assert_eq!(std::fs::read(root.join("sub").join("b.bin")).unwrap(), b"new b");
        assert_eq!(std::fs::read(root.join("keep.txt")).unwrap(), b"mine");
        assert!(!root.join("a.exe.acecm-prev").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn one_job_at_a_time() {
        assert!(begin("first", 0));
        assert!(!begin("second", 0));
        update(json!({"state": "done"}));
        assert!(begin("third", 0));
        update(json!({"state": "idle"}));
    }
}
