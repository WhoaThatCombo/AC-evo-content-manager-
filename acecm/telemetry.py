"""Live car positions, read out of a dedicated server process.

The server never publishes coordinates - they travel over the netcode UDP path,
which is not logged, and its HTTP port returns only a client count. But the
server RUNS the physics, so the positions are in its memory. `server_telemetry.py`
finds them by diffing which float triplets inside the track bounding box MOVE;
cars are the only thing in there that does.

⭐ Telemetry is PER SERVER. A tracker attaches to one process and serves one
endpoint, so a shared port could only ever describe one server. Each profile
gets its own tracker, on its own port, bound to that server's pid and reading
that server's log - so several servers can be watched at the same time.

⭐ With NO AI on the grid, every mover IS a player. That removes the hard part:
when exactly one connected player and one car are unattributed, the pairing is
forced rather than guessed.

⚠ A car that never moves cannot be seen at all - movement is the only signal.
A player sitting in the pits shows in the player count but not on the map.
"""
import json
import math
import os
import re
import subprocess
import sys
import time
import urllib.request

from . import config, logs, servers

TELEM_URL = "http://127.0.0.1:{}/"
TRACKERS = os.path.join(config.DATA, "trackers.json")


# ---------------------------------------------------------------- registry --
def _load():
    """profile id -> {port, pid, server_pid}"""
    try:
        return json.load(open(TRACKERS, encoding="utf-8"))
    except Exception:
        return {}


def _save(t):
    json.dump(t, open(TRACKERS, "w", encoding="utf-8"), indent=2)


def _free(port):
    """Nothing at all is listening here."""
    import socket
    with socket.socket() as s:
        s.settimeout(0.4)
        return s.connect_ex(("127.0.0.1", port)) != 0


def default_profile():
    """Which server the telemetry page means when none was chosen.

    ⚠ Never fall back to "the first profile". The map used to resolve that way
    while the cars came from "the first tracker", so opening Telemetry from the
    nav drew NURBURGRING under cars from a different server entirely - it simply
    looked like the wrong track. Prefer a server that actually has a tracker,
    and use the SAME profile for both halves.
    """
    t = _load()
    profs = servers.load()
    live = [p["id"] for p in profs
            if t.get(p["id"]) and running(t[p["id"]]["port"])]
    if live:
        return live[0]
    if t:
        return next(iter(t))
    return profs[0]["id"] if profs else None


def _port_for(profile_id):
    """A stable, unique port per profile, handed out from the base upwards.

    ⚠ Skipping only ports in our own registry is not enough. A tracker left
    over from an earlier run - or anything else - can hold the base port, and
    then every profile is handed the same number and they all "find" that one
    stranger. Require the port to be genuinely unoccupied.
    """
    t = _load()
    if profile_id in t:
        return t[profile_id]["port"]
    base = int(config.CFG.get("telemetry_port", 8091))
    used = {v["port"] for v in t.values()}
    port = base
    while port in used or not _free(port):
        port += 1
        if port > base + 200:
            raise RuntimeError("no free telemetry port")
    return port


def _url(port):
    return TELEM_URL.format(port)


def running(port):
    try:
        with urllib.request.urlopen(_url(port), timeout=1.5):
            return True
    except Exception:
        return False


def status_all():
    """Telemetry state for every profile, for the Servers tab."""
    t = _load()
    out = {}
    for prof in servers.load():
        rec = t.get(prof["id"])
        out[prof["id"]] = {
            "port": rec["port"] if rec else None,
            "running": bool(rec and running(rec["port"])),
            "server_pid": rec.get("server_pid") if rec else None,
        }
    return out


