//! Talk to the game's Gameface inspector (Chrome DevTools on :9444). Ported
//! from acecm/gameui.py; the scripts themselves live in gameui_js.rs.
//!
//! The UI is cohtml. Stock EVO leaves the inspector off; launch_game enables
//! it in the exe before Steam starts the game. Steam drops every launch flag
//! except Launch Options, so Drive cannot start a session from argv - it has
//! to press Start the same way the menu does.
//!
//! Everything here BLOCKS (call it off the async runtime).

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use base64::Engine;
use serde_json::{json, Value};

use crate::gameui_js as js;

const HOST: &str = "127.0.0.1";

fn port() -> u16 {
    std::env::var("INSPECTOR_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(9444)
}

fn addr() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port()))
}

pub fn listening() -> bool {
    TcpStream::connect_timeout(&addr(), Duration::from_millis(400)).is_ok()
}

pub fn list_views() -> Result<Vec<Value>, String> {
    let r = reqwest::blocking::Client::builder().timeout(Duration::from_secs(8)).build()
        .map_err(|e| e.to_string())?
        .get(format!("http://{HOST}:{}/json/list", port())).send().map_err(|e| e.to_string())?;
    let t = r.text().map_err(|e| e.to_string())?;
    match serde_json::from_str::<Value>(&t).map_err(|e| e.to_string())? {
        Value::Array(a) => Ok(a),
        _ => Err("not a list".into()),
    }
}

// ------------------------------------------------------ a tiny ws client --
// ⚠ Hand-rolled on purpose, like the Python one: the inspector speaks just
// enough websocket for DevTools, and a minimal handshake is what it accepts.
fn recv_exact(s: &mut TcpStream, n: usize) -> std::io::Result<Vec<u8>> {
    let mut buf = vec![0u8; n];
    s.read_exact(&mut buf).map_err(|e| {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            std::io::Error::new(std::io::ErrorKind::ConnectionAborted, "socket closed")
        } else { e }
    })?;
    Ok(buf)
}

