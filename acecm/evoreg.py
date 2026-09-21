"""The EvoForge server directory - a second, live source of EVO servers.

`backend.server_list()` can only show what the PROXY saw, and the proxy only
sees the list when a game client asks for it: Kunos will not answer us
directly, because the first frame of that session is a Steam auth ticket bound
to a real client. So with the game closed our list is a snapshot.

This directory has neither problem. It is a plain unauthenticated JSON
endpoint - no EvoForge licence, no game session, no proxy - so it works with
the game closed, which is exactly when you want to be fetching content.

    https://registry.evoforge.fr/?a=list          (their primary)
    https://join.evoforge.fr/registry/?a=list     (their documented fallback)

What makes it worth having is not the COUNT - it is usually a handful - but
the KIND: these are modded and open-world servers, and each one advertises the
track it runs plus its own HTTP share to fetch that track from.

⚠ The content is NOT hosted by EvoForge. Each server carries
`share:{port,url}` pointing at the operator's own box, and `content[]` of
`{kind,id,name,file,bytes,sha,fp}`. Downloading it in order to join is what
the operator published it for; rehosting someone's mod is not ours to grant.

⚠ Written from the endpoint's observed SHAPE, not from EvoForge's code -
their app is proprietary and ACECM is GPL. We also identify ourselves
honestly rather than sending their `EvoForge registry` user-agent.
"""
import json
import os
import threading
import time
import urllib.error
import urllib.request

from . import config
from . import logs
from .version import VERSION

BASES = (
    "https://registry.evoforge.fr/",
    "https://join.evoforge.fr/registry/",
)
# They cache `list` for 20 s in their own client; match it rather than
# hammering a small third-party service on every page draw.
TTL = 20.0
# ⚠ Short, and it matters. This runs behind the Drive server list, and the
# browser's api() helper aborts at 12 s - so two bases at 8 s each could burn
# 16 s and the list simply never rendered. Nothing here is worth a UI stall.
TIMEOUT = 3.0
# A host that is down stays down for a bit; retrying it every 20 s just pays
# the timeout over and over.
FAIL_BACKOFF = 300.0
UA = "ACECM/%s (+https://github.com/WhoaThatCombo/AC-evo-content-manager-)" % VERSION

_mem = {"at": 0.0, "servers": [], "base": "", "failed_at": 0.0}
_refreshing = threading.Lock()


def _cache_path():
    return os.path.join(config.DATA, "evoforge_registry.json")


def _get(base, action="list"):
    url = base + "?a=" + action
    req = urllib.request.Request(url, headers={
        "User-Agent": UA,
        "Accept": "application/json",
    })
    with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
        return json.loads(r.read().decode("utf-8", "replace"))


def _int(v):
    try:
        return int(v)
    except (TypeError, ValueError):
        return 0


def _norm(rec):
    """Their record -> the field names the rest of ACECM already speaks.

    Kept deliberately close to what `backend.server_list()` yields so
    drive._public_briefs can merge the two without a second code path.
    """
    ip = str(rec.get("ip") or "").strip()
    port = int(rec.get("tcpPort") or 0)
    track = rec.get("track") or {}
    share = rec.get("share") or {}
    live = rec.get("live") if isinstance(rec.get("live"), dict) else {}
    info = rec.get("info") if isinstance(rec.get("info"), dict) else {}
    return {
        # identity: the directory has no server_id, and ip:port is what
        # actually identifies a server anyway (see acecm-multi-server-wrong-join)
        "server_id": str(rec.get("key") or ("%s:%s" % (ip, port))),
        "server_name": rec.get("name") or "(unnamed)",
        "server_ip": ip,
        "server_tcp_port": port,
        "server_udp_port": port,
        "track": track.get("id") or track.get("name") or "",
        "layout": track.get("layout") or "",
        # ⚠ live.players is the LIST of drivers, not a count - rendering it
        # straight put "[object Object]" in the player column. The number is
        # live.count, and the slot count is info.pits; there is no maxPlayers.
        "players": _int(live.get("count")),
        "max_players": _int(info.get("pits")),
        "game_mode": (info.get("mode") or "").upper(),
        # extras the game's own list never carries
        "source": "evoforge",
        "track_origin": track.get("origin") or "",
        "share_url": share.get("url") or "",
        "content": rec.get("content") or [],
        "country": rec.get("country") or "",
        "page": rec.get("page") or "",
    }


def fetch(force=False, block=True):
    """The directory, memory-cached for TTL and disk-cached across runs.

    ⚠ `block=False` is what any UI path must use. This talks to a third-party
    host, and a request the browser gives up on at 12 s cannot be allowed to
    depend on someone else's uptime: with the host unreachable the two bases
    took 16 s between them and the Drive server list never rendered at all.
    Non-blocking hands back whatever we already have - possibly nothing - and
    refreshes in the background, so the list is at worst one poll out of date
    instead of missing.
    """
    now = time.time()
    if not force and (now - _mem["at"]) < TTL and _mem["servers"]:
        return {"ok": True, "servers": list(_mem["servers"]),
                "cached": True, "base": _mem["base"]}
    if not block:
        if now - _mem.get("failed_at", 0.0) > FAIL_BACKOFF:
            _kick()
        return {"ok": True, "servers": list(_mem["servers"]),
                "cached": True, "stale": True, "base": _mem["base"]}
    last = ""
    for base in BASES:
        try:
            d = _get(base)
        except (urllib.error.URLError, OSError, ValueError) as ex:
            last = str(ex)
            continue
        if not isinstance(d, dict) or not d.get("ok"):
            last = "registry said no"
            continue
        servers = [_norm(s) for s in (d.get("servers") or [])
                   if s and s.get("ip") and s.get("tcpPort")]
        _mem.update(at=now, servers=servers, base=base)
        try:
            json.dump({"at": now, "servers": servers},
                      open(_cache_path(), "w", encoding="utf-8"))
        except OSError:
            pass
        return {"ok": True, "servers": list(servers), "cached": False,
                "base": base}
    # offline: a stale directory still tells you which servers exist
    try:
        d = json.load(open(_cache_path(), encoding="utf-8"))
        return {"ok": True, "servers": d.get("servers") or [], "cached": True,
                "stale": True, "error": last}
    except (OSError, ValueError):
        pass
    _mem["failed_at"] = time.time()
    logs.LOG.info("evoforge registry unavailable: %s", last)
    return {"ok": False, "servers": [], "error": last or "unavailable"}


def content_for(ip, tcp):
    """What that server publishes, and where to get it."""
    for s in fetch().get("servers") or []:
        if s["server_ip"] == str(ip) and int(s["server_tcp_port"]) == int(tcp):
            return {"ok": True, "share_url": s["share_url"],
                    "content": s["content"], "name": s["server_name"]}
    return {"ok": False, "error": "not in the EvoForge directory"}


def _kick():
    """Refresh in the background. One at a time, never blocking a caller."""
    if not _refreshing.acquire(blocking=False):
        return
    def go():
        try:
            fetch(force=True, block=True)
        except Exception:                          # noqa: BLE001
            pass
        finally:
            # defensive: a module reload can swap the lock out from under a
            # thread that is still running, and a refresh must never take the
            # process down over its own bookkeeping
            try:
                _refreshing.release()
            except RuntimeError:
                pass
    threading.Thread(target=go, daemon=True).start()
