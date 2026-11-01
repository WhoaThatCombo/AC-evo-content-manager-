//! The evoview tab: set up and launch evoview's track-drive mode
//! (`evoview <content.kspkg> --track <id> --drive-car <car>`), plus the
//! files evoview keeps its controls in - %APPDATA%\evoview\wheel.cfg (what
//! `--wheel-setup` writes) and ffb.cfg (the Numpad +/- strength).
//!
//! ⚠ evoview gains features on branches that land at different times. So
//! nothing here assumes which build is installed: `caps()` reads the exe once
//! per stamp and reports which EVOVIEW_* variables, flags and wheel roles it
//! contains (each is a literal string in the binary), and the page shows an
//! option only when its token is there. A new knob is one line in the page's
//! table (web/js/14d-evoview.js) and nothing here.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Map, Value};

use crate::{config, paths, viewer};

fn stamp(p: &Path) -> String {
    std::fs::metadata(p).map(|m| {
        let t = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
        format!("{}:{t}", m.len())
    }).unwrap_or_default()
}

// ------------------------------------------------------------- the exe --

/// What one evoview build knows, read out of its own bytes.
#[derive(Clone, Default)]
struct Exe {
    stamp: String,
    /// every EVOVIEW_* name in the binary
    env: Vec<String>,
    /// render.rs KEYS: the key list it prints at start
    keys: String,
    /// asked-for tokens (flags, wheel roles) -> present
    seen: HashMap<String, bool>,
}

static EXE: Mutex<Option<Exe>> = Mutex::new(None);

fn scan(exe: &Path) -> Exe {
    let st = stamp(exe);
    if let Some(e) = EXE.lock().unwrap().as_ref().filter(|e| e.stamp == st) {
        return e.clone();
    }
    let bytes = std::fs::read(exe).unwrap_or_default();
    let mut env: Vec<String> = vec![];
    // ⚠ string literals sit back to back with no separator
    // ("EVOVIEW_DIRTEVOVIEW_RAIN..."): a name also ends where the next begins
    let at: Vec<usize> = memchr::memmem::find_iter(&bytes, b"EVOVIEW_").collect();
    for (n, &i) in at.iter().enumerate() {
        let stop = at.get(n + 1).copied().unwrap_or(bytes.len()).min(i + 48);
        let name: String = bytes[i..stop].iter()
            .take_while(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || **b == b'_')
            .map(|b| *b as char).collect();
        if name.len() > 8 && !name.ends_with('_') {
            env.push(name);
        }
    }
    env.sort();
    env.dedup();
    // "KEYS\n  view ..." up to "Esc quit"; the help text is one literal
    let keys = memchr::memmem::find(&bytes, b"KEYS\n  view").and_then(|a| {
        let rest = &bytes[a..(a + 8192).min(bytes.len())];
        let end = memchr::memmem::find(rest, b"Esc quit")?;
        Some(String::from_utf8_lossy(&rest[..end + 8]).replace("\r\n", "\n"))
    }).unwrap_or_default();
    let e = Exe { stamp: st, env, keys, seen: HashMap::new() };
    *EXE.lock().unwrap() = Some(e.clone());
    e
}

/// Which of these literal strings does the exe contain? (cached per stamp)
fn has_tokens(exe: &Path, tokens: &[String]) -> Map<String, Value> {
    let mut e = scan(exe);
    let want: Vec<&String> = tokens.iter().filter(|t| !t.is_empty() && !e.seen.contains_key(*t)).collect();
    if !want.is_empty() {
        let bytes = std::fs::read(exe).unwrap_or_default();
        for t in want {
            e.seen.insert(t.clone(), memchr::memmem::find(&bytes, t.as_bytes()).is_some());
        }
        *EXE.lock().unwrap() = Some(e.clone());
    }
    tokens.iter().map(|t| (t.clone(), json!(e.seen.get(t).copied().unwrap_or(false)))).collect()
}

fn exe_has(exe: &Path, token: &str) -> bool {
    has_tokens(exe, &[token.to_string()]).get(token) == Some(&json!(true))
}

/// The upscaler DLLs evoview loads from beside itself (not shipped in git).
fn dlls(exe: &Path) -> Value {
    let d = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    let all = |names: &[&str]| names.iter().all(|n| d.join(n).is_file());
    json!({"dlss": all(&["evoview_ngx.dll", "nvngx_dlss.dll"]),
           "fsr": all(&["amd_fidelityfx_loader_vk.dll", "amd_fidelityfx_upscaler_vk.dll"])})
}

