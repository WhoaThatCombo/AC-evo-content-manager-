//! Which cars have geometry, across the base game and every installed mod -
//! ported from viewer.packages / viewer.index. Cached in
//! <data>/viewer/index.json (shared with the backend, same stamp format).
//! Also launches evoview on a car or a track.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde_json::{json, Map, Value};

use crate::{detect, kspkg, mods, paths};

fn root() -> PathBuf {
    let d = paths::data().join("viewer");
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn cars_root() -> PathBuf {
    root().join("cars")
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// The client's content.kspkg (fresh detection, like viewer.package).
pub fn package() -> Option<String> {
    // dev/test only: point every client-archive read AND write at a copy
    if let Ok(p) = std::env::var("ACECM_CLIENT_KSPKG") {
        return Path::new(&p).is_file().then_some(p);
    }
    let g = detect::steam_app_dir(detect::APPID_GAME);
    let g = if g.is_empty() { detect::find("game_dir", true) } else { g };
    if g.is_empty() {
        return None;
    }
    let p = Path::new(&g).join("content.kspkg");
    p.is_file().then(|| s(&p))
}

/// (mod name or "", package path): the base game, then each mod package
/// (client folder first, a name seen once).
pub fn packages() -> Vec<(String, String)> {
    let mut out = vec![];
    if let Some(b) = package() {
        out.push((String::new(), b));
    }
    let mut seen = HashSet::new();
    for d in [mods::client_mods_dir(), mods::mods_dir()] {
        let dir = Path::new(&d);
        if d.is_empty() || !dir.is_dir() {
            continue;
        }
        let mut names: Vec<String> = std::fs::read_dir(dir).map(|rd| {
            rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()
        }).unwrap_or_default();
        names.sort();
        for f in names {
            if !f.to_lowercase().ends_with(".kspkg") {
                continue;
            }
            if !seen.insert(f.to_lowercase()) {
                continue;
            }
            let stem = f.rsplit_once('.').map(|(a, _)| a.to_string()).unwrap_or(f.clone());
            out.push((stem, s(&dir.join(&f))));
        }
    }
    out
}

fn mtime_secs(m: &std::fs::Metadata) -> u64 {
    m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0)
}

fn stamp(path: &str) -> String {
    std::fs::metadata(path).map(|m| format!("{}:{}", m.len(), mtime_secs(&m))).unwrap_or_default()
}

pub fn pretty(car_id: &str) -> String {
    let s = car_id.strip_prefix("ks_").unwrap_or(car_id);
    s.split('_').filter(|w| !w.is_empty()).map(|w| {
        if w.chars().count() <= 3 && w.chars().all(|c| c.is_alphabetic()) {
            w.to_uppercase()
        } else {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
                None => String::new(),
            }
        }
    }).collect::<Vec<_>>().join(" ")
}

fn mark(cars: &mut [Value]) {
    // recomputed on every read: it changes whenever a car is viewed
    let root = cars_root();
    for c in cars.iter_mut() {
        let id = c.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        c["extracted"] = json!(root.join(&id).is_dir());
    }
}

/// Every car that has geometry, cached on the packages AND the mod folders.
/// index(false), reused for a few seconds. ⚠ Each call lists every package,
/// stats them, counts both mod folders and parses index.json - ~26 ms - and
/// a picture check asks package_for() once per car: 121 cars made Home and
/// Play take 1-3 s to open on a real install.
static INDEX_MEM: std::sync::Mutex<Option<(std::time::Instant, Value)>> = std::sync::Mutex::new(None);
const INDEX_TTL_MS: u128 = 3000;

pub fn index(refresh: bool) -> Value {
    if !refresh {
        if let Some((at, v)) = INDEX_MEM.lock().unwrap().as_ref() {
            if at.elapsed().as_millis() < INDEX_TTL_MS {
                return v.clone();
            }
        }
    }
    let v = index_now(refresh);
    *INDEX_MEM.lock().unwrap() = Some((std::time::Instant::now(), v.clone()));
    v
}

