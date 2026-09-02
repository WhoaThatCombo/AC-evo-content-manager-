"""Advertisement the local backend puts in the multiplayer list.

The dedicated server is started from a *profile*. The lobby process is a
different process and used to invent Nordschleife + a stale cars.json. This
file is the handshake: ACECM writes it when a server starts, the backend
re-reads it on every list/entry request so a track change does not need a
backend restart.
"""
import json
import os

from . import config, content, logs
from . import netutil

# GameModeType as the client enum numbers it. PRACTICE is the only value we
# have confirmed from a live Register/list exchange (the browser prints
# GameModeType_NONE for 0). Unknown modes stay at PRACTICE rather than 0,
# which greys the row out.
# Numbers from ClientCommandsUtil.GameModeType. Anything unknown stays
# PRACTICE (10) rather than 0, which greys the browser row out.
GAME_MODE_TYPE = {
    "NONE": 0,
    "RACE_WEEKEND": 1,
    "SRO_RACE": 2,
    "INSTANT_RACE": 3,
    "SUPERPOLE": 4,
    "FREEROAM": 5,
    "DRIFT": 6,
    "RALLY": 7,
    "HOTSTINT": 8,
    "HOTLAP": 9,
    "PRACTICE": 10,
    "TEST_DRIVE": 11,
    "A_TO_B": 12,
}

PATH = os.path.join(config.DATA, "lobby.json")


def _mode_type(name):
    key = (name or "PRACTICE").strip().upper()
    key = key.replace("GAMEMODETYPE_", "")
    return GAME_MODE_TYPE.get(key, GAME_MODE_TYPE["PRACTICE"])


def _cars_for(profile):
    from . import servers as srv
    return srv.allowed_car_ids(profile)


def _event(profile):
    # ⚠ A deployed custom track wins. The advertisement is built from the
    # profile, and reading track_index alone made a Miami server advertise
    # "Watkins Glen International / Short Inner Loop" - the stock event whose
    # index the profile still carried. Players browse on this.
    custom = (profile.get("custom_track") or "").strip()
    if custom:
        layout = ""
        try:
            from . import contentsync, tracks
            folder = (contentsync.track_map() or {}).get(custom, "")
            if folder:
                src = os.path.join(contentsync.tracks_dir(), folder)
                layout = tracks.read_track_folder(src)["layout"]
        except Exception as ex:
            logs.LOG.info("layout for %s: %s", custom, ex)
        return {"track": custom, "layout": layout,
                "event_name": f"{layout} Race" if layout else custom,
                "track_index": int(profile.get("track_index") or 0)}

    idx = int(profile.get("track_index") or 0)
    try:
        tracks = content.tracks().get("tracks") or []
    except Exception:
        tracks = []
    ev = next((t for t in tracks if t.get("index") == idx), None)
    if not ev and 0 <= idx < len(tracks):
        ev = tracks[idx]
    ev = ev or {}
    label = (profile.get("track_label") or "").strip()
    return {
        "track": ev.get("track") or "",
        "layout": ev.get("layout") or "",
        "event_name": label or ev.get("name") or ev.get("event_name") or "",
        "track_index": idx,
    }


def from_profile(profile):
    profile = profile or {}
    ev = _event(profile)
    hour = int(profile.get("tod_hour") or 13)
    minute = int(profile.get("tod_minute") or 0)
    lan = netutil.lan_ipv4()
    return {
        "server_id": profile.get("id") or "local-0000-0000-0000-000000000001",
        "server_name": profile.get("name") or "ACECM server",
        "tcp_port": int(profile.get("tcp_port") or 9700),
        "udp_port": int(profile.get("tcp_port") or 9700),
        "http_port": int(profile.get("http_port") or 8080),
        "max_players": int(profile.get("max_players") or 90),
        "time_of_day": f"{hour:02d}:{minute:02d}",
        "game_mode": profile.get("game_mode") or "PRACTICE",
        "game_mode_type": _mode_type(profile.get("game_mode")),
        "cars": _cars_for(profile),
        "lan_ip": lan,
        "loopback": "127.0.0.1",
        **ev,
    }


def write(profile):
    os.makedirs(config.DATA, exist_ok=True)
    blob = from_profile(profile)
    json.dump(blob, open(PATH, "w", encoding="utf-8"), indent=2)
    return blob


