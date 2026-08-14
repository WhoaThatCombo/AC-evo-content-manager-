"""Start the AC EVO dedicated server with virtual AI cars, and LEAVE IT UP.

Unlike the test harness this does not kill the server after a window, and it
does NOT pass -no_lobby, so the server registers with the lobby and shows in
the in-game browser.

Requires the loose AI spline files that make VirtualAIProvider work:
    content/tracks/<track>/layouts/<layout>.ideal_line.aisplinedata
    content/tracks/<track>/layouts/<layout>.pitlane.aisplinedata
The server's content.kspkg ships neither; the VFS falls back to loose files on
disk, which is how this is solved without touching the archive.

Stop it with:  taskkill /IM AssettoCorsaEVOServer.exe /F
Log:           serverConfig/vai_server.log
"""
import base64
import re
import json
import os
import struct
import subprocess
import sys
import zlib

# ⚠ The server folder comes from the ENVIRONMENT first, and only then from
# where this script happens to sit. In a frozen ACECM the script is unpacked
# into %TEMP%\_MEIxxxx\tools and run from there, so __file__ points at a temp
# folder that holds no exe, no events_*.json and no serverConfig - the launch
# silently does nothing and no log ever appears. It works from a source
# checkout purely because the script lives next to the server there.
SRV = (os.environ.get("SERVER_DIR")
       or os.path.dirname(os.path.abspath(__file__)))
EXE = os.path.join(SRV, os.environ.get("SERVER_EXE", "AssettoCorsaEVOServer.exe"))
LOG = os.path.join(SRV, "serverConfig",
                   os.environ.get("LOG_FILE", "vai_server.log"))

NAME = os.environ.get("SERVER_NAME", "vAI local test")
PORT = int(os.environ.get("PORT", "9700"))
HTTP = int(os.environ.get("HTTP_PORT", "8080"))
N_AI = os.environ.get("N_AI", "4")
MAX_PLAYERS = int(os.environ.get("MAX_PLAYERS", "24"))
EVENT_IDX = int(os.environ.get("EVENT_IDX", "0"))   # 0 = Brands Hatch GP
SKILL_MIN = os.environ.get("SKILL_MIN", "70")
SKILL_MAX = os.environ.get("SKILL_MAX", "95")
TOD_HOUR = int(os.environ.get("TOD_HOUR", "13"))     # midday
TOD_MINUTE = int(os.environ.get("TOD_MINUTE", "0"))
TIME_MULT = int(os.environ.get("TIME_MULT", "0"))    # 0 = frozen