/// GET /api/evoview/caps?t=a,b: the tokens asked for, the env list, the keys.
pub fn caps(tokens: &str) -> Value {
    let Some(exe) = viewer::viewer_exe() else { return json!({"ok": false, "error": "evoview.exe not found"}) };
    let exe = PathBuf::from(exe);
    let list: Vec<String> = tokens.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).take(200).collect();
    let e = scan(&exe);
    json!({"ok": true, "exe": exe, "env": e.env, "keys": e.keys, "has": has_tokens(&exe, &list), "dlls": dlls(&exe)})
}

// ----------------------------------------------------------- catalogue --

/// Racing-line layouts per track folder, from the archive's
/// `layout_<id>.ideal_line.aisplinedata` - the names evoview's --layout
/// matches (trackdrive::racing_line_layout). Cached on the package stamp.
fn layouts(pkg: &Path) -> BTreeMap<String, Vec<String>> {
    static C: Mutex<Option<(String, BTreeMap<String, Vec<String>>)>> = Mutex::new(None);
    let st = stamp(pkg);
    if let Some((k, v)) = C.lock().unwrap().as_ref() {
        if *k == st {
            return v.clone();
        }
    }
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for e in crate::kspkg::entries(pkg).unwrap_or_default() {
        let low = e.path.to_lowercase();
        let Some(rest) = low.strip_prefix("content\\tracks\\") else { continue };
        let Some(stem) = low.rsplit('\\').next().and_then(|f| f.strip_suffix(".ideal_line.aisplinedata")) else { continue };
        if low.contains("pit") {
            continue;
        }
        let folder = rest.split('\\').next().unwrap_or("").to_string();
        let id = stem.rsplit_once("layout_").map(|(_, l)| l.to_string()).unwrap_or_default();
        if !folder.is_empty() && !id.is_empty() {
            let v = out.entry(folder).or_default();
            if !v.contains(&id) {
                v.push(id);
            }
        }
    }
    // evoview's own default first: a "gp" name, then the shortest
    for v in out.values_mut() {
        v.sort_by_key(|k| (!k.contains("gp"), k.len(), k.clone()));
    }
    *C.lock().unwrap() = Some((st, out.clone()));
    out
}

fn squash(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_lowercase()
}

fn pretty_layout(id: &str) -> String {
    id.split('_').filter(|w| !w.is_empty()).map(|w| {
        if w.len() <= 3 { w.to_uppercase() } else {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        }
    }).collect::<Vec<_>>().join(" ")
}

/// Every track the client can load (tracks.table: name -> folder), with its
/// racing-line layouts and, where the events list knows them, their lengths.
fn tracks(pkg: &Path) -> Vec<Value> {
    let map = crate::trackimport::track_map(false);
    let lays = layouts(pkg);
    let ev = crate::content::tracks();
    let rows = ev.get("tracks").and_then(|t| t.as_array()).cloned().unwrap_or_default();
    let mut out: Vec<Value> = map.iter().filter_map(|(name, folder)| {
        let folder = folder.as_str()?.to_string();
        let mine: Vec<&Value> = rows.iter().filter(|r| {
            r.get("track").and_then(|v| v.as_str()).is_some_and(|t| t.eq_ignore_ascii_case(name))
                || r.get("folder").and_then(|v| v.as_str()) == Some(folder.as_str())
        }).collect();
        let ids = lays.get(&folder.to_lowercase()).cloned().unwrap_or_default();
        let lone = ids.len() == 1 && mine.len() == 1;
        let layouts: Vec<Value> = ids.iter().map(|id| {
            // "Layout 1A-V2" names layout_1a_v2; a lone layout is the lone event
            let same = |l: &str| { let l = squash(l); l.strip_prefix("layout").unwrap_or(&l) == squash(id) };
            let ev = mine.iter().find(|r| lone || r.get("layout").and_then(|v| v.as_str()).is_some_and(same));
            let label = ev.filter(|_| !lone).and_then(|r| r.get("layout")).and_then(|v| v.as_str()).map(String::from).unwrap_or_else(|| pretty_layout(id));
            json!({"id": id, "label": label, "length_m": ev.and_then(|r| r.get("length_m")).cloned().unwrap_or(Value::Null)})
        }).collect();
        let length = layouts.first().and_then(|l| l.get("length_m")).cloned()
            .or_else(|| mine.first().and_then(|r| r.get("length_m")).cloned()).unwrap_or(Value::Null);
        Some(json!({"folder": folder, "name": name, "layouts": layouts, "length_m": length,
                    "mod": mine.iter().any(|r| r.get("mod") == Some(&json!(true)))}))
    }).collect();
    out.sort_by_key(|t| t["name"].as_str().unwrap_or("").to_lowercase());
    out
}

