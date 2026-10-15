//! The lobby backend: our TLS keypair, the two client-exe patches (lobby URL
//! slot, UI inspector), and the proxy process - ported from acecm/backend.py.
//! The proxy itself is still tools/acevo_proxy.py, run as a child; the
//! shell OWNS that child (the backend asks here via the bridge).
//!
//! ⚠ The keypair is generated on this machine and never shipped: the proxy
//! terminates the client's TLS, so a shared key would protect nothing.
//! ⚠ Patches follow one rule: find the site by its bytes, verify them, back
//! up (only while the backup is the SAME build - an old backup restored over
//! an updated exe once silently downgraded the game), and write nothing
//! unless the expected bytes are there.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::{config, detect, lobby, misc, paths, procs, profiles, protos, sidecar};

pub const KUNOS_CLIENT_URL: &[u8] = b"wss://b.gk.sd:6990/9kB3F1CAx7mQ2zL5vN8pD4sT6yW0eU1r/000500";

fn backend_port() -> u16 {
    config::load().get("backend_port").and_then(|v| v.as_u64()).unwrap_or(448) as u16
}

pub fn certs_dir() -> PathBuf {
    let d = paths::data().join("certs");
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn cert_paths() -> (PathBuf, PathBuf) {
    (certs_dir().join("cert.pem"), certs_dir().join("key.pem"))
}

fn wanted_san() -> (Vec<String>, Vec<String>, String) {
    let lan = misc::lan_ipv4();
    let mut ips = vec!["127.0.0.1".to_string()];
    if !lan.is_empty() {
        ips.push(lan.clone());
    }
    (vec!["localhost".into(), "b.gk.sd".into()], ips, lan)
}

fn cert_sans(cert: &Path) -> (Vec<String>, Vec<String>) {
    let Ok(pem) = std::fs::read(cert) else { return (vec![], vec![]) };
    let Ok((_, p)) = x509_parser::pem::parse_x509_pem(&pem) else { return (vec![], vec![]) };
    let Ok(crt) = p.parse_x509() else { return (vec![], vec![]) };
    let (mut names, mut ips) = (vec![], vec![]);
    if let Ok(Some(san)) = crt.subject_alternative_name() {
        for n in &san.value.general_names {
            match n {
                x509_parser::extensions::GeneralName::DNSName(d) => names.push(d.to_string()),
                x509_parser::extensions::GeneralName::IPAddress(b) => {
                    if b.len() == 4 {
                        ips.push(format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3]));
                    } else if b.len() == 16 {
                        let a: [u8; 16] = (*b).try_into().unwrap();
                        ips.push(std::net::Ipv6Addr::from(a).to_string());
                    }
                }
                _ => {}
            }
        }
    }
    (names, ips)
}

pub fn cert_covers() -> bool {
    let (cert, key) = cert_paths();
    if !cert.exists() || !key.exists() {
        return false;
    }
    let (hn, hi) = cert_sans(&cert);
    let (wn, wi, _) = wanted_san();
    wn.iter().all(|n| hn.contains(n)) && wi.iter().all(|i| hi.contains(i))
}

/// A self-signed keypair covering localhost, b.gk.sd and the LAN address
/// (regenerated when the LAN address moved, or a guest rejects the cert).
pub fn ensure_cert(force: bool) -> Value {
    let (cert, key) = cert_paths();
    if !force && cert_covers() {
        return json!({"ok": true, "existing": true, "cert": cert.to_string_lossy(), "lan_ip": misc::lan_ipv4()});
    }
    match make_cert(&cert, &key) {
        Ok((names, ips, lan)) => json!({"ok": true, "created": true, "cert": cert.to_string_lossy(),
                                        "lan_ip": lan, "san": {"dns": names, "ips": ips}}),
        Err(e) => json!({"ok": false, "error": e}),
    }
}

fn make_cert(cert: &Path, key: &Path) -> Result<(Vec<String>, Vec<String>, String), String> {
    use rsa::pkcs1::EncodeRsaPrivateKey;
    use rsa::pkcs8::EncodePrivateKey;
    let (names, ips, lan) = wanted_san();
    let mut rng = rand::thread_rng();
    let k = rsa::RsaPrivateKey::new(&mut rng, 2048).map_err(|e| e.to_string())?;
    let pk8 = k.to_pkcs8_pem(rsa::pkcs8::LineEnding::LF).map_err(|e| e.to_string())?;
    let kp = rcgen::KeyPair::from_pkcs8_pem_and_sign_algo(&pk8, &rcgen::PKCS_RSA_SHA256).map_err(|e| e.to_string())?;
    let mut params = rcgen::CertificateParams::new(names.clone()).map_err(|e| e.to_string())?;
    for ip in &ips {
        if let Ok(a) = ip.parse::<std::net::IpAddr>() {
            params.subject_alt_names.push(rcgen::SanType::IpAddress(a));
        }
    }
    let mut dn = rcgen::DistinguishedName::new();
    dn.push(rcgen::DnType::CommonName, "localhost");
    params.distinguished_name = dn;
    let now = time::OffsetDateTime::now_utc();
    params.not_before = now - time::Duration::days(1);
    params.not_after = now + time::Duration::days(3650);
    let mut serial = [0u8; 16];
    let _ = getrandom::fill(&mut serial);
    serial[0] &= 0x7f;
    params.serial_number = Some(rcgen::SerialNumber::from_slice(&serial));
    let c = params.self_signed(&kp).map_err(|e| e.to_string())?;
    std::fs::write(cert, c.pem()).map_err(|e| e.to_string())?;
    // the same encoding the backend wrote (TraditionalOpenSSL / PKCS#1)
    let pk1 = k.to_pkcs1_pem(rsa::pkcs1::LineEnding::LF).map_err(|e| e.to_string())?;
    std::fs::write(key, pk1.as_bytes()).map_err(|e| e.to_string())?;
    Ok((names, ips, lan))
}

