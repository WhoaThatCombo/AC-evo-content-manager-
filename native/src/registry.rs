//! The share list (<data>/registry.json) and serving its content to
//! joiners: the public list (with its size cache), per-server manifests,
//! declared files and one-tar track packs. Ported from registry.py.

use serde_json::{json, Map, Value};

use crate::{misc, paths, profiles};

fn path() -> std::path::PathBuf {
    paths::data().join("registry.json")
}

pub fn template() -> Map<String, Value> {
    match json!({"id": "", "name": "My EVO server", "description": "", "ip": "", "port": 9700,
                 "profile_id": "", "required_mods": [], "required_tracks": [], "public": true}) {
        Value::Object(m) => m,
        _ => unreachable!(),
    }
}

pub fn load() -> Vec<Value> {
    std::fs::read(path()).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.as_array().cloned()).unwrap_or_default()
}

pub fn save(items: &[Value]) -> std::io::Result<()> {
    crate::profiles::write_json_indent2(&path(), &Value::Array(items.to_vec()))
}

/// The game port an entry should advertise: its linked profile, else the
/// only profile when there is exactly one.
fn profile_port(profile_id: &str) -> i64 {
    let profs = profiles::load();
    if !profile_id.is_empty() {
        if let Some(p) = profs.iter().find(|p| p.get("id").and_then(|v| v.as_str()) == Some(profile_id)) {
            return p.get("tcp_port").and_then(|v| v.as_i64()).unwrap_or(0);
        }
    }
    if profs.len() == 1 {
        return profs[0].get("tcp_port").and_then(|v| v.as_i64()).unwrap_or(0);
    }
    0
}

/// Fill only what is blank: a host who typed an address or port meant it.
pub fn autofill(e: &mut Map<String, Value>) {
    if e.get("ip").and_then(|v| v.as_str()).map(|s| s.trim().is_empty()).unwrap_or(true) {
        e.insert("ip".into(), json!(misc::public_ipv4()));
    }
    let port = e.get("port").and_then(|v| v.as_i64()).unwrap_or(0);
    if port == 0 || port == 9700 {
        let pid = e.get("profile_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let p = profile_port(&pid);
        if p != 0 {
            e.insert("port".into(), json!(p));
        }
    }
}

pub fn upsert(entry: &Value) -> Value {
    let mut e = template();
    if let Value::Object(m) = entry {
        for (k, v) in m {
            e.insert(k.clone(), v.clone());
        }
    }
    autofill(&mut e);
    if e.get("id").and_then(|v| v.as_str()).unwrap_or("").is_empty() {
        e.insert("id".into(), json!(profiles::new_id()));
    }
    let id = e["id"].clone();
    let mut items: Vec<Value> = load().into_iter().filter(|x| x.get("id") != Some(&id)).collect();
    let v = Value::Object(e);
    items.push(v.clone());
    let _ = save(&items);
    v
}

pub fn remove(id: &str) {
    let items: Vec<Value> = load().into_iter().filter(|x| x.get("id").and_then(|v| v.as_str()) != Some(id)).collect();
    let _ = save(&items);
}

// ------------------------------------------------------------------ hashes --
// SHA-256 cached on (size, mtime_ns) in <data>/hashes.json - the SAME file
// and format the backend keeps. ⚠ The backend holds its copy in memory and
// rewrites the whole file, so ours is merged into what is on disk at write
// time; losing an entry to the other side only costs one re-hash.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

static HASHES: Mutex<Option<Map<String, Value>>> = Mutex::new(None);
static HASH_DIRTY: Mutex<Option<Map<String, Value>>> = Mutex::new(None);

fn hashcache() -> PathBuf {
    paths::data().join("hashes.json")
}

fn mtime_ns(md: &std::fs::Metadata) -> u128 {
    md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0)
}

fn abspath(p: &Path) -> String {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf()).to_string_lossy().into_owned()
}

pub fn file_digest(path: &Path) -> Option<String> {
    use sha2::Digest;
    use std::io::Read;
    let md = std::fs::metadata(path).ok()?;
    let key = abspath(path);
    let (size, ns) = (md.len(), mtime_ns(&md));
    {
        let mut g = HASHES.lock().unwrap();
        let c = g.get_or_insert_with(|| std::fs::read(hashcache()).ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| if let Value::Object(m) = v { Some(m) } else { None }).unwrap_or_default());
        if let Some(hit) = c.get(&key) {
            if hit.get("size").and_then(|v| v.as_u64()) == Some(size)
                && hit.get("mtime_ns").and_then(|v| v.as_u64()).map(|x| x as u128) == Some(ns) {
                if let Some(s) = hit.get("sha256").and_then(|v| v.as_str()) {
                    return Some(s.to_string());
                }
            }
        }
    }
    let mut f = std::fs::File::open(path).ok()?;
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    let digest: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    let rec = json!({"size": size, "mtime_ns": ns as u64, "sha256": digest});
    HASHES.lock().unwrap().get_or_insert_with(Map::new).insert(key.clone(), rec.clone());
    HASH_DIRTY.lock().unwrap().get_or_insert_with(Map::new).insert(key, rec);
    Some(digest)
}

