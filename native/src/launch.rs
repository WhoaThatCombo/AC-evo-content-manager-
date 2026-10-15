//! Start the game client - ported from backend.launch_game and friends.
//!
//! Steam Launch Options never reach a process we start (the game logs
//! `Arguments: 1`), and steam_api often hands off to Steam, which starts a
//! fresh process without our flags. The rewritten lobby URL in the exe is
//! what actually steers a stock client; the flags below are belt and braces.

use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use crate::{backend, config, detect, logs, paths, procs, trackdeploy};

fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

fn cfg_str(cfg: &Map<String, Value>, k: &str) -> String {
    match cfg.get(k) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(v) => v.to_string(),
    }
}

pub fn backend_ws_url(port: i64) -> String {
    format!("wss://127.0.0.1:{port}/communicationNode/dev")
}

/// Flags that cut boot time - the game's own, off unless Settings says so.
pub fn fast_boot_flags(already: &[String]) -> Vec<String> {
    let cfg = config::load();
    let mut want = Vec::new();
    if !truthy(cfg.get("fast_boot")) {
        return want;
    }
    for name in ["skip_cars_preload", "quick_gamemode_swap", "no_idle_camera_sequence"] {
        if !already.iter().any(|a| a.contains(name)) {
            want.push(format!("-{name}"));
        }
    }
    let scene = cfg_str(&cfg, "startup_scene").trim().to_string();
    if !scene.is_empty() && !already.iter().any(|a| a.contains("startup_scene")) {
        want.push(format!("-startup_scene={scene}"));
    }
    want
}

#[cfg(windows)]
fn is_elevated() -> bool {
    unsafe { windows_sys::Win32::UI::Shell::IsUserAnAdmin() != 0 }
}

#[cfg(not(windows))]
fn is_elevated() -> bool {
    // nothing on Linux needs elevation to patch the game
    unsafe { libc_geteuid() == 0 }
}

#[cfg(not(windows))]
unsafe fn libc_geteuid() -> u32 {
    unsafe extern "C" { fn geteuid() -> u32; }
    unsafe { geteuid() }
}

/// Hand a URL to the shell (steam://...), as the logged-in user.
pub fn open_url(url: &str) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        let w = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
        let (op, u) = (w("open"), w(url));
        let r = unsafe { ShellExecuteW(std::ptr::null_mut(), op.as_ptr(), u.as_ptr(), std::ptr::null(), std::ptr::null(), 1) };
        r as isize > 32
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("xdg-open").arg(url).spawn().is_ok()
    }
}

fn steam_running() -> bool {
    !procs::pids_named(&["steam"]).is_empty()
}

/// steam.exe plus the helper that comes up once the client is usable.
fn steam_client_ready() -> bool {
    steam_running() && !procs::pids_named(&["steamwebhelper"]).is_empty()
}

/// Make sure Steam is actually up before steam://rungameid (a cold
/// steam.exe drops the URL or toasts a permission error).
pub fn ensure_steam(wait: f64) -> Value {
    if steam_client_ready() {
        return json!({"ok": true, "already": true});
    }
    let root = detect::steam_root();
    let steam = Path::new(&root).join("steam.exe");
    let mut started = false;
    if !steam_running() {
        if !steam.is_file() {
            return json!({"ok": false, "error": "Steam is not running and steam.exe was not found. Open Steam, then try again"});
        }
        let mut c = std::process::Command::new(&steam);
        c.arg("-silent");
        if !root.is_empty() {
            c.current_dir(&root);
        }
        match c.spawn() {
            Ok(_) => {
                procs::invalidate_snapshot();
                started = true;
                logs::info(format!("started Steam; waiting up to {wait:.0}s for the client"));
            }
            Err(e) => return json!({"ok": false, "error": format!("could not start Steam: {e}")}),
        }
    } else {
        logs::info(format!("Steam is up but still starting; waiting up to {wait:.0}s"));
    }
    let t0 = Instant::now();
    let limit = Duration::from_secs_f64(wait);
    while t0.elapsed() < limit {
        if steam_client_ready() {
            std::thread::sleep(Duration::from_millis(800));
            let waited = (t0.elapsed().as_secs_f64().min(wait) * 10.0).round() / 10.0;
            return json!({"ok": true, "started": started, "ready": true, "waited": waited});
        }
        std::thread::sleep(Duration::from_millis(400));
    }
    if steam_running() {
        logs::warn(format!("Steam did not finish starting in {wait:.0}s — launching the game anyway"));
        return json!({"ok": true, "started": started, "ready": false, "waited": wait});
    }
    json!({"ok": false, "error": format!("Steam did not open within {}s — open Steam yourself, then Drive again", wait as i64)})
}

