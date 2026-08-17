"""Pictures for the car and track lists.

Two different problems, two different answers:

CARS get a real Vulkan render. evoview already loads a car straight out of the
archive and can write a PNG headlessly, and the full sweep proved it on all 74
cars, so the list shows the actual car - paint, rims, tyres and all - rather
than a name in a table. Renders are cached on disk and only ever made once.

TRACKS use the cover art the game already ships. `uiresources\\images\\tracks\\`
holds a handful of plain PNGs (imola_01.png, cota_2.png ...). Not every track
has one, and there is nothing to invent when it does not - those simply show
blank. The per-layout `.texture` files in the same folder are outline maps in
the engine's own compressed format; decoding those is a separate job, so they
are left alone for now.

⚠ Rendering is serialised deliberately. Each car costs a GPU context and ~2 s,
and firing 74 at once would fight the viewer the user may have open.
"""
import os
import re
import subprocess
import sys
import threading
import time

from . import config, kspkg, logs, viewer

CACHE = os.path.join(config.DATA, "thumbs")
CARS = os.path.join(CACHE, "cars")
TRACKS = os.path.join(CACHE, "tracks")
SIZE = "480x320"
# ⚠ The detail pane shows one car much larger than a list row does, and a
# 480px render blown up to fill it is visibly soft - it reads as a low-quality
# app rather than a low-resolution file. Big renders are made ON DEMAND for the
# car you actually opened, never for all 85, because each one costs an evoview
# launch.
BIG = "1440x960"
_LOCK = threading.Lock()
_JOB = {"state": "idle", "done": 0, "total": 0, "current": "", "made": 0}
# tracks decode separately from cars: different work, different button
_COVER_JOB = {"state": "idle", "done": 0, "total": 0, "current": "", "made": 0}


def _dirs():
    for d in (CARS, TRACKS):
        os.makedirs(d, exist_ok=True)


def car_path(car_id, big=False):
    _dirs()
    safe = re.sub(r"[^A-Za-z0-9_.-]", "_", car_id)
    return os.path.join(CARS, safe + ("@big.png" if big else ".png"))


def track_path(folder):
    _dirs()
    return os.path.join(TRACKS, re.sub(r"[^A-Za-z0-9_.-]", "_", folder) + ".png")


# ------------------------------------------------------------------- cars --

def render_car(car_id, force=False, timeout=180, make=True, big=False):
    """One car, rendered by evoview into the cache. Returns the path or None.

    make=False (the list GET) never launches evoview. Typing in Drive used
    to rebuild every row and fire this on each key, which opened a console
    per car and stole focus after one character.
    """
    if not car_id:
        return None
    out = car_path(car_id, big=big)
    if os.path.isfile(out) and not force:
        return out
    if not make:
        return None
    exe = viewer.viewer_exe()
    if not exe:
        return None
    pkg = viewer._package_for(car_id)
    if not pkg:
        return None
    cmd = [exe, pkg, "--car", car_id, "--shot", out,
           "--size", BIG if big else SIZE,
           "--yaw", "2.35", "--pitch", "0.17"]
    base = viewer.package()
    # a mod package holds only its own car; the base game supplies shared tyres
    if base and os.path.abspath(base) != os.path.abspath(pkg):
        cmd += ["--base", base]
    from . import winproc
    try:
        r = winproc.hidden_run(cmd, capture_output=True, text=True,
                               timeout=timeout, cwd=os.path.dirname(exe))
    except subprocess.TimeoutExpired:
        logs.LOG.warning("thumb %s: timed out", car_id)
        return None
    if r.returncode != 0 or not os.path.isfile(out):
        logs.LOG.warning("thumb %s: exit %s %s", car_id, r.returncode,
                         (r.stdout or r.stderr or "")[-200:])
        return None
    return out


def build_all(force=False):
    """Render every car in the background; poll job() for progress."""
    with _LOCK:
        if _JOB["state"] == "running":
            return {"ok": False, "error": "already building"}
        _JOB.update({"state": "running", "done": 0, "total": 0,
                     "current": "", "made": 0})

    def run():
        cars = viewer.index().get("cars", [])
        _JOB["total"] = len(cars)
        t0 = time.time()
        for i, c in enumerate(cars, 1):
            _JOB["done"], _JOB["current"] = i, c.get("label") or c["id"]
            try:
                if render_car(c["id"], force):
                    _JOB["made"] += 1
            except Exception as ex:
                logs.LOG.warning("thumb %s: %s", c["id"], ex)
        _JOB.update({"state": "done", "current": "",
                     "seconds": round(time.time() - t0, 1)})

    threading.Thread(target=run, daemon=True).start()
    return {"ok": True}