/// GET /api/evoview/catalog: everything the Drive page needs in one answer.
pub fn catalog() -> Value {
    let exe = viewer::viewer_exe();
    let pkg = viewer::package();
    let cfg = config::load();
    let profile = cfg.get("evoview").cloned().unwrap_or(json!({}));
    let Some(pkg) = pkg else {
        return json!({"ok": false, "error": "game install not found - set the game folder in Settings", "profile": profile});
    };
    let cars = viewer::index(false).get("cars").and_then(|c| c.as_array()).cloned().unwrap_or_default();
    // the game's own names and a preset per model, for the name and the
    // picture (renders are cached per trim: id + preset, as Play asks)
    let presets = crate::content::cars();
    let mut first: HashMap<String, (String, String)> = HashMap::new();
    for p in presets.get("cars").and_then(|c| c.as_array()).into_iter().flatten() {
        let (Some(m), Some(id)) = (p.get("model").and_then(|v| v.as_str()), p.get("id").and_then(|v| v.as_str())) else { continue };
        let label = p.get("base_label").or(p.get("label")).and_then(|v| v.as_str()).unwrap_or("").to_string();
        first.entry(m.to_string()).or_insert((id.to_string(), label));
    }
    let cars: Vec<Value> = cars.iter().map(|c| {
        let id = c["id"].as_str().unwrap_or("");
        let (preset, label) = first.get(id).cloned().unwrap_or_default();
        let label = if label.is_empty() { c["label"].clone() } else { json!(label) };
        json!({"id": id, "label": label, "mod": c["mod"], "preset": preset})
    }).collect();
    let mut v = json!({"ok": true, "exe": exe, "package": pkg, "tracks": tracks(Path::new(&pkg)),
                       "cars": cars, "profile": profile, "state": state()});
    if let Some(e) = exe.as_ref().map(PathBuf::from) {
        let x = scan(&e);
        v["env"] = json!(x.env);
        v["keys"] = json!(x.keys);
        v["dlls"] = dlls(&e);
        v["setups_supported"] = json!(exe_has(&e, "--list-setups"));
    }
    v
}

/// POST /api/evoview/profile: remember the page's choices (config "evoview").
pub fn save_profile(p: &Value) -> Value {
    if !p.is_object() {
        return json!({"ok": false, "error": "profile must be an object"});
    }
    match config::save(&json!({"evoview": p})) {
        Ok(_) => json!({"ok": true}),
        Err(e) => json!({"ok": false, "error": e.to_string()}),
    }
}

// -------------------------------------------------------------- setups --

/// The player's saved setups for a car. A build with --list-setups is asked
/// (it knows which car each was saved for); an older one gets the folder,
/// Saved Games\ACE\Car Setups\<car name>\<track>\<name>.carsetup, read here
/// with the car matched by its folder's screen name.
pub fn setups(car: &str, label: &str) -> Value {
    let exe = viewer::viewer_exe().map(PathBuf::from);
    if let Some(exe) = exe.as_ref().filter(|e| exe_has(e, "--list-setups")) {
        let mut c = std::process::Command::new(exe);
        c.arg("--list-setups");
        if !car.is_empty() {
            c.arg(car);
        }
        c.current_dir(paths::data()).stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
        crate::procs::hide(&mut c);
        if let Ok(o) = c.output() {
            let list: Vec<Value> = String::from_utf8_lossy(&o.stdout).lines()
                .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
                .filter_map(|l| {
                    let f: Vec<&str> = l.split('\t').collect();
                    (f.len() >= 4).then(|| json!({"path": f[0], "folder": f[1], "track": f[2], "name": f[3],
                                                 "car": f.get(4).copied().unwrap_or(""), "fits": true}))
                }).collect();
            return json!({"ok": true, "source": "evoview", "setups": list});
        }
    }
    let Some(home) = std::env::var_os("USERPROFILE") else { return json!({"ok": true, "source": "folder", "setups": []}) };
    let root = Path::new(&home).join("Saved Games").join("ACE").join("Car Setups");
    let want = squash(label);
    let mut list = vec![];
    let mut stack = vec![root.clone()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("carsetup")) {
                continue;
            }
            let rel: Vec<String> = p.strip_prefix(&root).map(|r| r.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect()).unwrap_or_default();
            let folder = rel.first().cloned().unwrap_or_default();
            let track = if rel.len() >= 3 { rel[rel.len() - 2].clone() } else { String::new() };
            let name = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            list.push(json!({"path": p, "folder": folder, "track": track, "name": name, "car": "",
                             "fits": !want.is_empty() && squash(&folder) == want}));
        }
    }
    json!({"ok": true, "source": "folder", "setups": list,
           "note": "this evoview cannot load setups yet (no --setup); listed for when it can"})
}