// ------------------------------------------------------------ exe patches --
pub fn game_exe() -> String {
    let cfg = config::load();
    let e = config::get_str(&cfg, "game_exe");
    if !e.is_empty() && Path::new(&e).is_file() {
        return e;
    }
    detect::find("game_exe", false)
}

pub fn redirect_url() -> String {
    format!("wss://127.0.0.1:{}/communicationNode/dev", backend_port())
}

fn local_markers() -> Vec<Vec<u8>> {
    let p = backend_port();
    vec![redirect_url().into_bytes(), format!("wss://localhost:{p}/communicationNode/dev").into_bytes(),
         b"wss://localhost:448/communicationNode/dev".to_vec(), b"wss://127.0.0.1:448/communicationNode/dev".to_vec()]
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    memchr::memmem::find(hay, needle)
}

/// (offset, kind) for the 58-byte client URL slot.
fn find_url_slot(data: &[u8]) -> Option<(usize, &'static str)> {
    if let Some(i) = find(data, KUNOS_CLIENT_URL) {
        return Some((i, "kunos"));
    }
    local_markers().iter().find_map(|m| find(data, m)).map(|i| (i, "local"))
}

/// Keep a backup only while it belongs to the CURRENT game build (a patch
/// flips a few bytes, so the same build is the same size).
fn refresh_backup(exe: &Path, bak: &Path, len: u64) -> Value {
    if let Ok(m) = std::fs::metadata(bak) {
        if m.len() == len {
            return json!({"kept": true});
        }
        let stale = PathBuf::from(format!("{}.stale", bak.display()));
        if std::fs::rename(bak, &stale).is_err() {
            let _ = std::fs::remove_file(bak);
        }
    }
    match std::fs::copy(exe, bak) {
        Ok(_) => json!({"made": true}),
        Err(e) => json!({"error": e.to_string()}),
    }
}

static PROBE: Mutex<Option<(String, Value)>> = Mutex::new(None);
static INSP: Mutex<Option<(String, Value)>> = Mutex::new(None);

fn exe_key(exe: &str) -> Option<String> {
    let m = std::fs::metadata(exe).ok()?;
    let ns = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
    Some(format!("{exe}|{ns}|{}", m.len()))
}

/// Read the client's rdata slot (do not trust the .bak_prebackend file).
pub fn probe_client_url() -> Value {
    let exe = game_exe();
    if exe.is_empty() {
        return json!({"rdata_patched": null, "error": "game exe not found"});
    }
    let Some(key) = exe_key(&exe) else { return json!({"rdata_patched": null, "error": "cannot stat the game exe"}) };
    if let Some((k, v)) = PROBE.lock().unwrap().as_ref() {
        if *k == key {
            return v.clone();
        }
    }
    let Ok(data) = std::fs::read(&exe) else { return json!({"rdata_patched": null, "error": "cannot read the game exe"}) };
    let slot = find_url_slot(&data);
    let info = json!({
        "exe": exe,
        "rdata_patched": slot.map(|(_, k)| k == "local"),
        "kunos_url_present": slot.map(|(_, k)| k == "kunos").unwrap_or(false),
        "slot": slot.map(|(o, _)| format!("{o:#x}")),
        "slot_kind": slot.map(|(_, k)| k),
        "intended": redirect_url(),
        "size": data.len(),
    });
    *PROBE.lock().unwrap() = Some((key, info.clone()));
    info
}

