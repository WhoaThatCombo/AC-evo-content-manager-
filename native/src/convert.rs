//! Convert an Assetto Corsa (AC1) car into an EVO mod with the bundled
//! converter (`aceconv.exe`, built from converter/ in this repository), run
//! as a hidden, low-priority child process; then drop the result into the
//! game's (and optionally the server's) mods folder.
//!
//! ACECM only starts the converter and reads its output. Everything the
//! converter needs from the game (the donor car it starts from, the material
//! templates, the sound template) it builds itself from the user's own
//! content.kspkg on first use, into the work folder. One conversion at a
//! time.
//!
//! The converter's commands used here:
//!   car <ac1 dir> <out dir> --id <id> [--content <content.kspkg>]
//!   check-car <car.kspkg> [content.kspkg]
//!   car-id <folder>...            the default id of each folder
//!   legacy-id <mods> cars <ac1 dir>   earlier conversions of the same car
//!   reset-car-settings <id>       forget the game's saved cameras / mirrors

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{LazyLock, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use crate::{config, logs, mods, procs};

pub(crate) struct Job {
    pub(crate) state: &'static str,
    pub(crate) car: String,
    pub(crate) id: String,
    pub(crate) lines: Vec<String>,
    pub(crate) started: Option<Instant>,
    pub(crate) secs: f64,
    pub(crate) pid: u32,
    pub(crate) result: Value,
    pub(crate) cancel: bool,
}

pub(crate) static JOB: LazyLock<Mutex<Job>> = LazyLock::new(|| Mutex::new(Job {
    state: "idle", car: String::new(), id: String::new(), lines: Vec::new(), started: None, secs: 0.0, pid: 0, result: Value::Null, cancel: false,
}));

impl Job {
    pub(crate) fn running(what: String, id: String) -> Job {
        Job { state: "running", car: what, id, lines: Vec::new(), started: Some(Instant::now()), secs: 0.0, pid: 0, result: Value::Null, cancel: false }
    }
}

fn exe_dir() -> PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf())).unwrap_or_default()
}

/// The converter: the copy unpacked from this exe's bundle; then one beside
/// the exe; then (a run from the source tree) the one this build compiled.
/// ACECM_ACECONV overrides all of them (development).
pub fn converter() -> Option<PathBuf> {
    let mut c: Vec<PathBuf> = Vec::new();
    if let Some(p) = std::env::var_os("ACECM_ACECONV") {
        c.push(PathBuf::from(p));
    }
    c.push(crate::bundle::tools_dir().join("aceconv.exe"));
    c.push(exe_dir().join("aceconv.exe"));
    if let Some(p) = option_env!("ACECM_CONVERTER_BUILT") {
        c.push(PathBuf::from(p));
    }
    c.into_iter().find(|p| p.is_file())
}

/// The converter's source revision (converter/UPSTREAM_REV at build time).
pub const CONVERTER_REV: &str = env!("ACECM_CONVERTER_REV");

fn default_ac1_root() -> String {
    let c = [
        PathBuf::from(r"C:\Program Files (x86)\Steam\steamapps\common\assettocorsa"),
        PathBuf::from(r"D:\SteamLibrary\steamapps\common\assettocorsa"),
        PathBuf::from(r"E:\SteamLibrary\steamapps\common\assettocorsa"),
    ];
    c.iter().find(|p| p.is_dir()).or(c.first()).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
}

fn defaults() -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("ac1_root".into(), json!(default_ac1_root()));
    // converted cars are built here (several hundred MB each), plus what the
    // converter generates from the game once (_aceconv\)
    m.insert("out_dir".into(), json!(crate::paths::data().join("converted").to_string_lossy()));
    // 0 = automatic: half the cores, fewer when memory is short
    m.insert("threads".into(), json!(0));
    // 0 = automatic: what is free, less a safety margin
    m.insert("memory_gb".into(), json!(0));
    m
}

/// The settings: defaults overlaid with config.json "convert" (and, from an
/// older development build, "dev_convert" - only its folders carry over).
pub(crate) fn settings() -> Map<String, Value> {
    let mut m = defaults();
    let cfg = config::load();
    let mut apply = |s: &Map<String, Value>, keys: &[&str]| {
        for (k, v) in s {
            if !keys.contains(&k.as_str()) {
                continue;
            }
            match v {
                Value::String(t) if !t.trim().is_empty() => { m.insert(k.clone(), json!(t.trim())); }
                Value::Number(_) => { m.insert(k.clone(), v.clone()); }
                _ => {}
            }
        }
    };
    if let Some(Value::Object(s)) = cfg.get("dev_convert") {
        apply(s, &["ac1_root", "out_dir"]);
    }
    if let Some(Value::Object(s)) = cfg.get("convert") {
        apply(s, &["ac1_root", "out_dir", "threads", "memory_gb"]);
    }
    m
}

pub(crate) fn s(m: &Map<String, Value>, k: &str) -> String {
    m.get(k).and_then(|v| v.as_str()).unwrap_or("").trim().to_string()
}

