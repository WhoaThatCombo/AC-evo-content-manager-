//! Writes that cannot leave a half-written file behind.
//!
//! ⚠ A truncate-then-write of a game or settings file that fails part way
//! (disk full, an antivirus lock, a crash) leaves the user with a broken
//! file; a backup copy that fails part way leaves a truncated "original"
//! that every later Restore puts back. Everything here writes a temp file
//! BESIDE the target (same volume) and renames it into place, so the target
//! is always either the old file or the complete new one.

use std::io::Write;
use std::path::{Path, PathBuf};

fn temp_beside(target: &Path) -> PathBuf {
    let mut b = [0u8; 4];
    let _ = getrandom::fill(&mut b);
    let tag: String = b.iter().map(|x| format!("{x:02x}")).collect();
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    target.with_file_name(format!("{name}.acecm-{tag}.part"))
}

/// Replace `path` with `data` atomically (flushed to disk before the rename).
pub fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let tmp = temp_beside(path);
    let r = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
}

/// Copy `src` over `dst` atomically, keeping the source's mtime.
pub fn copy_atomic(src: &Path, dst: &Path) -> std::io::Result<()> {
    let tmp = temp_beside(dst);
    let r = (|| {
        std::fs::copy(src, &tmp)?;
        if let Ok(t) = std::fs::metadata(src).and_then(|m| m.modified()) {
            if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&tmp) {
                let _ = f.set_modified(t);
                let _ = f.sync_all();
            }
        }
        std::fs::rename(&tmp, dst)
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
}

/// Make `bak` from `src` unless it already exists. Ok(true) when made.
/// ⚠ The caller must STOP on Err: writing the original after a failed
/// backup is how a user ends up with no way back.
pub fn backup_once(src: &Path, bak: &Path) -> std::io::Result<bool> {
    if bak.exists() {
        return Ok(false);
    }
    copy_atomic(src, bak)?;
    Ok(true)
}
