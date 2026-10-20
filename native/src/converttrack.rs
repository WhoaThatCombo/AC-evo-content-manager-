//! Convert an Assetto Corsa (AC1) track with the bundled converter and
//! install it.
//!
//! `aceconv track <ac1 dir> <work> --id <id>` writes <work>\<id>\
//! (static_mesh, textures, containers, ...) plus ui\track_decl.json: the
//! exact tracks.table / track_containers.table rows. The converter never
//! writes the game's files. Installing - only when the user asks for it -
//! copies the folder into the game's mods and swaps those rows into
//! content.kspkg (old rows for the same folder or name, and those of an
//! earlier conversion it replaces, go first), through the same backed-up,
//! verified, roll-back-able writer as track deploy.

use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::convert::{check, converter, default_ids, game_content, history, history_id, kn5_protected, now, run_converter, s, save_history, settings, valid_folder, valid_id, Job, JOB};
use crate::{logs, mods, procs};

fn layouts(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir).map(|r| r.flatten().filter(|e| {
        let n = e.file_name().to_string_lossy().to_lowercase();
        e.path().is_dir() && n != "ui" && n != "skins" && e.path().join("data").join("surfaces.ini").is_file()
    }).map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    v.sort();
    v
}

fn ui_track(dir: &Path, layout: Option<&str>) -> Map<String, Value> {
    let mut m = Map::new();
    let cands: Vec<PathBuf> = layout.map(|l| dir.join("ui").join(l).join("ui_track.json")).into_iter().chain([dir.join("ui").join("ui_track.json")]).collect();
    for p in cands {
        let Ok(b) = std::fs::read(&p) else { continue };
        let t: String = String::from_utf8_lossy(&b).trim_start_matches('\u{feff}').chars().map(|c| if c == '\t' || c == '\r' || c == '\n' { ' ' } else { c }).collect();
        let v: Value = serde_json::from_str(&t).unwrap_or(Value::Null);
        for k in ["name", "country", "city", "length", "pitboxes", "author", "description", "tags", "width", "run", "version", "url"] {
            if let Some(x) = v.get(k) {
                m.insert(k.into(), match x {
                    Value::String(st) => json!(st.trim()),
                    Value::Array(a) => json!(a.iter().filter_map(|t| t.as_str()).map(|t| t.trim()).collect::<Vec<_>>().join(", ")),
                    o => json!(o.to_string()),
                });
            }
        }
        break;
    }
    m
}

fn root() -> PathBuf {
    Path::new(&s(&settings(), "ac1_root")).join("content").join("tracks")
}

static TRACK_CACHE: crate::convert::ListCache = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// GET /api/convert/tracks
pub fn tracks() -> Value {
    let root = root();
    let Ok(rd) = std::fs::read_dir(&root) else {
        return json!({"ok": false, "error": format!("No Assetto Corsa tracks folder at {}", root.display()), "tracks": []});
    };
    let hist = history();
    let mut out = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        let Some(base) = crate::convert::cached_entry(&p, &TRACK_CACHE, |p| {
            let ls = layouts(p);
            if ls.is_empty() && !p.join("data").join("surfaces.ini").is_file() {
                return None;
            }
            let mut t = ui_track(p, ls.first().map(|x| x.as_str()));
            t.insert("layouts".into(), json!(ls));
            t.insert("protected".into(), json!(track_kn5s(p).iter().any(|k| kn5_protected(k))));
            Some(t)
        }) else { continue };
        let folder = e.file_name().to_string_lossy().into_owned();
        let mut t = base;
        t.insert("folder".into(), json!(folder));
        let key = format!("track:{folder}");
        t.insert("last".into(), hist.get(&key).cloned().unwrap_or(Value::Null));
        out.push(t);
    }
    let folders: Vec<String> = out.iter().filter_map(|t| t.get("folder").and_then(|v| v.as_str()).map(String::from)).collect();
    let ids = default_ids("tracks", &folders);
    let mut out: Vec<Value> = out.into_iter().map(|mut t| {
        let f = t["folder"].as_str().unwrap_or("").to_string();
        let id = history_id(&hist, &format!("track:{f}"), "tracks").or_else(|| ids.get(&f).cloned()).unwrap_or_default();
        t.insert("id".into(), json!(id));
        Value::Object(t)
    }).collect();
    let key = |v: &Value| v["name"].as_str().or(v["folder"].as_str()).unwrap_or("").to_lowercase();
    out.sort_by_key(key);
    json!({"ok": true, "root": root.to_string_lossy(), "tracks": out})
}

