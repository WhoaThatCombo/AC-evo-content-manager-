//! Sharing follows the server configuration - ported from acecm/autoshare.py.
//! What a server needs that is modded (an imported track, car mods) is
//! published in the share list, and anything the SERVER is missing is
//! copied over from the client side at start.
//! ⚠ The client and the dedicated server keep car mods in DIFFERENT
//! folders: a car you can drive is not a car the server can host.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use serde_json::{json, Value};

use crate::{mods, profiles, registry, trackdeploy, trackimport};

const AUTO_NOTE: &str = "shared automatically because a server here needs it";

fn sv(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

fn mod_by_car() -> HashMap<String, String> {
    let mut out = HashMap::new();
    for side in ["client", "server"] {
        for m in mods::installed(side).get("mods").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
            for c in m.get("cars").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
                if let Some(id) = c.get("id").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
                    out.insert(id.to_string(), sv(&m, "name"));
                }
            }
        }
    }
    out
}

fn imported_tracks() -> HashSet<String> {
    trackimport::installed_tracks().into_iter().collect()
}

/// The FOLDER a profile's custom track lives in (custom_track holds what a
/// player sees, "Highlands Drift", not highlands_kp).
pub fn track_folder(profile: &Value) -> String {
    let name = sv(profile, "custom_track").trim().to_string();
    if name.is_empty() {
        return String::new();
    }
    if imported_tracks().contains(&name) {
        return name;
    }
    let table = trackimport::track_map(false);
    if let Some(h) = table.get(&name).and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
        return h.to_string();
    }
    let low = name.to_lowercase();
    table.iter().find(|(k, _)| k.to_lowercase() == low)
        .and_then(|(_, v)| v.as_str()).unwrap_or("").to_string()
}

/// What is modded about this server (stock content excluded on purpose).
pub fn needs(profile: &Value) -> Value {
    let imported = imported_tracks();
    let by_car = mod_by_car();
    let mut tracks = vec![];
    let f = track_folder(profile);
    if !f.is_empty() && imported.contains(&f) {
        tracks.push(f);
    }
    let mut mods_ = BTreeSet::new();
    for car in profile.get("cars").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
        if let Some(n) = car.as_str().and_then(|c| by_car.get(c)) {
            mods_.insert(n.clone());
        }
    }
    json!({"tracks": tracks, "mods": mods_.into_iter().collect::<Vec<_>>()})
}

/// What we actually advertise; a host can unshare the hosted track.
pub fn share_needs(profile: &Value) -> Value {
    let mut want = needs(profile);
    let skip: HashSet<String> = profile.get("unshared_tracks").and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|t| t.as_str().map(String::from)).collect()).unwrap_or_default();
    let t: Vec<Value> = want["tracks"].as_array().cloned().unwrap_or_default().into_iter()
        .filter(|t| !skip.contains(t.as_str().unwrap_or(""))).collect();
    want["tracks"] = json!(t);
    want
}

fn auto_entry(profile: &Value) -> Option<Value> {
    registry::load().into_iter().find(|e| e.get("auto") == Some(&json!(true))
        && e.get("profile_id") == profile.get("id"))
}

