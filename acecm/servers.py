"""Server profiles: save a configuration, start it, watch it, stop it.

A profile is everything the launcher needs (track, car list, AI count, ports,
time of day) stored as JSON, so a server can be recreated exactly rather than
rebuilt from remembered environment variables. Starting one shells out to the
existing start_vai_server.py, which already knows how to encode the base64
serverconfig/seasondefinition blobs the exe expects.
"""
import json
import os
import re
import subprocess
import sys
import time
import uuid
import urllib.request

from . import config, content, install, logs

PROFILES = os.path.join(config.DATA, "profiles.json")
# profile id -> {pid, started}. Written when we launch, so a profile can be
# tied to ITS server process rather than "whichever server is running" - which
# is the whole prerequisite for per-server telemetry.
RUNTIME = os.path.join(config.DATA, "runtime.json")

TEMPLATE = {
    "name": "New server",
    "track_index": 18,            # Nordschleife Touristenfahrten
    "game_mode": "PRACTICE",
    "session_type": "UNRANKED",
    # ⚠ NO virtual-AI cars by default. vAI is a netcode load-testing feature
    # driven by a patched server binary; we do not ship bots, so a new profile
    # must never start any. Raising this also requires the patched exe.
    "ai": 0,
    "skill_min": 70,
    "skill_max": 95,
    "max_players": 90,
    "tcp_port": 9700,
    "http_port": 8080,
    # time + weather
    "tod_hour": 13,
    "tod_minute": 0,
    "time_mult": 0,               # 0 = frozen
    "weather": "CLEAR",
    "weather_behaviour": "STATIC",
    "grip": "OPTIMUM",
    # rules
    "tuning": "TuningAllowed",
    "pi_min": 0.0,
    "pi_max": 0.0,
    "practice_duration": 9999,
    "cycle": False,
    # access
    "driver_password": "",
    "spectator_password": "",
    "admin_password": "",
    # content + extras
    "cars": [],                   # empty = every Kunos car + installed mods
    "entry_list_path": "",
    "results_path": "",
    "entry_list_url": "",
    "results_post_url": "",
    # ports: internal can differ from the listener (0 = same as tcp_port)
    "tcp_internal_port": 0,
    "udp_internal_port": 0,
    # session pacing (the launcher used to hardcode both to 10)
    "overtime_wait": 10,
    "max_wait_to_box": 10,
    # penalties, per server. These ride in the -seasondefinition blob
    # (SimpleGameConfig fields 20-23), so each server carries its own and
    # nothing in content.kspkg has to be patched.
    # ⚠ Accepted by the server without error, but the season definition it
    # builds is byte-identical with them on or off, so enforcement is NOT
    # confirmed - it needs an on-track cut to verify.
    # A custom track is hosted by borrowing an existing track's slots, so the
    # event still reports the HOST's name ("Road Atlanta"). Set this to what is
    # actually in the slot so maps and labels don't lie.
    "track_label": "",
    # Which DEPLOYED track to host, by the name clients know it as. Empty means
    # host the stock event at track_index. Kept separate from track_label so a
    # cosmetic fix can never change what the server actually runs.
    "custom_track": "",
    "penalties": False,
    "car_cut_tyres_out": 4,          # wheels off track before it counts (1-4)
    "warning_trigger_countdown": 3,  # warnings before the penalty lands
    "time_penalty_ms": 5000,         # the penalty itself
    # full in-game date, not just the hour
    "tod_year": 2024,
    "tod_month": 8,
    "tod_day": 15,
    "tod_second": 0,
    # flags
    "no_lobby": False,          # True = private, will NOT appear in the browser
    "write_results": False,
    "export_json": False,
    # per-car handicaps: "car:ballast:restrictor,..."
    "car_handicaps": "",
    "telemetry": True,
    "log": "vai_server.log",
}