pub fn flush_cache() {
    let dirty = HASH_DIRTY.lock().unwrap().take().unwrap_or_default();
    if dirty.is_empty() {
        return;
    }
    let mut disk: Map<String, Value> = std::fs::read(hashcache()).ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| if let Value::Object(m) = v { Some(m) } else { None }).unwrap_or_default();
    for (k, v) in dirty {
        disk.insert(k, v);
    }
    let tmp = hashcache().with_extension("json.part");
    if std::fs::write(&tmp, profiles::py_dumps(&Value::Object(disk))).is_ok() {
        let _ = std::fs::rename(&tmp, hashcache());
    }
}

// ---------------------------------------------------------------- manifest --
fn home() -> PathBuf {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_default()
}

fn sv(e: &Value, k: &str) -> String {
    e.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string()
}

fn strs(e: &Value, k: &str) -> Vec<String> {
    e.get(k).and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
}

/// Both halves of a car mod, from either side's mod folder.
fn mod_files(name: &str) -> Vec<(String, PathBuf)> {
    let mut dirs: Vec<String> = Vec::new();
    for d in [crate::mods::mods_dir(), crate::mods::client_mods_dir()] {
        if !d.is_empty() && Path::new(&d).is_dir() && !dirs.contains(&d) {
            dirs.push(d);
        }
    }
    let mut out = Vec::new();
    for ext in [".kspkg", ".json"] {
        for d in &dirs {
            let p = Path::new(d).join(format!("{name}{ext}"));
            if p.is_file() {
                out.push((format!("mods/{name}{ext}"), p));
                break;
            }
        }
    }
    out
}

pub fn mod_files_pub(name: &str) -> Vec<(String, PathBuf)> {
    mod_files(name)
}

pub fn track_files_pub(folder: &str) -> Vec<(String, PathBuf)> {
    track_files(folder)
}

/// os.walk order: this folder's files, then each subfolder in turn.
fn walk_files(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>, folder: &str) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut subdirs = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            subdirs.push(p);
        } else {
            let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().replace('\\', "/");
            out.push((format!("tracks/{folder}/{rel}"), p));
        }
    }
    for d in subdirs {
        walk_files(root, &d, out, folder);
    }
}

/// Everything in a track folder; the IMPORTED folder first (a deploy
/// package is only the logic, the art is in Saved Games).
fn track_files(folder: &str) -> Vec<(String, PathBuf)> {
    let roots = [
        home().join("Saved Games").join("ACE").join("mods").join("content").join("tracks").join(folder),
        paths::data().join("track_packages").join(folder),
        Path::new(&crate::detect::effective_server_dir()).join("track_packages").join(folder),
        home().join("Downloads").join(folder),
    ];
    let mut out = Vec::new();
    for root in roots {
        if root.is_dir() {
            walk_files(&root, &root, &mut out, folder);
            break;
        }
    }
    out
}

fn find_entry(sid: &str) -> Option<Value> {
    load().into_iter().find(|e| sv(e, "id") == sid)
}

/// Every file a server requires. `digests=false` when only sizes matter.
pub fn manifest(sid: &str, base_url: &str, digests: bool) -> Value {
    let Some(entry) = find_entry(sid) else { return json!({"ok": false, "error": "no such server"}) };
    let mut files = Vec::new();
    let mut missing = Vec::new();
    let mut total: u64 = 0;
    for (kind, names, finder) in [("mod", strs(&entry, "required_mods"), mod_files as fn(&str) -> Vec<(String, PathBuf)>),
                                  ("track", strs(&entry, "required_tracks"), track_files)] {
        for name in names {
            let got = finder(&name);
            if got.is_empty() {
                missing.push(format!("{kind} {name}"));
            }
            for (rel, path) in got {
                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                total += size;
                let sha = if digests { file_digest(&path).unwrap_or_default() } else { String::new() };
                files.push(json!({"path": rel, "size": size, "sha256": sha, "kind": kind,
                                  "url": format!("{base_url}/api/registry/file?id={sid}&path={rel}")}));
            }
        }
    }
    if digests {
        flush_cache();
    }
    let mut server = Map::new();
    for k in ["id", "name", "description", "ip", "port"] {
        server.insert(k.into(), entry.get(k).cloned().unwrap_or(Value::Null));
    }
    json!({"ok": true, "server": server, "files": files, "total_bytes": total, "missing_locally": missing})
}

