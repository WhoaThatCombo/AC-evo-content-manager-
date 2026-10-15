//! Fire "Go to Pitlane" from a wheel, a button box, a controller or a key
//! combination - ported from hotkey.py. The pause menu hides the button on
//! track; the command underneath (gameui::back_to_pit) has no such gate.
//!
//! ⚠ TWO input APIs: wheels/button boxes answer joyGetPosEx (DirectInput),
//! an Xbox pad reports its buttons only through XInput.
//! ⚠ Read-only: nothing is synthesised or acquired exclusively.

use serde_json::{json, Value};

const MAX_JOY: u32 = 16;
const MAX_PADS: u32 = 4;

pub const XINPUT_BUTTONS: [(u32, &str); 14] = [(0x0001, "DPad Up"), (0x0002, "DPad Down"), (0x0004, "DPad Left"),
    (0x0008, "DPad Right"), (0x0010, "Start"), (0x0020, "Back"), (0x0040, "L Stick"), (0x0080, "R Stick"),
    (0x0100, "LB"), (0x0200, "RB"), (0x1000, "A"), (0x2000, "B"), (0x4000, "X"), (0x8000, "Y")];

#[cfg(windows)]
mod win {
    use std::sync::OnceLock;

    #[repr(C)]
    struct JoyInfoEx { size: u32, flags: u32, x: u32, y: u32, z: u32, r: u32, u: u32, v: u32, buttons: u32, button_number: u32, pov: u32, r1: u32, r2: u32 }
    #[repr(C)]
    #[derive(Default)]
    struct XState { packet: u32, buttons: u16, lt: u8, rt: u8, lx: i16, ly: i16, rx: i16, ry: i16 }

    type JoyFn = unsafe extern "system" fn(u32, *mut JoyInfoEx) -> u32;
    type XFn = unsafe extern "system" fn(u32, *mut XState) -> u32;

    fn winmm() -> Option<&'static (libloading::Library, JoyFn)> {
        static L: OnceLock<Option<(libloading::Library, JoyFn)>> = OnceLock::new();
        L.get_or_init(|| unsafe {
            let lib = libloading::Library::new("winmm.dll").ok()?;
            let f: JoyFn = *lib.get::<JoyFn>(b"joyGetPosEx").ok()?;
            Some((lib, f))
        }).as_ref()
    }

    fn xinput() -> Option<&'static (libloading::Library, XFn)> {
        static L: OnceLock<Option<(libloading::Library, XFn)>> = OnceLock::new();
        L.get_or_init(|| {
            for name in ["XInput1_4.dll", "xinput1_3.dll", "XInput9_1_0.dll"] {
                unsafe {
                    if let Ok(lib) = libloading::Library::new(name) {
                        if let Ok(f) = lib.get::<XFn>(b"XInputGetState") {
                            let f = *f;
                            return Some((lib, f));
                        }
                    }
                }
            }
            None
        }).as_ref()
    }

    pub fn read_joy(id: u32) -> Option<u32> {
        let (_, f) = winmm()?;
        let mut info = JoyInfoEx { size: std::mem::size_of::<JoyInfoEx>() as u32, flags: 0x80, x: 0, y: 0, z: 0, r: 0, u: 0, v: 0, buttons: 0, button_number: 0, pov: 0, r1: 0, r2: 0 };
        if unsafe { f(id, &mut info) } != 0 {
            return None;
        }
        Some(info.buttons)
    }

    pub fn read_pad(id: u32) -> Option<u32> {
        let (_, f) = xinput()?;
        let mut st = XState::default();
        if unsafe { f(id, &mut st) } != 0 {
            return None;
        }
        Some(st.buttons as u32)
    }

    pub fn key_down(vk: i32) -> bool {
        unsafe { (windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(vk) as u16 & 0x8000) != 0 }
    }

    pub fn game_focused() -> bool {
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW};
        unsafe {
            let h = GetForegroundWindow();
            if h.is_null() {
                return false;
            }
            let mut buf = [0u16; 256];
            let n = GetWindowTextW(h, buf.as_mut_ptr(), 256);
            String::from_utf16_lossy(&buf[..n.max(0) as usize]).to_lowercase().contains("assetto corsa evo")
        }
    }
}

#[cfg(not(windows))]
mod win {
    pub fn read_joy(_id: u32) -> Option<u32> { None }
    pub fn read_pad(_id: u32) -> Option<u32> { None }
    pub fn key_down(_vk: i32) -> bool { false }
    pub fn game_focused() -> bool { false }
}

pub fn read(kind: &str, dev: u32) -> Option<u32> {
    if kind == "pad" { win::read_pad(dev) } else { win::read_joy(dev) }
}

/// Everything answering right now, and what it is holding.
pub fn devices() -> Value {
    let mut out = Vec::new();
    for jid in 0..MAX_JOY {
        let Some(mask) = win::read_joy(jid) else { continue };
        let pressed: Vec<u32> = (0..32).filter(|i| mask & (1 << i) != 0).map(|i| i + 1).collect();
        out.push(json!({"kind": "joy", "id": jid, "buttons": mask, "label": format!("Joystick {jid}"), "pressed": pressed}));
    }
    for pid in 0..MAX_PADS {
        let Some(mask) = win::read_pad(pid) else { continue };
        let pressed: Vec<&str> = XINPUT_BUTTONS.iter().filter(|(b, _)| mask & b != 0).map(|(_, n)| *n).collect();
        out.push(json!({"kind": "pad", "id": pid, "buttons": mask, "label": format!("Controller {}", pid + 1), "pressed": pressed}));
    }
    json!(out)
}