# --- everything below used to be hardcoded ---------------------------------
# Values are the enum names the server itself accepts; the valid sets were read
# out of the protobuf schemas and the server binary, not guessed:
#   session   MultiplayerServerListSessionType_{BOTH,UNRANKED,RANKED}
#   game mode GameModeType_{PRACTICE,RACE_WEEKEND,INSTANT_RACE,HOTLAP,DRIFT,...}
#   weather   GameModeSelectionWeatherType_{CLEAR,SCATTERED_CLOUDS,BROKEN_CLOUDS,
#             OVERCAST,DRIZZLE,RAIN,HEAVY_RAIN,DAMP,CUSTOM}
#   behaviour GameModeSelectionWeatherBehaviour_{STATIC,DYNAMIC}
#   grip      InitialGrip_{GREEN,FAST,OPTIMUM}
#   tuning    TuningAllowed | TuningDenied
ADMIN_PASSWORD = os.environ.get("ADMIN_PASSWORD", "")
DRIVER_PASSWORD = os.environ.get("DRIVER_PASSWORD", "")
SPECTATOR_PASSWORD = os.environ.get("SPECTATOR_PASSWORD", "")
SESSION_TYPE = os.environ.get("SESSION_TYPE", "UNRANKED")
TUNING = os.environ.get("TUNING", "TuningAllowed")
PI_MIN = float(os.environ.get("PI_MIN", "0"))
PI_MAX = float(os.environ.get("PI_MAX", "0"))
GAME_MODE = os.environ.get("GAME_MODE", "PRACTICE")
WEATHER = os.environ.get("WEATHER", "CLEAR")
WEATHER_BEHAVIOUR = os.environ.get("WEATHER_BEHAVIOUR", "STATIC")
GRIP = os.environ.get("GRIP", "OPTIMUM")
PRACTICE_DURATION = int(os.environ.get("PRACTICE_DURATION", "9999"))
CYCLE = os.environ.get("CYCLE", "0") == "1"
# --- ports: listener and internal can differ (old manager exposed both) ----
PORT_TCP_INTERNAL = int(os.environ.get("PORT_TCP_INTERNAL", "0"))
PORT_UDP_INTERNAL = int(os.environ.get("PORT_UDP_INTERNAL", "0"))
# --- session pacing (were hardcoded to 10) ---------------------------------
OVERTIME_WAIT = int(os.environ.get("OVERTIME_WAIT", "10"))
MAX_WAIT_TO_BOX = int(os.environ.get("MAX_WAIT_TO_BOX", "10"))
# --- penalties, PER SERVER --------------------------------------------------
# Penalties do NOT have to be patched into content.kspkg. The exe builds the
# season itself from the -seasondefinition blob (a BuildSeasonDefinitionRequest),
# and SimpleGameConfig carries the penalty settings, so every server launched
# from this script can have its own without touching any archive.
#
# ⚠ enable_custom_penalities is spelled with the typo the exe uses. The JSON is
# matched by field NAME, so "penalties" would be silently ignored.
PENALTIES = os.environ.get("PENALTIES", "0") == "1"
# how many wheels may leave the track before it counts as a cut (1-4)
CAR_CUT_TYRES_OUT = int(os.environ.get("CAR_CUT_TYRES_OUT", "4"))
# warnings a driver gets before the penalty actually lands
WARNING_TRIGGER_COUNTDOWN = int(os.environ.get("WARNING_TRIGGER_COUNTDOWN", "3"))
# the time penalty itself, in milliseconds
TIME_PENALTY_MS = int(os.environ.get("TIME_PENALTY_MS", "5000"))
# --- full in-game date, not just the hour ----------------------------------
TOD_YEAR = int(os.environ.get("TOD_YEAR", "2024"))
TOD_MONTH = int(os.environ.get("TOD_MONTH", "8"))
TOD_DAY = int(os.environ.get("TOD_DAY", "15"))
TOD_SECOND = int(os.environ.get("TOD_SECOND", "0"))
# --- flags -----------------------------------------------------------------
# -no_lobby keeps the server OFF the public browser (it will not register with
# Kunos). Useful for a private session, and required if you want a clean
# window before players can find you.
NO_LOBBY = os.environ.get("NO_LOBBY", "0") == "1"
WRITE_RESULTS = os.environ.get("WRITE_RESULTS", "0") == "1"
EXPORT_JSON = os.environ.get("EXPORT_JSON", "0") == "1"
# per-car handicaps, as "car:ballast:restrictor,car2:..."
CAR_HANDICAPS = os.environ.get("CAR_HANDICAPS", "")
ENTRY_LIST_PATH = os.environ.get("ENTRY_LIST_PATH", "")
RESULTS_PATH = os.environ.get("RESULTS_PATH", "")
ENTRY_LIST_URL = os.environ.get("ENTRY_LIST_URL", "")
RESULTS_POST_URL = os.environ.get("RESULTS_POST_URL", "")


def _enum(prefix, value):
    """Accept either the bare name (RAIN) or the full enum (..._RAIN)."""
    v = (value or "").strip()
    return v if v.startswith(prefix) else f"{prefix}{v.upper()}"


def encode_blob(obj):
    j = json.dumps(obj).encode("utf-8")
    return base64.b64encode(struct.pack(">I", len(j)) + zlib.compress(j)).decode("ascii")


