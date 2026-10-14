//! Reading Kunos .kspkg archives - ported from acecm/kspkg.py.
//!
//! ```text
//! [data region: every file XOR'd with the engine key; the key phase RESETS
//!  TO 0 at each file's own start]
//! [fixed 64 MiB table: one 256-byte bucket per path, phase 0 per bucket]
//! bucket: [0:N] NUL-terminated path (backslashes) [232:240] u64 FNV-1a-64
//!         id of the path as UTF-16LE  [240:244] u32 size  [248:256] u64 offset
//! ```
//! ⚠ `.texturemips` are stored UNENCRYPTED (mip pages stream straight off
//! disk); decrypting them turns them into noise that looks like a tiling bug.
//! ⚠ Offsets need all 64 bits - Kunos's own content.kspkg is ~74 GB.
#![allow(dead_code)]

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub const KEY: [u8; 8] = [0xc1, 0x35, 0x11, 0x7d, 0xa9, 0x21, 0x97, 0x9f];
pub const BUCKET: usize = 256;
pub const TABLE: u64 = 64 * 1024 * 1024;

/// XOR with the engine key, phase taken from `offset` (relative to the
/// start of the thing being decrypted - a file, or the table).
pub fn decrypt(buf: &mut [u8], offset: u64) {
    let ph = (offset % 8) as usize;
    for (i, b) in buf.iter_mut().enumerate() {
        *b ^= KEY[(i + ph) % 8];
    }
}

fn plain(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".texturemips")
}

/// The file name of an entry. ⚠ Entries use Windows separators, so this is
/// NOT Path::file_name on Linux (a backslash is a normal character there).
pub fn entry_name(entry: &str) -> &str {
    entry.rsplit(['\\', '/']).next().unwrap_or(entry)
}

pub fn entry_dir(entry: &str) -> String {
    let n = entry.replace('/', "\\");
    n.rsplit_once('\\').map(|(d, _)| d.to_string()).unwrap_or_default()
}

pub const FNV_BASIS: u64 = 0xcbf29ce484222325;
pub const FNV_PRIME: u64 = 0x100000001b3;

/// The record's sort key: FNV-1a-64 over the path as UTF-16LE (as given -
/// the hash is case-sensitive).
pub fn entry_id(path: &str) -> u64 {
    let mut h = FNV_BASIS;
    for u in path.encode_utf16() {
        for c in u.to_le_bytes() {
            h = (h ^ c as u64).wrapping_mul(FNV_PRIME);
        }
    }
    h
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub path: String,
    pub size: u32,
    pub offset: u64,
}

fn parse_bucket(dec: &[u8]) -> Option<Entry> {
    let end = dec.iter().position(|&b| b == 0)?;
    if end == 0 {
        return None;
    }
    let path = std::str::from_utf8(&dec[..end]).ok()?;
    if !path.contains('\\') {
        return None;
    }
    let size = u32::from_le_bytes(dec[240..244].try_into().unwrap());
    let offset = u64::from_le_bytes(dec[248..256].try_into().unwrap());
    Some(Entry { path: path.to_string(), size, offset })
}

/// Every named entry. ⚠ Empty for a file too small to BE an archive (a
/// partial download in a mods folder must not abort the whole scan).
pub fn entries(path: &Path) -> std::io::Result<Vec<Entry>> {
    let total = std::fs::metadata(path)?.len();
    if total < TABLE {
        return Ok(vec![]);
    }
    let mut f = File::open(path)?;
    f.seek(SeekFrom::Start(total - TABLE))?;
    // most buckets are empty and hold the bare keystream; skip those undecrypted
    let mut filler = [0u8; BUCKET];
    decrypt(&mut filler, 0);
    let mut out = vec![];
    let mut chunk = vec![0u8; BUCKET * 4096];
    let mut left = TABLE as usize;
    while left > 0 {
        let want = chunk.len().min(left);
        f.read_exact(&mut chunk[..want])?;
        left -= want;
        for b in chunk[..want].chunks_exact_mut(BUCKET) {
            if *b == filler[..] {
                continue;
            }
            decrypt(b, 0); // buckets are 256-aligned: always phase 0
            if let Some(e) = parse_bucket(b) {
                out.push(e);
            }
        }
    }
    Ok(out)
}

