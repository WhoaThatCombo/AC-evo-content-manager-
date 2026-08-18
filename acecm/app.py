"""Assetto Corsa EVO Content Manager - app server.

    python -m acecm            ->  http://localhost:8730

Stdlib only. Serves the UI and a small JSON API over the modules in this
package; no framework, so it runs anywhere Python does.
"""
import json
import mimetypes
import os
import shutil
import socketserver
import subprocess
import threading
import time
import urllib.parse
from http.server import BaseHTTPRequestHandler

from . import (backend, config, content, contentsync, detect, drive,
               hooking, install,
               installer,
               logs, lobby, netutil, overview, patching, realai, version,
               registry, servers, settings as gamesettings,
               telemetry, thumbs, tracks as trackdeploy, viewer)


_INSTALL = {"state": "idle", "detail": "", "done": 0, "total": 0, "files": []}

# Set when the update swap script launched us (it passes --okflag). A
# freshly swapped build waits the outgoing one out instead of deferring
# to it - see the bind loop in serve().
_JUST_UPDATED = False


def _install_content(body):
    """Download everything a server needs that this machine lacks.

    Runs in a thread and reports through /api/browser/status: a track is a
    thousand files and roughly a gigabyte, so the request must not be the thing
    holding the progress.
    """
    if _INSTALL["state"] == "running":
        return {"ok": False, "error": "an install is already running"}
    base = body.get("base") or ""
    # ⚠ A host publishes one entry PER THING - four tracks and a car mod here -
    # so fetching a single id took the track and silently ignored the car. Take
    # every id offered, and de-duplicate: separate entries can require the same
    # file, and downloading it twice is just slower.
    ids = body.get("ids") or ([body["id"]] if body.get("id") else [])
    need, seen, errors = [], set(), []
    stale = []
    for sid in ids:
        p = contentsync.plan(base, sid)
        if not p.get("ok"):
            errors.append(f"{sid}: {p.get('error')}")
            continue
        for e in p["need"]:
            if e["path"] not in seen:
                seen.add(e["path"])
                need.append(e)
        for f in (p.get("stale") or []):
            if f not in stale:
                stale.append(f)
    if not need and stale:
        # nothing to download, but the host has removed files since last time
        got = contentsync.remove_stale(stale)
        return {"ok": True, "files": 0, "bytes": 0,
                "removed": len(got.get("removed") or [])}
    if not need:
        return {"ok": False,
                "error": ("; ".join(errors) if errors
                          else "you already have everything this host offers")}
    p = {"ok": True, "need": need,
         "bytes": sum(e.get("size", 0) for e in need)}
    _INSTALL.update({"state": "running", "detail": "starting", "done": 0,
                     "total": p["bytes"], "files": []})

    def run():
        try:
            contentsync.install_files(need, _INSTALL)
            # ⚠ AFTER the downloads, never before: a failed install must not
            # leave the joiner with files deleted and nothing to replace them.
            dropped = contentsync.remove_stale(stale) if stale else {}
            if dropped.get("removed"):
                _INSTALL["removed"] = len(dropped["removed"])
            extra = _INSTALL.get("warning")
            _INSTALL.update({"state": "done",
                             "detail": (f"{len(need)} file(s) installed"
                                        + (f" — {extra}" if extra else ""))})
        except Exception as ex:
            logs.LOG.exception("content install failed")
            _INSTALL.update({"state": "error", "detail": str(ex)})

    threading.Thread(target=run, daemon=True).start()
    return {"ok": True, "files": len(need), "bytes": p["bytes"]}


def _json(handler, obj, code=200):
    body = json.dumps(obj).encode("utf-8")
    handler.send_response(code)
    handler.send_header("Content-Type", "application/json")
    handler.send_header("Content-Length", str(len(body)))
    handler.send_header("Cache-Control", "no-store")
    handler.end_headers()
    handler.wfile.write(body)


# LAN/internet may only pull published content. Everything else is this
# machine's admin console (Drive, patches, update, settings) and must not
# answer on 0.0.0.0.
_SHARE_GET = frozenset((
    "/api/registry/list",
    "/api/registry/manifest",
    "/api/registry/file",
    "/api/registry/pack",
    # read-only live map. No start/stop, no admin.
    "/live",
    "/live.html",
    "/live.js",
    "/style.css",
    "/api/live",
    "/api/live/track",
    "/api/live/board",
    "/api/live/link",
))


