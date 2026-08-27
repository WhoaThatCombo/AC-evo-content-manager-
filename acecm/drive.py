"""Quick Drive: real single-player, like AC Content Manager's home.

EVO will not take -car/-track and spawn. The client already has the
pieces: GameModeSave under Saved Games\\ACE\\GameModes, plus the
startup_gamemode / load_single_car flags. We write the save (track,
time, mode) and launch into that session. No dedicated server.
"""
import json
import os
import re
import shutil
import threading
import time

from . import backend, config, content, contentsync, gameui, logs, servers
from . import settings as gamesettings

DRIVE_ID = "acecm-drive"
STATE = os.path.join(config.DATA, "drive.json")

SP_MODES = [
    "PRACTICE", "INSTANT_RACE", "HOTLAP", "HOTSTINT",
    "DRIFT", "FREEROAM", "RACE_WEEKEND", "TEST_DRIVE",
]

AI_MODES = ("INSTANT_RACE", "RACE_WEEKEND")
AGGRO = ("Safe", "Normal", "Competitive")

_DEFAULT = {
    "via": "sp",
    "server_id": "",
    "local_id": "",
    "server_ip": "",
    "server_tcp_port": 0,
    "password": "",
    "car": "",
    "track_index": 18,
    "custom_track": "",
    "game_mode": "PRACTICE",
    "weather": "CLEAR",
    "tod_hour": 13,
    "tod_minute": 0,
    "num_opponents": 10,
    "skill_min": 80,
    "skill_max": 95,
    "aggressiveness": "Safe",
    "single_make": True,
    "duration_min": 90,
    "practice_min": 10,
    "quali_min": 15,
    "warmup_min": 10,
    "race_laps": 10,
    "starting_position": 0,
}

_JOB = {
    "phase": "idle",
    "hint": "",
    "fault": "",
    "started": 0,
    "join": None,
    "launch": None,
    "wrote": None,
}


def _load_pick():
    try:
        got = json.load(open(STATE, encoding="utf-8"))
    except Exception:
        got = {}
    return {**_DEFAULT, **{k: got.get(k, _DEFAULT[k]) for k in _DEFAULT}}


def _save_pick(pick):
    os.makedirs(config.DATA, exist_ok=True)
    blob = {**_load_pick(), **pick}
    json.dump(blob, open(STATE, "w", encoding="utf-8"), indent=2)
    return blob


def _set(**kw):
    _JOB.update(kw)
    return dict(_JOB)


def _mode_name(pick):
    raw = (pick.get("game_mode") or "PRACTICE").strip().upper()
    raw = raw.replace("GAMEMODETYPE_", "")
    if raw not in SP_MODES:
        raw = "PRACTICE"
    return "GameModeType_" + raw


def _mode_short(pick):
    return _mode_name(pick).replace("GameModeType_", "")


def _pint(pick, key, lo, hi, fallback):
    try:
        n = int(pick.get(key) if pick.get(key) is not None else fallback)
    except (TypeError, ValueError):
        n = fallback
    return max(lo, min(hi, n))


def _aggro(pick):
    raw = str(pick.get("aggressiveness") or "Safe").strip()
    for name in AGGRO:
        if raw.lower() == name.lower():
            return name
    return "Safe"


def _apply_mode_fields(obj, pick, mode):
    """num_opponents / skill live on session 0 (sessionZero). Race
    Weekend keeps practice / quali / warmup / race as sessions 1-3."""
    short = mode.replace("GameModeType_", "")
    if short == "RACE_WEEKEND":
        want = [
            ("Race Weekend", "TIME",
             _pint(pick, "practice_min", 1, 240, 10) * 60),
            ("Qualifying", "TIME",
             _pint(pick, "quali_min", 1, 120, 15) * 60),
            ("Warmup", "TIME",
             _pint(pick, "warmup_min", 0, 60, 10) * 60),
            ("Race", "LAPS",
             _pint(pick, "race_laps", 1, 200, 10)),
        ]
        while len(obj.sessions) < 4:
            obj.sessions.add()
        for i, (name, dur_t, dur) in enumerate(want):
            s = obj.sessions[i]
            if not s.name:
                s.name = name
            s.duration = dur
            try:
                _set_enum(s, "duration_type",
                          "GameModeSelectionDuration_" + dur_t)
            except ValueError:
                pass
    elif not obj.sessions:
        obj.sessions.add()

    sess = obj.sessions[0]
    if short in ("PRACTICE", "HOTLAP", "HOTSTINT", "TEST_DRIVE"):
        sess.duration = _pint(pick, "duration_min", 1, 600, 90) * 60
        try:
            _set_enum(sess, "duration_type",
                      "GameModeSelectionDuration_TIME")
        except ValueError:
            pass
    elif short == "INSTANT_RACE":
        sess.duration = _pint(pick, "race_laps", 1, 200, 10)
        try:
            _set_enum(sess, "duration_type",
                      "GameModeSelectionDuration_LAPS")
        except ValueError:
            pass
        pos = _pint(pick, "starting_position", 0, 40, 0)
        if pos:
            sess.starting_position = pos

    if short in AI_MODES:
        sess.num_opponents = _pint(pick, "num_opponents", 1, 40, 10)
        sess.min_strength = _pint(pick, "skill_min", 0, 100, 80)
        sess.max_strength = _pint(pick, "skill_max", 0, 100, 95)
        if sess.max_strength < sess.min_strength:
            sess.min_strength, sess.max_strength = (
                sess.max_strength, sess.min_strength)
        sess.single_make = bool(pick.get("single_make", True))
        try:
            _set_enum(sess, "grid_type", "GameModeSelectionGridType_AUTO")
        except ValueError:
            pass
        try:
            _set_enum(sess, "aggressivness", _aggro(pick))
        except ValueError:
            pass


def _event(pick):
    tracks = []
    try:
        tracks = content.tracks().get("tracks") or []
    except Exception:
        pass
    name = (pick.get("custom_track") or "").strip()
    if name:
        hit = next((t for t in tracks
                    if (t.get("custom_track") or t.get("track") or "") == name),
                   None)
        if hit:
            return hit
    idx = int(pick.get("track_index") or 0)
    return next((t for t in tracks if t.get("index") == idx), None)


def _car_model(preset_id):
    """Folder the client load_single_car flag wants (ks_bmw_m4_gt3)."""
    if not preset_id:
        return ""
    cars = []
    try:
        cars = content.cars().get("cars") or []
    except Exception:
        pass
    hit = next((c for c in cars if c.get("id") == preset_id), None)
    if hit and hit.get("model"):
        return hit["model"]
    try:
        from . import carsmap
        return carsmap.model_for(preset_id) or preset_id
    except Exception:
        return preset_id


def _backup(path):
    if not os.path.isfile(path):
        return
    bak = path + ".bak_acecm"
    if not os.path.isfile(bak):
        shutil.copy2(path, bak)
    hist = path + "." + time.strftime("%Y%m%d-%H%M%S") + ".bak"
    shutil.copy2(path, hist)