fn n(m: &Map<String, Value>, k: &str) -> u64 {
    m.get(k).and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|t| t.trim().parse().ok()))).unwrap_or(0)
}

/// The work folder and what the converter keeps in it from the game.
pub(crate) fn out_dir() -> PathBuf {
    PathBuf::from(s(&settings(), "out_dir"))
}

fn generated_dir() -> PathBuf {
    out_dir().join("_aceconv")
}

// ------------------------------------------------------------- threads --

/// (total, available) physical memory in bytes.
pub fn memory() -> (u64, u64) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if unsafe { GlobalMemoryStatusEx(&mut m) } != 0 {
            return (m.ullTotalPhys, m.ullAvailPhys);
        }
    }
    (0, 0)
}

/// Memory one converter worker thread can take at its peak (a car's largest
/// textures in flight), and what the converter needs before any worker.
const PER_THREAD: u64 = 700 << 20;
const BASE: u64 = 900 << 20;
/// left free for the game, the browser and everything else
const MARGIN: u64 = 2 << 30;

/// The worker threads to give the converter now, and why.
pub fn threads() -> (usize, String) {
    let st = settings();
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2);
    let asked = n(&st, "threads") as usize;
    let want = if asked == 0 { (cores / 2).max(1) } else { asked.min(cores) };
    let (total, avail) = memory();
    let budget = match n(&st, "memory_gb") {
        0 => avail.saturating_sub(MARGIN),
        gb => (gb << 30).min(avail),
    };
    if total == 0 {
        return (want, format!("{want} of {cores} cores"));
    }
    let fit = (budget.saturating_sub(BASE) / PER_THREAD).max(1) as usize;
    let gb = |b: u64| b as f64 / (1u64 << 30) as f64;
    if fit < want {
        (fit, format!("{fit} of {cores} cores: {:.1} GB of memory free, about {:.1} GB per thread", gb(avail), gb(PER_THREAD)))
    } else {
        (want, format!("{want} of {cores} cores ({:.1} GB of memory free)", gb(avail)))
    }
}

// ---------------------------------------------------------- the runner --

/// A converter command with ACECM's environment: the game's content.kspkg,
/// the work folder for what is generated from it, the thread count; hidden
/// and below normal priority so the PC stays usable.
pub(crate) fn command(conv: &Path, args: &[String]) -> Command {
    let mut cmd = Command::new(conv);
    cmd.args(args).stdin(Stdio::null());
    let game = game_content();
    if game.is_file() {
        cmd.env("ACECONV_GAME_CONTENT", &game);
    }
    let gen = generated_dir();
    cmd.env("ACECONV_DONOR_DIR", gen.join("donor"));
    cmd.env("ACECONV_FMOD_DIR", gen.join("fmod_template"));
    let t = threads().0.to_string();
    cmd.env("ACECONV_THREADS", &t);
    cmd.env("ACECONV_SHARE_THREADS", &t);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS (the converter
        // drops itself to idle as well)
        cmd.creation_flags(0x0800_0000 | 0x0000_4000);
    }
    cmd
}

/// The game's content.kspkg (the one every client-archive reader uses,
/// ACECM_CLIENT_KSPKG included).
pub(crate) fn game_content() -> PathBuf {
    crate::viewer::package().map(PathBuf::from).unwrap_or_else(|| Path::new(&crate::detect::find("game_dir", false)).join("content.kspkg"))
}

/// A short converter command's stdout lines (ids, earlier conversions).
pub(crate) fn query(args: &[String]) -> Result<Vec<String>, String> {
    let conv = converter().ok_or("the converter is missing from this build")?;
    let o = command(&conv, args).stdout(Stdio::piped()).stderr(Stdio::piped()).output().map_err(|e| format!("could not run the converter: {e}"))?;
    if !o.status.success() {
        let e = String::from_utf8_lossy(&o.stderr);
        return Err(e.lines().last().unwrap_or("the converter failed").to_string());
    }
    Ok(String::from_utf8_lossy(&o.stdout).lines().map(|l| l.trim_end().to_string()).collect())
}

/// Run a conversion, streaming its output into the job. None when it could
/// not start (the job says why). (exit code, seconds, cancelled)
pub(crate) fn run_converter(conv: &Path, args: &[String], cwd: &Path) -> Option<(i32, f64, bool)> {
    let t0 = Instant::now();
    let (t, why) = threads();
    JOB.lock().unwrap().lines.push(format!("ACECM: converter {} with {t} thread(s) - {why}", if CONVERTER_REV.is_empty() { "(local build)" } else { CONVERTER_REV }));
    let mut cmd = command(conv, args);
    cmd.current_dir(cwd).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let mut j = JOB.lock().unwrap();
            j.state = "error";
            j.result = json!({"ok": false, "error": format!("could not start the converter: {e}")});
            return None;
        }
    };
    procs::invalidate_snapshot();
    JOB.lock().unwrap().pid = child.id();
    let err = child.stderr.take().map(|e| std::thread::spawn(move || {
        for l in BufReader::new(e).lines().map_while(Result::ok) {
            JOB.lock().unwrap().lines.push(format!("! {l}"));
        }
    }));
    if let Some(o) = child.stdout.take() {
        for l in BufReader::new(o).lines().map_while(Result::ok) {
            JOB.lock().unwrap().lines.push(l);
        }
    }
    if let Some(h) = err {
        let _ = h.join();
    }
    let code = child.wait().ok().and_then(|s| s.code()).unwrap_or(-1);
    Some((code, t0.elapsed().as_secs_f64(), JOB.lock().unwrap().cancel))
}

