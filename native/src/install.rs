//! Installing and removing content - ported from install.py.
//!
//! CARS are a .kspkg + .json pair and go to BOTH sides (the dedicated
//! server's mods folder and the client's): ⚠ the .json is what makes a car
//! selectable, a mod without one installs "fine" and never appears.
//! TRACKS are a folder under the client's mods/content/tracks plus rows in
//! the client's tracks.table (trackdeploy.rs).

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Map, Value};

use crate::contentsync::{self, under};
use crate::{archive, logs, mods, paths, registry};

fn sv(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(x) => x.to_string(),
    }
}

fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

fn size(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

fn show(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// splitext: ("name", ".ext") with Python's rules (a leading dot is not an ext)
fn splitext(f: &str) -> (String, String) {
    let base_start = f.rfind(['/', '\\']).map(|i| i + 1).unwrap_or(0);
    let name = &f[base_start..];
    let trimmed = name.trim_start_matches('.');
    let lead = name.len() - trimmed.len();
    match trimmed.rfind('.') {
        Some(i) => {
            let i = base_start + lead + i;
            (f[..i].to_string(), f[i..].to_string())
        }
        None => (f.to_string(), String::new()),
    }
}

fn basename(n: &str) -> String {
    n.rsplit(['/', '\\']).next().unwrap_or("").to_string()
}

pub fn mods_dir(create: bool) -> String {
    let d = mods::mods_dir();
    if create && !d.is_empty() {
        let _ = std::fs::create_dir_all(&d);
    }
    d
}

fn client_mods_dir() -> String {
    mods::client_mods_dir()
}

// -------------------------------------------------------------- the feed --
/// What the long half of an install is doing right now (the drop request
/// can run for minutes; the page polls this).
pub static INGEST: Mutex<Option<Map<String, Value>>> = Mutex::new(None);

fn ingest_set(f: impl FnOnce(&mut Map<String, Value>)) {
    let mut g = INGEST.lock().unwrap();
    let m = g.get_or_insert_with(|| match json!({"state": "idle", "detail": "", "done": 0, "total": 0}) { Value::Object(m) => m, _ => unreachable!() });
    f(m);
}

pub fn ingest_state() -> Map<String, Value> {
    let mut out = Map::new();
    ingest_set(|m| out = m.clone());
    out
}

pub fn ingest_begin(what: &str, total: u64) {
    ingest_set(|m| {
        m.insert("state".into(), json!("running"));
        m.insert("detail".into(), json!(what));
        m.insert("done".into(), json!(0));
        m.insert("total".into(), json!(total));
    });
}

pub fn ingest_step(n: u64, name: &str) {
    ingest_set(|m| {
        let d = m.get("done").and_then(|v| v.as_u64()).unwrap_or(0);
        m.insert("done".into(), json!(d + n));
        if !name.is_empty() {
            m.insert("detail".into(), json!(name));
        }
    });
}

pub fn ingest_end(detail: &str) {
    ingest_set(|m| {
        m.insert("state".into(), json!("idle"));
        m.insert("detail".into(), json!(detail));
    });
}

/// THE "content changed" hook.
pub fn after_content_change() {
    crate::content::forget_lists();
    registry::forget_public_sizes();
}

// ------------------------------------------------------------------ cars --
/// Perform the copy fix_hint describes.
pub fn apply_fix(name: &str) -> Value {
    let a = mods::audit();
    let row = a.get("mods").and_then(|m| m.as_array()).and_then(|rows| rows.iter().find(|r| sv(r, "name") == name).cloned());
    let Some(fix) = row.as_ref().and_then(|r| r.get("fix")).filter(|f| f.is_object()).cloned() else {
        return json!({"ok": false, "error": format!("nothing automatically fixable for {name}")});
    };
    let (src, dst) = (sv(&fix, "src"), sv(&fix, "dst"));
    if !Path::new(&src).is_file() {
        return json!({"ok": false, "error": format!("source file missing: {src}")});
    }
    if let Some(p) = Path::new(&dst).parent() {
        let _ = std::fs::create_dir_all(p);
    }
    if let Err(e) = copy2(Path::new(&src), Path::new(&dst)) {
        return json!({"ok": false, "error": e.to_string()});
    }
    json!({"ok": true, "copied": dst, "what": fix.get("what")})
}

/// shutil.copy2: bytes plus the modification time
fn copy2(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::copy(src, dst)?;
    if let Ok(t) = std::fs::metadata(src).and_then(|m| m.modified()) {
        if let Ok(f) = std::fs::OpenOptions::new().write(true).open(dst) {
            let _ = f.set_modified(t);
        }
    }
    Ok(())
}

fn copy_file(src: &Path, dst: &Path, force: bool) -> std::io::Result<&'static str> {
    if !src.is_file() {
        return Ok("missing");
    }
    if let Some(p) = dst.parent() {
        std::fs::create_dir_all(p)?;
    }
    if dst.is_file() && !force {
        return Ok(if size(src) == size(dst) { "same" } else { "differs" });
    }
    copy2(src, dst)?;
    Ok("copied")
}

fn refresh_lobby() -> Value {
    let items = crate::profiles::load();
    let current = crate::lobby::read();
    let sid = sv(&current, "server_id");
    let chosen = if sid.is_empty() { None } else { items.iter().find(|p| sv(p, "id") == sid).cloned() };
    let chosen = chosen.or_else(|| items.first().cloned());
    crate::lobby::write(&chosen.unwrap_or(json!({})))
}

/// Make client and server mod folders agree.
pub fn sync(direction: &str, force: bool, names: Option<Vec<String>>) -> Value {
    let direction = direction.trim().to_lowercase();
    let direction = if direction.is_empty() { "to_server".to_string() } else { direction };
    if !["to_server", "to_client", "both"].contains(&direction.as_str()) {
        return json!({"ok": false, "error": format!("unknown direction {direction}")});
    }
    let srv = mods_dir(true);
    let cli = client_mods_dir();
    if cli.is_empty() {
        return json!({"ok": false, "error": "client mods folder not found"});
    }
    let _ = std::fs::create_dir_all(&cli);
    let au = mods::audit();
    let wanted = names.unwrap_or_else(|| au.get("mods").and_then(|m| m.as_array()).cloned().unwrap_or_default().iter().map(|r| sv(r, "name")).collect());
    let (mut copied, mut skipped, mut errors) = (Vec::new(), Vec::new(), Vec::new());
    let mut one = |name: &str, src_dir: &str, dst_dir: &str, label: &str| {
        for ext in [".kspkg", ".json"] {
            let src = Path::new(src_dir).join(format!("{name}{ext}"));
            let dst = Path::new(dst_dir).join(format!("{name}{ext}"));
            if !src.is_file() {
                continue;
            }
            match copy_file(&src, &dst, force) {
                Err(e) => errors.push(format!("{name}{ext}: {e}")),
                Ok(st) => {
                    let rec = format!("{label}:{name}{ext}");
                    if st == "copied" { copied.push(json!(rec)) } else { skipped.push(json!({"file": rec, "why": st})) }
                }
            }
        }
    };
    for name in &wanted {
        if direction == "to_server" || direction == "both" {
            one(name, &cli, &srv, "->server");
        }
        if direction == "to_client" || direction == "both" {
            one(name, &srv, &cli, "->client");
        }
    }
    let lobby_info = refresh_lobby();
    let again = mods::audit();
    json!({"ok": errors.is_empty(), "direction": direction, "copied": copied, "skipped": skipped, "errors": errors,
           "server_dir": srv, "client_dir": cli, "problems_before": au.get("problems"),
           "problems_after": again.get("problems"), "lobby": lobby_info})
}

/// Look at a folder or .zip and report the mods it could install.
pub fn scan_source(path: &str) -> Value {
    let p = Path::new(path);
    if path.is_empty() || !p.exists() {
        return json!({"ok": false, "error": "path not found"});
    }
    let mut found: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    if p.is_file() && path.to_lowercase().ends_with(".zip") {
        match archive::entries(p) {
            Ok(es) => for e in es {
                let (base, ext) = splitext(&basename(&e.name));
                let ext = ext.to_lowercase();
                if ext == ".kspkg" || ext == ".json" {
                    found.entry(base).or_default().insert(ext);
                }
            },
            Err(e) => return json!({"ok": false, "error": format!("bad zip: {e}")}),
        }
    } else if p.is_dir() {
        for e in std::fs::read_dir(p).into_iter().flatten().flatten() {
            let f = e.file_name().to_string_lossy().into_owned();
            let (base, ext) = splitext(&f);
            let ext = ext.to_lowercase();
            if ext == ".kspkg" || ext == ".json" {
                found.entry(base).or_default().insert(ext);
            }
        }
    } else {
        return json!({"ok": false, "error": "expected a folder or a .zip"});
    }
    let mods: Vec<Value> = found.iter().map(|(b, e)| json!({"name": b, "kspkg": e.contains(".kspkg"), "json": e.contains(".json"),
        "complete": e.contains(".kspkg") && e.contains(".json")})).collect();
    let complete = mods.iter().filter(|m| m["complete"] == json!(true)).count();
    json!({"ok": true, "path": path, "mods": mods, "complete": complete})
}

fn mod_dests(create: bool) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for d in [mods_dir(create), client_mods_dir()] {
        if d.is_empty() {
            continue;
        }
        let key = std::path::absolute(&d).map(|p| p.to_string_lossy().to_lowercase()).unwrap_or_else(|_| d.to_lowercase());
        if !seen.insert(key) {
            continue;
        }
        if create {
            let _ = std::fs::create_dir_all(&d);
        }
        out.push(d);
    }
    out
}