def _set_enum(msg, field, name):
    desc = msg.DESCRIPTOR.fields_by_name[field]
    val = desc.enum_type.values_by_name.get(name)
    if val is None:
        raise ValueError(f"unknown {field} value {name}")
    setattr(msg, field, val.number)


def _write_gamemode(pick):
    """Stamp GameModeSave + lastgamemode so startup_gamemode has something to load."""
    from . import protos as ap

    if backend._game_running():
        return {"ok": False,
                "error": "close the game first — it reads these saves at "
                         "boot and overwrites them on exit"}
    ev = _event(pick)
    if not ev:
        return {"ok": False, "error": "that track index is not in the list"}
    mode = _mode_name(pick)
    root = gamesettings.settings_dir()
    gmdir = os.path.join(root, "GameModes")
    os.makedirs(gmdir, exist_ok=True)
    save_path = os.path.join(gmdir, mode + ".gamemodesave")
    last_path = os.path.join(gmdir, "gamemode.lastgamemode")

    if not ap.has("GameModeSave"):
        return {"ok": False, "error": "GameModeSave schema is missing"}

    obj = ap.new("GameModeSave")
    if os.path.isfile(save_path):
        obj.ParseFromString(open(save_path, "rb").read())
    _set_enum(obj, "type", mode)
    _apply_mode_fields(obj, pick, mode)
    sess = obj.sessions[0]
    if not sess.name:
        sess.name = (pick.get("game_mode") or "PRACTICE").replace("_", " ").title()
    hour = _pint(pick, "tod_hour", 0, 23, 13)
    minute = _pint(pick, "tod_minute", 0, 59, 0)
    for s in obj.sessions:
        tod = s.time_of_day
        if not tod.year:
            tod.year, tod.month, tod.day = 2014, 8, 15
        tod.hour = hour
        tod.minute = minute
        if not tod.time_multiplier:
            tod.time_multiplier = 1.0
        ti = s.track_item
        ti.name = ev.get("track") or ti.name
        ti.layout = ev.get("layout") or ti.layout
        ti.event_name = ev.get("name") or ev.get("event_name") or ti.event_name
        if ev.get("length_m"):
            ti.track_length = int(ev["length_m"])
        ti.nation = ""
        ti.continent = ""
        ti.is_enabled = True

    model = _car_model(pick.get("car"))
    if model and _mode_short(pick) not in AI_MODES:
        del sess.models[:]
        m = sess.models.add()
        m.name = model
        m.display_name = (next(
            (c.get("label") for c in (content.cars().get("cars") or [])
             if c.get("id") == pick.get("car")), "") or model)
        m.is_enabled = True

    _backup(save_path)
    open(save_path, "wb").write(obj.SerializeToString())

    wname = "GameModeSelectionWeatherType_" + (
        pick.get("weather") or "CLEAR").strip().upper()
    if ap.has("LastGameMode") and os.path.isfile(last_path):
        last = ap.new("LastGameMode")
        last.ParseFromString(open(last_path, "rb").read())
        _set_enum(last, "type", mode)
        # weather_type is the selected preset. weather_data[] is the
        # library — do not stamp RAIN onto slot 0 (that used to be CLEAR).
        try:
            _set_enum(last, "weather_type", wname)
        except ValueError:
            logs.LOG.warning("drive unknown weather %s", wname)
        if last.weather_data:
            try:
                first = last.weather_data[0].DESCRIPTOR.fields_by_name["type"]
                clear = first.enum_type.values_by_name.get(
                    "GameModeSelectionWeatherType_CLEAR")
                if clear and last.weather_data[0].type != clear.number:
                    last.weather_data[0].type = clear.number
            except Exception:
                pass
        _backup(last_path)
        open(last_path, "wb").write(last.SerializeToString())

    ti = sess.track_item
    logs.LOG.info(
        "drive wrote %s track=%s/%s car=%s weather=%s tod=%02d:%02d "
        "ai=%s skill=%s-%s",
        mode, ti.name, ti.layout, model, wname, hour, minute,
        sess.num_opponents if _mode_short(pick) in AI_MODES else 0,
        sess.min_strength, sess.max_strength)
    return {"ok": True, "mode": mode, "save": save_path,
            "track": ti.name, "layout": ti.layout, "car_model": model,
            "weather": wname, "tod_hour": hour, "tod_minute": minute,
            "num_opponents": sess.num_opponents}


def _ensure_backend():
    """The client is rewritten to localhost:448. Without the proxy the
    garage cannot list/set cars (BackendDisconnected)."""
    st = backend.state()
    if st.get("listening"):
        return {"ok": True, "already": True}
    return backend.start("proxy")


def _stop_leftover_server():
    try:
        prof = next((p for p in servers.load() if p.get("id") == DRIVE_ID), None)
        if prof and servers.status(prof).get("running"):
            servers.stop(DRIVE_ID)
            logs.LOG.info("drive stopped leftover Quick Drive dedicated server")
    except Exception as ex:
        logs.LOG.warning("drive could not stop leftover server: %s", ex)


def _car_ids_of(sv):
    raw = []
    for x in list(sv.get("allowed_cars_list") or []) + list(
            sv.get("allowed_cars_list_full") or sv.get("cars") or []):
        if isinstance(x, dict):
            raw.append(x.get("car_name") or x.get("name") or x.get("id") or "")
        else:
            raw.append(str(x or ""))
    return [c for c in raw if c]


def _car_allowed(sv, car_id):
    allow = _car_ids_of(sv)
    if not allow:
        return True
    if car_id in allow:
        return True
    model = _car_model(car_id)
    blob = (car_id or "") + " " + (model or "")
    return any(a and a in blob or blob.find(a) >= 0 for a in allow)


def _local_briefs():
    """ACECM profiles as Drive can start / join them."""
    from . import lobby
    out = []
    for p in servers.load():
        st = servers.status(p)
        try:
            ev = lobby._event(p)
        except Exception:
            ev = {}
        cars = []
        try:
            cars = servers.allowed_car_ids(p)
        except Exception:
            cars = []
        out.append({
            "id": p.get("id"),
            "name": p.get("name") or "(unnamed)",
            "running": bool(st.get("running")),
            "clients": st.get("clients"),
            "track": ev.get("track") or p.get("custom_track") or "",
            "layout": ev.get("layout") or "",
            "custom_track": (p.get("custom_track") or "").strip(),
            "tcp_port": int(p.get("tcp_port") or 9700),
            "udp_port": int(p.get("tcp_port") or 9700),
            "http_port": int(p.get("http_port") or 8080),
            "max_players": int(p.get("max_players") or 90),
            "cars": cars,
            "no_lobby": bool(p.get("no_lobby")),
            "listed": not bool(p.get("no_lobby")),
            "locked": bool(p.get("driver_password")),
            "game_mode": p.get("game_mode") or "PRACTICE",
        })
    return out


def _find_local(pick):
    lid = (pick.get("local_id") or pick.get("server_id") or "").strip()
    if not lid:
        return None
    return next((p for p in servers.load() if p.get("id") == lid), None)


