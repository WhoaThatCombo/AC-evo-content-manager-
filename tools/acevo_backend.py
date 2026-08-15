"""A minimal local AC EVO lobby backend.

The client normally talks to Kunos's backend over a TLS websocket, exchanging
protobuf. The `-backend` flag points it somewhere else - its own help text
suggests `wss://localhost:448/communicationNode/42`, i.e. this is exactly how
Kunos developed locally.

Every frame is a serialised BackendMessage:

    message BackendMessage {
        google.protobuf.Any msg = 1;    // the real message, by type URL
        bool unwrap_message = 2;
    }

Schemas come from acevo_proto, which loads 90 FileDescriptorProto blobs lifted
out of the client - so the message definitions are exact, not guessed.

    python acevo_backend.py                 listen on wss://localhost:448
    PORT=448 SERVER_IP=127.0.0.1 ...        override

Then launch the game with:
    -backend=wss://localhost:448/communicationNode/42

STATUS: exploratory. The unknowns are (a) whether the client accepts a
self-signed certificate, (b) what the URL path means, and (c) whether it
expects Steam auth before registration. Logging is deliberately loud so the
first real connection tells us all three.
"""
import asyncio
import os
import ssl
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import websockets
from google.protobuf import any_pb2

import acevo_proto as ap

HERE = os.path.dirname(os.path.abspath(__file__))
PORT = int(os.environ.get("PORT", "448"))
SERVER_IP = os.environ.get("SERVER_IP", "127.0.0.1")
SERVER_TCP = int(os.environ.get("SERVER_TCP", "9700"))
SERVER_UDP = int(os.environ.get("SERVER_UDP", "9700"))
SERVER_NAME = os.environ.get("SERVER_NAME", "RIP acedit (local)")
SERVER_ID = os.environ.get("SERVER_ID", "local-0000-0000-0000-000000000001")
SERVER_HTTP = os.environ.get("SERVER_HTTP", "http://127.0.0.1:8080/")
# must match the dedicated server's frozen practice_time_of_day hour
TIME_OF_DAY = os.environ.get("TIME_OF_DAY", "13:00")
# Written by start_vai_server.py next to the dedicated server. Set CARS_JSON if
# your server lives elsewhere; without it the allowed-car list is empty and the
# client can only spectate (see gotcha 3 in the README).
CARS_JSON = os.environ.get("CARS_JSON",
                           os.path.join(os.path.dirname(HERE), "cars.json"))
PROTOCOL_VERSION = int(os.environ.get("PROTOCOL_VERSION", "8"))
SERVER_VERSION = int(os.environ.get("SERVER_VERSION", "6"))
LOBBY_JSON = os.environ.get("LOBBY_JSON", "")
LAN_IP = os.environ.get("LAN_IP", "")
OUR_IPS = {p.strip() for p in os.environ.get("OUR_IPS", "").split(",") if p.strip()}
OUR_IPS.update({"127.0.0.1", "localhost", "::1"})
if SERVER_IP:
    OUR_IPS.add(SERVER_IP)
if LAN_IP:
    OUR_IPS.add(LAN_IP)

TYPE_PREFIX = "type.googleapis.com/"
# Echoed from the client's RegisterRequest so an update that bumps
# protocol/server version does not silently drop our list entry.
_versions = {"protocol": PROTOCOL_VERSION, "server": SERVER_VERSION}


def _default_lobby_json():
    """Where ACECM keeps lobby.json, frozen build or source checkout."""
    for base in (os.path.join(os.environ.get("LOCALAPPDATA", ""), "ACECM",
                              "data"),
                 os.path.join(os.path.expanduser("~"), "Downloads", "acecm",
                              "data")):
        p = os.path.join(base, "lobby.json")
        if os.path.isfile(p):
            return p
    return ""


def _lobby():
    # ⚠ Fall back to ACECM's own data folder. LOBBY_JSON is set when ACECM
    # launches the backend, but a backend started by hand had no default at
    # all - so _lobby() returned {} and the advertised entry silently used the
    # placeholder name and no track, which looks like the lobby is broken
    # rather than simply unconfigured.
    path = LOBBY_JSON or _default_lobby_json()
    if path and os.path.isfile(path):
        try:
            import json
            return json.load(open(path, encoding="utf-8"))
        except Exception as ex:
            print(f"  (lobby.json unreadable: {ex})")
    return {}