/// The bytes of one entry, decrypted unless it is a .texturemips.
pub fn read_entry(f: &mut File, e: &Entry) -> std::io::Result<Vec<u8>> {
    f.seek(SeekFrom::Start(e.offset))?;
    let mut buf = vec![0u8; e.size as usize];
    f.read_exact(&mut buf)?;
    if !plain(&e.path) {
        decrypt(&mut buf, 0);
    }
    Ok(buf)
}

/// One entry by exact path WITHOUT scanning: the table is a dense array
/// sorted by entry_id, so this is a binary search of ~18 bucket reads.
/// ⚠ the empty tail is marked by a zero id, NOT the kind field (0 is a real
/// kind). ⚠ equal ids are walked - a hash collision must not return the
/// wrong file.
pub fn find_entry(path: &Path, entry_path: &str) -> std::io::Result<Option<Entry>> {
    let total = std::fs::metadata(path)?.len();
    if total < TABLE {
        return Ok(None);
    }
    let start = total - TABLE;
    let mut f = File::open(path)?;
    let mut rec = |i: u64| -> std::io::Result<[u8; BUCKET]> {
        let mut b = [0u8; BUCKET];
        f.seek(SeekFrom::Start(start + i * BUCKET as u64))?;
        f.read_exact(&mut b)?;
        decrypt(&mut b, 0);
        Ok(b)
    };
    let id = |b: &[u8; BUCKET]| u64::from_le_bytes(b[232..240].try_into().unwrap());
    let (mut lo, mut hi) = (0u64, TABLE / BUCKET as u64);
    while lo < hi {
        let mid = (lo + hi) / 2;
        if id(&rec(mid)?) != 0 { lo = mid + 1 } else { hi = mid }
    }
    let count = lo;
    let want = entry_id(entry_path);
    let (mut lo, mut hi) = (0u64, count);
    while lo < hi {
        let mid = (lo + hi) / 2;
        if id(&rec(mid)?) < want { lo = mid + 1 } else { hi = mid }
    }
    let target = entry_path.to_lowercase();
    let mut i = lo;
    while i < count {
        let b = rec(i)?;
        if id(&b) != want {
            break;
        }
        if let Some(e) = parse_bucket(&b) {
            if e.path.to_lowercase() == target {
                return Ok(Some(e));
            }
        }
        i += 1;
    }
    Ok(None)
}

/// Extract one entry by exact path (case-insensitive) to `dst`.
pub fn extract_one(path: &Path, entry_path: &str, dst: &Path) -> std::io::Result<bool> {
    let e = match find_entry(path, entry_path)? {
        Some(e) => e,
        None => {
            let t = entry_path.to_lowercase();
            match entries(path)?.into_iter().find(|e| e.size > 0 && e.path.to_lowercase() == t) {
                Some(e) => e,
                None => return Ok(false),
            }
        }
    };
    let mut f = File::open(path)?;
    let data = read_entry(&mut f, &e)?;
    if let Some(d) = dst.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(dst, data)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_utf16() {
        // FNV-1a-64 of "" is the basis; of "a" in UTF-16LE is two bytes
        assert_eq!(entry_id(""), FNV_BASIS);
        let mut h = FNV_BASIS;
        for c in [b'a', 0u8] { h = (h ^ c as u64).wrapping_mul(FNV_PRIME); }
        assert_eq!(entry_id("a"), h);
    }

    #[test]
    fn names() {
        assert_eq!(entry_name("content\\cars\\ks_x\\a.json"), "a.json");
        assert_eq!(entry_dir("content\\cars\\ks_x\\a.json"), "content\\cars\\ks_x");
        assert_eq!(entry_dir("a.json"), "");
    }
}