fn send_raw(s: &mut TcpStream, opcode: u8, data: &[u8]) -> std::io::Result<()> {
    let mut hdr = vec![0x80 | opcode];
    let n = data.len();
    if n < 126 {
        hdr.push(0x80 | n as u8);
    } else if n < 65536 {
        hdr.push(0x80 | 126);
        hdr.extend_from_slice(&(n as u16).to_be_bytes());
    } else {
        hdr.push(0x80 | 127);
        hdr.extend_from_slice(&(n as u64).to_be_bytes());
    }
    let mut mask = [0u8; 4];
    let _ = getrandom::fill(&mut mask);
    hdr.extend_from_slice(&mask);
    hdr.extend(data.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    s.write_all(&hdr)
}

fn recv_raw(s: &mut TcpStream) -> std::io::Result<(u8, Vec<u8>)> {
    let h = recv_exact(s, 2)?;
    let op = h[0] & 0x0F;
    let mut ln = (h[1] & 0x7F) as u64;
    if ln == 126 {
        let b = recv_exact(s, 2)?;
        ln = u16::from_be_bytes([b[0], b[1]]) as u64;
    } else if ln == 127 {
        let b = recv_exact(s, 8)?;
        ln = u64::from_be_bytes(b.try_into().unwrap());
    }
    if h[1] & 0x80 != 0 {
        let mask = recv_exact(s, 4)?;
        let mut data = recv_exact(s, ln as usize)?;
        for (i, b) in data.iter_mut().enumerate() {
            *b ^= mask[i % 4];
        }
        return Ok((op, data));
    }
    Ok((op, recv_exact(s, ln as usize)?))
}

fn recv_frame(s: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    loop {
        let (op, payload) = recv_raw(s)?;
        match op {
            0x1 => return Ok(payload),
            0x8 => return Err(std::io::Error::new(std::io::ErrorKind::ConnectionAborted, "server closed the websocket")),
            0x9 => send_raw(s, 0xA, &payload)?,
            _ => {}
        }
    }
}

fn recv_json(s: &mut TcpStream) -> std::io::Result<Value> {
    let f = recv_frame(s)?;
    serde_json::from_slice(&f).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

enum EvalErr {
    Io(String),
    Other(String),
}

fn evaluate_once(expr: &str, page: &str, timeout: f64, user_gesture: bool) -> Result<Option<Value>, EvalErr> {
    let io = |e: std::io::Error| EvalErr::Io(e.to_string());
    let to = Duration::from_secs_f64(timeout);
    let mut s = TcpStream::connect_timeout(&addr(), to).map_err(io)?;
    s.set_read_timeout(Some(to)).ok();
    s.set_write_timeout(Some(to)).ok();
    let mut k = [0u8; 16];
    let _ = getrandom::fill(&mut k);
    let key = base64::engine::general_purpose::STANDARD.encode(k);
    let p = port();
    s.write_all(format!("GET /devtools/page/{page} HTTP/1.1\r\nHost: {HOST}:{p}\r\n\
        Upgrade: websocket\r\nConnection: Upgrade\r\n\
        Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n").as_bytes()).map_err(io)?;
    let mut resp = Vec::new();
    // ⚠ read the handshake byte-wise: anything past \r\n\r\n is a frame
    while !resp.ends_with(b"\r\n\r\n") {
        let mut b = [0u8; 1];
        // the game closes the socket mid-handshake while the inspector is
        // still starting: that must fail, not spin
        match s.read(&mut b) {
            Ok(0) => return Err(EvalErr::Io("inspector closed the handshake".into())),
            Ok(_) => resp.push(b[0]),
            Err(e) => return Err(io(e)),
        }
        if resp.len() > 65536 {
            return Err(EvalErr::Io("oversized handshake".into()));
        }
    }
    let first = String::from_utf8_lossy(resp.split(|&c| c == b'\r').next().unwrap_or(&[])).into_owned();
    if !first.contains("101") {
        return Err(EvalErr::Io(format!("upgrade failed: {first}")));
    }
    send_raw(&mut s, 0x1, json!({"id": 1, "method": "Runtime.enable"}).to_string().as_bytes()).map_err(io)?;
    s.set_read_timeout(Some(Duration::from_secs(4))).ok();
    let mut ctx = None;
    while ctx.is_none() {
        match recv_json(&mut s) {
            Ok(m) => {
                if m.get("method").and_then(|v| v.as_str()) == Some("Runtime.executionContextCreated") {
                    ctx = m.pointer("/params/context/id").cloned();
                }
            }
            Err(_) => break,
        }
    }
    let Some(ctx) = ctx else { return Ok(None) };
    s.set_read_timeout(Some(to)).ok();
    let params = json!({"expression": expr, "returnByValue": true, "awaitPromise": true,
                        "userGesture": user_gesture, "contextId": ctx});
    send_raw(&mut s, 0x1, json!({"id": 2, "method": "Runtime.evaluate", "params": params}).to_string().as_bytes()).map_err(io)?;
    loop {
        let m = recv_json(&mut s).map_err(|e| {
            if e.kind() == std::io::ErrorKind::InvalidData { EvalErr::Other(e.to_string()) } else { io(e) }
        })?;
        if m.get("id").and_then(|v| v.as_i64()) == Some(2) {
            return Ok(Some(m));
        }
    }
}

pub fn evaluate(expr: &str, page: &str, timeout: f64, attempts: u32, user_gesture: bool) -> Result<Value, String> {
    let mut last: Option<String> = None;
    for _ in 0..attempts {
        match evaluate_once(expr, page, timeout, user_gesture) {
            Ok(Some(r)) => return Ok(r),
            Ok(None) => {}
            Err(EvalErr::Io(e)) => {
                last = Some(e);
                if !listening() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(300));
            }
            Err(EvalErr::Other(e)) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| "no reply from the inspector".into()))
}

/// {"ok": true, "value": ...} or {"ok": false, "error": ...}
pub fn js_value(msg: &Value) -> Value {
    if msg.is_null() {
        return Value::Null;
    }
    let res = msg.get("result").filter(|r| r.is_object()).unwrap_or(msg);
    if let Some(ex) = res.get("exceptionDetails").filter(|e| !e.is_null() && *e != &json!({})) {
        let text = ex.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let desc = match ex.pointer("/exception/description") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Null) | None => String::new(),
            Some(v) => v.to_string(),
        };
        return json!({"ok": false, "error": format!("{text} {desc}").trim()});
    }
    let r = res.get("result").cloned().unwrap_or(json!({}));
    let v = match r.get("value") {
        Some(v) => v.clone(),
        None => r.get("description").cloned().unwrap_or(Value::Null),
    };
    json!({"ok": true, "value": v})
}