def refresh():
    """Write the corrected advertisement back to lobby.json.

    ⚠ This is the part that actually matters. The backend does NOT call into
    ACECM - it re-reads this FILE on every list request - so correcting the
    blob only in read() fixed what ACECM displays and left the game being told
    the old thing. Whatever is true has to end up on disk.

    Returns the blob written, or {} when nothing changed.
    """
    live = running_config()
    if not live:
        return {}
    try:
        cur = json.load(open(PATH, encoding="utf-8"))
    except Exception:
        cur = {}
    merged = {**cur, **live}
    if merged == cur:
        return {}
    os.makedirs(config.DATA, exist_ok=True)
    json.dump(merged, open(PATH, "w", encoding="utf-8"), indent=2)
    logs.LOG.info("lobby.json corrected from the live server: %s / %s",
                  merged.get("server_name"), merged.get("track"))
    return merged


def read():
    """The advertisement, corrected against the server that is actually up.

    ⚠ The stored blob is written from a PROFILE at start time, so it is only a
    statement of intent. It goes wrong whenever the two drift: a server started
    outside ACECM, a profile edited afterwards, or a custom track deployed
    under a borrowed slot - which is how the list came to advertise
    "Road Atlanta / GP" for a server running Highlands. The client trusts this
    and shows it to everyone browsing, so it has to describe reality.
    """
    try:
        blob = json.load(open(PATH, encoding="utf-8"))
    except Exception:
        blob = {}
    live = running_config()
    if live:
        blob.update(live)
    return blob


def running_config():
    """What the live dedicated server was ACTUALLY started with.

    Its own HTTP port reports only `clients`, `version` and `protocol` - no
    name and no track. But the server is launched with its whole configuration
    and season encoded on the command line, so the truth is readable from the
    running process itself, whoever started it.
    """
    import base64
    import struct
    import zlib

    cmd = _server_cmdline()
    if not cmd:
        return {}

    def blob(flag):
        i = cmd.find(flag)
        if i < 0:
            return {}
        raw = cmd[i + len(flag):].split(" ")[0].strip().strip('"')
        try:
            data = base64.b64decode(raw)
            # 4-byte big-endian length, then zlib
            return json.loads(zlib.decompress(data[4:]).decode("utf-8"))
        except Exception as ex:
            logs.LOG.info("could not decode %s: %s", flag, ex)
            return {}

    cfg, season = blob("-serverconfig="), blob("-seasondefinition=")
    ev = (season.get("event") or {}) if isinstance(season, dict) else {}
    out = {}
    if cfg.get("server_name"):
        out["server_name"] = cfg["server_name"]
    if cfg.get("max_players"):
        out["max_players"] = int(cfg["max_players"])
    for k in ("server_tcp_listener_port", "server_udp_listener_port",
              "server_http_port"):
        if cfg.get(k):
            out[{"server_tcp_listener_port": "tcp_port",
                 "server_udp_listener_port": "udp_port",
                 "server_http_port": "http_port"}[k]] = int(cfg[k])
    if ev.get("track"):
        out["track"] = ev["track"]
    if ev.get("layout"):
        out["layout"] = ev["layout"]
    if ev.get("event_name"):
        out["event_name"] = ev["event_name"]
    cars = [c.get("car_name") for c in (cfg.get("allowed_cars_list_full") or [])
            if isinstance(c, dict) and c.get("car_name")]
    if cars:
        out["cars"] = cars
    if out:
        out["live"] = True
    return out


def _server_cmdline():
    """The command line of the running dedicated server, or ''."""
    try:
        from . import winproc
        # ⚠ Match BOTH exe names. The launcher can run the stock binary
        # (AssettoCorsaEVOServer.stock.exe), and filtering on the plain name
        # alone reported "nothing running" while a server was plainly up -
        # so the advertisement silently kept using stale profile values.
        for pid in winproc.pids_named_prefix("assettocorsaevoserver"):
            cmd = winproc.cmdline(pid)
            if cmd:
                return cmd.strip()
        return ""
    except Exception as ex:
        logs.LOG.info("could not read the server command line: %s", ex)
        return ""
