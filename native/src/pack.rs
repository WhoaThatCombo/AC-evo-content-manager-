//! Shareable server packages: one zip holding a server's track and car mods,
//! for a joiner a direct fetch cannot serve well (bad internet, far away).
//! Ported from pack.py.
//!
//! ⚠ The contents come from the same code that decides what a joiner would
//! DOWNLOAD (autoshare.share_needs + registry's finders), so the package and
//! the network path cannot drift apart. `acecm-package.json` at the root is
//! what tells a dropped file apart from an ordinary mod zip.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::{contentsync, install, logs, registry};

pub const MARKER: &str = "acecm-package.json";
pub const FORMAT: i64 = 1;

fn digest(p: &Path) -> String {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    if let Ok(mut f) = std::fs::File::open(p) {
        let mut buf = vec![0u8; 1 << 20];
        while let Ok(n) = f.read(&mut buf) {
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
        }
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn strs(v: &Value, k: &str) -> Vec<String> {
    v.get(k).and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect()).unwrap_or_default()
}

/// (files [(rel, src, kind, size)], want, missing)
fn contents(profile: &Value) -> (Vec<(String, PathBuf, &'static str, u64)>, Value, Vec<String>) {
    let want = crate::autoshare::share_needs(profile);
    let (mut files, mut missing) = (Vec::new(), Vec::new());
    for (kind, names) in [("mod", strs(&want, "mods")), ("track", strs(&want, "tracks"))] {
        for name in names {
            let got = if kind == "mod" { registry::mod_files_pub(&name) } else { registry::track_files_pub(&name) };
            if got.is_empty() {
                missing.push(format!("{kind} {name}"));
                continue;
            }
            for (rel, path) in got {
                match std::fs::metadata(&path) {
                    Ok(m) => files.push((rel, path, kind, m.len())),
                    Err(_) => missing.push(format!("{kind} {name}: {rel}")),
                }
            }
        }
    }
    (files, want, missing)
}

/// What a package would contain, without building it.
pub fn preview(profile: &Value) -> Value {
    let (files, want, missing) = contents(profile);
    json!({"ok": true, "tracks": strs(&want, "tracks"), "mods": strs(&want, "mods"),
           "files": files.len(), "bytes": files.iter().map(|f| f.3).sum::<u64>(), "missing": missing})
}

fn default_dir() -> PathBuf {
    let d = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_default().join("Downloads");
    if d.is_dir() { d } else { crate::paths::data() }
}

fn safe_name(s: &str) -> String {
    let keep: String = s.chars().map(|c| if c.is_alphanumeric() || " -_".contains(c) { c } else { '_' }).collect();
    let j = keep.split_whitespace().collect::<Vec<_>>().join("_");
    if j.is_empty() { "server".into() } else { j }
}

/// Write the package.
pub fn build(profile: &Value, dest: Option<&str>, digests: bool) -> Value {
    let (files, want, missing) = contents(profile);
    if files.is_empty() {
        return json!({"ok": false, "error": "this server needs no custom content - its track and cars are all stock, so there is nothing to share", "missing": missing});
    }
    let pname = profile.get("name").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).unwrap_or("server");
    let name = safe_name(pname);
    let mut out = dest.map(PathBuf::from).unwrap_or_else(|| default_dir().join(format!("ACECM {name} package.zip")));
    if out.is_dir() {
        out = out.join(format!("ACECM {name} package.zip"));
    }
    if let Some(p) = out.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    let track = [profile.get("track_label"), profile.get("custom_track")].into_iter()
        .filter_map(|v| v.and_then(|x| x.as_str()).filter(|s| !s.is_empty())).next().unwrap_or("");
    let mut man = json!({"format": FORMAT,
        "built": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        "built_by": "ACECM",
        "server": {"name": profile.get("name").and_then(|v| v.as_str()).unwrap_or(""), "track": track,
                   "tracks": strs(&want, "tracks"), "mods": strs(&want, "mods")},
        "files": []});
    let total: u64 = files.iter().map(|f| f.3).sum();
    install::ingest_begin(&format!("packaging {name}"), total);
    let tmp = PathBuf::from(format!("{}.part", out.display()));
    let r = (|| -> Result<(), String> {
        // ⚠ DEFLATED: the receiver is the one with bad internet
        let mut z = zip::ZipWriter::new(std::io::BufWriter::new(std::fs::File::create(&tmp).map_err(|e| e.to_string())?));
        let opt = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).large_file(true);
        for (rel, src, kind, size) in &files {
            let rel = rel.replace('\\', "/");
            man["files"].as_array_mut().unwrap().push(json!({"path": rel, "size": size, "kind": kind,
                "sha256": if digests { digest(src) } else { String::new() }}));
            z.start_file(&rel, opt).map_err(|e| e.to_string())?;
            let mut f = std::fs::File::open(src).map_err(|e| e.to_string())?;
            std::io::copy(&mut f, &mut z).map_err(|e| e.to_string())?;
            install::ingest_step(*size, rel.rsplit('/').next().unwrap_or(""));
        }
        z.start_file(MARKER, zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated)).map_err(|e| e.to_string())?;
        z.write_all(py_dumps_indent1(&man).as_bytes()).map_err(|e| e.to_string())?;
        z.finish().map_err(|e| e.to_string())?.flush().map_err(|e| e.to_string())?;
        Ok(())
    })().and_then(|_| { let _ = std::fs::remove_file(&out); std::fs::rename(&tmp, &out).map_err(|e| e.to_string()) });
    install::ingest_end("");
    if let Err(e) = r {
        let _ = std::fs::remove_file(&tmp);
        logs::warn(format!("pack build: {e}"));
        return json!({"ok": false, "error": format!("could not build the package: {e}")});
    }
    let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    logs::info(format!("built server package {} ({} file(s), {:.1} MB)", out.display(), files.len(), size as f64 / 1e6));
    json!({"ok": true, "path": out.to_string_lossy(), "files": files.len(), "bytes": size, "raw_bytes": total,
           "missing": missing, "server": man["server"]})
}

