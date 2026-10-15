//! The files the exe carries inside itself (see build.rs): the web UI, the
//! catalogs and the car viewer. A release build unpacks them once per build
//! into the data folder and reads them from there; a debug build reads the
//! source tree, so UI edits show without rebuilding.

use std::path::{Path, PathBuf};

use sha2::Digest;

include!(concat!(env!("OUT_DIR"), "/bundle_files.rs"));

/// Release builds use the bundle; ACECM_BUNDLE=1 makes a debug build do so.
pub fn active() -> bool {
    std::env::var("ACECM_REPO").is_err()
        && (!cfg!(debug_assertions) || std::env::var("ACECM_BUNDLE").map(|v| v == "1").unwrap_or(false))
}

pub fn dir() -> PathBuf {
    crate::paths::data().join("bundle")
}

/// A digest of everything embedded: unpacking is skipped while it matches.
fn stamp() -> String {
    let mut h = sha2::Sha256::new();
    for (rel, packed, size) in FILES {
        h.update(rel.as_bytes());
        h.update((*size as u64).to_le_bytes());
        h.update(packed);
    }
    h.finalize().iter().take(12).map(|b| format!("{b:02x}")).collect()
}

fn inflate(packed: &[u8], size: usize) -> Vec<u8> {
    use std::io::Read;
    let mut out = Vec::with_capacity(size);
    let _ = flate2::read::DeflateDecoder::new(packed).read_to_end(&mut out);
    out
}

/// Unpack the embedded files if this build has not already. ⚠ Each file is
/// written to a temp name and renamed, so a crash mid-way never leaves a
/// half-written evoview.exe that later fails to start; the stamp is written
/// last, so an interrupted unpack simply happens again.
pub fn ensure() {
    if !active() {
        return;
    }
    let d = dir();
    let want = stamp();
    let stamp_file = d.join("bundle.stamp");
    if std::fs::read_to_string(&stamp_file).map(|s| s.trim() == want).unwrap_or(false) {
        return;
    }
    // ⚠ The UI and catalogs now, the viewer exe in the background: writing a
    // fresh 8 MB exe is what antivirus scans, and it held the first window
    // after every update back by ~1.7 s. Nothing on the first page needs it.
    let is_exe = |rel: &str| rel.to_ascii_lowercase().ends_with(".exe");
    let failed_now = unpack(&d, FILES.iter().filter(|f| !is_exe(f.0)));
    std::thread::spawn(move || {
        let failed = failed_now + unpack(&d, FILES.iter().filter(|f| is_exe(f.0)));
        if failed == 0 {
            let _ = std::fs::write(&stamp_file, &want);
            crate::logs::info(format!("bundle: unpacked {} file(s) to {}", FILES.len(), d.display()));
        }
    });
}

/// Write each file whose content differs; returns how many failed.
fn unpack<'a>(d: &Path, files: impl Iterator<Item = &'a (&'a str, &'a [u8], usize)>) -> usize {
    let mut failed = 0;
    for (rel, packed, size) in files {
        let data = &inflate(packed, *size);
        if data.len() != *size {
            crate::logs::warn(format!("bundle: {rel} did not unpack ({} of {size} bytes)", data.len()));
            failed += 1;
            continue;
        }
        let p = d.join(rel);
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::read(&p).map(|have| have == *data).unwrap_or(false) {
            continue;
        }
        let tmp = p.with_extension("partial");
        let ok = std::fs::write(&tmp, data).and_then(|_| std::fs::rename(&tmp, &p));
        if let Err(e) = ok {
            // a running evoview.exe is locked: keep the old copy, retry next start
            let _ = std::fs::remove_file(&tmp);
            crate::logs::warn(format!("bundle: could not write {}: {e}", p.display()));
            failed += 1;
        }
    }
    failed
}

/// Where the UI files are: the bundle, or the source tree.
pub fn web_dir() -> PathBuf {
    if active() { dir().join("web") } else { crate::paths::repo().join("acecm").join("web") }
}

/// Where the bundled tools data is (cars.json, events_*.json, evoview.exe).
pub fn tools_dir() -> PathBuf {
    if active() { dir().join("tools") } else { crate::paths::repo().join("tools") }
}