def job():
    return dict(_JOB)


def have():
    _dirs()
    return {os.path.splitext(f)[0] for f in os.listdir(CARS)
            if f.endswith(".png")}


# ----------------------------------------------------------------- tracks --

_COVERS = None


def _cover_index():
    """Track folder -> archive path of a shipped PNG cover, if any.

    ⚠ The file names do not match folder names exactly (`brands_hatch_indy.png`
    for brands_hatch, `cota_2.png` for cota), so match on the longest folder
    name the file name starts with. Anything unmatched simply has no cover.
    """
    global _COVERS
    if _COVERS is not None:
        return _COVERS
    _COVERS = {}
    pkg = viewer.package()
    if not pkg:
        return _COVERS
    from . import contentsync
    folders = sorted(set(contentsync.track_map().values()), key=len,
                     reverse=True)
    try:
        for p, s, _o in kspkg.iter_entries(pkg):
            low = p.lower()
            if not s or "\\images\\tracks\\" not in low or not low.endswith(".png"):
                continue
            stem = low.rsplit("\\", 1)[-1][:-len(".png")]
            for f in folders:
                if stem.startswith(f.lower()):
                    _COVERS.setdefault(f, p)
                    break
    except Exception as ex:
        logs.LOG.warning("cover index: %s", ex)
    return _COVERS


def track_cover(folder):
    """Path to a cached cover PNG, or None when the game ships none."""
    out = track_path(folder)
    if os.path.isfile(out):
        return out
    entry = _cover_index().get(folder)
    if entry:
        pkg = viewer.package()
        try:
            with open(pkg, "rb") as f:
                for p, s, o in kspkg.iter_entries(pkg):
                    if p == entry:
                        open(out, "wb").write(kspkg.read_entry(f, s, o, p))
                        return out
        except Exception as ex:
            logs.LOG.warning("cover %s: %s", folder, ex)
    return _decode_cover(folder)


# ⚠ Only FOUR tracks ship a .png. The rest ship the same photo as a BC7
# .texture at 1920x1080 or 3840x2160 - which is why "no cover" looked like the
# game simply had none. Decoding one small mip gives every stock track a real
# picture, and a thumbnail never needs the 4K level.
_PAGE = 64 * 1024
_TILE = 256                     # BC7 tiles are 256x256, one tile per page


