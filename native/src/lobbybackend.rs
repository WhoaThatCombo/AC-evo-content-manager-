//! A minimal local AC EVO lobby backend - ported from tools/acevo_backend.py.
//! Run as `acecm --tool acevo_backend` (the Backend page's "standalone" mode).
//!
//! The client normally talks to Kunos's backend over a TLS websocket; its
//! `-backend` flag points it here instead. Every frame is a BackendMessage
//! wrapping an Any; this answers just enough for the menu to load and lists
//! ONE server - ours, from lobby.json. Unknown requests get the matching
//! (empty) Response type, which keeps the client moving.

use std::net::SocketAddr;

use futures_util::{SinkExt, StreamExt};
use prost_reflect::{DynamicMessage, Kind, ReflectMessage, Value as PV};
use tokio_tungstenite::tungstenite::Message;

use crate::lobbyproxy::{self as lp, env, fill_entry, new, unwrap, wrap};

/// Set a scalar by dotted path ("account_pguid.a"), converting to the
/// field's declared kind; nested messages are created on the way.
fn set_path(m: &mut DynamicMessage, path: &str, v: PV) {
    let (head, rest) = match path.split_once('.') { Some((h, r)) => (h, Some(r)), None => (path, None) };
    let Some(fd) = m.descriptor().get_field_by_name(head) else { return };
    match rest {
        Some(r) => {
            let Kind::Message(md) = fd.kind() else { return };
            let mut inner = match m.get_field(&fd).into_owned() { PV::Message(x) => x, _ => DynamicMessage::new(md) };
            set_path(&mut inner, r, v);
            let _ = m.try_set_field(&fd, PV::Message(inner));
        }
        None => {
            let conv = match (&fd.kind(), &v) {
                (Kind::String, PV::String(_)) | (Kind::Bool, PV::Bool(_)) => Some(v),
                (k, PV::I64(n)) => Some(match k {
                    Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => PV::I32(*n as i32),
                    Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => PV::I64(*n),
                    Kind::Uint32 | Kind::Fixed32 => PV::U32(*n as u32),
                    Kind::Uint64 | Kind::Fixed64 => PV::U64(*n as u64),
                    Kind::Float => PV::F32(*n as f32),
                    Kind::Double => PV::F64(*n as f64),
                    Kind::Enum(_) => PV::EnumNumber(*n as i32),
                    Kind::Bool => PV::Bool(*n != 0),
                    _ => return,
                }),
                _ => None,
            };
            if let Some(c) = conv {
                let _ = m.try_set_field(&fd, c);
            }
        }
    }
}

fn get_str(m: &DynamicMessage, name: &str) -> String {
    m.get_field_by_name(name).and_then(|v| v.as_str().map(String::from)).unwrap_or_default()
}

fn get_int(m: &DynamicMessage, name: &str) -> Option<i64> {
    let v = m.get_field_by_name(name)?.into_owned();
    match v {
        PV::I32(x) => Some(x as i64), PV::I64(x) => Some(x), PV::U32(x) => Some(x as i64),
        PV::U64(x) => Some(x as i64), PV::EnumNumber(x) => Some(x as i64), _ => None,
    }
}