# Valid values, read out of the protobuf schemas and the server binary - not
# invented. The UI renders these as dropdowns so a typo cannot reach the server.
OPTIONS = {
    "game_mode": ["PRACTICE", "RACE_WEEKEND", "INSTANT_RACE", "HOTLAP",
                  "HOTSTINT", "DRIFT", "FREEROAM", "RALLY", "SUPERPOLE",
                  "TEST_DRIVE", "A_TO_B", "SRO_RACE"],
    "session_type": ["UNRANKED", "RANKED", "BOTH"],
    "weather": ["CLEAR", "SCATTERED_CLOUDS", "BROKEN_CLOUDS", "OVERCAST",
                "DAMP", "DRIZZLE", "RAIN", "HEAVY_RAIN", "CUSTOM"],
    "weather_behaviour": ["STATIC", "DYNAMIC"],
    "grip": ["OPTIMUM", "FAST", "GREEN"],
    "tuning": ["TuningAllowed", "TuningDenied"],
}


def load():
    try:
        return json.load(open(PROFILES, encoding="utf-8"))
    except Exception:
        return []


def save_all(items):
    json.dump(items, open(PROFILES, "w", encoding="utf-8"), indent=2)
    return items


def upsert(profile):
    items = load()
    # ⚠ Merge over the EXISTING profile, not just the template. Saving a
    # partial update (e.g. only the penalty fields) used to reset every other
    # setting - name, track, ports - back to defaults.
    current = next((p for p in items if p["id"] == profile.get("id")), {})
    profile = {**TEMPLATE, **current, **profile}
    if not profile.get("id"):
        # ⚠ int(time.time()) collides for anything created in the same
        # second - two profiles made together got the SAME id and one
        # silently overwrote the other. Add randomness.
        profile["id"] = f"srv{int(time.time())}{uuid.uuid4().hex[:4]}"
        items.append(profile)
    else:
        items = [profile if p["id"] == profile["id"] else p for p in items]
    save_all(items)
    return profile


def delete(pid):
    save_all([p for p in load() if p["id"] != pid])


def runtime():
    try:
        return json.load(open(RUNTIME, encoding="utf-8"))
    except Exception:
        return {}


def _save_runtime(r):
    json.dump(r, open(RUNTIME, "w", encoding="utf-8"), indent=2)


def _alive(pid):
    from . import winproc
    return winproc.alive(pid)


def bind_pid(pid_map):
    r = runtime()
    r.update(pid_map)
    _save_runtime(r)


def _server_pids():
    """Running dedicated-server processes -> {pid: exe name}."""
    from . import winproc
    res = {}
    for pid in winproc.pids_named("AssettoCorsaEVOServer",
                                  "AssettoCorsaEVOServer.stock"):
        res[pid] = "AssettoCorsaEVOServer"
    return res


def _custom_event(profile):
    """The event JSON for a deployed track, or "" to use the stock index.

    ⚠ Driven by `custom_track`, NOT by `track_label`. They read alike and mean
    different things: track_label is cosmetic - what is really sitting in a
    borrowed slot, so maps and labels do not lie - and it is consumed by the
    lobby ad, the dashboard and telemetry. Letting it also SELECT the track
    would change what an existing profile hosts the moment someone corrected a
    label, with no warning. One field per meaning.

    The layout is read from the installed track's own `layout_<name>.scene`
    container, because the server matches layout names CASE-SENSITIVELY - a
    guess like "Layout" resolves no containers at all and the track loads bare.
    """
    name = (profile.get("custom_track") or "").strip()
    if not name:
        return ""
    ev = {"track": name}
    try:
        from . import contentsync, tracks
        folder = (contentsync.track_map() or {}).get(name, "")
        if folder:
            src = os.path.join(contentsync.tracks_dir(), folder)
            ev["layout"] = tracks.read_track_folder(src)["layout"]
    except Exception as ex:
        logs.LOG.warning("no layout for custom track %r: %s", name, ex)
    if not ev.get("layout"):
        # ⚠ Refuse rather than guess. A wrong layout starts a server that
        # resolves no containers - it looks up, and the track is unusable.
        logs.LOG.error("custom track %r has no resolvable layout - "
                       "hosting the stock event instead", name)
        return ""
    ev["event_name"] = f"{ev['layout']} Race"
    return json.dumps(ev)