/// Open the game as the logged-in Steam user, not as our token.
fn start_via_steam(appid: &str) -> (String, Vec<String>) {
    let url = format!("steam://rungameid/{appid}");
    // explorer.exe is not elevated even when we are - that is what makes
    // Family Share / "run as admin to patch" actually launch
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    let explorer = Path::new(&windir).join("explorer.exe");
    if cfg!(windows) && explorer.is_file() && std::process::Command::new(&explorer).arg(&url).spawn().is_ok() {
        return ("steam-url".into(), vec![explorer.to_string_lossy().into_owned(), url]);
    }
    if open_url(&url) {
        return ("steam-url".into(), vec![url]);
    }
    let steam = Path::new(&detect::steam_root()).join("steam.exe");
    if steam.is_file() && std::process::Command::new(&steam).args(["-applaunch", appid]).spawn().is_ok() {
        return ("steam".into(), vec![steam.to_string_lossy().into_owned(), "-applaunch".into(), appid.into()]);
    }
    (String::new(), Vec::new())
}

fn is_identifier(k: &str) -> bool {
    let mut c = k.chars();
    matches!(c.next(), Some(ch) if ch == '_' || ch.is_alphabetic()) && c.all(|ch| ch == '_' || ch.is_alphanumeric())
}

/// argv + env for the client on this platform.
fn client_launch_cmd(exe: &str, args: &[String]) -> Result<Vec<String>, String> {
    if cfg!(windows) {
        let mut v = vec![exe.to_string()];
        v.extend(args.iter().cloned());
        return Ok(v);
    }
    // ⚠ under Proton - still owned by the backend until the Linux port
    Err("direct launch under Proton is not ported yet".into())
}

fn spawn(argv: &[String], cwd: &str, env: &[(String, String)]) -> std::io::Result<()> {
    let mut c = std::process::Command::new(&argv[0]);
    c.args(&argv[1..]).current_dir(cwd);
    for (k, v) in env {
        c.env(k, v);
    }
    let r = c.spawn().map(|_| ());
    // the caller polls for this process next (snapshot cache)
    procs::invalidate_snapshot();
    r
}

