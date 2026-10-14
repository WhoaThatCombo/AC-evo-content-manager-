//! AI spline files - ported from acecm/splines.py.
//!
//! The server's content.kspkg has no .aisplinedata; the client archive has
//! every stock layout and custom imports keep theirs loose. The engine (and
//! the tracker) read loose files from
//!   <server>/content/tracks/<folder>/layouts/<layout>.{ideal_line,pitlane}.aisplinedata
//! so: find them wherever they are, copy them there.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde_json::{json, Map, Value};

use crate::{content, detect, kspkg, paths, trackimport, viewer};

pub fn server_tracks() -> PathBuf {
    Path::new(&detect::effective_server_dir()).join("content").join("tracks")
}

fn kind(name: &str) -> &'static str {
    let n = name.to_lowercase();
    if n.contains(".ideal_line.") { "ideal_line" } else if n.contains(".pitlane.") { "pitlane" } else { "other" }
}

fn stem(name: &str) -> String {
    name.split('.').next().unwrap_or("").to_string()
}

fn sv(r: &Value, k: &str) -> String {
    r.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string()
}

/// Stock splines inside the client's content.kspkg, cached on size+mtime
/// in <data>/spline_index.json (shared).
pub fn archive_index(refresh: bool) -> Vec<Value> {
    let Some(pkg) = viewer::package() else { return vec![] };
    let Ok(m) = std::fs::metadata(&pkg) else { return vec![] };
    let mt = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
    let stamp = format!("{}:{mt}", m.len());
    let cache = paths::data().join("spline_index.json");
    if !refresh {
        if let Some(got) = std::fs::read(&cache).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) {
            if got.get("stamp").and_then(|v| v.as_str()) == Some(&stamp) {
                return got.get("entries").and_then(|e| e.as_array()).cloned().unwrap_or_default();
            }
        }
    }
    let mut out = vec![];
    for e in kspkg::entries(Path::new(&pkg)).unwrap_or_default() {
        let parts: Vec<&str> = e.path.split('\\').collect();
        if parts.len() < 5 || parts[0].to_lowercase() != "content" || parts[1].to_lowercase() != "tracks" {
            continue;
        }
        let f = parts[parts.len() - 1];
        if !f.to_lowercase().ends_with(".aisplinedata") || kind(f) == "other" {
            continue;
        }
        out.push(json!({"folder": parts[2], "layout": stem(f), "kind": kind(f), "file": f,
                        "archive": e.path, "size": e.size, "source": "archive"}));
    }
    let _ = std::fs::write(&cache, serde_json::to_vec(&json!({"stamp": stamp, "entries": out})).unwrap_or_default());
    out
}

fn walk_layouts(root: &Path, source: &str) -> Vec<Value> {
    let mut out = vec![];
    let Ok(rd) = std::fs::read_dir(root) else { return out };
    let mut folders: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    folders.sort();
    for folder in folders {
        let ldir = root.join(&folder).join("layouts");
        let Ok(rd) = std::fs::read_dir(&ldir) else { continue };
        for e in rd.flatten() {
            let f = e.file_name().to_string_lossy().into_owned();
            let k = kind(&f);
            if k == "other" {
                continue;
            }
            let p = ldir.join(&f);
            let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            out.push(json!({"folder": folder, "layout": stem(&f), "kind": k, "file": f,
                            "path": p.to_string_lossy(), "size": size, "source": source}));
        }
    }
    out
}

pub fn imported() -> Vec<Value> {
    walk_layouts(Path::new(&trackimport::tracks_dir()), "import")
}

pub fn on_server() -> Vec<Value> {
    walk_layouts(&server_tracks(), "server")
}

pub fn dest_path(folder: &str, filename: &str) -> Result<PathBuf, String> {
    let folder = folder.trim().replace('/', "\\");
    let filename = Path::new(filename).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if folder.is_empty() || filename.is_empty() || folder.split('\\').any(|p| p == "..") {
        return Err("bad spline dest".into());
    }
    Ok(server_tracks().join(folder).join("layouts").join(filename))
}