// -------------------------------------------------------------- launch --

struct Run {
    child: std::process::Child,
    stdin: Option<std::process::ChildStdin>,
    started: std::time::Instant,
    what: String,
}
static RUN: Mutex<Option<Run>> = Mutex::new(None);
static LAST: Mutex<Option<Value>> = Mutex::new(None);

fn log_path() -> PathBuf {
    crate::logs::log_dir().join("evoview_drive.log")
}

fn tail(n: usize) -> String {
    let t = std::fs::read(log_path()).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let lines: Vec<&str> = t.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// The last line that reads like a reason evoview gave up.
fn reason(log: &str) -> String {
    log.lines().rev().find(|l| {
        let l = l.to_lowercase();
        ["could not", "error", "panicked", "no collision", "not found", "failed", "no drivable"].iter().any(|k| l.contains(k))
    }).unwrap_or("").trim().to_string()
}

fn set_last(v: Value) {
    *LAST.lock().unwrap() = Some(v);
}

/// GET /api/evoview/state: the drive ACECM launched (one at a time).
pub fn state() -> Value {
    let mut g = RUN.lock().unwrap();
    if let Some(r) = g.as_mut() {
        match r.child.try_wait() {
            Ok(None) => {
                return json!({"state": if r.started.elapsed().as_secs() < 8 { "starting" } else { "running" },
                              "pid": r.child.id(), "what": r.what, "seconds": r.started.elapsed().as_secs(),
                              "log": tail(14)});
            }
            Ok(Some(code)) => {
                let secs = r.started.elapsed().as_secs_f32();
                let log = tail(40);
                let ok = code.success();
                // a quick non-zero exit is a load failure, not the player quitting
                let v = if !ok || secs < 6.0 {
                    let why = reason(&log);
                    json!({"state": "error", "what": r.what, "code": code.code(),
                           "detail": if why.is_empty() { format!("evoview stopped after {secs:.0} s (exit code {})", code.code().unwrap_or(-1)) } else { why },
                           "log": log})
                } else {
                    json!({"state": "exited", "what": r.what, "code": code.code(), "log": log})
                };
                crate::logs::info(format!("evoview drive ended: {}", v["state"]));
                *g = None;
                set_last(v.clone());
                return v;
            }
            Err(e) => {
                *g = None;
                set_last(json!({"state": "error", "detail": e.to_string()}));
            }
        }
    }
    LAST.lock().unwrap().clone().unwrap_or(json!({"state": "idle"}))
}

fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// POST /api/evoview/launch {profile, env, replace}. The core choices
/// (track, car, layout, setup, livery) are built here; `env` is the page's
/// knob table already turned into EVOVIEW_* values - each must be a name this
/// exe contains, or it is left out and reported.
pub fn launch(body: &Value) -> Value {
    let p = body.get("profile").cloned().unwrap_or(json!({}));
    let s = |k: &str| p.get(k).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let (track, car) = (s("track"), s("car"));
    if track.is_empty() || car.is_empty() {
        return json!({"ok": false, "error": "pick a track and a car first"});
    }
    let ok_id = |x: &str| x.chars().all(|c| c.is_ascii_alphanumeric() || "_-. ".contains(c)) && x.len() < 120;
    if !ok_id(&track) || !ok_id(&car) {
        return json!({"ok": false, "error": "that track or car id has characters evoview cannot take"});
    }
    let _ = save_profile(&p);
    let Some(exe) = viewer::viewer_exe() else {
        return json!({"ok": false, "error": "evoview.exe not found. Put it in the tools folder next to ACECM, or set viewer_exe in Settings."});
    };
    let Some(pkg) = viewer::package() else {
        return json!({"ok": false, "error": "game install not found - set the game folder in Settings"});
    };
    let exe_p = PathBuf::from(&exe);
    {
        let mut g = RUN.lock().unwrap();
        if let Some(r) = g.as_mut() {
            if matches!(r.child.try_wait(), Ok(None)) {
                if body.get("replace") != Some(&json!(true)) {
                    return json!({"ok": false, "busy": true, "error": format!("evoview is already running ({}) - stop it first", r.what)});
                }
                let _ = r.child.kill();
                let _ = r.child.wait();
            }
            *g = None;
        }
    }
    let mut args = vec![pkg.clone(), "--track".to_string(), track.clone(), "--drive-car".to_string(), car.clone()];
    let mut skipped: Vec<String> = vec![];
    let layout = s("layout");
    if !layout.is_empty() && ok_id(&layout) {
        args.push("--layout".into());
        args.push(layout);
    }
    let setup = s("setup");
    if !setup.is_empty() {
        if exe_has(&exe_p, "--list-setups") {
            args.push("--setup".into());
            args.push(setup);
        } else {
            skipped.push("--setup (this evoview has no saved-setup support yet)".into());
        }
    }
    let asked: Vec<String> = body.get("env").and_then(|e| e.as_object()).map(|m| m.keys().cloned().collect()).unwrap_or_default();
    let known = has_tokens(&exe_p, &asked);
    let mut env: Vec<(String, String)> = vec![];
    let livery = s("livery");
    if !livery.is_empty() && ok_id(&livery) {
        env.push(("EVOVIEW_VISUAL".into(), livery));
    }
    if let Some(m) = body.get("env").and_then(|e| e.as_object()) {
        for (k, v) in m {
            let val = match v {
                Value::String(x) => x.clone(),
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => if *b { "1".into() } else { "0".into() },
                _ => continue,
            };
            let name_ok = k.starts_with("EVOVIEW_") && k.len() < 48 && k.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
            if !name_ok || val.len() > 200 || val.contains('\0') {
                skipped.push(format!("{k} (not a valid evoview setting)"));
                continue;
            }
            if known.get(k) != Some(&json!(true)) {
                skipped.push(format!("{k} (this evoview does not know it)"));
                continue;
            }
            env.retain(|(n, _)| n != k);
            env.push((k.clone(), val));
        }
    }
    let log = log_path();
    let f = match std::fs::File::create(&log) {
        Ok(f) => f,
        Err(e) => return json!({"ok": false, "error": format!("cannot write {}: {e}", log.display())}),
    };
    let mut c = std::process::Command::new(&exe);
    c.args(&args).current_dir(paths::data()).stdin(std::process::Stdio::piped());
    match f.try_clone() {
        Ok(f2) => { c.stdout(f2).stderr(f); }
        Err(e) => return json!({"ok": false, "error": e.to_string()}),
    }
    // ⚠ a clean slate, as the test checklist's `Get-ChildItem Env:EVOVIEW_*
    // | Remove-Item`: anything ACECM inherited must not leak into the drive
    for (k, _) in std::env::vars() {
        if k.to_uppercase().starts_with("EVOVIEW_") {
            c.env_remove(k);
        }
    }
    for (k, v) in &env {
        c.env(k, v);
    }
    crate::procs::hide(&mut c);
    let mut child = match c.spawn() {
        Ok(ch) => ch,
        Err(e) => return json!({"ok": false, "error": format!("could not start evoview: {e}")}),
    };
    let what = format!("{car} on {track}");
    let stdin = child.stdin.take();
    let pid = child.id();
    *RUN.lock().unwrap() = Some(Run { child, stdin, started: std::time::Instant::now(), what: what.clone() });
    *LAST.lock().unwrap() = None;
    let mut ps: Vec<String> = vec!["Get-ChildItem Env:EVOVIEW_* | Remove-Item".into()];
    ps.extend(env.iter().map(|(k, v)| format!("$env:{k}={}", ps_quote(v))));
    let cmd = format!("{}\n& {} {}", ps.join("; "), ps_quote(&exe), args.iter().map(|a| if a.starts_with("--") { a.clone() } else { ps_quote(a) }).collect::<Vec<_>>().join(" "));
    crate::logs::info(format!("evoview drive: pid {pid} {what} env {:?} skipped {:?}", env, skipped));
    json!({"ok": true, "pid": pid, "what": what, "args": args, "env": env.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
           "skipped": skipped, "command": cmd, "log": log})
}

/// POST /api/evoview/stop: ask the drive to quit (stdin), then end it.
pub fn stop() -> Value {
    let mut g = RUN.lock().unwrap();
    let Some(r) = g.as_mut() else { return json!({"ok": true, "running": false}) };
    if let Some(si) = r.stdin.as_mut() {
        use std::io::Write;
        let _ = writeln!(si, "quit").and_then(|_| si.flush());
    }
    for _ in 0..20 {
        if !matches!(r.child.try_wait(), Ok(None)) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    if matches!(r.child.try_wait(), Ok(None)) {
        let _ = r.child.kill();
        let _ = r.child.wait();
    }
    let what = r.what.clone();
    *g = None;
    set_last(json!({"state": "exited", "what": what, "stopped": true}));
    json!({"ok": true})
}

// ------------------------------------------------------------ controls --

fn cfg_dir() -> PathBuf {
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| ".".into()).join("evoview")
}

/// wheel.cfg as rows: `role axis <axis> <rest> <full> <device>` /
/// `role button <n> <device>` (wheel.rs Mapping::load's grammar).
fn wheel_rows(text: &str) -> Vec<Value> {
    text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).filter_map(|l| {
        let f: Vec<&str> = l.splitn(3, ' ').collect();
        let (role, kind, rest) = (f.first()?, f.get(1)?, f.get(2).copied().unwrap_or(""));
        match *kind {
            "axis" => {
                let g: Vec<&str> = rest.splitn(4, ' ').collect();
                Some(json!({"role": role, "kind": "axis", "axis": g.first()?.parse::<i64>().ok()?,
                            "rest": g.get(1)?.parse::<i64>().ok()?, "full": g.get(2)?.parse::<i64>().ok()?,
                            "dev": g.get(3).copied().unwrap_or("")}))
            }
            "button" => {
                let (n, dev) = rest.split_once(' ').unwrap_or((rest, ""));
                Some(json!({"role": role, "kind": "button", "button": n.parse::<i64>().ok()?, "dev": dev}))
            }
            _ => None,
        }
    }).collect()
}