class Handler(BaseHTTPRequestHandler):
    server_version = "ACECM"
    # Per-file track downloads used HTTP/1.0 (new TCP every file). 1.1
    # lets a client reuse the socket; the real speed win is the pack.
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def _peer_local(self):
        ip = (self.client_address or ("",))[0]
        return ip in ("127.0.0.1", "::1", "::ffff:127.0.0.1")

    def _deny_remote(self):
        return _json(self, {
            "ok": False,
            "error": "that action is only allowed from this PC",
        }, 403)

    # ---------------------------------------------------------------- GET --
    def do_GET(self):
        path, _, qs = self.path.partition("?")
        q = urllib.parse.parse_qs(qs)
        if not self._peer_local() and path not in _SHARE_GET:
            return self._deny_remote()
        try:
            if path == "/api/state":
                return _json(self, self._state())
            if path == "/api/install":
                return _json(self, installer.status())
            if path == "/api/overview":
                return _json(self, overview.overview())
            if path == "/api/cars":
                return _json(self, content.cars())
            if path == "/api/cars/map":
                from . import carsmap
                tab = carsmap.table((q.get("refresh") or [""])[0] == "1")
                return _json(self, {
                    "presets": tab.get("presets"),
                    "models": tab.get("models"),
                    "count": len(tab.get("presets") or {}),
                    "error": tab.get("error"),
                })
            if path == "/api/lobby":
                return _json(self, {"path": lobby.PATH, "lobby": lobby.read(),
                                    "lan_ip": netutil.lan_ipv4()})
            if path == "/api/game/worker":
                return _json(self, realai.worker_status())
            if path == "/api/drive":
                return _json(self, drive.options())
            if path == "/api/drive/status":
                return _json(self, drive.status())
            # 3D viewer: which cars can be shown, and how a pending
            # extraction is getting on
            # --- joining someone else's server ---------------------------
            # Delivery lives in registry.py; these answer the questions that
            # come first - what do I already have, and what would this server
            # cost me?
            if path == "/api/browser/status":
                return _json(self, dict(_INSTALL))
            if path == "/api/browser/local":
                return _json(self, contentsync.local(
                    (q.get("refresh") or [""])[0] == "1"))
            if path == "/api/browser/discover":
                # ⚠ Two ports, briefly. Seven ports at 4s each is 28 seconds of
                # a blocked request before we can say "that host is not running
                # ACECM" - long enough that the webview reports the app as not
                # responding, which reads as a crash rather than an answer.
                # A pasted share URL is one address: give it longer, it is
                # often on the other side of someone's router.
                raw = (q.get("host") or [""])[0]
                _h, explicit, given = contentsync.parse_target(raw)
                to = 8.0 if (given or explicit) else 2.0
                return _json(self, contentsync.discover(
                    raw, (q.get("port") or [None])[0],
                    ports=(8092, 8093), timeout=to))
            if path == "/api/livery":
                # the garage, and what each car is allowed to wear
                from . import livery
                model = (q.get("model") or [""])[0]
                out = {"ok": True, "cars": livery.garage()}
                if model:
                    out["allowed"] = livery.allowed(model)
                    out["designs"] = [
                        {"name": d["name"], "slots": sorted(d["slots"])}
                        for d in livery.designs(model)]
                return _json(self, out)
            if path == "/api/share":
                return _json(self, contentsync.share_info())
            if path == "/api/share/auto":
                # what each server needs, what it is missing, what we publish
                from . import autoshare
                out = []
                for p in servers.load():
                    out.append({"id": p.get("id"), "name": p.get("name"),
                                "needs": autoshare.needs(p),
                                "gaps": autoshare.server_gaps(p)})
                return _json(self, {"ok": True, "servers": out})
            if path in ("/live", "/live.html"):
                return self._static("/live.html")
            if path == "/live.js":
                return self._static("/live.js")
            if path == "/api/live":
                return _json(self, telemetry.public_cars(
                    (q.get("id") or [None])[0] or None))
            if path == "/api/live/track":
                return _json(self, telemetry.public_track(
                    (q.get("id") or [None])[0] or None))
            if path == "/api/live/board":
                return _json(self, telemetry.public_board(
                    (q.get("id") or [None])[0] or None))
            if path == "/api/live/link":
                return _json(self, telemetry.live_link())
            if path == "/api/browser/tag":
                ips = q.get("ip") or []
                if not ips and q.get("ips"):
                    ips = (q.get("ips") or [""])[0].split(",")
                return _json(self, contentsync.tag_hosts(ips))
            if path == "/api/browser/needs":
                # servers running something this machine cannot load, from the
                # remembered list - answerable with the game closed
                lst = backend.server_list()
                miss = contentsync.needs(lst.get("servers") or [])
                return _json(self, {"ok": lst.get("ok"),
                                    "cached": lst.get("cached", False),
                                    "captured_at": lst.get("captured_at"),
                                    "total": len(lst.get("servers") or []),
                                    "missing": miss})
            if path == "/api/browser/plan":
                return _json(self, contentsync.plan(
                    (q.get("base") or [""])[0], (q.get("id") or [""])[0]))
            # --- pictures for the car and track lists --------------------
            if path == "/api/thumb/car":
                # Cached PNG only for list rows: rendering here blocked the UI
                # and spawned an evoview console on every Drive keystroke.
                # ⚠ big=1 is the exception and DOES render, because it is one
                # car the user has deliberately opened - and it is the only way
                # the detail pane gets a picture that is not upscaled. force=1
                # replaces a stale render rather than reusing it.
                big = (q.get("big") or ["0"])[0] == "1"
                force = (q.get("force") or ["0"])[0] == "1"
                cid = (q.get("id") or [""])[0]
                if big:
                    shot = thumbs.render_car(cid, big=True, force=force,
                                             make=True)
                    # fall back to the small one rather than showing nothing
                    return self._send_png(shot or thumbs.render_car(
                        cid, make=False))
                return self._send_png(thumbs.render_car(cid, make=False,
                                                        force=force))
            if path == "/api/thumb/track":
                return self._send_png(thumbs.track_cover(
                    (q.get("folder") or [""])[0]))
            if path == "/api/thumbs/status":
                return _json(self, {**thumbs.job(),
                                    "have": sorted(thumbs.have()),
                                    "covers": thumbs.covers_available(),
                                    "covers_have": sorted(thumbs.covers_have()),
                                    "cover_job": thumbs.cover_job()})
            if path == "/api/viewer/cars":
                return _json(self, viewer.index(
                    (q.get("refresh") or [""])[0] == "1"))
            if path == "/api/viewer/job":
                return _json(self, viewer.job((q.get("id") or [""])[0]))
            if path == "/api/viewer/status":
                return _json(self, {
                    "exe": viewer.viewer_exe(),
                    "package": viewer.package(),
                })
            if path == "/api/models":
                return _json(self, content.models_seen())
            if path == "/api/mods":
                return _json(self, install.installed(
                    (q.get("side") or ["server"])[0]))
            if path == "/api/mods/audit":
                return _json(self, install.audit())
            if path == "/api/mods/scan":
                return _json(self, install.scan_source((q.get("path") or [""])[0]))
            if path == "/api/library":
                return _json(self, install.library())
            if path == "/api/library/clip":
                return _json(self, install.clip_item(
                    (q.get("kind") or [""])[0],
                    (q.get("name") or [""])[0]))
            if path == "/api/library/export":
                return self._library_export(
                    (q.get("kind") or [""])[0],
                    (q.get("name") or [""])[0])
            if path == "/api/trackdeploy":
                return _json(self, {**trackdeploy.state(),
                                    **trackdeploy.packages(
                                        (q.get("dir") or [None])[0]),
                                    # tracks already imported here, which is
                                    # how anyone actually gets one
                                    "imported": trackdeploy.importable()})
            if path == "/api/tracks/installed":
                return _json(self, install.tracks_installed())
            if path == "/api/splines":
                from . import splines
                return _json(self, splines.status())
            if path == "/api/tracks":
                return _json(self, content.tracks())
            if path == "/api/profiles":
                items = servers.load()
                for it in items:
                    it["car_policy"] = servers.car_policy(it)
                    if "allow_kunos" not in it:
                        it["allow_kunos"] = servers.infer_allow_kunos(it)
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
            if path == "/api/registry/pack":
                return self._serve_pack((q.get("id") or [""])[0],
                                        (q.get("track") or [""])[0])
            if path == "/api/gamesettings":
                return _json(self, {**gamesettings.state(),
                                    **gamesettings.discover()})
            if path == "/api/gamesettings/export":
                only = [x for x in (q.get("file") or []) if x]
                return _json(self, gamesettings.export_bundle(only or None))
            if path == "/api/gamesettings/backups":
                return _json(self, gamesettings.backups(
                    (q.get("file") or [""])[0]))
            if path == "/api/gamesettings/read":
                return _json(self, gamesettings.read(
                    (q.get("file") or [""])[0]))
            if path == "/api/patches":
                return _json(self, patching.overview())
            if path == "/api/patches/inspect":
                return _json(self, patching.inspect(
                    (q.get("target") or [config.server_exe() or ""])[0]))
            if path == "/api/backend":
                return _json(self, backend.state())
            if path == "/api/telemetry":
                return _json(self, telemetry.cars((q.get("id") or [None])[0]))
            if path == "/api/detect":
                return _json(self, detect.all_paths(
                    (q.get("refresh") or ["0"])[0] == "1"))
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
                # ?force=1 from the Updater page's own button; the dashboard's
                # per-render call takes the cached answer.
                force = (q.get("force") or ["0"])[0] == "1"
                return _json(self, version.check(force=force))
            if path == "/api/telemetry/leaderboard":
                return _json(self, telemetry.leaderboard(
                    (q.get("id") or [None])[0]))
            if path == "/api/telemetry/track":
                return _json(self, telemetry.track((q.get("id") or [None])[0]))
            if path == "/api/telemetry/status":
                return _json(self, telemetry.status_all())
            if path == "/api/browser/why":
                return _json(self, backend.browser_chain())
            if path == "/api/browser":
                return _json(self, backend.server_list())
            if path == "/api/join/state":
                return _json(self, backend.join_state())
            if path == "/api/backend/log":
                mode = (q.get("mode") or ["proxy"])[0]
                return _json(self, backend.log(mode))
            if path == "/api/config":
                return _json(self, config.public_cfg())
            return self._static(path)
        except Exception as ex:
            # ⚠ Log the TRACEBACK. This used to return a one-line message and
            # throw the stack away, which made every API failure a guessing game.
            logs.exception(f"GET {self.path}", ex)
            return _json(self, {"error": f"{type(ex).__name__}: {ex}",
                                "see": "Logs tab"}, 500)

    # --------------------------------------------------------------- POST --
    def do_POST(self):
        path, _, qs = self.path.partition("?")
        if not self._peer_local():
            return self._deny_remote()
        # Drag-drop streams the file; do not parse it as JSON.
        if path == "/api/drop/part":
            return self._drop_part(urllib.parse.parse_qs(qs))
        n = int(self.headers.get("Content-Length") or 0)
        try:
            body = json.loads(self.rfile.read(n) or b"{}")
        except Exception:
            body = {}
        try:
            if path == "/api/viewer/open":
                return _json(self, viewer.start_open(
                    body.get("id") or "", body.get("paint") or ""))
            if path == "/api/viewer/open_track":
                return _json(self, viewer.start_open_track(
                    body.get("folder") or body.get("id") or ""))
            if path == "/api/profiles/save":
                return _json(self, servers.upsert(body))
            if path == "/api/profiles/delete":
                servers.delete(body.get("id"))
                return _json(self, {"ok": True})
            if path == "/api/mods/install":
                return _json(self, install.install(body.get("path"),
                                                   body.get("only"),
                                                   overwrite=bool(body.get("overwrite"))))
            if path == "/api/drop":
                if body.get("cancel") and body.get("id"):
                    install.drop_cleanup(body.get("id"))
                    return _json(self, {"ok": True, "cancelled": True})
                ow = bool(body.get("overwrite"))
                if body.get("id"):
                    return _json(self, install.ingest_staging(
                        body.get("id"), overwrite=ow))
                return _json(self, install.ingest(body.get("path"),
                                                  overwrite=ow))
            if path == "/api/library/remove":
                return _json(self, install.remove_item(
                    body.get("kind"), body.get("name")))
            if path == "/api/library/export":
                return _json(self, install.export_item(
                    body.get("kind"), body.get("name"),
                    body.get("dest") or None))
            if path == "/api/mods/fix":
                return _json(self, install.apply_fix(body.get("name")))
            if path == "/api/mods/remove":
                return _json(self, install.remove(body.get("name")))
            if path == "/api/mods/sync":
                return _json(self, install.sync(
                    body.get("direction") or "to_server",
                    bool(body.get("force")),
                    body.get("names")))
            if path == "/api/app/restart":
                return _json(self, installer.restart())
            if path == "/api/livery/apply":
                # ⚠ Writes the player's garage. It backs the record up first,
                # and only ever accepts a colour the car's own design allows -
                # an arbitrary path is what crashed the game while this was
                # being worked out.
                from . import livery
                car = body.get("file") or ""
                slot = body.get("slot") or "EXT SKIN"
                color = body.get("color") or ""
                model = body.get("model") or ""
                ok_list = livery.allowed(model).get(slot, []) if model else []
                if ok_list and color not in ok_list:
                    return _json(self, {
                        "ok": False,
                        "error": "that colour is not offered for this car"})
                return _json(self, livery.apply_color(car, slot, color))
            if path == "/api/app/show":
                # A second launch asks the instance that owns the window to
                # raise it, rather than opening a rival webview on the same
                # profile - see ui.focus for why that matters.
                from . import ui
                return _json(self, {"ok": ui.focus()})
            if path == "/api/install/run":
                return _json(self, installer.install(
                    desktop=body.get("desktop", True)))
            if path == "/api/install/remove":
                return _json(self, installer.uninstall(
                    remove_exe=bool(body.get("purge"))))
            if path == "/api/thumbs/covers":
                return _json(self, thumbs.build_covers(
                    bool(body.get("force"))))
            if path == "/api/thumbs/build":
                return _json(self, thumbs.build_all(
                    bool(body.get("force"))))
            if path == "/api/browser/scan":
                lst = backend.server_list()
                pool = (contentsync.needs(lst.get("servers") or [])
                        if body.get("only_missing", True)
                        else (lst.get("servers") or []))
                return _json(self, contentsync.scan(pool))
            if path == "/api/browser/install":
                return _json(self, _install_content(body))
            if path == "/api/trackdeploy/deploy":
                # native = install at the track's own paths, leaving every
                # stock track intact; the old path borrows Road Atlanta's slots
                if body.get("native"):
                    return _json(self, trackdeploy.deploy_native(
                        body.get("path"), dry_run=bool(body.get("dry_run"))))
                return _json(self, trackdeploy.deploy(body.get("path")))
            if path == "/api/trackdeploy/restore":
                return _json(self, trackdeploy.restore())
            if path == "/api/splines/ship":
                from . import splines
                if body.get("all"):
                    return _json(self, splines.ship_all())
                return _json(self, splines.ship(
                    body.get("folder"), body.get("layout")))
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
                # ⚠ honour the id. Ignoring it made every "stop this server"
                # stop all of them.
                return _json(self, servers.stop(body.get("id")))
            if path == "/api/update/apply":
                return _json(self, version.apply())
            if path == "/api/gamesettings/import":
                return _json(self, gamesettings.import_bundle(
                    body.get("bundle") or {}, body.get("only"),
                    bool(body.get("include_devices"))))
            if path == "/api/gamesettings/restore_backup":
                return _json(self, gamesettings.restore_backup(
                    body.get("file"), body.get("name")))
            if path == "/api/backend/start":
                return _json(self, backend.start(body.get("mode", "proxy")))
            if path == "/api/backend/stop":
                return _json(self, backend.stop())
            if path == "/api/backend/redirect":
                action = (body.get("action") or "apply").lower()
                if action == "restore":
                    return _json(self, backend.restore_redirect())
                return _json(self, backend.apply_redirect())
            if path == "/api/join":
                return _json(self, backend.join(body.get("id"),
                                                body.get("shape", "bare")))
            if path == "/api/game/launch":
                return _json(self, backend.launch_game())
            if path == "/api/game/launch_ai":
                return _json(self, realai.launch(
                    int(body.get("opponents") or 16),
                    int(body.get("min_strength") or 70),
                    int(body.get("max_strength") or 95),
                    bool(body.get("small_window"))))
            if path == "/api/game/attach_worker":
                return _json(self, realai.attach_worker(
                    body.get("id") or "",
                    body.get("ai_player", True)))
            if path == "/api/drive":
                return _json(self, drive.start(body))
            if path == "/api/drive/capture":
                return _json(self, drive.capture_list())
            if path == "/api/drive/list":
                return _json(self, drive.publish_local(
                    body.get("id") or body.get("local_id")))
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
            "server_exe_ok": bool(config.server_exe())
                             and os.path.exists(config.server_exe()),
            "server_exe": config.server_exe(),
            "server_exe_found": detect.server_candidates(),
            "tools_ok": os.path.isdir(config.tools_dir()),
        }

    def _send_png(self, path):
        """A cached image, or 404 so the UI can leave the tile blank."""
        if not path or not os.path.isfile(path):
            return _json(self, {"error": "no image"}, 404)
        data = open(path, "rb").read()
        self.send_response(200)
        self.send_header("Content-Type", "image/png")
        self.send_header("Content-Length", str(len(data)))
        # cheap to regenerate, and a stale car render is confusing after a
        # repaint - keep it short rather than forever
        self.send_header("Cache-Control", "max-age=300")
        self.end_headers()
        self.wfile.write(data)

    def _base(self):
        host = self.headers.get("Host") or f"127.0.0.1:{config.CFG['ui_port']}"
        return f"http://{host}"

    def _drop_part(self, q):
        """One file from a window drop, streamed to a staging folder."""
        did = (q.get("id") or [""])[0]
        name = (q.get("name") or [self.headers.get("X-Filename") or ""])[0]
        n = int(self.headers.get("Content-Length") or 0)
        r = install.drop_part(did, name, self.rfile, size=n)
        return _json(self, r, 200 if r.get("ok") else 400)

    def _library_export(self, kind, name):
        """Build the pack and stream it (or JSON-error if it cannot)."""
        r = install.export_item(kind, name)
        if not r.get("ok"):
            return _json(self, r, 400)
        path = r.get("path")
        if not path or not os.path.isfile(path):
            return _json(self, {"error": "export file missing"}, 500)
        filename = os.path.basename(path)
        ctype = ("application/zip" if filename.lower().endswith(".zip")
                 else "application/x-tar")
        size = os.path.getsize(path)
        self.send_response(200)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(size))
        self.send_header("Content-Disposition",
                         f'attachment; filename="{filename}"')
        self.end_headers()
        with open(path, "rb") as fh:
            while True:
                chunk = fh.read(1 << 20)
                if not chunk:
                    break
                try:
                    self.wfile.write(chunk)
                except (BrokenPipeError, ConnectionResetError):
                    return

    def _serve_pack(self, sid, folder):
        """One tar for a whole shared track — see registry.ensure_track_pack."""
        folder = (folder or "").strip()
        entry = next((e for e in registry.load() if e["id"] == sid), None)
        if not entry or folder not in (entry.get("required_tracks") or []):
            return _json(self, {"error": "track is not shared on that entry"}, 404)
        try:
            packed = registry.ensure_track_pack(folder)
        except Exception as ex:
            logs.LOG.exception("track pack %s", folder)
            return _json(self, {"error": f"pack failed: {ex}"}, 500)
        if not packed or not os.path.isfile(packed):
            return _json(self, {"error": "no files for that track"}, 404)
        size = os.path.getsize(packed)
        sha = registry.track_pack_sha(folder)
        self.send_response(200)
        self.send_header("Content-Type", "application/x-tar")
        self.send_header("Content-Length", str(size))
        if sha:
            self.send_header("X-ACECM-SHA256", sha)
        self.send_header("Content-Disposition",
                         f'attachment; filename="{folder}.tar"')
        self.end_headers()
        with open(packed, "rb") as fh:
            while True:
                chunk = fh.read(1 << 20)
                if not chunk:
                    break
                try:
                    self.wfile.write(chunk)
                except (BrokenPipeError, ConnectionResetError):
                    return

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
        web = os.path.abspath(config.WEB)
        full = os.path.abspath(full)
        try:
            inside = os.path.commonpath([web, full]) == web
        except ValueError:
            inside = False
        if not inside or not os.path.isfile(full):
            self.send_error(404)
            return
        ctype = mimetypes.guess_type(full)[0] or "application/octet-stream"
        data = open(full, "rb").read()
        # Bust the WebView2 disk / V8 code cache on every new build. A
        # persistent profile otherwise keeps serving yesterday's app.js.
        if rel == "index.html":
            tag = version.VERSION.encode("ascii", "replace")
            data = data.replace(b'href="/style.css"',
                                b'href="/style.css?v=' + tag + b'"')
            data = data.replace(b'src="/app.js"',
                                b'src="/app.js?v=' + tag + b'"')
        self.send_response(200)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(data)