# ------------------------------------------------------------------ start --
def start(profile_id, baseline_ai=False):
    """Start a tracker bound to ONE profile's server process."""
    prof = next((p for p in servers.load() if p["id"] == profile_id), None)
    if not prof:
        return {"ok": False, "error": "no such profile"}
    st = servers.status(prof)
    if not st.get("running"):
        return {"ok": False, "error": "that server is not running"}
    server_pid = st.get("pid")
    if not server_pid:
        return {"ok": False, "error":
                "cannot tell which process is this server - start it from "
                "ACECM so its pid is recorded"}

    # ⚠ "the port answers" does NOT mean the tracker is ours. Only a registry
    # record proves this profile started it; without that check two profiles
    # both adopted one stale tracker and neither was ever really tracked.
    rec = _load().get(profile_id)
    if rec and running(rec["port"]):
        return {"ok": True, "already": True, "port": rec["port"]}
    port = _port_for(profile_id)

    script = config.tool_script("server_telemetry.py")
    if not os.path.isfile(script):
        return {"ok": False, "error": f"not found: {script}"}

    env = dict(os.environ)
    env["TELEM_PORT"] = str(port)              # one endpoint per server
    # The coordinate filter must match THIS server's track, not a baked-in one.
    bb = None
    try:
        bb = bbox(profile_id)
    except Exception:
        bb = None
    if bb:
        env["TELEM_BBOX"] = ",".join(f"{v:.1f}" for v in bb)
    # The centreline too: the box alone lets origin junk through on any track
    # near (0,0), which is how Barber reported phantom cars at (0,0,0).
    try:
        t = track(profile_id)
        if t.get("ok") and t.get("points"):
            pts = list(t["points"])
            # Include the PIT LANE, which can be well over 30 m from the racing
            # line - without it a car driving through the pits is filtered out
            # as off-track junk.
            pts += _pitlane_points(t.get("spline"))
            tp = os.path.join(config.DATA, f"track_{profile_id}.json")
            json.dump(pts, open(tp, "w", encoding="utf-8"))
            env["TELEM_TRACK"] = tp
    except Exception:
        pass
    targs = ["--pid", str(server_pid),        # THIS server, not "a" server
             "--log", config.server_log(prof.get("log", "vai_server.log"))]
    if baseline_ai:
        targs.append("--baseline-ai")
    args = config.tool_cmd("server_telemetry", targs)
    log = open(os.path.join(config.DATA, f"telemetry_{profile_id}.log"), "w",
               encoding="utf-8", errors="replace")
    p = subprocess.Popen(args, cwd=config.server_dir(), env=env,
                         stdout=log, stderr=subprocess.STDOUT)
    logs.launched(f"tracker for {prof.get('name')!r}", args, p.pid,
                  port=port, server_pid=server_pid,
                  bbox=env.get("TELEM_BBOX"), log=log.name)
    t = _load()
    t[profile_id] = {"port": port, "pid": p.pid, "server_pid": server_pid}
    _save(t)
    return {"ok": True, "port": port, "pid": p.pid, "server_pid": server_pid}


def stop(profile_id=None):
    """Stop one profile's tracker, or every tracker."""
    t = _load()
    targets = [profile_id] if profile_id else list(t)
    stopped = []
    for key in targets:
        rec = t.get(key)
        if not rec:
            continue
        subprocess.run(["powershell.exe", "-NoProfile", "-Command",
                        f"Stop-Process -Id {rec['pid']} -Force "
                        f"-ErrorAction SilentlyContinue"],
                       capture_output=True, timeout=20)
        stopped.append(key)
        t.pop(key, None)
    _save(t)
    return {"ok": True, "stopped": stopped}