/// The game's controller bindings (read only): Saved Games\ACE\
/// input_devices.inputdeviceconfiguration - InputDeviceConfiguration {1
/// InputDeviceCommandList {1 controller {1 product_name}, 2 commands {1
/// ActionBase {1 input, 3 payload}, 2 index (the DirectInput button), 3
/// require_shift}}} - the same walk evoview's wheel.rs does.
fn game_binds() -> Vec<Value> {
    let p = Path::new(&crate::gamesettings::settings_dir()).join("input_devices.inputdeviceconfiguration");
    let Ok(d) = std::fs::read(&p) else { return vec![] };
    use crate::pb::{walk, Val};
    let names = action_names();
    let mut out = vec![];
    for dev in walk(&d).unwrap_or_default().into_iter().filter(|f| f.num == 1 && f.wire == 2) {
        let mut name = String::new();
        let mut cmds = vec![];
        for g in walk(dev.val.bytes()).unwrap_or_default() {
            match (g.num, g.wire) {
                (1, 2) => {
                    for h in walk(g.val.bytes()).unwrap_or_default() {
                        if h.num == 1 && h.wire == 2 {
                            name = String::from_utf8_lossy(h.val.bytes()).into_owned();
                        }
                    }
                }
                (2, 2) => {
                    let (mut act, mut pay, mut idx, mut shift) = (0u64, 0u64, None, false);
                    for h in walk(g.val.bytes()).unwrap_or_default() {
                        match (h.num, &h.val) {
                            (1, Val::Bytes(b)) => {
                                for k in walk(b).unwrap_or_default() {
                                    match (k.num, k.val) {
                                        (1, Val::Int(n)) => act = n,
                                        (3, Val::Int(n)) => pay = n,
                                        _ => {}
                                    }
                                }
                            }
                            (2, Val::Int(n)) => idx = Some(*n),
                            (3, Val::Int(n)) => shift = *n != 0,
                            _ => {}
                        }
                    }
                    if let Some(i) = idx {
                        cmds.push(json!({"action": act, "name": names.get(&act).cloned().unwrap_or_default(),
                                         "payload": pay, "button": i, "shift": shift}));
                    }
                }
                _ => {}
            }
        }
        out.push(json!({"device": name, "commands": cmds}));
    }
    out
}