/// Map a manifest path back to a real file; undeclared means not served.
pub fn resolve(sid: &str, rel: &str) -> Option<PathBuf> {
    let entry = find_entry(sid)?;
    for m in strs(&entry, "required_mods") {
        if let Some((_, p)) = mod_files(&m).into_iter().find(|(r, _)| r == rel) {
            return Some(p);
        }
    }
    for t in strs(&entry, "required_tracks") {
        if let Some((_, p)) = track_files(&t).into_iter().find(|(r, _)| r == rel) {
            return Some(p);
        }
    }
    None
}

// ------------------------------------------------------------ track packs --
fn track_sig(files: &[(String, PathBuf)]) -> String {
    let (mut n, mut total, mut latest) = (0u64, 0u64, 0u128);
    for (_, p) in files {
        if let Ok(md) = std::fs::metadata(p) {
            n += 1;
            total += md.len();
            latest = latest.max(mtime_ns(&md));
        }
    }
    format!("{n}:{total}:{latest}")
}

static PACK_LOCKS: Mutex<Option<HashMap<String, Arc<Mutex<()>>>>> = Mutex::new(None);

fn packs_dir() -> PathBuf {
    paths::data().join("packs")
}

/// One uncompressed tar of a track folder, rebuilt only when files change
/// (one build at a time per track: a joiner pulls it as parallel ranges).
pub fn ensure_track_pack(folder: &str) -> Option<PathBuf> {
    let folder = folder.trim().replace('\\', "/").trim_matches('/').to_string();
    if folder.is_empty() || folder.contains('/') || folder == "." || folder == ".." {
        return None;
    }
    let lk = PACK_LOCKS.lock().unwrap().get_or_insert_with(HashMap::new)
        .entry(folder.clone()).or_insert_with(|| Arc::new(Mutex::new(()))).clone();
    let _g = lk.lock().unwrap();
    build_track_pack(&folder)
}

fn build_track_pack(folder: &str) -> Option<PathBuf> {
    let files = track_files(folder);
    if files.is_empty() {
        return None;
    }
    let d = packs_dir();
    let _ = std::fs::create_dir_all(&d);
    let tar_path = d.join(format!("{folder}.tar"));
    let sig_path = d.join(format!("{folder}.tar.sig"));
    let sha_path = d.join(format!("{folder}.tar.sha256"));
    // v2: the tar also carries acecm_track.json so the joiner can write tables
    let want = format!("v2:{}", track_sig(&files));
    let have = std::fs::read_to_string(&sig_path).map(|s| s.trim().to_string()).unwrap_or_default();
    if have == want && tar_path.is_file() && sha_path.is_file() {
        return Some(tar_path);
    }
    let tmp = d.join(format!("{folder}.tar.part.{}.{:?}", std::process::id(), std::thread::current().id()).replace(['(', ')'], ""));
    let prefix = format!("tracks/{folder}/");
    let built = (|| -> std::io::Result<()> {
        let f = std::fs::File::create(&tmp)?;
        let mut tar = tar::Builder::new(std::io::BufWriter::with_capacity(1 << 20, f));
        tar.mode(tar::HeaderMode::Complete);
        for (rel, path) in &files {
            let arc = rel.strip_prefix(&prefix).map(String::from)
                .unwrap_or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            if arc.is_empty() || arc.starts_with('/') || arc.split('/').any(|p| p == "..") {
                continue;
            }
            tar.append_path_with_name(path, &arc)?;
        }
        let meta = crate::trackdeploy::pack_meta(folder);
        let blob = profiles::py_dumps_indent2(&meta).into_bytes();
        let mut h = tar::Header::new_gnu();
        h.set_size(blob.len() as u64);
        h.set_mode(0o644);
        h.set_mtime(SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0));
        h.set_entry_type(tar::EntryType::Regular);
        tar.append_data(&mut h, "acecm_track.json", blob.as_slice())?;
        let mut w = tar.into_inner()?;
        use std::io::Write;
        w.flush()
    })();
    if let Err(e) = built {
        crate::logs::warn(format!("track pack {folder}: {e}"));
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    let _ = std::fs::remove_file(&tar_path);
    std::fs::rename(&tmp, &tar_path).ok()?;
    let digest = file_digest(&tar_path).unwrap_or_default();
    let _ = std::fs::write(&sig_path, format!("{want}\n"));
    let _ = std::fs::write(&sha_path, format!("{digest}\n"));
    flush_cache();
    Some(tar_path)
}

