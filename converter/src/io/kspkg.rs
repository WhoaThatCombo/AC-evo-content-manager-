//! Writing a car mod `.kspkg` from memory.
//!
//! Data region from offset 0, each file XOR'd with the engine key at phase
//! 0 - EXCEPT `.texturemips`, stored plaintext with record kind 0 (the game
//! streams them straight to the GPU; every working mod package does this).
//! Then the 64 MiB table: 256-byte records (path, kind @228, path length in
//! chars @230, FNV-1a-64 id @232, size @240, offset @248), directories as
//! kind 1, sorted by id like Kunos's own archives, unused records left as
//! encrypted zeros.
//! ⚠ XOR in u64 words: a byte loop over a ~600 MB package is far too slow.

use anyhow::{bail, Context, Result};
use std::collections::BTreeSet;
use std::io::Write;
use std::path::Path;

pub const KEY: [u8; 8] = [0xc1, 0x35, 0x11, 0x7d, 0xa9, 0x21, 0x97, 0x9f];
const BUCKET: usize = 256;
const TABLE: usize = 64 * 1024 * 1024;

pub fn xor(buf: &mut [u8]) {
    let k = u64::from_le_bytes(KEY);
    let (head, body, tail) = unsafe { buf.align_to_mut::<u64>() };
    // the key phase is relative to the start of the buffer
    for (i, b) in head.iter_mut().enumerate() {
        *b ^= KEY[i % 8];
    }
    let ph = head.len() % 8;
    let kw = if ph == 0 { k } else { k.rotate_right((ph * 8) as u32) };
    for w in body.iter_mut() {
        *w ^= kw;
    }
    let base = head.len() + body.len() * 8;
    for (i, b) in tail.iter_mut().enumerate() {
        *b ^= KEY[(base + i) % 8];
    }
}

pub fn entry_id(path: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    // the game hashes the LOWERCASED path (every stock record's id matches
    // the lower-cased path; mixed-case ids are not found)
    for u in path.to_lowercase().encode_utf16() {
        for c in u.to_le_bytes() {
            h = (h ^ c as u64).wrapping_mul(0x100000001b3);
        }
    }
    h
}

fn record(path: &str, kind: u16, size: u32, offset: u64) -> Result<[u8; BUCKET]> {
    let raw = path.as_bytes();
    if raw.len() > BUCKET - 33 {
        bail!("path too long for a record: {path}");
    }
    let mut r = [0u8; BUCKET];
    r[..raw.len()].copy_from_slice(raw);
    r[228..230].copy_from_slice(&kind.to_le_bytes());
    r[230..232].copy_from_slice(&(path.chars().count() as u16).to_le_bytes());
    r[232..240].copy_from_slice(&entry_id(path).to_le_bytes());
    r[240..244].copy_from_slice(&size.to_le_bytes());
    r[248..256].copy_from_slice(&offset.to_le_bytes());
    Ok(r)
}

/// Write `files` (archive path with backslashes, bytes) to `out`.
pub fn write(out: &Path, files: &mut [(String, Vec<u8>)]) -> Result<()> {
    let tmp = out.with_extension("kspkg.part");
    let f = std::fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
    let mut w = std::io::BufWriter::with_capacity(8 << 20, f);
    let mut recs: Vec<[u8; BUCKET]> = Vec::new();
    let mut dirs = BTreeSet::new();
    let mut off = 0u64;
    for (path, data) in files.iter_mut() {
        let plain = path.to_ascii_lowercase().ends_with(".texturemips");
        if !plain {
            xor(data);
        }
        w.write_all(data)?;
        recs.push(record(path, if plain { 0 } else { 0x100 }, data.len() as u32, off)?);
        off += data.len() as u64;
        if !plain {
            xor(data); // hand the caller its bytes back unchanged
        }
        let mut d = path.as_str();
        while let Some((parent, _)) = d.rsplit_once('\\') {
            dirs.insert(parent.to_string());
            d = parent;
        }
    }
    for d in &dirs {
        recs.push(record(d, 1, 0, 0)?);
    }
    recs.sort_by_key(|r| u64::from_le_bytes(r[232..240].try_into().unwrap()));
    if recs.len() * BUCKET > TABLE {
        bail!("too many files for one package ({})", recs.len());
    }
    let mut table = vec![0u8; TABLE];
    for (i, r) in recs.iter().enumerate() {
        table[i * BUCKET..(i + 1) * BUCKET].copy_from_slice(r);
    }
    // every 256-byte record restarts the key at phase 0, and 256 % 8 == 0,
    // so one pass over the whole table is the same thing
    xor(&mut table);
    w.write_all(&table)?;
    w.flush()?;
    drop(w);
    std::fs::rename(&tmp, out).with_context(|| format!("rename to {}", out.display()))?;
    Ok(())
}

