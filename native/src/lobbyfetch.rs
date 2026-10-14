//! Fetch the public server list straight from the lobby - no game. Ported
//! from tools/lobby_fetch.py; run as `acecm --tool lobby_fetch --out FILE
//! [--game-dir DIR]`.
//!
//! The lobby only answers a client proving it owns the game, with a Steam
//! ticket in the first frame. This mints the SAME kind the game does -
//! GetAuthTicketForWebApi("evoBackendMini") through the game's own
//! steam_api64.dll under its app id - then speaks the lobby protocol.
//!
//! ⚠ A SEPARATE, SHORT-LIVED PROCESS on purpose: while steam_api is up in a
//! process, Steam treats that process as AC EVO running (and would block the
//! real game). Here it lasts the ~2 s of one fetch.

use std::path::Path;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use prost_reflect::Value as PV;
use serde_json::json;
use tokio_tungstenite::tungstenite::Message;

use crate::lobbyproxy::{capture_list, new, unwrap, wrap};

const APP_ID: &str = "3058630";
const IDENTITY: &[u8] = b"evoBackendMini\0";
const CB_WEBAPI_TICKET: i32 = 168;

struct Steam {
    lib: libloading::Library,
    user: *mut core::ffi::c_void,
    handle: u32,
}

// the raw pointer is only an opaque Steam interface handle; the ticket is
// minted on a worker thread and shut down on ours, as the Python tool did
unsafe impl Send for Steam {}

impl Steam {
    fn load(game_dir: &Path) -> Result<Steam, String> {
        // must be in the environment before the dll initialises
        unsafe {
            std::env::set_var("SteamAppId", APP_ID);
            std::env::set_var("SteamGameId", APP_ID);
        }
        let lib = unsafe { libloading::Library::new(game_dir.join("steam_api64.dll")) }.map_err(|e| e.to_string())?;
        Ok(Steam { lib, user: std::ptr::null_mut(), handle: 0 })
    }