/// Run `aceconv check-car` / `check-track`: the summary {ok, errors,
/// warnings, lines}. The lines also go to the job's output.
pub(crate) fn check(conv: &Path, args: &[String]) -> Value {
    let out = command(conv, args).stdout(Stdio::piped()).stderr(Stdio::piped()).output();
    let (ok, lines) = match out {
        Ok(o) => {
            let mut l: Vec<String> = String::from_utf8_lossy(&o.stdout).lines().map(|l| l.to_string()).collect();
            l.extend(String::from_utf8_lossy(&o.stderr).lines().map(|x| format!("! {x}")));
            (o.status.success(), l)
        }
        Err(e) => (false, vec![format!("could not run the validator: {e}")]),
    };
    // "<what>: N files, R references, E errors, W warnings"
    let count = |word: &str| -> Option<u64> {
        let first = lines.first()?;
        let i = first.find(&format!(" {word}"))?;
        first[..i].rsplit(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
    };
    let errors = count("errors").unwrap_or(if ok { 0 } else { 1 });
    let warnings = count("warnings").unwrap_or(0);
    {
        let mut j = JOB.lock().unwrap();
        j.lines.push(format!("ACECM: checking the result ({})", args.first().map(|s| s.as_str()).unwrap_or("")));
        j.lines.extend(lines.iter().map(|l| format!("  check: {l}")));
    }
    json!({"ok": ok, "errors": errors, "warnings": warnings, "lines": lines})
}

// -------------------------------------------------------------- history --

fn history_path() -> PathBuf {
    crate::paths::data().join("convert_history.json")
}

pub(crate) fn history() -> Map<String, Value> {
    let read = |p: PathBuf| std::fs::read_to_string(p).ok().and_then(|t| serde_json::from_str::<Map<String, Value>>(&t).ok());
    read(history_path())
        // an older development build's history: its ids are of the old
        // scheme, so only the "converted / in the game" marks are used
        .or_else(|| read(crate::paths::data().join("dev_convert_history.json")))
        .unwrap_or_default()
}

pub(crate) fn save_history(h: &Map<String, Value>) {
    let p = history_path();
    let _ = std::fs::create_dir_all(p.parent().unwrap_or(Path::new(".")));
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, serde_json::to_string_pretty(h).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

pub(crate) fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

// ---------------------------------------------------------------- ids --

/// Is `id` a valid converted id of that kind (the converter's own rule:
/// the stock character set; cars ks_ + 13..37 characters, tracks 3..16)?
pub(crate) fn valid_id(kind: &str, id: &str) -> bool {
    let chars = !id.is_empty() && id.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
        && id.starts_with(|c: char| c.is_ascii_lowercase()) && !id.ends_with('_');
    if kind == "cars" {
        chars && id.starts_with("ks_") && (13..=37).contains(&id.len())
    } else {
        chars && (3..=16).contains(&id.len())
    }
}

static IDS: LazyLock<Mutex<std::collections::HashMap<String, String>>> = LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

/// The converter's default id for each AC1 folder (`car-id` / `track-id`,
/// which also steer clear of the stock ids). Asked once per folder per run.
pub(crate) fn default_ids(kind: &str, folders: &[String]) -> std::collections::HashMap<String, String> {
    let key = |f: &str| format!("{kind}|{f}");
    let missing: Vec<String> = {
        let c = IDS.lock().unwrap();
        folders.iter().filter(|f| !c.contains_key(&key(f))).cloned().collect()
    };
    // a long command line is split (Windows allows ~32k characters)
    for chunk in missing.chunks(200) {
        let mut args = vec![if kind == "cars" { "car-id" } else { "track-id" }.to_string()];
        args.extend(chunk.iter().cloned());
        if let Ok(lines) = query(&args) {
            let mut c = IDS.lock().unwrap();
            for l in lines {
                if let Some((f, id)) = l.split_once('\t') {
                    if valid_id(kind, id.trim()) {
                        c.insert(key(f), id.trim().to_string());
                    }
                }
            }
        }
    }
    let c = IDS.lock().unwrap();
    folders.iter().filter_map(|f| c.get(&key(f)).map(|id| (f.clone(), id.clone()))).collect()
}

/// Earlier conversions of the same AC1 source in a mods folder
/// (`aceconv legacy-id`): [{id, strength, path, why}], strongest first.
/// strength 2 = it names the same source folder, 1 = the same display name.
pub(crate) fn earlier(mods_dir: &Path, kind: &str, src: &Path) -> Vec<Value> {
    if !mods_dir.is_dir() {
        return vec![];
    }
    let lines = query(&["legacy-id".into(), mods_dir.to_string_lossy().into_owned(), kind.into(), src.to_string_lossy().into_owned()]).unwrap_or_default();
    lines.iter().filter_map(|l| {
        let p: Vec<&str> = l.split('\t').collect();
        if p.len() < 4 || p[0].is_empty() || !p[0].bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
            return None;
        }
        Some(json!({"id": p[0], "strength": p[1].parse::<u8>().unwrap_or(1), "path": p[2], "why": p[3]}))
    }).collect()
}

/// GET /api/convert/earlier?kind=cars|tracks&folder=<ac1 folder>
pub fn earlier_api(kind: &str, folder: &str) -> Value {
    if !valid_folder(folder) || (kind != "cars" && kind != "tracks") {
        return json!({"ok": false, "error": "bad request"});
    }
    let st = settings();
    let src = Path::new(&s(&st, "ac1_root")).join("content").join(kind).join(folder);
    json!({"ok": true, "earlier": earlier(Path::new(&mods::client_mods_dir()), kind, &src)})
}

/// Forget the game's saved cameras and mirrors of these car ids
/// (`aceconv reset-car-settings`), after copying the two settings files to
/// <data>\convert_backups\<time>\. The game re-applies a saved entry over a
/// reinstalled car, so a replaced conversion would keep its old cameras.
pub(crate) fn reset_car_settings(ids: &[String]) -> Value {
    let Some(saved) = std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join("Saved Games").join("ACE")).filter(|d| d.is_dir()) else {
        return json!({"ok": true, "skipped": "no Saved Games\\ACE folder"});
    };
    if procs::game_running() {
        return json!({"ok": false, "skipped": "the game is running: its saved cameras for this car were left alone (close the game and convert again to reset them)"});
    }
    let backup = crate::paths::data().join("convert_backups").join(format!("{}_settings", now()));
    let mut copied = Vec::new();
    for f in ["CarCameras.carcamerausersettings", "CarMirrors.carmirrorsusersettings"] {
        let p = saved.join(f);
        if p.is_file() {
            if let Err(e) = std::fs::create_dir_all(&backup).and_then(|_| std::fs::copy(&p, backup.join(f))) {
                return json!({"ok": false, "error": format!("could not back up {}: {e} - nothing was reset", p.display())});
            }
            copied.push(f);
        }
    }
    if copied.is_empty() {
        return json!({"ok": true, "removed": []});
    }
    let mut removed = Vec::new();
    let mut errors = Vec::new();
    for id in ids {
        match query(&["reset-car-settings".into(), id.clone()]) {
            Ok(lines) => removed.extend(lines.into_iter().filter(|l| l.starts_with("removed "))),
            Err(e) => errors.push(format!("{id}: {e}")),
        }
    }
    json!({"ok": errors.is_empty(), "removed": removed, "backup": backup.to_string_lossy(), "error": errors.join("; ")})
}

