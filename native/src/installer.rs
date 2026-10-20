//! Putting ACECM somewhere permanent, with shortcuts - ported from
//! installer.py.
//!
//!     %LOCALAPPDATA%\Programs\ACECM\ACECM.exe
//!
//! Per-user on purpose: no admin prompt. ⚠ The data folder is never touched:
//! installing, reinstalling and uninstalling all leave profiles alone.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::updater::{self, VERSION};

const APP: &str = crate::edition::APP;

pub fn install_dir() -> PathBuf {
    // dev override: test installs never touch the real one
    if let Ok(d) = std::env::var("ACECM_INSTALL_DIR") {
        return PathBuf::from(d);
    }
    if cfg!(windows) {
        let base = std::env::var("LOCALAPPDATA").map(PathBuf::from)
            .unwrap_or_else(|_| home().join("AppData").join("Local"));
        base.join("Programs").join(APP)
    } else {
        // ~/.local/bin: on PATH, needs no root (immutable /usr)
        home().join(".local").join("bin")
    }
}

fn home() -> PathBuf {
    std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("."))
}

pub fn installed_exe() -> PathBuf {
    install_dir().join(if cfg!(windows) { format!("{APP}.exe") } else { APP.to_string() })
}

fn start_menu() -> PathBuf {
    if let Ok(d) = std::env::var("ACECM_SHORTCUT_DIR") {
        return PathBuf::from(d).join("menu").join(format!("{APP}.{}", if cfg!(windows) { "lnk" } else { "desktop" }));
    }
    if cfg!(windows) {
        let base = std::env::var("APPDATA").map(PathBuf::from).unwrap_or_else(|_| home().join("AppData").join("Roaming"));
        base.join("Microsoft").join("Windows").join("Start Menu").join("Programs").join(format!("{APP}.lnk"))
    } else {
        let base = std::env::var("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|_| home().join(".local").join("share"));
        base.join("applications").join(format!("{APP}.desktop"))
    }
}

fn desktop_link() -> PathBuf {
    // ⚠ ask the shell: a OneDrive-redirected profile has no ~\Desktop
    let ext = if cfg!(windows) { "lnk" } else { "desktop" };
    if let Ok(d) = std::env::var("ACECM_SHORTCUT_DIR") {
        return PathBuf::from(d).join("desktop").join(format!("{APP}.{ext}"));
    }
    PathBuf::from(crate::detect::desktop()).join(format!("{APP}.{ext}"))
}

/// The file to install, or None when running from source.
pub fn running_exe() -> Option<PathBuf> {
    if !updater::frozen() {
        return None;
    }
    std::env::current_exe().ok().map(|p| std::path::absolute(&p).unwrap_or(p))
}

fn same(a: &Path, b: &Path) -> bool {
    let n = |p: &Path| std::path::absolute(p).unwrap_or(p.to_path_buf()).to_string_lossy().to_lowercase();
    if cfg!(windows) { n(a) == n(b) } else { a == b }
}

fn ps(p: &Path) -> String {
    p.to_string_lossy().to_string()
}

pub fn status() -> Value {
    let exe = installed_exe();
    let here = running_exe();
    let mut out = json!({
        "frozen": updater::frozen(),
        "install_dir": ps(&install_dir()),
        "installed": exe.is_file(),
        "installed_exe": ps(&exe),
        "running_exe": here.as_ref().map(|p| ps(p)),
        "running_installed": here.as_ref().map(|h| same(h, &exe)).unwrap_or(false),
        "start_menu": start_menu().is_file(),
        "desktop": desktop_link().is_file(),
        "version": VERSION,
    });
    if exe.is_file() {
        if let Ok(m) = std::fs::metadata(&exe) {
            out["installed_size"] = json!(m.len());
        }
    }
    if !updater::frozen() {
        out["note"] = json!("running from source - build the exe first (cargo build --release), then install from the built copy");
    }
    out
}

/// A .lnk via the shell (PowerShell is always present), or an XDG entry.
fn shortcut(link: &Path, target: &Path, args: &str, desc: &str) -> Result<PathBuf, String> {
    let dir = target.parent().map(ps).unwrap_or_default();
    if !cfg!(windows) {
        // ⚠ must be executable or the file manager shows it as text
        let icon = target.parent().map(|d| d.join("acecm.ico")).filter(|p| p.is_file());
        let mut body = vec!["[Desktop Entry]".to_string(), "Type=Application".into(), format!("Name={APP}"),
            format!("Comment={}", if desc.is_empty() { APP } else { desc }),
            format!("Exec=\"{}\"{}", target.display(), if args.is_empty() { String::new() } else { format!(" {args}") }),
            format!("Path={dir}"), "Terminal=false".into(), "Categories=Game;Utility;".into(), "StartupNotify=true".into()];
        if let Some(i) = icon {
            body.insert(5, format!("Icon={}", i.display()));
        }
        if let Some(d) = link.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        std::fs::write(link, body.join("\n") + "\n").map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(link, std::fs::Permissions::from_mode(0o755));
        }
        return Ok(link.to_path_buf());
    }
    let q = |s: &str| s.replace('\'', "''");
    let script = format!(
        "$s = (New-Object -ComObject WScript.Shell).CreateShortcut('{}');$s.TargetPath = '{}';$s.Arguments = '{}';$s.WorkingDirectory = '{}';$s.Description = '{}';$s.Save()",
        q(&ps(link)), q(&ps(target)), q(args), q(&dir), q(desc));
    let mut c = std::process::Command::new("powershell");
    c.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
    crate::procs::hide(&mut c);
    let o = c.output().map_err(|e| e.to_string())?;
    if !o.status.success() {
        let e = String::from_utf8_lossy(if o.stderr.is_empty() { &o.stdout } else { &o.stderr }).trim().to_string();
        return Err(if e.is_empty() { "shortcut failed".into() } else { e });
    }
    Ok(link.to_path_buf())
}