/// evaluate + js_value; an error becomes {"ok": false, "error": ...}
fn run(expr: &str, page: &str, timeout: f64, attempts: u32, gesture: bool) -> Result<Value, String> {
    evaluate(expr, page, timeout, attempts, gesture).map(|m| js_value(&m))
}

fn ok_str(r: &Value) -> Option<String> {
    if r.get("ok") == Some(&json!(true)) {
        Some(match r.get("value") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Null) | None => String::new(),
            Some(v) => v.to_string(),
        })
    } else {
        None
    }
}

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

fn view_id(v: &Value) -> String {
    match v.get("id") {
        Some(Value::String(s)) if !s.is_empty() => s.clone(),
        Some(Value::Number(n)) if n.as_f64() != Some(0.0) => n.to_string(),
        _ => "0".into(),
    }
}

/// The inspector page that is actually the menu, not HUD.
pub fn menu_page() -> String {
    let Ok(views) = list_views() else { return "0".into() };
    for v in &views {
        let url = v.get("url").and_then(|u| u.as_str()).unwrap_or("").to_lowercase();
        if ["menu.html", "singleplayer.html", "ingame.html"].iter().any(|x| url.contains(x)) {
            return view_id(v);
        }
    }
    "0".into()
}

/// The HUD view specifically (ingame.html in the pits, hud.html driving).
pub fn hud_page() -> Option<String> {
    let views = list_views().ok()?;
    for v in &views {
        let url = v.get("url").and_then(|u| u.as_str()).unwrap_or("").to_lowercase();
        if url.contains("uiresources/hud.html") || url.contains("uiresources/ingame.html") {
            return Some(view_id(v));
        }
    }
    None
}

/// Cheap readiness check.
pub fn ready() -> bool {
    if !listening() {
        return false;
    }
    run("!!(window.ksUI && window.GAMEMODESELECTION)", &menu_page(), 2.0, 1, false)
        .map(|r| r.get("ok") == Some(&json!(true)) && truthy(r.get("value")))
        .unwrap_or(false)
}

/// Live boot gate: "page|path|car|dest|paint|locked".
pub fn boot_state() -> String {
    if !listening() {
        return String::new();
    }
    run(js::BOOT, &menu_page(), 2.0, 1, false).ok().and_then(|r| ok_str(&r)).unwrap_or_default()
}

pub fn boot_parts(s: &str) -> Vec<String> {
    s.to_lowercase().split('|').map(String::from).collect()
}

pub fn boot_page(s: &str) -> String {
    boot_parts(s).into_iter().next().unwrap_or_default()
}

pub fn paintshop_up(s: &str) -> bool {
    let p = boot_parts(s);
    p.len() > 4 && p[4] == "paint"
}

/// Home or single-player page with a current car, paintshop not overlaid.
pub fn home_ready(s: &str) -> bool {
    !paintshop_up(s) && matches!(boot_page(s).as_str(), "home" | "sp")
}

pub fn session_loading(s: &str) -> bool {
    let s = s.to_lowercase();
    s.contains("ingame") || s.contains("|session")
}

