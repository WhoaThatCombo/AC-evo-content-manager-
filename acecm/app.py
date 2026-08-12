"""Assetto Corsa EVO Content Manager - app server.

    python -m acecm            ->  http://localhost:8730

Stdlib only. Serves the UI and a small JSON API over the modules in this
package; no framework, so it runs anywhere Python does.
"""
import json
import mimetypes
import os
import socketserver
import threading
import time
import urllib.parse
from http.server import BaseHTTPRequestHandler

from . import (backend, config, content, hooking, install, logs, patching, version,
               registry, servers, settings as gamesettings,
               telemetry, tracks as trackdeploy)


def _json(handler, obj, code=200):
    body = json.dumps(obj).encode("utf-8")
    handler.send_response(code)
    handler.send_header("Content-Type", "application/json")
    handler.send_header("Content-Length", str(len(body)))
    handler.send_header("Cache-Control", "no-store")
    handler.end_headers()
    handler.wfile.write(body)


class Handler(BaseHTTPRequestHandler):
    server_version = "ACECM"

    def log_message(self, *a):
        pass

    # ---------------------------------------------------------------- GET --
    def do_GET(self):
        path, _, qs = self.path.partition("?")
        q = urllib.parse.parse_qs(qs)
        try:
            if path == "/api/state":
                return _json(self, self._state())
            if path == "/api/cars":
                return _json(self, content.cars())
            if path == "/api/models":
                return _json(self, content.models_seen())
            if path == "/api/mods":
                return _json(self, install.installed(
                    (q.get("side") or ["server"])[0]))
            if path == "/api/mods/audit":
                return _json(self, install.audit())
            if path == "/api/mods/scan":
                return _json(self, install.scan_source((q.get("path") or [""])[0]))
            if path == "/api/trackdeploy":
                return _json(self, {**trackdeploy.state(),
                                    **trackdeploy.packages(
                                        (q.get("dir") or [None])[0])})
            if path == "/api/tracks/installed":
                return _json(self, install.tracks_installed())
            if path == "/api/tracks":
                return _json(self, content.tracks())
            if path == "/api/profiles":
                items = servers.load()
                return _json(self, {"profiles": items,
                                    "template": servers.TEMPLATE,
                                    "options": servers.OPTIONS,
                                    "telemetry": telemetry.status_all()})
            if path == "/api/server/status":
                pid = (q.get("id") or [None])[0]
                prof = next((p for p in servers.load() if p["id"] == pid), None)
                if not prof:
                    return _json(self, {"error": "no such profile"}, 404)
                return _json(self, servers.status(prof))
            if path == "/api/server/log":
                pid = (q.get("id") or [None])[0]
                prof = next((p for p in servers.load() if p["id"] == pid), None)
                return _json(self, servers.log_tail(prof or {}))
            # ---- public server list + content delivery -------------------
            if path == "/api/registry":
                return _json(self, {"servers": registry.load(),
                                    "template": registry.TEMPLATE})
            if path == "/api/registry/list":
                return _json(self, registry.public_list(self._base()))
            if path == "/api/registry/manifest":
                return _json(self, registry.manifest(
                    (q.get("id") or [""])[0], self._base()))
            if path == "/api/registry/file":
                return self._serve_content((q.get("id") or [""])[0],
                                           (q.get("path") or [""])[0])
            if path == "/api/gamesettings":
                return _json(self, {**gamesettings.state(),
                                    **gamesettings.discover()})
            if path == "/api/gamesettings/read":
                return _json(self, gamesettings.read(
                    (q.get("file") or [""])[0]))
            if path == "/api/patches":
                return _json(self, patching.overview())
            if path == "/api/patches/inspect":
                return _json(self, patching.inspect(
                    (q.get("target") or [config.server_exe()])[0]))
            if path == "/api/backend":
                return _json(self, backend.state())
            if path == "/api/telemetry":
                return _json(self, telemetry.cars((q.get("id") or [None])[0]))
            if path == "/api/version":
                return _json(self, {"name": version.NAME,
                                    "version": version.VERSION,
                                    "frozen": config.FROZEN,
                                    "data_dir": config.DATA})
            if path == "/api/logs":
                return _json(self, logs.tail(
                    int((q.get("lines") or ["300"])[0]),
                    (q.get("file") or ["acecm.log"])[0]))
            if path == "/api/logs/files":
                return _json(self, logs.files())
            if path == "/api/update/check":
                return _json(self, version.check())
            if path == "/api/telemetry/leaderboard":
                return _json(self, telemetry.leaderboard(
                    (q.get("id") or [None])[0]))
            if path == "/api/telemetry/track":
                return _json(self, telemetry.track((q.get("id") or [None])[0]))
            if path == "/api/telemetry/status":
                return _json(self, telemetry.status_all())
            if path == "/api/browser":
                return _json(self, backend.server_list())
            if path == "/api/join/state":
                return _json(self, backend.join_state())
            if path == "/api/backend/log":
                mode = (q.get("mode") or ["proxy"])[0]
                return _json(self, backend.log(mode))
            if path == "/api/config":
                return _json(self, config.CFG)
            return self._static(path)
        except Exception as ex:
            # ⚠ Log the TRACEBACK. This used to return a one-line message and
            # throw the stack away, which made every API failure a guessing game.
            logs.exception(f"GET {self.path}", ex)
            return _json(self, {"error": f"{type(ex).__name__}: {ex}",
                                "see": "Logs tab"}, 500)

    # --------------------------------------------------------------- POST --
    def do_POST(self):
        path, _, _ = self.path.partition("?")
        n = int(self.headers.get("Content-Length") or 0)
        try:
            body = json.loads(self.rfile.read(n) or b"{}")
        except Exception:
            body = {}
        try:
            if path == "/api/profiles/save":
                return _json(self, servers.upsert(body))
            if path == "/api/profiles/delete":
                servers.delete(body.get("id"))
                return _json(self, {"ok": True})
            if path == "/api/mods/install":
                return _json(self, install.install(body.get("path"),
                                                   body.get("only")))
            if path == "/api/mods/fix":
                return _json(self, install.apply_fix(body.get("name")))
            if path == "/api/mods/remove":
                return _json(self, install.remove(body.get("name")))
            if path == "/api/trackdeploy/deploy":
                return _json(self, trackdeploy.deploy(body.get("path")))
            if path == "/api/trackdeploy/restore":
                return _json(self, trackdeploy.restore())
            if path == "/api/telemetry/start":
                return _json(self, telemetry.start(
                    body.get("id"), bool(body.get("baseline_ai"))))
            if path == "/api/telemetry/stop":
                return _json(self, telemetry.stop(body.get("id")))
            if path == "/api/registry/save":
                return _json(self, registry.upsert(body))
            if path == "/api/registry/delete":
                return _json(self, registry.remove(body.get("id")))
            if path == "/api/gamesettings/write":
                return _json(self, gamesettings.write(body.get("file"),
                                                      body.get("values") or {}))
            if path == "/api/gamesettings/restore":
                return _json(self, gamesettings.restore(body.get("file")))
            if path == "/api/hook/plan":
                # Plan a trampoline and hand back sites ready to register as a
                # patch - so a hook inherits verify/backup/build-gating.
                target = body.get("target") or config.server_exe()
                pe = patching.PEInfo(target)
                cave = body.get("cave")
                if not cave:
                    caves = pe.code_caves()
                    if not caves:
                        return _json(self, {"ok": False, "error": "no code cave"})
                    cave = caves[0]["va"]
                try:
                    plan = hooking.build(
                        pe, int(body["site"]), int(cave),
                        payload=bytes.fromhex(body.get("payload", "")),
                        minimum=int(body.get("minimum", 5)))
                    return _json(self, {"ok": True, "target": target,
                                        "cave": hex(int(cave)), **plan})
                except Exception as ex:
                    return _json(self, {"ok": False,
                                        "error": f"{type(ex).__name__}: {ex}"})
            if path == "/api/patches/save":
                return _json(self, patching.upsert(body))
            if path == "/api/patches/apply":
                ps = [x for x in patching.load() if x.id == body.get("id")]
                if not ps:
                    return _json(self, {"ok": False, "error": "no such patch"}, 404)
                return _json(self, ps[0].apply(force=bool(body.get("force"))))
            if path == "/api/patches/restore":
                ps = [x for x in patching.load() if x.id == body.get("id")]
                if not ps:
                    return _json(self, {"ok": False, "error": "no such patch"}, 404)
                return _json(self, ps[0].restore())
            if path == "/api/patches/delete":
                return _json(self, patching.remove(body.get("id")))
            if path == "/api/server/start":
                pid = body.get("id")
                prof = next((p for p in servers.load() if p["id"] == pid), None)
                if not prof:
                    return _json(self, {"ok": False, "error": "no profile"}, 404)
                return _json(self, servers.start(prof))
            if path == "/api/server/stop":
                return _json(self, servers.stop())
            if path == "/api/update/apply":
                return _json(self, version.apply(body.get("url"),
                                                 body.get("sha256")))
            if path == "/api/backend/start":
                return _json(self, backend.start(body.get("mode", "proxy")))
            if path == "/api/backend/stop":
                return _json(self, backend.stop())
            if path == "/api/join":
                return _json(self, backend.join(body.get("id"),
                                                body.get("shape", "bare")))
            if path == "/api/game/launch":
                return _json(self, backend.launch_game())
            if path == "/api/config":
                return _json(self, config.save(body))
            return _json(self, {"error": "unknown endpoint"}, 404)
        except Exception as ex:
            logs.exception(f"POST {self.path}", ex, body=body)
            return _json(self, {"error": f"{type(ex).__name__}: {ex}",
                                "see": "Logs tab"}, 500)

    # ------------------------------------------------------------- helpers --
    def _state(self):
        profs = servers.load()
        return {
            "profiles": len(profs),
            "running": any(servers.status(p)["running"] for p in profs[:1]),
            "backend": backend.state(),
            "server_dir": config.server_dir(),
            "server_exe_ok": os.path.exists(config.server_exe()),
            "tools_ok": os.path.isdir(config.tools_dir()),
        }

    def _base(self):
        host = self.headers.get("Host") or f"127.0.0.1:{config.CFG['ui_port']}"
        return f"http://{host}"

    def _serve_content(self, sid, rel):
        """Stream a declared content file.

        ⚠ Only files a registry entry actually declares are servable -
        resolve() returns None for anything else, so a crafted `path` cannot
        reach arbitrary files on disk.
        """
        full = registry.resolve(sid, rel)
        if not full or not os.path.isfile(full):
            return _json(self, {"error": "not a declared content file"}, 404)
        size = os.path.getsize(full)
        self.send_response(200)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Content-Length", str(size))
        self.send_header("Content-Disposition",
                         f'attachment; filename="{os.path.basename(full)}"')
        self.end_headers()
        with open(full, "rb") as fh:
            while True:
                chunk = fh.read(1 << 20)
                if not chunk:
                    break
                try:
                    self.wfile.write(chunk)
                except (BrokenPipeError, ConnectionResetError):
                    return          # client cancelled the download

    def _static(self, path):
        rel = "index.html" if path in ("/", "") else path.lstrip("/")
        full = os.path.normpath(os.path.join(config.WEB, rel))
        if not full.startswith(config.WEB) or not os.path.isfile(full):
            self.send_error(404)
            return
        ctype = mimetypes.guess_type(full)[0] or "application/octet-stream"
        data = open(full, "rb").read()
        self.send_response(200)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(data)