/// json.dumps(v, indent=1)
fn py_dumps_indent1(v: &Value) -> String {
    let two = crate::profiles::py_dumps_indent2(v);
    // indent=2 -> indent=1: halve each line's leading run of spaces
    two.lines().map(|l| {
        let n = l.len() - l.trim_start_matches(' ').len();
        format!("{}{}", " ".repeat(n / 2), &l[n..])
    }).collect::<Vec<_>>().join("\n")
}

pub fn read_manifest(path: &Path) -> Value {
    let Some(b) = crate::archive::read_member(path, MARKER) else { return json!({}) };
    match serde_json::from_slice::<Value>(&b) {
        Ok(m) if m.is_object() && m.get("files").map(|f| !f.is_null()).unwrap_or(false) => m,
        _ => json!({}),
    }
}

/// Cheap enough to call on every dropped file.
pub fn is_package(path: &Path) -> bool {
    path.to_string_lossy().to_lowercase().ends_with(".zip") && path.is_file()
        && crate::archive::names(path).iter().any(|n| n == MARKER)
}

/// Unpack a server package into this machine's content.
pub fn install(path: &Path) -> Value {
    let man = read_manifest(path);
    if man.as_object().map(|m| m.is_empty()).unwrap_or(true) {
        return json!({"ok": false, "error": "not an ACECM server package"});
    }
    if man.get("format").and_then(|v| v.as_i64()).unwrap_or(0) > FORMAT {
        return json!({"ok": false, "error": "this package was made by a newer ACECM - update, then drop it again"});
    }
    // sizes from the zip's own directory, not from the manifest inside it
    if let Some(bad) = install::archive_budget(path, Path::new(&contentsync::tracks_dir())) {
        return bad;
    }
    let entries = man.get("files").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let total: u64 = entries.iter().map(|e| e.get("size").and_then(|v| v.as_u64()).unwrap_or(0)).sum::<u64>().max(1);
    install::ingest_begin("installing server package", total);
    let (mut written, mut skipped) = (0usize, 0usize);
    let mut folders = std::collections::BTreeSet::new();
    let mut bad = Vec::new();
    let r = (|| -> Result<(), String> {
        let mut z = zip::ZipArchive::new(std::fs::File::open(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        for e in &entries {
            let rel = e.get("path").and_then(|v| v.as_str()).unwrap_or("").replace('\\', "/");
            // ⚠ destination() validates AND is the network install's mapping
            let dest = match contentsync::destination(&rel) {
                Ok(d) => d,
                Err(ex) => { bad.push(format!("{rel}: {ex}")); continue; }
            };
            let size = e.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
            let want = e.get("sha256").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let have = dest.is_file() && std::fs::metadata(&dest).map(|m| m.len() == size).unwrap_or(false)
                && (want.is_empty() || registry::file_digest(&dest).as_deref() == Some(want.as_str()));
            if have {
                skipped += 1;
            } else {
                if let Some(p) = dest.parent() {
                    std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
                }
                let tmp = PathBuf::from(format!("{}.part", dest.display()));
                {
                    let mut src = z.by_name(&rel).map_err(|e| e.to_string())?;
                    let mut out = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
                    std::io::copy(&mut src, &mut out).map_err(|e| e.to_string())?;
                }
                if !want.is_empty() && digest(&tmp) != want {
                    let _ = std::fs::remove_file(&tmp);
                    bad.push(format!("{rel}: checksum mismatch"));
                    continue;
                }
                let _ = std::fs::remove_file(&dest);
                std::fs::rename(&tmp, &dest).map_err(|e| e.to_string())?;
                written += 1;
            }
            let parts: Vec<&str> = rel.split('/').collect();
            if parts.len() >= 3 && parts[0] == "tracks" {
                folders.insert(parts[1].to_string());
            }
            install::ingest_step(size, rel.rsplit('/').next().unwrap_or(""));
        }
        // a track on disk is not loadable until it is in the client tables
        for f in &folders {
            contentsync::register_downloaded_track(f);
        }
        Ok(())
    })();
    install::ingest_end("");
    install::after_content_change();
    if let Err(e) = r {
        logs::warn(format!("pack install: {e}"));
        return json!({"ok": false, "error": format!("could not install the package: {e}")});
    }
    logs::info(format!("installed server package {}: {written} written, {skipped} already had, {} track(s)",
        path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), folders.len()));
    json!({"ok": true, "written": written, "already_had": skipped, "tracks": folders.into_iter().collect::<Vec<_>>(),
           "problems": bad, "server": man.get("server").cloned().unwrap_or(json!({}))})
}
