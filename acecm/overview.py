"""What is happening right now, and what is wrong.

The dashboard used to count things that exist - profiles, cars, layouts. All
true, none of it answers the questions someone actually opens ACECM with: is my
server up, is anyone on it, and why did that not work? So this gathers live
state and, more importantly, a list of problems phrased as something to DO.

⚠ Everything here must be cheap and must not raise. It runs on every dashboard
load, so each probe is wrapped: a broken mod folder or an unreachable server
should cost that one line, not the whole page.
"""
import os
import socket

from . import backend, config, detect, install, registry, servers


def _port_busy(port):
    """Is something already listening here?

    ⚠ Worth knowing BEFORE launching. A dedicated server started on an occupied
    port retries in a tight loop, and the retry storm has taken this machine
    down entirely - 120 GB of virtual address space and a hard freeze. Naming
    the conflict up front is the cheapest possible fix.
    """
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    try:
        s.settimeout(0.15)
        return s.connect_ex(("127.0.0.1", int(port))) == 0
    except Exception:
        return False
    finally:
        s.close()


_EVENTS = None


def _events():
    """The event catalogue a profile's track_index points into."""
    global _EVENTS
    if _EVENTS is None:
        import json
        _EVENTS = []
        for name in ("events_practice.json", "events_race_weekend.json"):
            try:
                d = json.load(open(os.path.join(config.server_dir(), name),
                                   encoding="utf-8"))
                _EVENTS = d.get("events") or []
                break
            except Exception:
                continue
    return _EVENTS


def _servers():
    out = []
    for p in servers.load():
        try:
            st = servers.status(p)
        except Exception:
            st = {"running": False, "pid": None, "clients": None}
        # ⚠ A profile stores an INDEX into events_*.json, not a track name.
        # Precedence, most specific first: a deployed custom track is what the
        # server will actually host; track_label is the truthful name when a
        # stock slot has been borrowed; otherwise the stock event.
        track = (p.get("custom_track") or p.get("track_label") or "").strip()
        layout = ""
        if not track:
            try:
                ev = _events()[int(p.get("track_index") or 0)]
                track, layout = ev.get("track", ""), ev.get("layout", "")
            except Exception:
                track = ""
        out.append({
            "id": p.get("id"), "name": p.get("name") or "(unnamed)",
            "track": track,
            "layout": layout,
            "port": p.get("tcp_port") or p.get("port") or 9700,
            "http_port": p.get("http_port") or 8080,
            "running": bool(st.get("running")),
            "pid": st.get("pid"),
            "clients": st.get("clients"),
        })
    return out


def attention(srv, be):
    """Problems worth acting on, most urgent first.

    Each item says what is wrong AND what to do - "TLS missing" alone sends
    someone hunting through Settings.
    """
    items = []
    exe = config.server_exe()
    if not exe or not os.path.exists(exe):
        found = detect.server_candidates()
        items.append({
            "level": "bad", "what": "Dedicated server executable not found",
            "do": (f"Found in that folder: {', '.join(found)} - pick one in "
                   f"Settings" if found else
                   "Install the dedicated server, or set server_dir in Settings"),
        })
    if not os.path.isdir(config.tools_dir()):
        items.append({"level": "warn", "what": "Backend tools folder missing",
                      "do": "Set tools_dir in Settings"})
    if not be.get("have_cert"):
        items.append({"level": "warn", "what": "No TLS certificate",
                      "do": "The client will not connect to our own lobby "
                            "without one - generate it on the Backend page"})

    # ⚠ Two profiles on one port cannot be told apart. Whichever is running
    # makes BOTH read as running, and stopping one by port could kill the
    # other - so this is an identity problem, not just a "they can't run
    # together" problem.
    try:
        for port, names in servers.port_clashes().items():
            items.append({
                "level": "warn",
                "what": f"{len(names)} profiles share port {port}: "
                        f"{', '.join(names)}",
                "do": "Give each its own port - while they share one, ACECM "
                      "cannot tell which is running, and stopping one may "
                      "stop the other",
            })
    except Exception:
        pass

    # a port conflict for a server that is NOT ours to claim
    for s in srv:
        if not s["running"] and _port_busy(s["port"]):
            items.append({
                "level": "bad",
                "what": f"Port {s['port']} is already in use, and "
                        f"“{s['name']}” wants it",
                "do": "Stop whatever holds it, or change this profile's port. "
                      "Starting anyway can lock the whole machine up",
            })

    try:
        broken = [r for r in install.audit().get("rows", [])
                  if r.get("problem")]
        if broken:
            items.append({
                "level": "warn",
                "what": f"{len(broken)} car mod(s) are not installed cleanly",
                "do": "See Content - a half-installed mod loads for you and "
                      "not for joiners",
            })
    except Exception:
        pass

    # hosting something nobody can download
    try:
        published = {t for e in registry.load()
                     for t in (e.get("required_tracks") or [])}
        from . import contentsync
        known = set(contentsync.track_map().values())
        hosting = {s["track"] for s in srv if s["running"] and s["track"]}
        if hosting and not published and known:
            items.append({
                "level": "warn",
                "what": "A server is running but no content is published",
                "do": "Players missing the track cannot download it from you - "
                      "deploy it from Content to publish automatically",
            })
        if published:
            items.append({
                "level": "info",
                "what": "Remote players fetch content from this ACECM, not "
                        "from the game port",
                "do": "They need TCP 8092 (this window) plus the game's "
                      "TCP+UDP port (usually 9700). Forward both if they "
                      "are not on your LAN, and allow 8092 in Windows Firewall.",
            })
    except Exception:
        pass
    return items


def overview():
    try:
        be = backend.state()
    except Exception:
        be = {}
    srv = _servers()
    running = [s for s in srv if s["running"]]
    players = sum(s["clients"] or 0 for s in running)
    try:
        from . import contentsync
        tracks = len(contentsync.track_map())
        cars = len(contentsync.installed_cars())
    except Exception:
        tracks = cars = 0
    try:
        shared = sum(len(e.get("required_tracks") or []) +
                     len(e.get("required_mods") or []) for e in registry.load())
    except Exception:
        shared = 0
    return {
        "servers": srv,
        "running": len(running),
        "players": players,
        "tracks": tracks,
        "cars": cars,
        "shared": shared,
        "backend": be,
        "attention": attention(srv, be),
        "server_dir": config.server_dir(),
    }