def _allow_share_port(port):
    """Ask Windows Firewall to allow inbound TCP on the content port.

    Friends need inbound TCP on this port for Get content. Admin routes
    refuse non-loopback peers, so the hole is share GETs only. Best-effort
    — a non-admin start just logs and continues.
    """
    name = f"ACECM-share-{int(port)}"
    old = f"ACECM-content-{int(port)}"
    try:
        # Drop the old any-profile rule if we created one. Public Wi‑Fi
        # does not need this port open.
        subprocess.run(
            ["netsh", "advfirewall", "firewall", "delete", "rule",
             f"name={old}"],
            capture_output=True, text=True, timeout=8)
        r = subprocess.run(
            ["netsh", "advfirewall", "firewall", "show", "rule",
             f"name={name}"],
            capture_output=True, text=True, timeout=8)
        if r.returncode == 0 and name in (r.stdout or ""):
            return
        r = subprocess.run(
            ["netsh", "advfirewall", "firewall", "add", "rule",
             f"name={name}", "dir=in", "action=allow", "protocol=TCP",
             f"localport={int(port)}", "profile=private"],
            capture_output=True, text=True, timeout=8)
        if r.returncode == 0:
            logs.LOG.info("firewall: allowed inbound TCP %s on private networks",
                          port)
        else:
            logs.LOG.warning("firewall: could not open TCP %s (need admin?): %s",
                             port, (r.stderr or r.stdout or "").strip()[:200])
    except Exception as ex:
        logs.LOG.warning("firewall: %s", ex)