/// The track's picture: ui/<layout>/preview.png, ui/preview.png, outline.
pub fn preview(folder: &str) -> Option<PathBuf> {
    image(folder, "", false)
}

/// ui/<layout>/{preview,outline}.png (any layout when none is named), then
/// ui/{preview,outline}.png; the preview falls back to the outline.
pub fn image(folder: &str, layout: &str, outline: bool) -> Option<PathBuf> {
    if !valid_folder(folder) || (!layout.is_empty() && !valid_folder(layout)) {
        return None;
    }
    let dir = root().join(folder);
    let lays: Vec<String> = if layout.is_empty() { layouts(&dir) } else { vec![layout.to_string()] };
    let names: &[&str] = if outline { &["outline.png"] } else { &["preview.png", "outline.png"] };
    let mut c: Vec<PathBuf> = Vec::new();
    for n in names {
        c.extend(lays.iter().map(|l| dir.join("ui").join(l).join(n)));
        c.push(dir.join("ui").join(n));
    }
    c.into_iter().find(|p| p.is_file())
}

/// Every .kn5 of a track (models_*.ini can point into sub-folders).
fn track_kn5s(dir: &Path) -> Vec<PathBuf> {
    fn walk(d: &Path, depth: u32, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(d) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                // skins / ui / extension / ai / data never hold models and can
                // hold hundreds of files (the panel waited on them)
                let n = e.file_name().to_string_lossy().to_lowercase();
                if depth < 3 && !matches!(n.as_str(), "skins" | "ui" | "extension" | "ai" | "data" | "sfx" | "texture" | "textures") {
                    walk(&p, depth + 1, out);
                }
            } else if p.extension().map(|x| x.eq_ignore_ascii_case("kn5")).unwrap_or(false) {
                out.push(p);
            }
        }
    }
    let mut v = Vec::new();
    walk(dir, 0, &mut v);
    v.sort();
    v
}

/// The folder to convert from: the AC1 track, or a replacement copy the user
/// points at (say an unencrypted copy its author sent).
fn source_dir(body: &Value, folder: &str) -> Result<PathBuf, String> {
    let src = body.get("source").and_then(|v| v.as_str()).unwrap_or("").trim().trim_matches('"').to_string();
    if src.is_empty() {
        return Ok(root().join(folder));
    }
    let p = PathBuf::from(&src);
    if !p.is_absolute() || !p.is_dir() {
        return Err(format!("{src} is not a folder"));
    }
    if layouts(&p).is_empty() && !p.join("data").join("surfaces.ini").is_file() {
        return Err(format!("{src} doesn't look like an Assetto Corsa track folder (no data\\surfaces.ini)"));
    }
    Ok(p)
}

/// GET /api/convert/track_info?track=<folder>[&source=<dir>]: everything
/// worth seeing before a conversion.
pub fn info(folder: &str, source: &str) -> Value {
    if !valid_folder(folder) {
        return json!({"ok": false, "error": "pick a track"});
    }
    let dir = match source_dir(&json!({"source": source}), folder) {
        Ok(d) => d,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    if !dir.is_dir() {
        return json!({"ok": false, "error": format!("{} not found", dir.display())});
    }
    let ls = layouts(&dir);
    let lays: Vec<Value> = if ls.is_empty() {
        vec![json!({"layout": "", "ui": ui_track(&dir, None), "outline": image(folder, "", true).is_some()})]
    } else {
        ls.iter().map(|l| json!({"layout": l, "ui": ui_track(&dir, Some(l)), "outline": image(folder, l, true).is_some()})).collect()
    };
    let mut total = 0u64;
    let files: Vec<Value> = track_kn5s(&dir).iter().map(|p| {
        let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        total += size;
        json!({"name": p.strip_prefix(&dir).unwrap_or(p).to_string_lossy(), "size": size, "protected": kn5_protected(p)})
    }).collect();
    let locked = files.iter().filter(|f| f["protected"].as_bool() == Some(true)).count();
    let ext = dir.join("extension").join("ext_config.ini").is_file();
    // track skins: skins\cm_skins\<name> (Content Manager) and skins\<name>
    let mut skins: Vec<String> = Vec::new();
    for base in [dir.join("skins").join("cm_skins"), dir.join("skins")] {
        if let Ok(rd) = std::fs::read_dir(&base) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if e.path().is_dir() && n != "cm_skins" && !skins.contains(&n) {
                    skins.push(n);
                }
            }
        }
    }
    skins.sort();
    // the converter fills placeholder textures from the first Content
    // Manager skin (by name), and reads the others not at all
    let mut cm: Vec<String> = std::fs::read_dir(dir.join("skins").join("cm_skins")).map(|r| r.flatten().filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    cm.sort();
    json!({"ok": true, "folder": folder, "dir": dir.to_string_lossy(), "source": !source.trim().is_empty(),
        "layouts": lays, "files": files, "kn5_bytes": total, "protected_files": locked, "csp_config": ext, "skins": skins,
        "skin_used": cm.first(), "earlier": crate::convert::earlier(Path::new(&mods::client_mods_dir()), "tracks", &dir)})
}

