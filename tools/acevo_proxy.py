"""Man-in-the-middle backend: real Kunos account, plus your local server.

acevo_backend.py REPLACES the backend, which means no account, no owned cars and
no real server list - everything has to be faked, and the economy is a project in
itself.

This instead sits in the middle:

    client  ->  this proxy  ->  wss://b.gk.sd:6990/<token>/000500  (real Kunos)

Everything is relayed untouched, so progression, garage and the genuine server
browser all work exactly as normal. The only interference:

  * MultiplayerServerListResponseServerList coming DOWN from Kunos gets our
    local server appended, so it appears alongside every public server.
  * MultiplayerServerListRequestServerEntry going UP for our own IP is answered
    locally (Kunos doesn't know about a 127.0.0.1 server).

So you get both backends at once and never restart the game to switch.

    python acevo_proxy.py

Requires the same URL patch as acevo_backend.py (patch_backend_url.py), since the
client must dial localhost:448 for us to sit in the middle.
"""
import asyncio
import json
import os
import ssl
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

# ⚠ Belt and braces with PYTHONUNBUFFERED from the launcher: when stdout is a
# file (which it always is when ACECM starts us) Python block-buffers it, so a
# proxy that is later killed writes a ZERO-BYTE log and every diagnostic print
# below is lost. Line buffering costs nothing here and makes the log usable
# while the proxy is still running, which is exactly when it is needed.
try:
    sys.stdout.reconfigure(line_buffering=True)
    sys.stderr.reconfigure(line_buffering=True)
except Exception:
    pass

import logging

import websockets

# ⚠ Without this a REJECTED client is completely silent. "*** CLIENT CONNECTED"
# below only prints after the TLS handshake and the WebSocket upgrade have both
# succeeded; websockets reports handshake and certificate failures through
# `logging`, which nothing here ever configured. So a client that dials us and
# is turned away looks byte-for-byte identical to a client that never dialled -
# an empty log either way, and no way to tell "the game ignored -backend=" from
# "the game reached us and we refused it".
logging.basicConfig(level=logging.INFO, stream=sys.stdout,
                    format="%(levelname)s %(name)s: %(message)s")
logging.getLogger("websockets.server").setLevel(logging.DEBUG)

import acevo_proto as ap
from acevo_backend import (PROTOCOL_VERSION, SERVER_VERSION, TIME_OF_DAY,
                           fill_entry, is_our_server, unwrap, wrap)

HERE = os.path.dirname(os.path.abspath(__file__))
PORT = int(os.environ.get("PORT", "448"))
# the client's original endpoint, from the string patch_backend_url.py replaces
UPSTREAM = os.environ.get(
    "UPSTREAM", "wss://b.gk.sd:6990/9kB3F1CAx7mQ2zL5vN8pD4sT6yW0eU1r/000500")
SERVER_IP = os.environ.get("SERVER_IP", "127.0.0.1")
SERVER_TCP = int(os.environ.get("SERVER_TCP", "9700"))

stats = {"up": 0, "down": 0, "injected": 0, "answered": 0}

# The live client socket, so "go to server" can be pushed at it on demand.
# See join_push.py: the client accepts a MultiplayerResponseGoToServer whose
# url is "join:<server_id>" and walks itself into that server, which is how we
# launch straight into a game instead of driving the menus.
CLIENT = {"ws": None, "loop": None}
# The full public server list, captured as it passes through. Kunos sends this
# in response to the client's RequestServerList, so we see every server the
# in-game browser would show - and can serve it to tools that have no client.
SERVER_LIST = {"at": 0, "servers": []}


def _capture_list(msg):
    """Flatten a ResponseServerList into plain JSON for the browser UI."""
    out = []
    for e in getattr(msg, "entry", []):
        d = {}
        for f in e.DESCRIPTOR.fields:
            try:
                v = getattr(e, f.name)
            except Exception:
                continue
            # protobuf >= 5 dropped FieldDescriptor.label; is_repeated is the
            # current spelling. Touching .label raises, which is exactly what
            # silently killed this capture the first time.
            repeated = getattr(f, "is_repeated", None)
            if repeated is None:
                try:
                    repeated = f.label == 3
                except Exception:
                    repeated = False
            if repeated:
                # ⚠ The CAR lists are kept in full. Everything else repeated
                # (players, friends, entry_list) is only interesting as a
                # count, but which cars a server allows is what decides
                # whether you can join it at all - and a count cannot answer
                # "do I have these?", so the browser could flag a missing
                # track and never a missing car.
                try:
                    if f.name == "allowed_cars_list":
                        d[f.name] = [str(x) for x in v]
                    elif f.name == "allowed_cars_list_full":
                        d[f.name] = [getattr(x, "car_name", "") for x in v]
                    else:
                        d[f.name] = len(v)      # counts, not the whole list
                    continue
                except TypeError:
                    pass
            if hasattr(v, "DESCRIPTOR"):
                continue                        # skip nested messages
            d[f.name] = v
        out.append(d)
    SERVER_LIST["servers"] = out
    SERVER_LIST["at"] = int(time.time())
