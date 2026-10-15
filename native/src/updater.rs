//! Build identity and the update check - ported from version.py.
//!
//! Updating a running .exe on Windows: download beside the data folder, then
//! hand off to a small batch script that waits for this process to exit,
//! swaps the files and relaunches. The new exe must write an --okflag file
//! once it is serving; if it does not, the script puts .old back.
//!
//! ⚠ The SHA-256 comes from the SAME GitHub release as the exe, so on its own
//! it only catches a corrupted download. Every release is also signed with an
//! Ed25519 key that never touches GitHub, and an update whose signature does
//! not verify against a pinned key is refused.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const NAME: &str = "Assetto Corsa EVO Content Manager";

/// What this platform's build is called in a GitHub release. Windows keeps
/// the historical name so existing installs keep updating.
pub const ASSET_NAME: &str = if cfg!(windows) { "ACECM.exe" } else { "ACECM" };

/// More than one entry = key rotation: ship the new key alongside the old,
/// sign with the new one, drop the old a few releases later.
const RELEASE_PUBKEYS: &[&str] = &["ScPZslRHuUNMFcqqHc6TG3My2SgS/EsY4THCkv2gs6A="];

/// A release build is the "frozen" one: a debug build runs from a source
/// tree and updates with git. ACECM_FROZEN=1 lets a debug build exercise it.
pub fn frozen() -> bool {
    !cfg!(debug_assertions) || std::env::var("ACECM_FROZEN").map(|v| v == "1").unwrap_or(false)
}

/// What a release signature covers: the version (so an OLD signed build
/// cannot be served as an update) and the asset name (so the Windows and
/// Linux binaries cannot be swapped).
pub fn release_message(asset: &str, ver: &str, sha256: &str) -> Vec<u8> {
    format!("ACECM-release-v1\n{}\n{}\n{}\n", asset.trim().to_lowercase(),
            ver.trim().trim_start_matches(['v', 'V']), sha256.trim().to_lowercase()).into_bytes()
}

/// (ok, reason). Never panics.
pub fn verify_release(asset: &str, ver: &str, sha256: &str, sig_text: &str) -> (bool, String) {
    use base64::Engine;
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let b64 = base64::engine::general_purpose::STANDARD;
    let Ok(sig) = b64.decode(sig_text.trim()) else {
        return (false, "the signature file is not valid base64".into());
    };
    let Ok(sig) = <[u8; 64]>::try_from(sig.as_slice()) else {
        return (false, "no signature for this release".into());
    };
    let sig = Signature::from_bytes(&sig);
    let msg = release_message(asset, ver, sha256);
    for k in RELEASE_PUBKEYS {
        let key = match b64.decode(k).ok().and_then(|b| <[u8; 32]>::try_from(b.as_slice()).ok())
            .and_then(|b| VerifyingKey::from_bytes(&b).ok()) {
            Some(k) => k,
            None => return (false, "bad pinned key".into()),
        };
        if key.verify(&msg, &sig).is_ok() {
            return (true, String::new());
        }
    }
    (false, "the signature does not match ACECM's release key".into())
}

/// Is version a newer than b? Numeric per dotted part, not text.
pub fn newer(a: &str, b: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> {
        v.split('.').map(|c| {
            let d: String = c.chars().filter(|x| x.is_ascii_digit()).collect();
            d.parse().unwrap_or(0)
        }).collect()
    };
    let (mut pa, mut pb) = (parts(a), parts(b));
    let n = pa.len().max(pb.len());
    pa.resize(n, 0);
    pb.resize(n, 0);
    pa > pb
}

fn token() -> String {
    let cfg = crate::config::load();
    let t = crate::config::get_str(&cfg, "update_token");
    let t = if t.trim().is_empty() { std::env::var("ACECM_GITHUB_TOKEN").unwrap_or_default() } else { t };
    t.trim().to_string()
}

