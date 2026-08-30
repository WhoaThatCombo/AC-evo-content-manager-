"""A public server list, with the content each server requires.

The game cannot deliver content itself. A client missing a car or track is
simply rejected (`JoinErrorMessage_CONTENT_UNAVAILABLE`), and nothing in the
protocol offers a download - `MultiplayerServerListEntry` has 43 fields and not
one is a URL. So content distribution has to live beside the game, which is how
Content Manager solved the same problem for AC1.

This module is that side:

  * a registry of servers, each declaring the mods and tracks it requires
  * a manifest per server: every required file with its size and SHA-256
  * the files themselves, served over the same HTTP port as the UI

A player runs `acecm_sync.py <registry-url>`: it compares hashes, downloads only
what is missing, installs to the right folders, and then the server is joinable.

⚠ Hashing a 700 MB mod is slow, so hashes are cached by (path, size, mtime) and
only recomputed when the file actually changes.
"""
import hashlib
import json
import os
import time
import uuid

from . import config, install, logs

REGISTRY = os.path.join(config.DATA, "registry.json")
HASHCACHE = os.path.join(config.DATA, "hashes.json")

TEMPLATE = {
    "id": "",
    "name": "My EVO server",
    "description": "",
    "ip": "",                    # public address players should use
    "port": 9700,
    "profile_id": "",            # link to a local server profile, optional
    "required_mods": [],         # mod base names in the server's mod folder
    "required_tracks": [],       # track package folder names
    "public": True,
}


# ------------------------------------------------------------------ hashes --
# ⚠ Held in memory and flushed once. Re-reading and rewriting the whole cache
# file per digest is fine for a handful of car mods but quadratic for a track
# folder - 3600 files meant 3600 full rewrites of a growing JSON, which is what
# made the first manifest of a 1 GB track take long enough to time out.
_CACHE = None
_DIRTY = False


def _cache():
    global _CACHE
    if _CACHE is None:
        try:
            _CACHE = json.load(open(HASHCACHE, encoding="utf-8"))
        except Exception:
            _CACHE = {}
    return _CACHE


def flush_cache():
    global _DIRTY
    if _DIRTY and _CACHE is not None:
        try:
            json.dump(_CACHE, open(HASHCACHE, "w", encoding="utf-8"))
        except OSError:
            pass
        _DIRTY = False


def file_digest(path):
    """SHA-256, cached on (size, mtime_ns) so big content is hashed once.

    ⚠ NANOSECONDS, not int(st_mtime). Truncating to whole seconds means a file
    rewritten within the same second at the same size keeps its old hash - and
    that is not a corner case here: re-exporting a track file usually preserves
    its size, and this hash is exactly what tells a joining player their copy
    is out of date. A stale digest means the host advertises content it no
    longer has and nobody ever receives the update. Proven with a same-size
    rewrite: the digest did not change.
    """
    global _DIRTY
    try:
        st = os.stat(path)
    except OSError:
        return None
    key = os.path.abspath(path)
    c = _cache()
    hit = c.get(key)
    if hit and hit.get("size") == st.st_size \
            and hit.get("mtime_ns") == st.st_mtime_ns:
        return hit["sha256"]
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    digest = h.hexdigest()
    c[key] = {"size": st.st_size, "mtime_ns": st.st_mtime_ns,
              "sha256": digest}
    _DIRTY = True
    return digest


# ---------------------------------------------------------------- registry --
def load():
    try:
        return json.load(open(REGISTRY, encoding="utf-8"))
    except Exception:
        return []


def save(items):
    json.dump(items, open(REGISTRY, "w", encoding="utf-8"), indent=2)
    return items


def _profile_port(profile_id=""):
    """The game port a registry entry should advertise.

    A linked profile is the answer when there is one. Otherwise, if this
    machine runs exactly ONE server, that is unambiguously the server the
    content belongs to - which covers the common case of a host who added
    tracks and never touched the registry by hand.
    """
    try:
        from . import servers
        profs = servers.load()
    except Exception:
        return 0
    if profile_id:
        hit = next((p for p in profs if p.get("id") == profile_id), None)
        if hit:
            return int(hit.get("tcp_port") or 0)
    if len(profs) == 1:
        return int(profs[0].get("tcp_port") or 0)
    return 0