fn same_path(a: &Path, b: &Path) -> bool {
    std::path::absolute(a).ok() == std::path::absolute(b).ok()
}

/// Write one half of a car pair onto every side we install to.
fn write_mod_file(name: &str, ext: &str, src: Result<&mut dyn Read, &Path>) -> std::io::Result<Vec<PathBuf>> {
    if !matches!(ext.to_lowercase().as_str(), ".kspkg" | ".json") {
        return Ok(Vec::new());
    }
    let dests = mod_dests(true);
    if dests.is_empty() {
        return Ok(Vec::new());
    }
    let first = Path::new(&dests[0]).join(format!("{name}{ext}"));
    match src {
        Err(path) => if !same_path(path, &first) { copy2(path, &first)?; },
        Ok(r) => {
            let mut f = std::fs::File::create(&first)?;
            std::io::copy(r, &mut f)?;
        }
    }
    let mut written = vec![first.clone()];
    for d in &dests[1..] {
        let dst = Path::new(d).join(format!("{name}{ext}"));
        if !same_path(&dst, &first) {
            copy2(&first, &dst)?;
        }
        written.push(dst);
    }
    Ok(written)
}

fn car_incoming_sizes(path: &str, wanted: &[Value]) -> BTreeMap<String, BTreeMap<String, u64>> {
    let mut out: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
    let names: HashSet<String> = wanted.iter().map(|m| sv(m, "name")).collect();
    let p = Path::new(path);
    if p.is_file() && path.to_lowercase().ends_with(".zip") {
        for e in archive::entries(p).unwrap_or_default() {
            let (base, ext) = splitext(&basename(&e.name));
            let ext = ext.to_lowercase();
            if (ext != ".kspkg" && ext != ".json") || !names.contains(&base) {
                continue;
            }
            out.entry(base).or_default().insert(ext, e.size);
        }
        return out;
    }
    for m in wanted {
        let n = sv(m, "name");
        for ext in [".kspkg", ".json"] {
            let src = p.join(format!("{n}{ext}"));
            if src.is_file() {
                out.entry(n.clone()).or_default().insert(ext.into(), size(&src));
            }
        }
    }
    out
}