CONTROL_PORT = int(os.environ.get("CONTROL_PORT", "8093"))


class _Control(BaseHTTPRequestHandler):
    """Tiny local control surface: POST /join?id=<server_id>."""

    def log_message(self, *a):
        pass

    def _send(self, obj, code=200):
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path.startswith("/servers"):
            return self._send({"count": len(SERVER_LIST["servers"]),
                               "captured_at": SERVER_LIST["at"],
                               "servers": SERVER_LIST["servers"]})
        self._send({"client_connected": CLIENT["ws"] is not None,
                    "stats": stats,
                    "servers_captured": len(SERVER_LIST["servers"]),
                    "servers_at": SERVER_LIST["at"]})

    def do_POST(self):
        if not self.path.startswith("/join"):
            return self._send({"error": "unknown"}, 404)
        n = int(self.headers.get("Content-Length") or 0)
        try:
            body = json.loads(self.rfile.read(n) or b"{}")
        except Exception:
            body = {}
        sid = body.get("id") or body.get("server_id") or body.get("host")
        shape = body.get("shape", "bare")
        tcp = body.get("tcp") or body.get("tcp_port")
        udp = body.get("udp") or body.get("udp_port") or tcp
        password = body.get("password") or ""
        ws, loop = CLIENT["ws"], CLIENT["loop"]
        if not sid:
            return self._send({"ok": False, "error": "no server id/host"}, 400)
        if not ws or not loop:
            return self._send({"ok": False,
                               "error": "no client connected to the backend - "
                                        "start the game first"}, 409)
        import join_push
        try:
            fut = asyncio.run_coroutine_threadsafe(
                join_push.push(ws, sid, shape, tcp=tcp, udp=udp,
                               password=password), loop)
            self._send({"ok": True, **fut.result(timeout=5)})
        except Exception as ex:
            self._send({"ok": False, "error": f"{type(ex).__name__}: {ex}"}, 500)


def start_control():
    srv = ThreadingHTTPServer(("127.0.0.1", CONTROL_PORT), _Control)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    print(f"control: http://127.0.0.1:{CONTROL_PORT}/  (POST /join {{\"id\":...}})")


def add_local_entry(raw, peer=None):
    """Append our server to a ResponseServerList travelling downstream."""
    try:
        name, msg = unwrap(raw)
    except Exception:
        return raw, None
    if name != "MultiplayerServerListResponseServerList":
        return raw, name
    try:
        _capture_list(msg)
    except Exception as ex:
        print(f"  (server-list capture failed: {ex})")
    try:
        # Put ours FIRST. Appended it sat after ~800 public rows, so the
        # browser looked empty of "our" server (and of anything on page 1
        # if a filter hid the rest).
        fill_entry(msg.entry.add(), peer=peer)
        blobs = [e.SerializeToString() for e in msg.entry]
        del msg.entry[:]
        first = msg.entry.add()
        first.ParseFromString(blobs[-1])
        for b in blobs[:-1]:
            e = msg.entry.add()
            e.ParseFromString(b)
        stats["injected"] += 1
        return wrap(msg, name), name
    except Exception as ex:
        print(f"  (inject failed: {ex})")
        return raw, name


async def pump_up(client, upstream):
    """client -> Kunos, answering our own server's entry lookups locally."""
    async for raw in client:
        stats["up"] += 1
        if isinstance(raw, str):
            # the Steam auth ticket; forward verbatim
            print(f"  UP   TEXT {raw[:60]}...")
            await upstream.send(raw)
            continue
        try:
            name, msg = unwrap(raw)
        except Exception:
            await upstream.send(raw)
            continue

        req_ip = getattr(msg, "server_ip", "") or ""
        req_tcp = getattr(msg, "server_tcp_port", 0) or 0
        if name == "MultiplayerServerListRequestServerEntry" and is_our_server(
                req_ip, req_tcp):
            # Kunos has never heard of a server on 127.0.0.1 / our LAN IP
            r = ap.new("MultiplayerServerListResponseServerEntry")
            fill_entry(r.entry, peer=getattr(client, "remote_address", None))
            await client.send(wrap(r, "MultiplayerServerListResponseServerEntry"))
            stats["answered"] += 1
            print(f"  ANS  {name} for {req_ip}:{req_tcp} (locally)")
            continue

        print(f"  UP   {name}")
        await upstream.send(raw)