/// Overwrite the Kunos client URL slot with our local backend URL.
pub fn apply_redirect() -> Value {
    let exe = game_exe();
    if exe.is_empty() {
        return json!({"ok": false, "error": "game exe not found"});
    }
    if procs::game_running() {
        return json!({"ok": false, "error": "the game is running — close it before patching"});
    }
    let new = redirect_url().into_bytes();
    let slot = KUNOS_CLIENT_URL.len();
    if new.len() > slot {
        return json!({"ok": false, "error": format!("URL too long ({} > {slot})", new.len())});
    }
    let Ok(mut data) = std::fs::read(&exe) else { return json!({"ok": false, "error": "could not read the client"}) };
    let Some((off, kind)) = find_url_slot(&data) else {
        return json!({"ok": false, "error": "neither the Kunos URL nor a local backend URL was found — the game may have updated"});
    };
    let mut want = new.clone();
    want.resize(slot, 0);
    if data[off..off + slot] == want[..] {
        *PROBE.lock().unwrap() = None;
        return json!({"ok": true, "already": true, "off": format!("{off:#x}"), "url": redirect_url()});
    }
    if kind == "kunos" && &data[off..off + slot] != KUNOS_CLIENT_URL {
        return json!({"ok": false, "error": "Kunos URL site is not intact — refusing"});
    }
    let bak = PathBuf::from(format!("{exe}.bak_prebackend"));
    let _ = refresh_backup(Path::new(&exe), &bak, data.len() as u64);
    data[off..off + slot].copy_from_slice(&want);
    if let Err(e) = std::fs::write(&exe, &data) {
        *PROBE.lock().unwrap() = None;
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            return json!({"ok": false, "needs_admin": true, "error": format!("no permission to modify {exe}. Run ACECM as administrator once — Steam drops -backend=, so this rewrite is what actually points the client at us")});
        }
        return json!({"ok": false, "error": format!("could not write the client: {e}")});
    }
    *PROBE.lock().unwrap() = None;
    json!({"ok": true, "off": format!("{off:#x}"), "was": kind, "url": redirect_url(), "backup": bak.to_string_lossy()})
}

/// Put the official Kunos client URL back.
pub fn restore_redirect() -> Value {
    let exe = game_exe();
    if exe.is_empty() {
        return json!({"ok": false, "error": "game exe not found"});
    }
    if procs::game_running() {
        return json!({"ok": false, "error": "the game is running — close it before restoring"});
    }
    let Ok(mut data) = std::fs::read(&exe) else { return json!({"ok": false, "error": "could not read the client"}) };
    if find(&data, KUNOS_CLIENT_URL).is_some() {
        *PROBE.lock().unwrap() = None;
        return json!({"ok": true, "already": true, "url": String::from_utf8_lossy(KUNOS_CLIENT_URL)});
    }
    let Some((off, _)) = find_url_slot(&data) else { return json!({"ok": false, "error": "no local backend URL found to restore"}) };
    data[off..off + KUNOS_CLIENT_URL.len()].copy_from_slice(KUNOS_CLIENT_URL);
    if let Err(e) = std::fs::write(&exe, &data) {
        return json!({"ok": false, "error": e.to_string()});
    }
    *PROBE.lock().unwrap() = None;
    json!({"ok": true, "off": format!("{off:#x}"), "url": String::from_utf8_lossy(KUNOS_CLIENT_URL)})
}