/// The installed copy's version, from the marker beside it (never by
/// running an unknown exe).
pub fn installed_version() -> String {
    std::fs::read_to_string(install_dir().join("version.txt")).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// Is version a older than b? Unparseable versions never block.
fn older(a: &str, b: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> { v.trim_start_matches('v').split('.').filter(|x| !x.is_empty() && x.chars().all(|c| c.is_ascii_digit())).filter_map(|x| x.parse().ok()).collect() };
    let (pa, pb) = (parts(a), parts(b));
    !pa.is_empty() && !pb.is_empty() && pa < pb
}

fn shortcuts(target: &Path, desktop: bool) -> Vec<String> {
    let mut made = Vec::new();
    for (link, want) in [(start_menu(), true), (desktop_link(), desktop)] {
        if !want {
            continue;
        }
        match shortcut(&link, target, "", &format!("{APP} {VERSION}")) {
            Ok(l) => made.push(ps(&l)),
            Err(e) => crate::logs::warn(format!("shortcut {}: {e}", link.display())),
        }
    }
    made
}

/// Copy this exe into the install folder and make the shortcuts.
/// ⚠ Refuses to go backwards unless forced: an old download left in
/// Downloads would otherwise overwrite a newer, updated install.
pub fn install(desktop: bool, force: bool) -> Value {
    let Some(src) = running_exe() else {
        return json!({"ok": false, "error": status()["note"]});
    };
    let dst = installed_exe();
    let there = installed_version();
    if !there.is_empty() && !force && older(VERSION, &there) {
        return json!({"ok": false, "downgrade": true,
            "error": format!("the installed copy is newer (v{there}) than this one (v{VERSION}) - this exe is an old download. Launch ACECM from the Start Menu, or install again with force to go back to v{VERSION}.")});
    }
    if same(&src, &dst) {
        let made = shortcuts(&dst, desktop);
        return json!({"ok": true, "already": true, "exe": ps(&dst), "shortcuts": made});
    }
    let _ = std::fs::create_dir_all(install_dir());
    // ⚠ copy to a temp name and rename, so a failed copy cannot leave a
    // half-written exe where the shortcut points
    let tmp = PathBuf::from(format!("{}.new", dst.display()));
    let r = std::fs::copy(&src, &tmp).and_then(|_| std::fs::rename(&tmp, &dst));
    if let Err(e) = r {
        let _ = std::fs::remove_file(&tmp);
        if e.kind() == std::io::ErrorKind::PermissionDenied || e.raw_os_error() == Some(32) {
            return json!({"ok": false, "error": "the installed copy is running - close that window first, then install again"});
        }
        return json!({"ok": false, "error": format!("could not copy: {e}")});
    }
    if let Err(e) = std::fs::write(install_dir().join("version.txt"), VERSION) {
        crate::logs::warn(format!("could not write the version marker: {e}"));
    }
    let made = shortcuts(&dst, desktop);
    crate::logs::info(format!("installed to {}", dst.display()));
    json!({"ok": true, "exe": ps(&dst), "shortcuts": made, "version": VERSION,
           "replaced": if there.is_empty() { Value::Null } else { json!(there) }})
}

/// A shortcut that starts ACECM as a server with no window - pointing at the
/// INSTALLED copy, so it still works months later.
pub fn headless_shortcut(desktop: bool) -> Value {
    let mut target = installed_exe();
    if !target.is_file() {
        match running_exe() {
            Some(r) => target = r,
            None => return json!({"ok": false, "error": "no installed ACECM.exe to point at - use Install to a permanent folder first"}),
        }
    }
    let rename = |p: PathBuf| {
        let name = p.file_name().map(|n| n.to_string_lossy().replace(&format!("{APP}.lnk"), &format!("{APP} Server.lnk"))).unwrap_or_default();
        p.with_file_name(name)
    };
    let (mut made, mut failed) = (Vec::new(), Vec::new());
    for (link, want) in [(rename(start_menu()), true), (rename(desktop_link()), desktop)] {
        if !want {
            continue;
        }
        match shortcut(&link, &target, "--headless", &format!("{APP} as a headless server (remote admin)")) {
            Ok(l) => made.push(ps(&l)),
            Err(e) => {
                crate::logs::warn(format!("headless shortcut {}: {e}", link.display()));
                failed.push(e);
            }
        }
    }
    if made.is_empty() {
        let e = failed.join("; ");
        return json!({"ok": false, "error": if e.is_empty() { "could not write the shortcut".to_string() } else { e }});
    }
    crate::logs::info(format!("headless shortcut(s): {}", made.join(", ")));
    json!({"ok": true, "made": made, "target": ps(&target)})
}

/// Quit, and relaunch after this PID is gone (never start a second copy
/// while this one still holds the port). After Download & install the swap
/// script is already waiting for us, so we only exit.
pub fn restart(quit: impl FnOnce() + Send + 'static) -> Value {
    let Some(exe) = running_exe() else {
        return json!({"ok": false, "error": "running from source - restart it the way you started it"});
    };
    let pending = updater::swap_pending();
    if !pending {
        if let Err(e) = updater::schedule_relaunch(&exe, std::process::id()) {
            return json!({"ok": false, "error": format!("could not schedule relaunch: {e}")});
        }
    }
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(if pending { 300 } else { 800 }));
        crate::logs::info(format!("restarting - {}", if pending { "swap will relaunch" } else { "relaunch after exit" }));
        quit();
    });
    json!({"ok": true, "restarting": true, "exe": ps(&exe), "pending_update": pending})
}