/// Already installed? And are the bytes the same size as what we have?
fn car_conflict(incoming: &BTreeMap<String, BTreeMap<String, u64>>) -> Option<Value> {
    let dests: Vec<(&str, String)> = [("server", mods_dir(false)), ("client", client_mods_dir())].into_iter().filter(|(_, d)| !d.is_empty()).collect();
    let mut conflicts = Vec::new();
    let mut any = false;
    for (name, files) in incoming {
        for (side, d) in &dests {
            for ext in [".kspkg", ".json"] {
                let dp = Path::new(d).join(format!("{name}{ext}"));
                if !dp.is_file() {
                    continue;
                }
                any = true;
                let inc = files.get(ext).copied();
                conflicts.push(json!({"name": name, "side": side, "file": format!("{name}{ext}"),
                    "same": inc.is_some() && inc == Some(size(&dp)), "have": size(&dp), "incoming": inc}));
            }
        }
    }
    if !any {
        return None;
    }
    let mut same = !conflicts.is_empty() && conflicts.iter().all(|c| c["same"] == json!(true));
    for (name, files) in incoming {
        for ext in files.keys() {
            for (_, d) in &dests {
                if !Path::new(d).join(format!("{name}{ext}")).is_file() {
                    same = false;
                }
            }
        }
    }
    let names: Vec<&String> = incoming.keys().collect();
    let label = if names.len() == 1 { names[0].clone() } else { format!("{} car mods", names.len()) };
    Some(json!({"ok": false, "need_confirm": true, "exists": true, "same": same, "kind": "car",
        "name": if names.len() == 1 { names[0].clone() } else { names.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ") },
        "label": label, "names": names, "conflicts": conflicts}))
}

/// Copy mod pairs from a folder or .zip onto the server AND the client.
pub fn install(path: &str, only: Option<Vec<String>>, overwrite: bool) -> Value {
    let scan = scan_source(path);
    if scan.get("ok") != Some(&json!(true)) {
        return scan;
    }
    let wanted: Vec<Value> = scan["mods"].as_array().cloned().unwrap_or_default().into_iter()
        .filter(|m| only.as_ref().map(|o| o.is_empty() || o.contains(&sv(m, "name"))).unwrap_or(true)).collect();
    if wanted.is_empty() {
        return json!({"ok": false, "error": "nothing selected to install"});
    }
    if !overwrite {
        if let Some(hit) = car_conflict(&car_incoming_sizes(path, &wanted)) {
            return hit;
        }
    }
    let (mut done, mut skipped) = (Vec::new(), Vec::new());
    let p = Path::new(path);
    let names: HashSet<String> = wanted.iter().map(|m| sv(m, "name")).collect();
    if p.is_file() {
        let dests = mod_dests(false);
        let dest0 = dests.first().cloned().unwrap_or_else(|| p.parent().map(show).unwrap_or_default());
        if let Some(bad) = archive_budget(p, Path::new(&dest0)) {
            return bad;
        }
        let r = archive::each_file(p, |n, _size, r| {
            let (base, ext) = splitext(&basename(n));
            if !matches!(ext.to_lowercase().as_str(), ".kspkg" | ".json") || !names.contains(&base) {
                return Ok(true);
            }
            write_mod_file(&base, &ext, Ok(r)).map_err(|e| e.to_string())?;
            done.push(format!("{base}{ext}"));
            Ok(true)
        });
        if let Err(e) = r {
            return json!({"ok": false, "error": e});
        }
    } else {
        for m in &wanted {
            let n = sv(m, "name");
            for ext in [".kspkg", ".json"] {
                let src = p.join(format!("{n}{ext}"));
                if src.exists() {
                    if let Err(e) = write_mod_file(&n, ext, Err(&src)) {
                        return json!({"ok": false, "error": e.to_string()});
                    }
                    done.push(format!("{n}{ext}"));
                } else {
                    skipped.push(format!("{n}{ext}"));
                }
            }
        }
    }
    let incomplete: Vec<&Value> = wanted.iter().filter(|m| m["complete"] != json!(true)).collect();
    let mut notes = Vec::new();
    for m in &incomplete {
        if m["json"] != json!(true) {
            notes.push(format!("{}: no .json - the car will NOT appear in the car list", sv(m, "name")));
        } else if m["kspkg"] != json!(true) {
            notes.push(format!("{}: no .kspkg - there is no content to load", sv(m, "name")));
        }
    }
    refresh_lobby();
    after_content_change();
    json!({"ok": true, "kind": "car", "installed": done, "missing": skipped,
           "incomplete": incomplete.iter().map(|m| sv(m, "name")).collect::<Vec<_>>(),
           "sides": mod_dests(false), "warning": if notes.is_empty() { Value::Null } else { json!(notes.join("; ")) }})
}

/// Drop a thing from every registry entry that listed it.
fn unshare_name(m: Option<&str>, t: Option<&str>) {
    let items = registry::load();
    let mut changed = false;
    let mut keep = Vec::new();
    for e in items {
        let strs = |k: &str| e.get(k).and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect::<Vec<_>>()).unwrap_or_default();
        let (mut ms, mut ts) = (strs("required_mods"), strs("required_tracks"));
        if let Some(m) = m { if ms.iter().any(|x| x == m) { ms.retain(|x| x != m); changed = true; } }
        if let Some(t) = t { if ts.iter().any(|x| x == t) { ts.retain(|x| x != t); changed = true; } }
        let mut e2 = e.as_object().cloned().unwrap_or_default();
        e2.insert("required_mods".into(), json!(ms));
        e2.insert("required_tracks".into(), json!(ts));
        if !ms.is_empty() || !ts.is_empty() { keep.push(Value::Object(e2)); } else { changed = true; }
    }
    if changed {
        let _ = registry::save(&keep);
    }
}

pub fn safe_base(name: &str) -> String {
    let n = basename(&name.replace('\\', "/")).trim().to_string();
    if n.is_empty() || n == "." || n == ".." || n.chars().any(|c| "/:*?\"<>|\0".contains(c)) {
        return String::new();
    }
    n
}

pub fn safe_folder(name: &str) -> String {
    let mut name = safe_base(name);
    let low = name.to_lowercase();
    if name.is_empty() || [".tar", ".zip", ".kspkg", ".json", ".tgz", ".gz"].iter().any(|s| low.ends_with(s)) {
        let (stem, ext) = splitext(&name);
        if [".tar", ".zip", ".tgz", ".gz"].contains(&ext.to_lowercase().as_str()) {
            name = stem;
            if name.to_lowercase().ends_with(".tar") {
                name.truncate(name.len() - 4);
            }
        }
    }
    if name.is_empty() || name == "." || name == ".." {
        return String::new();
    }
    name
}

/// Delete a car pair, both sides by default (a half-pair left behind is how
/// a "removed" mod still shows up).
pub fn remove(name: &str, sides: Option<&[&str]>) -> Value {
    let name = safe_base(name);
    if name.is_empty() {
        return json!({"ok": false, "error": "bad name"});
    }
    let want = sides.unwrap_or(&["server", "client"]);
    let mut dirs = Vec::new();
    if want.contains(&"server") { dirs.push(mods_dir(false)); }
    if want.contains(&"client") { dirs.push(client_mods_dir()); }
    let (mut gone, mut stuck) = (Vec::new(), Vec::new());
    for d in dirs {
        if d.is_empty() || !Path::new(&d).is_dir() {
            continue;
        }
        for ext in [".kspkg", ".json"] {
            let p = Path::new(&d).join(format!("{name}{ext}"));
            if !p.exists() {
                continue;
            }
            // ⚠ never let one locked file stop the rest
            match std::fs::remove_file(&p) {
                Ok(()) => gone.push(show(&p)),
                Err(e) => stuck.push(format!("{} ({})", show(&p), io_err(&e))),
            }
        }
    }
    unshare_name(Some(&name), None);
    refresh_lobby();
    after_content_change();
    if !stuck.is_empty() {
        return json!({"ok": false, "removed": gone, "error": format!("could not delete {} - close the game and stop the server, then remove it again", stuck.join("; "))});
    }
    json!({"ok": !gone.is_empty(), "removed": gone})
}