def publish_local(profile_id):
    """Make this profile list in the in-game browser with truthful ads.

    Starts the lobby proxy, writes lobby.json from the profile (custom
    track + cars + name), and turns off Private if it was on. Does not
    start the dedicated server — Join does that.
    """
    from . import lobby
    prof = next((p for p in servers.load() if p.get("id") == profile_id), None)
    if not prof:
        return {"ok": False, "error": "no such server profile"}
    changed = False
    if prof.get("no_lobby"):
        prof["no_lobby"] = False
        servers.upsert(prof)
        changed = True
    ad = lobby.write(prof)
    be = _ensure_backend()
    st = backend.state()
    return {
        "ok": bool(st.get("listening") or (be and be.get("ok"))),
        "listed": True,
        "cleared_private": changed,
        "ad": {
            "name": ad.get("server_name"),
            "track": ad.get("track"),
            "layout": ad.get("layout"),
            "tcp_port": ad.get("tcp_port"),
            "cars": ad.get("cars") or [],
            "lan_ip": ad.get("lan_ip"),
        },
        "backend": bool(st.get("listening")),
        "hint": ("Open Multiplayer in-game — the row is tagged [ACECM]. "
                 "Friends on the LAN see the same name at your LAN IP."),
    }


def _port_open(port, host="127.0.0.1"):
    import socket
    try:
        port = int(port)
    except (TypeError, ValueError):
        return False
    s = socket.socket()
    s.settimeout(0.4)
    try:
        return s.connect_ex((host, port)) == 0
    except OSError:
        return False
    finally:
        s.close()


def _wait_server(prof, seconds=50):
    """HTTP answering is not enough — join uses TCP 9700."""
    tcp = int(prof.get("tcp_port") or 9700)
    until = time.time() + seconds
    last = {}
    while time.time() < until:
        last = servers.status(prof)
        last["tcp_open"] = _port_open(tcp)
        if last.get("tcp_open"):
            return last
        time.sleep(0.8)
    last["tcp_open"] = _port_open(tcp)
    return last


def _run_local(pick):
    from . import lobby
    prof = _find_local(pick)
    if not prof:
        _set(phase="failed", fault="pick one of your ACECM servers first")
        return
    if not _car_allowed(
            {"cars": servers.allowed_car_ids(prof)}, pick.get("car")):
        _set(phase="failed",
             fault="that car is not on this server's allowed list")
        return
    if prof.get("no_lobby"):
        prof["no_lobby"] = False
        servers.upsert(prof)
    _set(phase="starting_backend", hint="starting the lobby so it lists")
    _ensure_backend()
    lobby.write(prof)
    st = servers.status(prof)
    if not st.get("running"):
        _set(phase="starting_server",
             hint="starting " + (prof.get("name") or "the server"))
        started = servers.start(prof)
        if not started.get("ok"):
            _set(phase="failed",
                 fault=started.get("error") or "could not start the server")
            return
        _set(phase="starting_server", hint="waiting for the dedicated server")
        st = _wait_server(prof)
        if not st.get("tcp_open"):
            _set(phase="failed",
                 fault="the dedicated server process started but never "
                       "opened TCP " + str(prof.get("tcp_port") or 9700)
                       + " — Join would fail with 'socket did not respond'. "
                       "Stop the server and start it again from Servers; "
                       "check Logs if it dies on season/track load")
            return
    else:
        st = _wait_server(prof, seconds=12)
        if not st.get("tcp_open"):
            _set(phase="failed",
                 fault="ACECM thinks that server is running, but nothing "
                       "is listening on 127.0.0.1:"
                       + str(prof.get("tcp_port") or 9700)
                       + " — Stop it and Start & Join again")
            return
    lobby.write(prof)
    try:
        lobby.refresh()
    except Exception:
        pass
    sv = {
        "server_id": prof.get("id") or "",
        "server_name": prof.get("name") or "ACECM",
        "server_ip": "127.0.0.1",
        "server_tcp_port": int(prof.get("tcp_port") or 9700),
        "server_udp_port": int(prof.get("tcp_port") or 9700),
        "cars": servers.allowed_car_ids(prof),
    }
    if not backend._game_running():
        _set(phase="launching_game", hint="launching the game")
        launched = backend.launch_game(extra_args=["-no_intro"])
        _JOB["launch"] = {k: launched.get(k)
                          for k in ("ok", "error", "via", "backend",
                                    "inspector_patched", "needs_admin")
                          if k in launched}
        if not launched.get("ok"):
            _set(phase="failed",
                 fault=launched.get("error") or "could not launch")
            return
    poked = _enter_and_join(pick, sv)
    _JOB["join"] = {k: poked.get(k) for k in
                    ("ok", "error", "note", "connected") if k in poked}
    if not poked.get("ok"):
        _set(phase="failed",
             fault=poked.get("error") or "could not join the local server")
        return
    _set(phase="launched",
         hint=poked.get("note") or "joining your server — pit menu next")


def _find_public(pick):
    sid = (pick.get("server_id") or "").strip()
    ip = (pick.get("server_ip") or "").strip()
    tcp = int(pick.get("server_tcp_port") or 0)
    for s in (backend.server_list().get("servers") or []):
        if sid and str(s.get("server_id") or "") == sid:
            return s
        if ip and int(s.get("server_tcp_port") or 0) == tcp \
                and str(s.get("server_ip") or "") == ip:
            return s
    if ip and tcp:
        return {"server_name": ip, "server_ip": ip,
                "server_tcp_port": tcp,
                "server_udp_port": pick.get("server_udp_port") or tcp,
                "cars": pick.get("allowed_cars") or []}
    return None


def _wait_page(want, seconds=20):
    """Wait until boot_state is this page (sp / mp / home). No extra goTo."""
    until = time.time() + seconds
    last = ""
    while time.time() < until:
        if not backend._game_running():
            return ""
        boot = gameui.boot_state()
        if boot and boot != last:
            logs.LOG.info("drive boot %s", boot)
            last = boot
        page = gameui.boot_page(boot)
        if page == want and not gameui.paintshop_up(boot):
            return boot
        if gameui.session_loading(boot) or gameui.in_pits(boot):
            return boot
        time.sleep(0.3)
    return last