def autofill(entry):
    """Fill in the address and port an entry did not set for itself.

    ⚠ Entries used to be written with ip='' and port=9700 REGARDLESS - every
    track import added one, so a host with twenty tracks had twenty entries
    all claiming the same port and no address at all. The published join
    string came out as "join::9700", and anything trying to work out which
    server an entry belonged to had nothing to go on.

    Only ever fills what is blank: a host who typed an address or a port
    meant it, and this must not overwrite them on the next save.
    """
    if not (entry.get("ip") or "").strip():
        try:
            from . import netutil
            entry["ip"] = netutil.public_ipv4() or ""
        except Exception:
            pass
    # 9700 is the template default, so it is indistinguishable from "not set"
    if not entry.get("port") or int(entry.get("port") or 0) == 9700:
        port = _profile_port(entry.get("profile_id") or "")
        if port:
            entry["port"] = port
    return entry


def upsert(entry):
    entry = {**TEMPLATE, **entry}
    autofill(entry)
    if not entry.get("id"):
        # ⚠ int(time.time()) collides for anything created in the same
        # second - two profiles made together got the SAME id and one
        # silently overwrote the other. Add randomness.
        entry["id"] = f"srv{int(time.time())}{uuid.uuid4().hex[:4]}"
    items = [e for e in load() if e["id"] != entry["id"]]
    items.append(entry)
    save(items)
    return entry


def remove(sid):
    save([e for e in load() if e["id"] != sid])
    return {"ok": True}


def backfill():
    """Fill in address and port on entries written before autofill existed.

    Runs at startup. Entries already carrying both are left alone, so this
    settles after one pass and costs nothing on every later boot.
    """
    items = load()
    if not items:
        return {"ok": True, "fixed": 0}
    fixed = []
    for e in items:
        before = (e.get("ip") or "", int(e.get("port") or 0))
        autofill(e)
        if (e.get("ip") or "", int(e.get("port") or 0)) != before:
            fixed.append(e.get("id"))
    if fixed:
        save(items)
        logs.LOG.info("registry: filled in address/port on %d entr%s",
                      len(fixed), "y" if len(fixed) == 1 else "ies")
    return {"ok": True, "fixed": len(fixed)}


# ---------------------------------------------------------------- manifest --
def _mod_files(name):
    """Both halves of a car mod. A .kspkg without its .json installs fine and
    then never appears in the car list, so the manifest always carries both.

    ⚠ Look on BOTH sides. This checked only the dedicated server's mod folder,
    but a mod is just as likely to be installed client-side only - on this
    machine six mods are client-side and one is on the server - so sharing
    those found no files and published an empty manifest.
    """
    dirs = []
    for fn in (install.mods_dir, install.client_mods_dir):
        try:
            d = fn()
            if d and os.path.isdir(d) and d not in dirs:
                dirs.append(d)
        except Exception:
            continue
    out = []
    for ext in (".kspkg", ".json"):
        for d in dirs:
            p = os.path.join(d, name + ext)
            if os.path.isfile(p):
                out.append(("mods/" + name + ext, p))
                break            # one copy is enough; they are the same file
    return out


def _track_files(folder):
    """Everything in a track folder, relative paths preserved.

    ⚠ The IMPORTED folder comes first, and it is what a joining player actually
    needs. A deploy package holds a track's logic - the handful of scene files
    that go into the server archive - while a playable track is the ~1 GB of art
    the client keeps in Saved Games. Serving the package would hand someone
    eight files and leave the game unable to load the track.

    Paths are `tracks/<folder>/<rel>`, which installs straight to the client's
    tracks folder - see contentsync.destination.
    """
    roots = [os.path.join(os.path.expanduser("~"), "Saved Games", "ACE",
                          "mods", "content", "tracks", folder),
             os.path.join(config.DATA, "track_packages", folder),
             os.path.join(config.server_dir(), "track_packages", folder),
             os.path.join(os.path.expanduser("~"), "Downloads", folder)]
    out = []
    for root in roots:
        if not os.path.isdir(root):
            continue
        for base, _, files in os.walk(root):
            for f in files:
                p = os.path.join(base, f)
                rel = os.path.relpath(p, root).replace("\\", "/")
                out.append((f"tracks/{folder}/{rel}", p))
        break
    return out