/// Python's ex.strerror for the common cases
fn io_err(e: &std::io::Error) -> String {
    match e.raw_os_error() {
        Some(32) => "The process cannot access the file because it is being used by another process".into(),
        Some(5) => "Access is denied".into(),
        _ => e.to_string(),
    }
}

// --------------------------------------------------------------- archives --
const JUNK: [&str; 11] = [".txt", ".md", ".jpg", ".jpeg", ".png", ".webp", ".url", ".pdf", ".nfo", ".html", ".htm"];

/// The single wrapper folder every file sits in, or "" (`tracks/<name>/`
/// counts as one two-deep wrapper).
pub fn archive_root(names: &[String]) -> String {
    let common = |depth: usize| -> String {
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for n in names {
            let n = n.replace('\\', "/");
            let n = n.trim_start_matches('/');
            if n.is_empty() || n.ends_with('/') {
                continue;
            }
            let parts: Vec<&str> = n.split('/').filter(|p| !p.is_empty()).collect();
            if parts.len() <= depth + 1 {
                if parts.last().map(|l| { let l = l.to_lowercase(); JUNK.iter().any(|j| l.ends_with(j)) }).unwrap_or(false) {
                    continue;
                }
                return String::new();
            }
            seen.insert(parts[depth].to_string());
            if seen.len() > 1 {
                return String::new();
            }
        }
        if seen.len() == 1 { seen.into_iter().next().unwrap() } else { String::new() }
    };
    let top = common(0);
    if top.is_empty() {
        return String::new();
    }
    if top.to_lowercase() == "tracks" {
        let second = common(1);
        if !second.is_empty() {
            return format!("{top}/{second}/");
        }
    }
    format!("{top}/")
}

pub fn archive_rel(rel: &str, root: &str) -> String {
    let rel = rel.replace('\\', "/");
    let rel = rel.trim_start_matches('/');
    if rel.is_empty() || rel.ends_with('/') || rel.split('/').any(|p| p == "..") {
        return String::new();
    }
    if basename(rel) == "acecm_track.json" {
        return "acecm_track.json".into();
    }
    let rel = if !root.is_empty() && rel.starts_with(root) { &rel[root.len()..] } else { rel };
    let mut parts: Vec<&str> = rel.split('/').filter(|p| !p.is_empty()).collect();
    if !parts.is_empty() && parts[0].to_lowercase() == "tracks" && parts.len() > 2 {
        parts.drain(..2);
    }
    parts.join("/")
}

fn archive_file_sizes(path: &Path) -> BTreeMap<String, u64> {
    let mut out = BTreeMap::new();
    let es = archive::entries(path).unwrap_or_default();
    let root = archive_root(&es.iter().map(|e| e.name.clone()).collect::<Vec<_>>());
    for e in es {
        let rel = archive_rel(&e.name, &root);
        if !rel.is_empty() {
            out.insert(rel, e.size);
        }
    }
    out
}

fn dir_file_sizes(root: &Path) -> BTreeMap<String, u64> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                stack.push(p);
            } else if let Ok(rel) = p.strip_prefix(root) {
                out.insert(rel.to_string_lossy().replace('\\', "/"), size(&p));
            }
        }
    }
    out
}

/// Same payload? The sidecar is ignored (hosts regenerate it).
fn sizes_match(inc: &BTreeMap<String, u64>, have: &BTreeMap<String, u64>) -> bool {
    let strip = |m: &BTreeMap<String, u64>| m.iter().filter(|(k, _)| k.as_str() != "acecm_track.json").map(|(k, v)| (k.clone(), *v)).collect::<BTreeMap<_, _>>();
    let (a, b) = (strip(inc), strip(have));
    !a.is_empty() && a == b
}

fn track_has_files(dest: &Path) -> bool {
    dest.is_dir() && std::fs::read_dir(dest).map(|mut r| r.next().is_some()).unwrap_or(false)
}

fn track_conflict(folder: &str, dest: &Path, incoming: &BTreeMap<String, u64>, label: &str) -> Option<Value> {
    if !track_has_files(dest) {
        return None;
    }
    let same = sizes_match(incoming, &dir_file_sizes(dest));
    Some(json!({"ok": false, "need_confirm": true, "exists": true, "same": same, "kind": "track",
        "name": folder, "label": if label.is_empty() { folder } else { label }, "folder": folder, "path": show(dest)}))
}

// ⚠ generous: flat-colour .dds really do compress 500-700x; free space is
// the real guard, this only catches the absurd
const BOMB_RATIO: u64 = 1000;
const BOMB_MIN_BYTES: u64 = 1 << 30;
const FREE_MARGIN: u64 = 1 << 30;

fn existing_dir(p: &Path) -> PathBuf {
    let mut p = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    while !p.is_dir() {
        match p.parent() {
            Some(q) if q != p => p = q.to_path_buf(),
            _ => break,
        }
    }
    p
}

#[cfg(windows)]
fn free_bytes(p: &Path) -> Option<u64> {
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let w: Vec<u16> = p.as_os_str().to_string_lossy().encode_utf16().chain(Some(0)).collect();
    let mut free = 0u64;
    let ok = unsafe { GetDiskFreeSpaceExW(w.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) };
    if ok != 0 { Some(free) } else { None }
}

#[cfg(not(windows))]
fn free_bytes(p: &Path) -> Option<u64> {
    let out = std::process::Command::new("df").args(["-Pk"]).arg(p).output().ok()?;
    let t = String::from_utf8_lossy(&out.stdout);
    t.lines().nth(1)?.split_whitespace().nth(3)?.parse::<u64>().ok().map(|k| k * 1024)
}