def _enter_and_join(pick, sv):
    """Home screen, set an allowed car, then Connect once on the list."""
    deadline = time.time() + 90
    _set(phase="waiting_for_menu", hint="waiting for the menu")
    state = _wait_ready(deadline)
    if state == "gone":
        return {"ok": False, "error": "the game closed before join"}
    if state == "live":
        return {"ok": True, "started": "already"}
    if state != "ready":
        return {"ok": False, "error": _menu_not_ready_error("join")}
    model = _car_model(pick.get("car"))
    have = gameui.current_car_name()
    if model and have != model:
        _set(phase="selecting_car", hint="setting " + model)
        try:
            chosen = gameui.select_car(model, pick.get("car") or "")
            logs.LOG.info("drive set car: %s", chosen)
        except OSError as ex:
            return {"ok": False, "error": f"could not set car: {ex}"}
    host = (sv.get("server_ip") or "").strip()
    tcp = int(sv.get("server_tcp_port") or 0)
    udp = int(sv.get("server_udp_port") or tcp)
    pw = pick.get("password") or ""
    sid = str(sv.get("server_id") or pick.get("server_id") or "")
    if not host or not tcp:
        return {"ok": False, "error": "that server has no address"}
    _ensure_backend()
    _set(phase="joining",
         hint="opening Multiplayer for "
              + (sv.get("server_name") or host))
    gameui.focus_game()
    try:
        went = gameui.enter_multiplayer()
        logs.LOG.info("drive ui multiplayer: %s", went)
    except OSError as ex:
        logs.LOG.warning("drive multiplayer goto lost: %s", ex)
    on = _wait_page("mp", 20)
    if gameui.boot_page(on) != "mp" and not gameui.session_loading(on):
        return {"ok": False,
                "error": "Multiplayer never stayed open (last "
                         + (on or "-") + ")"}
    # Let the page fetch the list once. Do not refresh it — overlapping
    # ServerList replies while connecting crash the physics thread.
    time.sleep(2.5)
    picked = None
    sent = False
    until = time.time() + 50
    while time.time() < until:
        if not backend._game_running():
            return {"ok": False, "error": "the game closed before join"}
        try:
            picked = gameui.join_public(host, tcp, pw, sid)
        except OSError as ex:
            logs.LOG.warning("drive join_public lost: %s", ex)
            time.sleep(0.8)
            continue
        logs.LOG.info("drive ui pick: %s", picked)
        val = str((picked or {}).get("value") or "")
        if val.startswith("connect:") or val == "already-sent":
            sent = True
            break
        if val.startswith("not-in-list:"):
            _set(phase="joining",
                 hint="list is up — that server is not on this page yet")
        elif val in ("waiting-list", "loading", "no-page"):
            _set(phase="joining", hint="waiting for the in-game server list")
        elif val == "no-car":
            _set(phase="joining", hint="waiting for the current car")
        time.sleep(1.0)
    if not sent:
        gameui.focus_game()
        return {"ok": False,
                "error": "the in-game list never selected that server "
                         "(" + str((picked or {}).get("value")) + ")"}
    _set(phase="joining", hint="Connect sent — waiting for the session")
    gameui.focus_game()
    # No more inspector traffic: a second Connect while physics loads
    # access-violates the physics thread (seen 2026-08-14 19:22).
    until = time.time() + 40
    while time.time() < until:
        if not backend._game_running():
            return {"ok": False,
                    "error": "the game closed while joining "
                             "(likely a crash — try Join again)"}
        txt = _this_boot_log()
        if "Established connection to server" in txt and ":0" not in txt:
            return {"ok": True, "join": picked, "connected": True}
        if "Game Started!" in txt:
            return {"ok": True, "join": picked, "connected": True}
        time.sleep(0.8)
    return {"ok": True, "join": picked,
            "note": "Join was pressed once — if you are still on the list, "
                    "the server refused (full, wrong car, password)"}


def _run_server(pick):
    sv = _find_public(pick)
    if not sv:
        _set(phase="failed",
             fault="that public server is not in the captured list — "
                   "open Multiplayer in-game once to refresh it")
        return
    if not _car_allowed(sv, pick.get("car")):
        _set(phase="failed",
             fault="that car is not on this server's allowed list")
        return
    _ensure_backend()
    if not backend._game_running():
        _set(phase="launching_game", hint="launching the game")
        launched = backend.launch_game(extra_args=["-no_intro"])
        _JOB["launch"] = {k: launched.get(k)
                          for k in ("ok", "error", "via", "backend",
                                    "inspector_patched", "needs_admin")
                          if k in launched}
        if not launched.get("ok"):
            _set(phase="failed",
                 fault=launched.get("error") or "could not launch")
            return
    poked = _enter_and_join(pick, sv)
    _JOB["join"] = {k: poked.get(k) for k in
                    ("ok", "error", "note", "connected") if k in poked}
    if not poked.get("ok"):
        _set(phase="failed",
             fault=poked.get("error") or "could not join the server")
        return
    _set(phase="launched",
         hint=poked.get("note") or "joining the server — pit menu next")


def _run(pick):
    try:
        if (pick.get("via") or "sp") == "server":
            _run_server(pick)
            return
        if (pick.get("via") or "") == "local":
            _run_local(pick)
            return
        _stop_leftover_server()
        if backend._game_running():
            _ensure_backend()
            poked = _enter_and_start(pick)
            if not poked.get("ok"):
                _set(phase="failed",
                     fault=poked.get("error") or "could not press Start")
                return
            _set(phase="launched",
                 hint="in the pit menu — change setup, then Drive in-game")
            return
        _set(phase="writing", hint="writing the single-player session",
             fault="", join=None, wrote=None)
        wrote = _write_gamemode(pick)
        _JOB["wrote"] = {k: wrote.get(k) for k in
                         ("ok", "mode", "track", "layout", "car_model", "error")}
        if not wrote.get("ok"):
            _set(phase="failed", fault=wrote.get("error") or "could not write save")
            return
        extra = [
            "-no_intro",
            f"-startup_gamemode={wrote['mode']}",
            f"--startup_gamemode={wrote['mode']}",
        ]
        if wrote.get("car_model"):
            extra += [
                f"-load_single_car={wrote['car_model']}",
                f"--load_single_car={wrote['car_model']}",
            ]
        threading.Thread(target=_ensure_backend, daemon=True).start()
        _set(phase="launching_game",
             hint=f"launching {wrote['mode']} at {wrote['track']}")
        launched = backend.launch_game(extra_args=extra)
        _JOB["launch"] = {k: launched.get(k)
                          for k in ("ok", "error", "via", "backend",
                                    "inspector_patched", "needs_admin")
                          if k in launched}
        if not launched.get("ok"):
            _set(phase="failed",
                 fault=launched.get("error") or "could not launch")
            return
        poked = _enter_and_start(pick)
        if not poked.get("ok"):
            _set(phase="failed",
                 fault=poked.get("error") or "could not press Start in the UI")
            return
        _set(phase="launched",
             hint="in the pit menu — change setup, then Drive in-game")
    except Exception as ex:
        logs.exception("drive", ex)
        _set(phase="failed", fault=f"{type(ex).__name__}: {ex}")


_LOG_TS = re.compile(r"\[(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2})")


