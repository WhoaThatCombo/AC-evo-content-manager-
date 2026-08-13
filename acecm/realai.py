"""Launch a *real* AI grid on the client (AiDriverEvo), not dedicated vAI.

The dedicated server has no AiDriverEvo. -virtual_ai_cars only interpolates a
reference lap (sendCarPhysicsUpdate is a stub). The client already knows how
to spawn LocalAIControlled opponents from Instant Race + AiCarData.

This is one process: start the exe with -ai_enable_evo_next and
-opponent_count. Not a client-per-bot farm.
"""
import os

from . import backend
from . import settings as gamesettings

INSTANT_SAVE = os.path.join("GameModes", "GameModeType_INSTANT_RACE.gamemodesave")


def _ace_path(rel):
    root = gamesettings.settings_dir()
    return os.path.join(root, rel.replace("/", os.sep))


def prepare_instant_race(opponents=16, min_strength=70, max_strength=95):
    """Do not rewrite GameModeSave.

    Our extracted GameModeSave schema is thinner than the on-disk blob.
    A round-trip dropped most opponent records (22 → 6). The client flag
    -opponent_count= already overrides the UI count, so we leave the save
    the game wrote alone.
    """
    path = _ace_path(INSTANT_SAVE)
    return {
        "file": path,
        "wrote": False,
        "exists": os.path.isfile(path),
        "num_opponents": int(opponents),
        "min_strength": int(min_strength),
        "max_strength": int(max_strength),
        "note": "count is passed as -opponent_count; save file not touched",
    }


def _state_path():
    from . import config
    return os.path.join(config.DATA, "ai_worker.json")


def _load_state():
    import json
    try:
        return json.load(open(_state_path(), encoding="utf-8"))
    except Exception:
        return {}


def _save_state(st):
    import json
    from . import config
    os.makedirs(config.DATA, exist_ok=True)
    try:
        json.dump(_plain(st), open(_state_path(), "w", encoding="utf-8"),
                  indent=2, default=str)
    except Exception:
        json.dump({"phase": st.get("phase"), "error": "state not serialisable"},
                  open(_state_path(), "w", encoding="utf-8"), indent=2)
    return st


def _plain(obj):
    """Drop cycles (join result used to put itself in attempts)."""
    if isinstance(obj, dict):
        return {k: _plain(v) for k, v in obj.items() if k != "attempts"}
    if isinstance(obj, list):
        return [_plain(x) for x in obj]
    return obj


def _newest_client_log():
    d = os.path.join(gamesettings.settings_dir() or "", "Logs")
    if not os.path.isdir(d):
        return ""
    logs = []
    for n in os.listdir(d):
        p = os.path.join(d, n)
        if os.path.isfile(p) and n.lower().endswith(".txt"):
            logs.append(p)
    if not logs:
        return ""
    return max(logs, key=os.path.getmtime)


_MARKERS = (
    "Creating AiDriverEvo",
    "Invalid or incomplete aiCarData",
    "FLAGS_ai_player_car",
    "LocalAIControlled",
    "MultiplayerResponseGoToServer",
    "Established connection to server",
    "RequestSpawnAiCar",
    "SpawnAi",
    "connecting gamecar",
    "virtual_ai",
    "VirtualAI",
    "sendCarPhysicsUpdate",
)


def scan_worker_logs(server_log=""):
    """Pull the lines that tell us if the worker is real AI or vAI."""
    out = {"client_log": "", "client_hits": [], "server_hits": []}
    cl = _newest_client_log()
    out["client_log"] = cl
    if cl:
        try:
            txt = open(cl, encoding="utf-8", errors="replace").read()[-200_000:]
            out["client_hits"] = [ln for ln in txt.splitlines()
                                  if any(m.lower() in ln.lower() for m in _MARKERS)][-40:]
        except OSError:
            pass
    if server_log and os.path.isfile(server_log):
        try:
            txt = open(server_log, encoding="utf-8", errors="replace").read()[-200_000:]
            keys = ("connecting gamecar", "connected (", "new carId",
                    "SpawnAi", "VirtualAI", "vAI", "AiDriver")
            out["server_hits"] = [ln for ln in txt.splitlines()
                                  if any(k.lower() in ln.lower() for k in keys)][-40:]
        except OSError:
            pass
    evos = sum(1 for ln in out["client_hits"] if "Creating AiDriverEvo" in ln)
    joined = any(
        "Established connection to server" in ln and ":0" not in ln
        for ln in out["client_hits"])
    out["ai_driver_evo_lines"] = evos
    out["joined"] = joined
    return out


def _menu_ready():
    cl = _newest_client_log()
    if not cl:
        return False
    try:
        txt = open(cl, encoding="utf-8", errors="replace").read()[-120_000:]
    except OSError:
        return False
    # Only the finished main page. The earlier "Init: menuState ... menu.html"
    # line is mid-load; poking / go_to_server there races the showroom car.
    return "MainPage template loaded" in txt


