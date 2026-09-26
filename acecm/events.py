"""One live snapshot of everything the UI shows as "in progress".

The page used to poll a dozen endpoints on its own timers - Drive status,
download progress, compression, server state - each page arming and disarming
its own. A slow endpoint or a timer that never re-armed looked like random
flakiness (see acecm-ui-endpoints-must-not-block, acecm-drive-poll-dead).

Now there is one snapshot, streamed to the page over /api/events and readable
once from /api/events/snapshot. Every page subscribes to the same feed.

⚠ BUILT BY ONE BACKGROUND THREAD, READ WITHOUT WAITING. The first version
computed the snapshot inside the request under a lock. While the game was
launching one of the probes stalled for 14 s, every request queued behind the
lock, and the Drive checklist sat on "nothing happening" for the whole launch -
the exact failure this module exists to remove. Now a stuck probe only makes
the snapshot stale (see `age`), it is logged by name, and nothing waits on it.
"""
import os
import threading
import time

from . import backend, compress, drive, evoshare, lobby, logs, servers, thumbs

FAST = 0.8      # drive + progress: what the Join checklist and bar need
SLOW = 3.0      # servers + proxy: they change on a human timescale
WARN = 2.0      # a probe slower than this is logged, by name

_state = {"snap": None, "slow_at": 0.0, "servers": [], "proxy": {}}
_start_lock = threading.Lock()
_thread = None
_wake = threading.Event()


def _servers():
    out = []
    for p in servers.load():
        try:
            st = servers.status(p)
        except Exception:                          # noqa: BLE001
            st = {}
        out.append({"id": p.get("id"), "name": p.get("name") or "",
                    "running": bool(st.get("running")),
                    "tcp_port": int(p.get("tcp_port") or 0),
                    "clients": st.get("clients")})
    return out


def _proxy():
    """Is the lobby proxy up, and is it serving OUR servers?"""
    js = backend.join_state()
    if not js.get("control"):
        return {"up": False, "ours": False}
    theirs = js.get("lobby") or ""
    ours = bool(theirs) and (os.path.normcase(os.path.abspath(theirs))
                             == os.path.normcase(os.path.abspath(lobby.PATH)))
    return {"up": True, "ours": ours,
            "client": bool(js.get("client_connected"))}


def _timed(name, fn, fallback):
    t = time.perf_counter()
    try:
        return fn()
    except Exception as ex:                        # noqa: BLE001
        logs.LOG.warning("events %s: %s", name, ex)
        return fallback
    finally:
        took = time.perf_counter() - t
        if took > WARN:
            logs.LOG.warning("events: %s took %.1fs", name, took)


def _build(progress):
    now = time.time()
    if now - _state["slow_at"] >= SLOW:
        _state["servers"] = _timed("servers", _servers, _state["servers"])
        _state["proxy"] = _timed("proxy", _proxy, _state["proxy"])
        _state["slow_at"] = time.time()
    d = _timed("drive", drive.status, {"ok": True, "phase": "idle"})
    d = dict(d)
    for k in ("pick", "wrote", "launch", "single_player"):
        d.pop(k, None)
    snap = {
        "ok": True,
        "at": time.time(),
        "drive": d,
        "progress": _timed("progress", progress, {"active": False}),
        "compress": _timed("compress", compress.status, {}),
        "evoshare": _timed("evoshare", evoshare.status, {}),
        # background picture jobs (car renders, track covers)
        "thumbs": _timed("thumbs", thumbs.job, {}),
        "covers": _timed("covers", thumbs.cover_job, {}),
        "servers": _state["servers"],
        "proxy": _state["proxy"],
    }
    _state["snap"] = snap
    return snap


def _loop(progress):
    while True:
        try:
            _build(progress)
        except Exception as ex:                    # noqa: BLE001
            logs.LOG.warning("events build: %s", ex)
        # woken early by kick() after something starts
        _wake.wait(FAST)
        _wake.clear()


def start(progress):
    """Start the builder thread once. `progress` is app._progress."""
    global _thread
    with _start_lock:
        if _thread is None:
            _thread = threading.Thread(target=_loop, args=(progress,),
                                       name="events", daemon=True)
            _thread.start()


def kick():
    """Rebuild now rather than at the next tick."""
    _wake.set()


def snapshot(progress, fresh=False):
    """The latest snapshot - never waits on a probe.

    The very first call (before the thread has produced anything) builds one
    inline so the page is not handed an empty state.
    """
    start(progress)
    snap = _state["snap"]
    if snap is None or fresh:
        snap = _build(progress)
    out = dict(snap)
    # how old it is, so a stalled builder is visible rather than silent
    out["age"] = round(time.time() - snap["at"], 1)
    d = dict(out.get("drive") or {})
    d["now"] = time.time()
    out["drive"] = d
    return out