/// Read a package written by `write` (or any engine-key .kspkg with the
/// table at the end): (path, bytes) for every file whose path contains
/// `filter` (case-insensitive).
pub fn read(pkg: &Path, filter: &str) -> Result<Vec<(String, Vec<u8>)>> {
    let mut out = Vec::new();
    read_each(pkg, filter, |p, d| {
        out.push((p.to_string(), d));
        Ok(())
    })?;
    Ok(out)
}

/// `read` one entry at a time: memory stays at one file however much matches
/// (an unpack of tracks\common_assets held 12.8 GB with `read`).
pub fn read_each(pkg: &Path, filter: &str, mut each: impl FnMut(&str, Vec<u8>) -> Result<()>) -> Result<usize> {
    let f = std::fs::File::open(pkg).with_context(|| format!("open {}", pkg.display()))?;
    let map = unsafe { memmap2::Mmap::map(&f)? };
    if map.len() < TABLE {
        bail!("{} is smaller than its table", pkg.display());
    }
    let mut table = map[map.len() - TABLE..].to_vec();
    xor(&mut table);
    let want = filter.to_ascii_lowercase();
    let mut n = 0;
    for r in table.chunks_exact(BUCKET) {
        let kind = u16::from_le_bytes([r[228], r[229]]);
        let size = u32::from_le_bytes(r[240..244].try_into().unwrap()) as usize;
        let off = u64::from_le_bytes(r[248..256].try_into().unwrap()) as usize;
        let end = r[..228].iter().position(|&b| b == 0).unwrap_or(228);
        if end == 0 || kind == 1 {
            continue;
        }
        let path = String::from_utf8_lossy(&r[..end]).into_owned();
        if !path.to_ascii_lowercase().contains(&want) {
            continue;
        }
        let Some(src) = map.get(off..off + size) else { bail!("{path}: past end of package") };
        let mut data = src.to_vec();
        if kind != 0 {
            xor(&mut data);
        }
        each(&path, data)?;
        n += 1;
    }
    Ok(n)
}

/// A package opened for browsing: list the table, read entries one by one.
pub struct Pkg {
    map: memmap2::Mmap,
    pub entries: Vec<Entry>,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub path: String,
    pub size: usize,
    /// the record's FNV-1a id as stored
    pub id: u64,
    off: usize,
    kind: u16,
}

impl Entry {
    /// byte offset of the data in the package (for reading in file order)
    pub fn offset(&self) -> usize {
        self.off
    }
}

impl Pkg {
    pub fn open(pkg: &Path) -> Result<Pkg> {
        let f = std::fs::File::open(pkg).with_context(|| format!("open {}", pkg.display()))?;
        let map = unsafe { memmap2::Mmap::map(&f)? };
        if map.len() < TABLE {
            bail!("{} is smaller than its table", pkg.display());
        }
        let mut table = map[map.len() - TABLE..].to_vec();
        xor(&mut table);
        let mut entries = Vec::new();
        for r in table.chunks_exact(BUCKET) {
            let kind = u16::from_le_bytes([r[228], r[229]]);
            let size = u32::from_le_bytes(r[240..244].try_into().unwrap()) as usize;
            let off = u64::from_le_bytes(r[248..256].try_into().unwrap()) as usize;
            let end = r[..228].iter().position(|&b| b == 0).unwrap_or(228);
            if end == 0 || kind == 1 {
                continue;
            }
            let id = u64::from_le_bytes(r[232..240].try_into().unwrap());
            entries.push(Entry { path: String::from_utf8_lossy(&r[..end]).into_owned(), size, id, off, kind });
        }
        Ok(Pkg { map, entries })
    }

    pub fn read(&self, e: &Entry) -> Result<Vec<u8>> {
        let Some(src) = self.map.get(e.off..e.off + e.size) else { bail!("{}: past end of package", e.path) };
        let mut data = src.to_vec();
        if e.kind != 0 {
            xor(&mut data);
        }
        Ok(data)
    }

    /// The stored bytes of an entry without a copy: plaintext for a
    /// `.texturemips` (kind 0), still XOR'd for everything else.
    pub fn raw(&self, e: &Entry) -> Option<&[u8]> {
        self.map.get(e.off..e.off + e.size)
    }

    pub fn is_plain(e: &Entry) -> bool {
        e.kind == 0
    }

    pub fn find(&self, path: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.path.eq_ignore_ascii_case(path))
    }
}
