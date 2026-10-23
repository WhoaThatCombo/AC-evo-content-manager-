//! The lobby proxy - ported from tools/acevo_proxy.py (+ the wire helpers
//! of acevo_backend.py and join_push.py). Run as its own process:
//! `acecm --tool acevo_proxy`, exactly where the Python one ran.
//!
//!     game --TLS ws--> this proxy :448 --TLS ws--> wss://b.gk.sd:6990/...
//!
//! Everything is relayed untouched (account, garage, the genuine browser),
//! except:
//!   * the server list coming DOWN is captured in full (for ACECM's own
//!     browser), trimmed for the game (930 rows froze its UI for 20-40 s),
//!     and gets OUR server put FIRST;
//!   * a RequestServerEntry going UP for our own address is answered here
//!     (Kunos has never heard of a 127.0.0.1 server).
//! A control port (127.0.0.1:8093) reports state, serves the captured list
//! and pushes "go to server" (join:<ip>:<tcp>) at the connected client.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use prost::Message as _;
use prost_reflect::{DynamicMessage, ReflectMessage, Value as PV};
use serde_json::{json, Map, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::protos;

const TYPE_PREFIX: &str = "type.googleapis.com/";

pub(crate) fn env(k: &str, d: &str) -> String {
    std::env::var(k).ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| d.to_string())
}

// ------------------------------------------------------------- wire ------
/// BackendMessage { Any msg = 1; bool unwrap_message = 2; } -> (type, message)
pub fn unwrap(raw: &[u8]) -> Option<(String, Option<DynamicMessage>)> {
    let env = protos::decode("BackendMessage", raw).ok()?;
    let any = match env.get_field_by_name("msg")?.into_owned() { PV::Message(m) => m, _ => return None };
    let url = match any.get_field_by_name("type_url")?.into_owned() { PV::String(s) => s, _ => String::new() };
    let value = match any.get_field_by_name("value")?.into_owned() { PV::Bytes(b) => b, _ => Default::default() };
    let name = url.rsplit('/').next().unwrap_or("").to_string();
    if name.is_empty() || !protos::has(&name) {
        return Some((name, None));
    }
    Some((name.clone(), protos::decode(&name, &value).ok()))
}

pub fn wrap(msg: &DynamicMessage, type_name: &str) -> Vec<u8> {
    let Some(env_d) = protos::find("BackendMessage") else { return vec![] };
    let mut env = DynamicMessage::new(env_d);
    let any_d = env.descriptor().get_field_by_name("msg").and_then(|f| f.kind().as_message().cloned());
    let Some(any_d) = any_d else { return vec![] };
    let mut any = DynamicMessage::new(any_d);
    any.set_field_by_name("type_url", PV::String(format!("{TYPE_PREFIX}{type_name}")));
    any.set_field_by_name("value", PV::Bytes(msg.encode_to_vec().into()));
    env.set_field_by_name("msg", PV::Message(any));
    env.set_field_by_name("unwrap_message", PV::Bool(false));
    env.encode_to_vec()
}

pub fn new(name: &str) -> Option<DynamicMessage> {
    protos::find(name).map(DynamicMessage::new)
}

static VERSIONS: Mutex<Option<(i64, i64)>> = Mutex::new(None);

/// (protocol, server): the env defaults until a RegisterRequest says otherwise.
pub(crate) fn versions() -> (i64, i64) {
    VERSIONS.lock().unwrap().unwrap_or_else(|| (env("PROTOCOL_VERSION", "8").parse().unwrap_or(8),
                                                env("SERVER_VERSION", "6").parse().unwrap_or(6)))
}

pub(crate) fn set_versions(protocol: Option<i64>, server: Option<i64>) {
    let (p, s) = versions();
    *VERSIONS.lock().unwrap() = Some((protocol.unwrap_or(p), server.unwrap_or(s)));
}

// ------------------------------------------------------------- lobby -----
fn lobby_json() -> PathBuf {
    let p = env("LOBBY_JSON", "");
    if !p.is_empty() {
        return PathBuf::from(p);
    }
    crate::lobby::path()
}

fn lobby() -> Map<String, Value> {
    std::fs::read(lobby_json()).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.as_object().cloned()).unwrap_or_default()
}

