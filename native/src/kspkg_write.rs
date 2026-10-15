//! Writing .kspkg archives - ported from acecm/kspkg_write.py.
//!
//! The 64 MiB tail is a DENSE ARRAY of 256-byte records sorted by
//! entry_id (FNV-1a-64 over the path as UTF-16LE), packed from index 0 with
//! no count field - the loader walks until an empty record:
//!   228 u16 kind (0x0100 file, 0x0001 dir) | 230 u16 path length in chars
//!   232 u64 id (sort key) | 240 u32 size | 248 u64 offset
//! ⚠ A new record needs a COMPLETE header (a zeroed one is skipped as empty)
//! and the WHOLE table must be re-sorted (new ids land anywhere).
//! ⚠ Payloads encrypt at phase 0 (each entry's keystream restarts at its own
//! start); records are 256-aligned so phase 0 too.
//! ⚠ Only for ENCRYPTED archives (Kunos's own). Some community mod archives
//! store plaintext, and blindly encrypting a write corrupts them - so a write
//! to an archive that does not look encrypted is refused.
#![allow(dead_code)]

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use serde_json::{json, Value};

use crate::kspkg::{decrypt, entry_id, BUCKET, TABLE};

pub const KIND_FILE: u16 = 0x0100;
pub const KIND_DIR: u16 = 0x0001;
pub const MAX_PATH: usize = BUCKET - 32 - 1;

pub type Rec = [u8; BUCKET];

fn u16_at(r: &Rec, o: usize) -> u16 { u16::from_le_bytes(r[o..o + 2].try_into().unwrap()) }
fn u32_at(r: &Rec, o: usize) -> u32 { u32::from_le_bytes(r[o..o + 4].try_into().unwrap()) }
fn u64_at(r: &Rec, o: usize) -> u64 { u64::from_le_bytes(r[o..o + 8].try_into().unwrap()) }

pub fn record_path(r: &Rec) -> String {
    let end = r.iter().position(|&b| b == 0).unwrap_or(0);
    String::from_utf8_lossy(&r[..end]).into_owned()
}

/// One complete 256-byte record.
pub fn bucket(path: &str, size: u32, offset: u64, kind: Option<u16>) -> Result<Rec, String> {
    let raw = path.as_bytes();
    if raw.len() > MAX_PATH {
        return Err(format!("path too long for a record ({}): {path}", raw.len()));
    }
    let mut r = [0u8; BUCKET];
    r[..raw.len()].copy_from_slice(raw);
    let kind = kind.unwrap_or(if size > 0 { KIND_FILE } else { KIND_DIR });
    r[228..230].copy_from_slice(&kind.to_le_bytes());
    r[230..232].copy_from_slice(&(path.chars().count() as u16).to_le_bytes());
    r[232..240].copy_from_slice(&entry_id(path).to_le_bytes());
    r[240..244].copy_from_slice(&size.to_le_bytes());
    r[248..256].copy_from_slice(&offset.to_le_bytes());
    Ok(r)
}

/// (table start, every used record decrypted, in table order).
pub fn read_records(archive: &Path) -> std::io::Result<(u64, Vec<Rec>)> {
    let total = std::fs::metadata(archive)?.len();
    if total < TABLE {
        return Err(std::io::Error::other("too small to be a kspkg"));
    }
    let start = total - TABLE;
    let mut f = File::open(archive)?;
    f.seek(SeekFrom::Start(start))?;
    let mut table = vec![0u8; TABLE as usize];
    f.read_exact(&mut table)?;
    let mut out = vec![];
    for c in table.chunks_exact_mut(BUCKET) {
        decrypt(c, 0);
        let end = c.iter().position(|&b| b == 0).unwrap_or(0);
        if end == 0 {
            continue;
        }
        out.push(<Rec>::try_from(&c[..]).unwrap());
    }
    Ok((start, out))
}

/// Complete any record whose header was left zeroed by older tooling.
pub fn repair_headers(records: &mut [Rec]) -> Vec<String> {
    let mut fixed = vec![];
    for r in records.iter_mut() {
        if u16_at(r, 228) != 0 || u64_at(r, 232) != 0 {
            continue;
        }
        let path = record_path(r);
        let size = u32_at(r, 240);
        r[228..230].copy_from_slice(&(if size > 0 { KIND_FILE } else { KIND_DIR }).to_le_bytes());
        r[230..232].copy_from_slice(&(path.chars().count() as u16).to_le_bytes());
        r[232..240].copy_from_slice(&entry_id(&path).to_le_bytes());
        fixed.push(path);
    }
    fixed
}

