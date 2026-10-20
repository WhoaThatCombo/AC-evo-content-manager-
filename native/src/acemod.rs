//! Compressed delivery of shared content: `.acemod` packages.
//!
//! The bundled converter's `pack-share` packs a car mod's `.kspkg` (or a
//! track folder) into an `.acemod`: files byte-identical to one in the
//! game's own content.kspkg travel as path + MD5 and are read back from the
//! joiner's game, the rest is LZMA-compressed. `unpack-share` rebuilds the
//! original byte for byte. A converted car of 580 MB is typically ~45 MB.
//!
//! HOST: for every shared car mod and track folder an `.acemod` is built in
//! the background (one at a time, idle priority, the Convert thread setting) the first time a
//! manifest names it and again whenever its files change. It is cached under
//! <data>\acemod\, keyed by the SHA-256 of what it rebuilds, and advertised
//! in the manifest beside the raw files (`acemods`: size, SHA-256 of the
//! package, SHA-256 of the rebuilt output, the files it covers). The raw
//! files stay listed, so older joiners are unaffected.
//!
//! JOINER: when the plan's missing files are exactly covered by an advert and
//! this machine has the converter and the game, the package is downloaded,
//! rebuilt against the local content.kspkg and every rebuilt file checked
//! against the manifest's SHA-256 before it is moved into place. Any failure
//! (another game version: a stock file missing or changed; a hash mismatch)
//! falls back to the raw files.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Map, Value};

use crate::{logs, paths, registry};

pub const FORMAT: &str = "acemod-1";

fn dir() -> PathBuf {
    paths::data().join("acemod")
}

fn plain(n: &str) -> bool {
    !n.is_empty() && n.len() <= 120 && n.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.')) && n != "." && n != ".." && !n.starts_with('.')
}

fn stem(kind: &str, name: &str) -> String {
    format!("{kind}_{name}")
}