fn lb_s(lb: &Map<String, Value>, k: &str) -> String {
    lb.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string()
}

fn lb_i(lb: &Map<String, Value>, k: &str, d: i64) -> i64 {
    match lb.get(k) {
        Some(Value::Number(n)) => n.as_i64().filter(|x| *x != 0).unwrap_or(d),
        Some(Value::String(s)) => s.parse().ok().filter(|x: &i64| *x != 0).unwrap_or(d),
        _ => d,
    }
}

fn lan_ip(lb: &Map<String, Value>) -> String {
    let l = env("LAN_IP", "");
    if !l.is_empty() {
        return l;
    }
    let l = lb_s(lb, "lan_ip");
    if !l.is_empty() {
        return l;
    }
    crate::misc::lan_ipv4()
}

fn our_ips(lb: &Map<String, Value>) -> HashSet<String> {
    let mut s: HashSet<String> = env("OUR_IPS", "").split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect();
    for x in ["127.0.0.1", "localhost", "::1"] {
        s.insert(x.into());
    }
    for x in [env("SERVER_IP", "127.0.0.1"), env("LAN_IP", ""), lb_s(lb, "lan_ip"), lan_ip(lb)] {
        if !x.is_empty() {
            s.insert(x);
        }
    }
    s
}

fn is_our_server(ip: &str) -> bool {
    !ip.trim().is_empty() && our_ips(&lobby()).contains(ip.trim())
}

/// Loopback for a local client, the LAN address for everyone else.
fn advertise_ip(peer: Option<SocketAddr>, lb: &Map<String, Value>) -> String {
    match peer {
        Some(p) if !p.ip().to_canonical().is_loopback() => {
            let l = lan_ip(lb);
            if !l.is_empty() { l } else { env("SERVER_IP", "127.0.0.1") }
        }
        _ => "127.0.0.1".into(),
    }
}

async fn live_players(lb: &Map<String, Value>) -> usize {
    let port = lb_i(lb, "http_port", 0);
    let url = if port != 0 { format!("http://127.0.0.1:{port}/") } else { env("SERVER_HTTP", "http://127.0.0.1:8080/") };
    let Ok(c) = reqwest::Client::builder().timeout(std::time::Duration::from_secs(2)).build() else { return 0 };
    let Ok(r) = c.get(url).send().await else { return 0 };
    let Ok(t) = r.text().await else { return 0 };
    serde_json::from_str::<Value>(&t).ok().and_then(|v| v.get("clients").and_then(|c| c.as_u64())).unwrap_or(0) as usize
}

fn tagged(name: &str) -> String {
    let tag = std::env::var("LOCAL_TAG").unwrap_or_else(|_| "[ACECM]".into());
    let n = name.trim();
    if tag.is_empty() || n.to_lowercase().contains(&tag.to_lowercase()) {
        return n.to_string();
    }
    format!("{tag} {n}").trim().to_string()
}

pub(crate) fn set_if(m: &mut DynamicMessage, name: &str, v: PV) {
    if m.descriptor().get_field_by_name(name).is_some() {
        let _ = m.try_set_field_by_name(name, v);
    }
}

pub(crate) fn enum_or_int(m: &DynamicMessage, name: &str, n: i64) -> PV {
    match m.descriptor().get_field_by_name(name).map(|f| f.kind()) {
        Some(prost_reflect::Kind::Enum(_)) => PV::EnumNumber(n as i32),
        Some(prost_reflect::Kind::Int64) | Some(prost_reflect::Kind::Sint64) | Some(prost_reflect::Kind::Sfixed64) => PV::I64(n),
        Some(prost_reflect::Kind::Uint32) | Some(prost_reflect::Kind::Fixed32) => PV::U32(n as u32),
        Some(prost_reflect::Kind::Uint64) | Some(prost_reflect::Kind::Fixed64) => PV::U64(n as u64),
        _ => PV::I32(n as i32),
    }
}