def _this_boot_log():
    """Newest log for THIS Drive. Steam may write a tiny stub then a real one."""
    since = int(_JOB.get("started") or 0)
    try:
        from . import settings as gamesettings
        folder = os.path.join(gamesettings.settings_dir() or "", "Logs")
        names = [n for n in os.listdir(folder)
                 if n.lower().startswith("log-") and n.lower().endswith(".txt")]
    except Exception:
        return ""
    cands = []
    for n in names:
        path = os.path.join(folder, n)
        try:
            st = os.stat(path)
        except OSError:
            continue
        if since and st.st_mtime < since - 5:
            continue
        cands.append((st.st_size, path))
    cands.sort(reverse=True)
    for size, path in cands[:2]:
        try:
            with open(path, encoding="utf-8", errors="replace") as f:
                if size > 250000:
                    f.seek(size - 250000)
                txt = f.read()
        except OSError:
            continue
        m = _LOG_TS.search(txt[:4000] if size <= 250000 else "")
        if m and since:
            try:
                t0 = time.mktime(time.strptime(m.group(1), "%Y-%m-%d %H:%M:%S"))
            except Exception:
                t0 = 0
            if t0 and t0 + 3 < since:
                continue
        return txt
    return ""


def _menu_boot_done(txt=None):
    """True after the home page exists and the paintshop overlay is gone.

    Starting during paintshop leaves an override:
      goto menu.html main/main  override: true
    Session start then skips 'goto_loadingpage timeattack' and dumps
    you on the main menu with the session running behind it.
    """
    txt = _this_boot_log() if txt is None else txt
    if not txt:
        return False
    mp = txt.rfind("MainPage template loaded")
    if mp < 0:
        return False
    ov = txt.rfind("override: true")
    return ov < mp


def _session_live(txt=None):
    """True once THIS boot has begun Practice. Log only — no inspector."""
    return "Game Started!" in (txt if txt is not None else _this_boot_log())


def _menu_not_ready_error(action="Start"):
    """Friend-facing reason the home screen never became Driveable."""
    if not gameui.listening():
        return (
            "the game launched but its menu inspector never came up on :"
            + str(gameui.PORT)
            + ". Drive cannot press " + action + " without it. Close EVO, "
            "then Drive again — ACECM will enable the inspector. If that "
            "write is refused, run ACECM as administrator once"
        )
    return ("the menu never became ready — open the game and press "
            + action + " once")


def _wait_ready(deadline):
    """Last live gate: menu.html + CurrentCar (or already on SP / pits).

    GAMEMODESELECTION exists on intro too. Starting then hits the
    paintshop override and dumps you on the home screen. The game log
    is not flushed until quit, so MainPage in the file is too late.
    """
    saw_game = False
    last_beat = 0
    last_boot = ""
    while time.time() < deadline:
        running = backend._game_running()
        if running:
            saw_game = True
        elif saw_game:
            logs.LOG.info("drive: game closed while waiting for the menu")
            return "gone"
        if _session_live():
            return "live"
        boot = gameui.boot_state()
        if boot and boot != last_boot:
            logs.LOG.info("drive boot %s", boot)
            last_boot = boot
        if gameui.session_loading(boot) or gameui.in_pits(boot):
            return "live"
        if gameui.home_ready(boot):
            return "ready"
        now = time.time()
        if now - last_beat >= 5:
            logs.LOG.info("drive waiting for home (game=%s inspector=%s boot=%s)",
                          running, gameui.listening(), boot or "-")
            last_beat = now
        time.sleep(0.3)
    return ""


def _enter_and_start(pick=None):
    """Set car / conditions once, open SP once, press Start once.

    The old loop called goTo + Start again whenever the session had not
    begun. The game answers a Start from home (or during paintshop) with
    `goto menu.html main/main`, so that retry is exactly the bounce both
    machines were seeing.
    """
    deadline = time.time() + 90
    _set(phase="waiting_for_menu", hint="waiting for the menu")
    state = _wait_ready(deadline)
    if state == "gone":
        return {"ok": False, "error": "the game closed before Start"}
    if state == "live":
        logs.LOG.info("drive session already live for this boot")
        gameui.focus_game()
        return {"ok": True, "started": "already"}
    if state != "ready":
        return {"ok": False, "error": _menu_not_ready_error("Start")}
    pick = pick or {}
    model = _car_model(pick.get("car"))
    if not backend._game_running():
        return {"ok": False, "error": "the game closed before Start"}
    if _session_live():
        gameui.focus_game()
        return {"ok": True, "started": "already"}
    gameui.focus_game()
    have = gameui.current_car_name()
    if model and have != model:
        _set(phase="selecting_car", hint="setting " + model)
        try:
            chosen = gameui.select_car(model, pick.get("car") or "")
        except OSError as ex:
            return {"ok": False, "error": "could not set car: " + str(ex)}
        logs.LOG.info("drive set car: %s", chosen)
    elif model:
        logs.LOG.info("drive car already %s", have)
    _set(phase="starting_session", hint="setting weather, time and mode")
    try:
        cond = gameui.apply_conditions(pick)
        logs.LOG.info("drive conditions: %s", cond)
    except OSError as ex:
        return {"ok": False, "error": "could not set conditions: " + str(ex)}
    _set(phase="starting_session", hint="opening Single Player")
    try:
        went = gameui.enter_singleplayer()
        logs.LOG.info("drive ui goto: %s", went)
    except OSError as ex:
        return {"ok": False, "error": "could not open Single Player: " + str(ex)}
    on = _wait_page("sp", 20)
    if gameui.boot_page(on) != "sp":
        return {"ok": False,
                "error": "Single Player never stayed open — the game "
                         "kept returning to the home menu (last "
                         + (on or "-") + ")"}
    _set(phase="starting_session", hint="starting the session")
    try:
        started = gameui.press_start()
    except OSError as ex:
        return {"ok": False, "error": "Start failed: " + str(ex)}
    logs.LOG.info("drive ui start: %s", started)
    gameui.focus_game()
    val = str((started or {}).get("value") or "")
    if not (started and started.get("ok")):
        return {"ok": False, "error": "Start failed: "
                + str((started or {}).get("error") or "no reply")}
    if val.startswith("fail:") or val.startswith("no-") or val in (
            "not-on-sp", "paintshop"):
        return {"ok": False, "error": "Start failed: " + val}
    until = time.time() + 25
    last_boot = on
    while time.time() < until:
        if not backend._game_running():
            return {"ok": False, "error": "the game closed after Start"}
        boot = gameui.boot_state()
        if boot and boot != last_boot:
            logs.LOG.info("drive boot %s", boot)
            last_boot = boot
        if (gameui.session_loading(boot) or gameui.in_pits(boot)
                or _session_live()):
            logs.LOG.info("drive session loading (%s)", boot)
            gameui.focus_game()
            return {"ok": True, "start": started}
        time.sleep(0.4)
    return {"ok": False,
            "error": "Start was pressed once but the session never "
                     "loaded (still " + (last_boot or "on the menu") + ")"}