pub fn inspector_port() -> u32 {
    std::env::var("INSPECTOR_PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(9444)
}

/// The inspector site by its CODE SHAPE (the address moves every build):
///   mov dword [rbp+X], <port>   c7 45 XX <4 bytes>      unpatched: ff ff ff ff
///   mov word  [rbp+Y], 0x01ZZ   66 c7 45 YY ZZ 01       unpatched ZZ=00, patched 01
/// Exactly one match of either shape, or nothing (ambiguous = refuse).
fn find_inspector(data: &[u8]) -> Option<(usize, usize, bool)> {
    let port = inspector_port().to_le_bytes();
    for (want_port, low, patched) in [([0xffu8; 4], 0x00u8, false), (port, 0x01u8, true)] {
        let mut hits = vec![];
        if data.len() >= 13 {
            for i in 0..=data.len() - 13 {
                if data[i] == 0xc7 && data[i + 1] == 0x45 && data[i + 3..i + 7] == want_port
                    && data[i + 7] == 0x66 && data[i + 8] == 0xc7 && data[i + 9] == 0x45
                    && data[i + 11] == low && data[i + 12] == 0x01
                {
                    hits.push(i);
                    if hits.len() > 1 {
                        break;
                    }
                }
            }
        }
        if hits.len() == 1 {
            return Some((hits[0], hits[0] + 7, patched));
        }
    }
    None
}

pub fn probe_inspector() -> Value {
    let exe = game_exe();
    if exe.is_empty() {
        return json!({"inspector_patched": null, "error": "game exe not found"});
    }
    let Some(key) = exe_key(&exe).map(|k| format!("{k}|{}", inspector_port())) else {
        return json!({"inspector_patched": null, "error": "cannot stat the game exe"});
    };
    if let Some((k, v)) = INSP.lock().unwrap().as_ref() {
        if *k == key {
            return v.clone();
        }
    }
    let info = match std::fs::read(&exe) {
        Err(e) => json!({"exe": exe, "inspector_patched": null, "error": e.to_string()}),
        Ok(data) => match find_inspector(&data) {
            None => json!({"exe": exe, "inspector_patched": null,
                           "error": "inspector code pattern not found (or ambiguous) - the game may have changed too much to auto-locate"}),
            Some((pf, gf, patched)) => {
                let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
                json!({"exe": exe, "inspector_patched": patched, "port_site": format!("{pf:#x}"),
                       "flag_site": format!("{gf:#x}"), "port_bytes": hex(&data[pf..pf + 7]),
                       "flag_bytes": hex(&data[gf..gf + 6]), "port": inspector_port(), "known": true})
            }
        },
    };
    *INSP.lock().unwrap() = Some((key, info.clone()));
    info
}

/// Turn on Gameface DevTools on :9444 so Drive can press Start/Join.
pub fn apply_inspector() -> Value {
    let exe = game_exe();
    if exe.is_empty() {
        return json!({"ok": false, "error": "game exe not found"});
    }
    let port = inspector_port();
    let Ok(mut data) = std::fs::read(&exe) else { return json!({"ok": false, "error": "could not read the client"}) };
    let Some((pf, gf, patched)) = find_inspector(&data) else {
        return json!({"ok": false, "error": "inspector code pattern not found (or ambiguous) - Drive cannot enable the menu inspector until this is re-derived for this game build"});
    };
    if patched {
        *INSP.lock().unwrap() = None;
        return json!({"ok": true, "already": true, "port": port});
    }
    if procs::game_running() {
        return json!({"ok": false, "error": "the game is running — close it before enabling the menu inspector"});
    }
    let bak = PathBuf::from(format!("{exe}.bak_preinspector"));
    let _ = refresh_backup(Path::new(&exe), &bak, data.len() as u64);
    data[pf + 3..pf + 7].copy_from_slice(&port.to_le_bytes());
    data[gf + 4] = 0x01;
    data[gf + 5] = 0x01;
    if let Err(e) = std::fs::write(&exe, &data) {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            return json!({"ok": false, "needs_admin": true, "error": format!("no permission to modify {exe}. Run ACECM as administrator once — Drive talks to the menu on :{port}, and stock EVO leaves that inspector off")});
        }
        return json!({"ok": false, "error": format!("could not write the client: {e}")});
    }
    *INSP.lock().unwrap() = None;
    json!({"ok": true, "port": port, "backup": bak.to_string_lossy(), "off": format!("{pf:#x}")})
}

// ------------------------------------------------------------ the proxy --
struct Child {
    child: std::process::Child,
}

static PROCS: Mutex<Option<HashMap<String, Child>>> = Mutex::new(None);
pub const CONTROL: &str = "http://127.0.0.1:8093";
const MODES: &[(&str, &str)] = &[("proxy", "acevo_proxy"), ("standalone", "acevo_backend")];

fn tool_name(mode: &str) -> Option<&'static str> {
    MODES.iter().find(|(m, _)| *m == mode).map(|(_, t)| *t)
}

fn log_path(mode: &str) -> PathBuf {
    paths::data().join(format!("backend_{mode}.log"))
}

pub fn log_tail_text(mode: &str) -> String {
    std::fs::read(log_path(mode)).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default()
}

pub fn log(mode: &str, lines: usize) -> Value {
    let p = log_path(mode);
    if !p.exists() {
        return json!({"lines": []});
    }
    let t = log_tail_text(mode);
    let all: Vec<&str> = t.lines().collect();
    json!({"lines": all[all.len().saturating_sub(lines)..]})
}

fn is_descendant(pid: u32, ancestor: u32) -> bool {
    let mut seen = pid;
    for _ in 0..6 {
        if seen == ancestor {
            return true;
        }
        seen = procs::ppid(seen);
        if seen == 0 || seen == 4 {
            return false;
        }
    }
    false
}

/// An ACECM backend from a PREVIOUS session still holding the port,
/// identified by COMMAND LINE (the exe may be "ACECM(3).exe").
fn orphan_on_port() -> Option<u32> {
    let me = std::process::id();
    procs::tcp_listen_pids(backend_port()).into_iter().filter(|p| *p != me)
        .find(|p| { let c = procs::cmdline(*p); MODES.iter().any(|(_, t)| c.contains(t)) })
}

pub fn stop() -> Value {
    if let Some(m) = PROCS.lock().unwrap().as_mut() {
        for (_, mut c) in m.drain() {
            let _ = procs::kill(c.child.id());
            let _ = c.child.wait();
        }
    }
    if let Some(pid) = orphan_on_port() {
        procs::kill(pid);
    }
    json!({"ok": true})
}

fn running() -> Value {
    let mut out = serde_json::Map::new();
    if let Some(m) = PROCS.lock().unwrap().as_mut() {
        for (mode, c) in m.iter_mut() {
            out.insert(mode.clone(), json!(matches!(c.child.try_wait(), Ok(None))));
        }
    }
    Value::Object(out)
}