class App(socketserver.ThreadingTCPServer):
    daemon_threads = True
    # NOT reusable: on Windows a second process can otherwise bind the same
    # port silently and requests land on whichever wins - that cost real
    # debugging time on the telemetry tools.
    allow_reuse_address = False


def _watch_lobby():
    """Keep lobby.json describing the server that is actually running.

    ⚠ The backend re-reads that file to build the entry players see, and it is
    written from a PROFILE at launch - so it goes stale the moment the two
    disagree (a server started outside ACECM, a profile edited afterwards).
    Correcting it only when ACECM reads it fixes ACECM's own display and leaves
    the game advertising the old track.

    Only does the expensive read when the set of server processes CHANGES, so
    an idle machine costs nothing.
    """
    seen = None

    def run():
        nonlocal seen
        while True:
            try:
                now = tuple(sorted(servers._server_pids()))
                if now != seen:
                    seen = now
                    lobby.refresh()
            except Exception as ex:
                logs.LOG.debug("lobby watch: %s", ex)
            time.sleep(5)

    threading.Thread(target=run, daemon=True).start()


def _rescan_content():
    """Forget what content we THINK exists, and look again.

    ⚠ Content is cached against the archives it came from, which does not cover
    everything: a mod folder emptied by hand, a track deleted, a package
    restored from a backup with its old timestamps. The symptom is ACECM
    insisting content is missing when it is sitting right there, and the only
    reliable cure a user finds is deleting the data folder.

    A restart is the natural moment to look again - it costs a couple of
    seconds and it is exactly what someone restarting the app is asking for.
    Expensive caches are left alone: file hashes are keyed per file on
    size+mtime, and car renders are keyed on the car itself.
    """
    stale = [os.path.join(config.DATA, "track_map.json"),
             os.path.join(config.DATA, "viewer", "index.json")]
    gone = 0
    for f in stale:
        try:
            if os.path.isfile(f):
                os.remove(f)
                gone += 1
        except OSError as ex:
            logs.LOG.warning("could not clear %s: %s", f, ex)
    if gone:
        logs.LOG.info("content caches cleared (%d) - rescanning on demand", gone)