def _port_shared(profile):
    """Does another profile use this one's HTTP port?

    If so, that port cannot identify anything: the two servers are only
    distinguishable by the pid we recorded at launch.
    """
    mine = profile.get("http_port", 8080)
    return any(o.get("id") != profile.get("id")
               and o.get("http_port", 8080) == mine
               for o in load())


def port_clashes():
    """Profiles that share a game or HTTP port, as {port: [names]}."""
    out = {}
    for key in ("tcp_port", "http_port"):
        seen = {}
        for p in load():
            seen.setdefault(p.get(key), []).append(p.get("name") or "?")
        for port, names in seen.items():
            if port and len(names) > 1:
                out.setdefault(port, [])
                for n in names:
                    if n not in out[port]:
                        out[port].append(n)
    return out


def status(profile):
    """Live state for one profile, tied to ITS process where possible."""
    st = {"running": False, "pid": None, "clients": None, "log_age": None}
    rec = runtime().get(profile.get("id") or "", {})
    pid = rec.get("pid")
    if pid and _alive(pid):
        st["running"], st["pid"] = True, pid
    elif not _port_shared(profile):
        # Fall back to the HTTP port - but ONLY when this profile alone uses
        # it. ⚠ The comment here used to claim the port "IS unique per
        # profile"; nothing enforces that, and two profiles sharing 8080 both
        # reported running whenever either was up. Worse, per-server Stop
        # resolves by the same port, so it could kill the other one's process.
        try:
            url = f"http://127.0.0.1:{profile.get('http_port', 8080)}/"
            with urllib.request.urlopen(url, timeout=1.0):
                st["running"] = True
        except Exception:
            pass
    else:
        st["ambiguous"] = True
    if st["running"]:
        try:
            url = f"http://127.0.0.1:{profile.get('http_port', 8080)}/"
            with urllib.request.urlopen(url, timeout=1.5) as r:
                st["clients"] = json.loads(r.read()).get("clients")
        except Exception:
            st["clients"] = None
    log = config.server_log(profile.get("log", "vai_server.log"))
    if os.path.exists(log):
        st["log_age"] = round(time.time() - os.path.getmtime(log), 1)
        st["log"] = log
    return st


# A healthy dedicated server sits around 1-2 GB. Anything past this is not a
# busy server, it is a runaway - and a runaway here can take the whole machine
# down, so kill it rather than let it keep growing.
MEM_LIMIT_GB = float(os.environ.get("ACECM_SERVER_MEM_LIMIT_GB", "12"))


def _watch_memory(pid, limit_gb=None):
    """Kill a dedicated server that starts consuming absurd memory.

    ⚠ This exists because a server started on an occupied port span retrying
    its sockets and reserved ~120 GB of virtual memory, exhausting the pagefile
    and hanging the machine. The port check should prevent that specific cause;
    this is the backstop for every cause we have not thought of.
    """
    from . import winproc
    limit = (limit_gb or MEM_LIMIT_GB) * (1 << 30)
    while True:
        time.sleep(5.0)
        ws = winproc.working_set(pid)
        if ws is None:
            return
        if ws > limit:
            winproc.kill(pid)
            logs.LOG.critical(
                "KILLED runaway server pid %s: %.1f GB exceeded the %.0f GB "
                "limit. Usual cause: started on a port already in use, which "
                "makes it spin retrying its sockets and leak.",
                pid, int(out)/(1<<30), limit/(1<<30))
            return