/// InputAction value -> name, from the game's own schema (when it loads).
fn action_names() -> HashMap<u64, String> {
    let pool = crate::protos::pool();
    let Some(m) = pool.all_messages().find(|m| m.name() == "ActionBase") else { return HashMap::new() };
    let Some(f) = m.get_field(1) else { return HashMap::new() };
    match f.kind() {
        prost_reflect::Kind::Enum(e) => e.values().map(|v| (v.number() as u64, v.name().to_string())).collect(),
        _ => HashMap::new(),
    }
}

/// Attached joysticks (winmm), named the way DirectInput names them (the
/// OEMName under the VID/PID key) - that is what wheel.cfg stores.
#[cfg(windows)]
fn joysticks() -> Vec<Value> {
    #[repr(C)]
    struct Caps {
        mid: u16, pid: u16, pname: [u16; 32],
        xmin: u32, xmax: u32, ymin: u32, ymax: u32, zmin: u32, zmax: u32,
        buttons: u32, pmin: u32, pmax: u32,
        rmin: u32, rmax: u32, umin: u32, umax: u32, vmin: u32, vmax: u32,
        caps: u32, max_axes: u32, axes: u32, max_buttons: u32,
        regkey: [u16; 32], oem: [u16; 260],
    }
    type CapsFn = unsafe extern "system" fn(usize, *mut Caps, u32) -> u32;
    static LIB: std::sync::OnceLock<Option<(libloading::Library, CapsFn)>> = std::sync::OnceLock::new();
    let Some((_, f)) = LIB.get_or_init(|| unsafe {
        let lib = libloading::Library::new("winmm.dll").ok()?;
        let f: CapsFn = *lib.get::<CapsFn>(b"joyGetDevCapsW").ok()?;
        Some((lib, f))
    }).as_ref() else { return vec![] };
    let mut out = vec![];
    for id in 0..16u32 {
        // ⚠ only ids that answer: an absent id re-enumerates and leaks
        // registry handles (hotkey.rs read_joy keeps the list of dead ones)
        let Some(buttons) = crate::hotkey::read("joy", id) else { continue };
        let mut c: Caps = unsafe { std::mem::zeroed() };
        if unsafe { f(id as usize, &mut c, std::mem::size_of::<Caps>() as u32) } != 0 {
            continue;
        }
        let w = |s: &[u16]| String::from_utf16_lossy(&s[..s.iter().position(|&x| x == 0).unwrap_or(s.len())]);
        let key = format!("System\\CurrentControlSet\\Control\\MediaProperties\\PrivateProperties\\Joystick\\OEM\\VID_{:04X}&PID_{:04X}", c.mid, c.pid);
        let oem: Option<String> = [winreg::enums::HKEY_CURRENT_USER, winreg::enums::HKEY_LOCAL_MACHINE].iter().find_map(|h| {
            winreg::RegKey::predef(*h).open_subkey(&key).ok()?.get_value::<String, _>("OEMName").ok()
        });
        let name = oem.filter(|s| !s.trim().is_empty()).unwrap_or_else(|| w(&c.pname));
        let pressed: Vec<u32> = (0..32).filter(|i| buttons & (1 << i) != 0).collect();
        out.push(json!({"id": id, "name": name.trim(), "vid": format!("{:04X}", c.mid), "pid": format!("{:04X}", c.pid),
                        "buttons": c.buttons, "axes": c.axes, "pressed": pressed}));
    }
    out
}