/// Our own server as a MultiplayerServerListEntry, from lobby.json.
pub async fn fill_entry(e: &mut DynamicMessage, peer: Option<SocketAddr>) {
    let lb = lobby();
    let s = |k: &str, d: &str| { let v = lb_s(&lb, k); if v.is_empty() { d.to_string() } else { v } };
    set_if(e, "server_id", PV::String(s("server_id", &env("SERVER_ID", "local-0000-0000-0000-000000000001"))));
    set_if(e, "server_ip", PV::String(advertise_ip(peer, &lb)));
    set_if(e, "server_name", PV::String(tagged(&s("server_name", &env("SERVER_NAME", "RIP acedit (local)")))));
    for (f, k, d) in [("server_tcp_port", "tcp_port", "SERVER_TCP"), ("server_udp_port", "udp_port", "SERVER_UDP")] {
        let v = lb_i(&lb, k, env(d, "9700").parse().unwrap_or(9700));
        let pv = enum_or_int(e, f, v);
        set_if(e, f, pv);
    }
    set_if(e, "track", PV::String(s("track", &env("TRACK", ""))));
    set_if(e, "layout", PV::String(s("layout", &env("LAYOUT", ""))));
    set_if(e, "event_name", PV::String(s("event_name", "")));
    // echo the versions the client registered with (standalone backend):
    // hardcoding 8/6 is what makes an entry vanish after a game update
    let (pv, sv) = versions();
    for (f, v) in [("protocol_version", pv),
                   ("server_version", sv),
                   ("game_mode_type", lb_i(&lb, "game_mode_type", 10)),
                   ("max_players", lb_i(&lb, "max_players", 90))] {
        let pv = enum_or_int(e, f, v);
        set_if(e, f, pv);
    }
    let n = live_players(&lb).await;
    set_if(e, "players", PV::List((1..=n).map(|i| PV::String(format!("driver {i}"))).collect()));
    let b = |k: &str| match lb.get(k) { Some(Value::Bool(x)) => *x, Some(Value::String(x)) => !x.is_empty(), _ => false };
    set_if(e, "driver_password", PV::Bool(b("driver_password")));
    set_if(e, "spectator_password", PV::Bool(b("spectator_password")));
    let mode = s("game_mode", "PRACTICE").replace("GameModeType_", "");
    set_if(e, "current_session", PV::String(crate::trackdeploy::title(&mode.replace('_', " "))));
    set_if(e, "time_of_day", PV::String(s("time_of_day", &env("TIME_OF_DAY", "13:00"))));
    set_if(e, "is_car_eligible", PV::Bool(true));
    let mut names: Vec<String> = lb.get("cars").and_then(|c| c.as_array()).map(|a| a.iter()
        .filter_map(|c| c.as_str().filter(|s| !s.is_empty()).map(String::from)).collect()).unwrap_or_default();
    if names.is_empty() {
        // no profile snapshot: every preset in the server's cars.json
        let path = std::env::var("CARS_JSON").ok().filter(|p| !p.is_empty())
            .unwrap_or_else(|| crate::content::catalog_path("cars.json"));
        names = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| v.get("cars").and_then(|c| c.as_array()).cloned()).unwrap_or_default().iter()
            .filter_map(|c| c.get("name").and_then(|n| n.as_str()).map(String::from))
            .filter(|n| n.rsplit_once("_mech_").map(|(h, t)| !h.is_empty() && !t.is_empty() && t.chars().all(|c| c.is_ascii_digit())).unwrap_or(false))
            .collect();
    }
    if let Some(fd) = e.descriptor().get_field_by_name("allowed_cars_list_full") {
        if let Some(cd) = fd.kind().as_message().cloned() {
            let items: Vec<PV> = names.iter().map(|n| {
                let mut a = DynamicMessage::new(cd.clone());
                set_if(&mut a, "car_name", PV::String(n.clone()));
                let z = enum_or_int(&a, "ballast", 0);
                set_if(&mut a, "ballast", z);
                let z = enum_or_int(&a, "restrictor", 0);
                set_if(&mut a, "restrictor", z);
                PV::Message(a)
            }).collect();
            let _ = e.try_set_field(&fd, PV::List(items));
        }
    }
    // ⚠ only where it is a plain string: in the current schema it is a
    // repeated field, which the Python version's assignment never filled
    // (one comma-joined element is not what the game sends)
    if e.descriptor().get_field_by_name("allowed_cars_list").map(|f| !f.is_list()).unwrap_or(false) {
        set_if(e, "allowed_cars_list", PV::String(names.join(",")));
    }
}