/// ⚠ reqwest drops Authorization on a redirect to another host, which the
/// signed release-asset CDN needs ("The specified blob does not exist"
/// otherwise). Both the OS store and the bundled roots are trusted: a
/// corporate proxy's root is only in the former, a never-updated Windows
/// install may lack an intermediate only the latter has.
fn client(timeout: u64) -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(timeout))
        .user_agent(format!("ACECM/{VERSION}"))
        .build()
        .unwrap_or_else(|_| reqwest::blocking::Client::new())
}

fn request(url: &str, accept: &str, timeout: u64) -> reqwest::blocking::RequestBuilder {
    let mut r = client(timeout).get(url).header("Accept", accept);
    let t = token();
    if !t.is_empty() {
        r = r.header("Authorization", format!("Bearer {t}"));
    }
    r
}

enum Fail {
    Http(u16),
    Other(String),
}

fn get_json(url: &str) -> Result<Value, Fail> {
    let r = request(url, "application/vnd.github+json", 20).send().map_err(|e| Fail::Other(format!("URLError: {e}")))?;
    if !r.status().is_success() {
        return Err(Fail::Http(r.status().as_u16()));
    }
    r.json::<Value>().map_err(|e| Fail::Other(format!("JSONDecodeError: {e}")))
}

fn get_text(url: &str) -> Option<String> {
    let r = request(url, "application/octet-stream", 20).send().ok()?;
    if !r.status().is_success() {
        return None;
    }
    r.text().ok()
}

/// Write the first URL that works. 404s are retried: GitHub's CDN lags
/// behind a just-replaced release and answers "blob does not exist".
fn download_file(urls: &[String], dest: &Path) -> Result<String, String> {
    use sha2::Digest;
    use std::io::{Read, Write};
    let mut last = "no download url".to_string();
    for url in urls {
        for attempt in 0..4 {
            let resp = request(url, "application/octet-stream", 300).send();
            let mut r = match resp {
                Ok(r) => r,
                Err(e) => { last = e.to_string(); break; }
            };
            let code = r.status().as_u16();
            if !r.status().is_success() {
                last = format!("HTTP Error {code}");
                if matches!(code, 404 | 502 | 503) && attempt + 1 < 4 {
                    std::thread::sleep(Duration::from_secs_f64(1.5 * (attempt + 1) as f64));
                    continue;
                }
                break;
            }
            let mut f = std::fs::File::create(dest).map_err(|e| e.to_string())?;
            let mut h = sha2::Sha256::new();
            let mut buf = vec![0u8; 1 << 20];
            let res: Result<(), String> = loop {
                match r.read(&mut buf) {
                    Ok(0) => break Ok(()),
                    Ok(n) => {
                        h.update(&buf[..n]);
                        if let Err(e) = f.write_all(&buf[..n]) { break Err(e.to_string()); }
                    }
                    Err(e) => break Err(e.to_string()),
                }
            };
            match res {
                Ok(()) => return Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect()),
                Err(e) => { last = e; break; }
            }
        }
    }
    Err(last)
}

static CHECK: Mutex<Option<(Instant, Value)>> = Mutex::new(None);
const CHECK_TTL: u64 = 30 * 60;

/// Cached: the dashboard asks on every render, and a release does not
/// appear more than once every half hour. The Updater page passes force.
/// A failure is never cached.
pub fn check(force: bool) -> Value {
    if let Some((at, hit)) = CHECK.lock().unwrap().as_ref() {
        if !force && at.elapsed().as_secs() < CHECK_TTL {
            let mut v = hit.clone();
            v["cached"] = json!(true);
            return v;
        }
    }
    let out = check_now();
    if out["ok"] == json!(true) {
        *CHECK.lock().unwrap() = Some((Instant::now(), out.clone()));
    }
    out
}

fn asset_url(a: &Value) -> String {
    a.get("url").and_then(|v| v.as_str()).filter(|s| !s.is_empty())
        .or_else(|| a.get("browser_download_url").and_then(|v| v.as_str())).unwrap_or("").to_string()
}