#[cfg(not(windows))]
fn joysticks() -> Vec<Value> {
    vec![]
}

/// GET /api/evoview/controls: wheel.cfg, attached devices, the game's own
/// bindings, the FFB strength and the cockpit aids evoview remembered.
pub fn controls() -> Value {
    let dir = cfg_dir();
    let wheel = dir.join("wheel.cfg");
    let text = std::fs::read_to_string(&wheel).ok();
    let boost = std::fs::read_to_string(dir.join("ffb.cfg")).ok()
        .and_then(|t| t.split_whitespace().nth(1).and_then(|v| v.parse::<f64>().ok())).filter(|v| *v > 0.0);
    // aids.cfg: `<car> <bias %> <tc> <abs>` per line (gfx-controls)
    let aids: Vec<Value> = std::fs::read_to_string(dir.join("aids.cfg")).unwrap_or_default().lines().filter_map(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        (f.len() >= 4).then(|| json!({"car": f[0], "bias": f[1], "tc": f[2], "abs": f[3]}))
    }).collect();
    let running = matches!(state()["state"].as_str(), Some("starting" | "running"));
    json!({"ok": true, "dir": dir, "wheel_cfg": wheel, "exists": text.is_some(),
           "binds": wheel_rows(text.as_deref().unwrap_or("")), "devices": joysticks(),
           "game": game_binds(), "game_file": Path::new(&crate::gamesettings::settings_dir()).join("input_devices.inputdeviceconfiguration"),
           "ffb_boost": boost, "aids": aids, "drive_running": running})
}

