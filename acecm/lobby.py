"""Advertisement the local backend puts in the multiplayer list.

The dedicated server is started from a *profile*. The lobby process is a
different process and used to invent Nordschleife + a stale cars.json. This
file is the handshake: ACECM writes it when a server starts, the backend
re-reads it on every list/entry request so a track change does not need a
backend restart.
"""
import json
import os

from . import config, content, install
from . import netutil

# GameModeType as the client enum numbers it. PRACTICE is the only value we
# have confirmed from a live Register/list exchange (the browser prints
# GameModeType_NONE for 0). Unknown modes stay at PRACTICE rather than 0,
# which greys the row out.
GAME_MODE_TYPE = {
    "NONE": 0,
    "PRACTICE": 10,
}

PATH = os.path.join(config.DATA, "lobby.json")


def _mode_type(name):
    key = (name or "PRACTICE").strip().upper()
    key = key.replace("GAMEMODETYPE_", "")
    return GAME_MODE_TYPE.get(key, GAME_MODE_TYPE["PRACTICE"])


def _cars_for(profile):
    chosen = [c for c in (profile.get("cars") or []) if c]
    if chosen:
        return chosen
    # Same union ACECM already passes as CARS_OVERRIDE when launching:
    # every Kunos preset plus every installed mod id. An empty list here
    # is what greys out DRIVE.
    try:
        kunos = [c["id"] for c in content.cars()["cars"] if c.get("kunos")]
    except Exception:
        kunos = []
    try:
        mods = list(install.car_names())
    except Exception:
        mods = []
    # preserve order, drop dupes
    seen, out = set(), []
    for c in kunos + mods:
        if c and c not in seen:
            seen.add(c)
            out.append(c)
    return out


def _event(profile):
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


def read():
    try:
        return json.load(open(PATH, encoding="utf-8"))
    except Exception:
        return {}
