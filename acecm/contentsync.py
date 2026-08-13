"""The browser side of content delivery: what a server needs, what you lack.

Delivery already exists - `registry.py` publishes a per-server manifest of every
required file with its SHA-256, and serves the bytes over the same HTTP port.
What was missing is everything BEFORE the download:

  * you had to know a host's ACECM address and type it into acecm_sync.py
  * nothing compared a server against what you already have
  * so "can I join this?" was answered by trying, and being rejected with
    CONTENT_UNAVAILABLE

This module answers it in advance. Point it at a host and it discovers the
ACECM beside the game server, lists what that server requires, and reports only
what THIS machine is missing - so the UI can offer one button instead of a
scavenger hunt.

⚠ The dedicated server cannot be the source. Its content.kspkg holds a track's
LOGIC only - seven scene files, a few MB - while a playable track is the ~1 GB
of art in the player's own mods folder. Those bytes exist only on the host's
game install, which is why delivery is an ACECM-to-ACECM affair and why a stock
Kunos server can never supply content. When no ACECM answers, say so plainly
rather than implying a download exists.
"""
import json
import os
import urllib.error
import urllib.parse
import urllib.request

from . import config, install, logs, registry

# ACECM picks a free port, so a host's address alone is not enough to find it.
# These are the ones it actually lands on in practice, cheapest first.
PORTS = (8092, 8093, 8730, 8080, 8091, 8094, 8095)
TIMEOUT = 4


def tracks_dir():
    """Where the client keeps imported tracks - EvoForge writes here too."""
    cfg = (config.CFG.get("client_tracks_dir") or "").strip()
    return cfg or os.path.join(os.path.expanduser("~"), "Saved Games", "ACE",
                               "mods", "content", "tracks")


def installed_tracks():
    d = tracks_dir()
    return sorted(n for n in os.listdir(d)) if os.path.isdir(d) else []


def installed_cars():
    try:
        got = install.installed("client")
    except Exception as ex:
        logs.LOG.info("installed cars: %s", ex)
        return []
    return sorted(m["name"] for m in got.get("mods", []) if m.get("name"))


def track_map(refresh=False):
    """Display name -> folder, straight out of the CLIENT's own tracks.table.

    ⭐ This is the authoritative answer to "do I have the track this server is
    running?", and it needs nothing from the user or the host. The lobby names
    a track the way players see it ("Highlands Drift"); the content lives in a
    folder named nothing like it (highlands_kp). Matching those by hand, or by
    guessing at the name, misfires the moment the two diverge.

    The client's archive already holds the mapping for every track it can load
    - and EvoForge registers imports there too, so a track someone downloaded
    yesterday appears here with no help from us:

        Barber Motorsports Park -> barber_mo_ra
        Highlands Drift         -> highlands_kp

    So: in the table means loadable, absent means genuinely missing.

    ⚠ Cached on the archive's size+mtime. It is a 25 GB file and the table sits
    at the end of it, so re-reading per browser row would be unusable.
    """
    from . import viewer
    pkg = viewer.package()
    if not pkg:
        return {}
    cache = os.path.join(config.DATA, "track_map.json")
    try:
        st = os.stat(pkg)
        stamp = f"{st.st_size}:{int(st.st_mtime)}"
    except OSError:
        return {}
    # ⚠ Also stamp the imported-tracks folder. The map itself comes from the
    # archive, but a track deleted by hand leaves the archive untouched - and
    # then we would keep listing a track whose files are gone.
    try:
        d = tracks_dir()
        stamp += f"|{int(os.stat(d).st_mtime)}:{len(os.listdir(d))}"
    except OSError:
        pass
    if not refresh:
        try:
            got = json.load(open(cache, encoding="utf-8"))
            if got.get("stamp") == stamp:
                return got["tracks"]
        except Exception:
            pass

    from . import kspkg
    from .tracktables import walk
    out = {}
    try:
        blob = None
        with open(pkg, "rb") as f:
            for p, s, o in kspkg.iter_entries(pkg):
                if p.lower() == "system\\tracks.table":
                    blob = kspkg.read_entry(f, s, o, p)
                    break
        if blob:
            top = list(walk(blob))
            for _f, _w, v in walk(top[0][2]):
                name = folder = None
                for g, gw, gv in walk(v):
                    if g != 2 or gw != 2:
                        continue
                    for h, hw, hv in walk(gv):
                        if hw != 2:
                            continue
                        if h == 1:
                            name = hv.decode("utf-8", "replace")
                        elif h == 3:
                            folder = hv.decode("utf-8", "replace").rsplit(
                                "\\", 1)[-1]
                if name and folder:
                    out[name] = folder
    except Exception as ex:
        logs.LOG.warning("track map: %s", ex)
        return {}
    try:
        json.dump({"stamp": stamp, "tracks": out},
                  open(cache, "w", encoding="utf-8"))
    except OSError:
        pass
    return out