fn role_ok(r: &str) -> bool {
    !r.is_empty() && r.len() < 32 && r.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// POST /api/evoview/bind {role, button, dev} sets a button; {role, clear}
/// drops the role. Every other line of wheel.cfg is kept as written, and the
/// first edit leaves wheel.cfg.acecm.bak beside it.
pub fn bind(b: &Value) -> Value {
    let role = b.get("role").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if !role_ok(&role) {
        return json!({"ok": false, "error": "not a wheel role"});
    }
    let clear = b.get("clear") == Some(&json!(true));
    let dev = b.get("dev").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let button = b.get("button").and_then(|v| v.as_i64()).unwrap_or(-1);
    if !clear && (!(0..128).contains(&button) || dev.is_empty() || dev.contains(['\n', '\r'])) {
        return json!({"ok": false, "error": "a button number 0-127 and a device are needed"});
    }
    let dir = cfg_dir();
    let p = dir.join("wheel.cfg");
    let text = std::fs::read_to_string(&p).unwrap_or_else(|_| "# evoview wheel mapping (--wheel-setup writes this)\n".into());
    let bak = dir.join("wheel.cfg.acecm.bak");
    if p.is_file() && !bak.exists() {
        let _ = std::fs::copy(&p, &bak);
    }
    let mut out: Vec<String> = text.lines().filter(|l| l.trim().split(' ').next() != Some(role.as_str())).map(String::from).collect();
    if !clear {
        out.push(format!("{role} button {button} {dev}"));
    }
    let _ = std::fs::create_dir_all(&dir);
    match std::fs::write(&p, out.join("\n") + "\n") {
        Ok(()) => json!({"ok": true}),
        Err(e) => json!({"ok": false, "error": format!("cannot write {}: {e}", p.display())}),
    }
}

/// POST /api/evoview/ffb {boost}: ffb.cfg, as Numpad +/- writes it.
pub fn set_ffb(b: &Value) -> Value {
    let Some(v) = b.get("boost").and_then(|v| v.as_f64()).filter(|v| v.is_finite()) else {
        return json!({"ok": false, "error": "boost must be a number"});
    };
    let v = v.clamp(0.2, 20.0);
    let dir = cfg_dir();
    let _ = std::fs::create_dir_all(&dir);
    match std::fs::write(dir.join("ffb.cfg"), format!("boost {v}\n")) {
        Ok(()) => json!({"ok": true, "boost": v}),
        Err(e) => json!({"ok": false, "error": e.to_string()}),
    }
}

/// POST /api/evoview/wheel_setup {roles}: evoview's own wizard, in a console
/// window of its own - it asks for each control and waits for Enter there.
pub fn wheel_setup(b: &Value) -> Value {
    let Some(exe) = viewer::viewer_exe() else { return json!({"ok": false, "error": "evoview.exe not found"}) };
    if matches!(state()["state"].as_str(), Some("starting" | "running")) {
        return json!({"ok": false, "error": "stop the drive first - the wizard needs the wheel to itself"});
    }
    let roles: Vec<String> = b.get("roles").and_then(|r| r.as_array()).into_iter().flatten()
        .filter_map(|r| r.as_str().map(String::from)).filter(|r| role_ok(r)).collect();
    let mut args = vec!["--wheel-setup".to_string()];
    args.extend(roles.iter().cloned());
    match crate::procs::spawn_console_window(Path::new(&exe), &args, &paths::data(), "evoview wheel setup") {
        Ok(pid) => json!({"ok": true, "pid": pid, "roles": roles}),
        Err(e) => json!({"ok": false, "error": e}),
    }
}
