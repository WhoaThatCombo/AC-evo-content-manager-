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

from . import config, install

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
    """SHA-256, cached on (size, mtime) so big content is hashed once."""
    global _DIRTY
    try:
        st = os.stat(path)
    except OSError:
        return None
    key = os.path.abspath(path)
    c = _cache()
    hit = c.get(key)
    if hit and hit.get("size") == st.st_size and hit.get("mtime") == int(st.st_mtime):
        return hit["sha256"]
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    digest = h.hexdigest()
    c[key] = {"size": st.st_size, "mtime": int(st.st_mtime), "sha256": digest}
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


def upsert(entry):
    entry = {**TEMPLATE, **entry}
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


def ensure_track_pack(folder):
    """One uncompressed tar of a track folder, rebuilt only when files change.

    3653 tiny files over a new TCP connection each cannot use a gigabit
    uplink. One 1 GB stream can. Cached under <data>/packs/.
    """
    folder = (folder or "").strip().replace("\\", "/").strip("/")
    if not folder or "/" in folder or folder in (".", ".."):
        return None
    files = _track_files(folder)
    if not files:
        return None
    d = os.path.join(config.DATA, "packs")
    os.makedirs(d, exist_ok=True)
    tar_path = os.path.join(d, folder + ".tar")
    sig_path = tar_path + ".sig"
    sha_path = tar_path + ".sha256"
    want = _track_sig(files)
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


def public_list(base_url=""):
    """What a player's sync tool fetches first."""
    out = []
    for e in load():
        if not e.get("public", True):
            continue
        # size only - hashing every server's content to draw a list is minutes
        m = manifest(e["id"], base_url, digests=False)
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