def manifest(sid, base_url="", digests=True):
    """Every file a server requires.

    ⚠ `digests=False` for anything that just needs the size. A track folder is
    thousands of files and about a gigabyte, so hashing it is seconds at best
    and minutes cold - long enough that a caller listing several servers times
    out and the connection is dropped mid-response.
    """
    entry = next((e for e in load() if e["id"] == sid), None)
    if not entry:
        return {"ok": False, "error": "no such server"}
    files, missing = [], []
    for kind, names, finder in (("mod", entry.get("required_mods", []),
                                 _mod_files),
                                ("track", entry.get("required_tracks", []),
                                 _track_files)):
        for name in names:
            got = finder(name)
            if not got:
                missing.append(f"{kind} {name}")
            for rel, path in got:
                files.append({"path": rel, "size": os.path.getsize(path),
                              "sha256": file_digest(path) if digests else "",
                              "kind": kind,
                              "url": f"{base_url}/api/registry/file?id={sid}"
                                     f"&path={rel}"})
    if digests:
        flush_cache()
    return {"ok": True, "server": {k: entry[k] for k in
                                   ("id", "name", "description", "ip", "port")},
            "files": files,
            "total_bytes": sum(f["size"] for f in files),
            "missing_locally": missing}


def _track_sig(files):
    """Cheap 'did anything change' stamp: count + bytes + newest mtime."""
    n = total = latest = 0
    for _rel, p in files:
        st = os.stat(p)
        n += 1
        total += st.st_size
        latest = max(latest, int(st.st_mtime_ns))
    return f"{n}:{total}:{latest}"


_PACK_LOCKS = {}
_PACK_LOCKS_GUARD = None


def _pack_lock(folder):
    """One build at a time per track.

    ⚠ A joiner now pulls the pack as several parallel range requests, so six
    handlers can ask for the same track at once. Without this they each miss
    the cache and rebuild a gigabyte-sized tar simultaneously - six times the
    disk and CPU for one download, and the first bytes do not move until one
    of them wins.
    """
    global _PACK_LOCKS_GUARD
    import threading
    if _PACK_LOCKS_GUARD is None:
        _PACK_LOCKS_GUARD = threading.Lock()
    with _PACK_LOCKS_GUARD:
        lk = _PACK_LOCKS.get(folder)
        if lk is None:
            lk = _PACK_LOCKS[folder] = threading.Lock()
        return lk


def ensure_track_pack(folder):
    """One uncompressed tar of a track folder, rebuilt only when files change.

    3653 tiny files over a new TCP connection each cannot use a gigabit
    uplink. One 1 GB stream can. Cached under <data>/packs/.
    """
    folder = (folder or "").strip().replace("\\", "/").strip("/")
    if not folder or "/" in folder or folder in (".", ".."):
        return None
    with _pack_lock(folder):
        return _build_track_pack(folder)


def _build_track_pack(folder):
    files = _track_files(folder)
    if not files:
        return None
    d = os.path.join(config.DATA, "packs")
    os.makedirs(d, exist_ok=True)
    tar_path = os.path.join(d, folder + ".tar")
    sig_path = tar_path + ".sig"
    sha_path = tar_path + ".sha256"
    # v2: tar also carries acecm_track.json so the joiner can write tables.
    want = "v2:" + _track_sig(files)
    have = ""
    try:
        have = open(sig_path, encoding="ascii").read().strip()
    except OSError:
        pass
    if have == want and os.path.isfile(tar_path) and os.path.isfile(sha_path):
        return tar_path
    import tarfile
    import threading
    tmp = f"{tar_path}.part.{os.getpid()}.{threading.get_ident()}"
    prefix = f"tracks/{folder}/"
    with tarfile.open(tmp, "w") as tar:
        for rel, path in files:
            arc = rel[len(prefix):] if rel.startswith(prefix) else os.path.basename(rel)
            if not arc or arc.startswith("/") or ".." in arc.split("/"):
                continue
            tar.add(path, arcname=arc)
        try:
            from . import tracks as trackmod
            meta = trackmod.pack_meta(folder)
            blob = json.dumps(meta, indent=2).encode("utf-8")
            info = tarfile.TarInfo(name="acecm_track.json")
            info.size = len(blob)
            import io
            tar.addfile(info, io.BytesIO(blob))
        except Exception as ex:
            from . import logs
            logs.LOG.warning("track pack meta %s: %s", folder, ex)
    os.replace(tmp, tar_path)
    digest = file_digest(tar_path)
    open(sig_path, "w", encoding="ascii").write(want + "\n")
    open(sha_path, "w", encoding="ascii").write((digest or "") + "\n")
    flush_cache()
    return tar_path


def track_pack_sha(folder):
    p = os.path.join(config.DATA, "packs", folder + ".tar.sha256")
    try:
        return open(p, encoding="ascii").read().strip()
    except OSError:
        return ""