/// Wait for a button and report which one (anything already held when this
/// starts is ignored - a paddle resting on a switch must not be captured).
pub fn capture(seconds: f64) -> Value {
    let snap = |d: &Value| (d["kind"].as_str().unwrap_or("").to_string(), d["id"].as_u64().unwrap_or(0));
    let mut start: std::collections::HashMap<(String, u64), u64> = Default::default();
    for d in devices().as_array().cloned().unwrap_or_default() {
        start.insert(snap(&d), d["buttons"].as_u64().unwrap_or(0));
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs_f64(seconds.max(0.5));
    while std::time::Instant::now() < deadline {
        for d in devices().as_array().cloned().unwrap_or_default() {
            let key = snap(&d);
            let now = d["buttons"].as_u64().unwrap_or(0);
            let was = *start.get(&key).unwrap_or(&0);
            let fresh = now & !was;
            if fresh != 0 {
                let bit = fresh & fresh.wrapping_neg();
                let label = d["label"].as_str().unwrap_or("");
                if key.0 == "pad" {
                    let name = XINPUT_BUTTONS.iter().find(|(b, _)| *b as u64 == bit).map(|(_, n)| n.to_string()).unwrap_or(format!("{bit:#x}"));
                    return json!({"ok": true, "kind": "pad", "id": key.1, "mask": bit, "label": format!("{label} · {name}")});
                }
                let num = 64 - bit.leading_zeros();
                return json!({"ok": true, "kind": "joy", "id": key.1, "mask": bit, "label": format!("{label} · button {num}")});
            }
            start.entry(key).or_insert(now);
        }
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    json!({"ok": false, "error": "no button pressed"})
}

/// 'ctrl+y' -> (modifier vks, key vk); None for an unknown name.
fn parse_combo(text: &str) -> Option<(Vec<i32>, i32)> {
    let parts: Vec<String> = text.split('+').map(|p| p.trim().to_lowercase()).filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return None;
    }
    let (mut mods, mut key) = (Vec::new(), None);
    for p in parts {
        match p.as_str() {
            "ctrl" => mods.push(0x11),
            "shift" => mods.push(0x10),
            "alt" => mods.push(0x12),
            s if s.len() == 1 && s.chars().all(|c| c.is_ascii_alphanumeric()) => key = Some(s.to_ascii_uppercase().as_bytes()[0] as i32),
            s if s.starts_with('f') && s[1..].parse::<i32>().map(|n| (1..=12).contains(&n)).unwrap_or(false) => key = Some(0x6F + s[1..].parse::<i32>().unwrap()),
            _ => return None,
        }
    }
    key.map(|k| (mods, k))
}

/// Is this combination held right now? (GetAsyncKeyState: nothing is
/// reserved system-wide, unlike RegisterHotKey.)
fn combo_held(text: &str) -> bool {
    match parse_combo(text) {
        Some((mods, key)) => mods.iter().all(|m| win::key_down(*m)) && win::key_down(key),
        None => false,
    }
}

static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn fire() {
    let r = crate::gameui::back_to_pit();
    crate::logs::info(format!("pit button: {}", if r["ok"] == json!(true) { "sent".to_string() } else { r["error"].as_str().unwrap_or("").to_string() }));
}

/// Poll the bound button / key combination and fire on the PRESS (edge,
/// not level: a held button must not teleport ten times).
/// ⚠ The keyboard combo is only honoured while the game is in front (ctrl+y
/// is redo in most editors).
pub fn watch() {
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        let mut last: std::collections::HashMap<String, bool> = Default::default();
        let mut cfg = crate::config::load();
        let mut cfg_at = std::time::Instant::now();
        loop {
            // the binding changes on a human timescale: re-read once a second
            if cfg_at.elapsed().as_secs() >= 1 {
                cfg = crate::config::load();
                cfg_at = std::time::Instant::now();
            }
            let kind = cfg.get("pit_btn_kind").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let mask = cfg.get("pit_btn_mask").and_then(|v| v.as_u64()).unwrap_or(0);
            let dev = match cfg.get("pit_btn_id") {
                Some(Value::Number(n)) => n.as_u64(),
                Some(Value::String(s)) if !s.is_empty() => s.parse().ok(),
                _ => None,
            };
            if !kind.is_empty() && mask != 0 {
                if let Some(dev) = dev {
                    if let Some(now) = read(&kind, dev as u32) {
                        let held = now as u64 & mask != 0;
                        let key = format!("{kind}:{dev}:{mask}");
                        if held && !last.get(&key).copied().unwrap_or(false) {
                            fire();
                        }
                        last.insert(key, held);
                    }
                }
            }
            let combo = cfg.get("pit_key").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if !combo.is_empty() {
                let held = combo_held(&combo) && win::game_focused();
                if held && !last.get("kbd").copied().unwrap_or(false) {
                    fire();
                }
                last.insert("kbd".into(), held);
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    });
}