def start(body=None):
    """Write the SP save and launch. Returns once the job is queued."""
    body = body or {}
    prev = _load_pick()
    via = (body.get("via") or prev.get("via") or "sp").strip().lower()
    if via not in ("sp", "server", "local"):
        via = "sp"
    pick = _save_pick({
        "via": via,
        "local_id": (body.get("local_id") or prev.get("local_id") or "").strip(),
        "server_id": (body.get("server_id") or prev.get("server_id") or "").strip(),
        "server_ip": (body.get("server_ip") or prev.get("server_ip") or "").strip(),
        "server_tcp_port": int(body.get("server_tcp_port")
                               if body.get("server_tcp_port") is not None
                               else prev.get("server_tcp_port") or 0),
        "server_udp_port": int(body.get("server_udp_port")
                               if body.get("server_udp_port") is not None
                               else prev.get("server_udp_port") or 0),
        "password": (body.get("password") if "password" in body
                     else prev.get("password") or ""),
        "car": (body.get("car") or "").strip(),
        "track_index": int(body.get("track_index")
                           if body.get("track_index") is not None
                           else prev["track_index"]),
        "custom_track": (body.get("custom_track")
                         if "custom_track" in body
                         else prev.get("custom_track") or "").strip(),
        "game_mode": (body.get("game_mode") or "PRACTICE").strip().upper(),
        "weather": (body.get("weather") or "CLEAR").strip().upper(),
        "tod_hour": int(body.get("tod_hour")
                        if body.get("tod_hour") is not None else 13),
        "tod_minute": int(body.get("tod_minute")
                          if body.get("tod_minute") is not None else 0),
        "num_opponents": int(body.get("num_opponents")
                             if body.get("num_opponents") is not None
                             else prev.get("num_opponents", 10)),
        "skill_min": int(body.get("skill_min")
                         if body.get("skill_min") is not None
                         else prev.get("skill_min", 80)),
        "skill_max": int(body.get("skill_max")
                         if body.get("skill_max") is not None
                         else prev.get("skill_max", 95)),
        "aggressiveness": str(body.get("aggressiveness")
                              or prev.get("aggressiveness") or "Safe"),
        "single_make": bool(body["single_make"]) if "single_make" in body
                       else bool(prev.get("single_make", True)),
        "duration_min": int(body.get("duration_min")
                            if body.get("duration_min") is not None
                            else prev.get("duration_min", 90)),
        "practice_min": int(body.get("practice_min")
                            if body.get("practice_min") is not None
                            else prev.get("practice_min", 10)),
        "quali_min": int(body.get("quali_min")
                         if body.get("quali_min") is not None
                         else prev.get("quali_min", 15)),
        "warmup_min": int(body.get("warmup_min")
                          if body.get("warmup_min") is not None
                          else prev.get("warmup_min", 10)),
        "race_laps": int(body.get("race_laps")
                         if body.get("race_laps") is not None
                         else prev.get("race_laps", 10)),
        "starting_position": int(body.get("starting_position")
                                 if body.get("starting_position") is not None
                                 else prev.get("starting_position", 0)),
    })
    if not pick.get("car"):
        return {"ok": False, "error": "pick a car first"}
    if pick.get("via") == "server":
        sv = _find_public(pick)
        if not sv:
            return {"ok": False, "error": "pick a public server first"}
        if not _car_allowed(sv, pick.get("car")):
            return {"ok": False,
                    "error": "that car is not allowed on this server"}
    elif pick.get("via") == "local":
        prof = _find_local(pick)
        if not prof:
            return {"ok": False, "error": "pick one of your ACECM servers"}
        if not _car_allowed({"cars": servers.allowed_car_ids(prof)},
                            pick.get("car")):
            return {"ok": False,
                    "error": "that car is not allowed on this server"}
    elif pick["game_mode"] not in SP_MODES:
        return {"ok": False, "error": f"{pick['game_mode']} is not a "
                                      "single-player mode"}
    phase = _JOB.get("phase")
    if phase in ("writing", "launching_game", "starting_backend",
                 "waiting_for_menu", "waiting_for_session",
                 "entering", "selecting_car", "starting_session",
                 "starting_server", "joining",
                 "capturing_list", "quitting_game"):
        if phase == "waiting_for_menu" and not backend._game_running():
            _set(phase="failed", fault="the game closed before Start")
        else:
            return {"ok": False, "error": "Drive is already running — wait for it"}
    if not content.tracks().get("tracks"):
        return {"ok": False,
                "error": "no track list — set the dedicated server folder "
                         "in Settings"}
    running = backend._game_running()
    if running and not gameui.listening():
        return {"ok": False, "error": _menu_not_ready_error("Start")}
    if pick.get("via") in ("server", "local"):
        _set(phase="entering" if running else "launching_game",
             hint="joining…" if running else "launching the game…",
             fault="", started=int(time.time()),
             join=None, launch=None, wrote=None)
    else:
        _set(phase="writing" if not running else "entering",
             hint="writing session…" if not running else "pressing Start…",
             fault="", started=int(time.time()),
             join=None, launch=None, wrote=None)
    threading.Thread(target=_run, args=(pick,), daemon=True).start()
    return {"ok": True, "phase": _JOB["phase"], "pick": pick,
            "hint": "writing the session, launching, then pressing Start"}


def _game_still_up():
    """Process table OR the window — inspector dying is not the game exiting."""
    if backend._game_running():
        return True
    try:
        return bool(gameui.window_open())
    except Exception:
        return False


def _close_game():
    """Ask the menu to quit, close the window, then force-kill the tree.

    QuitGame from the multiplayer page often only returns 'quit' and leaves
    the window up (or a confirm dialog). The inspector can also be dead
    after a hung refresh, so JS is not enough — WM_CLOSE then taskkill /T.
    """
    from . import winproc
    try:
        logs.LOG.info("drive quit: %s", gameui.quit_game())
    except Exception as ex:
        logs.LOG.warning("drive quit js: %s", ex)
    try:
        if gameui.close_window():
            logs.LOG.info("drive quit: posted WM_CLOSE")
    except Exception as ex:
        logs.LOG.warning("drive quit wm_close: %s", ex)
    deadline = time.time() + 5
    while time.time() < deadline:
        if not _game_still_up():
            return True
        time.sleep(0.3)
    try:
        logs.LOG.info("drive quit: taskkill %s",
                      winproc.kill_named("AssettoCorsaEVO.exe"))
    except Exception as ex:
        logs.LOG.warning("drive quit taskkill: %s", ex)
    for pid in (winproc.pids_named("AssettoCorsaEVO")
                + winproc.pids_named_prefix("assettocorsaevo")):
        try:
            winproc.kill(pid)
            logs.LOG.info("drive killed leftover EVO pid %s", pid)
        except Exception as ex:
            logs.LOG.warning("drive kill %s: %s", pid, ex)
    time.sleep(0.5)
    up = _game_still_up()
    if up:
        logs.LOG.warning("drive quit: game still running after force-kill")
    return not up


def _fresh_list(since):
    lst = backend.server_list()
    n = len(lst.get("servers") or [])
    at = int(lst.get("captured_at") or 0)
    return n, at, bool(n and at >= since - 3)