def resolve(sid, rel):
    """Map a manifest path back to a real file, refusing anything outside."""
    entry = next((e for e in load() if e["id"] == sid), None)
    if not entry:
        return None
    for mod in entry.get("required_mods", []):
        for r, p in _mod_files(mod):
            if r == rel:
                return p
    for trk in entry.get("required_tracks", []):
        for r, p in _track_files(trk):
            if r == rel:
                return p
    return None                      # not declared -> not served


_SIZES = {"key": None, "at": 0.0, "bytes": {}, "busy": False}
_SIZES_TTL = 300.0


def _public_sizes(entries, base_url):
    """content_bytes per entry id, cached.

    ⚠ This is what made "fetch content right after a server starts" fail for
    the first few tries. Sizing an entry means walking its whole track folder
    - Shutoku alone is 14,965 files - and that was redone on EVERY request,
    warm or cold, taking ~7.5s for this machine's 19 entries. discover() gives
    up after 4s, so the probe was dropped mid-response and the host looked
    like it was not running ACECM. Retrying only worked once the OS directory
    cache happened to bring one attempt under the timeout, which is exactly
    the "takes several attempts" people saw.

    Sizes only move when content is added or removed, so they do not need
    recomputing per request.
    """
    import threading
    import time as _time
    key = tuple(sorted(
        (e["id"], tuple(e.get("required_mods") or []),
         tuple(e.get("required_tracks") or [])) for e in entries))
    now = _time.monotonic()
    fresh = _SIZES["key"] == key and (now - _SIZES["at"]) < _SIZES_TTL
    if fresh:
        return _SIZES["bytes"]

    def compute():
        got = {}
        for e in entries:
            m = manifest(e["id"], base_url, digests=False)
            got[e["id"]] = m.get("total_bytes", 0) if m.get("ok") else 0
        return got

    # ⚠ Never recompute on the request path when something usable is already
    # cached. Blocking here is what the 4s probe timeout trips over, and a
    # plain TTL brought that back every time it expired - the request that
    # happened to land on the expiry paid 7.5s and was dropped, so fetching
    # worked most of the time and failed the rest. Answer with what we have
    # and refresh behind it; slightly stale sizes are cosmetic, a dropped
    # probe is not.
    if _SIZES["bytes"]:
        # ⚠ `busy` only stops a SECOND refresh being started. It must not send
        # this request off to compute synchronously instead - that reinstates
        # the very block being avoided, and measurably so: a probe arriving
        # while a refresh ran took 10.09s and was dropped, right after an
        # expired-cache probe had been served in 0.03s.
        if not _SIZES.get("busy"):
            _SIZES["busy"] = True

            def refresh():
                try:
                    _SIZES.update(key=key, at=_time.monotonic(),
                                  bytes=compute())
                except Exception as ex:
                    from . import logs
                    logs.LOG.info("refreshing share sizes: %s", ex)
                finally:
                    _SIZES["busy"] = False

            threading.Thread(target=refresh, daemon=True).start()
        return _SIZES["bytes"]

    got = compute()
    _SIZES.update(key=key, at=now, bytes=got)
    return got


def forget_public_sizes():
    """Drop the size cache - call when content on this host changes."""
    _SIZES.update(key=None, at=0.0, bytes={})


def warm_public_list(base_url=""):
    """Compute the sizes off the request path, so the first probe is fast."""
    try:
        public_list(base_url)
    except Exception as ex:
        from . import logs
        logs.LOG.info("warming the share list: %s", ex)


def public_list(base_url=""):
    """What a player's sync tool fetches first."""
    out = []
    entries = [e for e in load() if e.get("public", True)]
    sizes = _public_sizes(entries, base_url)
    for e in entries:
        # size only - hashing every server's content to draw a list is minutes
        m = {"ok": True, "total_bytes": sizes.get(e["id"], 0)}
        out.append({
            "id": e["id"], "name": e["name"], "description": e.get("description", ""),
            "ip": e.get("ip", ""), "port": e.get("port", 9700),
            "join": f"join:{e.get('ip','')}:{e.get('port',9700)}",
            "required_mods": e.get("required_mods", []),
            "required_tracks": e.get("required_tracks", []),
            "content_bytes": m.get("total_bytes", 0) if m.get("ok") else 0,
            "manifest": f"{base_url}/api/registry/manifest?id={e['id']}",
        })
    return {"servers": out, "generated": int(time.time())}