fn check_now() -> Value {
    let cfg = crate::config::load();
    let repo = crate::config::get_str(&cfg, "update_repo").trim().to_string();
    let url = crate::config::get_str(&cfg, "update_url").trim().to_string();
    if repo.is_empty() && url.is_empty() {
        return json!({"ok": true, "current": VERSION, "checked": false,
                      "hint": "set update_repo (owner/name) in Settings to check GitHub for new builds"});
    }
    let res = (|| -> Result<Value, Fail> {
        if !repo.is_empty() {
            let rel = get_json(&format!("https://api.github.com/repos/{repo}/releases/latest"))?;
            let tag = rel.get("tag_name").and_then(|v| v.as_str()).filter(|s| !s.is_empty())
                .or_else(|| rel.get("name").and_then(|v| v.as_str())).unwrap_or("");
            let latest = tag.trim_start_matches(['v', 'V']).to_string();
            let assets = rel.get("assets").and_then(|v| v.as_array()).cloned().unwrap_or_default();
            // ⚠ per-platform asset: a Linux build matching acecm.exe would
            // replace itself with something it cannot execute
            let want = ASSET_NAME.to_lowercase();
            let named = |n: &str| assets.iter().find(|a| a.get("name").and_then(|v| v.as_str()).unwrap_or("").to_lowercase() == n).cloned();
            let asset = named(&want);
            let mut sha = asset.as_ref().and_then(|a| a.get("digest")).and_then(|v| v.as_str())
                .map(|d| d.rsplit(':').next().unwrap_or("").to_string()).unwrap_or_default();
            // prefer the .sha256 we upload: GitHub's digest is often missing
            if let Some(sa) = named(&format!("{want}.sha256")) {
                let u = asset_url(&sa);
                if !u.is_empty() {
                    if let Some(t) = get_text(&u) {
                        if let Some(h) = t.split_whitespace().next() {
                            if h.len() == 64 {
                                sha = h.to_string();
                            }
                        }
                    }
                }
            }
            let mut sig = String::new();
            if let Some(sa) = named(&format!("{want}.sig")) {
                let u = asset_url(&sa);
                if !u.is_empty() {
                    if let Some(t) = get_text(&u) {
                        sig = t.trim().to_string();
                    }
                }
            }
            // ⚠ prefer the API asset URL: browser_download_url 404s after a
            // tag is deleted and recreated
            let api_url = asset.as_ref().and_then(|a| a.get("url")).and_then(|v| v.as_str()).unwrap_or("").to_string();
            let cdn_url = asset.as_ref().and_then(|a| a.get("browser_download_url")).and_then(|v| v.as_str()).unwrap_or("").to_string();
            let notes: String = rel.get("body").and_then(|v| v.as_str()).unwrap_or("").chars().take(4000).collect();
            return Ok(json!({"ok": true, "current": VERSION, "checked": true,
                "source": format!("github:{repo}"),
                "latest": latest, "available": newer(&latest, VERSION),
                "notes": notes,
                "published": rel.get("published_at").cloned().unwrap_or(Value::Null),
                "url": if api_url.is_empty() { cdn_url.clone() } else { api_url },
                "browser_url": cdn_url,
                "sha256": sha,
                "sig": sig,
                "error": if asset.is_some() { Value::Null } else { json!("that release has no ACECM.exe asset attached") }}));
        }
        let man = get_json(&url)?;
        let s = |k: &str| man.get(k).map(|v| match v { Value::String(s) => s.clone(), Value::Null => String::new(), o => o.to_string() }).unwrap_or_default();
        let latest = s("version");
        Ok(json!({"ok": true, "current": VERSION, "checked": true, "source": url,
                  "latest": latest, "available": newer(&latest, VERSION),
                  "notes": man.get("notes").cloned().unwrap_or(json!("")), "url": man.get("url").cloned().unwrap_or(json!("")),
                  "sha256": man.get("sha256").cloned().unwrap_or(json!("")), "sig": man.get("sig").cloned().unwrap_or(json!(""))}))
    })();
    match res {
        Ok(v) => v,
        Err(Fail::Http(code)) => {
            let hint = match code {
                404 => "either that repo has no releases yet, or it is PRIVATE - a private repo returns 404 to an unauthenticated check. Make it public, or set update_token to a GitHub token with 'repo' scope.",
                401 | 403 => "the token was rejected, or the API rate limit was hit.",
                _ => "",
            };
            json!({"ok": false, "current": VERSION, "repo": repo, "error": format!("HTTP {code}"), "hint": hint})
        }
        Err(Fail::Other(e)) => json!({"ok": false, "current": VERSION, "error": e}),
    }
}