fn same(size: u64, dest: &Path) -> bool {
    std::fs::metadata(dest).map(|m| m.is_file() && m.len() == size).unwrap_or(false)
}

/// One pass over the client archive for every needed (archive path -> dest).
fn extract_many(needed: &[Value]) -> (Vec<String>, Vec<String>) {
    let (mut copied, mut skipped) = (vec![], vec![]);
    if needed.is_empty() {
        return (copied, skipped);
    }
    let Some(pkg) = viewer::package() else { return (copied, skipped) };
    let want: HashMap<String, &Value> = needed.iter().map(|n| (sv(n, "archive").to_lowercase(), n)).collect();
    let Ok(mut fh) = std::fs::File::open(&pkg) else { return (copied, skipped) };
    for e in kspkg::entries(Path::new(&pkg)).unwrap_or_default() {
        let Some(rec) = want.get(&e.path.to_lowercase()) else { continue };
        let dest = PathBuf::from(sv(rec, "dest"));
        let tag = format!("archive:{}/{}", sv(rec, "folder"), sv(rec, "file"));
        if same(e.size as u64, &dest) {
            skipped.push(tag);
            continue;
        }
        if let Some(d) = dest.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(data) = kspkg::read_entry(&mut fh, &e) {
            let tmp = dest.with_extension("aisplinedata.part");
            if std::fs::write(&tmp, data).is_ok() && std::fs::rename(&tmp, &dest).is_ok() {
                copied.push(tag);
            }
        }
    }
    (copied, skipped)
}

fn matches(rec: &Value, folder: &str, layout: &str) -> bool {
    (folder.is_empty() || sv(rec, "folder") == folder)
        && (layout.is_empty() || sv(rec, "layout").to_lowercase().contains(layout)
            || sv(rec, "file").to_lowercase().contains(layout))
}

/// Copy missing splines for one track (or every track) onto the server.
pub fn ship(folder: &str, layout: &str) -> Value {
    let folder = folder.trim();
    let layout = layout.trim().to_lowercase();
    let (mut copied, mut skipped, mut errors) = (vec![], vec![], vec![]);
    for rec in imported() {
        if !matches(&rec, folder, &layout) {
            continue;
        }
        let tag = format!("import:{}/{}", sv(&rec, "folder"), sv(&rec, "file"));
        match dest_path(&sv(&rec, "folder"), &sv(&rec, "file")) {
            Ok(dest) => {
                let size = rec.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
                if same(size, &dest) {
                    skipped.push(tag);
                    continue;
                }
                if let Some(d) = dest.parent() {
                    let _ = std::fs::create_dir_all(d);
                }
                match std::fs::copy(sv(&rec, "path"), &dest) {
                    Ok(_) => copied.push(tag),
                    Err(e) => errors.push(format!("{}/{}: {e}", sv(&rec, "folder"), sv(&rec, "file"))),
                }
            }
            Err(e) => errors.push(format!("{}/{}: {e}", sv(&rec, "folder"), sv(&rec, "file"))),
        }
    }
    let mut need = vec![];
    for mut rec in archive_index(false) {
        if !matches(&rec, folder, &layout) {
            continue;
        }
        match dest_path(&sv(&rec, "folder"), &sv(&rec, "file")) {
            Ok(dest) => {
                let size = rec.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
                if same(size, &dest) {
                    skipped.push(format!("archive:{}/{}", sv(&rec, "folder"), sv(&rec, "file")));
                    continue;
                }
                rec["dest"] = json!(dest.to_string_lossy());
                need.push(rec);
            }
            Err(e) => errors.push(e),
        }
    }
    let (ac, sk) = extract_many(&need);
    copied.extend(ac);
    skipped.extend(sk);
    let (cn, sn) = (copied.len(), skipped.len());
    json!({"ok": errors.is_empty(), "folder": if folder.is_empty() { Value::Null } else { json!(folder) },
           "layout": if layout.is_empty() { Value::Null } else { json!(layout) },
           "copied": copied, "skipped": skipped, "errors": errors, "copied_n": cn, "skipped_n": sn})
}

