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

fn begin(detail: &str, total: u64) {
    let mut g = JOB.lock().unwrap();
    // a new job starts clean: no leftover warning or error from the last one
    g.remove("warning");
    g.remove("_error");
    g.remove("removed");
    for (k, v) in [("state", json!("running")), ("detail", json!(detail)), ("done", json!(0)), ("total", json!(total)), ("files", json!([]))] {
        g.insert(k.into(), v);
    }
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
        begin("adding to the game's track list", 0);
        let r2 = reg.clone();
        std::thread::spawn(move || {
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
    begin("starting", bytes);
    let n = need.len();
    std::thread::spawn(move || {
        contentsync::install_files(&need, &JOB, &reg);
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
    begin("restoring imported tracks", miss.len() as u64);
    let n = miss.len();
    std::thread::spawn(move || {
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
    begin("sending", 0);
    std::thread::spawn(move || {
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
    begin("packing the server build", plan.get("bytes").and_then(|v| v.as_u64()).unwrap_or(0));
    std::thread::spawn(move || {
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
    let (mut wrote, mut skipped) = (0usize, Vec::new());
    let r = crate::archive::each_file(&src, |name, _sz, rd| {
        let dest = contentsync::under(&root_abs.to_string_lossy(), &name.split(['/', '\\']).filter(|p| !p.is_empty()).collect::<Vec<_>>());
        let Ok(dest) = dest else { skipped.push(name.to_string()); return Ok(true) };
        if let Some(p) = dest.parent() {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        let mut out = std::fs::File::create(&dest).map_err(|e| e.to_string())?;
        std::io::copy(rd, &mut out).map_err(|e| e.to_string())?;
        wrote += 1;
        Ok(true)
    });
    install::drop_cleanup(did);
    if let Err(e) = r {
        logs::warn(format!("applying a server build: {e}"));
        return json!({"ok": false, "error": format!("could not unpack: {e}")});
    }
    if !skipped.is_empty() {
        logs::warn(format!("server build: refused {} path(s) outside the server folder: {:?}", skipped.len(), &skipped[..skipped.len().min(3)]));
    }
    logs::info(format!("applied a server build: {wrote} file(s) into {root}"));
    json!({"ok": true, "applied": wrote, "root": root, "refused": skipped.len()})
}

/// Whatever long content job is running, for the bar the whole app shows.
/// ⚠ Ingest FIRST: a pack downloads then unpacks, and the unpack is what
/// is moving.
pub fn progress() -> Value {
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