/// An empty account is accepted but leaves the driver unidentified, so the
/// client never asks for owned cars - fill a whole local driver.
fn account(req: &DynamicMessage) -> Option<DynamicMessage> {
    let mut r = new("GameEconomyClientResponseAccount")?;
    let sid = get_str(req, "steam_id");
    set_path(&mut r, "account_id", PV::String(if sid.is_empty() { "local".into() } else { sid }));
    set_path(&mut r, "is_new", PV::Bool(false));
    set_path(&mut r, "account_pguid.a", PV::I64(1));
    set_path(&mut r, "account_pguid.b", PV::I64(1));
    // one available driver
    if let Some(dd) = r.descriptor().get_field_by_name("driver_data") {
        if let Kind::Message(ddm) = dd.kind() {
            let mut d = match r.get_field(&dd).into_owned() { PV::Message(x) => x, _ => DynamicMessage::new(ddm) };
            if let Some(ad) = d.descriptor().get_field_by_name("available_drivers") {
                if let Kind::Message(drm) = ad.kind() {
                    let mut drv = DynamicMessage::new(drm);
                    set_path(&mut drv, "driver_pguid.a", PV::I64(2));
                    set_path(&mut drv, "driver_pguid.b", PV::I64(2));
                    set_path(&mut drv, "first_name", PV::String("Local".into()));
                    set_path(&mut drv, "last_name", PV::String("Driver".into()));
                    let _ = d.try_set_field(&ad, PV::List(vec![PV::Message(drv)]));
                }
            }
            set_path(&mut d, "current_driver_pguid.a", PV::I64(2));
            set_path(&mut d, "current_driver_pguid.b", PV::I64(2));
            let _ = r.try_set_field(&dd, PV::Message(d));
        }
    }
    for (k, v) in [("first_name", "Local"), ("last_name", "Driver"), ("shortcut_name", "LOC"), ("nickname", "Local"), ("nationality_alpha_3", "GBR")] {
        set_path(&mut r, &format!("current_driver_personal_data.{k}"), PV::String(v.into()));
    }
    set_path(&mut r, "current_driver_progression_data.current_level", PV::I64(50));
    set_path(&mut r, "current_driver_progression_data.money", PV::I64(99_999_999));
    set_path(&mut r, "current_driver_progression_data.xp_points", PV::I64(1_000_000));
    Some(r)
}

async fn entry_into(r: &mut DynamicMessage, field: &str, peer: SocketAddr, list: bool) {
    let Some(fd) = r.descriptor().get_field_by_name(field) else { return };
    let Kind::Message(ed) = fd.kind() else { return };
    let mut e = DynamicMessage::new(ed);
    fill_entry(&mut e, Some(peer)).await;
    let _ = r.try_set_field(&fd, if list { PV::List(vec![PV::Message(e)]) } else { PV::Message(e) });
}

async fn reply_for(name: &str, msg: &DynamicMessage, peer: SocketAddr) -> Option<(String, DynamicMessage)> {
    match name {
        "GameEconomyClientRequestAccount" => return account(msg).map(|r| ("GameEconomyClientResponseAccount".into(), r)),
        "RegisterRequest" => {
            let mut r = new("RegisterResponse")?;
            set_path(&mut r, "action_id", PV::I64(get_int(msg, "action_id").unwrap_or(0)));
            set_path(&mut r, "is_registered", PV::Bool(true));
            if let Some(p) = get_int(msg, "platform_type") {
                set_path(&mut r, "platform_type", PV::I64(p));
            }
            // (sic: the schema spells it procol_version)
            let proto = get_int(msg, "procol_version").or_else(|| get_int(msg, "protocol_version")).filter(|v| *v != 0);
            let ver = get_int(msg, "server_version").filter(|v| *v != 0);
            lp::set_versions(proto, ver);
            let (p, s) = lp::versions();
            println!("     (echo protocol={p} server={s})");
            return Some(("RegisterResponse".into(), r));
        }
        "MultiplayerServerListRequestServerList" => {
            let mut r = new("MultiplayerServerListResponseServerList")?;
            entry_into(&mut r, "entry", peer, true).await;
            if let Some(n) = get_int(msg, "request_no") {
                set_path(&mut r, "request_no", PV::I64(n));
            }
            return Some(("MultiplayerServerListResponseServerList".into(), r));
        }
        "MultiplayerServerListRequestServerEntry" => {
            let mut r = new("MultiplayerServerListResponseServerEntry")?;
            entry_into(&mut r, "entry", peer, false).await;
            return Some(("MultiplayerServerListResponseServerEntry".into(), r));
        }
        "MultiplayerServerListRequestConnectToServer" => {
            return new("MultiplayerServerListResponseConnectToServer").map(|r| ("MultiplayerServerListResponseConnectToServer".into(), r));
        }
        _ => {}
    }
    // generic: every Request has a Response of the same name
    if name.contains("Request") {
        let cand = name.replacen("Request", "Response", 1);
        if let Some(mut r) = new(&cand) {
            let sid = get_str(msg, "steam_id");
            set_path(&mut r, "account_id", PV::String(if sid.is_empty() { env("SERVER_ID", "local-0000-0000-0000-000000000001") } else { sid }));
            set_path(&mut r, "is_new", PV::Bool(false));
            println!("     (generic {cand})");
            return Some((cand, r));
        }
    }
    None
}