def _lan_ip():
    if LAN_IP:
        return LAN_IP
    lb = _lobby()
    if lb.get("lan_ip"):
        return lb["lan_ip"]
    try:
        import socket
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        try:
            s.connect(("1.1.1.1", 80))
            ip = s.getsockname()[0]
        finally:
            s.close()
        if ip and not ip.startswith("127."):
            return ip
    except OSError:
        pass
    return ""


def _peer_host(peer):
    if peer is None:
        return ""
    if isinstance(peer, (tuple, list)) and peer:
        return str(peer[0] or "")
    if isinstance(peer, str):
        return peer
    return ""


def _advertise_ip(peer=None):
    """Loopback for a local client, LAN address for everyone else."""
    host = _peer_host(peer)
    lan = _lan_ip()
    if not host or host in ("127.0.0.1", "::1", "localhost") or host.startswith("127."):
        return "127.0.0.1"
    return lan or SERVER_IP or "127.0.0.1"


def is_our_server(ip, tcp=None, udp=None):
    """Should we answer RequestServerEntry for this address?"""
    if not ip:
        return False
    ip = ip.strip()
    lb = _lobby()
    known = set(OUR_IPS)
    if lb.get("lan_ip"):
        known.add(lb["lan_ip"])
    lan = _lan_ip()
    if lan:
        known.add(lan)
    if SERVER_IP:
        known.add(SERVER_IP)
    return ip in known


def unwrap(raw):
    """BackendMessage -> (type name, parsed message) or (None, None)."""
    env = ap.new("BackendMessage")
    env.ParseFromString(raw)
    url = env.msg.type_url
    name = url.split("/")[-1]
    if not name or not ap.has(name):
        return name or None, None
    inner = ap.new(name)
    inner.ParseFromString(env.msg.value)
    return name, inner


def wrap(msg, type_name):
    env = ap.new("BackendMessage")
    env.msg.type_url = TYPE_PREFIX + type_name
    env.msg.value = msg.SerializeToString()
    env.unwrap_message = False
    return env.SerializeToString()


def live_player_count():
    """Real client count from the dedicated server's own HTTP port.

    The lobby entry is otherwise invented, so the browser showed 0/90 while the
    server actually had ~30 people on it. This is the one number the server will
    tell us for free; everything else in the row is still static.
    """
    try:
        import json
        import urllib.request
        url = SERVER_HTTP
        lb = _lobby()
        if lb.get("http_port"):
            url = f"http://127.0.0.1:{int(lb['http_port'])}/"
        with urllib.request.urlopen(url, timeout=2) as r:
            return int(json.loads(r.read()).get("clients", 0))
    except Exception:
        return 0


LOCAL_TAG = os.environ.get("LOCAL_TAG", "[ACECM]")


def _tagged(name):
    """Prefix the injected entry so it is obvious which row is ours.

    ⚠ A PREFIX, not a suffix: the browser truncates long names, and the whole
    point is lost if the marker is the part that gets cut. Never doubled, so a
    server already called "... [LOCAL]" is left alone.
    """
    name = (name or "").strip()
    if not LOCAL_TAG:
        return name
    if LOCAL_TAG.lower() in name.lower():
        return name
    return f"{LOCAL_TAG} {name}".strip()