# Ports claimed by a launch that has not finished starting yet: port -> when.
# A dedicated server takes ~30 s to bind, and until it does, nothing on the
# machine shows it as in use.
_LAUNCHING = {}
STARTUP_GRACE = float(os.environ.get("ACECM_STARTUP_GRACE", "45"))


def port_busy(port):
    """Is anything already listening here?"""
    import socket
    with socket.socket() as s:
        s.settimeout(0.4)
        return s.connect_ex(("127.0.0.1", int(port))) == 0


def server_track_names():
    """Track names the SERVER's own archive can resolve, from its tracks.table."""
    import re
    try:
        from . import config, kspkg_write
        pkg = os.path.join(config.server_dir(), "content.kspkg")
        # ⚠ raw string: "system\tracks.table" is a TAB followed by "racks",
        # which matches no entry and silently reports the server as having no
        # tracks at all - which then let every start through unchecked.
        blob = kspkg_write.read_entry(pkg, r"system\tracks.table")
        if not blob:
            return []
        from .tracktables import walk
        top = list(walk(blob))
        if not top:
            return []
        out = []
        for _f, _w, row in walk(top[0][2]):
            for g, gw, gv in walk(row):
                if g != 2 or gw != 2:
                    continue
                for h, hw, hv in walk(gv):
                    if h == 1 and hw == 2:
                        out.append(hv.decode("utf-8", "replace"))
        return out
    except Exception as ex:
        logs.LOG.info("could not read the server's track names: %s", ex)
        return []


def _custom_track_problem(profile):
    """Why this profile's custom track would fail, or "" if it is fine."""
    name = (profile.get("custom_track") or "").strip()
    if not name:
        return ""
    known = server_track_names()
    if not known:
        return ""                      # cannot check - do not block the start
    if name in known:
        return ""
    close = [k for k in known if name.split()[0].lower() in k.lower()]
    return (f"the server does not have a track called {name!r} - it would "
            f"start and then die loading an empty path. Deploy it from "
            f"Content first"
            + (f" (its archive calls it {close[0]!r})" if close else ""))