/// Start the proxy (or standalone backend) and wait until OUR child
/// listens - spawning is not starting.
pub fn start(mode: &str) -> Value {
    let Some(tool) = tool_name(mode) else { return json!({"ok": false, "error": format!("unknown mode {mode}")}) };
    let script = paths::repo().join("tools").join(format!("{tool}.py"));
    // both are ported (acecm --tool acevo_proxy / acevo_backend);
    // ACECM_PY_PROXY=1 runs the Python scripts instead, for comparison
    let native = std::env::var("ACECM_PY_PROXY").is_err();
    if !native && !script.exists() {
        return json!({"ok": false, "error": format!("missing {}", script.display())});
    }
    let made = ensure_cert(false);
    if made["ok"] != true {
        return json!({"ok": false, "error": format!("cannot create a TLS keypair: {}", made["error"].as_str().unwrap_or(""))});
    }
    stop();
    if !lobby::path().exists() {
        let items = profiles::load();
        lobby::write(items.first().unwrap_or(&json!({})));
    }
    // ⚠ without the schemas the proxy binds, relays and captures NOTHING
    // while every indicator reads healthy
    if !protos::has("BackendMessage") {
        protos::extract(false);
    }
    if !protos::has("BackendMessage") {
        return json!({"ok": false, "error": "the game's message schemas are missing, so the backend could read lobby traffic but never understand it - the server browser would stay empty with everything looking fine. They are read from your own AssettoCorsaEVO.exe: set game_dir in Settings to your Assetto Corsa EVO install and start it again",
                      "protos": protos::cache_dir().to_string_lossy()});
    }
    let Ok(log) = std::fs::File::create(log_path(mode)) else { return json!({"ok": false, "error": "cannot open the backend log"}) };
    let cfg = config::load();
    let port = backend_port();
    let lan = misc::lan_ipv4();
    let mut ours: Vec<String> = vec!["127.0.0.1".into(), "localhost".into(), "::1".into()];
    if !lan.is_empty() {
        ours.push(lan.clone());
    }
    ours.sort();
    let mut c = if native {
        let mut c = std::process::Command::new(std::env::current_exe().unwrap_or_default());
        c.args(["--tool", tool]);
        c
    } else {
        let mut c = std::process::Command::new(sidecar::python());
        c.args(["-u", &script.to_string_lossy()]);
        c
    };
    c.current_dir(paths::data())
        .env("ACECM_PROTOS", protos::cache_dir()).env("ACECM_CERTS", certs_dir())
        .env("LOBBY_JSON", lobby::path()).env("OUR_IPS", ours.join(","))
        .env("PORT", port.to_string())
        .env("BACKEND_LISTEN", { let l = config::get_str(&cfg, "backend_listen"); if l.is_empty() { "127.0.0.1".into() } else { l } })
        .env("PYTHONUNBUFFERED", "1")
        // it goes with us, even when we are killed rather than closed
        .env("ACECM_PARENT_PID", std::process::id().to_string())
        .stdout(log.try_clone().unwrap()).stderr(log);
    if !lan.is_empty() {
        c.env("SERVER_IP", &lan).env("LAN_IP", &lan);
    }
    procs::hide(&mut c);
    let child = match c.spawn() {
        Ok(ch) => ch,
        Err(e) => return json!({"ok": false, "error": format!("could not start the backend: {e}")}),
    };
    let pid = child.id();
    PROCS.lock().unwrap().get_or_insert_with(HashMap::new).insert(mode.to_string(), Child { child });
    let until = Instant::now() + Duration::from_secs(30);
    let mut owner = vec![];
    while Instant::now() < until {
        owner = procs::tcp_listen_pids(port);
        if owner.iter().any(|o| is_descendant(*o, pid)) {
            return json!({"ok": true, "pid": pid, "mode": mode, "port": port});
        }
        if !procs::alive(pid) {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    // ⚠ never leave a late child running after calling it a failure
    if let Some(m) = PROCS.lock().unwrap().as_mut() {
        if let Some(mut c) = m.remove(mode) {
            let _ = procs::kill(c.child.id());
            let _ = c.child.wait();
        }
    }
    let tail: Vec<String> = log_tail_text(mode).lines().filter(|l| !l.trim().is_empty())
        .map(String::from).collect::<Vec<_>>().into_iter().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect();
    let why = if let Some(o) = owner.first() {
        format!("port {port} is already held by another process (pid {o}), so this backend could not bind. Close the other ACECM or backend window and start it again.")
    } else if !procs::alive(pid) {
        format!("the backend exited immediately without listening on port {port} - see the detail below")
    } else {
        format!("the backend is running but never listened on port {port}")
    };
    json!({"ok": false, "pid": pid, "mode": mode, "port": port, "error": why, "detail": tail})
}

/// Last "Connecting to backend at …" line in the newest game log: the only
/// proof of where the running client actually dialled.
pub fn game_backend_seen() -> Value {
    let ace = detect::find("ace_dir", false);
    let folder = Path::new(&ace).join("Logs");
    if ace.is_empty() || !folder.is_dir() {
        return json!({"url": null, "log": null});
    }
    let mut names: Vec<String> = std::fs::read_dir(&folder).map(|rd| rd.flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| { let l = n.to_lowercase(); l.starts_with("log-") && l.ends_with(".txt") }).collect()).unwrap_or_default();
    if names.is_empty() {
        return json!({"url": null, "log": null});
    }
    names.sort();
    let path = folder.join(names.last().unwrap());
    let Ok(f) = std::fs::File::open(&path) else { return json!({"url": null, "log": path.to_string_lossy()}) };
    use std::io::Read;
    let mut buf = vec![];
    let _ = f.take(2_000_000).read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    let needle = "Connecting to backend at ";
    let url = text.lines().filter_map(|l| l.find(needle).map(|i| l[i + needle.len()..].trim().to_string())).last();
    json!({"url": url, "log": path.to_string_lossy()})
}

pub fn state() -> Value {
    let _ = ensure_cert(false);
    let (cert, key) = cert_paths();
    let port = backend_port();
    let lan = misc::lan_ipv4();
    let mut ours: Vec<String> = vec!["127.0.0.1".into(), "localhost".into(), "::1".into()];
    if !lan.is_empty() {
        ours.push(lan.clone());
    }
    ours.sort();
    let probe = probe_client_url();
    json!({
        "port": port,
        "listening": !procs::tcp_listen_pids(port).is_empty(),
        "running": running(),
        "have_cert": cert.exists() && key.exists(),
        "cert_covers_lan": cert_covers(),
        "tools_dir": crate::bundle::tools_dir().to_string_lossy(),
        "lan_ip": lan, "our_ips": ours,
        "launch_backend": redirect_url(),
        "client_patched": probe.get("rdata_patched").cloned().unwrap_or(Value::Null),
        "client_url": probe,
        "inspector_patched": probe_inspector().get("inspector_patched").cloned().unwrap_or(Value::Null),
        "inspector_port": inspector_port(),
        "game_backend": game_backend_seen(),
    })
}

/// Is a game client attached to our backend and joinable?
pub fn join_state() -> Value {
    if procs::tcp_listen_pids(8093).is_empty() {
        return json!({"control": false, "error": "proxy not listening", "hint": "start the proxy backend to enable direct join"});
    }
    let got = reqwest::blocking::Client::builder().timeout(Duration::from_millis(400)).build().ok()
        .and_then(|c| c.get(format!("{CONTROL}/")).send().ok())
        .and_then(|r| r.text().ok()).and_then(|t| serde_json::from_str::<Value>(&t).ok());
    match got {
        Some(Value::Object(mut m)) => {
            m.insert("control".into(), json!(true));
            Value::Object(m)
        }
        _ => json!({"control": false, "error": "OSError", "hint": "start the proxy backend to enable direct join"}),
    }
}

fn seen_note(seen: &Value) -> String {
    let url = seen.get("url").and_then(|v| v.as_str()).unwrap_or("");
    if url.is_empty() {
        return String::new();
    }
    if url.contains("127.0.0.1") || url.contains("localhost") {
        return format!(" (game log: {url})");
    }
    format!(" (game log still shows {url} — Steam dropped -backend= and rdata was not rewritten)")
}

/// Why the server browser is empty, as a checklist (every link can be
/// missing silently, so name them).
pub fn browser_chain() -> Value {
    let st = state();
    let cu = st.get("client_url").cloned().unwrap_or(json!({}));
    let captured = std::fs::read(paths::data().join("server_list.json")).ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.get("servers").and_then(|s| s.as_array()).map(|a| a.len())).unwrap_or(0);
    let exe = game_exe();
    let live = join_state();
    let t = |v: Option<&Value>| match v { Some(Value::Bool(b)) => *b, Some(Value::Null) | None => false, Some(_) => true };
    let talks = t(live.get("client_connected")) || t(st.get("client_patched")) || t(cu.get("rdata_patched"));
    let detail = if t(live.get("client_connected")) {
        "a client is connected to the proxy right now".to_string()
    } else if t(st.get("client_patched")) || t(cu.get("rdata_patched")) {
        "rdata rewritten, so the next game start reaches us".to_string()
    } else {
        format!("rdata still points at Kunos and no client is on the proxy{}{}",
                if t(live.get("control")) { "" } else { " (and the proxy's control port is not answering, so it may have started only partly)" },
                seen_note(st.get("game_backend").unwrap_or(&json!({}))))
    };
    let steps = vec![
        json!({"ok": !exe.is_empty(), "what": "game executable located",
               "fix": "Set game_exe in Settings to AssettoCorsaEVO.exe. Without it ACECM has to start the game through Steam, which drops the -backend= flag and the list never reaches us",
               "detail": exe}),
        json!({"ok": t(st.get("listening")), "what": format!("backend proxy listening on :{}", st["port"]),
               "fix": "Start it on the Backend page"}),
        json!({"ok": t(st.get("have_cert")), "what": "TLS certificate present",
               "fix": "Generate it on the Backend page - the client will not connect to the proxy without one"}),
        json!({"ok": talks, "what": "game client talks to the local backend",
               "fix": "Close the game, then Launch from ACECM. Launch rewrites the lobby URL in the exe first — that is required. Steam drops -backend=, so an unpatched client never reaches this proxy. If the rewrite says it needs administrator, run ACECM as admin once (Program Files). Steam 'Verify integrity' undoes the rewrite.",
               "detail": detail}),
        json!({"ok": captured > 0,
               "what": if captured > 0 { format!("server list captured ({captured} servers)") } else { "server list captured".into() },
               "fix": if t(live.get("client_connected")) {
                   "The client IS connected to us, so the link is fine - open Multiplayer in-game now and the list is captured as it loads. If it stays empty with the client connected, the proxy could not reach Kunos upstream: check the backend log on this page."
               } else {
                   "With the three above done, open Multiplayer in-game once. After that it is remembered and works with the game closed"
               }}),
    ];
    let first = steps.iter().find(|s| s["ok"] != true).cloned();
    json!({"ok": first.is_none(), "steps": steps,
           "blocked_on": first.map(|f| f["what"].clone()).unwrap_or(Value::Null), "captured": captured})
}