def _auto_proxy():
    return bool(config.CFG.get("auto_proxy", True))


def _autostart_proxy():
    """Background: start the lobby proxy if Settings says so."""
    if not _auto_proxy():
        return
    def go():
        try:
            from . import backend
            st = backend.state()
            if st.get("listening"):
                logs.LOG.info("auto proxy: already listening")
                return
            r = backend.start("proxy")
            if r.get("ok"):
                logs.LOG.info("auto proxy started on :%s pid=%s",
                              r.get("port"), r.get("pid"))
            else:
                logs.LOG.warning("auto proxy failed: %s",
                                 r.get("error") or r)
        except Exception as ex:
            logs.exception("auto proxy", ex)
    threading.Thread(target=go, daemon=True, name="auto-proxy").start()


def _autostop_proxy():
    """Kill the lobby proxy only when ACECM itself is going away."""
    if not _auto_proxy():
        return
    try:
        from . import backend
        backend.stop()
        logs.LOG.info("auto proxy stopped with ACECM")
    except Exception as ex:
        logs.LOG.warning("auto proxy stop: %s", ex)


class AlreadyRunning(Exception):
    """An ACECM is already serving; its URL is the argument."""


def _acecm_answering(port):
    """Is the thing on this port one of ours?"""
    import urllib.request
    try:
        with urllib.request.urlopen(
                f"http://127.0.0.1:{port}/api/version", timeout=2.5) as r:
            return (json.loads(r.read()) or {}).get("name") == version.NAME
    except Exception:
        return False