// ----------------------------------------------------------- the list ----
pub struct Shared {
    pub list: Mutex<(u64, Vec<Value>)>,
    pub stats: Mutex<Map<String, Value>>,
    pub client: Mutex<Option<mpsc::UnboundedSender<Message>>>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn pv_json(v: &PV) -> Option<Value> {
    Some(match v {
        PV::Bool(b) => json!(b),
        PV::I32(n) => json!(n), PV::I64(n) => json!(n), PV::U32(n) => json!(n), PV::U64(n) => json!(n),
        PV::F32(f) => json!(*f as f64), PV::F64(f) => json!(f),
        PV::String(s) => json!(s),
        PV::EnumNumber(n) => json!(n),
        PV::Bytes(_) | PV::Message(_) | PV::List(_) | PV::Map(_) => return None,
    })
}

/// Flatten a ResponseServerList for ACECM's browser - every field of every
/// entry in declaration order, repeated ones as counts, EXCEPT the car lists
/// (which cars a server allows decides whether you can join at all).
pub fn capture_list(msg: &DynamicMessage) -> Vec<Value> {
    let entries = match msg.get_field_by_name("entry").map(|v| v.into_owned()) { Some(PV::List(l)) => l, _ => vec![] };
    let mut out = vec![];
    for e in entries {
        let PV::Message(e) = e else { continue };
        let mut d = Map::new();
        for f in e.descriptor().fields() {
            let v = e.get_field(&f).into_owned();
            if f.is_list() {
                let PV::List(items) = &v else { continue };
                let j = match f.name() {
                    "allowed_cars_list" => json!(items.iter().filter_map(|x| x.as_str().map(String::from)).collect::<Vec<_>>()),
                    "allowed_cars_list_full" => json!(items.iter().map(|x| match x {
                        PV::Message(m) => m.get_field_by_name("car_name").and_then(|c| c.as_str().map(String::from)).unwrap_or_default(),
                        _ => String::new(),
                    }).collect::<Vec<_>>()),
                    _ => json!(items.len()),
                };
                d.insert(f.name().into(), j);
                continue;
            }
            if f.is_map() {
                // python counted maps like any repeated field
                if let PV::Map(m) = &v {
                    d.insert(f.name().into(), json!(m.len()));
                }
                continue;
            }
            if let Some(j) = pv_json(&v) {
                d.insert(f.name().into(), j);
            }
        }
        out.push(Value::Object(d));
    }
    out
}

fn list_cap() -> usize {
    env("ACECM_LIST_CAP", "150").parse().unwrap_or(150)
}

/// Addresses the game list must keep: the join pin (10 min) + favourites.
fn keep_addresses() -> HashSet<String> {
    let dir = lobby_json().parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let mut keep = HashSet::new();
    for f in ["join_pin.json", "favourites.json"] {
        let Some(data) = std::fs::read(dir.join(f)).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else { continue };
        let items = match data {
            Value::Object(o) => {
                let at = o.get("at").and_then(|v| v.as_f64()).unwrap_or(0.0);
                if (now() as f64) - at > 600.0 { continue }
                vec![Value::Object(o)]
            }
            Value::Array(a) => a,
            _ => vec![],
        };
        for x in items {
            let ip = x.get("ip").map(|v| match v { Value::String(s) => s.clone(), o => o.to_string() }).unwrap_or_default();
            let tcp = x.get("tcp").map(|v| match v { Value::String(s) => s.clone(), Value::Null => String::new(), o => o.to_string() }).unwrap_or_default();
            if !ip.is_empty() && !tcp.is_empty() {
                keep.insert(format!("{ip}:{tcp}"));
            }
        }
    }
    keep
}

fn trim(msg: &mut DynamicMessage) {
    let Some(PV::List(entries)) = msg.get_field_by_name("entry").map(|v| v.into_owned()) else { return };
    let cap = list_cap();
    if cap == 0 || entries.len() <= cap {
        return;
    }
    let keep = keep_addresses();
    let mut rows: Vec<(u8, i64, usize, PV)> = entries.into_iter().enumerate().map(|(i, e)| {
        let (addr, players) = match &e {
            PV::Message(m) => (format!("{}:{}",
                m.get_field_by_name("server_ip").and_then(|v| v.as_str().map(String::from)).unwrap_or_default(),
                m.get_field_by_name("server_tcp_port").map(|v| match v.into_owned() { PV::I32(n) => n as i64, PV::U32(n) => n as i64, PV::I64(n) => n, PV::U64(n) => n as i64, _ => 0 }).unwrap_or(0)),
                m.get_field_by_name("players").map(|v| match v.into_owned() { PV::List(l) => l.len() as i64, _ => 0 }).unwrap_or(0)),
            _ => (String::new(), 0),
        };
        (if keep.contains(&addr) { 0 } else { 1 }, -players, i, e)
    }).collect();
    let n = rows.len();
    rows.sort_by(|a, b| (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2)));
    let pinned = rows.iter().filter(|r| r.0 == 0).count();
    rows.truncate(cap.max(pinned));
    rows.sort_by_key(|r| r.2); // keep Kunos' original order
    let kept: Vec<PV> = rows.into_iter().map(|r| r.3).collect();
    println!("  trimmed list {n} -> {} for the game UI", kept.len());
    msg.set_field_by_name("entry", PV::List(kept));
}