/// Push the client into a dedicated server (join:<ip>:<tcp>, never a uuid).
pub fn join(server_id: &Value, shape: &str, tcp: Value, udp: Value, password: &str) -> Value {
    let udp = if udp.is_null() || udp == json!(0) { tcp.clone() } else { udp };
    let body = json!({"id": server_id, "shape": shape, "tcp": tcp, "udp": udp, "password": password});
    let r = reqwest::blocking::Client::builder().timeout(Duration::from_secs(8)).build().ok()
        .map(|c| c.post(format!("{CONTROL}/join")).header("Content-Type", "application/json")
            .body(profiles::py_dumps(&body)).send());
    match r {
        Some(Ok(resp)) => resp.text().ok().and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_else(|| json!({"ok": false, "error": "bad reply"})),
        Some(Err(e)) => json!({"ok": false, "error": e.to_string(), "hint": "is the proxy backend running?"}),
        None => json!({"ok": false, "error": "no http client"}),
    }
}

// ------------------------------------------------- the public server list --
fn list_cache() -> PathBuf {
    paths::data().join("server_list.json")
}

/// captured_at of the saved list, or 0 (cheap: the stamp is near the top).
pub fn cached_at(path: &Path) -> u64 {
    use std::io::Read;
    let Ok(f) = std::fs::File::open(path) else { return 0 };
    let mut head = vec![0u8; 4096];
    let n = f.take(4096).read(&mut head).unwrap_or(0);
    let head = String::from_utf8_lossy(&head[..n]);
    let Some(i) = head.find("\"captured_at\"") else { return 0 };
    let rest = &head[i + 13..];
    let digits: String = rest.trim_start_matches([':', ' ']).chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().unwrap_or(0)
}