fn index_now(refresh: bool) -> Value {
    let pkgs = packages();
    if pkgs.is_empty() {
        return json!({"error": "game install not found - set the game folder in Settings", "cars": []});
    }
    let cache = root().join("index.json");
    let mut parts: Vec<String> = pkgs.iter().map(|(_, p)| {
        let base = Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        format!("{base}:{}", stamp(p))
    }).collect();
    for d in [mods::client_mods_dir(), mods::mods_dir()] {
        if d.is_empty() || !Path::new(&d).is_dir() {
            continue;
        }
        if let Ok(m) = std::fs::metadata(&d) {
            let n = std::fs::read_dir(&d).map(|r| r.count()).unwrap_or(0);
            parts.push(format!("{d}:{}:{n}", mtime_secs(&m)));
        }
    }
    let st = parts.join("|");
    if !refresh {
        if let Some(got) = std::fs::read(&cache).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) {
            if got.get("stamp").and_then(|v| v.as_str()) == Some(&st) {
                let mut cars = got.get("cars").and_then(|c| c.as_array()).cloned().unwrap_or_default();
                mark(&mut cars);
                return json!({"cars": cars, "cached": true});
            }
        }
    }
    // car id -> (mod, pkg); first package wins, like setdefault
    let mut found: BTreeMap<String, (String, String)> = BTreeMap::new();
    let mut order: Vec<String> = vec![];
    let mut bad = vec![];
    for (m, pkg) in &pkgs {
        // ⚠ one unreadable package must not empty the whole list
        match kspkg::entries(Path::new(pkg)) {
            Ok(es) => {
                for e in es {
                    if e.size == 0 {
                        continue;
                    }
                    let low = e.path.to_lowercase();
                    if !low.starts_with("content\\cars\\") || !low.contains("\\meshes\\") || !low.ends_with(".mesh") {
                        continue;
                    }
                    let parts: Vec<&str> = e.path.split('\\').collect();
                    if parts.len() > 2 && !parts[2].to_lowercase().starts_with("common") {
                        let id = parts[2].to_string();
                        if !found.contains_key(&id) {
                            order.push(id.clone());
                            found.insert(id, (m.clone(), pkg.clone()));
                        }
                    }
                }
            }
            Err(_) => bad.push(Path::new(pkg).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
        }
    }
    // a mod's real name is declared against its PRESET id; take the label
    // from the mod's own manifest for the folder id we list cars by
    let mut labels: Map<String, Value> = mods::car_names();
    for which in ["client", "server"] {
        let inst = mods::installed(which);
        for m in inst.get("mods").and_then(|v| v.as_array()).into_iter().flatten() {
            let Some(name) = m.get("name").and_then(|v| v.as_str()) else { continue };
            for c in m.get("cars").and_then(|v| v.as_array()).into_iter().flatten() {
                if let Some(l) = c.get("label").filter(|l| l.as_str().map(|s| !s.is_empty()).unwrap_or(!l.is_null())) {
                    labels.entry(name.to_string()).or_insert(l.clone());
                    break;
                }
            }
        }
    }
    let mut cars: Vec<Value> = order.iter().map(|id| {
        let (m, pkg) = &found[id];
        let label = match labels.get(id) {
            Some(Value::String(s)) if !s.is_empty() => json!(s),
            Some(v) if !v.is_null() && !v.is_string() => v.clone(),
            _ => json!(pretty(id)),
        };
        json!({"id": id, "label": label, "mod": !m.is_empty(), "pkg": pkg})
    }).collect();
    cars.sort_by(|a, b| {
        let k = |c: &Value| (c["mod"].as_bool().unwrap_or(false), c["label"].as_str().unwrap_or("").to_string());
        k(a).cmp(&k(b))
    });
    let _ = std::fs::write(&cache, serde_json::to_vec(&json!({"stamp": st, "cars": cars})).unwrap_or_default());
    mark(&mut cars);
    json!({"cars": cars, "cached": false, "unreadable": bad})
}

// ----------------------------------------------------------- the viewer --
static JOBS: std::sync::Mutex<Option<std::collections::HashMap<String, Value>>> = std::sync::Mutex::new(None);

fn set_job(id: &str, state: &str, detail: &str) {
    JOBS.lock().unwrap().get_or_insert_with(Default::default).insert(id.to_string(), json!({"state": state, "detail": detail}));
    crate::logs::info(format!("viewer {id}: {state} {detail}"));
}

pub fn job(id: &str) -> Value {
    JOBS.lock().unwrap().get_or_insert_with(Default::default).get(id).cloned().unwrap_or(json!({"state": "idle", "detail": ""}))
}

/// evoview.exe: the setting, then beside us, then the checkout next door.
pub fn viewer_exe() -> Option<String> {
    let cfg = crate::config::load();
    let c = crate::config::get_str(&cfg, "viewer_exe").trim().to_string();
    if !c.is_empty() && Path::new(&c).is_file() {
        return Some(c);
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(d) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        roots.push(d.join("tools"));
        roots.push(d.clone());
    }
    roots.push(crate::bundle::tools_dir());
    let repo = paths::repo();
    roots.push(repo.join("tools"));
    roots.push(repo.join("viewer"));
    roots.push(root());
    if let Some(p) = repo.parent() {
        roots.push(p.join("evoview").join("target").join("release"));
    }
    roots.into_iter().map(|r| r.join("evoview.exe")).find(|p| p.is_file()).map(|p| s(&p))
}