def _run_capture():
    launched_us = False
    since = int(_JOB.get("started") or time.time())
    try:
        _ensure_backend()
        st = backend.state()
        if not st.get("listening"):
            _set(phase="capturing_list", hint="starting the lobby proxy")
            r = backend.start("proxy")
            if not r.get("ok"):
                _set(phase="failed",
                     fault=r.get("error") or "could not start the proxy")
                return
        if backend._game_running() and not gameui.listening():
            _set(phase="failed", fault=_menu_not_ready_error("capture"))
            return
        if not backend._game_running():
            _set(phase="launching_game",
                 hint="launching the game to pull the public list")
            launched = backend.launch_game(extra_args=["-no_intro"])
            _JOB["launch"] = {k: launched.get(k)
                              for k in ("ok", "error", "via", "backend",
                                        "inspector_patched", "needs_admin")
                              if k in launched}
            if not launched.get("ok"):
                _set(phase="failed",
                     fault=launched.get("error") or "could not launch")
                return
            launched_us = True
        _set(phase="waiting_for_menu", hint="waiting for the home screen")
        state = _wait_ready(time.time() + 90)
        if state == "gone":
            _set(phase="failed", fault="the game closed before the list arrived")
            return
        if state != "ready" and state != "live":
            if launched_us:
                _close_game()
            _set(phase="failed",
                 fault=_menu_not_ready_error("capture"))
            return
        _set(phase="capturing_list", hint="opening the in-game server list")
        gameui.focus_game()
        try:
            logs.LOG.info("drive capture goto: %s", gameui.enter_multiplayer())
        except Exception as ex:
            logs.LOG.warning("drive capture goto: %s", ex)
        time.sleep(1.2)
        try:
            logs.LOG.info("drive capture refresh: %s",
                          gameui.refresh_server_list())
        except Exception as ex:
            logs.LOG.warning("drive capture refresh: %s", ex)
        deadline = time.time() + 75
        last_n = 0
        while time.time() < deadline:
            if not backend._game_running():
                _set(phase="failed",
                     fault="the game closed before the list arrived")
                return
            n, at, fresh = _fresh_list(since)
            last_n = n
            if fresh and n >= 10:
                break
            _set(phase="capturing_list",
                 hint="waiting for the public list… " + str(n) + " so far")
            if time.time() > deadline - 12:
                try:
                    gameui.refresh_server_list()
                except Exception:
                    pass
            time.sleep(0.6)
        else:
            n, at, fresh = _fresh_list(since)
            last_n = n
            if not (fresh and n >= 1):
                if launched_us:
                    _close_game()
                _set(phase="failed",
                     fault="the public list did not come through — "
                           "is the lobby URL rewritten? Open Multiplayer "
                           "once and watch the Backend tab.")
                return
        _set(phase="quitting_game",
             hint="got " + str(last_n) + " servers — closing the game")
        _close_game()
        _JOB["captured"] = last_n
        _set(phase="launched",
             hint="captured " + str(last_n)
                  + " public servers — game closed, Drive is ready")
    except Exception as ex:
        logs.exception("drive capture", ex)
        if launched_us:
            try:
                _close_game()
            except Exception:
                pass
        _set(phase="failed", fault=f"{type(ex).__name__}: {ex}")


def capture_list():
    """Launch, open Multiplayer, write server_list.json, quit the game."""
    phase = _JOB.get("phase")
    if phase in ("writing", "launching_game", "starting_backend",
                 "waiting_for_menu", "waiting_for_session",
                 "entering", "selecting_car", "starting_session",
                 "starting_server", "joining",
                 "capturing_list", "quitting_game"):
        return {"ok": False, "error": "Drive is already running — wait for it"}
    if backend._game_running() and not gameui.listening():
        return {"ok": False, "error": _menu_not_ready_error("capture")}
    _set(phase="capturing_list", hint="preparing to pull the public list",
         fault="", started=int(time.time()), join=None, launch=None,
         wrote=None, captured=0)
    threading.Thread(target=_run_capture, daemon=True).start()
    return {"ok": True, "phase": _JOB["phase"],
            "hint": "launching the game, grabbing the list, then quitting"}


def _public_briefs():
    lst = backend.server_list()
    out = []
    for s in lst.get("servers") or []:
        cars = _car_ids_of(s)
        out.append({
            "id": s.get("server_id") or "",
            "name": s.get("server_name") or "(unnamed)",
            "server_ip": s.get("server_ip") or "",
            "server_tcp_port": s.get("server_tcp_port") or 0,
            "server_udp_port": s.get("server_udp_port") or 0,
            "track": s.get("track") or "",
            "layout": s.get("layout") or "",
            "game_mode": s.get("game_mode_type") or s.get("current_session") or "",
            "players": s.get("players") or 0,
            "max_players": s.get("max_players") or 0,
            "ping": s.get("ping") or 0,
            "cars": cars,
            "locked": bool(s.get("driver_password")),
        })
    # ⚠ Car ids are sent INTERNED: a pool of the distinct ids, and each
    # server's list as indices into it. A full public list is ~35k car
    # references drawn from ~143 distinct ids, so spelling every one out made
    # that single field 75% of the whole Drive payload (825 KB of 1.1 MB).
    # The browser expands it back to strings on arrival, so everything
    # downstream still sees a plain list of ids - and because the expansion
    # reuses the pool's strings, the page holds 143 strings instead of 35k.
    pool = []
    seen = {}
    for s in out:
        idx = []
        for cid in s["cars"]:
            i = seen.get(cid)
            if i is None:
                i = seen[cid] = len(pool)
                pool.append(cid)
            idx.append(i)
        s["cars"] = idx
    return {
        "servers": out,
        "car_pool": pool,
        "count": lst.get("count") or len(out),
        "captured_at": lst.get("captured_at"),
        "cached": bool(lst.get("cached")),
        "error": lst.get("error"),
        "hint": lst.get("hint"),
    }


FAVOURITES = os.path.join(config.DATA, "favourites.json")


def favourites():
    """Servers the user pinned, newest first.

    The captured public list is a snapshot that goes stale and is rebuilt by
    launching the game; a favourite is an address, which does not. So this
    stores the address and the name it had when it was saved, and looks the
    rest up fresh each time it is shown.
    """
    try:
        items = json.load(open(FAVOURITES, encoding="utf-8"))
    except Exception:
        items = []
    return {"ok": True, "favourites": [f for f in items if f.get("ip")]}


def _save_favourites(items):
    try:
        os.makedirs(os.path.dirname(FAVOURITES), exist_ok=True)
        json.dump(items, open(FAVOURITES, "w", encoding="utf-8"), indent=2)
    except OSError as ex:
        logs.LOG.warning("could not write favourites: %s", ex)
    return items


def favourite_add(target, name=""):
    """Pin an address. Re-adding one updates its name rather than duplicating."""
    info = direct_lookup(target)
    if not info.get("ok"):
        return info
    ip, tcp = info["ip"], info["tcp"]
    items = favourites()["favourites"]
    # ⚠ ip+port is the identity, NOT the name. Server names change (they carry
    # player counts and Discord invites), and keying on them would pin the
    # same server twice the moment its name did.
    items = [f for f in items
             if not (f.get("ip") == ip and int(f.get("tcp") or 0) == tcp)]
    items.insert(0, {
        "id": f"{ip}:{tcp}",
        "name": (name or info.get("name") or f"{ip}:{tcp}").strip(),
        "ip": ip, "tcp": tcp, "udp": info.get("udp") or tcp,
        "added": int(time.time()),
    })
    _save_favourites(items)
    return {"ok": True, "favourites": items, "added": f"{ip}:{tcp}"}