/// Remove the shortcuts, and optionally the installed copy. Never the data.
pub fn uninstall(remove_exe: bool) -> Value {
    let mut gone = Vec::new();
    for link in [start_menu(), desktop_link()] {
        if link.is_file() {
            match std::fs::remove_file(&link) {
                Ok(()) => gone.push(ps(&link)),
                Err(e) => crate::logs::warn(format!("could not remove {}: {e}", link.display())),
            }
        }
    }
    if remove_exe {
        let exe = installed_exe();
        if exe.is_file() {
            if let Err(e) = std::fs::remove_file(&exe) {
                return json!({"ok": false, "removed": gone, "error": format!("could not remove {}: {e} (is it running?)", exe.display())});
            }
            gone.push(ps(&exe));
        }
    }
    json!({"ok": true, "removed": gone, "data_kept": ps(&crate::paths::data())})
}

// ------------------------------------------------- update by download --
// A build run from OUTSIDE the install folder (a fresh download) offers to
// replace an older installed copy. ⚠ Never fires for the installed copy.

pub fn pending_update() -> Value {
    let st = status();
    let iv = installed_version();
    let b = |k: &str| st[k].as_bool().unwrap_or(false);
    let (frozen, installed, running_installed) = (b("frozen"), b("installed"), b("running_installed"));
    json!({
        "offer": frozen && installed && !running_installed && older(&iv, VERSION),
        "offer_install": frozen && !installed && !running_installed,
        "installed": installed,
        "installed_version": iv,
        "this_version": VERSION,
        "install_dir": ps(&install_dir()),
        "install_exe": st["installed_exe"],
        "running_installed": running_installed,
    })
}

/// A running image is opened share-read only, so write access fails.
fn exe_locked(p: &Path) -> bool {
    p.is_file() && std::fs::OpenOptions::new().write(true).open(p).is_err()
}

