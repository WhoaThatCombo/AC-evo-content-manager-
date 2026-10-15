//! Deploying tracks into the server archive and registering them in the
//! client's - ported from acecm/tracks.py (native path; the legacy
//! slot-borrow injector stays in the backend).
//!
//! ⚠ Archive KEYS are always lowercase: the engine lowercases before
//! hashing (not one of 137,219 stock records has a capital), so a key with
//! capitals hashes to a path nothing ever asks for.
//! ⚠ The server broadcasts PATHS, not names, so the folder deployed must be
//! the folder clients already hold.
//! Every function that writes takes the archive path explicitly, so it can
//! be exercised on a copy.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::{json, Map, Value};

use crate::{detect, kspkg_write, procs, splines, trackimport, tracktables, viewer, paths};

pub const TEMPLATE_TRACK: &str = "Road Atlanta";

const ROLE_BY_NAME: &[(&str, &str)] = &[
    ("ground_grass", "race_scenery"),
    ("spawnpoints_grid", "spawnpoints_grid"),
    ("spawnpoints_hotlap", "spawnpoints_hotlap"),
    ("spawnpoints_pitlane", "spawnpoints_pitlane"),
    ("timelines", "timelines"),
];

fn arc(p: &str) -> String {
    p.to_lowercase()
}

pub fn server_kspkg() -> PathBuf {
    Path::new(&detect::effective_server_dir()).join("content.kspkg")
}

pub fn client_kspkg() -> Option<String> {
    viewer::package()
}

pub fn server_running() -> bool {
    !procs::pids_named(&["AssettoCorsaEVOServer", "AssettoCorsaEVOServer.stock"]).is_empty()
}

fn home() -> PathBuf {
    std::env::var("USERPROFILE").map(PathBuf::from).unwrap_or_default()
}

fn import_root() -> PathBuf {
    home().join("Saved Games").join("ACE").join("mods").join("content").join("tracks")
}