// ------------------------------------------------------------- info --

/// GET /api/convert: settings, whether each part is there, the history.
pub fn info() -> Value {
    let st = settings();
    let game = crate::detect::find("game_dir", false);
    let conv = converter();
    let (t, why) = threads();
    let (total, avail) = memory();
    let gen = generated_dir();
    json!({
        "ok": true,
        "settings": st,
        "present": {
            "ac1_root": Path::new(&s(&st, "ac1_root")).join("content").join("cars").is_dir()
                || Path::new(&s(&st, "ac1_root")).join("content").join("tracks").is_dir(),
            "aceconv": conv.is_some(),
            "out_dir": Path::new(&s(&st, "out_dir")).is_dir(),
            "game_content": game_content().is_file(),
            // built from the game on the first conversion (a minute or two)
            "donor": gen.join("donor").join("stamp.txt").is_file(),
        },
        "converter": conv.map(|p| p.to_string_lossy().into_owned()),
        "converter_rev": CONVERTER_REV,
        "threads": t, "threads_why": why,
        "cores": std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0),
        "memory": {"total": total, "available": avail},
        "game_dir": game,
        "game_running": procs::game_running(),
        "client_mods": mods::client_mods_dir(),
        "server_mods": mods::mods_dir(),
        "history": history(),
    })
}