// ------------------------------------------------------------- handshake --
pub fn rollback_flag_path() -> PathBuf {
    crate::paths::data().join("update-rollback.flag")
}

pub fn pending_path() -> PathBuf {
    crate::paths::data().join("update-pending.flag")
}

/// True when a downloaded exe is waiting for this process to exit.
pub fn swap_pending() -> bool {
    pending_path().is_file()
}

static ROLLBACK: Mutex<Option<Value>> = Mutex::new(None);

/// Set on this process if the previous swap had to put .old back.
pub fn last_rollback() -> Option<Value> {
    ROLLBACK.lock().unwrap().clone()
}

/// If the last update was rolled back, remember it for the dashboard.
pub fn consume_rollback() {
    let p = rollback_flag_path();
    if !p.is_file() {
        return;
    }
    let _ = std::fs::remove_file(&p);
    *ROLLBACK.lock().unwrap() = Some(json!({
        "what": "The last update did not start, so the previous build was restored",
        "do": "You can try updating again. The broken download was discarded.",
    }));
    crate::logs::warn("previous update was rolled back — restored the previous exe");
}

/// The new exe is serving. Tell the swap script it can keep us.
pub fn confirm_update(okflag: Option<&str>) {
    let Some(f) = okflag.filter(|s| !s.is_empty()) else { return };
    if let Some(d) = Path::new(f).parent() {
        let _ = std::fs::create_dir_all(d);
    }
    match std::fs::write(f, "ok\n") {
        Ok(()) => crate::logs::info(format!("update handshake: wrote {f}")),
        Err(e) => crate::logs::warn(format!("update handshake could not write {f}: {e}")),
    }
}

// ----------------------------------------------------------------- swap --
fn launch_mode_flags() -> String {
    // ⚠ keep the launch MODE across an update: a headless server that came
    // back in window mode went dark for its remote operator
    let argv: Vec<String> = std::env::args().collect();
    ["--headless", "--no-ui", "--browser"].iter().filter(|f| argv.iter().any(|a| a == *f))
        .map(|f| format!(" {f}")).collect()
}