/// PIDs actually running the INSTALLED exe - not some other ACECM.
pub fn running_installed_pids() -> Vec<u32> {
    let dst = installed_exe();
    let name = dst.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    crate::procs::pids_named(&[&name]).into_iter().filter(|pid| {
        let got = crate::procs::exe_path(*pid);
        !got.is_empty() && same(Path::new(&got), &dst)
    }).collect()
}

/// Helpers (`ACECM.exe --tool ...`) from the installed exe: each holds the
/// image open exactly like the window does.
fn helper_pids() -> Vec<u32> {
    running_installed_pids().into_iter().filter(|p| format!(" {} ", crate::procs::cmdline(*p)).contains(" --tool ")).collect()
}

fn stop_helpers(reason: &str) {
    let killed: Vec<u32> = helper_pids().into_iter().filter(|p| crate::procs::kill(*p)).collect();
    if !killed.is_empty() {
        crate::logs::info(format!("update: stopped {} leftover helper(s) {killed:?} ({reason})", killed.len()));
    }
}

/// Ask a running copy to exit. The UPDATER may only quit the INSTALLED copy;
/// STARTUP TAKEOVER (any_acecm) just wants the port back.
pub fn ask_installed_to_quit(any_acecm: bool) -> bool {
    let port = crate::paths::listen().port;
    let mine = running_installed_pids();
    let owners = crate::procs::tcp_listen_pids(port);
    if !any_acecm && !owners.is_empty() && !mine.is_empty() && !owners.iter().any(|o| mine.contains(o)) {
        crate::logs::info(format!("update: port {port} is held by pid(s) {owners:?}, which are not the installed copy {mine:?} - not sending quit there"));
        return false;
    }
    let base = format!("http://127.0.0.1:{port}");
    let r = reqwest::blocking::Client::builder().no_proxy().timeout(std::time::Duration::from_secs(5)).build()
        .and_then(|c| c.post(format!("{base}/api/app/quit")).header("Content-Type", "application/json")
            .header("Origin", base.as_str()).body("{}").send());
    match r.and_then(|r| r.json::<Value>()) {
        Ok(v) => v["ok"] == json!(true),
        Err(e) => {
            crate::logs::info(format!("quit request to running install: {e}"));
            false
        }
    }
}

fn wait_unlocked(p: &Path, secs: f64) -> bool {
    let end = std::time::Instant::now() + std::time::Duration::from_secs_f64(secs);
    while std::time::Instant::now() < end {
        if !exe_locked(p) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(400));
    }
    !exe_locked(p)
}

fn spawn_detached(exe: &Path, args: &[&str]) -> std::io::Result<()> {
    let mut c = std::process::Command::new(exe);
    c.args(args);
    if let Some(d) = exe.parent() {
        c.current_dir(d);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP: outlive this updater
        c.creation_flags(0x0000_0008 | 0x0000_0200);
    }
    c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    c.spawn().map(|_| ())
}

/// Replace the installed copy with THIS exe, then relaunch it.
pub fn apply_update(relaunch: bool) -> Value {
    let st = status();
    if !updater::frozen() {
        return json!({"ok": false, "error": st.get("note").cloned().unwrap_or(json!("not a frozen build"))});
    }
    let dst = installed_exe();
    crate::logs::info(format!("self-update: v{} -> v{VERSION}  (this exe: {})",
        { let v = installed_version(); if v.is_empty() { "?".into() } else { v } },
        running_exe().map(|p| ps(&p)).unwrap_or_default()));
    if exe_locked(&dst) {
        // the window first (so it cannot respawn its proxy), then helpers
        ask_installed_to_quit(false);
        if !wait_unlocked(&dst, 8.0) {
            stop_helpers("installed exe still locked after quit");
        }
        if !wait_unlocked(&dst, 20.0) {
            let pids = running_installed_pids();
            let who = if pids.is_empty() { String::new() } else {
                format!(" (process {})", pids.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(", "))
            };
            return json!({"ok": false, "running": true, "pids": pids,
                "error": format!("the installed ACECM is still running{who} - close that window, then run this update again")});
        }
    }
    let r = install(st["desktop"].as_bool().unwrap_or(true), true);
    if r["ok"] != json!(true) {
        return r;
    }
    if relaunch {
        if let Err(e) = spawn_detached(&dst, &["--relaunch"]) {
            return json!({"ok": false, "exe": ps(&dst),
                "error": format!("installed v{VERSION} but could not relaunch it: {e}. Start it from the Start Menu.")});
        }
    }
    json!({"ok": true, "exe": ps(&dst), "version": VERSION, "replaced": r["replaced"], "relaunched": relaunch})
}