/// Does this archive look XOR-encrypted? A real Kunos archive's records
/// only parse after decryption; a plaintext one parses as-is.
/// ⚠ Decided by the record's own checksum: its id (at 232) must be the
/// FNV hash of its name, read either as stored or after decryption. The
/// old test (a backslash in the name) called a plaintext archive whose
/// first record is a bare directory "encrypted" - and XOR-corrupted it.
/// A table that matches neither way is an error: refuse to write.
pub fn is_encrypted(archive: &Path) -> std::io::Result<bool> {
    let total = std::fs::metadata(archive)?.len();
    if total < TABLE {
        return Err(std::io::Error::other("too small to be an archive"));
    }
    let mut f = File::open(archive)?;
    f.seek(SeekFrom::Start(total - TABLE))?;
    let mut first = [0u8; BUCKET];
    f.read_exact(&mut first)?;
    let valid = |r: &Rec| {
        let end = r.iter().position(|&b| b == 0).unwrap_or(0);
        end > 0 && end <= MAX_PATH && std::str::from_utf8(&r[..end]).map(|n| u64_at(r, 232) == crate::kspkg::entry_id(n)).unwrap_or(false)
    };
    if valid(&first) {
        return Ok(false);
    }
    let mut dec = first;
    crate::kspkg::decrypt(&mut dec, 0);
    if valid(&dec) {
        return Ok(true);
    }
    Err(std::io::Error::other("the archive table is not recognised - refusing to touch it"))
}

fn build_table(records: &mut Vec<Rec>) -> Result<Vec<u8>, String> {
    records.sort_by_key(|r| u64_at(r, 232));
    let ids: Vec<u64> = records.iter().map(|r| u64_at(r, 232)).collect();
    if ids.windows(2).any(|w| w[0] >= w[1]) {
        return Err("record ids are not unique and ascending".into());
    }
    let mut table = vec![0u8; TABLE as usize];
    for (i, r) in records.iter().enumerate() {
        let mut b = *r;
        decrypt(&mut b, 0);
        table[i * BUCKET..(i + 1) * BUCKET].copy_from_slice(&b);
    }
    let mut blank = [0u8; BUCKET];
    decrypt(&mut blank, 0);
    for i in records.len()..(TABLE as usize / BUCKET) {
        table[i * BUCKET..(i + 1) * BUCKET].copy_from_slice(&blank);
    }
    Ok(table)
}

/// Apply `changes` (archive path -> new bytes) to the records, appending the
/// payloads with `put` from `cursor`. Order is the order given, like the
/// Python dict. Returns (added, replaced, end cursor).
fn apply(records: &mut Vec<Rec>, changes: &[(String, Vec<u8>)], mut cursor: u64,
         put: &mut dyn FnMut(&[u8]) -> std::io::Result<()>) -> Result<(Vec<String>, Vec<String>, u64), String> {
    let mut idx: std::collections::HashMap<String, usize> = records.iter().enumerate()
        .map(|(i, r)| (record_path(r).to_lowercase(), i)).collect();
    let (mut added, mut replaced) = (vec![], vec![]);
    for (path, blob) in changes {
        let mut enc = blob.clone();
        decrypt(&mut enc, 0); // ⚠ phase 0, not the file offset
        put(&enc).map_err(|e| e.to_string())?;
        match idx.get(&path.to_lowercase()) {
            Some(&i) => {
                records[i][240..244].copy_from_slice(&(blob.len() as u32).to_le_bytes());
                records[i][248..256].copy_from_slice(&cursor.to_le_bytes());
                replaced.push(path.clone());
            }
            None => {
                records.push(bucket(path, blob.len() as u32, cursor, None)?);
                idx.insert(path.to_lowercase(), records.len() - 1);
                added.push(path.clone());
            }
        }
        cursor += blob.len() as u64;
    }
    Ok((added, replaced, cursor))
}

/// Copy `src` to `dst`, adding or replacing entries. Nothing in the data
/// region moves; payloads go where the table used to begin.
pub fn write_archive(src: &Path, dst: &Path, changes: &[(String, Vec<u8>)]) -> Result<Value, String> {
    let _lock = crate::archlock::lock()?;
    if !is_encrypted(src).map_err(|e| e.to_string())? {
        return Err(format!("{} is not an encrypted archive - refusing to write", src.display()));
    }
    let (start, mut records) = read_records(src).map_err(|e| e.to_string())?;
    let repaired = repair_headers(&mut records);
    let mut fin = File::open(src).map_err(|e| e.to_string())?;
    let mut fout = std::io::BufWriter::with_capacity(1 << 22, File::create(dst).map_err(|e| e.to_string())?);
    std::io::copy(&mut (&mut fin).take(start), &mut fout).map_err(|e| e.to_string())?;
    let (added, replaced, _end) = apply(&mut records, changes, start, &mut |b| fout.write_all(b))?;
    let table = build_table(&mut records)?;
    fout.write_all(&table).map_err(|e| e.to_string())?;
    fout.flush().map_err(|e| e.to_string())?;
    drop(fout);
    let size = std::fs::metadata(dst).map(|m| m.len()).unwrap_or(0);
    Ok(json!({"added": added, "replaced": replaced, "repaired": repaired,
              "records": records.len(), "size": size}))
}

