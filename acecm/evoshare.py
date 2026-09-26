"""Fetch content from an EvoForge server's share.

Servers listed in the EvoForge directory publish their modded track from
their own box. The protocol was read out of EvoForge 1.0.13 and confirmed
live; it is unauthenticated and needs no EvoForge licence.

    GET <base>evoforge.json     manifest: {app,version,server,content[],
                                proto, active, waiting, max}
    GET <base>index/<file>      gzipped json index of the pack
    GET <base>content/<file>    the .kspkg itself, Range-resumable

The index is what makes this worth doing: entries carry absolute byte spans
into the pack, so we fetch only what the player is missing instead of pulling
multiple GB.

    {"v":1, "bytes":2197355359,
     "table":{"offset":2130246495,"size":67108864},
     "entries":[{"n":"content/tracks/X/...", "f":256, "s":337, "o":0, "h":""}]}

(`n` uses backslashes in the real payload.) Entry bytes are ordinary kspkg
payload: decrypt each one with `kspkg.decrypt(raw, 0)` - offset 0, the
keystream restarts per entry - and never for `.texturemips`, exactly as
`kspkg.read_entry` does. Verified end to end against a live share.

TRAPS, all three confirmed by probing:
  - The key is the `file` name, NEVER the `id`. `/content/<id>` answers
    "404 not shared".
  - Any other path is RELAYED to the dedicated server, so a bare
    `/<file>.kspkg` gives 502 and `/` gives the game's own status JSON.
    Neither means the share is down.
  - The payload is encrypted. Ranged bytes look like garbage until decrypted.

Be a good guest: the manifest carries `max` (concurrent download slots,
observed 3) with `active`/`waiting`. One stream at a time, and we identify as
ACECM rather than wearing EvoForge's user-agent.
"""
import gzip
import json
import os
import threading
import time
import urllib.error
import urllib.request

from . import contentsync
from . import kspkg
from . import logs
from .version import VERSION

UA = "ACECM/%s (+https://github.com/WhoaThatCombo/AC-evo-content-manager-)" % VERSION
# ⚠ Under the browser's 12 s api() abort. manifest/index are small requests
# on a UI path: a slow host must not stall Get content past the point the page
# gives up. The content STREAM passes its own, much longer timeout.
TIMEOUT = 8
# One ranged GET per contiguous run. Entries sit back to back, so a fresh
# install coalesces into a handful of streams rather than thousands of
# requests; a small gap is cheaper to download than to ask for separately.
GAP_TOLERANCE = 1 << 20          # 1 MB
CHUNK = 1 << 20

_state = {"active": False}
_lock = threading.Lock()


def _url(base, *parts):
    b = (base or "").strip()
    if not b.endswith("/"):
        b += "/"
    return b + "/".join(str(p).lstrip("/") for p in parts)


def _get(url, rng=None, timeout=TIMEOUT):
    h = {"User-Agent": UA, "Accept": "*/*"}
    if rng:
        h["Range"] = "bytes=%d-%d" % (rng[0], rng[1])
    return urllib.request.urlopen(
        urllib.request.Request(url, headers=h), timeout=timeout)


def manifest(base):
    """What this share publishes."""
    try:
        with _get(_url(base, "evoforge.json")) as r:
            d = json.loads(r.read().decode("utf-8", "replace"))
    except (urllib.error.URLError, OSError, ValueError) as ex:
        return {"ok": False, "error": str(ex)}
    if not isinstance(d, dict) or not d.get("content"):
        return {"ok": False, "error": "not an EvoForge share"}
    return {"ok": True, **d}


def index(base, fname):
    """The pack's entry table. Served gzipped."""
    try:
        with _get(_url(base, "index", fname)) as r:
            raw = r.read()
    except (urllib.error.URLError, OSError) as ex:
        return {"ok": False, "error": str(ex)}
    if raw[:2] == b"\x1f\x8b":
        try:
            raw = gzip.decompress(raw)
        except OSError as ex:
            return {"ok": False, "error": "bad gzip: %s" % ex}
    try:
        d = json.loads(raw.decode("utf-8", "replace"))
    except ValueError as ex:
        return {"ok": False, "error": "bad index: %s" % ex}
    return {"ok": True, **d}


def _rel(n):
    """Pack path -> the relative path contentsync.destination understands.

    Entries are `content\\tracks\\<folder>\\...`; destination() wants
    `tracks/<folder>/...`.
    """
    p = (n or "").replace("\\", "/").lstrip("/")
    if p.lower().startswith("content/"):
        p = p[len("content/"):]
    return p


def car_presets(idx):
    """The mechanical preset names inside a car pack.

    The mods-root .json descriptor needs these: a car whose .kspkg is present
    but whose .json is missing does not appear in the car list at all, which
    ACECM already calls out as "the failure mode that looks like a server
    bug". The names are readable straight off the index - one
    presets/<name>.mechanicalcarpreset per mechanical trim.
    """
    out = []
    for e in idx.get("entries") or []:
        n = (e.get("n") or "").replace("\\", "/")
        if n.lower().endswith(".mechanicalcarpreset"):
            out.append(n.rsplit("/", 1)[-1].rsplit(".", 1)[0])
    return sorted(set(out))