def fill_entry(entry, peer=None):
    """Populate a MultiplayerServerListEntry for our own server.

    Track, cars, name and ToD come from lobby.json (the running ACECM
    profile). server_ip is peer-aware: 127.0.0.1 for a client on this
    machine, the LAN IPv4 for everyone else.
    """
    def setif(name, value):
        if name in [f.name for f in entry.DESCRIPTOR.fields]:
            try:
                setattr(entry, name, value)
                return True
            except Exception:
                pass
        return False

    lb = _lobby()
    setif("server_id", lb.get("server_id") or SERVER_ID)
    setif("server_ip", _advertise_ip(peer))
    # ⚠ Mark it. This row is INJECTED by the proxy and reached over loopback or
    # the LAN, while the very same server also registers with Kunos and appears
    # again with its public address - two rows, same name, different routes.
    # The tag says which one is ours, for the host and for anyone else running
    # ACECM. Set LOCAL_TAG="" to advertise a bare name.
    setif("server_name", _tagged(lb.get("server_name") or SERVER_NAME))
    setif("server_tcp_port", int(lb.get("tcp_port") or SERVER_TCP))
    setif("server_udp_port", int(lb.get("udp_port") or SERVER_UDP))
    setif("track", lb.get("track") or os.environ.get("TRACK") or "")
    setif("layout", lb.get("layout") or os.environ.get("LAYOUT") or "")
    setif("event_name", lb.get("event_name") or "")
    # Echo the versions the client just registered with. Hardcoding 8/6 is
    # what makes an entry vanish after a game update.
    setif("protocol_version", _versions["protocol"])
    setif("server_version", _versions["server"])
    setif("game_mode_type", int(lb.get("game_mode_type") or 10))
    try:
        del entry.players[:]
        for i in range(live_player_count()):
            entry.players.append(f"driver {i + 1}")
    except Exception as ex:
        print(f"  (could not fill players: {ex})")
    setif("max_players", int(lb.get("max_players") or 90))
    mode = (lb.get("game_mode") or "PRACTICE").replace("GameModeType_", "")
    pretty = mode.replace("_", " ").title()
    if pretty == "Practice":
        pretty = "Practice"
    setif("current_session", pretty)
    setif("time_of_day", lb.get("time_of_day") or TIME_OF_DAY)
    setif("is_car_eligible", True)

    # Drive-button list: prefer the profile snapshot. Fall back to cars.json
    # PLUS any leftover names (mods used to be stripped here).
    names = [c for c in (lb.get("cars") or []) if c]
    if not names:
        try:
            import json
            import re
            cars = json.load(open(CARS_JSON, encoding="utf-8"))["cars"]
            names = [c["name"] for c in cars if re.fullmatch(r".+_mech_\d+", c["name"])]
        except Exception as ex:
            print(f"  (could not fill car list: {ex})")
            names = []
    try:
        fields = [f.name for f in entry.DESCRIPTOR.fields]
        if "allowed_cars_list_full" in fields:
            del entry.allowed_cars_list_full[:]
            for n in names:
                a = entry.allowed_cars_list_full.add()
                a.car_name = n
                a.ballast = 0
                a.restrictor = 0
        setif("allowed_cars_list", ",".join(names))
    except Exception as ex:
        print(f"  (could not write car list: {ex})")
    return entry