/// The script that replaces a running exe once it exits. Split out so it
/// can be tested without downloading anything.
pub fn write_swap_script(exe: &Path, new: &Path, pid: u32, bat: Option<PathBuf>, blog: Option<PathBuf>,
                         relaunch: bool, ver: Option<&str>, okflag: Option<String>) -> std::io::Result<(PathBuf, PathBuf)> {
    let data = crate::paths::data();
    let bat = bat.unwrap_or_else(|| data.join("_update.bat"));
    let blog = blog.unwrap_or_else(|| data.join("_update.log"));
    let exe_s = exe.to_string_lossy().to_string();
    let new_s = new.to_string_lossy().to_string();
    let inst = exe.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    let marker = format!("{inst}\\version.txt");
    let prev_marker = format!("{marker}.prev");
    let rolled = rollback_flag_path().to_string_lossy().to_string();
    let okflag = if relaunch && okflag.is_none() {
        let mut b = [0u8; 16];
        let _ = getrandom::fill(&mut b);
        let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
        Some(std::env::temp_dir().join(format!("acecm-upok-{hex}.flag")).to_string_lossy().to_string())
    } else { okflag };
    let image = exe.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "ACECM.exe".into());
    let stem = exe.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "ACECM".into());
    let mut args = String::new();
    if relaunch {
        if let Some(v) = ver {
            args += &format!(" --updated {v}");
        }
        if let Some(o) = &okflag {
            args += &format!(" --okflag \"{o}\"");
        }
        args += &launch_mode_flags();
    }
    let mut l: Vec<String> = vec![
        "@echo off".into(),
        // ⚠ delayed expansion: %VAR% inside a block is expanded at PARSE time
        "setlocal enabledelayedexpansion".into(),
        format!("set LOG=\"{}\"", blog.to_string_lossy()),
        format!("echo [%date% %time%] waiting for pid {pid} > %LOG%"),
        "rem Wait for ACECM to exit - Windows will not let a running exe be".into(),
        "rem replaced. Use ping, not timeout: timeout needs a console.".into(),
        ":wait".into(),
        format!("tasklist /fi \"PID eq {pid}\" 2>nul | find \"{pid}\" >nul"),
        "if not errorlevel 1 (ping -n 2 127.0.0.1 >nul & goto wait)".into(),
        // ⚠ helpers run from the same exe keep its image locked; match on the
        // PATH so an unrelated ACECM elsewhere is not collateral
        "rem clear helpers still running the exe we are replacing".into(),
        format!("powershell -NoProfile -NonInteractive -Command \"Get-Process -Name {stem} -ErrorAction SilentlyContinue | Where-Object {{ $_.Path -eq '{}' }} | Stop-Process -Force -ErrorAction SilentlyContinue\" >>%LOG% 2>&1",
                exe_s.replace('\'', "''")),
        "rem the file can stay locked for a moment after the process goes".into(),
        "ping -n 4 127.0.0.1 >nul".into(),
        "set TRIES=0".into(),
        ":swap".into(),
        format!("move /y \"{exe_s}\" \"{exe_s}.old\" >>%LOG% 2>&1"),
        "if errorlevel 1 (".into(),
        "  set /a TRIES+=1".into(),
        "  if !TRIES! lss 20 (ping -n 2 127.0.0.1 >nul & goto swap)".into(),
        "  echo could not replace the exe - it is still locked >>%LOG%".into(),
        "  goto giveup".into(),
        ")".into(),
        format!("move /y \"{new_s}\" \"{exe_s}\" >>%LOG% 2>&1"),
        "if errorlevel 1 (".into(),
        "  echo could not move the new exe into place - restoring >>%LOG%".into(),
        format!("  copy /y \"{exe_s}.old\" \"{exe_s}\" >>%LOG% 2>&1"),
        "  goto giveup".into(),
        ")".into(),
        "echo swapped ok >>%LOG%".into(),
        format!("if exist \"{marker}\" copy /y \"{marker}\" \"{prev_marker}\" >>%LOG% 2>&1"),
    ];
    if let Some(v) = ver {
        l.push(format!("echo {v}> \"{marker}\""));
    }
    if relaunch {
        l.push(format!("start \"ACECM\" /d \"{inst}\" \"{exe_s}\"{args} --relaunch"));
        l.push("set W=0".into());
        l.push(":waitok".into());
        l.push("ping -n 3 127.0.0.1 >nul".into());
        l.push(match &okflag { Some(o) => format!("if exist \"{o}\" goto upok"), None => "goto upok".into() });
        l.push("set /a W+=1".into());
        l.push("if !W! lss 20 goto waitok".into());
        // a live exe after the swap is success even if the flag was lost
        l.push(format!("tasklist /fi \"IMAGENAME eq {image}\" 2>nul | find /i \"{image}\" >nul"));
        l.push("if not errorlevel 1 (".into());
        l.push("  echo new exe is running without a flag - keeping it >>%LOG%".into());
        l.push("  goto upok".into());
        l.push(")".into());
        l.push("echo new exe did not start - rolling back >>%LOG%".into());
        l.push(":rollback".into());
        l.push(format!("copy /y \"{exe_s}.old\" \"{exe_s}\" >>%LOG% 2>&1"));
        l.push(format!("if exist \"{prev_marker}\" copy /y \"{prev_marker}\" \"{marker}\" >>%LOG% 2>&1"));
        l.push(format!("echo rollback> \"{rolled}\""));
        l.push(format!("start \"ACECM\" /d \"{inst}\" \"{exe_s}\" --relaunch"));
        l.push("goto done".into());
        l.push(":upok".into());
        if let Some(o) = &okflag {
            l.push(format!("del \"{o}\" >nul 2>&1"));
        }
        l.push(format!("del \"{}\" >nul 2>&1", pending_path().to_string_lossy()));
        l.push("echo handshake ok >>%LOG%".into());
        l.push("goto done".into());
    }
    l.push(":giveup".into());
    l.push("echo giveup >>%LOG%".into());
    if relaunch {
        l.push(format!("start \"\" /d \"{inst}\" \"{exe_s}\""));
    }
    l.push(":done".into());
    l.push("del \"%~f0\"".into());
    std::fs::create_dir_all(&data)?;
    let mut text = l.join("\r\n");
    text.push_str("\r\n");
    std::fs::write(&bat, text)?;
    Ok((bat, blog))
}