pub fn track_pack_sha(folder: &str) -> String {
    std::fs::read_to_string(packs_dir().join(format!("{folder}.tar.sha256"))).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// Is this track folder shared on that entry? (pack requests must be)
pub fn shares_track(sid: &str, folder: &str) -> bool {
    find_entry(sid).map(|e| strs(&e, "required_tracks").iter().any(|t| t == folder)).unwrap_or(false)
}

// ------------------------------------------------------------ public list --
struct Sizes {
    key: Option<String>,
    at: Option<Instant>,
    bytes: Option<Map<String, Value>>,
    busy: bool,
}

static SIZES: Mutex<Sizes> = Mutex::new(Sizes { key: None, at: None, bytes: None, busy: false });
const SIZES_TTL: u64 = 300;

fn compute_sizes(entries: &[Value], base_url: &str) -> Map<String, Value> {
    let mut got = Map::new();
    for e in entries {
        let id = sv(e, "id");
        let m = manifest(&id, base_url, false);
        got.insert(id, if m.get("ok") == Some(&json!(true)) { m.get("total_bytes").cloned().unwrap_or(json!(0)) } else { json!(0) });
    }
    got
}

/// content_bytes per entry id, cached - never recomputed on the request
/// path once something usable is cached (a joiner's probe gives up at 4 s).
fn public_sizes(entries: &[Value], base_url: &str) -> Map<String, Value> {
    let mut keys: Vec<String> = entries.iter().map(|e| json!([sv(e, "id"), strs(e, "required_mods"), strs(e, "required_tracks")]).to_string()).collect();
    keys.sort();
    let key = keys.join("|");
    {
        let mut g = SIZES.lock().unwrap();
        let fresh = g.key.as_deref() == Some(key.as_str()) && g.at.map(|t| t.elapsed().as_secs() < SIZES_TTL).unwrap_or(false);
        if fresh {
            return g.bytes.clone().unwrap_or_default();
        }
        if g.bytes.as_ref().map(|b| !b.is_empty()).unwrap_or(false) {
            if !g.busy {
                g.busy = true;
                let (entries, base, key) = (entries.to_vec(), base_url.to_string(), key.clone());
                std::thread::spawn(move || {
                    let got = compute_sizes(&entries, &base);
                    let mut g = SIZES.lock().unwrap();
                    g.key = Some(key);
                    g.at = Some(Instant::now());
                    g.bytes = Some(got);
                    g.busy = false;
                });
            }
            return g.bytes.clone().unwrap_or_default();
        }
    }
    let got = compute_sizes(entries, base_url);
    let mut g = SIZES.lock().unwrap();
    g.key = Some(key);
    g.at = Some(Instant::now());
    g.bytes = Some(got.clone());
    got
}

pub fn forget_public_sizes() {
    let mut g = SIZES.lock().unwrap();
    g.key = None;
    g.at = None;
    g.bytes = None;
}

/// What a joiner's sync fetches first.
pub fn public_list(base_url: &str) -> Value {
    let entries: Vec<Value> = load().into_iter().filter(|e| e.get("public").map(|p| p.as_bool().unwrap_or(true)).unwrap_or(true)).collect();
    let sizes = public_sizes(&entries, base_url);
    let mut out = Vec::new();
    for e in &entries {
        let id = sv(e, "id");
        let ip = e.get("ip").cloned().unwrap_or(json!(""));
        let port = e.get("port").cloned().unwrap_or(json!(9700));
        let show = |v: &Value| match v { Value::String(s) => s.clone(), v => v.to_string() };
        out.push(json!({
            "id": id, "name": e.get("name").cloned().unwrap_or(Value::Null),
            "description": e.get("description").cloned().unwrap_or(json!("")),
            "ip": ip, "port": port,
            "join": format!("join:{}:{}", show(&ip), show(&port)),
            "required_mods": e.get("required_mods").cloned().unwrap_or(json!([])),
            "required_tracks": e.get("required_tracks").cloned().unwrap_or(json!([])),
            "content_bytes": sizes.get(&id).cloned().unwrap_or(json!(0)),
            "manifest": format!("{base_url}/api/registry/manifest?id={id}"),
        }));
    }
    json!({"servers": out, "generated": SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)})
}
