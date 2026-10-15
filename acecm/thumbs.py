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
import hashlib
import json
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
# ⚠ The card on Drive is ~460px wide on a normal window, so a 480px render
# was being displayed at roughly 1:1 and looked soft on any hi-dpi screen.
# 900x600 costs the same single evoview launch and survives being shown big.
SIZE = "900x600"
# ⚠ The detail pane shows one car much larger than a list row does, and a
# 480px render blown up to fill it is visibly soft - it reads as a low-quality
# app rather than a low-resolution file. Big renders are made ON DEMAND for the
# car you actually opened, never for all 85, because each one costs an evoview
# launch.
BIG = "1440x960"
_LOCK = threading.Lock()
_FAILED = set()   # pictures that failed to render this session
_JOB = {"state": "idle", "done": 0, "total": 0, "current": "", "made": 0}
# tracks decode separately from cars: different work, different button
_COVER_JOB = {"state": "idle", "done": 0, "total": 0, "current": "", "made": 0}


def _dirs():
    for d in (CARS, TRACKS):
        os.makedirs(d, exist_ok=True)


def car_path(car_id, big=False, visual="", paint=""):
    """Where one car's render lives.

    ⚠ Keyed by the LOOK, not just the model. A car's trims share one model
    folder, so a model-only name meant "BMW M2 Coupe Standard" and
    "Performance" fought over a single file - whichever rendered last won and
    both rows showed it. The livery is in the key for the same reason: change
    your colour and the old picture is no longer of your car.
    """
    _dirs()
    safe = re.sub(r"[^A-Za-z0-9_.-]", "_", car_id)
    tag = ""
    if visual or paint:
        raw = f"{visual}|{paint}".lower()
        tag = "@" + hashlib.sha1(raw.encode("utf-8")).hexdigest()[:10]
    return os.path.join(CARS, safe + tag + ("@big.png" if big else ".png"))


def preset_look(preset_id):
    """(model, visual preset, EXT SKIN paint) for a mechanical preset id.

    The trim comes from the game's own preset data; the paint from the
    player's garage, so the preview is of THEIR car rather than the showroom
    default. Anything unknown comes back empty and the render falls back to
    evoview's own defaults.
    """
    model = visual = paint = ""
    try:
        from . import carsmap
        t = carsmap.table()
        model = (t.get("presets") or {}).get(preset_id, "")
        visual = ((t.get("variants") or {}).get(preset_id) or {}).get("visual", "")
    except Exception:
        pass
    if not model:
        # ⚠ a MOD's presets are not in the base game's table: its own
        # manifest names the car. Without this the model was the preset id,
        # no garage paint matched, and the picture fell back to the bare model
        try:
            from . import install
            model = install.car_models().get(preset_id) or ""
        except Exception:
            model = ""
    if not model:
        model = preset_id
    paint = _garage_paints().get(model, "")
    return model, visual, paint


_GARAGE = {"key": None, "map": {}}


def _garage_paints():
    """model -> the EXT SKIN paint the player has on it.

    ⚠ Memoised. livery.garage() walks and parses every saved car (84 files
    here), and preset_look() is called once per thumbnail REQUEST and once per
    car in a render pass - so reading it each time turned one page of the car
    list into thousands of file reads and stalled the render job before it
    had listed a single car.
    """
    try:
        from . import livery
        d = livery._saved_cars_dir()
        key = (d, os.path.getmtime(d)) if d and os.path.isdir(d) else (d, 0)
    except Exception:
        d, key = "", ("", 0)
    if _GARAGE["key"] == key:
        return _GARAGE["map"]
    out = {}
    try:
        from . import livery
        for c in livery.garage() or []:
            model = c.get("model")
            skin = ((c.get("slots") or {}).get("EXT SKIN") or "").strip()
            if model and skin and model not in out:
                out[model] = skin.replace(chr(92), "/").rsplit("/", 1)[-1]
    except Exception as ex:
        logs.LOG.info("garage paints: %s", ex)
    _GARAGE["key"], _GARAGE["map"] = key, out
    return out