/// The track Single Player has selected ("Name/Layout"), or "".
pub fn selected_track(timeout: f64) -> String {
    let Ok(m) = evaluate(&format!("({})()", js::CURRENT_TRACK), &menu_page(), timeout, 2, false) else { return String::new() };
    // ⚠ Python str()s the whole js_value dict here; only the value matters
    let v = js_value(&m);
    let val = ok_str(&v).unwrap_or_default().trim().to_string();
    if matches!(val.as_str(), "/" | "-" | "none" | "null" | "undefined") { String::new() } else { val }
}

pub fn go_home() -> Value {
    let e = "(function(){if(!window.ksUI)return 'no-ksUI';\
             try{ksUI.goTo('menu.html','main/main');return 'home';}\
             catch(e){return 'fail:'+String(e&&e.message||e);}})()";
    run(e, &menu_page(), 3.0, 1, false).unwrap_or_else(|e| json!({"ok": false, "error": e}))
}

pub fn back_to_pit() -> Value {
    let Some(page) = hud_page() else { return json!({"ok": false, "error": "not in a session"}) };
    let r = run(js::BACK_TO_PIT, &page, 6.0, 2, false).unwrap_or_else(|e| json!({"ok": false, "error": e}));
    if r.get("ok") != Some(&json!(true)) {
        return json!({"ok": false, "error": r.get("error").filter(|e| truthy(Some(e))).cloned().unwrap_or(json!("no answer"))});
    }
    let val = r.get("value").cloned().unwrap_or(Value::Null);
    let sent = val == json!("sent");
    json!({"ok": sent, "state": val, "error": if sent { Value::Null } else { json!(ok_str(&r).unwrap_or_default()) }})
}

pub fn set_mph(on: bool) -> Value {
    let Some(page) = hud_page() else { return json!({"ok": false, "error": "the HUD is not open"}) };
    let r = run(if on { js::MPH } else { js::MPH_OFF }, &page, 8.0, 2, false).unwrap_or_else(|e| json!({"ok": false, "error": e}));
    if r.get("ok") != Some(&json!(true)) {
        return json!({"ok": false, "error": r.get("error").filter(|e| truthy(Some(e))).cloned().unwrap_or(json!("no answer"))});
    }
    json!({"ok": true, "state": r.get("value").cloned().unwrap_or(Value::Null)})
}

pub fn enter_singleplayer() -> Result<Value, String> {
    run(js::GOTO, &menu_page(), 10.0, 4, false)
}

/// Open the in-game public list, deep-linked to one server when we know it
/// (`multiplayer/serverlist/<ip>:<port>|<password>`). ⚠ The deep link
/// searches the LOADED list and shows a modal when the row is missing, so
/// callers pass deep=false unless they know it is there.
pub fn enter_multiplayer(ip: &str, tcp: u32, password: &str, deep: bool) -> Result<Value, String> {
    let target = if deep && !ip.is_empty() && tcp != 0 { format!("{ip}:{tcp}|{password}") } else { String::new() };
    run(&format!("({})({})", js::GOTO_MP, json!(target)), &menu_page(), 10.0, 4, false)
}

/// Is the in-game server list page actually open?
pub fn server_list_page() -> bool {
    run("!!document.querySelector('ks-page-serverlist')", &menu_page(), 6.0, 1, false)
        .map(|r| truthy(r.get("value"))).unwrap_or(false)
}

/// Select the row in the in-game list and press Join once.
pub fn join_public(ip: &str, tcp: u32, password: &str, server_id: &str) -> Result<Value, String> {
    let want = json!({"ip": ip, "tcp": tcp, "password": password, "id": server_id});
    run(&format!("({})({})", js::JOIN_PUBLIC, want), &menu_page(), 8.0, 1, false)
}

pub fn refresh_server_list() -> Result<Value, String> {
    run(js::REFRESH_LIST, &menu_page(), 4.0, 1, false)
}