/// Make the share list match this profile.
/// Share or unshare one track, including one a server is hosting.
/// Unsharing a hosted track is remembered on the profile
/// (`unshared_tracks`) so the next save/start does not publish it again.
pub fn set_track(folder: &str, share: bool, label: &str) -> Value {
    let folder = folder.trim();
    if folder.is_empty() {
        return json!({"ok": false, "error": "no track"});
    }
    let mut items = crate::profiles::load();
    for p in items.iter_mut() {
        let skip: Vec<String> = p.get("unshared_tracks").and_then(|v| v.as_array()).map(|a| a.iter()
            .filter_map(|t| t.as_str().filter(|s| !s.is_empty()).map(String::from)).collect()).unwrap_or_default();
        let uses = track_folder(p) == folder;
        if share {
            p["unshared_tracks"] = json!(skip.iter().filter(|t| t.as_str() != folder).collect::<Vec<_>>());
        } else if uses && !skip.iter().any(|t| t == folder) {
            let mut s = skip.clone();
            s.push(folder.to_string());
            p["unshared_tracks"] = json!(s);
        }
    }
    let _ = crate::profiles::save_all(&items);
    for p in &items {
        publish(p);
    }
    if share {
        let already = registry::load().iter().any(|e| e.get("required_tracks").and_then(|v| v.as_array())
            .map(|a| a.iter().any(|t| t.as_str() == Some(folder))).unwrap_or(false));
        if !already {
            let l = if label.is_empty() { folder } else { label };
            registry::upsert(&json!({"name": format!("{l} (track)"), "description": format!("Track {folder} shared by this host"),
                                     "required_tracks": [folder], "public": true}));
        }
    } else {
        let mut kept = Vec::new();
        for e in registry::load() {
            let tracks: Vec<Value> = e.get("required_tracks").and_then(|v| v.as_array()).cloned().unwrap_or_default()
                .into_iter().filter(|t| t.as_str() != Some(folder)).collect();
            let mods = e.get("required_mods").and_then(|v| v.as_array()).map(|a| !a.is_empty()).unwrap_or(false);
            if !tracks.is_empty() || mods {
                let mut e2 = e.clone();
                e2["required_tracks"] = json!(tracks);
                kept.push(e2);
            }
        }
        let _ = registry::save(&kept);
    }
    registry::forget_public_sizes();
    json!({"ok": true, "shared": share, "folder": folder})
}

pub fn publish(profile: &Value) -> Value {
    let want = share_needs(profile);
    let existing = auto_entry(profile);
    let empty = want["tracks"].as_array().map(|a| a.is_empty()).unwrap_or(true)
        && want["mods"].as_array().map(|a| a.is_empty()).unwrap_or(true);
    let mut out = json!({"ok": true});
    if empty {
        out["shared"] = json!(false);
        if let Some(e) = existing {
            registry::remove(&sv(&e, "id"));
            out["removed"] = json!(true);
        }
    } else {
        let mut entry = json!({
            "name": format!("{} (content)", { let n = sv(profile, "name"); if n.is_empty() { "server".into() } else { n } }),
            "description": AUTO_NOTE, "profile_id": profile.get("id").cloned().unwrap_or(Value::Null),
            "auto": true, "public": true,
            "port": profile.get("tcp_port").filter(|v| v.as_i64().map(|n| n != 0).unwrap_or(false)).cloned().unwrap_or(json!(9700)),
            "required_tracks": want["tracks"], "required_mods": want["mods"],
        });
        if let Some(id) = existing.as_ref().map(|e| sv(e, "id")).filter(|s| !s.is_empty()) {
            entry["id"] = json!(id);
        }
        let saved = registry::upsert(&entry);
        out["shared"] = json!(true);
        out["id"] = saved.get("id").cloned().unwrap_or(Value::Null);
    }
    if let (Some(o), Value::Object(w)) = (out.as_object_mut(), want) {
        o.extend(w);
    }
    out
}

/// Drop automatic entries whose profile is gone, and the old
/// "(hosted here)" rows track deploy used to create for every import.
pub fn prune(profiles_: &[Value]) -> Vec<String> {
    let alive: HashSet<String> = profiles_.iter().map(|p| sv(p, "id")).collect();
    let (mut drop, mut keep) = (vec![], vec![]);
    for e in registry::load() {
        let leftover = sv(&e, "name").ends_with("(hosted here)") && e.get("auto") != Some(&json!(true))
            && sv(&e, "profile_id").trim().is_empty();
        let stale = e.get("auto") == Some(&json!(true)) && !alive.contains(&sv(&e, "profile_id"));
        if leftover || stale { drop.push(e) } else { keep.push(e) }
    }
    if !drop.is_empty() {
        let _ = registry::save(&keep);
        registry::forget_public_sizes();
    }
    drop.iter().map(|e| sv(e, "id")).collect()
}

// ⚠ cached on the archive's size+mtime: the index is 64 MiB at the end of a
// big file, so asking once per profile per render would be pointless work
static IN_PKG: Mutex<Option<((u64, u64), HashSet<String>)>> = Mutex::new(None);