def track_path(folder):
    _dirs()
    return os.path.join(TRACKS, re.sub(r"[^A-Za-z0-9_.-]", "_", folder) + ".png")


# ------------------------------------------------------------------- cars --

def render_car(car_id, force=False, timeout=180, make=True, big=False,
               visual="", paint=""):
    """One car, rendered by evoview into the cache. Returns the path or None.

    make=False (the list GET) never launches evoview. Typing in Drive used
    to rebuild every row and fire this on each key, which opened a console
    per car and stole focus after one character.
    """
    if not car_id:
        return None
    out = car_path(car_id, big=big, visual=visual, paint=paint)
    if os.path.isfile(out) and not force:
        return out
    if not make:
        return None
    # a car the viewer cannot load is not retried on every page visit
    if not force and out in _FAILED:
        return None
    exe = viewer.viewer_exe()
    if not exe:
        return None
    pkg = viewer._package_for(car_id)
    if not pkg:
        return None
    cmd = [exe, pkg, "--car", car_id, "--shot", out,
           "--size", BIG if big else SIZE,
           "--yaw", "2.35", "--pitch", "0.17",
           # zoom > 1 moves the camera closer (shot.rs: distance /= zoom).
           # The default framing leaves a lot of empty floor around the car.
           "--zoom", "1.35"]
    base = viewer.package()
    # a mod package holds only its own car; the base game supplies shared tyres
    if base and os.path.abspath(base) != os.path.abspath(pkg):
        cmd += ["--base", base]
    from . import winproc
    # ⚠ Tell evoview WHICH trim and colour. Without EVOVIEW_VISUAL it takes
    # the first visual preset it finds, so every trim of a car came out the
    # same; EVOVIEW_PAINT overrides the showroom default with the colour the
    # player actually has on the car.
    try:
        # ⚠ viewer_cmd RETURNS a fresh child_env, so these must be set AFTER
        # it, not before. Setting them first and then doing
        # `cmd, env = viewer_cmd(cmd)` silently threw both away, and every trim
        # of a car rendered as the first visual preset again - the exact bug
        # EVOVIEW_VISUAL exists to prevent. viewer.open_car has the order right.
        cmd, env = viewer.viewer_cmd(cmd)
        if visual:
            env["EVOVIEW_VISUAL"] = visual
        if paint:
            env["EVOVIEW_PAINT"] = paint
        r = winproc.hidden_run(cmd, capture_output=True, text=True,
                               timeout=timeout, cwd=config.DATA,
                               env=env)
    except subprocess.TimeoutExpired:
        logs.LOG.warning("thumb %s: timed out", car_id)
        return None
    if r.returncode != 0 or not os.path.isfile(out):
        logs.LOG.warning("thumb %s: exit %s %s", car_id, r.returncode,
                         (r.stdout or r.stderr or "")[-200:])
        _FAILED.add(out)
        return None
    _FAILED.discard(out)
    _set_record(out, {"renderer": _renderer_id(), "source": _source_fp(car_id)})
    return out


# ---------------------------------------------------------- picture records
# ⚠ The render cache used to ask only "is there a file": pictures from an old
# viewer (flat paint, missing decals, cars that did not render at all) stayed
# for good while the viewer improved, and a game or mod update never redrew
# the car it changed. Every picture now records what it was drawn FROM - the
# renderer and the car's package - and is redrawn by itself when either
# changes. Same file and format as the native app (native/src/thumbs.rs), so
# moving between the two does not redraw everything.

# Bump when render_car's framing (size, yaw, pitch, zoom) changes.
RECIPE = "v1 900x600 1440x960 yaw2.35 pitch0.17 zoom1.35"
_RECORDS_FILE = os.path.join(CARS, "sources.json")
_RECORDS = {"map": None}
_RID = {"key": None, "id": ""}


def _renderer_id():
    """Hash of the viewer exe + RECIPE ("" when there is no viewer)."""
    exe = viewer.viewer_exe()
    if not exe or not os.path.isfile(exe):
        return ""
    st = os.stat(exe)
    key = (exe, st.st_size, st.st_mtime_ns)
    if _RID["key"] != key:
        import hashlib
        with open(exe, "rb") as f:
            _RID.update(key=key, id=hashlib.sha256(f.read()).hexdigest()[:16])
    return f"{_RID['id']} {RECIPE}"