# ------------------------------------------------------------------- cars --
def cars(profile_id=None, annotate=True):
    """Live cars for one profile's server, identities resolved where honest."""
    t = _load()
    if profile_id is None:
        profile_id = default_profile()
    rec = t.get(profile_id or "")
    if not rec:
        return {"ok": False, "error": "no tracker for that server",
                "hint": "start telemetry from the Servers tab"}
    prof = next((p for p in servers.load() if p["id"] == profile_id), None)
    try:
        with urllib.request.urlopen(_url(rec["port"]), timeout=3) as r:
            data = json.loads(r.read())
    except Exception as ex:
        return {"ok": False, "error": f"{type(ex).__name__}",
                "hint": f"tracker on port {rec['port']} is not answering"}

    rows = _dedupe(data.get("cars", []))
    players = _connected_players(
        config.server_log((prof or {}).get("log", "vai_server.log")))

    named = [c for c in rows if c.get("name")]
    unnamed = [c for c in rows if not c.get("name")]
    claimed = {c["name"] for c in named}
    free = [p for p in players if p["id"] not in claimed]

    inferred = 0
    # No bots => every mover is a player. One unattributed each way is forced.
    if len(free) == 1 and len(unnamed) == 1:
        unnamed[0].update({"name": free[0]["id"],
                           "display": free[0].get("display"),
                           "model": free[0].get("model"),
                           "ai": False, "inferred": True})
        inferred = 1

    # best lap per carId, so a predicted lap has a reference to scale.
    # ⚠ annotate=False when leaderboard() calls us - it needs the live car ids
    # and we need its best laps, so calling each other unguarded recursed
    # forever and hung every telemetry request.
    if annotate:
        try:
            lb = leaderboard(profile_id)
            best_by_car = {r["carid"]: r["best"] for r in lb.get("rows", [])}
        except Exception:
            best_by_car = {}
        rows = _lap_and_gaps(profile_id, rows, best_by_car)

    return {
        "ok": True, "cars": rows, "players": players,
        "profile_id": profile_id, "port": rec["port"],
        "server": (prof or {}).get("name"),
        "counts": {"cars": len(rows), "players_connected": len(players),
                   "named": len(named) + inferred, "inferred": inferred,
                   "unidentified": max(0, len(unnamed) - inferred)},
        "note": ("a car that never moves cannot be detected - movement is the "
                 "only signal"),
    }


def _dedupe(rows, tol=6.0):
    """Collapse the several memory slots one car exposes into one entry.

    A single car appears at more than one address holding near-identical
    coordinates (copies updated a frame apart), so one player rendered as two
    overlapping cars - observed live: "combo" and an "unidentified" car at
    exactly (3957, 4899) doing 73 km/h. Keep the NAMED representative so the
    identity is not lost to an anonymous duplicate.
    """
    kept = []
    for c in rows:
        for k in kept:
            if (abs(k["x"] - c["x"]) < tol and abs(k["z"] - c["z"]) < tol
                    and abs((k.get("y") or 0) - (c.get("y") or 0)) < tol):
                if not k.get("name") and c.get("name"):
                    k.update({key: c[key] for key in
                              ("name", "display", "model", "ai", "id")
                              if key in c})
                k["duplicates"] = k.get("duplicates", 1) + 1
                break
        else:
            kept.append(dict(c))
    return kept


def _connected_players(log=None):
    """Real players currently on that server, from its log."""
    log = log or config.server_log()
    try:
        txt = open(log, encoding="utf-8", errors="replace").read()
    except OSError:
        return []
    starts = [m.start() for m in re.finditer(r"Build release", txt)]
    if starts:
        txt = txt[starts[-1]:]
    live = {}
    for line in txt.splitlines():
        m = re.search(r"connecting gamecar (\S+) \((.*?) \| (\d+)\)", line)
        if m:
            live[m.group(1)] = {"carid": m.group(1), "id": m.group(3),
                                "display": m.group(2).strip() or None,
                                "model": None}
            continue
        m = re.search(r"(\S+) connected \(\w+\) on car (\S+), with new carId (\S+)",
                      line)
        if m and not m.group(1).upper().startswith("VAI-"):
            rec = live.setdefault(m.group(3), {"carid": m.group(3),
                                               "id": m.group(1),
                                               "display": None})
            rec["model"] = m.group(2)
            continue
        m = re.search(r"Disconnected carId (\S+)", line)
        if m:
            live.pop(m.group(1), None)
    return list(live.values())