def local(refresh=False):
    return {"tracks": installed_tracks(), "cars": installed_cars(),
            "track_map": track_map(refresh), "tracks_dir": tracks_dir()}


def _get(url, timeout=TIMEOUT):
    req = urllib.request.Request(url, headers={"User-Agent": "ACECM"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read() or b"{}")


def discover(host, port=None):
    """Find the ACECM sharing content beside a game server.

    Returns its base URL and the servers it publishes, or an explanation. A
    host that simply is not running ACECM is the normal case, not an error.
    """
    host = (host or "").strip()
    if not host:
        return {"ok": False, "error": "no host given"}
    tried = []
    for p in ([int(port)] if port else PORTS):
        base = f"http://{host}:{p}"
        tried.append(p)
        try:
            got = _get(f"{base}/api/registry/list")
        except (urllib.error.URLError, OSError, ValueError):
            continue
        servers = got.get("servers") if isinstance(got, dict) else got
        if servers is None:
            continue
        return {"ok": True, "base": base, "port": p,
                "servers": servers}
    return {"ok": False, "tried": tried,
            "error": f"no ACECM answering on {host} - the host is not sharing "
                     f"content, so anything you are missing has to come from "
                     f"them another way"}


def plan(base, server_id):
    """What joining that server would cost THIS machine.

    Compares the host's manifest against local files by SHA-256, so content you
    already have - however you got it - is never downloaded again.
    """
    try:
        # ⚠ generous: the host hashes every file the first time it publishes a
        # track, and a cold gigabyte is not a few seconds. Later calls hit its
        # cache and return promptly.
        man = _get(f"{base}/api/registry/manifest?id={server_id}", timeout=600)
    except Exception as ex:
        return {"ok": False, "error": f"manifest unreadable: {ex}"}
    if not man.get("ok"):
        return {"ok": False, "error": man.get("error") or "no manifest"}

    need, have = [], []
    for f in man.get("files", []):
        dest = destination(f["path"])
        if os.path.isfile(dest) and os.path.getsize(dest) == f.get("size") \
                and (not f.get("sha256")
                     or registry.file_digest(dest) == f["sha256"]):
            have.append(f["path"])
        else:
            need.append({**f, "dest": dest})
    return {"ok": True, "server": man.get("server", {}),
            "need": need, "have": len(have),
            "bytes": sum(f.get("size", 0) for f in need),
            "missing_on_host": man.get("missing_locally") or []}


def destination(rel):
    """Where a manifest path belongs here.

    ⚠ Must match acecm_sync.destination exactly - the two install the same
    content, and a track landing anywhere but the game's tracks folder is
    invisible to it.
    """
    rel = rel.replace("\\", "/")
    if rel.startswith("mods/"):
        return os.path.join(install.client_mods_dir(), os.path.basename(rel))
    if rel.startswith("tracks/"):
        return os.path.join(tracks_dir(), *rel[len("tracks/"):].split("/"))
    return os.path.join(tracks_dir(), os.path.basename(rel))


def fetch(entry, progress=None):
    """Download one manifest entry, verify it, then move it into place.

    ⚠ Write to .part and rename only after the hash matches. A truncated file
    left at the real path looks installed, and the failure surfaces later as a
    missing-content rejection or a crash on load.
    """
    dest = entry.get("dest") or destination(entry["path"])
    os.makedirs(os.path.dirname(dest), exist_ok=True)
    tmp = dest + ".part"
    size = entry.get("size") or 0
    done = 0
    req = urllib.request.Request(entry["url"], headers={"User-Agent": "ACECM"})
    with urllib.request.urlopen(req, timeout=60) as r, open(tmp, "wb") as fh:
        while True:
            chunk = r.read(1 << 20)
            if not chunk:
                break
            fh.write(chunk)
            done += len(chunk)
            if progress:
                progress(done, size)
    got = registry.file_digest(tmp)
    if entry.get("sha256") and got != entry["sha256"]:
        os.remove(tmp)
        raise ValueError(f"checksum mismatch for {entry['path']} "
                         f"(got {got[:12]}, expected {entry['sha256'][:12]})")
    os.replace(tmp, dest)
    return dest