/// Track folders players have imported loose (EvoForge writes these).
pub fn client_track_folders() -> Vec<String> {
    let d = import_root();
    let mut v: Vec<String> = std::fs::read_dir(&d).map(|rd| rd.flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    v.sort();
    v
}

/// Python str.title(): a letter is upper-cased when the character before it
/// is not a letter (digits count as a boundary).
pub fn title(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev = false;
    for c in s.chars() {
        if c.is_alphabetic() {
            if prev { out.extend(c.to_lowercase()) } else { out.extend(c.to_uppercase()) }
            prev = true;
        } else {
            out.push(c);
            prev = false;
        }
    }
    out
}

fn strip_name(f: &str) -> String {
    title(&f.rsplit_once('_').map(|(a, _)| a).unwrap_or(f).replace('_', " "))
}

/// One canonical display name per folder. ⚠ Stripping EvoForge's random
/// "_xxxx" suffix collides for real (Tatsumi PA and Daishi PA both strip to
/// "Shutoko R"), so a folder whose stripped name is not unique keeps its
/// full name - upsert_track_row keys by display name.
pub fn display_name_for(folder: &str) -> String {
    let name = strip_name(folder);
    let clash = client_track_folders().iter()
        .any(|f| f != folder && f != "common_assets" && strip_name(f) == name);
    if clash { title(&folder.replace('_', " ")) } else { name }
}

/// What the local client's tracks.table calls this folder, if anything.
pub fn client_name(folder: &str) -> String {
    for (name, f) in trackimport::track_map(false) {
        if f.as_str() == Some(folder) {
            return name;
        }
    }
    String::new()
}

pub struct TrackFolder {
    pub folder: String,
    pub own_folder: String,
    pub layout: String,
    /// archive key -> bytes, in the order they are written
    pub files: Vec<(String, Vec<u8>)>,
    /// role -> this track's container stem, in discovery order
    pub containers: Vec<(String, String)>,
}

impl TrackFolder {
    pub fn container_map(&self) -> HashMap<String, String> {
        self.containers.iter().cloned().collect()
    }
}

/// An imported track folder as an installable source (root .scene, .track
/// and containers\, exactly what EvoForge writes).
pub fn read_track_folder(src: &Path, folder: Option<&str>) -> Result<TrackFolder, String> {
    let own = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let folder = folder.filter(|f| !f.is_empty()).map(String::from).unwrap_or(own.clone());
    let root = src.join(format!("{own}.scene"));
    let data = src.join(format!("{own}.track"));
    if !root.is_file() || !data.is_file() {
        return Err(format!("{} has no {own}.scene / {own}.track", src.display()));
    }
    if folder != own && folder.chars().count() != own.chars().count() {
        return Err(format!("cannot install '{own}' as '{folder}': names differ in length and scenes reference their own folder"));
    }
    let load = |p: &Path| -> Result<Vec<u8>, String> {
        let b = std::fs::read(p).map_err(|e| e.to_string())?;
        Ok(if folder != own { replace_bytes(&b, own.as_bytes(), folder.as_bytes()) } else { b })
    };
    let mut files = vec![
        (arc(&format!("content\\tracks\\{folder}\\{folder}.scene")), load(&root)?),
        (arc(&format!("content\\tracks\\{folder}\\{folder}.track")), load(&data)?),
    ];
    let mut containers: Vec<(String, String)> = vec![];
    let mut layout = None;
    let cdir = src.join("containers");
    let mut names: Vec<String> = std::fs::read_dir(&cdir).map(|rd| {
        rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()
    }).unwrap_or_default();
    names.sort();
    let set = |k: &str, v: &str, c: &mut Vec<(String, String)>| {
        if let Some(e) = c.iter_mut().find(|(r, _)| r == k) { e.1 = v.to_string() } else { c.push((k.to_string(), v.to_string())) }
    };
    for name in names {
        let Some(stem) = name.strip_suffix(".scene") else { continue };
        files.push((arc(&format!("content\\tracks\\{folder}\\containers\\{name}")), load(&cdir.join(&name))?));
        if let Some(l) = stem.strip_prefix("layout_") {
            set("layout", stem, &mut containers);
            layout = Some(l.to_string());
        } else if let Some((_, role)) = ROLE_BY_NAME.iter().find(|(n, _)| *n == stem) {
            set(role, stem, &mut containers);
        }
    }
    let layout = layout.ok_or_else(|| format!("{} has no layout_*.scene container", src.display()))?;
    Ok(TrackFolder { folder, own_folder: own, layout, files, containers })
}

pub fn replace_bytes(hay: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    if from.is_empty() {
        return hay.to_vec();
    }
    let mut out = Vec::with_capacity(hay.len());
    let mut i = 0;
    while i < hay.len() {
        if hay[i..].starts_with(from) {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.push(hay[i]);
            i += 1;
        }
    }
    out
}

/// Which dedicated-server build this is: its exe's size and mtime (a game
/// update replaces the exe; nothing of ours writes to it).
fn server_build() -> String {
    let d = detect::effective_server_dir();
    let exe = ["AssettoCorsaEVOServer.exe", "AssettoCorsaEVOServer.stock.exe"].iter()
        .map(|n| Path::new(&d).join(n)).find(|p| p.is_file());
    exe.and_then(|p| std::fs::metadata(p).ok()).map(|m| {
        let t = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
        format!("{}:{t}", m.len())
    }).unwrap_or_default()
}

fn build_note(bak: &str) -> PathBuf {
    PathBuf::from(format!("{bak}.build"))
}

/// ⚠ Err stops the deploy: rewriting the archive after a failed backup left
/// no way back, and a half-copied .bak_pretrack then counted as "exists"
/// for ever (the copy now goes to a temp name and is renamed when whole).
fn backup(kspkg: &Path) -> Result<Value, String> {
    let bak = format!("{}.bak_pretrack", kspkg.display());
    match crate::safefs::backup_once(kspkg, Path::new(&bak)) {
        Ok(made) => {
            if made {
                // the build the backup belongs to (Restore checks it)
                let _ = std::fs::write(build_note(&bak), server_build());
            }
            Ok(json!({"backup": bak, "made": made}))
        }
        Err(e) => Err(format!("could not back up {} first ({e}) - nothing was changed", kspkg.display())),
    }
}

fn merge(a: &mut Value, b: Value) {
    if let (Some(a), Value::Object(b)) = (a.as_object_mut(), b) {
        a.extend(b);
    }
}

/// Install a track at its OWN paths into `kspkg` - no stock track is
/// overwritten. Handles an imported track folder (a slot-borrow package
/// with manifest.json still goes to the backend).
pub fn deploy_native_into(kspkg: &Path, pkg_dir: &Path, dry_run: bool, folder: Option<&str>,
                          display_name: Option<&str>, check_running: bool) -> Value {
    let _lock = match crate::archlock::lock() { Ok(g) => g, Err(e) => return json!({"ok": false, "error": e}) };
    if pkg_dir.join("manifest.json").is_file() {
        return json!({"ok": false, "error": "slot-borrow packages are handled by the backend"});
    }
    if !pkg_dir.is_dir() {
        return json!({"ok": false, "error": format!("not a folder: {}", pkg_dir.display())});
    }
    if check_running && server_running() {
        return json!({"ok": false, "error": "stop the server before deploying a track - it holds content.kspkg open and writing under it corrupts both"});
    }
    let src = match read_track_folder(pkg_dir, folder) {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    // ⚠ default to the name CLIENTS already use for this folder
    let display = display_name.filter(|d| !d.is_empty()).map(String::from)
        .or_else(|| Some(client_name(&src.own_folder)).filter(|n| !n.is_empty()))
        .unwrap_or_else(|| strip_name(&src.own_folder));
    let folder = src.folder.clone();
    let installed = client_track_folders();
    let mismatch = !installed.is_empty() && !installed.contains(&folder);
    if !kspkg.is_file() {
        return json!({"ok": false, "error": format!("server archive not found: {}", kspkg.display())});
    }
    let (tk, tc) = match (read_entry(kspkg, r"system\tracks.table"), read_entry(kspkg, r"system\track_containers.table")) {
        (Some(a), Some(b)) => (a, b),
        _ => return json!({"ok": false, "error": "archive has no system tables"}),
    };
    let edit = tracktables::upsert_track_row(&tk, &display, &folder, Some(TEMPLATE_TRACK)).and_then(|new_tk| {
        tracktables::upsert_container_rows(&tc, &display, &folder, &src.layout, &src.container_map(), TEMPLATE_TRACK)
            .map(|(new_tc, modes)| (new_tk, new_tc, modes))
    });
    let (new_tk, new_tc, modes) = match edit {
        Ok(x) => x,
        Err(e) => return json!({"ok": false, "error": format!("table edit failed: {e}")}),
    };
    let mut fnames: Vec<&String> = src.files.iter().map(|(k, _)| k).collect();
    fnames.sort();
    let containers: Map<String, Value> = src.containers.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    let mut plan = json!({
        "ok": true, "folder": folder, "package_folder": src.own_folder,
        "renamed": folder != src.own_folder, "client_folders": installed,
        "warning": if mismatch { json!(format!("no client here has imported '{folder}' - joiners whose folder differs cannot load it")) } else { Value::Null },
        "display_name": display, "layout": src.layout, "files": fnames,
        "containers": containers, "modes": modes,
        "tracks_table": format!("{} -> {} B", tk.len(), new_tk.len()),
        "containers_table": format!("{} -> {} B", tc.len(), new_tc.len()),
    });
    if dry_run {
        return plan;
    }
    let bak = match backup(kspkg) {
        Ok(b) => b,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let mut changes = src.files.clone();
    changes.push((r"system\tracks.table".into(), new_tk));
    changes.push((r"system\track_containers.table".into(), new_tc));
    let tmp = PathBuf::from(format!("{}.native_tmp", kspkg.display()));
    let t0 = Instant::now();
    let res = match kspkg_write::write_archive(kspkg, &tmp, &changes) {
        Ok(r) => r,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            let mut o = json!({"ok": false, "error": e, "hint": "the archive was backed up first - use Restore to undo"});
            merge(&mut o, bak);
            return o;
        }
    };
    let check = kspkg_write::verify(&tmp);
    if check["ok"] != true {
        let _ = std::fs::remove_file(&tmp);
        let mut o = json!({"ok": false, "error": "written archive failed verification", "detail": check});
        merge(&mut o, bak);
        return o;
    }
    if let Err(e) = std::fs::rename(&tmp, kspkg) {
        let _ = std::fs::remove_file(&tmp);
        let mut o = json!({"ok": false, "error": e.to_string()});
        merge(&mut o, bak);
        return o;
    }
    // loose AI lines: the archive cannot see a new .aisplinedata path, the
    // VFS loads one from disk
    let layouts = pkg_dir.join("layouts");
    let shipped = if layouts.is_dir() {
        let mut n = 0;
        for e in std::fs::read_dir(&layouts).into_iter().flatten().flatten() {
            let f = e.file_name().to_string_lossy().into_owned();
            if !f.ends_with(".aisplinedata") {
                continue;
            }
            if let Ok(dest) = splines::dest_path(&folder, &f) {
                if let Some(d) = dest.parent() {
                    let _ = std::fs::create_dir_all(d);
                }
                if std::fs::copy(e.path(), &dest).is_ok() {
                    n += 1;
                }
            }
        }
        json!({"copied": n, "from": layouts.to_string_lossy()})
    } else {
        splines::ship(&folder, "")
    };
    merge(&mut plan, json!({"seconds": (t0.elapsed().as_secs_f64() * 10.0).round() / 10.0,
                            "written": res, "verified": check, "splines": shipped}));
    merge(&mut plan, bak);
    plan
}

pub fn deploy_native(pkg_dir: &Path, dry_run: bool, folder: Option<&str>, display_name: Option<&str>) -> Value {
    deploy_native_into(&server_kspkg(), pkg_dir, dry_run, folder, display_name, true)
}

fn read_entry(archive: &Path, path: &str) -> Option<Vec<u8>> {
    let e = crate::kspkg::find_entry(archive, path).ok().flatten().or_else(|| {
        let t = path.to_lowercase();
        crate::kspkg::entries(archive).ok()?.into_iter().find(|e| e.path.to_lowercase() == t)
    })?;
    let mut f = std::fs::File::open(archive).ok()?;
    crate::kspkg::read_entry(&mut f, &e).ok()
}

/// Sidecar a joiner needs to register this track in THEIR tables.
pub fn pack_meta(folder: &str) -> Value {
    let mut display = display_name_for(folder);
    let (mut layout, mut containers) = ("layout".to_string(), Map::new());
    if let Ok(src) = read_track_folder(&Path::new(&trackimport::tracks_dir()).join(folder), Some(folder)) {
        layout = src.layout.clone();
        containers = src.containers.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
        let c = client_name(folder);
        if !c.is_empty() {
            display = c;
        }
    }
    json!({"folder": folder, "display_name": display, "layout": layout, "containers": containers})
}

fn table_write_ok(check: &Value) -> bool {
    // ⚠ only the records WE wrote are grounds to call our write bad; a
    // pre-existing header quirk elsewhere in the 120k-record client archive
    // is somebody else's
    let ours = [r"system\tracks.table", r"system\track_containers.table"];
    let our_bad = check.get("bad_headers").and_then(|b| b.as_array())
        .map(|b| b.iter().any(|p| ours.contains(&p.as_str().unwrap_or("")))).unwrap_or(false);
    check["sorted"] == true && check["unique"] == true && !our_bad
}

fn forget_track_map() {
    let _ = std::fs::remove_file(paths::data().join("track_map.json"));
}

/// Write this track into an archive's tracks + containers tables, in place.
pub fn register_track_into(pkg: &Path, folder: &str, meta: &Value, check_game: bool) -> Value {
    let _lock = match crate::archlock::lock() { Ok(g) => g, Err(e) => return json!({"ok": false, "error": e}) };
    let folder = folder.trim();
    if folder.is_empty() {
        return json!({"ok": false, "error": "no folder"});
    }
    let mut meta = meta.as_object().cloned().unwrap_or_default();
    let has = |m: &Map<String, Value>, k: &str| m.get(k).map(|v| !v.is_null() && v != "" && v != &json!({})).unwrap_or(false);
    if !has(&meta, "display_name") || !has(&meta, "containers") {
        let mut base = pack_meta(folder).as_object().cloned().unwrap_or_default();
        for (k, v) in meta.iter() {
            if !v.is_null() && v != "" && v != &json!({}) && v != &json!([]) {
                base.insert(k.clone(), v.clone());
            }
        }
        meta = base;
    }
    let s = |k: &str| meta.get(k).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let mut display = s("display_name");
    let layout = { let l = s("layout"); if l.is_empty() { "layout".to_string() } else { l } };
    let mut containers: HashMap<String, String> = meta.get("containers").and_then(|c| c.as_object())
        .map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string())).collect())
        .unwrap_or_default();
    if containers.is_empty() {
        containers.insert("layout".into(), "layout".into());
    }
    if display.is_empty() {
        display = display_name_for(folder);
    }
    if check_game && procs::game_running() {
        return json!({"ok": false, "needs_close": true, "error": "close the game, then fetch again — content.kspkg is locked while it is running"});
    }
    let (tk, tc) = match (read_entry(pkg, r"system\tracks.table"), read_entry(pkg, r"system\track_containers.table")) {
        (Some(a), Some(b)) => (a, b),
        _ => return json!({"ok": false, "error": "client archive has no system tables"}),
    };
    let edit = tracktables::upsert_track_row(&tk, &display, folder, Some(TEMPLATE_TRACK)).and_then(|new_tk| {
        tracktables::upsert_container_rows(&tc, &display, folder, &layout, &containers, TEMPLATE_TRACK)
            .map(|(new_tc, modes)| (new_tk, new_tc, modes))
    });
    let (new_tk, new_tc, modes) = match edit {
        Ok(x) => x,
        Err(e) => return json!({"ok": false, "error": format!("table edit failed: {e}")}),
    };
    let written = match kspkg_write::write_inplace(pkg, &[(r"system\tracks.table".into(), new_tk),
                                                       (r"system\track_containers.table".into(), new_tc)]) {
        Ok(w) => w,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let check = kspkg_write::verify(pkg);
    if !table_write_ok(&check) {
        // ⚠ put the old tables back: the bad ones were left on disk
        let undone = kspkg_write::undo_inplace(pkg, &written);
        return json!({"ok": false, "error": "client archive failed verification after the table write", "detail": check,
                      "rolled_back": undone.is_ok(), "rollback_error": undone.err()});
    }
    forget_track_map();
    json!({"ok": true, "folder": folder, "display_name": display, "layout": layout,
           "modes": modes, "written": written})
}