/// None if `path` is safe to unpack into `dest`, else an error value -
/// checked BEFORE anything is deleted or written.
pub fn archive_budget(path: &Path, dest: &Path) -> Option<Value> {
    let es = match archive::entries(path) {
        Ok(e) => e,
        Err(e) => return Some(json!({"ok": false, "error": format!("cannot read the archive: {e}")})),
    };
    let fname = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    for e in &es {
        if let Some(packed) = e.packed {
            if e.size > BOMB_MIN_BYTES && e.size > packed * BOMB_RATIO {
                return Some(json!({"ok": false, "error": format!("refusing {fname}: {} claims {:.1} GB from {:.1} MB - that is a decompression bomb, not game content",
                    e.name, e.size as f64 / 1e9, packed as f64 / 1e6)}));
            }
        }
    }
    let total: u64 = es.iter().map(|e| e.size).sum();
    if let Some(free) = free_bytes(&existing_dir(dest)) {
        if total as i128 > free as i128 - FREE_MARGIN as i128 {
            return Some(json!({"ok": false, "error": format!("{fname} unpacks to {:.1} GB but only {:.1} GB is free there - make room first",
                total as f64 / 1e9, free as f64 / 1e9)}));
        }
    }
    None
}

fn log_skipped(path: &Path, skipped: &[String]) {
    if skipped.is_empty() {
        return;
    }
    let fname = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    logs::warn(format!("{fname}: skipped {} non-content file(s): {}{}", skipped.len(),
        skipped.iter().take(8).cloned().collect::<Vec<_>>().join(", "), if skipped.len() > 8 { " …" } else { "" }));
}

