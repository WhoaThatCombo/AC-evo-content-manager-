//! The penalty toggle on the Servers page - ported from penalties.py (whose
//! core is tools/penalties_tool.py, proven working).
//!
//! Two entries inside content.kspkg are cleared: the time-attack
//! penalty-investigation defaults, and the trigger list embedded in the
//! practice season (at a fixed nested field path).
//!
//! ⚠ A Kunos content update replaces content.kspkg wholesale and with it
//! re-enables penalties: the state is always READ, never assumed.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Value};

use crate::kspkg::{self, BUCKET, TABLE};

const TARGET1: &str = r"system\defaults\timeattack.penaltyinvestigations";
const TARGET2: &str = r"content\data\practice.seasondefinition";
const BAK1: &str = "timeattack_penaltyinvestigations.bak";
const BAK2: &str = "practice_seasondefinition.bak";
const NESTED_PATH: [u64; 7] = [10, 2, 4, 2, 25, 2, 22];

// ------------------------------------------------------- raw protobuf --
fn read_varint(d: &[u8], mut p: usize) -> Option<(u64, usize)> {
    let (mut r, mut shift) = (0u64, 0u32);
    loop {
        let b = *d.get(p)?;
        p += 1;
        r |= ((b & 0x7f) as u64) << shift;
        if b & 0x80 == 0 {
            return Some((r, p));
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
}

fn write_varint(mut n: u64, out: &mut Vec<u8>) {
    loop {
        let b = (n & 0x7f) as u8;
        n >>= 7;
        if n != 0 { out.push(b | 0x80) } else { out.push(b); return; }
    }
}

enum F<'a> {
    Len(&'a [u8]),
    Var(u64),
    Fixed(&'a [u8]),
}

fn pb_walk(d: &[u8]) -> Option<Vec<(u64, u8, F<'_>)>> {
    let (mut out, mut p) = (Vec::new(), 0usize);
    while p < d.len() {
        let (tag, np) = read_varint(d, p)?;
        p = np;
        let (fnum, wt) = (tag >> 3, (tag & 7) as u8);
        if fnum == 0 {
            return None;
        }
        match wt {
            2 => {
                let (len, np) = read_varint(d, p)?;
                p = np;
                let end = p.checked_add(len as usize)?;
                if end > d.len() {
                    return None;
                }
                out.push((fnum, 2, F::Len(&d[p..end])));
                p = end;
            }
            0 => {
                let (v, np) = read_varint(d, p)?;
                p = np;
                out.push((fnum, 0, F::Var(v)));
            }
            5 | 1 => {
                let n = if wt == 5 { 4 } else { 8 };
                if p + n > d.len() {
                    return None;
                }
                out.push((fnum, wt, F::Fixed(&d[p..p + n])));
                p += n;
            }
            _ => return None,
        }
    }
    Some(out)
}

fn reencode(fields: &[(u64, u8, Vec<u8>, u64)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (fnum, wt, bytes, var) in fields {
        write_varint((fnum << 3) | *wt as u64, &mut out);
        match wt {
            2 => { write_varint(bytes.len() as u64, &mut out); out.extend_from_slice(bytes); }
            0 => write_varint(*var, &mut out),
            _ => out.extend_from_slice(bytes),
        }
    }
    out
}

fn clear_nested(d: &[u8], path: &[u64]) -> Result<Vec<u8>, String> {
    let top = pb_walk(d).ok_or("not a valid message")?;
    let mut fields = Vec::new();
    let mut replaced = false;
    for (fnum, wt, v) in top {
        let (bytes, var) = match v { F::Len(b) | F::Fixed(b) => (b.to_vec(), 0), F::Var(x) => (Vec::new(), x) };
        if fnum == path[0] && wt == 2 && !replaced {
            let nb = if path.len() == 1 { Vec::new() } else { clear_nested(&bytes, &path[1..])? };
            fields.push((fnum, wt, nb, 0));
            replaced = true;
        } else {
            fields.push((fnum, wt, bytes, var));
        }
    }
    if !replaced {
        return Err(format!("field {} not found", path[0]));
    }
    Ok(reencode(&fields))
}

fn nested_is_empty(d: &[u8], path: &[u64]) -> Result<bool, String> {
    let mut node = pb_walk(d).ok_or("unreadable")?;
    for (i, fnum) in path.iter().enumerate() {
        let m = node.iter().find_map(|(f, w, v)| if f == fnum && *w == 2 { if let F::Len(b) = v { Some(*b) } else { None } } else { None })
            .ok_or("path not found (game version may differ)")?;
        if i == path.len() - 1 {
            return Ok(m.is_empty());
        }
        node = pb_walk(m).ok_or("nested parse failed")?;
    }
    Ok(false)
}

// ------------------------------------------------------------ the archive --
fn read_entry(pkg: &Path, rel: &str) -> Result<Option<(Vec<u8>, u32)>, String> {
    let Some(e) = kspkg::find_entry(pkg, rel).map_err(|e| e.to_string())? else { return Ok(None) };
    let mut f = File::open(pkg).map_err(|e| e.to_string())?;
    f.seek(SeekFrom::Start(e.offset)).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; e.size as usize];
    f.read_exact(&mut buf).map_err(|e| e.to_string())?;
    kspkg::decrypt(&mut buf, 0);
    Ok(Some((buf, e.size)))
}

/// Write `content` where the table was, then the table after it with this
/// entry's size/offset updated - exactly penalties_tool's scheme.
fn write_entry(pkg: &Path, rel: &str, content: &[u8]) -> Result<(), String> {
    let _lock = crate::archlock::lock()?;
    let total = std::fs::metadata(pkg).map_err(|e| e.to_string())?.len();
    let table_start = total - TABLE;
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(pkg).map_err(|e| e.to_string())?;
    f.seek(SeekFrom::Start(table_start)).map_err(|e| e.to_string())?;
    let mut table = vec![0u8; TABLE as usize];
    f.read_exact(&mut table).map_err(|e| e.to_string())?;
    // buckets are 256-aligned, so every one decrypts at phase 0: compare the
    // ENCRYPTED needle instead of decrypting a quarter-million buckets
    let mut needle = rel.as_bytes().to_vec();
    needle.push(0);
    let mut enc = needle.clone();
    kspkg::decrypt(&mut enc, 0);
    let idx = table.chunks_exact(BUCKET).position(|b| b.starts_with(&enc)).ok_or(format!("{rel} not found in archive"))?;
    let mut bucket = table[idx * BUCKET..(idx + 1) * BUCKET].to_vec();
    kspkg::decrypt(&mut bucket, 0);
    bucket[240..244].copy_from_slice(&(content.len() as u32).to_le_bytes());
    bucket[248..256].copy_from_slice(&table_start.to_le_bytes());
    kspkg::decrypt(&mut bucket, 0);
    table[idx * BUCKET..(idx + 1) * BUCKET].copy_from_slice(&bucket);
    let mut data = content.to_vec();
    kspkg::decrypt(&mut data, 0);
    f.seek(SeekFrom::Start(table_start)).map_err(|e| e.to_string())?;
    f.write_all(&data).map_err(|e| e.to_string())?;
    f.write_all(&table).map_err(|e| e.to_string())?;
    f.set_len(table_start + data.len() as u64 + TABLE).map_err(|e| e.to_string())?;
    Ok(())
}

/// "off", "on", or a short problem description.
pub fn state(pkg: &Path) -> String {
    let r = (|| -> Result<String, String> {
        let (d1, d2) = (read_entry(pkg, TARGET1)?, read_entry(pkg, TARGET2)?);
        let (Some((_, s1)), Some((d2, _))) = (d1, d2) else { return Ok("target files not found (version mismatch?)".into()) };
        Ok(if s1 == 0 && nested_is_empty(&d2, &NESTED_PATH)? { "off" } else { "on" }.into())
    })();
    r.unwrap_or_else(|e| format!("unreadable ({e})"))
}

pub fn patch(pkg: &Path) -> Result<Vec<String>, String> {
    let mut status = Vec::new();
    let folder = pkg.parent().map(Path::to_path_buf).unwrap_or_default();
    match read_entry(pkg, TARGET1)? {
        None => status.push(format!("{TARGET1} not found - skipped")),
        Some((_, 0)) => status.push(format!("{TARGET1} already empty")),
        Some((d1, s1)) => {
            std::fs::write(folder.join(BAK1), &d1).map_err(|e| e.to_string())?;
            write_entry(pkg, TARGET1, b"")?;
            status.push(format!("cleared {TARGET1} ({s1} -> 0 bytes)"));
        }
    }
    match read_entry(pkg, TARGET2)? {
        None => status.push(format!("{TARGET2} not found - skipped")),
        Some((d2, _)) => {
            let r = (|| -> Result<String, String> {
                if nested_is_empty(&d2, &NESTED_PATH)? {
                    return Ok(format!("{TARGET2} triggers already cleared"));
                }
                std::fs::write(folder.join(BAK2), &d2).map_err(|e| e.to_string())?;
                write_entry(pkg, TARGET2, &clear_nested(&d2, &NESTED_PATH)?)?;
                Ok(format!("cleared embedded triggers in {TARGET2}"))
            })();
            status.push(r.unwrap_or_else(|e| format!("skipped {TARGET2}: {e}")));
        }
    }
    Ok(status)
}

pub fn restore(pkg: &Path) -> Result<Vec<String>, String> {
    let folder = pkg.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut status = Vec::new();
    for (target, bak) in [(TARGET1, BAK1), (TARGET2, BAK2)] {
        let p = folder.join(bak);
        if !p.is_file() {
            status.push(format!("no backup for {target} ({bak}) - cannot restore this one; verify game files in Steam"));
            continue;
        }
        let original = std::fs::read(&p).map_err(|e| e.to_string())?;
        write_entry(pkg, target, &original)?;
        status.push(format!("restored {target} ({} bytes)", original.len()));
    }
    Ok(status)
}

fn server_kspkg() -> PathBuf {
    crate::trackdeploy::server_kspkg()
}

fn client_kspkg() -> Option<PathBuf> {
    crate::trackdeploy::client_kspkg().map(PathBuf::from)
}

fn stat_key(p: &Option<PathBuf>) -> String {
    p.as_ref().and_then(|p| std::fs::metadata(p).ok()).map(|m| format!("{}:{:?}", m.len(), m.modified().ok())).unwrap_or_default()
}

static CACHE: Mutex<Option<(String, Value)>> = Mutex::new(None);

/// State of both installs - cached on the two archives' stat, so a toggle
/// (or a Kunos update) recomputes on its own.
pub fn status() -> Value {
    let server = Some(server_kspkg()).filter(|p| p.is_file());
    let client = client_kspkg().filter(|p| p.is_file());
    let key = format!("{}|{}", stat_key(&server), stat_key(&client));
    if let Some((k, v)) = CACHE.lock().unwrap().as_ref() {
        if *k == key {
            return v.clone();
        }
    }
    let mut out = json!({"ok": true});
    if let Some(s) = &server {
        out["server"] = json!({"path": s.to_string_lossy(), "state": state(s)});
    }
    if let Some(c) = &client {
        out["client"] = json!({"path": c.to_string_lossy(), "state": state(c)});
    }
    *CACHE.lock().unwrap() = Some((key, out.clone()));
    out
}

/// side: "server" or "client"; off disables, false restores.
pub fn set_penalties(side: &str, off: bool) -> Value {
    let pkg = if side == "server" { Some(server_kspkg()) } else { client_kspkg() };
    let Some(pkg) = pkg.filter(|p| p.is_file()) else {
        return json!({"ok": false, "error": format!("{side} content.kspkg not found")});
    };
    if side == "server" && !crate::serverproc::server_pids().is_empty() {
        return json!({"ok": false, "error": "stop the server before changing penalties - it holds content.kspkg open"});
    }
    if side == "client" && crate::procs::game_running() {
        return json!({"ok": false, "needs_close": true, "error": "close the game before changing penalties"});
    }
    match if off { patch(&pkg) } else { restore(&pkg) } {
        Ok(lines) => json!({"ok": true, "detail": lines, "state": state(&pkg)}),
        Err(e) => json!({"ok": false, "error": e}),
    }
}
