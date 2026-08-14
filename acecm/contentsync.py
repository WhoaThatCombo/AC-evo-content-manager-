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


def loadable_cars():
    """Every car id this CLIENT can actually drive.

    Two sources, because there are two kinds of car:
      * Kunos presets, which live in the base archive - carsmap reads them
      * mod presets, which each mod's own .json declares

    ⚠ Ids, not names. A server advertises `preset_apex_ind_h_mech_1`, and
    matching that against a display name like "APEX INDYCAR INDY" is the same
    identity-vs-label trap as tracks.
    """
    out = set()
    try:
        from . import carsmap
        out |= set((carsmap.table().get("presets") or {}).keys())
    except Exception as ex:
        logs.LOG.info("carsmap for loadable cars: %s", ex)
    try:
        for m in install.installed("client").get("mods") or []:
            for c in m.get("cars") or []:
                if c.get("id"):
                    out.add(c["id"])
    except Exception as ex:
        logs.LOG.info("client mods for loadable cars: %s", ex)
    return sorted(out)


def local(refresh=False):
    return {"tracks": installed_tracks(), "cars": installed_cars(),
            "car_ids": loadable_cars(),
            "track_map": track_map(refresh), "tracks_dir": tracks_dir()}


def _get(url, timeout=TIMEOUT):
    req = urllib.request.Request(url, headers={"User-Agent": "ACECM"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read() or b"{}")


def parse_target(raw):
    """Turn a pasted share link, host:port, or bare IP into (host, port, base)."""
    raw = (raw or "").strip()
    if not raw:
        return "", None, ""
    if "://" in raw:
        u = urllib.parse.urlparse(raw)
        host = (u.hostname or "").strip("[]")
        port = u.port
        if host:
            scheme = u.scheme or "http"
            netloc = u.netloc or (f"{host}:{port}" if port else host)
            return host, port, f"{scheme}://{netloc}"
        return "", None, ""
    if raw.count(":") == 1:
        left, right = raw.rsplit(":", 1)
        if right.isdigit() and left:
            return left, int(right), ""
    return raw, None, ""


def share_info():
    """What a host should send a joining player."""
    from . import netutil
    port = int(config.CFG.get("ui_port") or 8092)
    lan = netutil.lan_ipv4() or ""
    pub = []
    for e in registry.load():
        if not e.get("public", True):
            continue
        pub.append({
            "id": e.get("id"), "name": e.get("name"),
            "tracks": e.get("required_tracks") or [],
            "mods": e.get("required_mods") or [],
        })
    return {
        "ok": True,
        "port": port,
        "listen": (config.CFG.get("listen") or "0.0.0.0"),
        "lan_ip": lan,
        "lan_url": f"http://{lan}:{port}" if lan else "",
        "localhost_url": f"http://127.0.0.1:{port}",
        "published": pub,
        "hint": (f"Players paste the URL into Server browser → Fetch from "
                 f"ACECM. They need TCP {port} (this app) and the game's "
                 f"TCP+UDP port (usually 9700). Same LAN: firewall on "
                 f"{port}. Different network: forward both."),
    }


def discover(host, port=None, ports=None, timeout=TIMEOUT):
    """Find the ACECM sharing content beside a game server.

    Accepts a bare IP, host:port, or a full http:// URL (the share link
    a host copies from Content). A host that simply is not running ACECM
    is the normal case, not an error.
    """
    host, parsed_port, given_base = parse_target(host)
    if port:
        try:
            parsed_port = int(port)
        except (TypeError, ValueError):
            pass
    if not host:
        return {"ok": False, "error": "no host given"}

    # ⚠ Asking OUR OWN server has to go over loopback. The browser row carries
    # the public address, and probing your own public IP from inside the same
    # network usually never comes back (no NAT hairpinning) - so a host that is
    # very much sharing content reports as not sharing, and the obvious
    # conclusion is that your own sharing is broken.
    try:
        from . import netutil
        if host in set(netutil.our_ips() or []):
            host = "127.0.0.1"
            if given_base:
                given_base = ""
    except Exception as ex:
        logs.LOG.debug("own-ip check: %s", ex)

    if given_base:
        try:
            got = _get(f"{given_base}/api/registry/list", timeout)
            servers = got.get("servers") if isinstance(got, dict) else got
            if servers is not None:
                return {"ok": True, "base": given_base,
                        "port": parsed_port, "servers": servers}
        except (urllib.error.URLError, OSError, ValueError) as ex:
            return {"ok": False, "tried": [parsed_port],
                    "error": f"no ACECM at {given_base} ({ex}). "
                             "The host must be running ACECM and TCP "
                             f"{parsed_port or 8092} must be reachable."}

    tried = []
    probe = [parsed_port] if parsed_port else list(ports or PORTS)
    for p in probe:
        if not p:
            continue
        base = f"http://{host}:{int(p)}"
        tried.append(int(p))
        try:
            got = _get(f"{base}/api/registry/list", timeout)
        except (urllib.error.URLError, OSError, ValueError):
            continue
        servers = got.get("servers") if isinstance(got, dict) else got
        if servers is None:
            continue
        return {"ok": True, "base": base, "port": int(p),
                "servers": servers}
    return {"ok": False, "tried": tried,
            "error": f"no ACECM answering on {host} "
                     f"(tried TCP {', '.join(str(p) for p in tried) or 'none'}). "
                     "Content is NOT the game port: the host must run ACECM "
                     "and allow inbound TCP 8092 (Windows Firewall + router "
                     "port-forward if you are not on the same LAN). "
                     "Game join still needs TCP+UDP 9700."}


def scan(servers, limit=24, workers=6):
    """Ask a set of hosts whether they can supply content, in parallel.

    Point this at the servers running something you do not have, and it comes
    back with the ones you can actually fix - all without the game running, so
    the content is in place before it next starts.

    ⚠ Only servers you are MISSING content for, capped, and only the two ports
    ACECM really uses. Probing every host in a 786-server list on seven ports
    would be a port scan of other people's machines, which is not what asking
    "can I join this?" should mean.
    """
    import concurrent.futures as cf

    seen, targets = set(), []
    for s in servers:
        ip = (s.get("server_ip") or "").strip()
        if not ip or ip in seen:
            continue
        seen.add(ip)
        targets.append(s)
        if len(targets) >= limit:
            break

    def probe(s):
        d = discover(s.get("server_ip"), None,
                     ports=(8092, 8093), timeout=1.5)
        if not d.get("ok"):
            return None
        return {"server": s.get("server_name"), "ip": s.get("server_ip"),
                "port": s.get("server_tcp_port"), "base": d["base"],
                "entries": [{"id": e.get("id"), "name": e.get("name"),
                             "tracks": e.get("required_tracks") or [],
                             "mods": e.get("required_mods") or [],
                             "bytes": e.get("content_bytes") or 0}
                            for e in (d.get("servers") or [])]}

    out = []
    with cf.ThreadPoolExecutor(max_workers=workers) as pool:
        for r in pool.map(probe, targets):
            if r and r["entries"]:
                out.append(r)
    return {"ok": True, "probed": len(targets), "hosts": out}


def needs(servers):
    """The servers in a list running a track this machine cannot load."""
    known = track_map()
    out = []
    for s in servers:
        t = (s.get("track") or "").strip()
        if t and t not in known:
            out.append(s)
    return out


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


def _entry_sid(entry):
    q = urllib.parse.parse_qs(urllib.parse.urlparse(entry.get("url") or "").query)
    return (q.get("id") or [""])[0]


def _entry_base(entry):
    u = urllib.parse.urlparse(entry.get("url") or "")
    if not u.scheme or not u.netloc:
        return ""
    return f"{u.scheme}://{u.netloc}"


def fetch_track_pack(base, sid, folder, files, progress=None):
    """Pull one tar and unpack it. Falls back to the caller on 404."""
    import tarfile
    url = f"{base}/api/registry/pack?id={urllib.parse.quote(sid)}" \
          f"&track={urllib.parse.quote(folder)}"
    dest_dir = os.path.join(tracks_dir(), folder)
    os.makedirs(dest_dir, exist_ok=True)
    tmp = dest_dir.rstrip("\\/") + ".tar.part"
    done = 0
    req = urllib.request.Request(url, headers={"User-Agent": "ACECM"})
    try:
        r = urllib.request.urlopen(req, timeout=600)
    except urllib.error.HTTPError as ex:
        if ex.code == 404:
            raise FileNotFoundError("host has no pack endpoint") from ex
        raise
    try:
        with r, open(tmp, "wb") as fh:
            want = (r.headers.get("X-ACECM-SHA256") or "").strip()
            while True:
                chunk = r.read(1 << 20)
                if not chunk:
                    break
                fh.write(chunk)
                done += len(chunk)
                if progress:
                    progress(done, int(r.headers.get("Content-Length") or 0))
    except Exception:
        try:
            os.remove(tmp)
        except OSError:
            pass
        raise
    got = registry.file_digest(tmp)
    if want and got != want:
        os.remove(tmp)
        raise ValueError(f"pack checksum mismatch for {folder}")
    with tarfile.open(tmp, "r:") as tar:
        tar.extractall(dest_dir, filter="data")
    try:
        os.remove(tmp)
    except OSError:
        pass
    if files:
        missing = [f["path"] for f in files
                   if not os.path.isfile(f.get("dest") or destination(f["path"]))]
        if missing:
            raise ValueError(f"pack for {folder} missed {len(missing)} file(s)")
    return dest_dir


def install_files(need, status):
    """Install a plan: one tar per track, then leftover files in parallel.

    A track is thousands of tiny files. One stream uses a gigabit uplink;
    one HTTP GET per file cannot.
    """
    import threading
    from collections import defaultdict
    from concurrent.futures import ThreadPoolExecutor, as_completed

    tracks = defaultdict(list)
    rest = []
    for e in need:
        rel = (e.get("path") or "").replace("\\", "/")
        parts = rel.split("/")
        if len(parts) >= 3 and parts[0] == "tracks" and parts[1]:
            tracks[parts[1]].append(e)
        else:
            rest.append(e)

    moved = 0
    installed = []

    def set_done(n):
        status["done"] = n

    for folder, files in tracks.items():
        sid = _entry_sid(files[0])
        base = _entry_base(files[0])
        packed = False
        if sid and base and len(files) >= 8:
            status["detail"] = f"track pack {folder} ({len(files)} files)"
            try:
                def tick(done, _size, base_n=moved):
                    set_done(base_n + done)
                fetch_track_pack(base, sid, folder, files, tick)
                moved += sum(f.get("size", 0) for f in files)
                set_done(moved)
                installed.extend(f["path"] for f in files)
                packed = True
            except FileNotFoundError:
                packed = False
            except Exception as ex:
                logs.LOG.warning("track pack %s failed, per-file fallback: %s",
                                 folder, ex)
                packed = False
        if not packed:
            rest.extend(files)

    lock = threading.Lock()
    if rest:
        status["detail"] = f"{len(rest)} leftover file(s)"

        def one(entry):
            contentsync_fetch = fetch
            contentsync_fetch(entry)
            return entry

        workers = 8 if len(rest) > 1 else 1
        with ThreadPoolExecutor(max_workers=workers) as pool:
            futs = [pool.submit(one, e) for e in rest]
            for i, fut in enumerate(as_completed(futs), 1):
                entry = fut.result()
                with lock:
                    moved += entry.get("size", 0)
                    set_done(moved)
                    installed.append(entry["path"])
                    status["detail"] = f"{i}/{len(rest)} {entry['path']}"

    status["files"] = installed
    return installed


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
    with urllib.request.urlopen(req, timeout=600) as r, open(tmp, "wb") as fh:
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