pub fn quit_game() -> Value {
    if !listening() {
        return json!({"ok": false, "error": "inspector not listening"});
    }
    run(js::QUIT, &menu_page(), 3.0, 1, true).unwrap_or_else(|e| json!({"ok": false, "error": e}))
}

pub fn press_start() -> Result<Value, String> {
    run(js::START, &menu_page(), 8.0, 2, true)
}

pub fn current_car_name() -> String {
    run("(window.CurrentCar && CurrentCar.model && (CurrentCar.model.name||'')) || ''", &menu_page(), 2.0, 1, false)
        .ok().and_then(|r| ok_str(&r)).unwrap_or_default()
}

/// Make this the current car: model + mechanical trim + visual preset.
pub fn select_car(model: &str, preset: &str, mech: &str, visual: &str) -> Result<Value, String> {
    let visual = if visual.is_empty() { preset } else { visual };
    let want = json!({"model": model, "preset": preset, "mech": mech, "visual": visual});
    run(&format!("({})({})", js::SELECT, want), &menu_page(), 30.0, 2, false)
}

/// Make this the selected track on the Single Player page.
pub fn select_track(name: &str, layout: &str, event_name: &str, track_length: i64) -> Result<Value, String> {
    let want = json!({"name": name, "layout": layout, "event_name": event_name, "track_length": track_length});
    run(&format!("({})({})", js::SET_TRACK, want), &menu_page(), 30.0, 2, false)
}

fn pint(p: &Value, k: &str, fallback: i64) -> i64 {
    match p.get(k) {
        Some(Value::Number(n)) if n.as_f64() != Some(0.0) => n.as_i64().unwrap_or_else(|| n.as_f64().unwrap_or(0.0) as i64),
        Some(Value::String(s)) if !s.is_empty() => s.trim().parse().unwrap_or(fallback),
        Some(Value::Bool(true)) => 1,
        _ => fallback,
    }
}

/// Weather, time, mode type, AI count/skill - same as the SP pages.
pub fn apply_conditions(pick: &Value, weather: &str, hour: i64, minute: i64) -> Result<Value, String> {
    let s = |k: &str| match pick.get(k) {
        Some(Value::String(x)) => x.clone(),
        Some(Value::Null) | None => String::new(),
        Some(v) => v.to_string(),
    };
    let mut mode = s("game_mode").trim().to_uppercase().replace("GAMEMODETYPE_", "");
    if !mode.is_empty() && !mode.starts_with("GAMEMODETYPE_") {
        mode = format!("GameModeType_{mode}");
    }
    let bare = mode.replace("GameModeType_", "");
    let ai = bare == "INSTANT_RACE" || bare == "RACE_WEEKEND";
    let or = |k: &str, d: &str| { let v = s(k); if v.is_empty() { d.to_string() } else { v } };
    let tod = |k: &str, d: i64| match pick.get(k) {
        None | Some(Value::Null) => d,
        Some(Value::Number(n)) => n.as_i64().unwrap_or_else(|| n.as_f64().unwrap_or(0.0) as i64),
        Some(Value::String(x)) => x.trim().parse().unwrap_or(d),
        Some(Value::Bool(b)) => *b as i64,
        _ => d,
    };
    let single = match pick.get("single_make") { None => true, v => truthy(v) };
    let weather_v = { let w = s("weather"); if !w.is_empty() { w } else if !weather.is_empty() { weather.to_string() } else { "CLEAR".into() } };
    let mut want = json!({
        "weather": weather_v,
        "hour": tod("tod_hour", hour),
        "minute": tod("tod_minute", minute),
        "mode": mode,
        "aggressiveness": or("aggressiveness", "Safe"),
        "single_make": single,
    });
    if ai {
        want["opponents"] = json!(pint(pick, "num_opponents", 10));
        want["min_strength"] = json!(pint(pick, "skill_min", 80));
        want["max_strength"] = json!(pint(pick, "skill_max", 95));
        want["starting_position"] = json!(pint(pick, "starting_position", 0));
        want["race_laps"] = json!(pint(pick, "race_laps", 10));
        if mode.contains("RACE_WEEKEND") {
            want["practice_min"] = json!(pint(pick, "practice_min", 10));
            want["quali_min"] = json!(pint(pick, "quali_min", 15));
            want["warmup_min"] = json!(pint(pick, "warmup_min", 10));
        }
    } else {
        want["duration_min"] = json!(pint(pick, "duration_min", 90));
    }
    run(&format!("({})({})", js::CONDITIONS, want), &menu_page(), 25.0, 2, false)
}