/// Unpack a track pack into dest: members cannot escape it, only content
/// types are written, a single wrapper folder is stripped.
fn extract_archive(path: &Path, dest: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let es = archive::entries(path)?;
    let root = archive_root(&es.iter().map(|e| e.name.clone()).collect::<Vec<_>>());
    ingest_begin("Unpacking", es.iter().map(|e| e.size).sum());
    let destr = show(dest);
    let mut skipped = Vec::new();
    archive::each_file(path, |name, sz, r| {
        let raw = name.replace('\\', "/");
        let raw = raw.trim_start_matches('/');
        if raw.is_empty() || raw.ends_with('/') || raw.split('/').any(|p| p == "..") {
            return Ok(true);
        }
        let rel = archive_rel(raw, &root);
        if rel.is_empty() {
            return Ok(true);
        }
        // ⚠ SKIP, do not fail: a readme in a track zip is fine
        if !contentsync::content_allowed(raw) {
            skipped.push(raw.to_string());
            return Ok(true);
        }
        let parts: Vec<&str> = rel.split('/').filter(|p| !p.is_empty()).collect();
        if parts.is_empty() {
            return Ok(true);
        }
        let target = under(&destr, &parts)?;
        if let Some(p) = target.parent() {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        let mut f = std::fs::File::create(&target).map_err(|e| e.to_string())?;
        std::io::copy(r, &mut f).map_err(|e| e.to_string())?;
        ingest_step(sz, &basename(raw));
        Ok(true)
    })?;
    log_skipped(path, &skipped);
    Ok(())
}

/// A joiner's pack: members keep their own layout (no wrapper strip), with
/// tarfile's "data" filter guards plus the content allowlist.
pub fn extract_tar_safe(path: &Path, dest: &Path, folder: &str) -> Result<(), String> {
    let mut t = tar::Archive::new(std::io::BufReader::new(std::fs::File::open(path).map_err(|e| e.to_string())?));
    let destr = show(dest);
    for e in t.entries().map_err(|e| e.to_string())? {
        let mut e = e.map_err(|e| e.to_string())?;
        let name = e.path().map_err(|e| e.to_string())?.to_string_lossy().replace('\\', "/");
        if name.starts_with('/') || name.split('/').any(|p| p == "..") || name.contains(':') {
            return Err(format!("refusing {name}: it would land outside the track folder"));
        }
        let parts: Vec<&str> = name.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
        if parts.is_empty() {
            continue;
        }
        match e.header().entry_type() {
            tar::EntryType::Directory => {
                let d = under(&destr, &parts)?;
                std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
            }
            tar::EntryType::Regular | tar::EntryType::Continuous => {
                if !contentsync::content_allowed(&name) {
                    logs::warn(format!("track pack {folder}: skipping {name} (not a content file)"));
                    continue;
                }
                let target = under(&destr, &parts)?;
                if let Some(p) = target.parent() {
                    std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
                }
                let mut f = std::fs::File::create(&target).map_err(|e| e.to_string())?;
                std::io::copy(&mut e, &mut f).map_err(|e| e.to_string())?;
            }
            // links, devices, fifos: never content
            _ => {}
        }
    }
    Ok(())
}

fn archive_meta(path: &Path) -> Value {
    let hit = archive::names(path).into_iter().find(|n| basename(&n.replace('\\', "/")) == "acecm_track.json");
    hit.and_then(|h| archive::read_member(path, &h)).and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .filter(|v| v.is_object()).unwrap_or(json!({}))
}

fn looks_like_track_dir(p: &Path) -> bool {
    if !p.is_dir() {
        return false;
    }
    let names: Vec<String> = std::fs::read_dir(p).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    if names.iter().any(|n| n == "acecm_track.json") {
        return true;
    }
    if names.iter().any(|n| n == "containers") && p.join("containers").is_dir() {
        return true;
    }
    names.iter().any(|n| { let l = n.to_lowercase(); l.ends_with(".scene") || l.ends_with(".track") })
}

fn merge(a: &mut Value, b: &Value) {
    if let (Some(a), Some(b)) = (a.as_object_mut(), b.as_object()) {
        for (k, v) in b {
            a.insert(k.clone(), v.clone());
        }
    }
}

fn track_result(folder: &str, meta: &Value, dest: &Path, reg: Value, default_warn: Option<&str>) -> Value {
    let ok = reg.get("ok") == Some(&json!(true));
    let dn = { let d = sv(meta, "display_name"); if d.is_empty() { folder.to_string() } else { d } };
    let warning = if ok { Value::Null } else {
        let e = sv(&reg, "error");
        if !e.is_empty() { json!(e) } else { default_warn.map(|w| json!(w)).unwrap_or(Value::Null) }
    };
    json!({"ok": true, "kind": "track", "folder": folder, "display_name": dn, "path": show(dest),
           "registered": ok, "register": reg, "warning": warning})
}

/// Install a multiplayer track tar/zip the same way Get content does.
pub fn install_track_pack(path: &Path, folder: Option<&str>, overwrite: bool) -> Value {
    if !path.is_file() {
        return json!({"ok": false, "error": "track pack not found"});
    }
    let mut meta = archive_meta(path);
    // ⚠ the archive's own wrapper folder beats the ZIP FILE NAME
    let wrapper = archive_root(&archive::names(path)).trim_end_matches('/').to_string();
    let stem = splitext(&path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).0;
    let pick = [folder.unwrap_or("").to_string(), sv(&meta, "folder"), wrapper, stem].into_iter().find(|s| !s.is_empty()).unwrap_or_default();
    let mut folder = safe_folder(&pick);
    if folder.is_empty() {
        return json!({"ok": false, "error": "could not tell the track folder name"});
    }
    let td = contentsync::tracks_dir();
    let mut dest = match under(&td, &[&folder]) { Ok(d) => d, Err(e) => return json!({"ok": false, "error": e}) };
    if !overwrite {
        let label = { let d = sv(&meta, "display_name"); if d.is_empty() { folder.clone() } else { d } };
        if let Some(hit) = track_conflict(&folder, &dest, &archive_file_sizes(path), &label) {
            return hit;
        }
    }
    if let Some(bad) = archive_budget(path, &dest) {
        return bad;
    }
    if overwrite && track_has_files(&dest) {
        if let Err(e) = std::fs::remove_dir_all(&dest) {
            return json!({"ok": false, "error": e.to_string()});
        }
    }
    if let Err(e) = extract_archive(path, &dest) {
        return json!({"ok": false, "error": e});
    }
    if let Some(side) = std::fs::read(dest.join("acecm_track.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) {
        merge(&mut meta, &side);
    }
    let mf = sv(&meta, "folder");
    if !mf.is_empty() && safe_folder(&mf) != folder {
        let want = safe_folder(&mf);
        if !want.is_empty() {
            if let Ok(dest2) = under(&td, &[&want]) {
                if dest2 != dest {
                    if dest2.is_dir() {
                        let _ = std::fs::remove_dir_all(&dest2);
                    }
                    if std::fs::rename(&dest, &dest2).is_ok() {
                        dest = dest2;
                        folder = want;
                    }
                }
            }
        }
    }
    ingest_begin("Registering in the client tables", 0);
    let reg = crate::trackdeploy::register_client_track(&folder, &meta);
    ingest_end("");
    after_content_change();
    track_result(&folder, &meta, &dest, reg, Some("files are in place; register the track when the game is closed"))
}

fn copytree(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for e in std::fs::read_dir(src)? {
        let e = e?;
        let to = dst.join(e.file_name());
        if e.file_type()?.is_dir() {
            copytree(&e.path(), &to)?;
        } else {
            copy2(&e.path(), &to)?;
        }
    }
    Ok(())
}

/// Copy an already-unpacked track tree into the client's tracks folder.
pub fn install_track_folder(path: &Path, folder: Option<&str>, overwrite: bool) -> Value {
    if !path.is_dir() {
        return json!({"ok": false, "error": "track folder not found"});
    }
    let meta = std::fs::read(path.join("acecm_track.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).filter(|v| v.is_object()).unwrap_or(json!({}));
    let pick = [folder.unwrap_or("").to_string(), sv(&meta, "folder"), path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()]
        .into_iter().find(|s| !s.is_empty()).unwrap_or_default();
    let folder = safe_folder(&pick);
    if folder.is_empty() {
        return json!({"ok": false, "error": "bad track folder name"});
    }
    let dest = match under(&contentsync::tracks_dir(), &[&folder]) { Ok(d) => d, Err(e) => return json!({"ok": false, "error": e}) };
    let label = { let d = sv(&meta, "display_name"); if d.is_empty() { folder.clone() } else { d } };
    let same_dir = same_path(path, &dest);
    if !overwrite && !same_dir {
        if let Some(hit) = track_conflict(&folder, &dest, &dir_file_sizes(path), &label) {
            return hit;
        }
    }
    if !same_dir {
        if dest.is_dir() {
            if !overwrite {
                return track_conflict(&folder, &dest, &dir_file_sizes(path), &label).unwrap_or(json!({"ok": false, "error": "track already exists"}));
            }
            if let Err(e) = std::fs::remove_dir_all(&dest) {
                return json!({"ok": false, "error": e.to_string()});
            }
        }
        if let Err(e) = copytree(path, &dest) {
            return json!({"ok": false, "error": e.to_string()});
        }
    }
    if truthy(Some(&meta)) {
        let mut s = crate::profiles::py_dumps_indent2(&meta);
        if cfg!(windows) {
            s = s.replace('\n', "\r\n");
        }
        let _ = std::fs::write(dest.join("acecm_track.json"), s);
    }
    ingest_begin("Registering in the client tables", 0);
    let reg = crate::trackdeploy::register_client_track(&folder, &meta);
    ingest_end("");
    after_content_change();
    track_result(&folder, &meta, &dest, reg, None)
}

/// Install whatever was dropped: car pair, car zip, or track pack.
pub fn ingest(path: &str, overwrite: bool) -> Value {
    let p = Path::new(path);
    if path.is_empty() || !p.exists() {
        return json!({"ok": false, "error": "nothing to install"});
    }
    if p.is_dir() {
        if looks_like_track_dir(p) {
            return install_track_folder(p, None, overwrite);
        }
        let scan = scan_source(path);
        if scan.get("ok") == Some(&json!(true)) && truthy(scan.get("mods")) {
            return install(path, None, overwrite);
        }
        return json!({"ok": false, "error": "that folder is not a car mod (.kspkg + .json) or a track pack"});
    }
    let low = path.to_lowercase();
    if low.ends_with(".tar") || low.ends_with(".tar.gz") || low.ends_with(".tgz") {
        return install_track_pack(p, None, overwrite);
    }
    if low.ends_with(".zip") {
        let meta = archive_meta(p);
        let scan = scan_source(path);
        if scan.get("ok") == Some(&json!(true)) && truthy(scan.get("complete")) {
            return install(path, None, overwrite);
        }
        if truthy(Some(&meta)) || !truthy(scan.get("mods")) {
            return install_track_pack(p, None, overwrite);
        }
        return install(path, None, overwrite);
    }
    if low.ends_with(".kspkg") || low.ends_with(".json") {
        let folder = p.parent().map(show).unwrap_or_default();
        let scan = if Path::new(&folder).is_dir() { scan_source(&folder) } else { json!({"ok": false}) };
        if scan.get("ok") == Some(&json!(true)) && truthy(scan.get("complete")) {
            return install(&folder, None, overwrite);
        }
        return json!({"ok": false, "error": "drop the matching .kspkg and .json together (or a zip of both)"});
    }
    json!({"ok": false, "error": "drop a car zip (.kspkg + .json) or a track pack (.tar)"})
}

// ------------------------------------------------------------------ drops --
fn drop_root() -> PathBuf {
    let d = paths::data().join("drop");
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn drop_staging(did: &str, create: bool) -> Option<PathBuf> {
    let did = did.trim();
    if !(8..=40).contains(&did.len()) || !did.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return None;
    }
    let d = drop_root().join(did);
    if create {
        let _ = std::fs::create_dir_all(&d);
    }
    if d.is_dir() || create { Some(d) } else { None }
}

/// Stream one dropped file into a staging folder. ⚠ Reads EXACTLY
/// Content-Length (an HTTP/1.1 browser never sends EOF).
pub fn drop_part_open(did: &str, filename: &str) -> Result<(PathBuf, std::fs::File), Value> {
    let name = safe_base(filename);
    if name.is_empty() {
        return Err(json!({"ok": false, "error": "bad filename"}));
    }
    let Some(dir) = drop_staging(did, true) else { return Err(json!({"ok": false, "error": "bad drop id"})) };
    let dest = dir.join(&name);
    let f = std::fs::File::create(&dest).map_err(|e| json!({"ok": false, "error": e.to_string()}))?;
    Ok((dest, f))
}

pub const DROP_LIMIT: u64 = 8 * 1024 * 1024 * 1024;

pub fn drop_cleanup(did: &str) {
    if let Some(d) = drop_staging(did, false) {
        let _ = std::fs::remove_dir_all(d);
    }
}

/// Finish a drag-drop. On a duplicate the files stay staged so the page can
/// ask and retry with overwrite without uploading again.
pub fn ingest_staging(did: &str, overwrite: bool) -> Value {
    let Some(d) = drop_staging(did, false).filter(|d| d.is_dir()) else { return json!({"ok": false, "error": "nothing was dropped"}) };
    let names: Vec<String> = std::fs::read_dir(&d).into_iter().flatten().flatten()
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false)).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    if names.is_empty() {
        drop_cleanup(did);
        return json!({"ok": false, "error": "drop was empty"});
    }
    // ⚠ the feed is reset however this ends
    let r = (|| {
        if names.len() == 1 {
            let one = d.join(&names[0]);
            // ⚠ a server package before the ordinary ingest: it is a zip too
            if crate::pack::is_package(&one) {
                return crate::pack::install(&one);
            }
            return ingest(&show(&one), overwrite);
        }
        if names.iter().any(|n| n.to_lowercase() == "acecm_track.json") || names.iter().any(|n| { let l = n.to_lowercase(); l.ends_with(".scene") || l.ends_with(".track") }) {
            return install_track_folder(&d, None, overwrite);
        }
        let scan = scan_source(&show(&d));
        if scan.get("ok") == Some(&json!(true)) && truthy(scan.get("mods")) {
            return install(&show(&d), None, overwrite);
        }
        json!({"ok": false, "error": "could not tell if that was a car or a track pack"})
    })();
    ingest_end("");
    if truthy(r.get("need_confirm")) {
        let mut r = r;
        r["id"] = json!(did);
        return r;
    }
    drop_cleanup(did);
    if r.get("ok") == Some(&json!(true)) {
        after_content_change();
    }
    r
}

/// Delete an imported track: its table rows FIRST, then its files.
pub fn remove_track(folder: &str) -> Value {
    let folder = safe_folder(folder);
    if folder.is_empty() {
        return json!({"ok": false, "error": "bad track folder"});
    }
    let Ok(dest) = under(&contentsync::tracks_dir(), &[&folder]) else { return json!({"ok": false, "error": "bad track folder"}) };
    if !dest.is_dir() {
        return json!({"ok": false, "error": "that track is not installed"});
    }
    let unreg = match crate::trackdeploy::client_kspkg() {
        Some(p) => crate::trackdeploy::unregister_track_into(Path::new(&p), &folder, "", true),
        None => json!({"ok": false, "error": "client content.kspkg not found"}),
    };
    if unreg.get("ok") != Some(&json!(true)) {
        if truthy(unreg.get("needs_close")) {
            return json!({"ok": false, "needs_close": true, "error": "close the game first - removing a track edits the client tables, and deleting the files without that leaves the game listing a track it cannot load"});
        }
        logs::warn(format!("could not unregister {folder} before deleting: {}", sv(&unreg, "error")));
    }
    if let Err(e) = std::fs::remove_dir_all(&dest) {
        return json!({"ok": false, "error": e.to_string()});
    }
    unshare_name(None, Some(&folder));
    let _ = std::fs::remove_file(paths::data().join("track_map.json"));
    json!({"ok": true, "removed": show(&dest), "folder": folder, "unregistered": unreg.get("ok") == Some(&json!(true))})
}

fn downloads() -> PathBuf {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_default().join("Downloads")
}

/// Zip the .kspkg + .json the way a drop can install it again.
pub fn export_car(name: &str, dest_dir: Option<&str>) -> Value {
    let name = safe_base(name);
    if name.is_empty() {
        return json!({"ok": false, "error": "bad name"});
    }
    let mut srcs: Vec<(String, PathBuf)> = Vec::new();
    for d in [client_mods_dir(), mods_dir(false)] {
        if d.is_empty() {
            continue;
        }
        for ext in [".kspkg", ".json"] {
            let p = Path::new(&d).join(format!("{name}{ext}"));
            if !srcs.iter().any(|(e, _)| e == ext) && p.is_file() {
                srcs.push((ext.to_string(), p));
            }
        }
    }
    if !srcs.iter().any(|(e, _)| e == ".kspkg") {
        return json!({"ok": false, "error": format!("no {name}.kspkg on this machine")});
    }
    let dd = dest_dir.map(PathBuf::from).unwrap_or_else(downloads);
    let _ = std::fs::create_dir_all(&dd);
    let dest = dd.join(format!("{name}.zip"));
    let r = (|| -> Result<(), String> {
        let mut z = zip::ZipWriter::new(std::fs::File::create(&dest).map_err(|e| e.to_string())?);
        let opt = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).large_file(true);
        for (ext, p) in &srcs {
            z.start_file(format!("{name}{ext}"), opt).map_err(|e| e.to_string())?;
            let mut f = std::fs::File::open(p).map_err(|e| e.to_string())?;
            std::io::copy(&mut f, &mut z).map_err(|e| e.to_string())?;
        }
        z.finish().map_err(|e| e.to_string())?;
        Ok(())
    })();
    if let Err(e) = r {
        return json!({"ok": false, "error": e});
    }
    json!({"ok": true, "kind": "car", "name": name, "path": show(&dest), "files": srcs.iter().map(|(e, _)| e.clone()).collect::<Vec<_>>()})
}

/// The same uncompressed tar friends pull with Get content.
pub fn export_track(folder: &str, dest_dir: Option<&str>) -> Value {
    let folder = safe_folder(folder);
    if folder.is_empty() {
        return json!({"ok": false, "error": "bad track folder"});
    }
    let Some(packed) = registry::ensure_track_pack(&folder).filter(|p| p.is_file()) else {
        return json!({"ok": false, "error": "no files for that track"});
    };
    let dd = dest_dir.map(PathBuf::from).unwrap_or_else(downloads);
    let _ = std::fs::create_dir_all(&dd);
    let dest = dd.join(format!("{folder}.tar"));
    if !same_path(&packed, &dest) {
        if let Err(e) = copy2(&packed, &dest) {
            return json!({"ok": false, "error": e.to_string()});
        }
    }
    json!({"ok": true, "kind": "track", "name": folder, "path": show(&dest), "pack": show(&packed)})
}

pub fn export_item(kind: &str, name: &str, dest_dir: Option<&str>) -> Value {
    match kind.trim().to_lowercase().as_str() {
        "car" => export_car(name, dest_dir),
        "track" => export_track(name, dest_dir),
        _ => json!({"ok": false, "error": "kind must be car or track"}),
    }
}

pub fn remove_item(kind: &str, name: &str) -> Value {
    match kind.trim().to_lowercase().as_str() {
        "car" => remove(name, None),
        "track" => remove_track(name),
        _ => json!({"ok": false, "error": "kind must be car or track"}),
    }
}

/// Text a host actually pastes: share name, folder, path, share URL.
pub fn clip_item(kind: &str, name: &str) -> Value {
    let kind = kind.trim().to_lowercase();
    let name = if kind == "car" { safe_base(name) } else { safe_folder(name) };
    if name.is_empty() {
        return json!({"ok": false, "error": "bad name"});
    }
    let lan = sv(&contentsync::share_info(), "lan_url");
    let shared;
    let mut path = String::new();
    let mut label = name.clone();
    let reg = registry::load();
    let listed = |k: &str| reg.iter().any(|e| e.get(k).and_then(|v| v.as_array()).map(|a| a.iter().any(|x| x.as_str() == Some(name.as_str()))).unwrap_or(false));
    if kind == "car" {
        for d in [client_mods_dir(), mods_dir(false)] {
            let p = Path::new(&d).join(format!("{name}.kspkg"));
            if !d.is_empty() && p.is_file() {
                path = show(&p);
                break;
            }
        }
        shared = listed("required_mods");
    } else {
        path = under(&contentsync::tracks_dir(), &[&name]).map(|p| show(&p)).unwrap_or_default();
        if let Some(t) = crate::trackimport::importable().into_iter().find(|t| sv(t, "folder") == name) {
            let d = sv(&t, "display_name");
            label = if d.is_empty() { name.clone() } else { d };
        }
        shared = listed("required_tracks");
    }
    let share = if shared { lan.clone() } else { String::new() };
    json!({"ok": true, "kind": kind, "name": name, "text": name, "label": label, "path": path,
           "share_url": share, "shared": shared,
           "variants": {"name": name, "label": label, "path": path, "share": share}})
}

/// Everything the Content page can manage.
pub fn library() -> Value {
    let au = mods::audit();
    let (mut shared_mods, mut shared_tracks) = (HashSet::new(), HashSet::new());
    for e in registry::load() {
        for m in e.get("required_mods").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
            if let Some(s) = m.as_str() { shared_mods.insert(s.to_string()); }
        }
        for t in e.get("required_tracks").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
            if let Some(s) = t.as_str() { shared_tracks.insert(s.to_string()); }
        }
    }
    let base_dir = { let c = sv(&au, "client_dir"); if c.is_empty() { sv(&au, "server_dir") } else { c } };
    let mut cars = Vec::new();
    for row in au.get("mods").and_then(|m| m.as_array()).cloned().unwrap_or_default() {
        let n = sv(&row, "name");
        cars.push(json!({"kind": "car", "name": n, "label": n,
            "size_mb": row.get("size_mb").filter(|v| truthy(Some(v))).cloned().unwrap_or(json!(0)),
            "cars": row.get("cars").filter(|v| truthy(Some(v))).cloned().unwrap_or(json!([])),
            "server_ok": row.get("server_ok"), "client_ok": row.get("client_ok"),
            "issues": row.get("issues").filter(|v| truthy(Some(v))).cloned().unwrap_or(json!([])),
            "ok": row.get("ok"), "shared": shared_mods.contains(&n),
            "path": show(&Path::new(&base_dir).join(format!("{n}.kspkg")))}));
    }
    let packs = paths::data().join("packs");
    let mut tracks = Vec::new();
    for t in crate::trackimport::importable() {
        let folder = sv(&t, "folder");
        let packed = packs.join(format!("{folder}.tar"));
        let size_mb = if packed.is_file() { json!(((size(&packed) as f64 / 1048576.0) * 10.0).round_ties_even() / 10.0) } else { json!(0) };
        let label = { let d = sv(&t, "display_name"); if d.is_empty() { folder.clone() } else { d } };
        tracks.push(json!({"kind": "track", "name": folder, "label": label, "folder": folder,
            "path": sv(&t, "path"), "layout": sv(&t, "layout"),
            "files": t.get("files").filter(|v| truthy(Some(v))).cloned().unwrap_or(json!(0)),
            "size_mb": size_mb, "ok": truthy(t.get("ok")), "error": t.get("error").cloned().unwrap_or(Value::Null),
            "shared": shared_tracks.contains(&folder)}));
    }
    let lan = sv(&contentsync::share_info(), "lan_url");
    let total = cars.len() + tracks.len();
    json!({"ok": true, "cars": cars, "tracks": tracks, "server_dir": au.get("server_dir"), "client_dir": au.get("client_dir"),
           "tracks_dir": contentsync::tracks_dir(), "share_url": lan, "total": total})
}

/// Write a body stream to a staged drop file, exactly `want` bytes.
pub fn drop_write(f: &mut std::fs::File, chunk: &[u8]) -> std::io::Result<()> {
    f.write_all(chunk)
}