async def pump_down(client, upstream):
    """Kunos -> client, injecting our server into the list."""
    async for raw in upstream:
        stats["down"] += 1
        if isinstance(raw, str):
            await client.send(raw)
            continue
        out, name = add_local_entry(
            raw, peer=getattr(client, "remote_address", None))
        if name == "MultiplayerServerListResponseServerList":
            print(f"  DOWN {name}  (+1 local entry)")
        await client.send(out)


async def handle(client):
    print(f"\n*** CLIENT CONNECTED {getattr(client, 'remote_address', None)}")
    # Remember the live socket and its loop so the control endpoint can push a
    # "go to server" at it from another thread. Without this the proxy relays
    # perfectly but /join always answers "no client connected".
    CLIENT["ws"] = client
    CLIENT["loop"] = asyncio.get_running_loop()
    print(f"    dialling upstream {UPSTREAM}")
    up_ssl = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
    up_ssl.check_hostname = False
    up_ssl.verify_mode = ssl.CERT_NONE          # we are not the trust anchor here
    try:
        # Kunos's permessage-deflate is not always a valid zlib stream
        # (websockets then raises "incorrect header check" and the list
        # never arrives). Uncompressed frames work on every machine.
        async with websockets.connect(
                UPSTREAM, ssl=up_ssl, max_size=None,
                ping_interval=None, compression=None) as upstream:
            print("    upstream connected - relaying")
            await asyncio.gather(pump_up(client, upstream),
                                 pump_down(client, upstream))
    except Exception as ex:
        print(f"*** proxy error: {type(ex).__name__}: {ex}")
    finally:
        if CLIENT["ws"] is client:
            CLIENT["ws"] = None
        print(f"*** closed. up={stats['up']} down={stats['down']} "
              f"injected={stats['injected']} answered={stats['answered']}")


async def main():
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    # ACECM_CERTS: a shipped build generates its keypair per machine rather
    # than carrying one, so it does not sit beside this script.
    _certs = os.environ.get("ACECM_CERTS") or HERE
    ctx.load_cert_chain(os.path.join(_certs, "cert.pem"),
                        os.path.join(_certs, "key.pem"))

    # ⚠ "connection is CONNECTING" and then silence forever is the hardest
    # state to act on: the socket is open and TLS never completes, and neither
    # websockets nor asyncio says a word about why. These two hooks split that
    # single symptom into causes you can actually do something about.
    #   ClientHello logged  -> the client IS speaking TLS; a later failure is
    #                          about the certificate or the cipher list
    #   never logged        -> nothing TLS-shaped arrived: the client dialled
    #                          plain ws://, or something in between (an
    #                          antivirus doing HTTPS inspection, a proxy) is
    #                          holding the connection open without passing it on
    def _client_hello(sslobj, servername, sslctx):
        print(f"    TLS ClientHello from client (sni={servername!r})")
    try:
        ctx.sni_callback = _client_hello
    except Exception:
        pass

    def _on_error(loop, context):
        exc = context.get("exception")
        print(f"    ! {context.get('message')}"
              + (f": {type(exc).__name__}: {exc}" if exc else ""))
    asyncio.get_running_loop().set_exception_handler(_on_error)
    print(f"proxy listening on wss://localhost:{PORT}/")
    print(f"upstream: {UPSTREAM}")
    print(f"injecting local server {SERVER_IP}:{SERVER_TCP} into the server list")
    start_control()
    print()
    # The real client asks for Sec-WebSocket-Protocol: wss. Echo it. A
    # missing echo still worked on one machine, but WebSocket++ can sit
    # in CONNECTING if the negotiated subprotocol never comes back.
    serve_kw = dict(ssl=ctx, ssl_handshake_timeout=15,
                    max_size=None, ping_interval=None,
                    compression=None,
                    subprotocols=["wss"])
    # ⚠ Without a handshake timeout a client that opens the socket and then
    # never completes TLS hangs in CONNECTING FOREVER: no error, no timeout,
    # nothing in the log but the connection appearing. asyncio only reports
    # "SSL handshake failed" once it is allowed to give up, so with no bound
    # set, the single most informative line never gets written.
    # ACECM used to health-check this port with a raw TCP connect, which
    # produced exactly that "CONNECTING forever" line and sent us chasing
    # a TLS bug that was our own probe.
    listen = (os.environ.get("BACKEND_LISTEN") or "127.0.0.1").strip() or "127.0.0.1"
    print(f"proxy bind {listen}:{PORT}")
    async with websockets.serve(handle, listen, PORT, **serve_kw):
        await asyncio.Future()


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        pass