def main():
    # RACE_WEEKEND draws from a different event list than PRACTICE, so which
    # file to read has to follow the mode rather than being hardcoded.
    events_file = os.environ.get(
        "EVENTS_FILE",
        "events_race_weekend.json" if "RACE" in GAME_MODE.upper()
        else "events_practice.json")
    ev = json.load(open(os.path.join(SRV, events_file)))["events"][EVENT_IDX]
    # ⚠ A custom track is not IN events_*.json - those list the stock events
    # only, so an index can never name one. CUSTOM_EVENT carries the whole
    # event as JSON instead:
    #
    #     CUSTOM_EVENT='{"track":"Highlands Drift","layout":"layout_drift"}'
    #
    # JSON rather than a delimited string because a track name is free text -
    # any separator we pick is a name someone can legitimately use. The server
    # resolves the name through its own system/tracks.table, so whatever is
    # deployed under it is what loads.
    custom = os.environ.get("CUSTOM_EVENT", "").strip()
    if custom:
        want = json.loads(custom)
        if not want.get("track") or not want.get("layout"):
            raise SystemExit("CUSTOM_EVENT needs at least track and layout")
        ev = {**ev, **want}
        ev.setdefault("event_name", f"{want['layout']} Race")
        print(f"custom track: {ev['track']} / {ev['layout']} "
              f"({ev['event_name']})")
    # cars.json was dumped from a client that has car MODS installed. Real
    # Kunos presets are always "<code>_mech_<n>"; the mods come through as
    # names truncated to 13 chars with no preset suffix (Tesla Model S Plaid,
    # SRT Tomahawk, Bugatti Bolide Drag Spec, ...). The server's content.kspkg
    # does not contain them, so letting a player pick one is a broken join.
    _raw = [c["name"] for c in json.load(open(os.path.join(SRV, "cars.json")))["cars"]]
    _all = [c for c in _raw if re.fullmatch(r".+_mech_\d+", c)]
    _mods = [c for c in _raw if c not in _all]
    if _mods:
        print(f"  excluded {len(_mods)} non-Kunos/modded cars: {', '.join(_mods)}")
    # Default: every car in the game is joinable. CARS_OVERRIDE=a,b,c narrows it.
    _ov = os.environ.get("CARS_OVERRIDE", "")
    cars = [c.strip() for c in _ov.split(",") if c.strip()] if _ov else _all

    # per-car ballast/restrictor: "car:ballast:restrictor,..."
    handicap = {}
    for item in filter(None, CAR_HANDICAPS.split(",")):
        bits = item.split(":")
        if bits and bits[0].strip():
            handicap[bits[0].strip()] = (
                int(bits[1]) if len(bits) > 1 and bits[1] else 0,
                int(bits[2]) if len(bits) > 2 and bits[2] else 0)

    server_cfg = {
        "server_tcp_listener_port": PORT, "server_udp_listener_port": PORT,
        "server_tcp_internal_port": PORT_TCP_INTERNAL or PORT,
        "server_udp_internal_port": PORT_UDP_INTERNAL or PORT,
        "server_http_port": HTTP,
        "server_name": NAME,
        "max_players": MAX_PLAYERS,
        "cycle": CYCLE,
        "allowed_cars_list_full": [
            {"car_name": c,
             "ballast": handicap.get(c, (0, 0))[0],
             "restrictor": handicap.get(c, (0, 0))[1]} for c in cars],
        "driver_password": DRIVER_PASSWORD,
        "spectator_password": SPECTATOR_PASSWORD,
        "admin_password": ADMIN_PASSWORD,
        "type": _enum("MultiplayerServerListSessionType_", SESSION_TYPE),
        "tuning_type": TUNING,
        "pi_min": PI_MIN, "pi_max": PI_MAX,
        "entry_list_path": ENTRY_LIST_PATH, "results_path": RESULTS_PATH,
        "entry_list_server_url": ENTRY_LIST_URL,
        "results_post_url": RESULTS_POST_URL,
    }
    season = {
        "game_type": _enum("GameModeType_", GAME_MODE),
        "event": {"track": ev["track"], "layout": ev["layout"],
                  "event_name": ev["event_name"],
                  "track_length": str(ev["track_length"])},
        "export_json": EXPORT_JSON,
        "game_config": {
            "practice_duration": PRACTICE_DURATION,
            "practice_time_of_day": {"year": TOD_YEAR, "month": TOD_MONTH,
                                     "day": TOD_DAY,
                                     "hour": TOD_HOUR, "minute": TOD_MINUTE,
                                     "second": TOD_SECOND,
                                     # time_multiplier 0 freezes the clock, so
                                     # the session stays at TOD_HOUR forever.
                                     # With 1 it advances in real time and the
                                     # server drifts into night after a few hours.
                                     "time_multiplier": TIME_MULT},
            "practice_overtime_waiting_next_session": OVERTIME_WAIT,
            "practice_max_wait_to_box": MAX_WAIT_TO_BOX,
            # spelling is the exe's, not a typo of ours - see PENALTIES above
            "enable_custom_penalities": PENALTIES,
            "car_cut_tyres_out": CAR_CUT_TYRES_OUT,
            "warning_trigger_countdown": WARNING_TRIGGER_COUNTDOWN,
            "time_penalty_ms": TIME_PENALTY_MS,
        },
        "weather_type": _enum("GameModeSelectionWeatherType_", WEATHER),
        "weather_behaviour": _enum("GameModeSelectionWeatherBehaviour_",
                                   WEATHER_BEHAVIOUR),
        "initial_grip": _enum("InitialGrip_", GRIP),
    }

    args = [EXE,
            f"-serverconfig={encode_blob(server_cfg)}",
            f"-seasondefinition={encode_blob(season)}"]
    # Only ask for virtual-AI cars if some were actually requested. With no
    # bots the flag is noise, and it only means anything on the vAI-patched
    # binary - a normal build runs the stock server and never passes it.
    if int(N_AI or 0) > 0:
        args.append(f"-virtual_ai_cars={N_AI}")
    # NOTE: no -no_lobby here, so it registers and appears in the browser.

    # Skill SPREAD. Without it every vAI gets the same skill, so all cars run
    # the identical spline at identical pace and stack into one clump. The
    # flag's own description: "lowest AI skill ... (and anything in between
    # scales accordingly)" - i.e. min/max define a range the field is spread
    # across. Deliberately NOT setting ai_disable_variations, which "disables
    # the consistency skill" and would remove the very inconsistency that
    # separates them.
    if int(N_AI or 0) > 0:
        if SKILL_MIN:
            args.append(f"-simexpo_ai_skill_min={SKILL_MIN}")
        if SKILL_MAX:
            args.append(f"-simexpo_ai_skill_max={SKILL_MAX}")
    # NOTE: -no_lobby is normally OFF so the server registers and appears in
    # the in-game browser. Turning it on makes the server invisible there.
    if NO_LOBBY:
        args.append("-no_lobby=true")
    if WRITE_RESULTS:
        args.append("-write_server_results=true")
    for extra in filter(None, os.environ.get("EXTRA_FLAGS", "").split()):
        args.append(extra)

    os.makedirs(os.path.dirname(LOG), exist_ok=True)
    out = open(LOG, "w", encoding="utf-8", errors="replace")
    p = subprocess.Popen(args, cwd=SRV, stdout=out, stderr=subprocess.STDOUT)
    print(f"server started: pid {p.pid}")
    print(f"  name  : {NAME}")
    print(f"  track : {ev['track']} / {ev['layout']}")
    print(f"  ports : TCP/UDP {PORT}, HTTP {HTTP}")
    print(f"  vAI   : {N_AI}")
    print(f"  slots : {MAX_PLAYERS}")
    print(f"  mode  : {_enum('GameModeType_', GAME_MODE)} / "
          f"{_enum('MultiplayerServerListSessionType_', SESSION_TYPE)}")
    print(f"  wx    : {WEATHER} {WEATHER_BEHAVIOUR}, grip {GRIP}, "
          f"{TOD_HOUR:02d}:{TOD_MINUTE:02d} x{TIME_MULT}")
    if DRIVER_PASSWORD or ADMIN_PASSWORD:
        print("  locked: password set")
    if NO_LOBBY:
        print("  lobby : NOT registering (private)")
    if handicap:
        print(f"  handicaps: {len(handicap)} car(s)")
    print(f"  log   : {LOG}")
    print("stop with: taskkill /IM AssettoCorsaEVOServer.exe /F")

    # TELEMETRY=1 launches the tracker alongside the server. This is the only
    # way to get a clean baseline: it attaches the moment the process exists and
    # settles as soon as the AI grid is moving, so it is watching BEFORE the
    # first human joins - which is what --baseline-ai needs, and what a manual
    # start never wins the race for on a busy server.
    if os.environ.get("TELEMETRY") == "1":
        tele = os.path.join(SRV, "server_telemetry.py")
        args = [sys.executable, "-u", tele, "--wait", "120", "--baseline-ai"]
        log = open(os.path.join(SRV, "serverConfig", "telemetry.log"), "w")
        subprocess.Popen(args, stdout=log, stderr=subprocess.STDOUT, cwd=SRV)
        print("telemetry: started (serverConfig/telemetry.log, JSON on :8091)")


if __name__ == "__main__":
    sys.exit(main())