def start(profile):
    """Launch the dedicated server for a profile.

    ⚠ REFUSE to start on ports that are already in use. Starting a second
    server on an occupied port does not fail cleanly: the server spins
    retrying its socket setup, logging
    "TCP socket SEND/RECV buffer COULD NOT set" over a million times, and
    leaks until it has reserved ~120 GB of virtual memory. On 2026-08-11 that
    exhausted the pagefile and hung the whole machine (288 MB of log in eight
    minutes). One cheap check is worth more than any amount of cleanup.
    """
    # ⚠ A custom track the SERVER cannot resolve is a crash, not an error. The
    # name is looked up in the server's own tracks.table; a miss yields empty
    # paths and the exe dies with "Trying to load a message with an empty
    # path", which names nothing useful. Check first and say what is wrong.
    bad = _custom_track_problem(profile)
    if bad:
        return {"ok": False, "error": bad}

    tcp = profile.get("tcp_port", 9700)
    http = profile.get("http_port", 8080)

    # ⚠ A LISTENING check alone is not enough. The server needs ~30 s to bind,
    # so a second Start click a few seconds later sails straight through and
    # you get two servers on one port - exactly the runaway above. Seen in the
    # wild on a fresh install: two launches on 9700/8080 seven seconds apart,
    # because nothing looked like it was happening yet.
    now = time.time()
    for port in (tcp, http):
        started = _LAUNCHING.get(port, 0)
        if now - started < STARTUP_GRACE:
            msg = (f"a server is already starting on port {port} "
                   f"({int(now - started)}s ago) - give it up to "
                   f"{STARTUP_GRACE:.0f}s to come up")
            logs.LOG.error("refusing to start %r: %s", profile.get("name"), msg)
            return {"ok": False, "error": msg}

    # A different profile whose process is alive and which owns these ports.
    rt = runtime()
    live = _server_pids()
    for other in load():
        if other.get("id") == profile.get("id"):
            continue
        pid = (rt.get(other.get("id") or "") or {}).get("pid")
        if pid and pid in live and (other.get("tcp_port") == tcp
                                    or other.get("http_port") == http):
            msg = (f"{other.get('name')!r} is already running on those ports "
                   f"(pid {pid}) - stop it first")
            logs.LOG.error("refusing to start %r: %s", profile.get("name"), msg)
            return {"ok": False, "error": msg}

    for label, port in (("game", tcp), ("HTTP", http)):
        if port_busy(port):
            msg = (f"{label} port {port} is already in use - stop whatever "
                   f"is on it first (a server left running on these ports "
                   f"will spin and eat all memory)")
            logs.LOG.error("refusing to start %r: %s",
                           profile.get("name"), msg)
            return {"ok": False, "error": msg}

    resolved_exe = config.server_exe()
    if not resolved_exe:
        return {"ok": False,
                "error": "dedicated server executable not found - set "
                         "server_dir in Settings to the folder that holds "
                         "AssettoCorsaEVOServer.exe"}

    launcher = config.tool_script("start_vai_server.py")
    if not os.path.exists(launcher):
        return {"ok": False, "error": f"launcher not found: {launcher}"}

    # Claim the ports BEFORE launching, so a second click within the startup
    # window is refused even though nothing is listening yet.
    _LAUNCHING[tcp] = _LAUNCHING[http] = now
    env = dict(os.environ)
    # ⚠ Tell the tool WHERE the server is. A frozen build unpacks these
    # scripts into a temp folder, so a tool that resolves the server dir
    # from its own __file__ finds nothing there - no exe, no events
    # json, nowhere to write a log - and the launch fails silently.
    env["SERVER_DIR"] = config.server_dir()
    env.update({
        # Full path. An empty CFG['server_exe'] used to be forwarded as
        # SERVER_EXE='', which join(dir, '') treats as the directory, and
        # Windows then raises Access Denied.
        "SERVER_EXE": resolved_exe,
        "LOG_FILE": profile.get("log", "vai_server.log"),
        "SERVER_NAME": profile.get("name", "ACECM server"),
        "N_AI": str(profile.get("ai", 0)),
        "MAX_PLAYERS": str(profile.get("max_players", 90)),
        "EVENT_IDX": str(profile.get("track_index", 0)),
        # ⚠ A deployed custom track has to be NAMED, not indexed - events_*.json
        # lists stock events only. Without this, picking a deployed track set a
        # label and nothing else, and the server hosted whichever stock event
        # the index still held: a profile called "Highland drift" ran
        # Nurburgring, and the name made it look like it had worked.
        "CUSTOM_EVENT": _custom_event(profile),
        "PORT": str(profile.get("tcp_port", 9700)),
        "HTTP_PORT": str(profile.get("http_port", 8080)),
        "TOD_HOUR": str(profile.get("tod_hour", 13)),
        "TOD_MINUTE": str(profile.get("tod_minute", 0)),
        "TIME_MULT": str(profile.get("time_mult", 0)),
        "GAME_MODE": profile.get("game_mode", "PRACTICE"),
        "SESSION_TYPE": profile.get("session_type", "UNRANKED"),
        "WEATHER": profile.get("weather", "CLEAR"),
        "WEATHER_BEHAVIOUR": profile.get("weather_behaviour", "STATIC"),
        "GRIP": profile.get("grip", "OPTIMUM"),
        "TUNING": profile.get("tuning", "TuningAllowed"),
        "PI_MIN": str(profile.get("pi_min", 0)),
        "PI_MAX": str(profile.get("pi_max", 0)),
        "PRACTICE_DURATION": str(profile.get("practice_duration", 9999)),
        "CYCLE": "1" if profile.get("cycle") else "0",
        "SKILL_MIN": str(profile.get("skill_min", 70)),
        "SKILL_MAX": str(profile.get("skill_max", 95)),
        "DRIVER_PASSWORD": profile.get("driver_password", "") or "",
        "SPECTATOR_PASSWORD": profile.get("spectator_password", "") or "",
        "ADMIN_PASSWORD": profile.get("admin_password", "") or "",
        "ENTRY_LIST_PATH": profile.get("entry_list_path", "") or "",
        "RESULTS_PATH": profile.get("results_path", "") or "",
        "ENTRY_LIST_URL": profile.get("entry_list_url", "") or "",
        "RESULTS_POST_URL": profile.get("results_post_url", "") or "",
        "PORT_TCP_INTERNAL": str(profile.get("tcp_internal_port", 0) or 0),
        "PORT_UDP_INTERNAL": str(profile.get("udp_internal_port", 0) or 0),
        "OVERTIME_WAIT": str(profile.get("overtime_wait", 10)),
        "MAX_WAIT_TO_BOX": str(profile.get("max_wait_to_box", 10)),
        "PENALTIES": "1" if profile.get("penalties") else "0",
        "CAR_CUT_TYRES_OUT": str(profile.get("car_cut_tyres_out", 4)),
        "WARNING_TRIGGER_COUNTDOWN":
            str(profile.get("warning_trigger_countdown", 3)),
        "TIME_PENALTY_MS": str(profile.get("time_penalty_ms", 5000)),
        "TOD_YEAR": str(profile.get("tod_year", 2024)),
        "TOD_MONTH": str(profile.get("tod_month", 8)),
        "TOD_DAY": str(profile.get("tod_day", 15)),
        "TOD_SECOND": str(profile.get("tod_second", 0)),
        "NO_LOBBY": "1" if profile.get("no_lobby") else "0",
        "WRITE_RESULTS": "1" if profile.get("write_results") else "0",
        "EXPORT_JSON": "1" if profile.get("export_json") else "0",
        "CAR_HANDICAPS": profile.get("car_handicaps", "") or "",
    })
    if profile.get("cars"):
        env["CARS_OVERRIDE"] = ",".join(profile["cars"])
    else:
        # The launcher defaults to the Kunos cars in cars.json, which does NOT
        # contain cars shipped by installed mods - so without this an installed
        # mod is never selectable. Pass the union explicitly.
        try:
            mod_ids = list(install.car_names())
            if mod_ids:
                kunos = [c["id"] for c in content.cars()["cars"] if c["kunos"]]
                env["CARS_OVERRIDE"] = ",".join(kunos + mod_ids)
        except Exception as ex:
            print(f"car list merge skipped: {ex}")
    if profile.get("telemetry"):
        env["TELEMETRY"] = "1"
    before = set(_server_pids())
    log_path = os.path.join(config.DATA, "last_start.log")
    out = open(log_path, "w", encoding="utf-8", errors="replace")
    cmd = config.tool_cmd("start_vai_server", [])
    proc = subprocess.Popen(cmd, env=env, cwd=config.server_dir(),
                            stdout=out, stderr=subprocess.STDOUT)
    logs.launched(f"server {profile.get('name')!r}", cmd, proc.pid,
                  tcp=profile.get("tcp_port"), http=profile.get("http_port"),
                  track=profile.get("track_index"), ai=profile.get("ai"),
                  log=log_path, exe=resolved_exe)
    # The launcher starts the exe and exits. A crash is also an exit, so
    # wait just long enough to see a non-zero code. Success used to look
    # identical to "Access Denied" in the UI, then the 45s grace lock
    # refused every retry.
    for _ in range(16):
        rc = proc.poll()
        if rc is not None:
            break
        time.sleep(0.25)
    rc = proc.poll()
    if rc not in (None, 0):
        _LAUNCHING.pop(tcp, None)
        _LAUNCHING.pop(http, None)
        try:
            out.flush()
            detail = open(log_path, encoding="utf-8",
                          errors="replace").read().strip()
        except OSError:
            detail = ""
        logs.LOG.error("server launcher exited %s: %s", rc, detail[-400:])
        return {"ok": False,
                "error": "the server launcher died before the exe started "
                         f"(exit {rc})",
                "detail": detail[-800:]}
    try:
        from . import lobby
        lobby.write(profile)
    except Exception as ex:
        logs.LOG.warning("lobby advertisement not written: %s", ex)

    def _capture():
        # The launcher spawns the exe, so its own pid is not the server's.
        # Watch for a NEW dedicated-server process and record it.
        import threading, time as _t
        for _ in range(30):
            _t.sleep(1.0)
            new = set(_server_pids()) - before
            if new:
                pid = sorted(new)[0]
                bind_pid({profile["id"]: {"pid": pid,
                                          "started": int(_t.time()),
                                          "http_port": profile.get("http_port")}})
                threading.Thread(target=_watch_memory, args=(pid,),
                                 daemon=True).start()
                # It exists now, so the normal checks can take over.
                _LAUNCHING.pop(profile.get("tcp_port", 9700), None)
                _LAUNCHING.pop(profile.get("http_port", 8080), None)
                return
    import threading
    threading.Thread(target=_capture, daemon=True).start()
    return {"ok": True}