/// Which package holds this car: the base game, or a mod's own.
pub fn package_for(car_id: &str) -> Option<String> {
    // car id -> its own package, from index(), rebuilt every few seconds
    // (scanning a cloned index per call cost ~2 ms a car)
    static MAP: std::sync::Mutex<Option<(std::time::Instant, std::collections::HashMap<String, String>)>> = std::sync::Mutex::new(None);
    let mut g = MAP.lock().unwrap();
    if !g.as_ref().map(|(at, _)| at.elapsed().as_millis() < INDEX_TTL_MS).unwrap_or(false) {
        let mut m = std::collections::HashMap::new();
        for c in index(false).get("cars").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
            if let Some(id) = c.get("id").and_then(|v| v.as_str()) {
                let pkg = c.get("pkg").and_then(|v| v.as_str()).unwrap_or("").to_string();
                m.entry(id.to_string()).or_insert(pkg);
            }
        }
        *g = Some((std::time::Instant::now(), m));
    }
    let hit = g.as_ref().and_then(|(_, m)| m.get(car_id).cloned());
    drop(g);
    match hit {
        Some(p) if !p.is_empty() => Some(p),
        _ => package(),
    }
}

/// evoview is a console binary: its own console, hidden; cwd is our data folder.
fn launch(args: &[String], env: &[(&str, &str)]) -> Result<(), String> {
    let exe = viewer_exe().ok_or("evoview.exe not found. Put it in the tools folder next to ACECM, or set viewer_exe in Settings.")?;
    let log = crate::logs::log_dir().join("evoview.log");
    crate::procs::spawn_hidden_console(Path::new(&exe), args, &paths::data(), &log, env).map(|_| ())
}

/// `<pkg> --car <id> [--base <content.kspkg>]` for a car in any package.
fn car_args(car_id: &str) -> Result<(String, Vec<String>), String> {
    let pkg = package_for(car_id).ok_or("game install not found")?;
    let mut args = vec![pkg.clone(), "--car".into(), car_id.to_string()];
    if let Some(base) = package() {
        if std::path::absolute(&base).ok() != std::path::absolute(&pkg).ok() {
            args.push("--base".into());
            args.push(base);
        }
    }
    Ok((pkg, args))
}

/// The viewer ACECM launched last, kept with a pipe to its stdin: evoview
/// reads `livery <stem>` lines there and switches in ~150 ms, instead of a
/// relaunch that reloads the whole car.
struct Running {
    car: String,
    paint: String,
    child: std::process::Child,
    stdin: std::process::ChildStdin,
}
static RUNNING: std::sync::Mutex<Option<Running>> = std::sync::Mutex::new(None);

/// Launch the viewer on a car, read straight out of its package. `livery` is
/// a visual preset stem (EVOVIEW_VISUAL); "" = the car's default. When the
/// viewer is already showing this car, it is switched in place.
pub fn open_car(car_id: &str, paint: &str, livery: &str) -> Result<Value, String> {
    let exe = viewer_exe().ok_or("evoview.exe not found. Put it in the tools folder next to ACECM, or set viewer_exe in Settings.")?;
    let (pkg, args) = car_args(car_id)?;
    let mut g = RUNNING.lock().unwrap();
    if let Some(r) = g.as_mut() {
        let alive = matches!(r.child.try_wait(), Ok(None));
        if alive && r.car == car_id && r.paint == paint && !livery.is_empty() {
            use std::io::Write;
            if writeln!(r.stdin, "livery {livery}").and_then(|_| r.stdin.flush()).is_ok() {
                set_job(car_id, "open", "");
                return Ok(json!({"ok": true, "package": pkg, "switched": true}));
            }
        }
        if !alive {
            *g = None;
        }
    }
    let log = crate::logs::log_dir().join("evoview.log");
    let f = std::fs::File::create(&log).map_err(|e| format!("cannot open {}: {e}", log.display()))?;
    let mut c = std::process::Command::new(&exe);
    c.args(&args).current_dir(paths::data())
        .stdin(std::process::Stdio::piped())
        .stdout(f.try_clone().map_err(|e| e.to_string())?)
        .stderr(f);
    if !paint.is_empty() {
        c.env("EVOVIEW_PAINT", paint);
    }
    if !livery.is_empty() {
        c.env("EVOVIEW_VISUAL", livery);
    }
    crate::procs::hide(&mut c);
    let mut child = c.spawn().map_err(|e| format!("could not start evoview: {e}"))?;
    if let Some(stdin) = child.stdin.take() {
        // an older viewer for another car keeps running, as before
        *g = Some(Running { car: car_id.to_string(), paint: paint.to_string(), child, stdin });
    }
    set_job(car_id, "open", "");
    Ok(json!({"ok": true, "package": pkg, "switched": false}))
}