/// Start the client, rewriting the exe first so Steam cannot skip us.
pub fn launch_game(extra_args: &[String]) -> Value {
    let cfg = config::load();
    let exe = backend::game_exe();
    let appid = { let a = cfg_str(&cfg, "steam_appid"); if a.is_empty() { "3058630".to_string() } else { a } };
    if exe.is_empty() {
        // ⚠ NOT ok: through Steam the client talks to Kunos, not our proxy
        if !cfg_str(&cfg, "steam_appid").is_empty() {
            open_url(&format!("steam://rungameid/{appid}"));
            return json!({"ok": false, "via": "steam", "started": true,
                "error": "the game exe could not be found, so it was started through Steam WITHOUT a rewritten lobby URL - the server browser will not fill. Set game_exe in Settings to AssettoCorsaEVO.exe and launch again."});
        }
        return json!({"ok": false, "error": "set game_exe in Settings"});
    }
    let mut patch = Value::Null;
    let mut inspector = Value::Null;
    if !truthy(backend::probe_inspector().get("inspector_patched")) {
        if procs::game_running() {
            return json!({"ok": false, "error": "the game is already running and its menu inspector is still off. Close it completely, then Launch again — ACECM will enable the inspector so Drive can press Start."});
        }
        inspector = backend::apply_inspector();
        if !truthy(inspector.get("ok")) {
            let extra = if truthy(inspector.get("needs_admin")) { " Run ACECM as administrator once to allow the write." } else { "" };
            let e = inspector.get("error").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).unwrap_or("could not enable the menu inspector");
            return json!({"ok": false, "error": format!("{e}{extra}"), "needs_admin": inspector.get("needs_admin"), "inspector": inspector});
        }
    }
    if !truthy(backend::probe_client_url().get("rdata_patched")) {
        if procs::game_running() {
            return json!({"ok": false, "error": "the game is already running and still points at the official lobby. Close it completely, then Launch again — ACECM will rewrite the URL first. -backend= is dropped by Steam and is not enough on its own."});
        }
        patch = backend::apply_redirect();
        if !truthy(patch.get("ok")) {
            let extra = if truthy(patch.get("needs_admin")) { " Run ACECM as administrator once to allow the write." } else { "" };
            let e = patch.get("error").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).unwrap_or("could not rewrite the client lobby URL");
            return json!({"ok": false, "error": format!("{e}{extra}"), "needs_admin": patch.get("needs_admin"), "patch": patch});
        }
    }
    // a content update wipes tracks.table rows we wrote: put them back
    // before the game commits to loading one and dies on an empty path
    if !procs::game_running() {
        let redec = trackdeploy::redeclare_client_tracks(false);
        if let Some(r) = redec.get("redeclared").and_then(|v| v.as_array()).filter(|a| !a.is_empty()) {
            let names: Vec<&str> = r.iter().filter_map(|x| x.get("folder").and_then(|f| f.as_str())).collect();
            logs::info(format!("auto-redeclared {} client track(s) wiped by an update before launch: {}", r.len(), names.join(", ")));
        } else if let Some(e) = redec.get("error").and_then(|v| v.as_str()).filter(|_| redec.get("ok") == Some(&json!(false))) {
            logs::warn(format!("auto-redeclare client tracks before launch: {e}"));
        }
    }
    let port = cfg.get("backend_port").and_then(|v| v.as_i64()).unwrap_or(448);
    let url = backend_ws_url(port);
    // env keeps insertion order, like the dict it is ported from
    let mut env: Vec<(String, String)> = vec![("FLAGS_backend".into(), url.clone())];
    let setenv = |env: &mut Vec<(String, String)>, k: &str, v: &str| {
        if let Some(e) = env.iter_mut().find(|(ek, _)| ek == k) { e.1 = v.to_string(); } else { env.push((k.to_string(), v.to_string())); }
    };
    let mut extra: Vec<String> = extra_args.to_vec();
    // always: the intro is a timed sequence, not loading
    if !extra.iter().any(|a| a.contains("no_intro")) {
        extra.push("-no_intro".into());
    }
    let fb = fast_boot_flags(&extra);
    extra.extend(fb);
    if extra.iter().any(|a| a.contains("ai_player_car")) {
        setenv(&mut env, "FLAGS_ai_player_car", "true");
    }
    if extra.iter().any(|a| a.contains("ai_enable_evo_next")) {
        setenv(&mut env, "FLAGS_ai_enable_evo_next", "true");
    }
    // gflags reads FLAGS_* from the environment; a bare bool is "true"
    let mut flag_names: Vec<String> = Vec::new();
    for a in &extra {
        let a = a.strip_prefix("--").or_else(|| a.strip_prefix('-')).unwrap_or(a);
        let (k, v) = match a.split_once('=') { Some((k, v)) => (k, v), None => (a, "true") };
        if !k.is_empty() && !v.is_empty() && is_identifier(k) {
            setenv(&mut env, &format!("FLAGS_{k}"), v);
            flag_names.push(k.to_string());
        }
    }
    let cwd = Path::new(&exe).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let flagfile = paths::data().join("evo.flags");
    {
        let mut lines = vec![format!("--backend={url}")];
        let has = |env: &Vec<(String, String)>, k: &str| env.iter().any(|(ek, ev)| ek == k && !ev.is_empty());
        if has(&env, "FLAGS_ai_player_car") {
            lines.push("--ai_player_car".into());
        }
        if has(&env, "FLAGS_ai_enable_evo_next") {
            lines.push("--ai_enable_evo_next".into());
        }
        for (k, v) in &env {
            if k.starts_with("FLAGS_") && !matches!(k.as_str(), "FLAGS_backend" | "FLAGS_ai_player_car" | "FLAGS_ai_enable_evo_next") {
                lines.push(format!("--{}={v}", &k[6..]));
            }
        }
        if std::fs::write(&flagfile, lines.join("\n") + "\n").is_ok() {
            let ff = flagfile.to_string_lossy();
            extra.push(format!("--flagfile={ff}"));
            extra.push(format!("-flagfile={ff}"));
            let mut names: Vec<String> = ["startup_gamemode", "load_single_car", "backend"].iter().map(|s| s.to_string()).chain(flag_names).collect();
            names.sort();
            names.dedup();
            let names = names.join(",");
            extra.push(format!("--fromenv={names}"));
            extra.push(format!("--tryfromenv={names}"));
        }
    }
    let steam = ensure_steam(20.0);
    if !truthy(steam.get("ok")) {
        let e = steam.get("error").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).unwrap_or("Steam is not running — open Steam, then Drive again");
        return json!({"ok": false, "error": e, "steam": steam});
    }
    // ⚠ Only start the exe ourselves when flags are wanted and we are not
    // elevated: steam_api checks ownership on the STARTING token. steam://
    // carries neither argv nor environment.
    let want_flags = extra.iter().any(|a| {
        let b = a.trim_start_matches('-');
        !(b.starts_with("flagfile") || b.starts_with("fromenv") || b.starts_with("tryfromenv"))
    });
    let mut via = String::new();
    let mut tried_direct: Value = Value::Null;
    let mut cmd: Vec<String> = Vec::new();
    let mut argv_full = vec![format!("-backend={url}"), format!("--backend={url}")];
    argv_full.extend(extra.iter().cloned());
    if truthy(cfg.get("direct_launch_refused")) {
        tried_direct = json!("skipped (refused before)");
    } else if want_flags && !is_elevated() {
        setenv(&mut env, "SteamAppId", &appid);
        setenv(&mut env, "SteamGameId", &appid);
        let mut launch_error = false;
        match client_launch_cmd(&exe, &argv_full) {
            Ok(c) => {
                cmd = c;
                match spawn(&cmd, &cwd, &env) {
                    Ok(()) => {
                        // ⚠ wait, then check it is STILL there: the refusal
                        // comes after steam_api initialises
                        std::thread::sleep(Duration::from_secs(if cfg!(windows) { 8 } else { 25 }));
                        if procs::game_running() {
                            via = "exe".into();
                            tried_direct = json!("ok");
                        } else {
                            tried_direct = json!("refused or exited within 8s");
                        }
                    }
                    Err(e) => { tried_direct = json!(format!("failed: {e}")); launch_error = true; }
                }
            }
            Err(e) => { tried_direct = json!(format!("failed: {e}")); launch_error = true; }
        }
        if via.is_empty() {
            // ⚠ only a REFUSAL is a fact about this machine worth keeping
            if !launch_error {
                let _ = config::save(&json!({"direct_launch_refused": true}));
            }
            logs::info(format!("direct launch {} - falling back to Steam, and not trying again (flags will not reach the game)",
                tried_direct.as_str().unwrap_or("")));
        }
    }
    if via.is_empty() {
        let (v, c) = start_via_steam(&appid);
        via = v;
        cmd = c;
    }
    if via.is_empty() {
        setenv(&mut env, "SteamAppId", &appid);
        setenv(&mut env, "SteamGameId", &appid);
        if let Ok(c) = client_launch_cmd(&exe, &argv_full) {
            cmd = c;
            via = "exe".into();
            if let Err(e) = spawn(&cmd, &cwd, &env) {
                return json!({"ok": false, "error": format!("could not start the game: {e}")});
            }
        }
    }
    let flags_env: Map<String, Value> = env.iter().filter(|(k, _)| k.starts_with("FLAGS_")).map(|(k, v)| (k.clone(), json!(v))).collect();
    let rdata = backend::probe_client_url().get("rdata_patched").cloned().unwrap_or(Value::Null);
    let insp = backend::probe_inspector().get("inspector_patched").cloned().unwrap_or(Value::Null);
    logs::info(format!("launched game client pid=None  {}\n    argv: {}",
        json!({"backend": url, "via": via, "rdata_patched": rdata, "inspector_patched": insp, "steam": steam,
               "flags_delivered": via == "exe", "direct_launch": tried_direct, "flags_env": flags_env}),
        cmd.join(" ")));
    json!({"ok": true, "via": via, "backend": url,
           "rdata_patched": rdata, "inspector_patched": insp,
           "patch": patch, "inspector": inspector, "steam": steam,
           "flags_delivered": via == "exe",
           "direct_launch": tried_direct,
           "flags_env": flags_env})
}