/// Capture, trim, and put our server FIRST in a list going down.
async fn add_local_entry(sh: &Shared, raw: &[u8], peer: Option<SocketAddr>) -> (Vec<u8>, Option<String>) {
    let Some((name, msg)) = unwrap(raw) else { return (raw.to_vec(), None) };
    if name != "MultiplayerServerListResponseServerList" {
        return (raw.to_vec(), Some(name));
    }
    let Some(mut msg) = msg else { return (raw.to_vec(), Some(name)) };
    *sh.list.lock().unwrap() = (now(), capture_list(&msg));
    trim(&mut msg);
    let entry_d = msg.descriptor().get_field_by_name("entry").and_then(|f| f.kind().as_message().cloned());
    let Some(entry_d) = entry_d else { return (raw.to_vec(), Some(name)) };
    let mut ours = DynamicMessage::new(entry_d);
    fill_entry(&mut ours, peer).await;
    let rest = match msg.get_field_by_name("entry").map(|v| v.into_owned()) { Some(PV::List(l)) => l, _ => vec![] };
    let mut all = vec![PV::Message(ours)];
    all.extend(rest);
    msg.set_field_by_name("entry", PV::List(all));
    bump(sh, "injected");
    (wrap(&msg, &name), Some(name))
}

fn bump(sh: &Shared, k: &str) {
    let mut s = sh.stats.lock().unwrap();
    let n = s.get(k).and_then(|v| v.as_u64()).unwrap_or(0) + 1;
    s.insert(k.into(), json!(n));
}

// ---------------------------------------------------------- TLS + ws -----
/// The roots the real Kunos backend is checked against: the Windows trust
/// store (so a corporate proxy's root also works), else the Mozilla list.
/// `b.gk.sd` presents a Let's Encrypt certificate for its own name.
fn upstream_roots() -> Arc<rustls::RootCertStore> {
    static ROOTS: std::sync::OnceLock<Arc<rustls::RootCertStore>> = std::sync::OnceLock::new();
    ROOTS.get_or_init(|| {
        let mut r = rustls::RootCertStore::empty();
        for c in rustls_native_certs::load_native_certs().certs {
            let _ = r.add(c);
        }
        if r.is_empty() {
            r.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        }
        Arc::new(r)
    }).clone()
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// TLS to the real Kunos backend, VERIFIED: the game's session traffic goes
/// through here, so an unverified link would let anyone on the network path
/// impersonate the lobby. permessage-deflate stays OFF - Kunos's is not
/// always a valid zlib stream.
pub async fn dial_upstream(url: &str, ua: Option<&str>, subprotocols: Option<&str>)
    -> Result<tokio_tungstenite::WebSocketStream<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>, String> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut req = url.into_client_request().map_err(|e| e.to_string())?;
    if let Some(ua) = ua {
        req.headers_mut().insert("User-Agent", ua.parse().unwrap());
    }
    if let Some(p) = subprotocols {
        req.headers_mut().insert("Sec-WebSocket-Protocol", p.parse().unwrap());
    }
    let host = req.uri().host().unwrap_or("").to_string();
    let port = req.uri().port_u16().unwrap_or(443);
    let tcp = tokio::net::TcpStream::connect((host.as_str(), port)).await.map_err(|e| e.to_string())?;
    let cfg = rustls::ClientConfig::builder_with_provider(provider()).with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?.with_root_certificates(upstream_roots()).with_no_client_auth();
    let name = rustls::pki_types::ServerName::try_from(host.clone()).map_err(|e| e.to_string())?;
    let tls = tokio_rustls::TlsConnector::from(Arc::new(cfg)).connect(name, tcp).await.map_err(|e| e.to_string())?;
    let wcfg = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default().max_message_size(None).max_frame_size(None);
    let (ws, _) = tokio_tungstenite::client_async_with_config(req, tls, Some(wcfg)).await.map_err(|e| e.to_string())?;
    Ok(ws)
}