def stop(profile_id=None):
    """Stop ONE profile's server, or every server when no id is given.

    ⚠ Identify the process before killing anything. This used to kill every
    dedicated server unconditionally, so a per-server Stop button took down the
    other servers too - and with several running that is somebody else's
    session ending for no visible reason.
    """
    from . import winproc
    if not profile_id:
        pids = list(_server_pids())
        for pid in pids:
            winproc.kill(pid)
        return {"ok": True, "stopped": pids, "scope": "all"}

    prof = next((p for p in load() if p.get("id") == profile_id), None)
    if not prof:
        return {"ok": False, "error": "no such profile"}

    # the pid recorded when WE launched it is the most reliable link
    rec = runtime().get(profile_id) or {}
    pid = rec.get("pid")
    if not (pid and _alive(pid)):
        # otherwise fall back to whoever holds this profile's HTTP port, which
        # is unique per profile - never "whichever server is running"
        pid = _pid_on_port(prof.get("http_port", 8080))
    if not pid:
        return {"ok": False,
                "error": "could not tell which process is this server - it may "
                         "have been started outside ACECM. Use Stop all, or "
                         "close it from Task Manager"}
    winproc.kill(pid)
    r = runtime()
    r.pop(profile_id, None)
    _save_runtime(r)
    return {"ok": True, "stopped": [pid], "scope": prof.get("name")}