pub struct LobbyState {
    pub running: bool,
    pub last_try: Option<Instant>,
    pub last: Value,
}

pub static LOBBY: Mutex<LobbyState> = Mutex::new(LobbyState { running: false, last_try: None, last: Value::Null });
static LOBBY_DONE: std::sync::Condvar = std::sync::Condvar::new();
const LOBBY_STALE: u64 = 300;
const LOBBY_RETRY: u64 = 60;

/// Pull the public list from the Kunos lobby without the game (single
/// flight: a second caller waits for the first fetch). ⚠ Never while the
/// game runs - the proxy captures it then, and a second "EVO" on the
/// account is not worth it.
pub fn lobby_fetch(wait: bool, timeout_s: u64) -> Value {
    if procs::game_running() {
        return json!({"ok": false, "error": "the game is running", "fallback": true});
    }
    {
        let mut g = LOBBY.lock().unwrap();
        if g.running {
            if !wait {
                return json!({"ok": true, "busy": true});
            }
            let (g2, _) = LOBBY_DONE.wait_timeout_while(g, Duration::from_secs(timeout_s), |s| s.running).unwrap();
            return if g2.last.is_null() { json!({"ok": false, "error": "timed out"}) } else { g2.last.clone() };
        }
        g.running = true;
        g.last_try = Some(Instant::now());
    }
    let mut res = lobby_fetch_run(timeout_s);
    if res.get("at").is_none() {
        res["at"] = json!(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0));
    }
    let mut g = LOBBY.lock().unwrap();
    g.last = res.clone();
    g.running = false;
    LOBBY_DONE.notify_all();
    res
}

