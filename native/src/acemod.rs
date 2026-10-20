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

/// The content.kspkg packages are built against and rebuilt from. Normally
/// the game's (ACECM_CLIENT_KSPKG included); ACECM_SHARE_BASE points only
/// this at another archive (tests: tables written to a small copy while the
/// stock files are read from the real game, read-only).
fn base_content() -> PathBuf {
    std::env::var_os("ACECM_SHARE_BASE").map(PathBuf::from).unwrap_or_else(crate::convert::game_content)
}

fn share_command(conv: &Path, args: &[String]) -> std::process::Command {
    let mut cmd = crate::convert::command(conv, args);
    let base = base_content();
    if base.is_file() {
        cmd.env("ACECONV_GAME_CONTENT", &base);
    }
    cmd
}

/// Run `pack-share src -> out`. `on_stage` hears each step as it starts
/// (the converter's timing lines). Ok = the package's size.
fn run_pack(src: &Path, out: &Path, raw_bytes: u64, on_stage: &dyn Fn(&str)) -> Result<u64, String> {
    use std::io::BufRead;
    let conv = crate::convert::converter().ok_or("no converter in this build")?;
    let _ = std::fs::remove_file(out);
    let mut cmd = share_command(&conv, &["pack-share".into(), src.to_string_lossy().into_owned(), out.to_string_lossy().into_owned()]);
    // threads: the Convert setting (half the cores, fewer when memory is
    // short), and fewer again for a big source - packing holds about three
    // copies of it in memory, plus ~400 MB per compressing thread. The
    // converter runs at idle priority.
    let (want, _) = crate::convert::threads();
    let (_, avail) = crate::convert::memory();
    let src_bytes = raw_bytes.max(std::fs::metadata(src).map(|m| if m.is_file() { m.len() } else { 0 }).unwrap_or(0));
    let n = if avail == 0 { want } else {
        let left = avail.saturating_sub((2u64 << 30) + 3 * src_bytes);
        want.min(((left / (400 << 20)) as usize).max(1))
    };
    cmd.env("ACECONV_SHARE_THREADS", n.to_string()).env("ACECONV_THREADS", n.to_string()).env("ACECONV_SHARE_TIMING", "1");
    cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped());
    on_stage("reading the files");
    let mut child = cmd.spawn().map_err(|e| format!("could not run the converter: {e}"))?;
    let mut last = String::new();
    if let Some(err) = child.stderr.take() {
        for l in std::io::BufReader::new(err).lines().map_while(Result::ok) {
            if l.starts_with("timing: read") {
                on_stage("finding files the game already has");
            } else if l.starts_with("timing: decide") {
                on_stage(&format!("compressing ({n} thread{})", if n == 1 { "" } else { "s" }));
            } else if l.starts_with("timing: compress") {
                on_stage("writing the package");
            } else if !l.trim().is_empty() {
                last = l;
            }
        }
    }
    let st = child.wait().map_err(|e| e.to_string())?;
    if !st.success() || !out.is_file() {
        let _ = std::fs::remove_file(out);
        return Err(if last.is_empty() { "pack-share failed".into() } else { last });
    }
    Ok(std::fs::metadata(out).map(|m| m.len()).unwrap_or(0))
}