def _fatal(msg):
    """Say something the user can actually see, then stop.

    Under pythonw there is no console, so this puts the reason in a message box
    as well as the log. A silent no-op on double-click is the worst possible
    failure: it looks like the app is broken rather than like something needs
    closing.
    """
    logs.LOG.error("%s", msg.replace("\n\n", " "))
    try:
        import ctypes
        ctypes.windll.user32.MessageBoxW(
            None, msg, f"{version.NAME} could not start", 0x10)
    except Exception:
        pass
    raise SystemExit(msg)
    # ⚠ 127.0.0.1, NOT "localhost". Windows resolves localhost to ::1 first and
    # we listen on IPv4 only, and the fallback to IPv4 costs ~2 SECONDS PER
    # CONNECTION on this stack - measured: 4 ms via 127.0.0.1, 2018 ms via
    # localhost, for the very same request. The browser opens several parallel
    # connections and new ones whenever an idle one is reaped, so that penalty
    # is paid over and over while the app is in use. This one word was the
    # single largest cause of the UI feeling slow; nothing in the Python was
    # ever the bottleneck.


def serve():
    """Bind and start the HTTP server on a background thread; return its URL.

    Split out from main() so a native window can own the MAIN thread - GUI
    toolkits require that - while the API keeps serving behind it.
    """
    logs.setup()
    from . import version
    version.consume_rollback()
    _rescan_content()
    _watch_lobby()
    port = config.CFG["ui_port"]
    # ⚠ 127.0.0.1 made the window work and made content sharing a lie.
    # Get content probes this HTTP port on the game server's IP. Bound
    # only to loopback, a friend on the LAN or the internet can never
    # reach it - "can't reach the IP" - even with 9700 forwarded.
    listen = (config.CFG.get("listen") or "0.0.0.0").strip() or "0.0.0.0"
    # ⚠ Wait for a HANDOVER before deciding the port is taken. After an update
    # (and after Restart) the replacement starts while the outgoing copy is
    # still letting go of the socket: it is gone as a process, but the port is
    # briefly still bound. Binding once and giving up meant the new copy saw
    # "already running", tried to raise a window on an instance that was
    # dying, and exited - so the app did not come back and you had to start it
    # a second time. Retry for a few seconds; a genuinely running instance is
    # still detected, it just costs a moment longer to say so.
    srv = None
    deadline = time.time() + 12.0
    last = None
    while True:
        try:
            srv = App((listen, port), Handler)
            break
        except OSError as ex:
            last = ex
            # ⚠ A JUST-UPDATED exe must never defer. It was started by the swap
            # script the moment the old one exited, so the copy still answering
            # on the port is the outgoing build finishing its shutdown - not a
            # second instance. Deferring to it meant the new build quit, the
            # script saw "running" and kept the swap, and the user was left
            # with nothing on screen and no way back but launching by hand.
            # Their update log said it plainly: "new exe is running without a
            # flag - keeping it", and then nothing.
            if not _JUST_UPDATED and _acecm_answering(port):
                break
            if time.time() >= deadline:
                break
            time.sleep(0.5)
    if srv is None:
        ex = last
        # ⚠ Do not just die here. run.bat starts us with pythonw, which has no
        # console, so a SystemExit message goes precisely nowhere: the user
        # double-clicks, nothing happens, and there is nothing to read. Worse,
        # the usual cause is a LEFTOVER ACECM with no window - so the app is
        # "already running" in a way nobody can see or act on.
        if _acecm_answering(port):
            # It really is ACECM. Show that one instead of refusing to start;
            # a second launch should bring the app up, not report an error.
            logs.LOG.info("ACECM is already serving on %s - opening that "
                          "instance instead of starting a second one", port)
            raise AlreadyRunning(f"http://127.0.0.1:{port}")
        _fatal(f"Port {port} is in use by something that is not ACECM, so the "
               f"app cannot start.\n\n{ex}\n\nClose whatever is using port "
               f"{port}, or change ui_port in ACECM's settings file:\n"
               f"{os.path.join(config.DATA, 'config.json')}")


    url = f"http://127.0.0.1:{port}"
    lan = ""
    try:
        from . import netutil
        lan = netutil.lan_ipv4() or ""
    except Exception:
        pass
    _allow_share_port(port)
    print(f"Assetto Corsa EVO Content Manager\n  {url}")
    if lan:
        print(f"  share     : http://{lan}:{port}  (TCP {port} must be reachable)")
    print(f"  server dir : {config.server_dir()}")
    print(f"  tools dir  : {config.tools_dir()}")
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv, url