/// A native yes/no (or OK) box, so a windowless launch can still ask.
pub fn messagebox(text: &str, title: &str, yesno: bool) -> bool {
    #[cfg(windows)]
    {
        let w = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
        let (t, c) = (w(text), w(title));
        // MB_YESNO|MB_OK, MB_ICONQUESTION, MB_SETFOREGROUND, MB_TOPMOST
        let style = (if yesno { 0x4 } else { 0x0 }) | 0x20 | 0x10000 | 0x40000;
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(std::ptr::null_mut(), t.as_ptr(), c.as_ptr(), style) == 6 }
    }
    #[cfg(not(windows))]
    {
        println!("{title}: {text}");
        if !yesno {
            return true;
        }
        let mut s = String::new();
        std::io::stdin().read_line(&mut s).ok();
        s.trim().to_lowercase().starts_with('y')
    }
}

pub fn prompt_update(pu: &Value) -> bool {
    messagebox(&format!("A newer ACECM is ready to install.\n\nInstalled:  v{}\nThis file:   v{}\n\nReplace the installed copy and restart it now?",
        pu["installed_version"].as_str().unwrap_or(""), pu["this_version"].as_str().unwrap_or("")), "ACECM update", true)
}

pub fn prompt_install(pu: &Value) -> bool {
    messagebox(&format!("ACECM v{} is not installed yet.\n\nInstall it to\n{}\nand add Start Menu / Desktop shortcuts?",
        pu["this_version"].as_str().unwrap_or(""), pu["install_dir"].as_str().unwrap_or("")), "Install ACECM", true)
}

// ------------------------------------------------------ temp housekeeping --
// Python onefile builds before 1.2.0 leaked a %TEMP%\_MEInnnnnn folder per
// update (one user had 139 of them, 4.7 GB). The native build has none of its
// own, but an install upgraded from the Python build still carries them.
// ⚠ Only folders carrying one of OUR markers: other PyInstaller apps use the
// same naming and may be running right now.
const OURS_MARKERS: [&str; 2] = ["acecm/web/app.js", "acecm/web/js/15-nav.js"];

fn stale_bundles(older_than: f64) -> Vec<PathBuf> {
    let root = std::env::temp_dir();
    let Ok(rd) = std::fs::read_dir(&root) else { return vec![] };
    let now = std::time::SystemTime::now();
    rd.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("_MEI")).map(|e| e.path())
        .filter(|d| d.is_dir() && OURS_MARKERS.iter().any(|m| d.join(m).is_file()))
        .filter(|d| std::fs::metadata(d).and_then(|m| m.modified()).ok()
            .and_then(|t| now.duration_since(t).ok()).map(|a| a.as_secs_f64() >= older_than).unwrap_or(false))
        .collect()
}

fn dir_size(d: &Path) -> u64 {
    let mut n = 0;
    if let Ok(rd) = std::fs::read_dir(d) {
        for e in rd.flatten() {
            let p = e.path();
            n += if p.is_dir() { dir_size(&p) } else { e.metadata().map(|m| m.len()).unwrap_or(0) };
        }
    }
    n
}

/// Delete what stale_bundles() found. ⚠ RENAME FIRST: Windows refuses to
/// rename a folder while any file inside is open, so the rename IS the
/// in-use test - a delete would gut a live copy file by file instead.
pub fn sweep_bundles() -> Value {
    let (mut freed, mut gone, mut kept) = (0u64, 0, 0);
    for d in stale_bundles(1800.0) {
        let size = dir_size(&d);
        let tmp = PathBuf::from(format!("{}.stale", d.display()));
        if tmp.exists() {
            let _ = std::fs::remove_dir_all(&tmp);
        }
        if std::fs::rename(&d, &tmp).is_err() {
            kept += 1;
            continue;
        }
        match std::fs::remove_dir_all(&tmp) {
            Ok(()) => { freed += size; gone += 1; }
            Err(_) => kept += 1,
        }
    }
    if gone > 0 {
        crate::logs::info(format!("removed {gone} stale bundle folder(s), {:.1} MB freed ({kept} still in use)", freed as f64 / 1e6));
    }
    json!({"removed": gone, "bytes": freed, "in_use": kept})
}