def _mip_layout(w, h, level):
    mw, mh = max(w >> level, 1), max(h >> level, 1)
    cols = max(-(-mw // _TILE), 1)
    rows = max(-(-mh // _TILE), 1)
    return mw, mh, cols, rows


def _decode_cover(folder, want=640):
    """Decode a track's BC7 cover to PNG, picking a mip near `want` wide."""
    ent = _texture_index().get(folder)
    if not ent:
        return None
    hdr_path, mips_path = ent
    pkg = viewer.package()
    try:
        import texture2ddecoder
        from PIL import Image
        from .tracktables import walk

        with open(pkg, "rb") as f:
            hdr = raw = None
            for p, s, o in kspkg.iter_entries(pkg):
                if p == hdr_path:
                    hdr = kspkg.read_entry(f, s, o, p)
                elif p == mips_path:
                    # ⚠ .texturemips are stored UNENCRYPTED - read_entry knows,
                    # but only by extension, so never decrypt these by hand.
                    raw = kspkg.read_entry(f, s, o, p)
            if hdr is None or raw is None:
                return None

        w = h = nmips = fmt = 0
        for fn, wire, v in walk(hdr):
            if wire != 0:
                continue
            if fn == 1: w = v
            elif fn == 2: h = v
            elif fn == 3: nmips = v
            elif fn == 4: fmt = v
        if fmt not in (33, 34, 35) or not w:
            return None                      # only BC7 covers, for now

        # walk the mip chain to the first level small enough to be a thumbnail
        off = 0
        for level in range(max(nmips, 1)):
            mw, mh, cols, rows = _mip_layout(w, h, level)
            size = cols * rows * _PAGE
            if off + size > len(raw):
                return None
            if mw <= want or level == nmips - 1:
                page = raw[off:off + size]
                break
            off += size
        else:
            return None

        # one tile per page, padded to the full tile in BOTH directions
        img = Image.new("RGBA", (cols * _TILE, rows * _TILE))
        for i in range(cols * rows):
            blob = page[i * _PAGE:(i + 1) * _PAGE]
            px = texture2ddecoder.decode_bc7(blob, _TILE, _TILE)
            tile = Image.frombytes("RGBA", (_TILE, _TILE), px, "raw", "BGRA")
            img.paste(tile, ((i % cols) * _TILE, (i // cols) * _TILE))
        img = img.crop((0, 0, mw, mh)).convert("RGB")
        img.save(track_path(folder), "PNG", optimize=True)
        logs.LOG.info("decoded cover for %s (%dx%d from mip %d)",
                      folder, mw, mh, level)
        return track_path(folder)
    except Exception as ex:
        logs.LOG.warning("decode cover %s: %s", folder, ex)
        return None


_TEXTURES = None


def _texture_index():
    """Track folder -> (.texture, .texturemips) of its cover photo.

    Files are `<track>-<layout>.texture`; a track with several layouts gets
    whichever comes first, since any of them is a photo of the same place.
    """
    global _TEXTURES
    if _TEXTURES is not None:
        return _TEXTURES
    _TEXTURES = {}
    pkg = viewer.package()
    if not pkg:
        return _TEXTURES
    folders = _aliases()
    stems = {}
    try:
        for p, s, _o in kspkg.iter_entries(pkg):
            low = p.lower()
            if not s or "\\images\\tracks\\" not in low:
                continue
            if low.endswith(".texture"):
                stems.setdefault(low[:-len(".texture")], {})["h"] = p
            elif low.endswith(".texturemips"):
                stems.setdefault(low[:-len(".texturemips")], {})["m"] = p
    except Exception as ex:
        logs.LOG.warning("texture index: %s", ex)
        return _TEXTURES
    for stem, pair in stems.items():
        if "h" not in pair or "m" not in pair:
            continue
        name = stem.rsplit("\\", 1)[-1]
        for alias, folder in folders:
            if name.startswith(alias):
                _TEXTURES.setdefault(folder, (pair["h"], pair["m"]))
                break
    return _TEXTURES


def _aliases():
    """(name a file might use, track folder), longest first.

    ⚠ The image files are named after the track's DISPLAY name, not its
    folder: `circuit_de_spa_francorchamps-gp` for the folder `spa`, and
    `red_bull_ring-gp` for `redbull_ring`. Matching on the folder alone quietly
    lost both, and they are base game tracks that certainly have a photo.
    """
    from . import contentsync
    out = []
    for name, folder in contentsync.track_map().items():
        out.append((folder.lower(), folder))
        flat = re.sub(r"[^a-z0-9]+", "_", name.lower()).strip("_")
        if flat:
            out.append((flat, folder))
            # "Nurburgring 24H" -> nurburgring; the suffix is the layout
            out.append((flat.split("_")[0], folder))
    # longest alias first, so `donington_park` wins over `donington`
    return sorted(set(out), key=lambda x: -len(x[0]))


def covers_available():
    """Tracks the game ships a cover for, either as PNG or as a BC7 texture."""
    return sorted(set(_cover_index()) | set(_texture_index()))


def covers_have():
    _dirs()
    return {os.path.splitext(f)[0] for f in os.listdir(TRACKS)
            if f.endswith(".png")}


def build_covers(force=False):
    """Decode every track cover in the background; poll cover_job().

    ⚠ Serialised, like the car renders. Each BC7 decode is a second or two of
    CPU, and the on-demand path already runs one per tile - firing all of them
    at once just makes the page slower while it fights itself.
    """
    with _LOCK:
        if _COVER_JOB["state"] == "running":
            return {"ok": False, "error": "already decoding"}
        _COVER_JOB.update({"state": "running", "done": 0, "total": 0,
                           "current": "", "made": 0})

    def run():
        from . import contentsync
        folders = sorted(set(contentsync.track_map().values()))
        _COVER_JOB["total"] = len(folders)
        t0 = time.time()
        for i, f in enumerate(folders, 1):
            _COVER_JOB["done"], _COVER_JOB["current"] = i, f
            try:
                if force:
                    p = track_path(f)
                    if os.path.isfile(p):
                        os.remove(p)
                if track_cover(f):
                    _COVER_JOB["made"] += 1
            except Exception as ex:
                logs.LOG.warning("cover %s: %s", f, ex)
        _COVER_JOB.update({"state": "done", "current": "",
                           "seconds": round(time.time() - t0, 1)})

    threading.Thread(target=run, daemon=True).start()
    return {"ok": True}


def cover_job():
    return dict(_COVER_JOB)