async def handle(ws):
    peer = getattr(ws, "remote_address", None)
    print(f"\n*** CONNECTED from {peer}  path={getattr(ws, 'path', '?')}")
    try:
        async for raw in ws:
            if isinstance(raw, str):
                print(f"  <- TEXT {raw[:200]}")
                continue
            try:
                name, msg = unwrap(raw)
            except Exception as ex:
                print(f"  <- {len(raw)}B  UNPARSEABLE: {ex}")
                continue
            print(f"  <- {name}")
            if msg is not None and str(msg).strip():
                for line in str(msg).strip().splitlines()[:12]:
                    print(f"       {line}")

            reply = None
            if name == "GameEconomyClientRequestAccount":
                # An empty account is accepted (the menu loads) but leaves the
                # driver unidentified, so the client never progresses to asking
                # for owned cars - which is why only some cars are offered.
                r = ap.new("GameEconomyClientResponseAccount")
                try:
                    r.account_id = getattr(msg, "steam_id", "") or "local"
                    r.is_new = False
                    r.account_pguid.a, r.account_pguid.b = 1, 1
                    drv = r.driver_data.available_drivers.add()
                    drv.driver_pguid.a, drv.driver_pguid.b = 2, 2
                    drv.first_name, drv.last_name = "Local", "Driver"
                    r.driver_data.current_driver_pguid.a = 2
                    r.driver_data.current_driver_pguid.b = 2
                    p = r.current_driver_personal_data
                    p.first_name, p.last_name = "Local", "Driver"
                    p.shortcut_name, p.nickname = "LOC", "Local"
                    p.nationality_alpha_3 = "GBR"
                    g = r.current_driver_progression_data
                    g.current_level = 50
                    g.money = 99999999
                    g.xp_points = 1000000
                except Exception as ex:
                    print(f"     (account fill: {ex})")
                reply = ("GameEconomyClientResponseAccount", r)

            elif name == "RegisterRequest":
                r = ap.new("RegisterResponse")
                r.action_id = getattr(msg, "action_id", 0)
                r.is_registered = True
                try:
                    r.platform_type = msg.platform_type
                except Exception:
                    pass
                try:
                    proto = getattr(msg, "procol_version", None)
                    if proto is None:
                        proto = getattr(msg, "protocol_version", None)
                    if proto:
                        _versions["protocol"] = int(proto)
                    ver = getattr(msg, "server_version", None)
                    if ver:
                        _versions["server"] = int(ver)
                    print(f"     (echo protocol={_versions['protocol']} "
                          f"server={_versions['server']})")
                except Exception as ex:
                    print(f"     (version echo: {ex})")
                reply = ("RegisterResponse", r)

            elif name == "MultiplayerServerListRequestServerList":
                r = ap.new("MultiplayerServerListResponseServerList")
                fill_entry(r.entry.add(), peer=peer)
                try:
                    r.request_no = msg.request_no
                except Exception:
                    pass
                reply = ("MultiplayerServerListResponseServerList", r)

            elif name == "MultiplayerServerListRequestServerEntry":
                r = ap.new("MultiplayerServerListResponseServerEntry")
                fill_entry(r.entry, peer=peer)
                reply = ("MultiplayerServerListResponseServerEntry", r)

            elif name == "MultiplayerServerListRequestConnectToServer":
                r = ap.new("MultiplayerServerListResponseConnectToServer")
                reply = ("MultiplayerServerListResponseConnectToServer", r)

            # Generic fallback: every request type has a matching response type
            # whose name is the same with Request -> Response. Answering blindly
            # keeps the client moving instead of stalling on the first message we
            # haven't special-cased, and shows us what it asks for next.
            if reply is None and name and "Request" in name:
                cand = name.replace("Request", "Response", 1)
                if ap.has(cand):
                    r = ap.new(cand)
                    for fname, val in (("account_id", str(getattr(msg, "steam_id", "")
                                                          or SERVER_ID)),
                                       ("is_new", False)):
                        try:
                            setattr(r, fname, val)
                        except Exception:
                            pass
                    reply = (cand, r)
                    print(f"     (generic {cand})")

            if reply:
                tn, m = reply
                await ws.send(wrap(m, tn))
                print(f"  -> {tn}")
            else:
                print("     (no handler - not replying)")
    except websockets.ConnectionClosed as ex:
        print(f"*** CLOSED: {ex}")
    except Exception as ex:
        print(f"*** ERROR: {type(ex).__name__}: {ex}")


async def main():
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    # ⚠ HERE is the PyInstaller unpack folder in a frozen build, and keys are
    # deliberately never shipped - so this looked next to the script for a
    # cert.pem that cannot exist there and standalone died on FileNotFoundError
    # every time from a built exe. ACECM_CERTS is where ACECM actually
    # generates the keypair; the proxy already honoured it, this did not.
    _certs = os.environ.get("ACECM_CERTS") or HERE
    _crt, _key = os.path.join(_certs, "cert.pem"), os.path.join(_certs, "key.pem")
    if not (os.path.exists(_crt) and os.path.exists(_key)):
        raise SystemExit(
            f"no TLS keypair in {_certs} - generate it on ACECM's Backend "
            f"page, or set ACECM_CERTS to the folder holding cert.pem/key.pem")
    ctx.load_cert_chain(_crt, _key)
    print(f"loaded {len(ap.loaded)} descriptors")
    lb = _lobby()
    lan = _lan_ip()
    listen = (os.environ.get("BACKEND_LISTEN") or "127.0.0.1").strip() or "127.0.0.1"
    print(f"listening on wss://{listen}:{PORT}/  (any path)")
    print(f"lobby.json: {LOBBY_JSON or '(none)'}")
    print(f"advertising {lb.get('server_name') or SERVER_NAME}  "
          f"track={lb.get('track') or '?'}  cars={len(lb.get('cars') or [])}")
    print(f"loopback 127.0.0.1:{int(lb.get('tcp_port') or SERVER_TCP)}  "
          f"lan {lan or '(none)'}")
    print("launch the game with:")
    print(f"  -backend=wss://127.0.0.1:{PORT}/communicationNode/dev\n")
    async with websockets.serve(handle, listen, PORT, ssl=ctx,
                                max_size=None, ping_interval=None):
        await asyncio.Future()


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        pass