fn build(t: &Task) -> Result<u64, String> {
    let d = dir();
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    let s = stem(&t.kind, &t.name);
    let tmp = d.join(format!("{s}.acemod.part"));
    let size = run_pack(&t.src, &tmp, t.raw_bytes, &|_| {})?;
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
    crate::convert::converter().is_some() && base_content().is_file()
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
        let mut cmd = share_command(&conv, &["unpack-share".into(), pkg.to_string_lossy().into_owned(), out.to_string_lossy().into_owned()]);
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

// ------------------------------------------------- the Content library --
//
// Export writes the same package to a file the user hands on; install takes
// one from any drop / path, rebuilds it against this machine's
// content.kspkg in the Convert work folder and installs the result exactly
// like the raw files (install.rs).

/// What an export is doing right now (the app-wide progress bar).
static EXPORT: Mutex<Option<(String, std::time::Instant)>> = Mutex::new(None);

/// The running export, as the progress feed shows it.
pub fn export_progress() -> Option<String> {
    EXPORT.lock().unwrap().as_ref().map(|(w, t)| format!("{w} · {} s", t.elapsed().as_secs()))
}

/// Pack a car mod's .kspkg (its .json beside it rides along) or a track
/// folder into `out`. Blocking; one at a time.
pub fn export(kind: &str, name: &str, src: &Path, out: &Path) -> Result<Value, String> {
    if crate::convert::converter().is_none() {
        return Err("this build has no converter - export as .zip instead".into());
    }
    {
        let mut g = EXPORT.lock().unwrap();
        if g.is_some() {
            return Err("an export is already running".into());
        }
        *g = Some((format!("Packing {name}"), std::time::Instant::now()));
    }
    let t = std::time::Instant::now();
    let raw = if src.is_dir() { dir_bytes(src) } else {
        std::fs::metadata(src).map(|m| m.len()).unwrap_or(0) + std::fs::metadata(src.with_extension("json")).map(|m| m.len()).unwrap_or(0)
    };
    let part = out.with_extension("acemod.part");
    let r = run_pack(src, &part, raw, &|s| {
        if let Some(g) = EXPORT.lock().unwrap().as_mut() {
            g.0 = format!("Packing {name}: {s}");
        }
    }).and_then(|size| {
        let _ = std::fs::remove_file(out);
        std::fs::rename(&part, out).map_err(|e| e.to_string())?;
        Ok(size)
    });
    *EXPORT.lock().unwrap() = None;
    let size = r.inspect_err(|_| { let _ = std::fs::remove_file(&part); })?;
    let secs = (t.elapsed().as_secs_f64() * 10.0).round() / 10.0;
    logs::info(format!("export {kind} {name}: {} -> {} MB in {secs} s", raw >> 20, size >> 20));
    Ok(json!({"ok": true, "kind": kind, "name": name, "path": out.to_string_lossy(), "format": FORMAT,
              "size": size, "raw_bytes": raw, "secs": secs}))
}

fn dir_bytes(d: &Path) -> u64 {
    std::fs::read_dir(d).into_iter().flatten().flatten().map(|e| {
        let p = e.path();
        if p.is_dir() { dir_bytes(&p) } else { e.metadata().map(|m| m.len()).unwrap_or(0) }
    }).sum()
}

/// The file's manifest, read without unpacking anything: the install
/// preview. `kind` is "car" (a .kspkg + .json) or "track" (a folder).
pub fn peek(p: &Path) -> Result<Value, String> {
    use std::io::Read;
    let mut f = std::fs::File::open(p).map_err(|e| e.to_string())?;
    let mut head = [0u8; 12];
    f.read_exact(&mut head).map_err(|_| "not an .acemod (too short)".to_string())?;
    if &head[..8] != b"ACEMOD\x01\n" {
        return Err("not an .acemod package".into());
    }
    let n = u32::from_le_bytes(head[8..12].try_into().unwrap()) as usize;
    if n == 0 || n > 64 << 20 {
        return Err("damaged .acemod (manifest length)".into());
    }
    let mut z = vec![0u8; n];
    f.read_exact(&mut z).map_err(|_| "damaged .acemod (truncated)".to_string())?;
    let mut js = Vec::new();
    liblzma::read::XzDecoder::new(&z[..]).take(256 << 20).read_to_end(&mut js).map_err(|e| format!("damaged .acemod manifest: {e}"))?;
    let m: Value = serde_json::from_slice(&js).map_err(|e| format!("damaged .acemod manifest: {e}"))?;
    let kind = match m.get("kind").and_then(|v| v.as_str()) {
        Some("kspkg") => "car",
        Some("dir") => "track",
        _ => return Err("unknown .acemod content".into()),
    };
    let name = sv(&m, "name");
    if !plain(&name) {
        return Err(format!("bad name in the package: {name:?}"));
    }
    let blobs = m.get("blobs").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let base: Vec<&Value> = blobs.iter().filter(|b| b.get("how").and_then(|v| v.as_str()) == Some("base")).collect();
    let extras: Vec<Value> = m.get("extras").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let extra_bytes: u64 = extras.iter().filter_map(|e| e.get(1).and_then(|v| v.as_u64())).sum();
    // file -> size (folder-relative, '/'): the duplicate check
    let files: Map<String, Value> = if kind == "car" {
        let mut f = Map::new();
        f.insert(format!("{name}.kspkg"), m["out_size"].clone());
        for e in &extras {
            if let (Some(n), Some(s)) = (e.get(0).and_then(|v| v.as_str()), e.get(1).and_then(|v| v.as_u64())) {
                f.insert(n.to_string(), json!(s));
            }
        }
        f
    } else {
        blobs.iter().filter_map(|b| Some((b.get("path")?.as_str()?.replace('\\', "/"), b.get("size")?.clone()))).collect()
    };
    let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    let out = m.get("out_size").and_then(|v| v.as_u64()).unwrap_or(0) + extra_bytes;
    let made = m.get("base_from").and_then(|v| v.as_u64()).unwrap_or(0);
    let here = std::fs::metadata(base_content()).map(|m| m.len()).unwrap_or(0);
    Ok(json!({"format": FORMAT, "kind": kind, "name": name, "size": size, "out_bytes": out,
              "files": files, "file_count": files.len(), "from_game": base.len(),
              "from_game_bytes": base.iter().filter_map(|b| b.get("size").and_then(|v| v.as_u64())).sum::<u64>(),
              "game_build": made, "same_game_build": made == 0 || made == here,
              "can_rebuild": crate::convert::converter().is_some() && (base.is_empty() || base_content().is_file())}))
}

/// The message for a package this game cannot rebuild.
pub const OTHER_GAME: &str = "This package was made with a different game version; ask for the .zip export";

/// Rebuild `pkg` into `out` (a .kspkg path for a car, beside which its
/// .json lands; a folder for a track). The converter checks the MD5 of the
/// whole result.
pub fn unpack(pkg: &Path, out: &Path) -> Result<(), String> {
    let conv = crate::convert::converter().ok_or("this build has no converter, so it cannot install an .acemod")?;
    let mut cmd = share_command(&conv, &["unpack-share".into(), pkg.to_string_lossy().into_owned(), out.to_string_lossy().into_owned()]);
    cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    let o = cmd.output().map_err(|e| format!("could not run the converter: {e}"))?;
    if o.status.success() {
        return Ok(());
    }
    let e = String::from_utf8_lossy(&o.stderr);
    let last = e.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim_start_matches("error: ").to_string();
    logs::warn(format!("acemod {}: {last}", pkg.display()));
    // a stock file missing / changed, or no game at all
    if ["different bytes", "game updated", "game was not found", "does not match the original"].iter().any(|k| last.contains(k)) {
        return Err(format!("{OTHER_GAME} ({last})"));
    }
    Err(if last.is_empty() { "rebuilding the package failed".into() } else { last })
}

/// The Convert page's check of a converted track (`check-track`), on a
/// rebuilt one: Ok, or why it must not be installed.
pub fn check_track(dir: &Path) -> Result<(), String> {
    let conv = crate::convert::converter().ok_or("no converter")?;
    let mut cmd = share_command(&conv, &["check-track".into(), dir.to_string_lossy().into_owned(), base_content().to_string_lossy().into_owned()]);
    cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    let o = cmd.output().map_err(|e| format!("could not run the validator: {e}"))?;
    if o.status.success() {
        return Ok(());
    }
    let mut l: Vec<String> = String::from_utf8_lossy(&o.stdout).lines().chain(String::from_utf8_lossy(&o.stderr).lines()).map(String::from).collect();
    l.retain(|x| !x.trim().is_empty());
    logs::warn(format!("acemod check-track {}: {}", dir.display(), l.join(" | ")));
    Err(format!("the track failed the pre-install check - not installed ({})", l.first().cloned().unwrap_or_default()))
}