/// Run a batch file windowless, in its own process group, so it outlives us.
/// ⚠ CREATE_NO_WINDOW, not DETACHED_PROCESS: detached leaves the batch with
/// no console and its redirections fail outright.
pub fn run_batch(bat: &Path) -> std::io::Result<()> {
    let mut c = std::process::Command::new("cmd");
    c.arg("/c").arg(bat).current_dir(crate::paths::data());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000 | 0x0000_0200);
    }
    c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    c.spawn().map(|_| ())
}

/// Start ACECM again only after this PID is gone - a second copy started
/// first fights for the port and dies.
pub fn schedule_relaunch(exe: &Path, pid: u32) -> std::io::Result<PathBuf> {
    #[cfg(not(windows))]
    {
        let script = format!("while kill -0 {pid} 2>/dev/null; do sleep 0.5; done; sleep 0.5; exec \"{}\" --relaunch", exe.display());
        std::process::Command::new("/bin/sh").arg("-c").arg(script)
            .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
            .spawn()?;
        return Ok(PathBuf::new());
    }
    #[cfg(windows)]
    {
        let data = crate::paths::data();
        std::fs::create_dir_all(&data)?;
        let bat = data.join("_relaunch.bat");
        let inst = exe.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
        let lines = [
            "@echo off".to_string(),
            ":wait".into(),
            format!("tasklist /fi \"PID eq {pid}\" 2>nul | find \"{pid}\" >nul"),
            "if not errorlevel 1 (ping -n 2 127.0.0.1 >nul & goto wait)".into(),
            "ping -n 2 127.0.0.1 >nul".into(),
            // --relaunch: the new copy waits for the port instead of deferring
            format!("start \"ACECM\" /d \"{inst}\" \"{}\" --relaunch{}", exe.display(), launch_mode_flags()),
            "del \"%~f0\"".into(),
        ];
        std::fs::write(&bat, lines.join("\r\n") + "\r\n")?;
        run_batch(&bat)?;
        Ok(bat)
    }
}

/// Replace the running executable now and restart it (Linux: renaming a
/// running binary is legal, so there is nothing to wait for).
#[cfg(not(windows))]
fn linux_swap(exe: &Path, new: &Path, ver: &str) -> Result<PathBuf, String> {
    use std::os::unix::fs::PermissionsExt;
    let old = PathBuf::from(format!("{}.old", exe.display()));
    let staged = PathBuf::from(format!("{}.new", exe.display()));
    std::fs::copy(new, &staged).map_err(|e| e.to_string())?;
    let mode = std::fs::metadata(exe).map(|m| m.permissions().mode()).unwrap_or(0o755) | 0o111;
    let _ = std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(mode));
    if let Err(e) = std::fs::rename(exe, &old) {
        let _ = std::fs::remove_file(&staged);
        return Err(format!("could not move the old build aside: {e}"));
    }
    if let Err(e) = std::fs::rename(&staged, exe) {
        let _ = std::fs::rename(&old, exe);
        return Err(format!("could not move the new build into place: {e}"));
    }
    let _ = std::fs::remove_file(new);
    crate::logs::info(format!("update: swapped in {ver} (previous build kept as {})", old.file_name().unwrap_or_default().to_string_lossy()));
    let _ = schedule_relaunch(exe, std::process::id());
    Ok(old)
}