/// SHA-256 over the covered files: "<path>\t<sha256>\n" per file, sorted by
/// path. For a car the package is one file, so it is that file's own digest.
pub fn listing_digest(kind: &str, files: &[(String, String)]) -> String {
    if kind == "car" {
        return files.first().map(|f| f.1.clone()).unwrap_or_default();
    }
    use sha2::Digest;
    let mut v: Vec<&(String, String)> = files.iter().collect();
    v.sort();
    let mut h = sha2::Sha256::new();
    for (p, s) in v {
        h.update(format!("{p}\t{s}\n").as_bytes());
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

// -------------------------------------------------------------- the host --

struct Task {
    kind: String,
    name: String,
    src: PathBuf,
    key: String,
    covers: Vec<String>,
    raw_bytes: u64,
}

static QUEUE: Mutex<VecDeque<Task>> = Mutex::new(VecDeque::new());
static BUSY: Mutex<Option<String>> = Mutex::new(None);
/// sources that failed to pack, by key: not retried until they change
static FAILED: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

fn info_path(kind: &str, name: &str) -> PathBuf {
    dir().join(format!("{}.json", stem(kind, name)))
}

fn read_info(kind: &str, name: &str) -> Option<Map<String, Value>> {
    std::fs::read(info_path(kind, name)).ok().and_then(|b| serde_json::from_slice(&b).ok())
}

/// The advert for one shared car mod (`kind` "car", `files` = its
/// mods/<name>.kspkg) or track folder (`kind` "track", every file), when its
/// package is built and current; otherwise its build is queued and None.
/// `files`: (manifest path, sha256, size); `src`: the .kspkg or the folder.
pub fn advert(kind: &str, name: &str, src: &Path, files: &[(String, String, u64)], url: &str) -> Option<Value> {
    if !plain(name) || files.is_empty() || files.iter().any(|f| f.1.is_empty()) || crate::convert::converter().is_none() {
        return None;
    }
    let pairs: Vec<(String, String)> = files.iter().map(|f| (f.0.clone(), f.1.clone())).collect();
    let key = listing_digest(kind, &pairs);
    let raw: u64 = files.iter().map(|f| f.2).sum();
    if let Some(i) = read_info(kind, name) {
        let file = dir().join(format!("{}.acemod", stem(kind, name)));
        if i.get("out_sha256").and_then(|v| v.as_str()) == Some(key.as_str()) && file.is_file()
            && std::fs::metadata(&file).map(|m| Some(m.len()) == i.get("size").and_then(|v| v.as_u64())).unwrap_or(false) {
            let mut a = json!({
                "format": FORMAT, "kind": kind, "name": name,
                "size": i["size"], "sha256": i["sha256"], "out_sha256": key,
                "raw_bytes": raw, "covers": files.iter().map(|f| f.0.clone()).collect::<Vec<_>>(), "url": url,
            });
            if kind == "track" {
                a["meta"] = crate::trackdeploy::pack_meta(name);
            }
            return Some(a);
        }
    }
    if FAILED.lock().unwrap().as_ref().and_then(|m| m.get(&key)).is_some() {
        return None;
    }
    queue(Task { kind: kind.into(), name: name.into(), src: src.to_path_buf(), key, covers: files.iter().map(|f| f.0.clone()).collect(), raw_bytes: raw });
    None
}

/// The ready package file to serve (only one whose info is complete).
pub fn ready_file(kind: &str, name: &str) -> Option<(PathBuf, String)> {
    if !plain(name) || !matches!(kind, "car" | "track") {
        return None;
    }
    let i = read_info(kind, name)?;
    let f = dir().join(format!("{}.acemod", stem(kind, name)));
    f.is_file().then(|| (f, i.get("sha256").and_then(|v| v.as_str()).unwrap_or("").to_string()))
}

fn queue(t: Task) {
    {
        let busy = BUSY.lock().unwrap();
        let mut q = QUEUE.lock().unwrap();
        let id = stem(&t.kind, &t.name);
        if busy.as_deref() == Some(id.as_str()) || q.iter().any(|x| stem(&x.kind, &x.name) == id) {
            return;
        }
        q.push_back(t);
        if busy.is_some() {
            return;
        }
    }
    std::thread::spawn(worker);
}

fn worker() {
    loop {
        let t = {
            let mut busy = BUSY.lock().unwrap();
            let Some(t) = QUEUE.lock().unwrap().pop_front() else {
                *busy = None;
                return;
            };
            *busy = Some(stem(&t.kind, &t.name));
            t
        };
        match build(&t) {
            Ok(n) => logs::info(format!("share package {}: {} -> {} MB", stem(&t.kind, &t.name), t.raw_bytes >> 20, n >> 20)),
            Err(e) => {
                logs::warn(format!("share package {}: {e}", stem(&t.kind, &t.name)));
                FAILED.lock().unwrap().get_or_insert_with(HashMap::new).insert(t.key.clone(), e);
            }
        }
    }
}

fn build(t: &Task) -> Result<u64, String> {
    let conv = crate::convert::converter().ok_or("no converter in this build")?;
    let d = dir();
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    let s = stem(&t.kind, &t.name);
    let tmp = d.join(format!("{s}.acemod.part"));
    let _ = std::fs::remove_file(&tmp);
    let mut cmd = crate::convert::command(&conv, &["pack-share".into(), t.src.to_string_lossy().into_owned(), tmp.to_string_lossy().into_owned()]);
    // threads: the Convert setting (half the cores, fewer when memory is
    // short), and fewer again for a big source - packing holds about three
    // copies of it in memory, plus ~400 MB per compressing thread. The
    // converter runs at idle priority.
    let (want, _) = crate::convert::threads();
    let (_, avail) = crate::convert::memory();
    let src_bytes = t.raw_bytes.max(std::fs::metadata(&t.src).map(|m| if m.is_file() { m.len() } else { 0 }).unwrap_or(0));
    let n = if avail == 0 { want } else {
        let left = avail.saturating_sub((2u64 << 30) + 3 * src_bytes);
        want.min(((left / (400 << 20)) as usize).max(1))
    };
    cmd.env("ACECONV_SHARE_THREADS", n.to_string()).env("ACECONV_THREADS", n.to_string());
    cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    let o = cmd.output().map_err(|e| format!("could not run the converter: {e}"))?;
    if !o.status.success() || !tmp.is_file() {
        let _ = std::fs::remove_file(&tmp);
        let e = String::from_utf8_lossy(&o.stderr);
        return Err(e.lines().last().unwrap_or("pack-share failed").to_string());
    }
    let size = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
    let sha = registry::file_digest(&tmp).ok_or("could not hash the package")?;
    let file = d.join(format!("{s}.acemod"));
    // the info goes first and names the package's digest, so a stale
    // package next to new info is never served (sizes / digests disagree)
    let _ = std::fs::remove_file(info_path(&t.kind, &t.name));
    std::fs::rename(&tmp, &file).map_err(|e| e.to_string())?;
    let info = json!({"format": FORMAT, "kind": t.kind, "name": t.name, "size": size, "sha256": sha, "out_sha256": t.key,
        "raw_bytes": t.raw_bytes, "covers": t.covers, "built": crate::convert::now()});
    let ip = info_path(&t.kind, &t.name);
    let part = ip.with_extension("json.part");
    std::fs::write(&part, serde_json::to_vec_pretty(&info).unwrap_or_default()).and_then(|_| std::fs::rename(&part, &ip)).map_err(|e| e.to_string())?;
    registry::flush_cache();
    Ok(size)
}

/// What the builder is doing (for the share page).
pub fn status() -> Value {
    let q: Vec<String> = QUEUE.lock().unwrap().iter().map(|t| stem(&t.kind, &t.name)).collect();
    json!({"building": *BUSY.lock().unwrap(), "queued": q})
}

// ------------------------------------------------------------ the joiner --

/// Can this machine rebuild packages (converter + game present)?
pub fn supported() -> bool {
    crate::convert::converter().is_some() && crate::convert::game_content().is_file()
}

fn sv(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

fn origin(url: &str) -> String {
    let Some(i) = url.find("://") else { return String::new() };
    let netloc = url[i + 3..].split(['/', '?', '#']).next().unwrap_or("");
    format!("{}://{netloc}", &url[..i]).to_lowercase()
}

/// The adverts of a manifest that cover only files in `need` (by path) and
/// that this machine can use, each with the bytes it saves.
pub fn usable(man: &Value, need: &[Value]) -> Vec<Value> {
    if !supported() {
        return vec![];
    }
    let have: HashMap<String, &Value> = need.iter().map(|e| (sv(e, "path"), e)).collect();
    man.get("acemods").and_then(|v| v.as_array()).cloned().unwrap_or_default().into_iter().filter(|a| {
        let covers: Vec<String> = a.get("covers").and_then(|v| v.as_array()).map(|c| c.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
        sv(a, "format") == FORMAT && matches!(sv(a, "kind").as_str(), "car" | "track") && plain(&sv(a, "name"))
            && !covers.is_empty() && covers.iter().all(|c| have.contains_key(c))
            && a.get("size").and_then(|v| v.as_u64()).unwrap_or(0) > 0
    }).collect()
}

/// Download one package, rebuild it and move the checked files into place.
/// `entries` are the plan's need entries it covers. Ok = the covered paths.
pub fn install(adv: &Value, entries: &[Value], progress: Option<crate::contentsync::Progress>) -> Result<Vec<String>, String> {
    let kind = sv(adv, "kind");
    let name = sv(adv, "name");
    let url = sv(adv, "url");
    let size = adv.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
    let want = sv(adv, "sha256");
    if !url.contains("/api/registry/acemod") || entries.is_empty() || !plain(&name) || want.is_empty() || size == 0 {
        return Err("not a usable package advert".into());
    }
    // ⚠ only from the host whose files it replaces
    if entries.iter().any(|e| origin(&sv(e, "url")) != origin(&url)) {
        return Err("package offered by another host".into());
    }
    let conv = crate::convert::converter().ok_or("no converter")?;
    let first = PathBuf::from(sv(&entries[0], "dest"));
    let work = first.parent().ok_or("no destination")?.to_path_buf();
    // a folder of our own beside the destination: same drive, cheap renames
    let tmpdir = if kind == "car" { work.join(".acecm_acemod") } else {
        PathBuf::from(crate::contentsync::tracks_dir()).join(format!(".{name}.acemod"))
    };
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).map_err(|e| e.to_string())?;
    let r = (|| -> Result<Vec<String>, String> {
        let pkg = tmpdir.join(format!("{name}.acemod"));
        crate::contentsync::download_to(&url, size, &pkg, progress)?;
        if registry::file_digest(&pkg).unwrap_or_default() != want {
            return Err("package checksum mismatch".into());
        }
        let out = if kind == "car" { tmpdir.join(format!("{name}.kspkg")) } else { tmpdir.join("out") };
        let mut cmd = crate::convert::command(&conv, &["unpack-share".into(), pkg.to_string_lossy().into_owned(), out.to_string_lossy().into_owned()]);
        cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
        let o = cmd.output().map_err(|e| format!("could not run the converter: {e}"))?;
        let _ = std::fs::remove_file(&pkg);
        if !o.status.success() {
            let e = String::from_utf8_lossy(&o.stderr);
            return Err(format!("rebuild failed (another game version?): {}", e.lines().last().unwrap_or("")));
        }
        // every rebuilt file against the manifest, before anything moves
        let mut moves: Vec<(PathBuf, PathBuf)> = Vec::new();
        let mut pairs = Vec::new();
        for e in entries {
            let rel = sv(e, "path");
            let src = if kind == "car" {
                if !rel.to_lowercase().ends_with(".kspkg") {
                    return Err(format!("{rel} is not the car package"));
                }
                out.clone()
            } else {
                let prefix = format!("tracks/{name}/");
                let sub = rel.strip_prefix(&prefix).ok_or_else(|| format!("{rel} is not in track {name}"))?;
                if sub.split('/').any(|p| p.is_empty() || p == "." || p == "..") {
                    return Err(format!("bad path {rel}"));
                }
                out.join(sub.replace('/', std::path::MAIN_SEPARATOR_STR))
            };
            let sha = sv(e, "sha256");
            let got = registry::file_digest(&src).unwrap_or_default();
            if sha.is_empty() || got != sha {
                return Err(format!("{rel}: rebuilt file differs from the host's"));
            }
            pairs.push((rel.clone(), sha));
            moves.push((src, PathBuf::from(sv(e, "dest"))));
        }
        if listing_digest(&kind, &pairs) != sv(adv, "out_sha256") {
            return Err("rebuilt output does not match the advert".into());
        }
        let mut done = Vec::new();
        for ((src, dst), e) in moves.iter().zip(entries) {
            if let Some(p) = dst.parent() {
                std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
            }
            if dst.exists() {
                std::fs::remove_file(dst).map_err(|x| format!("{} is in use: {x}", dst.display()))?;
            }
            if std::fs::rename(src, dst).is_err() {
                std::fs::copy(src, dst).map_err(|x| format!("{}: {x}", dst.display()))?;
            }
            done.push(sv(e, "path"));
        }
        if kind == "track" {
            if let Some(m) = adv.get("meta").filter(|m| m.is_object()) {
                let d = PathBuf::from(crate::contentsync::tracks_dir()).join(&name);
                let _ = std::fs::write(d.join("acecm_track.json"), crate::profiles::py_dumps_indent2(m));
            }
        }
        Ok(done)
    })();
    let _ = std::fs::remove_dir_all(&tmpdir);
    registry::flush_cache();
    r
}
