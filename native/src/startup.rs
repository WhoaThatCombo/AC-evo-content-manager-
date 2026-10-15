//! What the app does once at start and keeps doing in the background -
//! ported from app.serve(). While the Python backend still runs behind the
//! shell it skips all of this (ACECM_NATIVE_URL), so nothing runs twice.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};

use crate::{config, logs, paths};

/// Forget what content we THINK exists. ⚠ Content is cached against the
/// archives it came from, which misses a mod folder emptied by hand or a
/// package restored with old timestamps; a restart is the natural moment to
/// look again. Expensive caches (file hashes, renders) are keyed per file
/// and left alone.
pub fn rescan_content() {
    let d = paths::data();
    let mut gone = 0;
    for f in [d.join("track_map.json"), d.join("viewer").join("index.json")] {
        if f.is_file() {
            match std::fs::remove_file(&f) {
                Ok(()) => gone += 1,
                Err(e) => logs::warn(format!("could not clear {}: {e}", f.display())),
            }
        }
    }
    if gone > 0 {
        logs::info(format!("content caches cleared ({gone}) - rescanning on demand"));
    }
}

/// Fill in address and port on entries written before autofill existed.
/// Entries carrying both are left alone, so this settles after one pass.
pub fn registry_backfill() -> Value {
    let mut items = crate::registry::load();
    if items.is_empty() {
        return json!({"ok": true, "fixed": 0});
    }
    let key = |e: &Value| (e.get("ip").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                           e.get("port").and_then(|v| v.as_i64()).unwrap_or(0));
    let mut fixed = 0;
    for e in items.iter_mut() {
        let before = key(e);
        if let Value::Object(m) = e {
            crate::registry::autofill(m);
        }
        if key(e) != before {
            fixed += 1;
        }
    }
    if fixed > 0 {
        let _ = crate::registry::save(&items);
        logs::info(format!("registry: filled in address/port on {fixed} entr{}", if fixed == 1 { "y" } else { "ies" }));
    }
    json!({"ok": true, "fixed": fixed})
}

/// Ask Windows Firewall to allow inbound TCP on the content port: friends
/// need it for Get content (admin routes refuse non-loopback peers, so the
/// hole is the share GETs only). Best-effort - a non-admin start just logs.
/// ⚠ profile=any: the people you share with arrive on whatever profile the
/// internet-facing adapter carries, and Windows files Ethernet as Public.
/// ⚠ REPLACE the rule, never skip because one exists: a stale rule scoped
/// to another profile could otherwise never be repaired.
fn allow_share_port(port: u16) {
    if !cfg!(windows) {
        // no single firewall to ask, and every route needs a privilege prompt
        logs::info(format!("share port {port}: if friends cannot fetch content, allow inbound TCP {port} in your firewall (firewalld: sudo firewall-cmd --add-port={port}/tcp)"));
        return;
    }
    let netsh = |args: &[String]| -> Option<std::process::Output> {
        let mut c = std::process::Command::new("netsh");
        c.args(args);
        crate::procs::hide(&mut c);
        c.output().ok()
    };
    let v = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    for name in [format!("ACECM-content-{port}"), format!("ACECM-share-{port}")] {
        let _ = netsh(&[v(&["advfirewall", "firewall", "delete", "rule"]), vec![format!("name={name}")]].concat());
    }
    let add = [v(&["advfirewall", "firewall", "add", "rule"]), vec![format!("name=ACECM-share-{port}")],
               v(&["dir=in", "action=allow", "protocol=TCP"]), vec![format!("localport={port}")], v(&["profile=any"])].concat();
    match netsh(&add) {
        Some(o) if o.status.success() => logs::info(format!("firewall: allowed inbound TCP {port} (all profiles)")),
        Some(o) => {
            let t = String::from_utf8_lossy(if o.stderr.is_empty() { &o.stdout } else { &o.stderr }).trim().chars().take(200).collect::<String>();
            logs::warn(format!("firewall: could not open TCP {port} (need admin?): {t}"));
        }
        None => logs::warn("firewall: netsh did not run"),
    }
}

/// Keep lobby.json describing the server that is actually running (the
/// backend re-reads that FILE for the entry players see). Only re-reads
/// when the set of server processes changes, so an idle machine costs
/// nothing.
fn watch_lobby() {
    std::thread::spawn(|| {
        let mut seen: Option<BTreeSet<u32>> = None;
        loop {
            let now: BTreeSet<u32> = crate::serverproc::server_pids().into_iter().collect();
            if seen.as_ref() != Some(&now) {
                seen = Some(now);
                let _ = crate::lobby::refresh(0);
            }
            std::thread::sleep(Duration::from_secs(5));
        }
    });
}

/// Keep the HUD showing mph while the setting is on. ⚠ Re-applied: the HUD
/// page is rebuilt every session, taking our element with it (the script is
/// idempotent). Only touches a game already running with the inspector open.
fn watch_hud() {
    std::thread::spawn(|| loop {
        let on = config::load().get("hud_mph").map(|v| !matches!(v, Value::Null | Value::Bool(false))).unwrap_or(false);
        if on && crate::gameui::listening() && crate::gameui::hud_page().is_some() {
            let _ = crate::gameui::set_mph(true);
        }
        std::thread::sleep(Duration::from_secs(5));
    });
}

/// Start the lobby proxy if Settings says so (replacing one that serves
/// another data folder's lobby).
fn autostart_proxy() {
    let on = config::load().get("auto_proxy").and_then(|v| v.as_bool()).unwrap_or(true);
    if !on {
        return;
    }
    std::thread::spawn(|| {
        let r = crate::drive::ensure_backend();
        if r["already"] == true {
            logs::info("auto proxy: already listening (ours)");
        } else if r["ok"] == true {
            logs::info(format!("auto proxy started on :{} pid={}", r["port"], r["pid"]));
        } else {
            logs::warn(format!("auto proxy failed: {}", r.get("error").and_then(|v| v.as_str()).map(String::from).unwrap_or(r.to_string())));
        }
    });
}

/// Everything app.serve() started, minus what the shell already did inline
/// (rollback flag, share sizes, lobby prefetch, bundle sweep, pit button).
pub fn run(port: u16) {
    rescan_content();
    // the third-party directory, so Drive's first open is not empty
    std::thread::spawn(|| { let _ = crate::evoreg::fetch(true, true); });
    std::thread::spawn(|| {
        let _ = registry_backfill();
        // drop leftover "(hosted here)" rows, then publish what the saved
        // profiles actually need
        let profs = crate::profiles::load();
        crate::autoshare::prune(&profs);
        for p in &profs {
            let _ = crate::autoshare::publish(p);
        }
    });
    watch_lobby();
    watch_hud();
    std::thread::spawn(move || allow_share_port(port));
    autostart_proxy();
}