/// Does this evoview know --list-liveries? (its --help says so; cached per exe stamp)
fn supports_liveries(exe: &str) -> bool {
    static SEEN: std::sync::Mutex<Option<(String, bool)>> = std::sync::Mutex::new(None);
    let st = format!("{exe}|{}", stamp(exe));
    if let Some((k, v)) = SEEN.lock().unwrap().as_ref() {
        if *k == st {
            return *v;
        }
    }
    let mut c = std::process::Command::new(exe);
    c.arg("--help").stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
    crate::procs::hide(&mut c);
    let ok = c.output().map(|o| String::from_utf8_lossy(&o.stdout).contains("--list-liveries")).unwrap_or(false);
    *SEEN.lock().unwrap() = Some((st, ok));
    ok
}

/// A car's liveries (visual presets): [{stem, name, number}], default first,
/// from `evoview --list-liveries`. Cached in <data>/viewer/liveries.json on
/// the package stamp and the viewer's, so the picker fills instantly after
/// the first look (~0.3 s for the listing itself).
pub fn liveries(car_id: &str) -> Value {
    let Some(exe) = viewer_exe() else { return json!({"error": "evoview.exe not found", "liveries": []}) };
    let (pkg, args) = match car_args(car_id) {
        Ok(x) => x,
        Err(e) => return json!({"error": e, "liveries": []}),
    };
    let key = format!("{}|{}|{}", stamp(&pkg), stamp(&exe), car_id);
    let path = root().join("liveries.json");
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _l = LOCK.lock().unwrap();
    let mut cache: Map<String, Value> = std::fs::read_to_string(&path).ok()
        .and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    if let Some(v) = cache.get(car_id).filter(|v| v.get("key").and_then(|k| k.as_str()) == Some(key.as_str())) {
        return json!({"liveries": v.get("liveries").cloned().unwrap_or(json!([]))});
    }
    // ⚠ An older evoview ignores --list-liveries and opens a window on the
    // car instead (and output() would wait for it to close). Ask --help first.
    if !supports_liveries(&exe) {
        return json!({"error": "this evoview has no livery support - update it", "liveries": []});
    }
    let mut c = std::process::Command::new(&exe);
    c.args(&args).arg("--list-liveries").current_dir(paths::data())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
    crate::procs::hide(&mut c);
    let out = match c.output() {
        Ok(o) => o,
        Err(e) => return json!({"error": format!("could not run evoview: {e}"), "liveries": []}),
    };
    let text = String::from_utf8_lossy(&out.stdout);
    // stem<TAB>name<TAB>number; an old viewer without the flag prints none
    let list: Vec<Value> = text.lines().filter_map(|l| {
        let mut it = l.split('\t');
        let stem = it.next()?.trim();
        let name = it.next()?.trim();
        if stem.is_empty() || stem.contains(' ') {
            return None;
        }
        let number = it.next().map(str::trim).unwrap_or("");
        Some(json!({"stem": stem, "name": name, "number": number}))
    }).collect();
    cache.insert(car_id.to_string(), json!({"key": key, "liveries": list}));
    let _ = std::fs::write(&path, serde_json::to_string(&Value::Object(cache)).unwrap_or_default());
    json!({"liveries": list})
}

/// Free-look on a track: a loose imported folder wins, else the archive.
pub fn open_track(folder: &str) -> Result<Value, String> {
    let folder = folder.trim();
    if folder.is_empty() {
        return Err("no track folder".into());
    }
    viewer_exe().ok_or("evoview.exe not found. Put it in the tools folder next to ACECM, or set viewer_exe in Settings.")?;
    let loose = Path::new(&crate::trackimport::tracks_dir()).join(folder);
    let pkg = package();
    let (args, src) = if loose.is_dir() {
        let mut a = vec![s(&loose), "--track".into()];
        if let Some(p) = &pkg {
            a.push("--base".into());
            a.push(p.clone());
        }
        (a, s(&loose))
    } else {
        let p = pkg.ok_or("game install not found")?;
        (vec![p.clone(), "--track".into(), folder.to_string()], p)
    };
    launch(&args, &[])?;
    set_job(&format!("track:{folder}"), "open", "");
    crate::logs::info(format!("viewer track {folder} from {src}"));
    Ok(json!({"ok": true, "folder": folder, "source": src}))
}

pub fn start_open(car_id: &str, paint: &str, livery: &str) -> Value {
    set_job(car_id, "starting", "");
    let (c, p, l) = (car_id.to_string(), paint.to_string(), livery.to_string());
    std::thread::spawn(move || {
        if let Err(e) = open_car(&c, &p, &l) {
            set_job(&c, "error", &e);
        }
    });
    json!({"ok": true})
}

pub fn start_open_track(folder: &str) -> Value {
    set_job(&format!("track:{folder}"), "starting", "");
    let f = folder.to_string();
    std::thread::spawn(move || {
        if let Err(e) = open_track(&f) {
            set_job(&format!("track:{f}"), "error", &e);
        }
    });
    json!({"ok": true})
}