/// POST /api/convert/settings {ac1_root, out_dir, threads, memory_gb}
pub fn save_settings(body: &Value) -> Value {
    let mut cur = match config::load().get("convert") { Some(Value::Object(m)) => m.clone(), _ => Map::new() };
    for k in ["ac1_root", "out_dir"] {
        if let Some(v) = body.get(k).and_then(|v| v.as_str()) {
            cur.insert(k.into(), json!(v.trim()));
        }
    }
    for k in ["threads", "memory_gb"] {
        if let Some(v) = body.get(k) {
            let x = v.as_u64().or_else(|| v.as_str().and_then(|t| t.trim().parse().ok())).unwrap_or(0).min(256);
            cur.insert(k.into(), json!(x));
        }
    }
    match config::save(&json!({"convert": cur})) {
        Ok(_) => info(),
        Err(e) => json!({"ok": false, "error": e.to_string()}),
    }
}

// ------------------------------------------------------------ the list --

/// What the car list shows from AC1's ui_car.json. The file is often not
/// strict JSON (raw tabs/newlines inside strings), so it is cleaned first and
/// the fields are pulled out by hand as a last resort.
fn ui_car(dir: &Path) -> Map<String, Value> {
    let mut m = Map::new();
    let Ok(b) = std::fs::read(dir.join("ui").join("ui_car.json")) else { return m };
    let t: String = String::from_utf8_lossy(&b).trim_start_matches('\u{feff}').chars().map(|c| if c == '\t' || c == '\r' || c == '\n' { ' ' } else { c }).collect();
    let v: Value = serde_json::from_str(&t).unwrap_or(Value::Null);
    let grab = |key: &str| -> String {
        if let Some(x) = v.get(key) {
            return match x { Value::String(s) => s.clone(), Value::Number(n) => n.to_string(), _ => String::new() };
        }
        let pat = format!("\"{key}\"");
        t.find(&pat).and_then(|i| {
            let rest = t[i + pat.len()..].trim_start().strip_prefix(':')?.trim_start();
            if let Some(r) = rest.strip_prefix('"') {
                r.find('"').map(|e| r[..e].to_string())
            } else {
                Some(rest.chars().take_while(|c| c.is_ascii_digit()).collect())
            }
        }).unwrap_or_default()
    };
    for k in ["name", "brand", "class", "year", "country"] {
        let s = grab(k).trim().to_string();
        if !s.is_empty() {
            m.insert(k.into(), json!(s));
        }
    }
    let spec = |k: &str| v.get("specs").and_then(|s| s.get(k)).and_then(|x| x.as_str().map(String::from).or_else(|| x.as_f64().map(|f| f.to_string()))).unwrap_or_default();
    for (k, out) in [("bhp", "power"), ("weight", "weight")] {
        let s = spec(k).trim().to_string();
        if !s.is_empty() && s != "-" && s != "--" {
            m.insert(out.into(), json!(s));
        }
    }
    m
}

/// The picture for a car: its first skin's preview.jpg (what AC1's own car
/// picker shows), else the badge.
pub fn preview(folder: &str) -> Option<PathBuf> {
    if !valid_folder(folder) {
        return None;
    }
    let car = Path::new(&s(&settings(), "ac1_root")).join("content").join("cars").join(folder);
    let mut skins: Vec<PathBuf> = std::fs::read_dir(car.join("skins")).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    skins.sort();
    skins.into_iter().map(|d| d.join("preview.jpg")).find(|p| p.is_file())
        .or_else(|| Some(car.join("ui").join("badge.png")).filter(|p| p.is_file()))
}

/// CSP's kn5 protection: the author locked the model. Those files are not
/// converted. Cached by file size + time, so the list stays fast.
const PROTECT_MARK: &[u8] = b"__AC_SHADERS_PATCH_KN5ENC";
static PROTECTED: LazyLock<Mutex<std::collections::HashMap<PathBuf, (u64, u64, bool)>>> = LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

pub(crate) fn kn5_protected(p: &Path) -> bool {
    let Ok(md) = std::fs::metadata(p) else { return false };
    let stamp = (md.len(), md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0));
    if let Some(&(l, t, v)) = PROTECTED.lock().unwrap().get(p) {
        if (l, t) == stamp {
            return v;
        }
    }
    // ⚠ the marker is at the START of some files and at the END of others
    use std::io::{Read, Seek, SeekFrom};
    let has = |b: &[u8]| b.windows(PROTECT_MARK.len()).any(|w| w == PROTECT_MARK);
    let v = std::fs::File::open(p).map(|mut f| {
        let mut buf = vec![0u8; 64 * 1024];
        let n = f.read(&mut buf).unwrap_or(0);
        if has(&buf[..n]) {
            return true;
        }
        if md.len() > buf.len() as u64 && f.seek(SeekFrom::End(-(buf.len() as i64))).is_ok() {
            let n = f.read(&mut buf).unwrap_or(0);
            return has(&buf[..n]);
        }
        false
    }).unwrap_or(false);
    PROTECTED.lock().unwrap().insert(p.to_path_buf(), (stamp.0, stamp.1, v));
    v
}

pub(crate) fn car_kn5s(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir).map(|r| r.flatten().map(|f| f.path())
        .filter(|p| p.is_file() && p.extension().map(|x| x.eq_ignore_ascii_case("kn5")).unwrap_or(false)).collect()).unwrap_or_default()
}