# ------------------------------------------------------------------ track --
def bbox(profile_id=None, margin=120.0, down=5.0, up=30.0):
    """The track's own 3D extents, for the tracker's coordinate filter.

    ⚠ The tracker used to carry ONE hardcoded bounding box, which happened to
    be Nurburgring's (x 500-7000, y 250-700, z 500-6000). On any other track
    every car falls outside it and is discarded before the movement diff ever
    runs - Barber sits at x[-329,576] y[17,39] z[-518,208] and showed nothing
    at all while the server log clearly had a player driving. Derive it from
    the track instead.
    """
    t = track(profile_id, _with_y=True)
    if not t.get("ok"):
        return None
    pts = t["points3"]
    xs = [p[0] for p in pts]
    ys = [p[1] for p in pts]
    zs = [p[2] for p in pts]
    # ⚠ Vertical margin is ASYMMETRIC on purpose. A car can leave the ground,
    # but never sinks far below the lowest point of the track - and a symmetric
    # margin was still admitting y=0, which is exactly the junk we are trying
    # to exclude on a track that sits near the world origin.
    return (min(xs) - margin, max(xs) + margin,
            min(ys) - down, max(ys) + up,
            min(zs) - margin, max(zs) + margin)


LAP_RE = re.compile(r"New lap carId (\S+): (\d+):(\d+)\.(\d+)")

# --------------------------------------------------------------- lap maths --
# Where a car is AROUND the lap, not just where it is on the map. Everything
# timing-related (gaps, predicted lap) needs distance along the track, so each
# car is projected onto the racing line and read off as metres from the line's
# start.
_SPLINE = {}     # profile id -> (points, cumulative distance, total length)
_LAPS = {}       # (profile id, car key) -> live lap state


def _spline_cum(profile_id):
    """(points, cumulative distance per point, lap length) for a profile."""
    hit = _SPLINE.get(profile_id)
    if hit:
        return hit
    t = track(profile_id)
    if not t.get("ok") or not t.get("points"):
        return None
    pts = t["points"]
    cum, d = [0.0], 0.0
    for i in range(1, len(pts)):
        d += math.hypot(pts[i][0] - pts[i - 1][0], pts[i][1] - pts[i - 1][1])
        cum.append(d)
    # close the loop so the last segment back to the start is counted
    total = d + math.hypot(pts[0][0] - pts[-1][0], pts[0][1] - pts[-1][1])
    _SPLINE[profile_id] = (pts, cum, total)
    return _SPLINE[profile_id]


def _project(profile_id, x, z):
    """Distance around the lap, in metres, for a world position."""
    sp = _spline_cum(profile_id)
    if not sp:
        return None
    pts, cum, _total = sp
    best, bi = None, 0
    for i, p in enumerate(pts):
        d = (p[0] - x) ** 2 + (p[1] - z) ** 2
        if best is None or d < best:
            best, bi = d, i
    return cum[bi]