fn lobby_fetch_run(timeout_s: u64) -> Value {
    let exe = game_exe();
    let game_dir = if exe.is_empty() { detect::find("game_dir", false) }
                   else { Path::new(&exe).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default() };
    if game_dir.is_empty() || !Path::new(&game_dir).join("steam_api64.dll").is_file() {
        return json!({"ok": false, "fallback": true, "error": "AC EVO's install folder was not found"});
    }
    if !protos::has("MultiplayerServerListResponseServerList") {
        protos::extract(false);
    }
    if !protos::has("MultiplayerServerListResponseServerList") {
        return json!({"ok": false, "fallback": true, "error": "the game's message schemas are missing"});
    }
    let t = Instant::now();
    let mut c = std::process::Command::new(std::env::current_exe().unwrap_or_default());
    c.args(["--tool", "lobby_fetch", "--out", &list_cache().to_string_lossy(), "--game-dir", &game_dir])
        .env("ACECM_PROTOS", protos::cache_dir()).current_dir(paths::data())
        .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    procs::hide(&mut c);
    let child = match c.spawn() {
        Ok(ch) => ch,
        Err(e) => return json!({"ok": false, "fallback": true, "error": e.to_string()}),
    };
    let pid = child.id();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || { let _ = tx.send(child.wait_with_output()); });
    let out = match rx.recv_timeout(Duration::from_secs(timeout_s)) {
        Ok(Ok(o)) => o,
        _ => { procs::kill(pid); return json!({"ok": false, "fallback": true, "error": "the lobby fetch timed out"}) }
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut res = text.lines().rev().find_map(|l| { let l = l.trim(); if l.starts_with('{') { serde_json::from_str::<Value>(l).ok() } else { None } })
        .unwrap_or_else(|| {
            let tail: String = format!("{}{}", text, String::from_utf8_lossy(&out.stderr)).trim().chars().rev().take(400).collect::<Vec<_>>().into_iter().rev().collect();
            json!({"ok": false, "error": if tail.is_empty() { format!("exit {:?}", out.status.code()) } else { tail }})
        });
    res["spawn_ms"] = json!(t.elapsed().as_millis() as u64);
    if res["ok"] != true {
        res["fallback"] = json!(true);
    }
    res
}

/// Refresh in the background when the saved list is stale. Never blocks.
pub fn lobby_prefetch(max_age: u64) -> bool {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    if now.saturating_sub(cached_at(&list_cache())) < max_age {
        return false;
    }
    {
        let g = LOBBY.lock().unwrap();
        if g.running || g.last_try.map(|t| t.elapsed().as_secs() < LOBBY_RETRY).unwrap_or(false) {
            return false;
        }
    }
    if procs::game_running() {
        return false;
    }
    std::thread::spawn(|| lobby_fetch(false, 30));
    true
}

pub fn lobby_prefetch_default() -> bool {
    lobby_prefetch(LOBBY_STALE)
}

/// Every public EVO server: whichever copy is NEWER, the proxy's capture or
/// the saved list (a 9:00 capture must not hide - or overwrite - a 9:30
/// direct fetch).
pub fn server_list() -> Value {
    let cache = list_cache();
    if !procs::tcp_listen_pids(8093).is_empty() {
        let got = reqwest::blocking::Client::builder().timeout(Duration::from_millis(1500)).build().ok()
            .and_then(|c| c.get(format!("{CONTROL}/servers")).send().ok())
            .and_then(|r| r.text().ok()).and_then(|t| serde_json::from_str::<Value>(&t).ok());
        if let Some(Value::Object(mut g)) = got {
            let live = g.get("servers").and_then(|s| s.as_array()).map(|a| !a.is_empty()).unwrap_or(false);
            let at = g.get("captured_at").and_then(|v| v.as_u64()).unwrap_or(0);
            if live && at >= cached_at(&cache) {
                let mut out = serde_json::Map::new();
                out.insert("ok".into(), json!(true));
                out.extend(std::mem::take(&mut g));
                let v = Value::Object(out);
                let _ = std::fs::write(&cache, profiles::py_dumps(&v));
                return v;
            }
        }
    }
    match std::fs::read(&cache).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) {
        Some(Value::Object(mut got)) => {
            let direct = got.get("source").and_then(|v| v.as_str()) == Some("lobby");
            let mtime = std::fs::metadata(&cache).ok().and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
            let at = got.get("captured_at").and_then(|v| v.as_u64()).filter(|x| *x != 0).unwrap_or(mtime);
            got.insert("ok".into(), json!(true));
            got.insert("cached".into(), json!(!direct));
            got.insert("captured_at".into(), json!(at));
            got.insert("note".into(), json!(if direct { "fetched from the lobby directly" } else {
                "from the last time the in-game browser was open - player counts and pings will be stale, but it is enough to see what content a server needs" }));
            if LOBBY.lock().unwrap().running {
                got.insert("refreshing".into(), json!(true));
            }
            Value::Object(got)
        }
        _ => json!({"ok": false, "error": "FileNotFoundError",
                    "hint": "start the proxy backend, then open Multiplayer in-game once so the list passes through - after that it is remembered and usable with the game closed"}),
    }
}
