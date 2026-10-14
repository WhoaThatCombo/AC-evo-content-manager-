//! Protobuf schemas extracted from the user's OWN game install - ported
//! from acecm/protos.py. Never redistributed: derived from Kunos' binary, so
//! each machine extracts them from its copy into <data>/protos/*.desc.
//!
//! ⚠ The descriptor's length is NOT marked in the exe; it is computed by
//! walking top-level FileDescriptorProto fields until the wire stops making
//! sense (guessing by size truncated most of them).
//! ⚠ The name pattern must accept a 0x0a length byte (a ten-character name),
//! or Math.proto - and everything that depends on it - disappears.
//! ⚠ Keyed on the SOURCE exes (path|size|mtime in .sources): a game update
//! changes the schemas, and a stale cache parses with last version's layout.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use prost::Message;
use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor};
use prost_types::FileDescriptorProto;
use serde_json::{json, Value};

use crate::{config, detect, paths};

pub fn cache_dir() -> PathBuf {
    // a tool process is handed the shell's schema folder explicitly
    let d = std::env::var("ACECM_PROTOS").map(PathBuf::from).unwrap_or_else(|_| paths::data().join("protos"));
    let _ = std::fs::create_dir_all(&d);
    d
}

fn sources() -> Vec<String> {
    let mut out = vec![];
    let cfg = config::load();
    let g = config::get_str(&cfg, "game_exe");
    let g = if g.is_empty() { detect::find("game_exe", false) } else { g };
    if !g.is_empty() && Path::new(&g).is_file() {
        out.push(g);
    }
    let s = detect::find("server_exe", false);
    if !s.is_empty() && Path::new(&s).is_file() {
        out.push(s);
    }
    out
}

fn varint(d: &[u8], i: &mut usize) -> Option<u64> {
    let (mut r, mut s) = (0u64, 0u32);
    loop {
        let b = *d.get(*i)?;
        *i += 1;
        if s < 64 {
            r |= ((b & 0x7f) as u64) << s;
        }
        if b & 0x80 == 0 {
            return Some(r);
        }
        s += 7;
    }
}

/// Exact length of the descriptor at `off`, by walking its top-level fields.
fn blob_len(d: &[u8], off: usize) -> usize {
    const KNOWN: [u64; 10] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 12];
    let end = d.len().min(off + (1 << 22));
    let mut i = off;
    while i < end {
        let start = i;
        let Some(key) = varint(d, &mut i) else { return start - off };
        let (f, wt) = (key >> 3, key & 7);
        if !KNOWN.contains(&f) || (wt != 0 && wt != 2) {
            return start - off;
        }
        if wt == 0 {
            if varint(d, &mut i).is_none() {
                return start - off;
            }
        } else {
            let Some(ln) = varint(d, &mut i) else { return start - off };
            i = i.saturating_add(ln as usize);
        }
        if i > end {
            return start - off;
        }
    }
    end - off
}

fn parse_at(d: &[u8], off: usize) -> Option<FileDescriptorProto> {
    let n = blob_len(d, off);
    for size in [n, n.saturating_sub(1), n.saturating_sub(2)] {
        if size < 8 {
            continue;
        }
        if let Ok(fd) = FileDescriptorProto::decode(&d[off..off + size]) {
            if fd.name.as_deref().map(|n| !n.is_empty()).unwrap_or(false) {
                return Some(fd);
            }
        }
    }
    None
}

fn stamp(srcs: &[String]) -> String {
    srcs.iter().map(|s| {
        let m = std::fs::metadata(s);
        let (size, mt) = m.map(|m| (m.len(), m.modified().ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0)))
            .unwrap_or((0, 0));
        format!("{s}|{size}|{mt}")
    }).collect::<Vec<_>>().join("\n")
}

fn descs(d: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(d).map(|rd| rd.flatten().map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("desc")).collect()).unwrap_or_default()
}