    fn ticket(&mut self) -> Result<(u64, String), String> {
        unsafe {
            let init: libloading::Symbol<unsafe extern "C" fn(*mut u8) -> i32> = self.lib.get(b"SteamAPI_InitFlat").map_err(|e| e.to_string())?;
            let mut err = [0u8; 1024];
            let r = init(err.as_mut_ptr());
            if r != 0 {
                let msg = String::from_utf8_lossy(&err[..err.iter().position(|&b| b == 0).unwrap_or(0)]).into_owned();
                return Err(format!("Steam is not running or not logged in (SteamAPI_InitFlat {r}: {msg})"));
            }
            let md_init: libloading::Symbol<unsafe extern "C" fn()> = self.lib.get(b"SteamAPI_ManualDispatch_Init").map_err(|e| e.to_string())?;
            md_init();
            let pipe_f: libloading::Symbol<unsafe extern "C" fn() -> i32> = self.lib.get(b"SteamAPI_GetHSteamPipe").map_err(|e| e.to_string())?;
            let pipe = pipe_f();
            let user_f: libloading::Symbol<unsafe extern "C" fn() -> *mut core::ffi::c_void> = self.lib.get(b"SteamAPI_SteamUser_v023").map_err(|e| e.to_string())?;
            self.user = user_f();
            if self.user.is_null() {
                return Err("Steam user interface unavailable".into());
            }
            let sid_f: libloading::Symbol<unsafe extern "C" fn(*mut core::ffi::c_void) -> u64> = self.lib.get(b"SteamAPI_ISteamUser_GetSteamID").map_err(|e| e.to_string())?;
            let sid = sid_f(self.user);
            let tk_f: libloading::Symbol<unsafe extern "C" fn(*mut core::ffi::c_void, *const u8) -> u32> = self.lib.get(b"SteamAPI_ISteamUser_GetAuthTicketForWebApi").map_err(|e| e.to_string())?;
            self.handle = tk_f(self.user, IDENTITY.as_ptr());
            if self.handle == 0 {
                return Err("Steam refused to issue a ticket".into());
            }
            let run: libloading::Symbol<unsafe extern "C" fn(i32)> = self.lib.get(b"SteamAPI_ManualDispatch_RunFrame").map_err(|e| e.to_string())?;
            let next: libloading::Symbol<unsafe extern "C" fn(i32, *mut u8) -> bool> = self.lib.get(b"SteamAPI_ManualDispatch_GetNextCallback").map_err(|e| e.to_string())?;
            let free: libloading::Symbol<unsafe extern "C" fn(i32)> = self.lib.get(b"SteamAPI_ManualDispatch_FreeLastCallback").map_err(|e| e.to_string())?;
            // CallbackMsg_t: i32 user, i32 id, ptr param, i32 size (24 B)
            let mut msg = [0u8; 24];
            let until = Instant::now() + Duration::from_secs(6);
            while Instant::now() < until {
                run(pipe);
                while next(pipe, msg.as_mut_ptr()) {
                    let cb = i32::from_le_bytes(msg[4..8].try_into().unwrap());
                    let par = u64::from_le_bytes(msg[8..16].try_into().unwrap()) as *const u8;
                    if cb == CB_WEBAPI_TICKET && !par.is_null() {
                        // GetTicketForWebApiResponse_t: handle, EResult, size, bytes
                        let head = std::slice::from_raw_parts(par, 12);
                        let eres = u32::from_le_bytes(head[4..8].try_into().unwrap());
                        let size = u32::from_le_bytes(head[8..12].try_into().unwrap()) as usize;
                        let raw = std::slice::from_raw_parts(par.add(12), size).to_vec();
                        free(pipe);
                        if eres != 1 {
                            return Err(format!("Steam ticket failed (EResult {eres})"));
                        }
                        return Ok((sid, raw.iter().map(|b| format!("{b:02X}")).collect()));
                    }
                    free(pipe);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err("Steam did not deliver a ticket in time".into())
        }
    }
}

impl Drop for Steam {
    fn drop(&mut self) {
        unsafe {
            if !self.user.is_null() && self.handle != 0 {
                if let Ok(cancel) = self.lib.get::<unsafe extern "C" fn(*mut core::ffi::c_void, u32)>(b"SteamAPI_ISteamUser_CancelAuthTicket") {
                    cancel(self.user, self.handle);
                }
            }
            if let Ok(shut) = self.lib.get::<unsafe extern "C" fn()>(b"SteamAPI_Shutdown") {
                shut();
            }
        }
    }
}

fn arg(args: &[String], k: &str) -> Option<String> {
    args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned()
}

/// `acecm --tool lobby_fetch --out FILE [--game-dir DIR]`
pub fn run(args: &[String]) -> i32 {
    let t0 = Instant::now();
    let out = arg(args, "--out").unwrap_or_default();
    let game_dir = arg(args, "--game-dir").or_else(|| std::env::var("ACECM_GAME_DIR").ok()).unwrap_or_default();
    let mut times = serde_json::Map::new();
    let ms = |t: Instant| t.elapsed().as_millis() as u64;
    let result = (|| -> Result<usize, String> {
        if out.is_empty() {
            return Err("--out is required".into());
        }
        if game_dir.is_empty() || !Path::new(&game_dir).join("steam_api64.dll").is_file() {
            return Err("AC EVO's folder (with steam_api64.dll) not found".into());
        }
        let mut steam = Steam::load(Path::new(&game_dir))?;
        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().worker_threads(2).build().map_err(|e| e.to_string())?;
        let t1 = Instant::now();
        let upstream = std::env::var("UPSTREAM").unwrap_or_else(|_| "wss://b.gk.sd:6990/9kB3F1CAx7mQ2zL5vN8pD4sT6yW0eU1r/000500".into());
        // ⚠ connect and mint the ticket AT THE SAME TIME (~0.6 s and
        // ~0.7-1.3 s): overlapped, only the longer one is paid
        let (ws, tk) = rt.block_on(async {
            let dial = crate::lobbyproxy::dial_upstream(&upstream, Some("WebSocket++/0.8.2"), Some("wss, ws"));
            let tk = tokio::task::spawn_blocking(move || { let r = steam.ticket(); (r, steam) });
            let ws = dial.await;
            times.insert("connect_ms".into(), json!(ms(t1)));
            let tk = tk.await.map_err(|e| e.to_string());
            (ws, tk)
        });
        let (ticket, steam) = tk?;
        times.insert("ticket_ms".into(), json!(ms(t1)));
        let (sid, ticket) = ticket?;
        let mut ws = ws?;
        let msg = rt.block_on(async {
            let mut nonce = [0u8; 8];
            let _ = getrandom::fill(&mut nonce);
            let nonce: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
            ws.send(Message::Text(format!("||42|4|{sid}|1|{nonce}|{ticket}").into())).await.map_err(|e| e.to_string())?;
            let mut reg = new("RegisterRequest").ok_or("the game's message schemas are missing")?;
            reg.set_field_by_name("identity_id", PV::String(sid.to_string()));
            let _ = reg.try_set_field_by_name("platform_type", PV::EnumNumber(1));
            let _ = reg.try_set_field_by_name("server_version", PV::I32(6));
            let _ = reg.try_set_field_by_name("procol_version", PV::I32(8));
            ws.send(Message::Binary(wrap(&reg, "RegisterRequest").into())).await.map_err(|e| e.to_string())?;
            let deadline = Instant::now() + Duration::from_secs(20);
            let mut asked = false;
            while Instant::now() < deadline {
                let left = deadline.saturating_duration_since(Instant::now()).max(Duration::from_millis(100));
                let m = tokio::time::timeout(left, ws.next()).await.map_err(|_| "the lobby did not send a server list in time")?;
                let Some(Ok(Message::Binary(b))) = m else { continue };
                let Some((name, msg)) = unwrap(&b) else { continue };
                if name == "RegisterResponse" && !asked {
                    if let Some(m) = &msg {
                        if m.get_field_by_name("is_registered").map(|v| v.as_bool() == Some(false)).unwrap_or(false) {
                            return Err("the lobby refused the login".to_string());
                        }
                    }
                    let mut req = new("MultiplayerServerListRequestServerList").ok_or("schemas missing")?;
                    let _ = req.try_set_field_by_name("request_no", PV::I32(1));
                    ws.send(Message::Binary(wrap(&req, "MultiplayerServerListRequestServerList").into())).await.map_err(|e| e.to_string())?;
                    asked = true;
                } else if name == "MultiplayerServerListResponseServerList" {
                    if let Some(m) = msg {
                        return Ok(m);
                    }
                }
            }
            Err("the lobby did not send a server list in time".to_string())
        });
        drop(steam); // CancelAuthTicket + SteamAPI_Shutdown, now
        let msg = msg?;
        times.insert("lobby_ms".into(), json!(ms(t1)));
        let t2 = Instant::now();
        let servers = capture_list(&msg);
        let n = servers.len();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let blob = json!({"ok": true, "count": n, "captured_at": now, "servers": servers, "source": "lobby"});
        let tmp = format!("{out}.part");
        std::fs::write(&tmp, crate::profiles::py_dumps(&blob)).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &out).map_err(|e| e.to_string())?;
        times.insert("write_ms".into(), json!(ms(t2)));
        Ok(n)
    })();
    let mut summary = match &result {
        Ok(n) => json!({"ok": true, "count": n}),
        Err(e) => json!({"ok": false, "error": e}),
    };
    summary["ms"] = json!(ms(t0));
    if let Some(o) = summary.as_object_mut() {
        o.extend(times);
    }
    println!("{summary}");
    if result.is_ok() { 0 } else { 1 }
}