/// Replace or add entries by rewriting only the index tail, in place. The
/// previous 64 MiB tail is saved as <archive>.bak_tables first, and put back
/// if anything fails.
pub fn write_inplace(archive: &Path, changes: &[(String, Vec<u8>)]) -> Result<Value, String> {
    let _lock = crate::archlock::lock()?;
    if !is_encrypted(archive).map_err(|e| e.to_string())? {
        return Err(format!("{} is not an encrypted archive - refusing to write", archive.display()));
    }
    let (start, mut records) = read_records(archive).map_err(|e| e.to_string())?;
    let repaired = repair_headers(&mut records);
    let orig = std::fs::metadata(archive).map_err(|e| e.to_string())?.len();
    let mut tail = vec![];
    {
        let mut f = File::open(archive).map_err(|e| e.to_string())?;
        f.seek(SeekFrom::Start(start)).map_err(|e| e.to_string())?;
        f.read_to_end(&mut tail).map_err(|e| e.to_string())?;
    }
    let bak = format!("{}.bak_tables", archive.display());
    std::fs::write(&bak, &tail).map_err(|e| e.to_string())?;
    let mut run = || -> Result<(Vec<String>, Vec<String>, usize), String> {
        let mut f = OpenOptions::new().read(true).write(true).open(archive).map_err(|e| e.to_string())?;
        f.seek(SeekFrom::Start(start)).map_err(|e| e.to_string())?;
        let (added, replaced, end) = apply(&mut records, changes, start, &mut |b| f.write_all(b))?;
        let table = build_table(&mut records)?;
        f.write_all(&table).map_err(|e| e.to_string())?;
        f.set_len(end + TABLE).map_err(|e| e.to_string())?;
        Ok((added, replaced, records.len()))
    };
    match run() {
        Ok((added, replaced, n)) => {
            let size = std::fs::metadata(archive).map(|m| m.len()).unwrap_or(0);
            Ok(json!({"added": added, "replaced": replaced, "repaired": repaired,
                      "records": n, "size": size, "backup": bak,
                      "undo": {"start": start, "len": orig}}))
        }
        Err(e) => {
            if let Ok(mut f) = OpenOptions::new().write(true).open(archive) {
                let _ = f.seek(SeekFrom::Start(start));
                let _ = f.write_all(&tail);
                let _ = f.set_len(orig);
            }
            Err(e)
        }
    }
}

/// Put back what write_inplace() replaced, from its .bak_tables copy (when
/// the result does not verify). `written` is write_inplace's answer.
pub fn undo_inplace(archive: &Path, written: &Value) -> Result<(), String> {
    let _lock = crate::archlock::lock()?;
    let start = written.pointer("/undo/start").and_then(|v| v.as_u64()).ok_or("nothing to undo")?;
    let len = written.pointer("/undo/len").and_then(|v| v.as_u64()).ok_or("nothing to undo")?;
    let bak = written.get("backup").and_then(|v| v.as_str()).ok_or("nothing to undo")?;
    let tail = std::fs::read(bak).map_err(|e| format!("cannot read {bak}: {e}"))?;
    if start + tail.len() as u64 != len {
        return Err("the saved table does not fit this archive any more".into());
    }
    let mut f = OpenOptions::new().write(true).open(archive).map_err(|e| e.to_string())?;
    f.seek(SeekFrom::Start(start)).map_err(|e| e.to_string())?;
    f.write_all(&tail).map_err(|e| e.to_string())?;
    f.set_len(len).map_err(|e| e.to_string())?;
    Ok(())
}

/// Structural check: sorted, unique, complete headers.
pub fn verify(archive: &Path) -> Value {
    let Ok((_s, records)) = read_records(archive) else { return json!({"ok": false}) };
    let ids: Vec<u64> = records.iter().map(|r| u64_at(r, 232)).collect();
    let mut bad = vec![];
    for r in &records {
        let p = record_path(r);
        let kind = u16_at(r, 228);
        if !(kind == KIND_FILE || kind == KIND_DIR) || u16_at(r, 230) as usize != p.chars().count()
            || u64_at(r, 232) != entry_id(&p)
        {
            bad.push(p);
        }
    }
    let sorted = ids.windows(2).all(|w| w[0] <= w[1]);
    let mut u = ids.clone();
    u.dedup();
    let unique = u.len() == ids.len();
    let ok = sorted && unique && bad.is_empty();
    bad.truncate(10);
    json!({"records": records.len(), "sorted": sorted, "unique": unique, "bad_headers": bad, "ok": ok})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archive_with(first: &Rec, name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("acecm_enc_test_{name}_{}.kspkg", std::process::id()));
        let mut t = vec![0u8; TABLE as usize];
        t[..BUCKET].copy_from_slice(first);
        std::fs::write(&p, t).unwrap();
        p
    }

    #[test]
    fn tells_plaintext_from_encrypted() {
        // a bare directory first: the case the old backslash test got wrong
        let plain = bucket("content", 0, 0, None).unwrap();
        let p = archive_with(&plain, "plain");
        assert!(!is_encrypted(&p).unwrap());
        let mut enc = plain;
        crate::kspkg::decrypt(&mut enc, 0);
        let e = archive_with(&enc, "enc");
        assert!(is_encrypted(&e).unwrap());
        let g = archive_with(&[0x41u8; BUCKET], "junk");
        assert!(is_encrypted(&g).is_err());
        for f in [p, e, g] {
            let _ = std::fs::remove_file(f);
        }
    }
}
