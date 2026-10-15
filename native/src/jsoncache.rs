//! Parsed JSON files, reused while the file is unchanged.
//!
//! ⚠ config.json and paths.json are read by nearly every request (several
//! times by some), and each read was an open + parse. The key is the file's
//! size and mtime from one stat call; a write by anyone - us, the Python
//! backend, a text editor - changes it. Our own writers also call forget(),
//! so a write is seen even inside one mtime tick.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use serde_json::Value;

type Key = (u64, Option<SystemTime>);
static CACHE: Mutex<Option<HashMap<PathBuf, (Key, Value)>>> = Mutex::new(None);

/// The file parsed as JSON, or None when it is missing or not JSON.
pub fn read(p: &Path) -> Option<Value> {
    let m = std::fs::metadata(p).ok()?;
    let key: Key = (m.len(), m.modified().ok());
    if let Some((k, v)) = CACHE.lock().unwrap().as_ref().and_then(|c| c.get(p)) {
        if *k == key {
            return Some(v.clone());
        }
    }
    let v: Value = serde_json::from_slice(&std::fs::read(p).ok()?).ok()?;
    CACHE.lock().unwrap().get_or_insert_with(HashMap::new).insert(p.to_path_buf(), (key, v.clone()));
    Some(v)
}

/// Drop a file's cached copy (after writing it).
pub fn forget(p: &Path) {
    if let Some(c) = CACHE.lock().unwrap().as_mut() {
        c.remove(p);
    }
}
