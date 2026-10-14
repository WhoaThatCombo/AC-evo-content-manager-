//! Imported tracks on this machine - ported from contentsync.tracks_dir /
//! installed_tracks / track_map and tracks.importable (read side only).

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde_json::{json, Map, Value};

use crate::{config, kspkg, paths, pb, viewer};

const NOT_TRACKS: &[&str] = &["track_garage", "car_dealership"];

pub fn tracks_dir() -> String {
    let cfg = config::load();
    let c = config::get_str(&cfg, "client_tracks_dir");
    if !c.is_empty() {
        return c;
    }
    let home = std::env::var("USERPROFILE").map(PathBuf::from).unwrap_or_default();
    home.join("Saved Games").join("ACE").join("mods").join("content").join("tracks")
        .to_string_lossy().into_owned()
}

pub fn installed_tracks() -> Vec<String> {
    let d = tracks_dir();
    let mut v: Vec<String> = std::fs::read_dir(&d).map(|rd| {
        rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()
    }).unwrap_or_default();
    v.sort();
    v
}

fn mtime(m: &std::fs::Metadata) -> u64 {
    m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0)
}

/// Display name -> folder, from the CLIENT's own system\tracks.table.
/// Cached in <data>/track_map.json (shared, same stamp).
pub fn track_map(refresh: bool) -> Map<String, Value> {
    let Some(pkg) = viewer::package() else { return Map::new() };
    let cache = paths::data().join("track_map.json");
    let Ok(m) = std::fs::metadata(&pkg) else { return Map::new() };
    let mut stamp = format!("{}:{}", m.len(), mtime(&m));
    // also the imported-tracks folder: a track deleted by hand leaves the
    // archive untouched
    let d = tracks_dir();
    if let Ok(dm) = std::fs::metadata(&d) {
        if let Ok(rd) = std::fs::read_dir(&d) {
            stamp += &format!("|{}:{}", mtime(&dm), rd.count());
        }
    }
    if !refresh {
        if let Some(got) = std::fs::read(&cache).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) {
            if got.get("stamp").and_then(|v| v.as_str()) == Some(&stamp) {
                return got.get("tracks").and_then(|t| t.as_object()).cloned().unwrap_or_default();
            }
        }
    }
    let Some(out) = read_track_table(Path::new(&pkg)) else { return Map::new() };
    let _ = std::fs::write(&cache, serde_json::to_vec(&json!({"stamp": stamp, "tracks": out})).unwrap_or_default());
    out
}

fn read_track_table(pkg: &Path) -> Option<Map<String, Value>> {
    let e = kspkg::entries(pkg).ok()?.into_iter()
        .find(|e| e.path.to_lowercase() == "system\\tracks.table")?;
    let mut f = std::fs::File::open(pkg).ok()?;
    let blob = kspkg::read_entry(&mut f, &e).ok()?;
    let top = pb::walk(&blob).ok()?;
    let rows = top.first()?.val.bytes();
    let mut out = Map::new();
    for row in pb::walk(rows).ok()? {
        let (mut name, mut folder) = (None, None);
        for g in pb::walk(row.val.bytes()).ok()? {
            if g.num != 2 || g.wire != 2 {
                continue;
            }
            for h in pb::walk(g.val.bytes()).ok()? {
                if h.wire != 2 {
                    continue;
                }
                let s = String::from_utf8_lossy(h.val.bytes()).into_owned();
                if h.num == 1 {
                    name = Some(s);
                } else if h.num == 3 {
                    folder = Some(s.rsplit('\\').next().unwrap_or("").to_string());
                }
            }
        }
        if let (Some(n), Some(fo)) = (name, folder) {
            if !n.is_empty() && !fo.is_empty() && !NOT_TRACKS.contains(&fo.to_lowercase().as_str()) {
                out.insert(n, json!(fo));
            }
        }
    }
    Some(out)
}

/// layout + file count of an imported track folder, or the reason it is
/// not usable (tracks.read_track_folder, without reading the bytes).
fn read_track_folder(src: &Path) -> Result<(String, usize), String> {
    let own = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let root = src.join(format!("{own}.scene"));
    let data = src.join(format!("{own}.track"));
    if !root.is_file() || !data.is_file() {
        return Err(format!("{} has no {own}.scene / {own}.track", src.to_string_lossy()));
    }
    let cdir = src.join("containers");
    let mut files = 2;
    let mut layout = None;
    let mut names: Vec<String> = std::fs::read_dir(&cdir).map(|rd| {
        rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()
    }).unwrap_or_default();
    names.sort();
    for n in names {
        let Some(stem) = n.strip_suffix(".scene") else { continue };
        files += 1;
        if let Some(l) = stem.strip_prefix("layout_") {
            layout = Some(l.to_string());
        }
    }
    match layout {
        Some(l) => Ok((l, files)),
        None => Err(format!("{} has no layout_*.scene container", src.to_string_lossy())),
    }
}

/// Tracks already imported here, ready to deploy as-is.
pub fn importable() -> Vec<Value> {
    let root = tracks_dir();
    let known: std::collections::HashMap<String, String> = track_map(false).into_iter()
        .filter_map(|(n, f)| Some((f.as_str()?.to_string(), n))).collect();
    let mut out = vec![];
    if !Path::new(&root).is_dir() {
        return out;
    }
    for folder in installed_tracks() {
        let src = Path::new(&root).join(&folder);
        let mut item = json!({
            "path": src.to_string_lossy(), "folder": folder, "source": "imported",
            "display_name": known.get(&folder).cloned().unwrap_or(folder.clone()),
        });
        match read_track_folder(&src) {
            Ok((layout, files)) => {
                item["layout"] = json!(layout);
                item["files"] = json!(files);
                item["ok"] = json!(true);
            }
            Err(e) => {
                item["ok"] = json!(false);
                item["error"] = json!(e);
            }
        }
        out.push(item);
    }
    out
}