class App(socketserver.ThreadingTCPServer):
    daemon_threads = True
    # NOT reusable: on Windows a second process can otherwise bind the same
    # port silently and requests land on whichever wins - that cost real
    # debugging time on the telemetry tools.
    allow_reuse_address = False


def serve():
    """Bind and start the HTTP server on a background thread; return its URL.

    Split out from main() so a native window can own the MAIN thread - GUI
    toolkits require that - while the API keeps serving behind it.
    """
    logs.setup()
    port = config.CFG["ui_port"]
    try:
        srv = App(("127.0.0.1", port), Handler)
    except OSError as ex:
        raise SystemExit(f"port {port} in use ({ex}) - is ACECM already running?")
    url = f"http://localhost:{port}"
    print(f"Assetto Corsa EVO Content Manager\n  {url}")
    print(f"  server dir : {config.server_dir()}")
    print(f"  tools dir  : {config.tools_dir()}")
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv, url


def main(mode="window"):
    """mode: window (native), browser (default browser), headless (serve only)."""
    srv, url = serve()
    if mode == "window":
        from . import ui
        if ui.available():
            ui.run(url)          # blocks until the window is closed
            return
        print("  (no native webview available - falling back to the browser)")
        mode = "browser"
    if mode == "browser":
        threading.Timer(0.6, lambda: os.startfile(url)).start()   # noqa: S606
    try:
        while True:
            time.sleep(3600)
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