/// Is this track already inside the server's own archive?
pub fn in_server_package(folder: &str) -> bool {
    let pkg = trackdeploy::server_kspkg();
    if folder.is_empty() || !pkg.is_file() {
        return false;
    }
    let Ok(m) = std::fs::metadata(&pkg) else { return false };
    let stamp = (m.len(), m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0));
    let mut g = IN_PKG.lock().unwrap();
    if g.as_ref().map(|(s, _)| *s != stamp).unwrap_or(true) {
        let Ok(es) = crate::kspkg::entries(&pkg) else { return false };
        let mut found = HashSet::new();
        for e in es {
            let low = e.path.to_lowercase().replace('/', "\\");
            if let Some(rest) = low.strip_prefix("content\\tracks\\") {
                if let Some((f, _)) = rest.split_once('\\') {
                    found.insert(f.to_string());
                }
            }
        }
        *g = Some((stamp, found));
    }
    g.as_ref().map(|(_, f)| f.contains(&folder.to_lowercase())).unwrap_or(false)
}

/// What this server is configured to use but does not actually have.
pub fn server_gaps(profile: &Value) -> Value {
    let want = needs(profile);
    let have: HashSet<String> = mods::installed("server").get("mods").and_then(|v| v.as_array())
        .map(|a| a.iter().map(|m| sv(m, "name")).collect()).unwrap_or_default();
    let missing: Vec<Value> = want["mods"].as_array().cloned().unwrap_or_default().into_iter()
        .filter(|m| !have.contains(m.as_str().unwrap_or(""))).collect();
    let f = track_folder(profile);
    let track = if !f.is_empty() && !in_server_package(&f) { f } else { String::new() };
    let ok = missing.is_empty() && track.is_empty();
    json!({"mods": missing, "track": track, "ok": ok})
}

/// Put an imported track into the SERVER's archive (done at start - it
/// rewrites the 64 MiB index). Skipped when it is already there.
pub fn deploy_track(folder: &str) -> Value {
    if folder.is_empty() {
        return json!({"ok": true, "skipped": "no custom track"});
    }
    if in_server_package(folder) {
        return json!({"ok": true, "skipped": "already in the package"});
    }
    let hit = trackimport::importable().into_iter().find(|x| sv(x, "folder") == folder);
    match hit.map(|h| sv(&h, "path")).filter(|p| !p.is_empty()) {
        Some(p) => trackdeploy::deploy_native(Path::new(&p), false, None, None),
        None => json!({"ok": false, "error": format!("no imported files found for {folder}")}),
    }
}

/// Copy anything the server is missing from the client side.
pub fn fill_server(profile: &Value) -> Value {
    let gaps = server_gaps(profile);
    let (src_dir, dst_dir) = (mods::client_mods_dir(), mods::mods_dir());
    let _ = std::fs::create_dir_all(&dst_dir);
    let (mut copied, mut failed) = (vec![], vec![]);
    for name in gaps["mods"].as_array().cloned().unwrap_or_default() {
        let name = name.as_str().unwrap_or("").to_string();
        let ok = [".kspkg", ".json"].iter().all(|ext| {
            let src = Path::new(&src_dir).join(format!("{name}{ext}"));
            src.is_file() && std::fs::copy(&src, Path::new(&dst_dir).join(format!("{name}{ext}"))).is_ok()
        });
        if ok { copied.push(name) } else { failed.push(name) }
    }
    let t = sv(&gaps, "track");
    let track = if t.is_empty() { json!({}) } else { deploy_track(&t) };
    let track_ok = track.get("ok").and_then(|v| v.as_bool()).unwrap_or(true);
    json!({"ok": failed.is_empty() && track_ok, "copied": copied, "failed": failed,
           "track": t, "track_deploy": track, "track_missing": if track_ok { String::new() } else { t.clone() }})
}

/// Fill the server, then publish what it needs.
pub fn sync(profile: &Value, fill: bool) -> Value {
    let filled = if fill { fill_server(profile) } else { json!({}) };
    let shared = publish(profile);
    json!({"ok": shared.get("ok").and_then(|v| v.as_bool()).unwrap_or(false), "filled": filled, "shared": shared})
}

/// Every profile's needs and gaps, for /api/share/auto.
pub fn overview() -> Value {
    let out: Vec<Value> = profiles::load().iter().map(|p| json!({
        "id": p.get("id").cloned().unwrap_or(Value::Null),
        "name": p.get("name").cloned().unwrap_or(Value::Null),
        "needs": share_needs(p), "gaps": server_gaps(p),
    })).collect();
    json!({"ok": true, "servers": out})
}