def _wait_and_join(host, tcp, udp=None, password=""):
    """Wait for lobby + main menu, poke LocalAI, then push join:<ip>:<port>.

    Do not enable FLAGS_ai_player_car at process start — that turns the
    paint-shop car into LocalAI and the client stops reading lobby traffic.
    Last successful go_to_server was while sitting on menu.html.
    """
    import time
    last = {}
    connected_at = None
    poked = None
    attempts = []
    deadline = time.time() + 90
    while time.time() < deadline:
        last = backend.join_state()
        if last.get("client_connected") and connected_at is None:
            connected_at = time.time()
        ready = bool(connected_at and _menu_ready())
        if ready:
            if poked is None:
                poked = backend.poke_ai_player_flag(True)
            # One pair of shapes per pass, then wait. Spam was lighting up
            # the proxy while the client was still settling.
            for shape in ("bare", "wrapped"):
                r = _plain(backend.join(host, shape=shape, tcp=tcp,
                                        udp=udp or tcp, password=password))
                r["poke"] = poked
                attempts.append(r)
            time.sleep(8)
            scan = scan_worker_logs()
            if scan.get("joined") or any(
                    "MultiplayerResponseGoToServer" in ln
                    for ln in scan.get("client_hits") or []):
                return {"ok": True, "sent": True, "attempts": attempts,
                        "join_state": last, "poke": poked, "scan": scan}
            if len(attempts) >= 8:
                break
        time.sleep(2)
    if attempts:
        return {"ok": True, "sent": True,
                "note": "pushed go_to_server; client has not logged the join yet",
                "attempts": attempts, "join_state": last, "poke": poked}
    return {"ok": False,
            "error": "worker never attached to the lobby — start the proxy "
                     "backend first, and confirm launch used -backend=",
            "join_state": last, "poke": poked}


def attach_worker(profile_id, ai_player=True):
    """Launch one client, join THIS dedicated server, run it as LocalAI.

    Proof we are looking for in the logs:
      client: Creating AiDriverEvo / FLAGS_ai_player_car
      client: Established connection to server 127.0.0.1
      server: connecting gamecar … (the worker)

    RequestSpawnAiCar is a *session* command (client → dedicated server),
    not a lobby message. This first slice gets a real-AI *player car* onto
    the session. Extra grid cars need that command after join works.
    """
    from . import lobby, logs, servers
    import threading
    import time

    prof = next((p for p in servers.load() if p["id"] == profile_id), None)
    if not prof:
        return {"ok": False, "error": "no such profile"}
    st = servers.status(prof)
    if not st.get("running"):
        return {"ok": False, "error": "start that dedicated server first"}
    b = backend.state()
    if not b.get("listening"):
        return {"ok": False, "error":
                "start the lobby backend (proxy) first — the worker has to "
                "see your server in the list"}
    flag = backend.restore_ai_flag_jumps()
    if not flag.get("ok"):
        return {"ok": False, "error": "could not restore ai_player_car jumps: "
                + str(flag.get("error"))}
    # Always refresh lobby.json from THIS profile. A stale file from another
    # server is how we previously pushed the wrong id/port.
    try:
        lobby.write(prof)
    except Exception as ex:
        logs.LOG.warning("lobby.write failed: %s", ex)
    tcp = int(prof.get("tcp_port") or 9700)
    host = "127.0.0.1"
    extra = ["-no_intro", "-ai_enable_evo_next"]
    # Do NOT pass -ai_player_car here. It is poked live after the menu.
    launched = backend.launch_game(extra_args=extra)
    if not launched.get("ok"):
        return launched
    rec = {
        "profile_id": prof["id"],
        "server_id": prof["id"],
        "join_url": f"join:{host}:{tcp} :{tcp}",
        "tcp": tcp,
        "started": int(time.time()),
        "phase": "launching",
        "launch": launched,
        "join": None,
        "ai": 0,
    }
    _save_state(rec)

    def _follow():
        rec["phase"] = "waiting_for_lobby"
        _save_state(rec)
        rec["join"] = _wait_and_join(host, tcp)
        rec["phase"] = "joined" if rec["join"].get("ok") or rec["join"].get("sent") \
            else "join_failed"
        time.sleep(8)
        slog = servers.status(prof).get("log") or ""
        rec["scan"] = scan_worker_logs(slog)
        rec["ai"] = rec["scan"].get("ai_driver_evo_lines") or 0
        _save_state(rec)
        logs.LOG.info("AI worker phase=%s join=%s evo_lines=%s",
                      rec["phase"], rec.get("join"), rec["ai"])

    threading.Thread(target=_follow, daemon=True).start()
    return {
        "ok": True,
        "phase": "launching",
        "server_id": prof["id"],
        "join_url": rec["join_url"],
        "profile": prof.get("name"),
        "hint": "Worker client starting. It will join '" + str(prof.get("name"))
                + "' via the lobby. Refresh status in ~30s. "
                "Look for Creating AiDriverEvo in the client log. "
                "Set this profile's AI count to 0 so vAI ghosts are not mixed in.",
        "launch": launched,
    }


def worker_status():
    rec = _load_state()
    if not rec:
        return {"ok": True, "attached": False}
    slog = ""
    try:
        from . import servers
        prof = next((p for p in servers.load()
                     if p["id"] == rec.get("profile_id")), None)
        if prof:
            slog = servers.status(prof).get("log") or ""
    except Exception:
        pass
    rec["scan"] = scan_worker_logs(slog)
    rec["attached"] = True
    rec["game_running"] = backend._game_running(
        backend._game_exe() or "AssettoCorsaEVO")
    return rec


def launch(opponents=16, min_strength=70, max_strength=95, small_window=False):
    """One client. Real AiDriverEvo opponents. Not dedicated-server vAI."""
    prep = prepare_instant_race(opponents, min_strength, max_strength)
    extra = [
        "-no_intro",
        "-ai_enable_evo_next",
        f"-opponent_count={int(opponents)}",
        f"-simexpo_ai_skill_min={int(min_strength)}",
        f"-simexpo_ai_skill_max={int(max_strength)}",
    ]
    if small_window:
        extra.append("-override_videosettings_width_height_fullscreen=1280,720,0")
    r = backend.launch_game(extra_args=extra)
    r["real_ai"] = True
    r["opponents"] = int(opponents)
    r["prepare"] = prep
    r["hint"] = (
        "One client. Single Player → Instant Race → Start. "
        "Log line to look for: Creating AiDriverEvo. "
        "The dedicated server's AI count is still vAI replay and is unused here."
    )
    return r
