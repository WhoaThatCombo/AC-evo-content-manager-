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
import urllib.error
import urllib.request

from . import contentsync
from . import kspkg
from . import logs
from .version import VERSION

UA = "ACECM/%s (+https://github.com/WhoaThatCombo/AC-evo-content-manager-)" % VERSION
TIMEOUT = 20
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


def plan(base, fname):
    """What of this pack is missing here.

    "Missing" is judged by size, because the index's per-entry hash `h` is
    empty on every entry observed - so there is nothing better to compare
    against, and calling a size check a checksum would be a lie.
    """
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


def start(base, fname, folder=""):
    """Download the missing entries in a background thread."""
    with _lock:
        if _state.get("active"):
            return {"ok": False, "error": "a download is already running"}
        _state.clear()
        _state.update(active=True, phase="planning", done=0, total=0,
                      bytes=0, want=0, error="", server=base, file=fname)
    threading.Thread(target=_run, args=(base, fname, folder),
                     daemon=True).start()
    return {"ok": True, "started": True}


def cancel():
    _set(cancel=True)
    return {"ok": True}


def _run(base, fname, folder):
    try:
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