def plan_car(base, fname, name=""):
    """A car mod installs as a WHOLE pack, not as extracted files.

    ⚠ Do not run a car through the track path. Its entries are
    `content/cars/...` (backslashes in the real payload), which contentsync.destination() has no branch for -
    they would fall through and land flattened in the tracks folder - and a
    car carries .curve/.animation/.tyre/.wing/.design/.otf, none of which are
    in CONTENT_EXTS, so most of it would be silently skipped. Every car mod
    already on this machine is a `<name>.kspkg` + `<name>.json` pair in the
    mods root; match that.
    """
    from . import install
    d = install.client_mods_dir()
    if not d:
        return {"ok": False, "error": "no client mods folder"}
    dest = os.path.join(d, fname)
    base_name = fname[:-6] if fname.lower().endswith(".kspkg") else fname
    side = os.path.join(d, base_name + ".json")
    man = manifest(base)
    want = 0
    for c in (man.get("content") or []):
        if c.get("file") == fname:
            want = int(c.get("bytes") or 0)
    have = os.path.isfile(dest) and (not want or os.path.getsize(dest) == want)
    return {"ok": True, "kind": "car", "dest": dest, "side": side,
            "bytes": 0 if have else want, "have": bool(have),
            "have_side": os.path.isfile(side),
            "need": [] if (have and os.path.isfile(side))
                    else [{"rel": fname, "s": want}]}


def plan(base, fname, kind="track"):
    """What of this pack is missing here.

    "Missing" is judged by size, because the index's per-entry hash `h` is
    empty on every entry observed - so there is nothing better to compare
    against, and calling a size check a checksum would be a lie.
    """
    if (kind or "").lower() == "car":
        return plan_car(base, fname)
    idx = index(base, fname)
    if not idx.get("ok"):
        return idx
    need, have, skipped = [], 0, 0
    for e in idx.get("entries") or []:
        rel = _rel(e.get("n"))
        try:
            dest = contentsync.destination(rel)
        except ValueError:
            skipped += 1                      # not a content file type
            continue
        size = int(e.get("s") or 0)
        try:
            if os.path.isfile(dest) and os.path.getsize(dest) == size:
                have += 1
                continue
        except OSError:
            pass
        need.append({"n": e.get("n"), "rel": rel, "dest": dest,
                     "o": int(e.get("o") or 0), "s": size})
    return {"ok": True, "total": len(idx.get("entries") or []),
            "have": have, "skipped": skipped, "need": need,
            "bytes": sum(x["s"] for x in need),
            "pack_bytes": int(idx.get("bytes") or 0)}


def _runs(need):
    """Coalesce entries into contiguous byte runs, in offset order."""
    items = sorted(need, key=lambda x: x["o"])
    runs = []
    for it in items:
        if runs and it["o"] - runs[-1]["end"] <= GAP_TOLERANCE:
            runs[-1]["end"] = max(runs[-1]["end"], it["o"] + it["s"])
            runs[-1]["items"].append(it)
        else:
            runs.append({"start": it["o"], "end": it["o"] + it["s"],
                         "items": [it]})
    return runs


def status():
    with _lock:
        return {"ok": True, **_state}


def _set(**kw):
    with _lock:
        _state.update(kw)


def start(base, fname, folder="", kind="track"):
    """Download what is missing, in a background thread."""
    with _lock:
        if _state.get("active"):
            return {"ok": False, "error": "a download is already running"}
        _state.clear()
        # `job` lets a watcher tell THIS download's end from the last one's
        job = time.time()
        _state.update(active=True, phase="planning", done=0, total=0,
                      bytes=0, want=0, error="", server=base, file=fname,
                      kind=kind, job=job)
    threading.Thread(target=_run, args=(base, fname, folder, kind),
                     daemon=True).start()
    return {"ok": True, "started": True, "job": job}


def cancel():
    _set(cancel=True)
    return {"ok": True}