pub(crate) fn server_tls() -> Result<tokio_rustls::TlsAcceptor, String> {
    let dir = PathBuf::from(env("ACECM_CERTS", &crate::backend::certs_dir().to_string_lossy()));
    let cert_pem = std::fs::read(dir.join("cert.pem")).map_err(|e| format!("cert.pem: {e}"))?;
    let key_pem = std::fs::read(dir.join("key.pem")).map_err(|e| format!("key.pem: {e}"))?;
    let certs: Vec<_> = rustls_pemfile::certs(&mut cert_pem.as_slice()).filter_map(|c| c.ok()).collect();
    let key = rustls_pemfile::private_key(&mut key_pem.as_slice()).map_err(|e| e.to_string())?.ok_or("no private key")?;
    let cfg = rustls::ServerConfig::builder_with_provider(provider()).with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?.with_no_client_auth().with_single_cert(certs, key).map_err(|e| e.to_string())?;
    Ok(tokio_rustls::TlsAcceptor::from(Arc::new(cfg)))
}

async fn handle(sh: Arc<Shared>, tcp: tokio::net::TcpStream, peer: SocketAddr, acceptor: tokio_rustls::TlsAcceptor, upstream_url: String) {
    use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
    let tls = match tokio::time::timeout(std::time::Duration::from_secs(15), acceptor.accept(tcp)).await {
        Ok(Ok(t)) => t,
        Ok(Err(e)) => { println!("    ! TLS handshake from {peer} failed: {e}"); return }
        Err(_) => { println!("    ! TLS handshake from {peer} timed out (15 s)"); return }
    };
    println!("    TLS done with {peer}");
    // ⚠ the real client asks for Sec-WebSocket-Protocol: wss - echo it, or
    // WebSocket++ can sit in CONNECTING
    let cb = |req: &Request, mut resp: Response| {
        let wants = req.headers().get("Sec-WebSocket-Protocol").and_then(|v| v.to_str().ok()).unwrap_or("");
        if wants.split(',').any(|p| p.trim() == "wss") {
            resp.headers_mut().insert("Sec-WebSocket-Protocol", "wss".parse().unwrap());
        }
        Ok(resp)
    };
    let wcfg = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default().max_message_size(None).max_frame_size(None);
    let client = match tokio_tungstenite::accept_hdr_async_with_config(tls, cb, Some(wcfg)).await {
        Ok(c) => c,
        Err(e) => { println!("    ! websocket upgrade from {peer} failed: {e}"); return }
    };
    println!("\n*** CLIENT CONNECTED {peer}");
    println!("    dialling upstream {upstream_url}");
    let upstream = match dial_upstream(&upstream_url, None, None).await {
        Ok(u) => u,
        Err(e) => { println!("*** proxy error: upstream: {e}"); return }
    };
    println!("    upstream connected - relaying");
    let (mut c_tx, mut c_rx) = client.split();
    let (mut u_tx, mut u_rx) = upstream.split();
    // everything bound for the client goes through one channel, so the
    // control port can push "go to server" between relayed frames
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
    *sh.client.lock().unwrap() = Some(tx.clone());
    let writer = tokio::spawn(async move {
        while let Some(m) = rx.recv().await {
            if c_tx.send(m).await.is_err() {
                break;
            }
        }
    });
    let sh_up = sh.clone();
    let tx_up = tx.clone();
    let up = async move {
        while let Some(Ok(m)) = c_rx.next().await {
            bump(&sh_up, "up");
            match m {
                Message::Text(t) => {
                    let s: &str = t.as_ref();
                    println!("  UP   TEXT {}...", s.chars().take(60).collect::<String>());
                    if u_tx.send(Message::Text(t)).await.is_err() { break }
                }
                Message::Binary(b) => {
                    let parsed = unwrap(&b);
                    if let Some((name, Some(msg))) = &parsed {
                        if name == "MultiplayerServerListRequestServerEntry" {
                            let ip = msg.get_field_by_name("server_ip").and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
                            if is_our_server(&ip) {
                                if let Some(mut r) = new("MultiplayerServerListResponseServerEntry") {
                                    if let Some(ed) = r.descriptor().get_field_by_name("entry").and_then(|f| f.kind().as_message().cloned()) {
                                        let mut e = DynamicMessage::new(ed);
                                        fill_entry(&mut e, Some(peer)).await;
                                        r.set_field_by_name("entry", PV::Message(e));
                                    }
                                    let _ = tx_up.send(Message::Binary(wrap(&r, "MultiplayerServerListResponseServerEntry").into()));
                                    bump(&sh_up, "answered");
                                    println!("  ANS  {name} for {ip} (locally)");
                                    continue;
                                }
                            }
                        }
                    }
                    if let Some((name, _)) = &parsed {
                        println!("  UP   {name}");
                    }
                    if u_tx.send(Message::Binary(b)).await.is_err() { break }
                }
                Message::Close(c) => { let _ = u_tx.send(Message::Close(c)).await; break }
                // pings are answered by each socket itself, never relayed
                _ => {}
            }
        }
    };
    let sh_dn = sh.clone();
    let down = async move {
        while let Some(Ok(m)) = u_rx.next().await {
            bump(&sh_dn, "down");
            match m {
                Message::Binary(b) => {
                    let (out, name) = add_local_entry(&sh_dn, &b, Some(peer)).await;
                    if name.as_deref() == Some("MultiplayerServerListResponseServerList") {
                        println!("  DOWN MultiplayerServerListResponseServerList  (+1 local entry)");
                    }
                    if tx.send(Message::Binary(out.into())).is_err() { break }
                }
                Message::Close(c) => { let _ = tx.send(Message::Close(c)); break }
                Message::Text(t) => { if tx.send(Message::Text(t)).is_err() { break } }
                _ => {}
            }
        }
    };
    tokio::select! { _ = up => {}, _ = down => {} }
    writer.abort();
    *sh.client.lock().unwrap() = None;
    let s = sh.stats.lock().unwrap().clone();
    println!("*** closed. up={} down={} injected={} answered={}", s["up"], s["down"], s["injected"], s["answered"]);
}