/// Display name or folder -> possible folder ids.
pub fn folders_for(name: &str) -> Vec<String> {
    let name = name.trim();
    if name.is_empty() {
        return vec![];
    }
    let tm = trackimport::track_map(false);
    let mut out = vec![];
    if let Some(f) = tm.get(name).and_then(|v| v.as_str()) {
        out.push(f.to_string());
    }
    let slug = name.to_lowercase().replace(' ', "_");
    out.push(slug.clone());
    for (disp, f) in &tm {
        let f = f.as_str().unwrap_or("");
        if disp.to_lowercase() == name.to_lowercase() || f.to_lowercase() == slug {
            out.push(f.to_string());
        }
    }
    let mut seen = HashSet::new();
    out.into_iter().filter(|f| !f.is_empty() && seen.insert(f.clone())).collect()
}

/// Splines for whatever this server profile is about to host.
pub fn ship_for_profile(profile: &Value) -> Value {
    let custom = sv(profile, "custom_track").trim().to_string();
    if !custom.is_empty() {
        let folder = folders_for(&custom).into_iter().next().unwrap_or_default();
        let src = Path::new(&trackimport::tracks_dir()).join(&folder);
        let layout = crate::trackdeploy::read_track_folder(&src, None).map(|t| t.layout).unwrap_or_default();
        return ship(&folder, &layout);
    }
    let idx = profile.get("track_index").and_then(|v| v.as_i64()).unwrap_or(0);
    let evs = content::tracks().get("tracks").and_then(|t| t.as_array()).cloned().unwrap_or_default();
    let ev = evs.iter().find(|t| t.get("index").and_then(|v| v.as_i64()) == Some(idx)).cloned()
        .or_else(|| if idx >= 0 { evs.get(idx as usize).cloned() } else { None })
        .unwrap_or(json!({}));
    let folder = sv(&ev, "track").to_lowercase().replace(' ', "_");
    ship(&folder, &sv(&ev, "layout"))
}

/// What the Content page shows.
pub fn status() -> Value {
    let srv = on_server();
    let imp = imported();
    let arc = archive_index(false);
    let on: HashSet<(String, String)> = srv.iter().map(|r| (sv(r, "folder"), sv(r, "file"))).collect();
    let missing_imp: Vec<&Value> = imp.iter().filter(|r| !on.contains(&(sv(r, "folder"), sv(r, "file")))).collect();
    let missing_arc = arc.iter().filter(|r| !on.contains(&(sv(r, "folder"), sv(r, "file")))).count();
    let folders: BTreeSet<String> = srv.iter().chain(&imp).chain(&arc).map(|r| sv(r, "folder")).collect();
    let mut kinds: Map<String, Value> = Map::new();
    for r in &srv {
        let e = kinds.entry(sv(r, "folder")).or_insert(json!([]));
        e.as_array_mut().unwrap().push(json!(sv(r, "kind")));
    }
    let ready = folders.iter().filter(|f| {
        let k = kinds.get(*f).and_then(|v| v.as_array()).cloned().unwrap_or_default();
        k.contains(&json!("ideal_line")) && k.contains(&json!("pitlane"))
    }).count();
    json!({
        "ok": true, "server": srv.len(), "imported": imp.len(), "archive": arc.len(),
        "missing": missing_imp.len() + missing_arc,
        "missing_imports": missing_imp.iter().map(|r| format!("{}/{}", sv(r, "folder"), sv(r, "file"))).collect::<Vec<_>>(),
        "folders": folders.len(), "ready_folders": ready,
        "note": "Stock lines come from the game archive. Custom tracks (Barber, Highlands) keep theirs in the import folder. Ship copies both next to the dedicated server.",
    })
}