/// "pitlane/main|coui://.../ingame.html" or similar.
pub fn where_() -> String {
    run(js::WHERE, &menu_page(), 8.0, 2, false).ok().and_then(|r| ok_str(&r)).unwrap_or_default()
}

pub fn in_pits(hint: Option<&str>) -> bool {
    let h = match hint { Some(h) => h.to_string(), None => where_() };
    h.to_lowercase().contains("pitlane")
}

// ------------------------------------------------------------ the window --
#[cfg(windows)]
mod win {
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    unsafe extern "system" fn each(h: HWND, lp: LPARAM) -> i32 {
        let mut buf = [0u16; 256];
        let n = unsafe { GetWindowTextW(h, buf.as_mut_ptr(), 256) };
        let t = String::from_utf16_lossy(&buf[..n.max(0) as usize]).to_lowercase();
        if t.contains("assetto corsa evo") {
            unsafe { *(lp as *mut HWND) = h };
            return 0;
        }
        1
    }

    pub fn find() -> HWND {
        let t = wide("Assetto Corsa EVO");
        let h = unsafe { FindWindowW(std::ptr::null(), t.as_ptr()) };
        if !h.is_null() {
            return h;
        }
        let mut found: HWND = std::ptr::null_mut();
        unsafe { EnumWindows(Some(each), &mut found as *mut HWND as LPARAM) };
        found
    }

    pub fn close() -> bool {
        let h = find();
        if h.is_null() {
            return false;
        }
        unsafe { PostMessageW(h, WM_CLOSE, 0, 0) };
        true
    }

    pub fn focus() -> bool {
        let h = find();
        if h.is_null() {
            return false;
        }
        unsafe {
            ShowWindow(h, SW_RESTORE);
            SetForegroundWindow(h);
        }
        true
    }
}

#[cfg(not(windows))]
mod win {
    const TITLE: &str = "Assetto Corsa EVO";

    fn try_cmds(cmds: &[&[&str]]) -> bool {
        cmds.iter().any(|argv| {
            std::process::Command::new(argv[0]).args(&argv[1..]).output()
                .map(|o| o.status.success()).unwrap_or(false)
        })
    }

    /// best effort: no Wayland equivalent; false lets the caller kill instead
    pub fn close() -> bool {
        try_cmds(&[&["wmctrl", "-c", TITLE], &["xdotool", "search", "--name", TITLE, "windowclose"]])
    }

    pub fn focus() -> bool {
        try_cmds(&[&["wmctrl", "-a", TITLE], &["xdotool", "search", "--name", TITLE, "windowactivate", "--sync"]])
    }
}

/// WM_CLOSE the EVO window. Works when the inspector is already dead.
pub fn close_window() -> bool {
    win::close()
}

/// True if an EVO window is still on screen (on Linux: the process).
pub fn window_open() -> bool {
    #[cfg(windows)]
    { !win::find().is_null() }
    #[cfg(not(windows))]
    { !crate::procs::pids_named(&["AssettoCorsaEVO"]).is_empty() || !crate::procs::pids_named_prefix(&["assettocorsaevo"]).is_empty() }
}

/// Put EVO in front (CDP steals focus; the game pauses occluded).
pub fn focus_game() -> bool {
    win::focus()
}