// ---------------------------------------------------------- control ------
fn join_url(target: &str, tcp: Option<i64>, udp: Option<i64>, password: &str) -> String {
    let t = target.trim();
    if t.starts_with("join:") {
        return t.to_string();
    }
    if let Some(tcp) = tcp.filter(|x| *x != 0) {
        let pw = if password.is_empty() { String::new() } else { format!("|{password}") };
        let u = udp.filter(|x| *x != 0).unwrap_or(tcp);
        return format!("join:{t}:{tcp}{pw} :{u}");
    }
    format!("join:{t}")
}

fn build_join(target: &str, shape: &str, tcp: Option<i64>, udp: Option<i64>, password: &str) -> Option<(Vec<u8>, String)> {
    let url = join_url(target, tcp, udp, password);
    let mut inner = new("MultiplayerResponseGoToServer")?;
    inner.set_field_by_name("url", PV::String(url.clone()));
    if shape == "bare" {
        return Some((wrap(&inner, "MultiplayerResponseGoToServer"), url));
    }
    let mut cmds = new("MultiplayerServerListCommands")?;
    cmds.set_field_by_name("response_go_to_server", PV::Message(inner));
    Some((wrap(&cmds, "MultiplayerServerListCommands"), url))
}

fn jnum(v: Option<&Value>) -> Option<i64> {
    match v { Some(Value::Number(n)) => n.as_i64(), Some(Value::String(s)) => s.parse().ok(), _ => None }
}