pub fn register_client_track(folder: &str, meta: &Value) -> Value {
    match client_kspkg() {
        Some(p) => register_track_into(Path::new(&p), folder, meta, true),
        None => json!({"ok": false, "error": "client content.kspkg not found"}),
    }
}

/// Take a track back out of an archive's tables, in place.
pub fn unregister_track_into(pkg: &Path, folder: &str, display_name: &str, check_game: bool) -> Value {
    let _lock = match crate::archlock::lock() { Ok(g) => g, Err(e) => return json!({"ok": false, "error": e}) };
    let folder = folder.trim();
    if folder.is_empty() {
        return json!({"ok": false, "error": "no folder"});
    }
    let display = [display_name.trim().to_string(), client_name(folder)].into_iter()
        .find(|d| !d.is_empty()).unwrap_or_else(|| display_name_for(folder));
    if check_game && procs::game_running() {
        return json!({"ok": false, "needs_close": true, "error": "close the game first - content.kspkg is locked while it is running"});
    }
    let (tk, tc) = match (read_entry(pkg, r"system\tracks.table"), read_entry(pkg, r"system\track_containers.table")) {
        (Some(a), Some(b)) => (a, b),
        _ => return json!({"ok": false, "error": "client archive has no system tables"}),
    };
    match tracktables::registered_names(&tk) {
        Ok(n) if !n.contains(&display) => {
            return json!({"ok": true, "folder": folder, "display_name": display, "already_absent": true});
        }
        Err(e) => return json!({"ok": false, "error": format!("table edit failed: {e}")}),
        _ => {}
    }
    // ⚠ only a row that points at THIS folder: removing by name alone could
    // take a stock track's rows out of the game
    match tracktables::rows_for_folder(&tk, folder) {
        Ok(rows) if rows.iter().any(|n| n.as_deref() == Some(display.as_str())) => {}
        Ok(_) => return json!({"ok": false, "error": format!("\"{display}\" belongs to another track, not {folder} - nothing was removed")}),
        Err(e) => return json!({"ok": false, "error": format!("table edit failed: {e}")}),
    }
    let edit = tracktables::remove_track_row(&tk, &display)
        .and_then(|a| tracktables::remove_container_rows(&tc, &display).map(|b| (a, b)));
    let (new_tk, new_tc) = match edit {
        Ok(x) => x,
        Err(e) => return json!({"ok": false, "error": format!("table edit failed: {e}")}),
    };
    let written = match kspkg_write::write_inplace(pkg, &[(r"system\tracks.table".into(), new_tk),
                                                       (r"system\track_containers.table".into(), new_tc)]) {
        Ok(w) => w,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    if !table_write_ok(&kspkg_write::verify(pkg)) {
        let undone = kspkg_write::undo_inplace(pkg, &written);
        return json!({"ok": false, "error": "client archive failed verification after the table write",
                      "rolled_back": undone.is_ok(), "rollback_error": undone.err()});
    }
    forget_track_map();
    json!({"ok": true, "folder": folder, "display_name": display, "written": written})
}

/// Imported tracks whose FILES are here but which the client table lost
/// (a game update replaces content.kspkg and with it those rows).
pub fn missing_from_client() -> Vec<String> {
    let known: std::collections::HashSet<String> = trackimport::track_map(false).values()
        .filter_map(|v| v.as_str().map(String::from)).collect();
    trackimport::importable().iter()
        .filter_map(|t| t.get("folder").and_then(|f| f.as_str()).filter(|f| !f.is_empty()).map(String::from))
        .filter(|f| !known.contains(f)).collect()
}

/// Re-register every loose custom track that a content update wiped from
/// `kspkg` (the SERVER's archive).
pub fn redeclare_tracks_into(kspkg: &Path, dry_run: bool, check_running: bool) -> Value {
    let _lock = match crate::archlock::lock() { Ok(g) => g, Err(e) => return json!({"ok": false, "error": e}) };
    if !kspkg.is_file() {
        return json!({"ok": false, "error": format!("server archive not found: {}", kspkg.display())});
    }
    if check_running && server_running() {
        return json!({"ok": false, "error": "stop the server before redeclaring tracks - it holds content.kspkg open and writing under it corrupts both"});
    }
    let Some(tk) = read_entry(kspkg, r"system\tracks.table") else {
        return json!({"ok": false, "error": r"archive has no system\tracks.table"});
    };
    let registered = match tracktables::registered_names(&tk) {
        Ok(r) => r,
        Err(e) => return json!({"ok": false, "error": format!("could not read tracks.table: {e}")}),
    };
    let mut missing = vec![];
    let folders = client_track_folders();
    for folder in &folders {
        if folder == "common_assets" {
            continue;
        }
        let display = display_name_for(folder);
        if registered.contains(&display) {
            continue;
        }
        match read_track_folder(&import_root().join(folder), Some(folder)) {
            Ok(info) => missing.push(json!({"folder": folder, "display_name": display,
                                            "own_folder": info.own_folder, "layout": info.layout, "ok": null})),
            Err(e) => missing.push(json!({"folder": folder, "display_name": display, "ok": false, "error": e})),
        }
    }
    if dry_run {
        return json!({"ok": true, "dry_run": true, "checked": missing.len(), "candidates": missing});
    }
    let mut results = vec![];
    for m in &missing {
        if m["ok"] == false {
            results.push(m.clone());
            continue;
        }
        let f = m["folder"].as_str().unwrap_or("");
        let r = deploy_native_into(kspkg, &import_root().join(f), false, Some(f),
                                   m["display_name"].as_str(), check_running);
        let mut row = m.clone();
        row["ok"] = json!(r.get("ok").and_then(|v| v.as_bool()).unwrap_or(false));
        row["detail"] = r;
        results.push(row);
    }
    let ok = results.iter().all(|r| r["ok"] != false);
    json!({"ok": ok,
           "redeclared": results.iter().filter(|r| r["ok"] == true).collect::<Vec<_>>(),
           "failed": results.iter().filter(|r| r["ok"] == false).collect::<Vec<_>>(),
           "already_registered": folders.len() as i64 - missing.len() as i64})
}

pub fn redeclare_tracks(dry_run: bool) -> Value {
    redeclare_tracks_into(&server_kspkg(), dry_run, true)
}

pub fn state() -> Value {
    let k = server_kspkg();
    let exists = k.is_file();
    let size = std::fs::metadata(&k).map(|m| (m.len() as f64 / 1048576.0 * 10.0).round() / 10.0).unwrap_or(0.0);
    json!({"kspkg": k.to_string_lossy(), "exists": exists,
           "size_mb": if exists { json!(size) } else { json!(0) },
           "backup": Path::new(&format!("{}.bak_pretrack", k.display())).is_file(),
           "server_running": server_running(),
           "note": "ACECM patches the server it runs, not the Steam copy"})
}

/// Put the pre-deploy archive back.
pub fn restore() -> Value {
    let _lock = match crate::archlock::lock() { Ok(g) => g, Err(e) => return json!({"ok": false, "error": e}) };
    let k = server_kspkg();
    let bak = format!("{}.bak_pretrack", k.display());
    if !Path::new(&bak).is_file() {
        return json!({"ok": false, "error": "no pre-deploy backup exists"});
    }
    if server_running() {
        return json!({"ok": false, "error": "stop the server first"});
    }
    // ⚠ a backup from BEFORE a game update would downgrade content.kspkg
    // under a newer server (version mismatch for every joiner)
    if let Ok(was) = std::fs::read_to_string(build_note(&bak)) {
        let now = server_build();
        if !was.trim().is_empty() && !now.is_empty() && was.trim() != now {
            return json!({"ok": false, "stale_backup": true,
                "error": "the dedicated server was updated after this backup was taken, so restoring it would put back the OLD game files. Nothing was restored - use Steam's 'Verify integrity of game files' on the dedicated server to get the stock archive back."});
        }
    }
    // ⚠ atomic: a copy that failed half way over the live archive left a
    // broken content.kspkg
    match crate::safefs::copy_atomic(Path::new(&bak), &k) {
        Ok(_) => json!({"ok": true, "restored_from": bak}),
        Err(e) => json!({"ok": false, "error": e.to_string()}),
    }
}

const REQUIRED: &[&str] = &["manifest.json", "containers.bin", "tracks_entry.bin"];

/// Is this a usable slot-borrow track package?
pub fn validate(pkg_dir: &Path) -> Value {
    if !pkg_dir.is_dir() {
        return json!({"ok": false, "error": "not a folder"});
    }
    let missing: Vec<&str> = REQUIRED.iter().copied().filter(|f| !pkg_dir.join(f).is_file()).collect();
    let has_override = pkg_dir.join("override").is_dir();
    let mut info = json!({"path": pkg_dir.to_string_lossy(), "missing": missing, "override": has_override});
    match std::fs::read(pkg_dir.join("manifest.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) {
        Some(man) => {
            info["display_name"] = man.get("display_name").cloned().unwrap_or(Value::Null);
            info["folder"] = man.get("folder").cloned().unwrap_or(Value::Null);
            let sm = man.get("slot_map").and_then(|v| v.as_object()).cloned().unwrap_or_default();
            info["files"] = json!(sm.len());
            let mut dirs: Vec<String> = sm.values().filter_map(|v| v.as_str())
                .map(|v| v.rsplit_once(['\\', '/']).map(|(d, _)| d.to_string()).unwrap_or_default())
                .collect::<std::collections::BTreeSet<_>>().into_iter().collect();
            dirs.truncate(3);
            info["host_slots"] = json!(dirs);
        }
        None => info["error"] = json!("manifest unreadable"),
    }
    if missing.contains(&"containers.bin") {
        info["note"] = json!("containers.bin missing - this package was built before that file existed and cannot be installed directly; rebuild it with build_track_package.py");
    }
    let ok = missing.is_empty() && has_override && info.get("error").is_none();
    info["ok"] = json!(ok);
    info
}

/// Find track packages in the usual places.
pub fn packages(extra: Option<&str>) -> Value {
    let mut roots = vec![paths::data().join("track_packages"),
                         Path::new(&detect::effective_server_dir()).join("track_packages"),
                         home().join("Downloads")];
    if let Some(e) = extra.filter(|e| !e.is_empty()) {
        roots.insert(0, PathBuf::from(e));
    }
    let mut seen = std::collections::HashSet::new();
    let mut out = vec![];
    for root in &roots {
        let Ok(rd) = std::fs::read_dir(root) else { continue };
        let mut names: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        for n in names {
            let d = root.join(&n);
            if !d.is_dir() || !seen.insert(d.clone()) {
                continue;
            }
            if d.join("manifest.json").is_file() {
                out.push(validate(&d));
            }
        }
    }
    json!({"packages": out, "roots": roots.iter().map(|r| r.to_string_lossy().into_owned()).collect::<Vec<_>>()})
}

/// The client-side twin of redeclare_tracks: a content update wipes the
/// CLIENT's tracks.table too, and single player reads THAT one.
pub fn redeclare_client_tracks(dry_run: bool) -> Value {
    let Some(pkg) = client_kspkg() else {
        return json!({"ok": false, "error": "client content.kspkg not found"});
    };
    if procs::game_running() {
        return json!({"ok": false, "needs_close": true, "error": "close the game before redeclaring client tracks - content.kspkg is locked while it is running"});
    }
    let _lock = match crate::archlock::lock() { Ok(g) => g, Err(e) => return json!({"ok": false, "error": e}) };
    let Some(tk) = read_entry(Path::new(&pkg), r"system\tracks.table") else {
        return json!({"ok": false, "error": r"client archive has no system\tracks.table"});
    };
    let registered = match tracktables::registered_names(&tk) {
        Ok(r) => r,
        Err(e) => return json!({"ok": false, "error": format!("could not read tracks.table: {e}")}),
    };
    let folders = client_track_folders();
    let missing: Vec<String> = folders.iter().filter(|f| *f != "common_assets").filter(|f| {
        let n = client_name(f);
        !registered.contains(if n.is_empty() { f.as_str() } else { n.as_str() })
    }).cloned().collect();
    if dry_run {
        return json!({"ok": true, "dry_run": true, "candidates": missing});
    }
    let results: Vec<Value> = missing.iter().map(|f| {
        let r = register_track_into(Path::new(&pkg), f, &json!({}), false);
        json!({"folder": f, "ok": r.get("ok").and_then(|v| v.as_bool()).unwrap_or(false), "detail": r})
    }).collect();
    let ok = results.iter().all(|r| r["ok"] == true);
    json!({"ok": ok,
           "redeclared": results.iter().filter(|r| r["ok"] == true).collect::<Vec<_>>(),
           "failed": results.iter().filter(|r| r["ok"] != true).collect::<Vec<_>>(),
           "already_registered": folders.len() as i64 - missing.len() as i64})
}