/// Per-folder list entries, rebuilt only when the folder's time changes: the
/// lists are asked for on every visit and after every conversion, and
/// reading hundreds of ui JSONs and kn5 headers each time made the page slow.
pub(crate) type ListCache = LazyLock<Mutex<std::collections::HashMap<PathBuf, (u64, Option<Map<String, Value>>)>>>;
pub(crate) static CAR_CACHE: ListCache = LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

fn mtime(p: &Path) -> Option<u64> {
    std::fs::metadata(p).ok()?.modified().ok()?.duration_since(UNIX_EPOCH).ok().map(|d| d.as_millis() as u64)
}

pub(crate) fn cached_entry(dir: &Path, cache: &ListCache, build: impl FnOnce(&Path) -> Option<Map<String, Value>>) -> Option<Map<String, Value>> {
    if !dir.is_dir() {
        return None;
    }
    let stamp = mtime(dir).unwrap_or(0);
    if let Some((t, v)) = cache.lock().unwrap().get(dir) {
        if *t == stamp {
            return v.clone();
        }
    }
    let v = build(dir);
    cache.lock().unwrap().insert(dir.to_path_buf(), (stamp, v.clone()));
    v
}

/// A small cached JPEG of a preview picture (the grid shows hundreds; the
/// originals are 100-300 KB each and decoding them all made the page lag).
pub fn thumb(src: &Path) -> Option<PathBuf> {
    let md = std::fs::metadata(src).ok()?;
    let stamp = md.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_millis();
    let mut h: u64 = 0xcbf29ce484222325;
    for b in src.to_string_lossy().bytes().chain(format!("|{}|{stamp}", md.len()).bytes()) {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    let dir = crate::paths::data().join("convert_thumbs");
    let out = dir.join(format!("{h:016x}.jpg"));
    if out.is_file() {
        return Some(out);
    }
    let img = image::open(src).ok()?;
    let t = img.thumbnail(384, 216).to_rgb8();
    let _ = std::fs::create_dir_all(&dir);
    let tmp = out.with_extension("tmp");
    let mut f = std::io::BufWriter::new(std::fs::File::create(&tmp).ok()?);
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut f, 82).encode_image(&t).ok()?;
    drop(f);
    std::fs::rename(&tmp, &out).ok()?;
    Some(out)
}

/// A history entry's id, when it is one of the current scheme.
pub(crate) fn history_id(h: &Map<String, Value>, key: &str, kind: &str) -> Option<String> {
    let e = h.get(key)?;
    if e.get("scheme").and_then(|v| v.as_u64()) != Some(2) {
        return None;
    }
    e.get("id").and_then(|v| v.as_str()).filter(|id| valid_id(kind, id)).map(String::from)
}

/// GET /api/convert/cars: AC1 cars that can be converted.
pub fn cars() -> Value {
    let st = settings();
    let root = Path::new(&s(&st, "ac1_root")).join("content").join("cars");
    let Ok(rd) = std::fs::read_dir(&root) else {
        return json!({"ok": false, "error": format!("No Assetto Corsa cars folder at {}", root.display()), "cars": []});
    };
    let hist = history();
    let mut out = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        let Some(base) = cached_entry(&p, &CAR_CACHE, |p| {
            let has_data = p.join("data.acd").is_file() || p.join("data").is_dir();
            let kn5s = car_kn5s(p);
            if !has_data || kn5s.is_empty() {
                return None;
            }
            let mut c = ui_car(p);
            c.insert("protected".into(), json!(kn5s.iter().any(|k| kn5_protected(k))));
            Some(c)
        }) else { continue };
        let folder = e.file_name().to_string_lossy().into_owned();
        let mut c = base;
        c.insert("folder".into(), json!(folder));
        c.insert("last".into(), hist.get(&folder).cloned().unwrap_or(Value::Null));
        out.push(c);
    }
    let folders: Vec<String> = out.iter().filter_map(|c| c.get("folder").and_then(|v| v.as_str()).map(String::from)).collect();
    let ids = default_ids("cars", &folders);
    let mut list: Vec<Value> = out.into_iter().map(|mut c| {
        let f = c["folder"].as_str().unwrap_or("").to_string();
        let id = history_id(&hist, &f, "cars").or_else(|| ids.get(&f).cloned()).unwrap_or_default();
        c.insert("id".into(), json!(id));
        Value::Object(c)
    }).collect();
    let key = |v: &Value| format!("{} {}", v["brand"].as_str().unwrap_or(""), v["name"].as_str().or(v["folder"].as_str()).unwrap_or("")).to_lowercase();
    list.sort_by_key(key);
    json!({"ok": true, "root": root.to_string_lossy(), "cars": list})
}