/// POST /api/convert/track_open {track, file?, source?}: show the track
/// folder (or select one of its files) in Explorer.
pub fn open(body: &Value) -> Value {
    let folder = body.get("track").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if !valid_folder(&folder) {
        return json!({"ok": false, "error": "pick a track"});
    }
    let dir = match source_dir(body, &folder) {
        Ok(d) => d,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let file = body.get("file").and_then(|v| v.as_str()).unwrap_or("");
    // only a file inside the track, no climbing out
    let target = if file.is_empty() {
        None
    } else if file.contains("..") || file.contains(':') || !dir.join(file).is_file() {
        return json!({"ok": false, "error": "no such file"});
    } else {
        Some(dir.join(file))
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

fn unhex(st: &str) -> Option<Vec<u8>> {
    if st.len() % 2 != 0 {
        return None;
    }
    (0..st.len()).step_by(2).map(|i| u8::from_str_radix(&st[i..i + 2], 16).ok()).collect()
}

fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<u64> {
    std::fs::create_dir_all(dst)?;
    let mut n = 0;
    for e in std::fs::read_dir(src)?.flatten() {
        let (a, b) = (e.path(), dst.join(e.file_name()));
        if a.is_dir() {
            n += copy_dir(&a, &b)?;
        } else {
            std::fs::copy(&a, &b)?;
            n += 1;
        }
    }
    Ok(n)
}

/// Edit both tables in place: drop rows `drop_*` matches, add `add_*`.
fn write_tables(drop_track: &dyn Fn(&[u8]) -> bool, drop_cont: &dyn Fn(&[u8]) -> bool, add_track: &[Vec<u8>], add_cont: &[Vec<u8>]) -> Result<Value, String> {
    let pkg = PathBuf::from(crate::trackdeploy::client_kspkg().ok_or("client content.kspkg not found")?);
    let _lock = crate::archlock::lock()?;
    let (tk, tc) = match (crate::trackdeploy::read_entry(&pkg, r"system\tracks.table"), crate::trackdeploy::read_entry(&pkg, r"system\track_containers.table")) {
        (Some(a), Some(b)) => (a, b),
        _ => return Err("client archive has no system tables".into()),
    };
    let new_tk = crate::tracktables::replace_rows_exact(&tk, drop_track, add_track)?;
    let new_tc = crate::tracktables::replace_rows_exact(&tc, drop_cont, add_cont)?;
    let written = crate::kspkg_write::write_inplace(&pkg, &[(r"system\tracks.table".into(), new_tk), (r"system\track_containers.table".into(), new_tc)])?;
    let check = crate::kspkg_write::verify(&pkg);
    if !crate::trackdeploy::table_write_ok(&check) {
        let undone = crate::kspkg_write::undo_inplace(&pkg, &written);
        return Err(format!("content.kspkg failed verification after the table write (rolled back: {})", undone.is_ok()));
    }
    crate::trackdeploy::forget_track_map();
    Ok(json!({"backup": written.get("backup").cloned().unwrap_or(Value::Null)}))
}

/// Rows of the track ids `ids` (by folder) or named `name`.
fn matchers(ids: &[String], name: &str) -> (impl Fn(&[u8]) -> bool, impl Fn(&[u8]) -> bool) {
    let folders: Vec<String> = ids.iter().map(|id| format!("content\\tracks\\{id}").to_lowercase()).collect();
    let needles: Vec<String> = folders.iter().map(|f| format!("{f}\\")).collect();
    let low = name.to_lowercase();
    let low2 = low.clone();
    let t = move |r: &[u8]| {
        let (n, b) = crate::tracktables::track_row_name_base(r);
        b.map(|b| folders.contains(&b.to_lowercase())).unwrap_or(false) || (!low.is_empty() && n.map(|n| n.to_lowercase() == low).unwrap_or(false))
    };
    let c = move |r: &[u8]| {
        let (n, paths) = crate::tracktables::container_row_name_paths(r);
        paths.iter().any(|p| { let p = p.to_lowercase(); needles.iter().any(|x| p.contains(x)) }) || (!low2.is_empty() && n.map(|n| n.to_lowercase() == low2).unwrap_or(false))
    };
    (t, c)
}

/// Swap the declaration rows into the client content.kspkg; the rows of
/// `also` (earlier conversions this one replaces) go too.
fn declare(decl: &Value, id: &str, also: &[String]) -> Result<Value, String> {
    let name = decl["display_name"].as_str().unwrap_or("");
    let row = decl["tracks_table_row"].as_str().and_then(unhex).ok_or("declaration without a tracks.table row")?;
    let crs: Vec<Vec<u8>> = decl["container_rows"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().and_then(unhex)).collect()).unwrap_or_default();
    if crs.is_empty() {
        return Err("declaration without container rows".into());
    }
    let mut ids = vec![id.to_string()];
    ids.extend(also.iter().cloned());
    // rows go by folder only: a name match could be another track's
    // (install_built refuses a clashing name first)
    let _ = name;
    let (t, c) = matchers(&ids, "");
    write_tables(&t, &c, &[row], &crs)
}

/// Another installed track (not one of `ours`) already called `name` in
/// tracks.table: its folder. Two tracks of one name break the game's lists.
fn name_clash(name: &str, ours: &[String]) -> Result<Option<String>, String> {
    let pkg = crate::trackdeploy::client_kspkg().ok_or("client content.kspkg not found")?;
    let tk = crate::trackdeploy::read_entry(Path::new(&pkg), r"system\tracks.table").ok_or("client archive has no tracks.table")?;
    let mine: Vec<String> = ours.iter().map(|i| format!(r"content\tracks\{i}").to_lowercase()).collect();
    Ok(crate::tracktables::track_rows(&tk)?.into_iter()
        .find(|(n, b)| n.trim().eq_ignore_ascii_case(name.trim()) && !mine.contains(&b.to_lowercase()))
        .map(|(_, b)| b))
}

/// The declaration's `ui_files` as (source, destination): from = relative
/// to the track folder, to = relative to the mods folder (the track picture
/// and map under uiresources\images\...). Anything that would climb out of
/// either, or land outside uiresources, is skipped.
fn ui_files(decl: &Value, track_dir: &Path) -> Vec<(PathBuf, PathBuf)> {
    let mods = PathBuf::from(mods::client_mods_dir());
    let safe = |r: &str| !r.is_empty() && !r.contains(':') && !r.starts_with(['\\', '/'])
        && r.split(['\\', '/']).all(|p| !p.is_empty() && p != "." && p != "..");
    let local = |r: &str| r.replace('/', "\\");
    decl["ui_files"].as_array().map(|a| a.iter().filter_map(|f| {
        let (from, to) = (f["from"].as_str()?, f["to"].as_str()?);
        (safe(from) && safe(to) && local(to).to_lowercase().starts_with(r"uiresources\"))
            .then(|| (track_dir.join(local(from)), mods.join(local(to))))
    }).collect()).unwrap_or_default()
}

/// Take a converted track's UI files out of the mods folder.
fn remove_ui_files(track_dir: &Path) -> usize {
    let Some(decl) = std::fs::read(track_dir.join("ui").join("track_decl.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else { return 0 };
    ui_files(&decl, track_dir).iter().filter(|(_, to)| std::fs::remove_file(to).is_ok()).count()
}

/// Install a converted track folder `built` as `id`: name check, files into
/// the mods, its UI images, then the declaration rows (and those of the
/// earlier conversions in `replace`, which go). Ok = (what was done,
/// replaced folders).
pub fn install_built(built: &Path, id: &str, replace: &[String]) -> Result<(Value, Vec<String>), String> {
    if procs::game_running() {
        return Err("the game is running - close it first (installing changes content.kspkg)".into());
    }
    let decl: Value = serde_json::from_slice(&std::fs::read(built.join("ui").join("track_decl.json")).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    if decl["track_id"].as_str() != Some(id) {
        return Err(format!("{} declares track {:?}, not {id}", built.display(), decl["track_id"].as_str().unwrap_or("")));
    }
    // only earlier conversions (they carry a declaration) are replaced
    let old: Vec<String> = replace.iter().filter(|o| converted_here(o).is_some()).cloned().collect();
    let name = decl["display_name"].as_str().unwrap_or("").to_string();
    let mut ours = vec![id.to_string()];
    ours.extend(old.iter().cloned());
    if let Some(other) = name_clash(&name, &ours)? {
        return Err(format!("another installed track ({other}) is already called \"{name}\" - remove it (or tick Replace when it is an earlier conversion) and install again"));
    }
    let dst = tracks_dir().join(id);
    if dst.exists() {
        if converted_here(id).is_none() {
            return Err(format!("{} exists and was not made by the converter - not replacing it", dst.display()));
        }
        remove_ui_files(&dst);
        std::fs::remove_dir_all(&dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    }
    let n = copy_dir(built, &dst).map_err(|e| format!("copy to {}: {e}", dst.display()))?;
    let mut ui = Vec::new();
    for (from, to) in ui_files(&decl, &dst) {
        if !from.is_file() {
            return Err(format!("the declaration lists {} but it is not there", from.display()));
        }
        if let Some(p) = to.parent() {
            std::fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
        }
        std::fs::copy(&from, &to).map_err(|e| format!("copy to {}: {e}", to.display()))?;
        ui.push(to.to_string_lossy().into_owned());
    }
    let mut d = declare(&decl, id, &old)?;
    let mut replaced = Vec::new();
    for o in &old {
        if let Some(p) = converted_here(o) {
            remove_ui_files(&p);
            if std::fs::remove_dir_all(&p).is_ok() {
                replaced.push(p.to_string_lossy().into_owned());
            }
        }
    }
    d["files"] = json!(n);
    d["ui_files"] = json!(ui);
    d["to"] = json!(dst.to_string_lossy());
    logs::info(format!("convert: installed track {id} ({n} files, {} UI files, replaced {replaced:?})", ui.len()));
    Ok((d, replaced))
}

/// POST /api/convert/track_install {id, dir?, replace?}: install an already
/// converted track - <work>\<id>, or `dir` (a converted folder declaring it).
pub fn install_api(body: &Value) -> Value {
    let id = body.get("id").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if !valid_id("tracks", &id) {
        return json!({"ok": false, "error": "bad track id"});
    }
    let replace: Vec<String> = body.get("replace").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str()).map(|x| x.trim().to_string())
        .filter(|x| *x != id && valid_folder(x) && x.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')).collect()).unwrap_or_default();
    let dir = body.get("dir").and_then(|v| v.as_str()).map(|s| s.trim().trim_matches('"').to_string()).filter(|s| !s.is_empty());
    let built = match dir {
        Some(d) if Path::new(&d).is_absolute() => PathBuf::from(d),
        Some(_) => return json!({"ok": false, "error": "dir must be an absolute folder"}),
        None => PathBuf::from(s(&settings(), "out_dir")).join(&id),
    };
    if JOB.lock().unwrap().state == "running" {
        return json!({"ok": false, "error": "a conversion is running"});
    }
    match install_built(&built, &id, &replace) {
        Ok((d, rep)) => {
            let mut h = history();
            for v in h.values_mut() {
                if v.get("id").and_then(|x| x.as_str()) == Some(id.as_str()) {
                    v["installed"] = json!([d.clone()]);
                }
            }
            save_history(&h);
            json!({"ok": true, "installed": d, "replaced": rep})
        }
        Err(e) => json!({"ok": false, "error": e}),
    }
}

fn tracks_dir() -> PathBuf {
    PathBuf::from(mods::client_mods_dir()).join("content").join("tracks")
}

/// A converted track's folder in the mods (it carries ui\track_decl.json).
fn converted_here(id: &str) -> Option<PathBuf> {
    let d = tracks_dir().join(id);
    d.join("ui").join("track_decl.json").is_file().then_some(d)
}

/// POST /api/convert/track_start {track, id, install, source?, replace: [old ids]}
pub fn start(body: &Value) -> Value {
    let track = body.get("track").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let id = body.get("id").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let install = body.get("install").and_then(|v| v.as_bool()).unwrap_or(true);
    let replace: Vec<String> = body.get("replace").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str()).map(|x| x.trim().to_string())
        .filter(|x| *x != id && valid_folder(x) && x.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')).collect()).unwrap_or_default();
    if !valid_folder(&track) {
        return json!({"ok": false, "error": "pick a track"});
    }
    if !valid_id("tracks", &id) {
        return json!({"ok": false, "error": "a track id is 3 to 16 characters of a-z, 0-9 and _, starting with a letter"});
    }
    if install && procs::game_running() {
        return json!({"ok": false, "error": "Close the game first: installing a track changes content.kspkg, which the game keeps open."});
    }
    let st = settings();
    let ac1 = match source_dir(body, &track) {
        Ok(d) => d,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let Some(conv) = converter() else {
        return json!({"ok": false, "error": "the converter is missing from this build"});
    };
    let out = PathBuf::from(s(&st, "out_dir"));
    if !ac1.is_dir() {
        return json!({"ok": false, "error": format!("AC1 track not found: {}", ac1.display())});
    }
    if !game_content().is_file() {
        return json!({"ok": false, "error": "Assetto Corsa EVO's content.kspkg was not found - set the game folder in Settings"});
    }
    if track_kn5s(&ac1).iter().any(|k| kn5_protected(k)) {
        return json!({"ok": false, "error": "This track is protected by its author (CSP encryption), so it can't be converted. If the author sent you an unencrypted copy, point \"Convert from\" at it."});
    }
    let _ = std::fs::create_dir_all(&out);
    {
        let mut j = JOB.lock().unwrap();
        if j.state == "running" {
            return json!({"ok": false, "error": format!("already converting {}", j.car)});
        }
        *j = Job::running(format!("track:{track}"), id.clone());
    }
    let args = vec!["track".to_string(), ac1.to_string_lossy().into_owned(), out.to_string_lossy().into_owned(), "--id".into(), id.clone()];
    logs::info(format!("convert track: {track} -> {id}"));
    std::thread::spawn(move || {
        let Some((code, secs, cancelled)) = run_converter(&conv, &args, &out) else { return };
        let built = out.join(&id);
        let decl_path = built.join("ui").join("track_decl.json");
        let mut res = Map::new();
        let mut ok = code == 0 && decl_path.is_file() && !cancelled;
        // never install what the validator rejects
        if ok {
            let c = check(&conv, &["check-track".into(), built.to_string_lossy().into_owned(), game_content().to_string_lossy().into_owned()]);
            if c["ok"] != true {
                ok = false;
                res.insert("error".into(), json!("the converted track failed the pre-install check - not installed (see Details)"));
            }
            res.insert("check".into(), c);
        }
        res.insert("code".into(), json!(code));
        res.insert("secs".into(), json!((secs * 10.0).round() / 10.0));
        res.insert("folder".into(), json!(built.to_string_lossy()));
        let mut installed = Vec::new();
        let mut replaced = Vec::new();
        if ok && install {
            let r = install_built(&built, &id, &replace).map(|(d, rep)| { replaced.extend(rep); d });
            match r {
                Ok(v) => installed.push(v),
                Err(e) => {
                    ok = false;
                    res.insert("error".into(), json!(e));
                }
            }
        } else if !ok {
            res.entry("error").or_insert(json!(if cancelled { "cancelled".to_string() } else { format!("converter exited with code {code}") }));
        }
        res.insert("ok".into(), json!(ok));
        res.insert("installed".into(), json!(installed));
        res.insert("replaced".into(), json!(replaced));
        let result = Value::Object(res);
        logs::info(format!("convert track: {id}: ok={ok} {secs:.0} s"));
        let key = {
            let mut j = JOB.lock().unwrap();
            j.state = if ok { "done" } else if cancelled { "cancelled" } else { "error" };
            j.secs = secs;
            j.pid = 0;
            j.result = result.clone();
            j.car.clone()
        };
        if cancelled {
            return;
        }
        let mut h = history();
        h.insert(key, json!({"id": id, "scheme": 2, "at": now(), "ok": ok, "secs": (secs * 10.0).round() / 10.0, "installed": installed,
            "check": result.get("check").map(|c| json!({"errors": c["errors"], "warnings": c["warnings"]}))}));
        save_history(&h);
    });
    json!({"ok": true, "started": true})
}

/// POST /api/convert/track_uninstall {id}: rows out of content.kspkg,
/// folder out of the mods.
pub fn uninstall(body: &Value) -> Value {
    let id = body.get("id").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if !valid_folder(&id) || !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
        return json!({"ok": false, "error": "bad id"});
    }
    if procs::game_running() {
        return json!({"ok": false, "error": "Close the game first."});
    }
    let Some(dst) = converted_here(&id) else {
        return json!({"ok": false, "error": format!("{} is not a converted track", tracks_dir().join(&id).display())});
    };
    let (t, c) = matchers(&[id.clone()], "");
    if let Err(e) = write_tables(&t, &c, &[], &[]) {
        return json!({"ok": false, "error": e});
    }
    let ui = remove_ui_files(&dst);
    let removed = std::fs::remove_dir_all(&dst).is_ok();
    let mut h = history();
    for v in h.values_mut() {
        if v.get("id").and_then(|x| x.as_str()) == Some(id.as_str()) {
            v["installed"] = json!([]);
        }
    }
    save_history(&h);
    logs::info(format!("convert: removed track {id}"));
    json!({"ok": true, "removed_folder": removed, "removed_ui_files": ui})
}

// ------------------------------------------------ sessions on converted tracks --

/// A converted track's declaration (<mods>\content\tracks\<folder>\ui\track_decl.json).
pub fn decl_of(folder: &str) -> Option<Value> {
    if !valid_folder(folder) {
        return None;
    }
    let p = tracks_dir().join(folder).join("ui").join("track_decl.json");
    serde_json::from_slice(&std::fs::read(p).ok()?).ok()
}

/// The sessions a converted track declares, one per layout: {track (the
/// tracks.table name), layout (its layout_name), event_names {race,
/// time_attack, practice, hotstint}, length_m}. These, never the AC1 folder
/// names, are what a game mode save must name.
pub fn decl_sessions(folder: &str) -> Option<Vec<Value>> {
    let d = decl_of(folder)?;
    let track = d["display_name"].as_str().unwrap_or("").to_string();
    let out: Vec<Value> = d["layouts"].as_array()?.iter().filter_map(|l| {
        let layout = l["layout_name"].as_str().or(l["name"].as_str())?.to_string();
        let ev = l.get("event_names").filter(|e| e.is_object())?.clone();
        Some(json!({"track": track, "layout": layout, "event_names": ev, "length_m": l.get("length_m").cloned().unwrap_or(Value::Null)}))
    }).collect();
    (!track.is_empty() && !out.is_empty()).then_some(out)
}

/// The event name of a session for a game mode (GameModeType_PRACTICE, ...).
pub fn event_for_mode(event_names: &Value, mode: &str) -> Option<String> {
    let m = mode.to_uppercase();
    let key = if m.contains("TIME_ATTACK") { "time_attack" } else if m.contains("HOT") { "hotstint" }
        else if m.contains("RACE") { "race" } else { "practice" };
    event_names.get(key).and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(String::from)
}

/// Does the client's track_containers.table hold an event of that name
/// (with that layout name, when the row carries one)? None = unreadable.
pub fn tables_have_event(event: &str, layout: &str) -> Option<bool> {
    let pkg = crate::trackdeploy::client_kspkg()?;
    let tc = crate::trackdeploy::read_entry(Path::new(&pkg), r"system\track_containers.table")?;
    let rows = crate::tracktables::container_rows_named(&tc).ok()?;
    Some(rows.iter().any(|(n, l)| n.eq_ignore_ascii_case(event) && (layout.is_empty() || l.is_empty() || l.eq_ignore_ascii_case(layout))))
}