def leaderboard(profile_id=None):
    """Lap times from the server's own log, keyed by carId.

    The server has no leaderboard endpoint - its HTTP port returns only a
    client count - but it logs every completed lap as
    `New lap carId <carId>: MM:SS.mmm`. That carId is the SAME key the
    telemetry binds a car to, so the board and the live map join exactly
    rather than being matched by name.

    ⚠ Only completed laps count. The log also carries
    "Couldn't create lap from opensplits" for out-laps and missed sectors;
    those are not laps and are deliberately ignored.
    """
    if not profile_id:
        profile_id = default_profile()
    prof = next((p for p in servers.load() if p["id"] == profile_id), None)
    log = config.server_log((prof or {}).get("log", "vai_server.log"))
    try:
        txt = open(log, encoding="utf-8", errors="replace").read()
    except OSError:
        return {"ok": False, "error": "no log for that server"}
    cut = [m.start() for m in re.finditer(r"Build release", txt)]
    if cut:
        txt = txt[cut[-1]:]

    laps = {}
    for cid, mm, ss, ms in LAP_RE.findall(txt):
        secs = int(mm) * 60 + int(ss) + int(ms) / (10 ** len(ms))
        laps.setdefault(cid, []).append(round(secs, 3))

    who = {p["carid"]: p for p in _connected_players(log)}
    live = {c.get("id") for c in
            (cars(profile_id, annotate=False).get("cars") or [])
            if c.get("id")}

    rows = []
    for cid, times in laps.items():
        ident = who.get(cid, {})
        rows.append({
            "carid": cid,
            "name": ident.get("id"),
            "display": ident.get("display"),
            "model": ident.get("model"),
            "laps": len(times),
            "best": min(times),
            "last": times[-1],
            "on_track": cid in live,          # joined to the live map by carId
            "connected": cid in who,
        })
    rows.sort(key=lambda r: r["best"])
    for i, r in enumerate(rows, 1):
        r["pos"] = i
    return {"ok": True, "profile_id": profile_id,
            "server": (prof or {}).get("name"), "rows": rows,
            "note": ("lap times come from the server log; only completed laps "
                     "count, so out-laps are absent")}


def _lap_and_gaps(profile_id, rows, best_by_car):
    """Annotate live cars with lap progress, predicted lap time and gaps.

    ⚠ Gaps are distance / speed, the same approximation a real timing screen
    uses. That is honest for cars running comparable pace and meaningless for a
    car that is nearly stopped, so it is suppressed below walking pace rather
    than reported as a huge number.

    ⚠ A predicted lap needs a reference. With a best lap for that car we scale
    it by how the current lap is going; with no best lap yet the only estimate
    available is elapsed / fraction complete, which is poor early in a lap - so
    it is withheld until the lap is properly under way and flagged as rough.
    """
    sp = _spline_cum(profile_id)
    if not sp:
        return rows
    _pts, _cum, total = sp
    now = time.time()

    for c in rows:
        key = c.get("id") or c.get("addr")
        s = _project(profile_id, c["x"], c["z"])
        if s is None:
            continue
        c["lap_m"] = round(s, 1)
        c["lap_pct"] = round(100.0 * s / total, 1) if total else None

        st = _LAPS.setdefault((profile_id, key),
                              {"s": s, "start": now, "synced": False})
        # Crossing the line makes distance collapse from ~total back to ~0.
        # Require a big drop so ordinary jitter near a spline seam cannot be
        # mistaken for a lap.
        if st["s"] - s > total * 0.5:
            st["start"] = now
            st["synced"] = True
        st["s"] = s

        # ⚠ Until we have actually SEEN this car cross the line, the lap clock
        # started whenever we first noticed it - somewhere mid-lap - so elapsed
        # is not lap time and anything derived from it is fiction. First
        # observation gave "predicted 11.2 s" for a 1:12 lap. Say nothing until
        # a real lap boundary has been observed.
        if not st["synced"]:
            c["lap_synced"] = False
            continue
        c["lap_synced"] = True
        elapsed = now - st["start"]
        c["lap_elapsed"] = round(elapsed, 2)

        frac = (s / total) if total else 0
        best = best_by_car.get(c.get("id"))
        if best and frac > 0.02:
            # How this lap compares with the reference so far, applied to the
            # rest of it.
            pace = elapsed / (best * frac) if best * frac > 0 else 1.0
            c["predicted"] = round(best * pace, 3)
            c["delta_best"] = round(c["predicted"] - best, 3)
            c["predicted_rough"] = False
        elif frac > 0.25:
            c["predicted"] = round(elapsed / frac, 3)
            c["predicted_rough"] = True

    # Gaps: order by distance around the lap, leader first.
    order = sorted([c for c in rows if c.get("lap_m") is not None],
                   key=lambda c: c["lap_m"], reverse=True)
    for i, c in enumerate(order):
        c["running_pos"] = i + 1
        speed = (c.get("kmh") or 0) / 3.6
        if i == 0:
            c["gap_ahead"] = 0.0
            c["gap_leader"] = 0.0
            continue
        if speed < 1.5:            # nearly stopped: a time gap means nothing
            c["gap_ahead"] = c["gap_leader"] = None
            continue
        c["gap_ahead"] = round((order[i - 1]["lap_m"] - c["lap_m"]) / speed, 2)
        c["gap_leader"] = round((order[0]["lap_m"] - c["lap_m"]) / speed, 2)
    return rows