def _run(base, fname, folder, kind="track"):
    try:
        if (kind or "").lower() == "car":
            return _run_car(base, fname)
        p = plan(base, fname)
        if not p.get("ok"):
            _set(active=False, phase="error",
                 error=p.get("error") or "no plan")
            return
        need = p["need"]
        if not need:
            _set(active=False, phase="done", done=0, total=0,
                 note="you already have everything this share offers")
            return
        _set(phase="downloading", total=len(need), want=p["bytes"])
        got = 0
        url = _url(base, "content", fname)
        for run in _runs(need):
            if _state.get("cancel"):
                _set(active=False, phase="cancelled")
                return
            with _get(url, rng=(run["start"], run["end"] - 1),
                      timeout=60) as r:
                buf = bytearray()
                base_off = run["start"]
                # stream the run, slicing entries out as their bytes arrive
                pending = sorted(run["items"], key=lambda x: x["o"])
                while pending:
                    if _state.get("cancel"):
                        _set(active=False, phase="cancelled")
                        return
                    want_to = pending[0]["o"] + pending[0]["s"] - base_off
                    while len(buf) < want_to:
                        chunk = r.read(CHUNK)
                        if not chunk:
                            break
                        buf += chunk
                    if len(buf) < want_to:
                        raise OSError("the share closed the stream early")
                    it = pending.pop(0)
                    at = it["o"] - base_off
                    _write(it, bytes(buf[at:at + it["s"]]))
                    got += it["s"]
                    _set(done=_state.get("done", 0) + 1, bytes=got)
        # ⚠ A track on disk is not yet a track the game can load - it has to
        # be in the client's tables, and that edit CANNOT happen while the
        # game is running because content.kspkg is locked. Report what
        # actually happened: claiming "added to the track list" when the
        # registration was refused is how someone ends up staring at a 2 GB
        # download the game will not show them.
        folder = folder or _folder_of(need)
        reg = {"ok": True, "skipped": True}
        if folder:
            reg = contentsync._register_downloaded_track(folder) or {}
        _set(active=False, phase="done", folder=folder,
             registered=bool(reg.get("ok")),
             needs_close=bool(reg.get("needs_close")),
             note=("" if reg.get("ok") else
                   (reg.get("error") or "could not add it to the track list")))
    except Exception as ex:                      # noqa: BLE001
        logs.LOG.warning("evoshare download failed: %s", ex)
        _set(active=False, phase="error", error=str(ex))


def _folder_of(need):
    for it in need:
        parts = it["rel"].split("/")
        if len(parts) >= 2 and parts[0] == "tracks":
            return parts[1]
    return ""


def _write(it, raw):
    """Decrypt exactly as kspkg.read_entry does, then place the file."""
    name = (it["n"] or "").lower()
    data = raw if name.endswith(".texturemips") else bytes(
        kspkg.decrypt(bytearray(raw), 0))
    dest = it["dest"]
    os.makedirs(os.path.dirname(dest), exist_ok=True)
    tmp = dest + ".part"
    with open(tmp, "wb") as f:
        f.write(data)
    os.replace(tmp, dest)


def _run_car(base, fname):
    """Fetch a whole car pack and write the descriptor beside it."""
    from . import install
    p = plan_car(base, fname)
    if not p.get("ok"):
        _set(active=False, phase="error", error=p.get("error") or "no plan")
        return
    dest, side = p["dest"], p["side"]
    want = int(p.get("bytes") or 0)
    if p.get("have") and p.get("have_side"):
        _set(active=False, phase="done", total=0, done=0,
             note="you already have this car")
        return
    _set(phase="downloading", total=1, want=want)
    if not p.get("have"):
        os.makedirs(os.path.dirname(dest), exist_ok=True)
        tmp = dest + ".part"
        # resume a part-finished file rather than starting the GB again
        at = os.path.getsize(tmp) if os.path.isfile(tmp) else 0
        url = _url(base, "content", fname)
        rng = (at, want - 1) if (at and want and at < want) else None
        with _get(url, rng=rng, timeout=60) as r,                 open(tmp, "ab" if at else "wb") as f:
            got = at
            while True:
                if _state.get("cancel"):
                    _set(active=False, phase="cancelled")
                    return
                chunk = r.read(CHUNK)
                if not chunk:
                    break
                f.write(chunk)
                got += len(chunk)
                _set(bytes=got)
        os.replace(tmp, dest)
        _set(done=1)
    # ⚠ The .json is what makes the car APPEAR. A pack without it installs
    # silently and then simply is not in the car list, which reads as a
    # broken download. Build it from the pack's own mechanical presets.
    wrote_side = False
    if not os.path.isfile(side):
        idx = index(base, fname)
        presets = car_presets(idx) if idx.get("ok") else []
        label = ""
        for c in (manifest(base).get("content") or []):
            if c.get("file") == fname:
                label = c.get("name") or c.get("id") or ""
        if presets:
            # ⚠ Match the shape the game's own descriptors use, fields and
            # all - install.py only reads three of them, but a descriptor
            # that is a subset of the real thing is a gamble taken for no
            # reason. performance_indicator is honestly 0: it is not in the
            # pack anywhere, and inventing a rating would be worse than
            # admitting we do not know it.
            json.dump({"cars": [{"name": n,
                                 "display_name": label or n,
                                 "performance_indicator": 0,
                                 "property_1": 0,
                                 "property_2": 0,
                                 "property_3": 0}
                                for n in presets]},
                      open(side, "w", encoding="utf-8"), indent=2)
            wrote_side = True
        else:
            logs.LOG.warning("no mechanicalcarpreset in %s - cannot write the "
                             "descriptor, the car will not be listed", fname)
    _set(active=False, phase="done", kind="car", dest=dest,
         wrote_descriptor=wrote_side,
         note="" if (wrote_side or os.path.isfile(side))
              else "downloaded, but no descriptor could be built - the car "
                   "will not appear in the car list")