/// Download a new build and schedule the swap on exit.
pub fn apply() -> Value {
    if !frozen() {
        return json!({"ok": false, "error": "running from source - update with git, not this"});
    }
    let info = check(false);
    // ⚠ never take url/sha256 from the caller: anyone who could reach the API
    // could feed a binary of their own
    let s = |k: &str| info.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let sha256 = s("sha256");
    let mut urls: Vec<String> = Vec::new();
    for u in [s("url"), s("browser_url")] {
        if !u.is_empty() && !urls.contains(&u) {
            urls.push(u);
        }
    }
    if urls.is_empty() {
        return json!({"ok": false, "error": "no download url"});
    }
    if sha256.len() != 64 {
        return json!({"ok": false, "error": "that release has no checksum — refusing to install"});
    }
    let latest = s("latest");
    let Ok(exe) = std::env::current_exe() else { return json!({"ok": false, "error": "cannot find the running exe"}) };
    // ⚠ not beside the running exe: a previous failed swap can leave that
    // file locked
    let data = crate::paths::data();
    let _ = std::fs::create_dir_all(&data);
    let mut new = data.join("ACECM-update.new");
    if new.is_file() && std::fs::remove_file(&new).is_err() {
        let mut b = [0u8; 4];
        let _ = getrandom::fill(&mut b);
        new = data.join(format!("ACECM-update-{}.new", b.iter().map(|x| format!("{x:02x}")).collect::<String>()));
    }
    let got = match download_file(&urls, &new) {
        Ok(h) => h,
        Err(e) => {
            if e.contains("404") || e.to_lowercase().contains("blob does not exist") {
                return json!({"ok": false, "error": "download failed: GitHub is still publishing the file (404). Wait a minute and try again, or close ACECM and run the new ACECM.exe from the GitHub release."});
            }
            return json!({"ok": false, "error": format!("download failed: {e}")});
        }
    };
    // ⚠ verify BEFORE scheduling the swap
    if got.to_lowercase() != sha256.to_lowercase() {
        let _ = std::fs::remove_file(&new);
        return json!({"ok": false, "error": format!("checksum mismatch (got {}, expected {})", &got[..12], &sha256[..12])});
    }
    // ⚠ AND the signature, over the hash actually downloaded
    let (ok_sig, why) = verify_release(ASSET_NAME, &latest, &got, &s("sig"));
    if !ok_sig {
        let _ = std::fs::remove_file(&new);
        crate::logs::warn(format!("update to {latest} refused: {why}"));
        return json!({"ok": false, "error": format!("refusing the update: {why}. Download it from the GitHub release page yourself if you trust it.")});
    }
    let ver = if latest.is_empty() { VERSION.to_string() } else { latest.clone() };
    #[cfg(not(windows))]
    {
        let old = match linux_swap(&exe, &new, &ver) {
            Ok(o) => o,
            Err(e) => return json!({"ok": false, "error": format!("update failed: {e}")}),
        };
        let _ = std::fs::write(pending_path(), &ver);
        return json!({"ok": true, "version": latest,
            "note": format!("installed and verified; close ACECM to finish - it will relaunch itself. The previous build is kept as {}.", old.file_name().unwrap_or_default().to_string_lossy())});
    }
    #[cfg(windows)]
    {
        let (bat, blog) = match write_swap_script(&exe, &new, std::process::id(), None, None, true, Some(&ver), None) {
            Ok(x) => x,
            Err(e) => return json!({"ok": false, "error": format!("could not write the swap script: {e}")}),
        };
        let _ = std::fs::write(pending_path(), &ver);
        if let Err(e) = run_batch(&bat) {
            return json!({"ok": false, "error": format!("could not start the swap: {e}")});
        }
        crate::logs::info(format!("update to {latest} downloaded and verified; swap scheduled ({})", blog.display()));
        json!({"ok": true, "version": latest, "log": blog.to_string_lossy(),
               "note": "downloaded and verified; close ACECM to finish the update - it will relaunch itself. If the new build does not start, the previous one is put back."})
    }
}