def _source_fp(model, cache=None):
    """The package a picture is drawn from, as "size:mtime"."""
    pkg = viewer._package_for(model)
    if not pkg:
        return ""
    if cache is not None and pkg in cache:
        return cache[pkg]
    try:
        st = os.stat(pkg)
        fp = f"{st.st_size}:{int(st.st_mtime)}"
    except OSError:
        fp = ""
    if cache is not None:
        cache[pkg] = fp
    return fp


def _records():
    with _LOCK:
        if _RECORDS["map"] is None:
            try:
                with open(_RECORDS_FILE, encoding="utf-8") as f:
                    _RECORDS["map"] = json.load(f) or {}
            except (OSError, ValueError):
                _RECORDS["map"] = {}
        return dict(_RECORDS["map"])


def _set_record(path, value):
    _records()
    with _LOCK:
        m = _RECORDS["map"]
        name = os.path.basename(path)
        if value is None:
            m.pop(name, None)
        else:
            m[name] = value
        tmp = _RECORDS_FILE + ".part"
        try:
            with open(tmp, "w", encoding="utf-8") as f:
                json.dump(m, f)
            os.replace(tmp, _RECORDS_FILE)
        except OSError as ex:
            logs.LOG.info("thumb records: %s", ex)


def _out_of_date(path, rid, src, rec):
    if not os.path.isfile(path):
        return True
    r = rec.get(os.path.basename(path))
    if not r:
        return True          # drawn before records existed: an older viewer
    return r.get("renderer") != rid or r.get("source") != src


def render_preset(preset_id, **kw):
    """Render the car as this PRESET wears it - its trim and your livery."""
    model, visual, paint = preset_look(preset_id)
    return render_car(model, visual=visual, paint=paint, **kw)


def build_all(force=False):
    """Render every car in the background; poll job() for progress."""
    with _LOCK:
        if _JOB["state"] == "running":
            return {"ok": False, "error": "already building"}
        _JOB.update({"state": "running", "done": 0, "total": 0,
                     "current": "", "made": 0})

    def run():
        # ⚠ One render per PRESET, not per model: the picker asks for a
        # picture of the trim you chose wearing your livery, so a pass over
        # models alone left every trim row falling back to the same photo.
        # Trims that look identical (the M2 CS Racing 350 and 450 differ
        # mechanically) resolve to the same visual+paint and therefore the
        # same cache file, so this costs no extra renders for them.
        jobs = _render_jobs()
        _JOB["total"] = len(jobs)
        t0 = time.time()
        rid = _renderer_id()
        fps = {}
        if force:
            _FAILED.clear()
        for i, (pid, label, model, visual, paint) in enumerate(jobs, 1):
            _JOB["done"], _JOB["current"] = i, label or pid
            try:
                pic = car_path(model, visual=visual, paint=paint)
                src = _source_fp(model, fps)
                if not force and not _out_of_date(pic, rid, src, _records()):
                    continue
                # redraw over an existing picture: it stays up until replaced
                if not render_car(model, os.path.isfile(pic), visual=visual,
                                  paint=paint):
                    continue
                _JOB["made"] += 1
                # the model's other pictures from the old renderer go: the
                # large one is redrawn on demand, and the plain fallback is
                # superseded by this trim/paint picture
                for other in (car_path(model, big=True, visual=visual, paint=paint),
                              car_path(model)):
                    if (other != pic and os.path.isfile(other)
                            and _out_of_date(other, rid, src, _records())):
                        try:
                            os.remove(other)
                        except OSError:
                            pass
                        _set_record(other, None)
            except Exception as ex:
                logs.LOG.warning("thumb %s: %s", pid, ex)
        _JOB.update({"state": "done", "current": "",
                     "seconds": round(time.time() - t0, 1)})

    threading.Thread(target=run, daemon=True).start()
    return {"ok": True}