/// GET /api/convert/status
pub fn status() -> Value {
    let j = JOB.lock().unwrap();
    let secs = match (j.state, j.started) { ("running", Some(t)) => t.elapsed().as_secs_f64(), _ => j.secs };
    json!({"ok": true, "state": j.state, "car": j.car, "id": j.id, "secs": (secs * 10.0).round() / 10.0, "lines": j.lines, "result": j.result})
}

pub(crate) fn valid_folder(f: &str) -> bool {
    !f.is_empty() && !f.contains(['/', '\\', ':', '\0']) && f != "." && f != ".."
}

/// Copy beside then rename: a half-written kspkg in the mods folder would
/// crash the game.
fn place(src: &Path, dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let name = src.file_name().ok_or("bad file name")?;
    let dst = dir.join(name);
    let tmp = dir.join(format!("{}.acecm.tmp", name.to_string_lossy()));
    std::fs::copy(src, &tmp).map_err(|e| format!("copy to {}: {e}", dir.display()))?;
    if dst.exists() {
        if let Err(e) = std::fs::remove_file(&dst) {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("{} is in use ({e}) - close the game first", dst.display()));
        }
    }
    std::fs::rename(&tmp, &dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    Ok(dst)
}

/// Take an earlier conversion `<id>.kspkg` + `<id>.json` out of a mods
/// folder into the work folder's _replaced\<time>\ (kept, not deleted).
fn retire_car(dir: &Path, id: &str, stamp: u64) -> Result<Vec<String>, String> {
    let mut moved = Vec::new();
    let keep = out_dir().join("_replaced").join(stamp.to_string());
    for ext in ["kspkg", "json"] {
        let p = dir.join(format!("{id}.{ext}"));
        if !p.is_file() {
            continue;
        }
        std::fs::create_dir_all(&keep).map_err(|e| format!("{}: {e}", keep.display()))?;
        let to = keep.join(format!("{id}.{ext}"));
        if std::fs::rename(&p, &to).is_err() {
            std::fs::copy(&p, &to).map_err(|e| format!("keep {}: {e}", p.display()))?;
            std::fs::remove_file(&p).map_err(|e| format!("{} is in use ({e}) - close the game first", p.display()))?;
        }
        moved.push(p.to_string_lossy().into_owned());
    }
    Ok(moved)
}