async fn handle(tcp: tokio::net::TcpStream, peer: SocketAddr, acceptor: tokio_rustls::TlsAcceptor) {
    use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
    let tls = match tokio::time::timeout(std::time::Duration::from_secs(15), acceptor.accept(tcp)).await {
        Ok(Ok(t)) => t,
        Ok(Err(e)) => { println!("    ! TLS handshake from {peer} failed: {e}"); return }
        Err(_) => { println!("    ! TLS handshake from {peer} timed out (15 s)"); return }
    };
    let mut path = String::from("?");
    let cb = |req: &Request, mut resp: Response| {
        path = req.uri().path().to_string();
        let wants = req.headers().get("Sec-WebSocket-Protocol").and_then(|v| v.to_str().ok()).unwrap_or("");
        if wants.split(',').any(|p| p.trim() == "wss") {
            resp.headers_mut().insert("Sec-WebSocket-Protocol", "wss".parse().unwrap());
        }
        Ok(resp)
    };
    let wcfg = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default().max_message_size(None).max_frame_size(None);
    let ws = match tokio_tungstenite::accept_hdr_async_with_config(tls, cb, Some(wcfg)).await {
        Ok(c) => c,
        Err(e) => { println!("    ! websocket upgrade from {peer} failed: {e}"); return }
    };
    println!("\n*** CONNECTED from {peer}  path={path}");
    let (mut tx, mut rx) = ws.split();
    while let Some(m) = rx.next().await {
        let raw = match m {
            Ok(Message::Binary(b)) => b,
            Ok(Message::Text(t)) => { let s: &str = t.as_ref(); println!("  <- TEXT {}", s.chars().take(200).collect::<String>()); continue }
            Ok(Message::Close(c)) => { println!("*** CLOSED: {c:?}"); return }
            Ok(_) => continue,
            Err(e) => { println!("*** ERROR: {e}"); return }
        };
        let Some((name, msg)) = unwrap(&raw) else { println!("  <- {}B  UNPARSEABLE", raw.len()); continue };
        println!("  <- {name}");
        let Some(msg) = msg else { println!("     (no handler - not replying)"); continue };
        match reply_for(&name, &msg, peer).await {
            Some((tn, r)) => {
                if tx.send(Message::Binary(wrap(&r, &tn).into())).await.is_err() {
                    return;
                }
                println!("  -> {tn}");
            }
            None => println!("     (no handler - not replying)"),
        }
    }
}

/// `acecm --tool acevo_backend`
pub fn run() -> i32 {
    crate::procs::exit_with_parent();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().worker_threads(2).build().expect("runtime");
    rt.block_on(async {
        let port: u16 = env("PORT", "448").parse().unwrap_or(448);
        let listen = env("BACKEND_LISTEN", "127.0.0.1");
        let acceptor = match lp::server_tls() {
            Ok(a) => a,
            Err(e) => { println!("no TLS keypair ({e}) - generate it on ACECM's Backend page, or set ACECM_CERTS to the folder holding cert.pem/key.pem"); return 1 }
        };
        println!("listening on wss://{listen}:{port}/  (any path)");
        println!("lobby.json: {}", env("LOBBY_JSON", "(none)"));
        println!("launch the game with:\n  -backend=wss://127.0.0.1:{port}/communicationNode/dev\n");
        let l = match tokio::net::TcpListener::bind((listen.as_str(), port)).await {
            Ok(l) => l,
            Err(e) => { println!("cannot bind {listen}:{port}: {e}"); return 1 }
        };
        loop {
            let Ok((tcp, peer)) = l.accept().await else { continue };
            tokio::spawn(handle(tcp, peer, acceptor.clone()));
        }
    })
}