def _pitlane_points(ideal_name):
    """The pit lane spline that sits beside a given ideal-line spline."""
    if not ideal_name:
        return []
    try:
        if config.server_dir() not in sys.path:
            sys.path.insert(0, config.server_dir())
        import parse_spline
        for base, _, files in os.walk(os.path.join(config.server_dir(),
                                                   "content", "tracks")):
            if ideal_name not in files:
                continue
            stem = ideal_name.replace(".ideal_line.aisplinedata", "")
            cand = os.path.join(base, stem + ".pitlane.aisplinedata")
            if os.path.isfile(cand):
                return [[p[0], p[2]] for p in parse_spline.points(cand)]
            return []
    except Exception:
        pass
    return []


def track(profile_id=None, _with_y=False):
    """Centreline for the map, from the spline that profile's server loads."""
    try:
        if config.server_dir() not in sys.path:
            sys.path.insert(0, config.server_dir())
        import parse_spline
        profs = servers.load()
        if not profile_id:
            profile_id = default_profile()
        prof = next((p for p in profs if p["id"] == profile_id), None)
        idx = (prof or {}).get("track_index", 18)
        evs = json.load(open(os.path.join(config.server_dir(),
                                          "events_practice.json"),
                             encoding="utf-8"))["events"]
        ev = evs[idx]
        # ⚠ events_practice.json holds DISPLAY names ("Nurburgring",
        # "Touristenfahrten"); files on disk use folder ids ("nurburgring",
        # "layout_nordschleife_touristenfahrten"). Resolve by searching the
        # layouts folder - do not construct the filename.
        folder = ev["track"].lower().replace(" ", "_")
        ldir = os.path.join(config.server_dir(), "content", "tracks", folder,
                            "layouts")
        want = ev["layout"].lower().replace(" ", "_")
        base = None
        if os.path.isdir(ldir):
            cands = [f for f in os.listdir(ldir)
                     if f.endswith(".ideal_line.aisplinedata")]
            match = [f for f in cands if want in f.lower()]
            pick = (match or cands or [None])[0]
            if pick:
                base = os.path.join(ldir, pick)
        if not base:
            return {"ok": False,
                    "error": f"no ideal-line spline for {ev['track']} / "
                             f"{ev['layout']} under {ldir}"}
        pts = parse_spline.points(base)
        if not pts:
            return {"ok": False, "error": "spline parsed to no points"}
        # A custom track borrows a host track's slots, so the event name is the
        # HOST's. Show what is really deployed there when the profile says so.
        label = (prof or {}).get("track_label") or ""
        # The racing line is not the centreline - it hugs the inside of
        # corners - so cars sit several metres off it and the map looks
        # miscalibrated when it is not. Draw the real edges where we can.
        left = right = None
        try:
            import parse_edges
            left, right = parse_edges.edges(base)
        except Exception:
            left = right = None
        out = {"ok": True, "track": label or ev["track"],
               "edges": ({"left": left, "right": right}
                         if left and right else None),
               "layout": ev["layout"],
               "host_track": ev["track"] if label else None,
               "spline": os.path.basename(base),
               "points": [[p[0], p[2]] for p in pts]}
        if _with_y:
            out["points3"] = pts
        return out
    except Exception as ex:
        return {"ok": False, "error": f"{type(ex).__name__}: {ex}"}