/// Pull FileDescriptorProtos out of the game/server exes into <data>/protos.
pub fn extract(force: bool) -> Value {
    let d = cache_dir();
    let srcs = sources();
    let st = stamp(&srcs);
    let stampfile = d.join(".sources");
    if !force && !descs(&d).is_empty() {
        if std::fs::read_to_string(&stampfile).ok().as_deref() == Some(st.as_str()) {
            return json!({"ok": true, "cached": true, "dir": d.to_string_lossy(), "count": descs(&d).len()});
        }
        for p in descs(&d) {
            let _ = std::fs::remove_file(p);
        }
    }
    if srcs.is_empty() {
        return json!({"ok": false, "error": "no game or server executable found to read the schemas from - set game_exe in Settings"});
    }
    let mut found: BTreeMap<String, FileDescriptorProto> = BTreeMap::new();
    for src in &srcs {
        let Ok(data) = std::fs::read(src) else { continue };
        // \x0a <len> <name>.proto, where <len> is exactly the name's length
        let mut i = 0;
        while i + 2 < data.len() {
            if data[i] == 0x0a {
                let len = data[i + 1] as usize;
                if (3 + 6..=60 + 6).contains(&len) && i + 2 + len <= data.len() {
                    let name = &data[i + 2..i + 2 + len];
                    let ok = name.ends_with(b".proto") && name[..len - 6].iter()
                        .all(|&c| c.is_ascii_alphanumeric() || c == b'_' || c == b'/');
                    if ok {
                        if let Some(fd) = parse_at(&data, i) {
                            let n = fd.name.clone().unwrap_or_default();
                            let bigger = found.get(&n).map(|p| fd.encoded_len() > p.encoded_len()).unwrap_or(true);
                            if bigger {
                                found.insert(n, fd);
                            }
                        }
                    }
                }
            }
            i += 1;
        }
    }
    for (name, fd) in &found {
        let safe = name.replace(['/', '\\'], "_") + ".desc";
        let _ = std::fs::write(d.join(safe), fd.encode_to_vec());
    }
    if !found.is_empty() {
        let _ = std::fs::write(&stampfile, &st);
    }
    *POOL.lock().unwrap() = None;
    json!({"ok": !found.is_empty(), "dir": d.to_string_lossy(), "count": found.len(),
           "sources": srcs.iter().map(|s| Path::new(s).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).collect::<Vec<_>>(),
           "error": if found.is_empty() { json!("no descriptors found in those files") } else { Value::Null }})
}

static POOL: Mutex<Option<DescriptorPool>> = Mutex::new(None);

/// Every .desc we have, added dependencies-first (retry until a pass adds
/// nothing). ⚠ Calls extract() each time the pool is built: it is cheap when
/// nothing moved and it is the only thing that notices a game update.
pub fn pool() -> DescriptorPool {
    if let Some(p) = POOL.lock().unwrap().as_ref() {
        return p.clone();
    }
    let _ = extract(false);
    let mut files: BTreeMap<String, FileDescriptorProto> = BTreeMap::new();
    for p in descs(&cache_dir()) {
        if let Ok(b) = std::fs::read(&p) {
            if let Ok(fd) = FileDescriptorProto::decode(b.as_slice()) {
                files.entry(fd.name.clone().unwrap_or_default()).or_insert(fd);
            }
        }
    }
    let mut pool = DescriptorPool::new();
    let mut pending: Vec<FileDescriptorProto> = files.into_values().collect();
    loop {
        let before = pending.len();
        pending.retain(|fd| pool.add_file_descriptor_proto(fd.clone()).is_err());
        if pending.is_empty() || pending.len() == before {
            break;
        }
    }
    *POOL.lock().unwrap() = Some(pool.clone());
    pool
}

/// A message type by its short name (the game's names are unqualified).
pub fn find(name: &str) -> Option<MessageDescriptor> {
    let p = pool();
    if let Some(m) = p.get_message_by_name(name) {
        return Some(m);
    }
    let hit = p.all_messages().find(|m| m.name() == name);
    hit
}

pub fn has(name: &str) -> bool {
    find(name).is_some()
}

/// Decode `raw` as message `name`; Err unless the WHOLE buffer was consumed.
pub fn decode(name: &str, raw: &[u8]) -> Result<DynamicMessage, String> {
    let d = find(name).ok_or_else(|| format!("the game's message schemas are not available ({name})"))?;
    DynamicMessage::decode(d, raw).map_err(|e| e.to_string())
}

pub fn state() -> Value {
    let p = pool();
    json!({"dir": cache_dir().to_string_lossy(), "descriptors": descs(&cache_dir()).len(),
           "messages": p.all_messages().count(), "files": p.files().count()})
}
