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

TYPE_PREFIX = "type.googleapis.com/"


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
        with urllib.request.urlopen(SERVER_HTTP, timeout=2) as r:
            return int(json.loads(r.read()).get("clients", 0))
    except Exception:
        return 0


def fill_entry(entry):
    """Populate a MultiplayerServerListEntry for our own server."""
    def setif(name, value):
        if name in [f.name for f in entry.DESCRIPTOR.fields]:
            try:
                setattr(entry, name, value)
                return True
            except Exception:
                pass
        return False

    setif("server_id", SERVER_ID)
    setif("server_ip", SERVER_IP)
    setif("server_name", SERVER_NAME)
    setif("server_tcp_port", SERVER_TCP)
    setif("server_udp_port", SERVER_UDP)
    setif("track", "nurburgring")
    setif("layout", "layout_nordschleife_touristenfahrten")
    setif("event_name", "Touristenfahrten")
    # ⚠ These two are why an entry silently vanishes from the browser. The
    # client logs "Igoring server with protocol version {} (mine: {}), server
    # version {} ({})" and drops anything that doesn't match. Values come from
    # the client's own RegisterRequest: procol_version 8, server_version 6.
    setif("protocol_version", PROTOCOL_VERSION)
    setif("server_version", SERVER_VERSION)
    # GameModeType_PRACTICE. Without this the browser row reads
    # "GameModeType_NONE" (the zero value) - the enum lives in ClientCommandsUtil.
    setif("game_mode_type", 10)
    # players/friends are STRINGS in this schema, not counts - setting an int
    # fails silently and the row renders blank.
    # players/friends are REPEATED strings - a list of names, not a count. The
    # browser shows len(players), which is why a scalar assignment left it 0/90.
    # We only know the count from the server's HTTP port, so synthesise that
    # many rows; the AI cars are included in the count.
    try:
        del entry.players[:]
        for i in range(live_player_count()):
            entry.players.append(f"driver {i + 1}")
    except Exception as ex:
        print(f"  (could not fill players: {ex})")
    setif("max_players", 90)
    setif("current_session", "Practice")
    setif("time_of_day", TIME_OF_DAY)
    setif("is_car_eligible", True)

    # The client only offers DRIVE for cars the entry says are allowed. With an
    # empty list it joins as a spectator even though it asked to drive ("as
    # spectator: false" appears in its own log). Mirror the dedicated server's
    # list, filtered the same way - real Kunos presets only, no mods.
    try:
        import json
        import re
        cars = json.load(open(CARS_JSON))["cars"]
        names = [c["name"] for c in cars
                 if re.fullmatch(r".+_mech_\d+", c["name"])]
        fields = [f.name for f in entry.DESCRIPTOR.fields]
        if "allowed_cars_list_full" in fields:
            for n in names:
                a = entry.allowed_cars_list_full.add()
                a.car_name = n
                a.ballast = 0
                a.restrictor = 0
        setif("allowed_cars_list", ",".join(names))
    except Exception as ex:
        print(f"  (could not fill car list: {ex})")
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
                reply = ("RegisterResponse", r)

            elif name == "MultiplayerServerListRequestServerList":
                r = ap.new("MultiplayerServerListResponseServerList")
                fill_entry(r.entry.add())
                try:
                    r.request_no = msg.request_no
                except Exception:
                    pass
                reply = ("MultiplayerServerListResponseServerList", r)

            elif name == "MultiplayerServerListRequestServerEntry":
                r = ap.new("MultiplayerServerListResponseServerEntry")
                fill_entry(r.entry)
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
    ctx.load_cert_chain(os.path.join(HERE, "cert.pem"), os.path.join(HERE, "key.pem"))
    print(f"loaded {len(ap.loaded)} descriptors")
    print(f"listening on wss://localhost:{PORT}/  (any path)")
    print(f"advertising {SERVER_NAME} at {SERVER_IP}:{SERVER_TCP}")
    print("launch the game with:")
    print(f"  -backend=wss://localhost:{PORT}/communicationNode/42\n")
    async with websockets.serve(handle, "0.0.0.0", PORT, ssl=ctx,
                                max_size=None, ping_interval=None):
        await asyncio.Future()


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        pass