/// POST /api/convert/start {car, id, install_client, install_server, replace: [old ids]}
pub fn start(body: &Value) -> Value {
    let car = body.get("car").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let id = body.get("id").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let client = body.get("install_client").and_then(|v| v.as_bool()).unwrap_or(true);
    let server = body.get("install_server").and_then(|v| v.as_bool()).unwrap_or(false);
    let replace: Vec<String> = body.get("replace").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str()).map(|x| x.trim().to_string())
        .filter(|x| *x != id && !x.is_empty() && x.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')).collect()).unwrap_or_default();
    if !valid_folder(&car) {
        return json!({"ok": false, "error": "pick a car"});
    }
    if !valid_id("cars", &id) {
        return json!({"ok": false, "error": "a car id is ks_ followed by lower-case letters, digits and _, 13 to 37 characters in all"});
    }
    let st = settings();
    let ac1 = Path::new(&s(&st, "ac1_root")).join("content").join("cars").join(&car);
    let Some(conv) = converter() else {
        return json!({"ok": false, "error": "the converter is missing from this build"});
    };
    let out = PathBuf::from(s(&st, "out_dir"));
    if !ac1.is_dir() {
        return json!({"ok": false, "error": format!("AC1 car not found: {}", ac1.display())});
    }
    if !game_content().is_file() {
        return json!({"ok": false, "error": "Assetto Corsa EVO's content.kspkg was not found - set the game folder in Settings"});
    }
    if car_kn5s(&ac1).iter().any(|k| kn5_protected(k)) {
        return json!({"ok": false, "error": "This car is protected by its author (CSP encryption), so it can't be converted. An unprotected copy from the author will work."});
    }
    if let Err(e) = std::fs::create_dir_all(&out) {
        return json!({"ok": false, "error": format!("output folder {}: {e}", out.display())});
    }
    {
        let mut j = JOB.lock().unwrap();
        if j.state == "running" {
            return json!({"ok": false, "error": format!("already converting {}", j.car)});
        }
        *j = Job::running(car.clone(), id.clone());
    }
    let args = vec!["car".to_string(), ac1.to_string_lossy().into_owned(), out.to_string_lossy().into_owned(),
        "--id".into(), id.clone(), "--content".into(), game_content().to_string_lossy().into_owned()];
    logs::info(format!("convert: {car} -> {id}"));
    std::thread::spawn(move || {
        let Some((code, secs, cancelled)) = run_converter(&conv, &args, &out) else { return };
        let pkg = out.join(format!("{id}.kspkg"));
        let manifest = out.join(format!("{id}.json"));
        let mut res = Map::new();
        let mut ok = code == 0 && pkg.is_file() && !cancelled;
        // never install what the validator rejects
        if ok {
            let c = check(&conv, &["check-car".into(), pkg.to_string_lossy().into_owned(), game_content().to_string_lossy().into_owned()]);
            if c["ok"] != true {
                ok = false;
                res.insert("error".into(), json!("the converted car failed the pre-install check - not installed (see Details)"));
            }
            res.insert("check".into(), c);
        }
        res.insert("code".into(), json!(code));
        res.insert("secs".into(), json!((secs * 10.0).round() / 10.0));
        if ok {
            res.insert("kspkg".into(), json!(pkg.to_string_lossy()));
            res.insert("size".into(), json!(std::fs::metadata(&pkg).map(|m| m.len()).unwrap_or(0)));
            let mut placed = Vec::new();
            let mut warn = Vec::new();
            let mut retired = Vec::new();
            let stamp = now();
            let targets: Vec<(&str, String)> = [(client, "game", mods::client_mods_dir()), (server, "server", mods::mods_dir())]
                .into_iter().filter(|t| t.0).map(|t| (t.1, t.2)).collect();
            for (what, dir) in &targets {
                let dir = PathBuf::from(dir);
                for f in [&pkg, &manifest] {
                    if !f.is_file() {
                        continue;
                    }
                    match place(f, &dir) {
                        Ok(p) => placed.push(json!({"to": what, "path": p.to_string_lossy()})),
                        Err(e) => warn.push(format!("{what}: {e}")),
                    }
                }
                // the earlier conversions this one replaces
                for old in &replace {
                    match retire_car(&dir, old, stamp) {
                        Ok(m) => retired.extend(m),
                        Err(e) => warn.push(format!("{what}: {e}")),
                    }
                }
            }
            if client && !placed.is_empty() {
                // stale saved cameras / mirrors would override the new car's
                let mut ids = vec![id.clone()];
                ids.extend(replace.iter().cloned());
                let r = reset_car_settings(&ids);
                if let Some(w) = r.get("skipped").and_then(|v| v.as_str()) {
                    if r["ok"] != true {
                        warn.push(w.to_string());
                    }
                }
                if let Some(e) = r.get("error").and_then(|v| v.as_str()).filter(|e| !e.is_empty()) {
                    warn.push(format!("saved cameras: {e}"));
                }
                res.insert("settings_reset".into(), r);
            }
            res.insert("installed".into(), json!(placed));
            res.insert("replaced".into(), json!(retired));
            if !warn.is_empty() {
                res.insert("warnings".into(), json!(warn));
            }
        } else {
            res.entry("error").or_insert(json!(if cancelled { "cancelled".to_string() } else { format!("converter exited with code {code}") }));
        }
        res.insert("ok".into(), json!(ok));
        let result = Value::Object(res);
        logs::info(format!("convert: {} -> {id}: ok={ok} {:.0} s", args[1], secs));
        let car = {
            let mut j = JOB.lock().unwrap();
            j.state = if ok { "done" } else if cancelled { "cancelled" } else { "error" };
            j.secs = secs;
            j.pid = 0;
            j.result = result.clone();
            j.car.clone()
        };
        if cancelled {
            return;
        }
        let mut h = history();
        h.insert(car, json!({"id": id, "scheme": 2, "at": now(), "ok": ok, "secs": (secs * 10.0).round() / 10.0,
            "installed": result.get("installed").cloned().unwrap_or(json!([])), "check": result.get("check").map(|c| json!({"errors": c["errors"], "warnings": c["warnings"]}))}));
        save_history(&h);
    });
    json!({"ok": true, "started": true})
}

/// POST /api/convert/cancel
pub fn cancel() -> Value {
    let pid = {
        let mut j = JOB.lock().unwrap();
        if j.state != "running" {
            return json!({"ok": false, "error": "nothing is running"});
        }
        j.cancel = true;
        j.pid
    };
    if pid != 0 {
        procs::kill(pid);
    }
    json!({"ok": true})
}

/// POST /api/convert/uninstall {id}: take a converted car out of the game's
/// and the server's mods folders (the converter output is kept).
pub fn uninstall(body: &Value) -> Value {
    let id = body.get("id").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if id.is_empty() || !id.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_') {
        return json!({"ok": false, "error": "bad id"});
    }
    let mut removed = Vec::new();
    let mut errors = Vec::new();
    for dir in [mods::client_mods_dir(), mods::mods_dir()] {
        for ext in ["kspkg", "json"] {
            let p = Path::new(&dir).join(format!("{id}.{ext}"));
            if p.is_file() {
                match std::fs::remove_file(&p) {
                    Ok(_) => removed.push(p.to_string_lossy().into_owned()),
                    Err(e) => errors.push(format!("{}: {e}", p.display())),
                }
            }
        }
    }
    let mut h = history();
    for v in h.values_mut() {
        if v.get("id").and_then(|x| x.as_str()) == Some(id.as_str()) {
            v["installed"] = json!([]);
        }
    }
    save_history(&h);
    json!({"ok": errors.is_empty(), "removed": removed, "error": errors.join("; ")})
}