def main(mode="window", okflag=None, relaunch=False):
    """mode: window (native), browser (default browser), headless (serve only).

    ⚠ `relaunch` covers BOTH ways one copy replaces another: the update swap
    (which also passes okflag) and the Restart button. Only the update case
    was handled before, so Restart still deferred to the outgoing copy and
    exited - which is why it never came back.
    """
    global _JUST_UPDATED
    _JUST_UPDATED = bool(okflag) or bool(relaunch)
    try:
        srv, url = serve()
    except AlreadyRunning as ex:
        # Second launch: ask the instance that is already there to show itself.
        # ⚠ Do NOT open our own window here. Two webviews cannot share one
        # WebView2 user-data folder: the newcomer takes the profile and the
        # running instance dies, so "launch twice" became "close the app".
        url = str(ex)
        # ⚠ CONFIRM THE UPDATE ANYWAY. This binary started fine - another
        # instance simply owns the port. The swap script restores the previous
        # exe unless the new one writes its handshake file, so returning here
        # without it made an update roll itself back: press Restart after
        # updating and you land on the OLD version, with nothing explaining
        # why. Deferring to a running instance is not a failed start.
        from . import version as _v
        _v.confirm_update(okflag)
        import urllib.request
        shown = False
        try:
            req = urllib.request.Request(url + "/api/app/show", data=b"{}",
                                         headers={"Content-Type":
                                                  "application/json"})
            with urllib.request.urlopen(req, timeout=5) as r:
                shown = bool((json.loads(r.read()) or {}).get("ok"))
        except Exception as exc:
            logs.LOG.info("could not raise the running window: %s", exc)
        logs.LOG.info("ACECM already running; %s",
                      "raised its window" if shown
                      else "opening it in the browser instead")
        if not shown:
            # Headless instance (no window to raise), so give the user
            # something rather than nothing.
            os.startfile(url)      # noqa: S606
        return
    # HTTP is up — this binary is good enough to keep. Write the flag
    # the swap script is waiting on BEFORE opening the window, so a
    # later window close is not mistaken for a failed update.
    from . import version
    version.confirm_update(okflag)
    _autostart_proxy()
    try:
        if mode == "window":
            from . import ui
            if not ui.available():
                raise SystemExit(
                    "no native window (WebView2 missing) - ACECM is a desktop app")
            ui.run(url)              # blocks until the window is closed
            return
        if mode == "browser":
            threading.Timer(0.6, lambda: os.startfile(url)).start()   # noqa: S606
        try:
            while True:
                time.sleep(3600)
        except KeyboardInterrupt:
            pass
    finally:
        _autostop_proxy()


if __name__ == "__main__":
    main()