def _render_jobs():
    """(preset id, label, model, visual, paint) for everything the UI shows.

    Falls back to the viewer's model list if the car inventory cannot be read,
    so a broken catalogue degrades to the old behaviour instead of rendering
    nothing at all.
    """
    jobs, seen = [], set()
    try:
        from . import content
        for c in content.cars().get("cars", []):
            pid = c.get("id")
            if not pid:
                continue
            model, visual, paint = preset_look(pid)
            key = (model, visual, paint)
            if not model or key in seen:
                continue
            seen.add(key)
            jobs.append((pid, c.get("label") or pid, model, visual, paint))
    except Exception as ex:
        logs.LOG.info("render jobs from content: %s", ex)
    if jobs:
        return jobs
    return [(c["id"], c.get("label") or c["id"], c["id"], "", "")
            for c in viewer.index().get("cars", []) if c.get("id")]


def ensure_renders():
    # the shell renders: one job, never two fighting over the GPU
    if os.environ.get("ACECM_NATIVE_URL"):
        from . import backend
        return backend._native("/api/_native/thumbs/ensure", timeout=10) is True
    return _ensure_renders_local()


def _ensure_renders_local():
    """Kick a background render pass when cars are missing their picture.

    ⚠ The list GET deliberately never renders - doing so spawned an evoview
    console per row on every Drive keystroke. The consequence was that a car
    only ever got a picture if somebody happened to run a pass from the Cars
    page, so newly installed or freshly fetched cars stayed blank tiles
    indefinitely, and so did any stock car that was not present the last time
    a pass ran.

    Safe to call often: build_all() skips cars that already have a render and
    refuses to start while one is running.
    """
    try:
        if _JOB.get("state") == "running":
            return False
        # ⚠ Compare real FILES, not bare model ids. Renders are keyed by
        # trim and livery now, so a name-based check against the model list
        # said "all present" while every trim picture was still missing.
        # also redraws pictures that are OUT OF DATE: a new viewer (ACECM
        # update), a new render recipe, a game update or an updated mod
        rid = _renderer_id()
        if not rid:
            return False
        rec, fps = _records(), {}
        due = False
        for j in _render_jobs():
            pic = car_path(j[2], visual=j[3], paint=j[4])
            if pic not in _FAILED and _out_of_date(pic, rid, _source_fp(j[2], fps), rec):
                due = True
                break
        if not due:
            return False
        logs.LOG.info("thumbs: some car pictures are missing or out of date - "
                      "redrawing them in the background")
        build_all()
        return True
    except Exception as ex:
        logs.LOG.info("ensure_renders: %s", ex)
        return False


def job():
    if os.environ.get("ACECM_NATIVE_URL"):
        from . import backend
        r = backend._native("/api/_native/thumbs/status", timeout=5)
        return {k: v for k, v in r.items()
                if k not in ("have", "covers", "covers_have", "cover_job")}
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


def _resolve_folder(folder):
    """Accept either a folder id or the DISPLAY NAME the UI sometimes sends.

    ⚠ The drive track list carries a display name in `track` for some layouts
    ("Circuit Of The Americas" rather than "cota"), so the cover lookup missed
    every Time Attack layout of COTA, Spa, Donington, Fuji, Red Bull Ring,
    Sebring and Watkins Glen - which read as "some base game tracks have no
    photo". Resolving here fixes it for every caller at once.
    """
    if not folder:
        return folder
    try:
        from . import contentsync
        m = contentsync.track_map()
    except Exception:
        return folder
    if folder in set(m.values()):
        return folder
    hit = m.get(folder)
    if hit:
        return hit
    low = folder.strip().lower()
    for name, f in m.items():
        if name.strip().lower() == low:
            return f
    return folder


def track_cover(folder):
    """Path to a cached cover PNG, or None when the game ships none."""
    folder = _resolve_folder(folder)
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
    if os.environ.get("ACECM_NATIVE_URL"):
        from . import backend
        return backend._native("/api/_native/thumbs/status",
                               timeout=5).get("cover_job") or {}
    return dict(_COVER_JOB)