def favourite_remove(fid):
    items = [f for f in favourites()["favourites"] if f.get("id") != fid]
    _save_favourites(items)
    return {"ok": True, "favourites": items}


def direct_lookup(raw):
    """Work out what is at a pasted address, so you can join without the list.

    The captured public list only exists after launching the game and opening
    Multiplayer, which is a lot of ceremony for "my friend sent me an IP". The
    join itself never needed it - _find_public already falls back to a plain
    ip+port - so this is about showing what you are about to join.

    Three tiers, best first:
      * the address is in the captured list -> the real entry, cars and all
      * the host runs ACECM -> its own registry says the name, and which
        tracks and mods the server needs
      * neither -> the address, honestly labelled as unknown. Still joinable;
        the game is the thing that decides whether it works.

    ⚠ The game port and the ACECM port are different (9700 vs 8092), and
    typing one where the other belongs is the obvious mistake. The pasted
    port is used for the GAME; ACECM is probed on its own ports.
    """
    host, port, _base = contentsync.parse_target(raw)
    if not host:
        return {"ok": False, "error": "type an address, like 1.2.3.4:9700"}
    tcp = int(port or 9700)

    for s in (backend.server_list().get("servers") or []):
        if str(s.get("server_ip") or "") == host \
                and int(s.get("server_tcp_port") or 0) == tcp:
            return {"ok": True, "source": "list", "ip": host, "tcp": tcp,
                    "udp": int(s.get("server_udp_port") or tcp),
                    "name": s.get("server_name") or host,
                    "track": s.get("track") or "",
                    "layout": s.get("layout") or "",
                    "players": s.get("players") or 0,
                    "max_players": s.get("max_players") or 0,
                    "locked": bool(s.get("driver_password")),
                    "cars": _car_ids_of(s)}

    # ⚠ Two ports, briefly - the same trap /api/browser/discover documents.
    # discover's default is seven ports at four seconds each, so a plain game
    # server that is not running ACECM took 28 SECONDS to come back and the
    # window looked hung. The pasted port is the GAME's; ACECM only ever
    # answers on its own, so probing the rest of the range buys nothing here.
    try:
        got = contentsync.discover(host, ports=(8092, 8093), timeout=2.0)
    except Exception as ex:
        got = {"ok": False, "error": str(ex)}
    if got.get("ok"):
        # an ACECM host can run several servers; prefer the one on this port
        entries = got.get("servers") or []
        # ⚠ Only accept a port match when it is UNAMBIGUOUS. A registry entry
        # defaults to 9700 and nothing forces a host to change it, so a
        # machine running several servers usually has every entry claiming
        # 9700 - taking "the first one on that port" would confidently fetch
        # the wrong server's content. One match means one match; anything
        # else falls through to offering them all.
        onport = [e for e in entries if int(e.get("port") or 0) == tcp]
        hit = onport[0] if len(onport) == 1 else (
            entries[0] if len(entries) == 1 else None)
        if hit:
            return {"ok": True, "source": "acecm", "ip": host,
                    "tcp": int(hit.get("port") or tcp), "udp": tcp,
                    "name": hit.get("name") or host,
                    "description": hit.get("description") or "",
                    "track": ", ".join(hit.get("required_tracks") or []),
                    "needs_mods": hit.get("required_mods") or [],
                    "needs_tracks": hit.get("required_tracks") or [],
                    "content_bytes": hit.get("content_bytes") or 0,
                    "base": got.get("base") or "",
                    # what Get content needs to fetch from this host
                    "sid": hit.get("id") or "",
                    "cars": []}
        # ⚠ Several servers behind one ACECM and none on the pasted port. We
        # cannot tell which one is meant, so offer them ALL to Get content
        # rather than guessing - fetching content for a server you did not
        # mean is slow, not wrong.
        ambiguous = len(onport) > 1
        return {"ok": True, "source": "acecm", "ip": host, "tcp": tcp,
                "udp": tcp, "name": host, "base": got.get("base") or "",
                "sids": [e.get("id") for e in entries if e.get("id")],
                "names": [e.get("name") or e.get("id") for e in entries],
                "note": (f"{len(onport)} of this host's {len(entries)} servers "
                         f"claim port {tcp}, so which one you mean is not "
                         f"clear - Get content will fetch what they all need"
                         if ambiguous else
                         f"{len(entries)} servers shared here and none lists "
                         f"port {tcp} - Get content will fetch what they all "
                         f"need"),
                "cars": []}

    return {"ok": True, "source": "unknown", "ip": host, "tcp": tcp,
            "udp": tcp, "name": f"{host}:{tcp}", "cars": [],
            "note": "not in your captured list and the host is not sharing "
                    "through ACECM, so its track and cars are unknown - "
                    "joining will still work if the address is right"}


def public_servers():
    """Just the public list, for the page to fetch after it has drawn.

    ⚠ Split out of options() on purpose. This is the slow half - it can be a
    live fetch of ~800 servers - and the Drive page does not need it to draw
    the car picker, the track list or the Drive button. Bundled together, one
    slow list made the whole page arrive late every time you switched back.
    """
    pub = _public_briefs()
    return {
        "ok": True,
        "servers": pub["servers"],
        "car_pool": pub["car_pool"],
        "servers_meta": {k: pub[k] for k in
                         ("count", "captured_at", "cached", "error", "hint")},
    }


def options():
    tracks = content.tracks()
    cars = content.cars()
    be = backend.state()
    return {
        "ok": True,
        "pick": _load_pick(),
        "game_modes": SP_MODES,
        "weather": servers.OPTIONS["weather"],
        "aggressiveness": list(AGGRO),
        "ai_modes": list(AI_MODES),
        "tracks": tracks.get("tracks") or [],
        "tracks_error": tracks.get("error"),
        "cars": cars.get("cars") or [],
        "cars_error": cars.get("error"),
        # the public list arrives separately - see public_servers()
        "servers": [],
        "car_pool": [],
        "servers_pending": True,
        "servers_meta": {},
        "local_servers": _local_briefs(),
        "backend": {
            "listening": bool(be.get("listening")),
            "client_patched": bool(be.get("client_patched")),
        },
    }


def status():
    pick = _load_pick()
    exe = backend._game_exe()
    return {
        "ok": True,
        "pick": pick,
        "phase": _JOB.get("phase") or "idle",
        "hint": _JOB.get("hint") or "",
        "fault": _JOB.get("fault") or "",
        "started": _JOB.get("started") or 0,
        "wrote": _JOB.get("wrote"),
        "launch": _JOB.get("launch"),
        "game_running": bool(exe and backend._game_running(exe)),
        "single_player": True,
        "captured": _JOB.get("captured") or 0,
    }