async fn control(sh: Arc<Shared>, port: u16) {
    use axum::routing::{get, post};
    let s1 = sh.clone();
    let s2 = sh.clone();
    let s3 = sh.clone();
    let app = axum::Router::new()
        .route("/servers", get(move || { let sh = s1.clone(); async move {
            let (at, list) = sh.list.lock().unwrap().clone();
            axum::Json(json!({"count": list.len(), "captured_at": at, "servers": list}))
        }}))
        .route("/join", post(move |body: axum::body::Bytes| { let sh = s2.clone(); async move {
            let b: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
            let sid = ["id", "server_id", "host"].iter().find_map(|k| b.get(*k).and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(String::from));
            let Some(sid) = sid else { return (axum::http::StatusCode::BAD_REQUEST, axum::Json(json!({"ok": false, "error": "no server id/host"}))) };
            let tx = sh.client.lock().unwrap().clone();
            let Some(tx) = tx else {
                return (axum::http::StatusCode::CONFLICT, axum::Json(json!({"ok": false, "error": "no client connected to the backend - start the game first"})));
            };
            let shape = b.get("shape").and_then(|v| v.as_str()).unwrap_or("bare").to_string();
            let tcp = jnum(b.get("tcp")).or(jnum(b.get("tcp_port")));
            let udp = jnum(b.get("udp")).or(jnum(b.get("udp_port"))).or(tcp);
            let pw = b.get("password").and_then(|v| v.as_str()).unwrap_or("").to_string();
            match build_join(&sid, &shape, tcp, udp, &pw) {
                Some((raw, url)) => {
                    println!("  PUSH go_to_server [{shape}] {}B -> {url}", raw.len());
                    let n = raw.len();
                    let _ = tx.send(Message::Binary(raw.into()));
                    (axum::http::StatusCode::OK, axum::Json(json!({"ok": true, "sent": true, "server_id": sid, "url": url, "shape": shape, "bytes": n})))
                }
                None => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, axum::Json(json!({"ok": false, "error": "schemas missing"}))),
            }
        }}))
        .fallback(get(move || { let sh = s3.clone(); async move {
            let (at, list) = sh.list.lock().unwrap().clone();
            let path = std::path::absolute(lobby_json()).unwrap_or(lobby_json());
            axum::Json(json!({"lobby": path.to_string_lossy(), "client_connected": sh.client.lock().unwrap().is_some(),
                              "stats": sh.stats.lock().unwrap().clone(), "servers_captured": list.len(), "servers_at": at}))
        }}));
    match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
        Ok(l) => { println!("control: http://127.0.0.1:{port}/  (POST /join {{\"id\":...}})"); let _ = axum::serve(l, app).await; }
        Err(e) => println!("control port {port}: {e}"),
    }
}

/// `acecm --tool acevo_proxy`
pub fn run() -> i32 {
    crate::procs::exit_with_parent();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().worker_threads(2).build().expect("runtime");
    rt.block_on(async {
        let port: u16 = env("PORT", "448").parse().unwrap_or(448);
        let upstream = env("UPSTREAM", "wss://b.gk.sd:6990/9kB3F1CAx7mQ2zL5vN8pD4sT6yW0eU1r/000500");
        let listen = env("BACKEND_LISTEN", "127.0.0.1");
        let acceptor = match server_tls() {
            Ok(a) => a,
            Err(e) => { println!("cannot load the TLS keypair: {e}"); return 1 }
        };
        let mut stats = Map::new();
        for k in ["up", "down", "injected", "answered"] { stats.insert(k.into(), json!(0)); }
        let sh = Arc::new(Shared { list: Mutex::new((0, vec![])), stats: Mutex::new(stats), client: Mutex::new(None) });
        println!("proxy listening on wss://localhost:{port}/");
        println!("upstream: {upstream}");
        tokio::spawn(control(sh.clone(), env("CONTROL_PORT", "8093").parse().unwrap_or(8093)));
        println!("proxy bind {listen}:{port}");
        let l = match tokio::net::TcpListener::bind((listen.as_str(), port)).await {
            Ok(l) => l,
            Err(e) => { println!("cannot bind {listen}:{port}: {e}"); return 1 }
        };
        loop {
            let Ok((tcp, peer)) = l.accept().await else { continue };
            println!("    connection from {peer}");
            tokio::spawn(handle(sh.clone(), tcp, peer, acceptor.clone(), upstream.clone()));
        }
    })
}