def _pid_on_port(port):
    """Which process is listening on a local port, or None."""
    try:
        ps = (f"(Get-NetTCPConnection -State Listen -LocalPort {int(port)} "
              f"-ErrorAction SilentlyContinue).OwningProcess")
        r = subprocess.run(["powershell", "-NoProfile", "-NonInteractive",
                            "-Command", ps], capture_output=True, text=True,
                           timeout=8)
        for line in (r.stdout or "").split():
            if line.strip().isdigit():
                pid = int(line.strip())
                if pid in _server_pids():
                    return pid
    except Exception as ex:
        logs.LOG.info("port lookup for %s: %s", port, ex)
    return None


def log_tail(profile, lines=120):
    path = config.server_log(profile.get("log", "vai_server.log"))
    if not os.path.exists(path):
        return {"lines": [], "error": "no log yet"}
    try:
        with open(path, "rb") as fh:
            fh.seek(0, os.SEEK_END)
            size = fh.tell()
            fh.seek(max(0, size - 200_000))
            text = fh.read().decode("utf-8", "replace")
    except OSError as ex:
        return {"lines": [], "error": str(ex)}
    keep = [l for l in text.splitlines()
            # the vAI message spam drowns everything useful
            if "ServerWorldTime" not in l]
    return {"lines": keep[-lines:]}
