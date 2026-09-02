"""The 3D car viewer: list what can be shown, extract it, launch evoview.

The viewer (evoview.exe) reads a car FOLDER directly - meshes, textures,
materials, liveries, tyres, animations - so nothing has to be pre-converted.
What it cannot do is read the game's content.kspkg, so this module is the
bridge: it lists the cars inside the client package, extracts one on demand
into a cache, and starts the viewer pointed at it.

Two caches, both keyed on the package's size+mtime so a game update
invalidates them:

    <DATA>/viewer/index.json     car ids found in content.kspkg
    <DATA>/viewer/cars/<id>/     an extracted car, reused on later views
    <DATA>/viewer/assets/        shared tyre meshes and paints

⚠ A car folder is NOT self-contained. Tyres and paints live under
content\\cars\\common_assets\\; without them a car renders on bare rims in its
material's own colour. See memory acevo-car-render-recipe.
"""
import json
import os
import re
import subprocess
import sys
import threading

from . import config, detect, install, kspkg, logs

_LOCK = threading.Lock()
_JOBS = {}          # car id -> {"state", "detail"}


_root_made = set()


def _root():
    # ⚠ makedirs on every call looks free and is not: cars_root/assets_root
    # go through here, so building the Drive page hit the filesystem ~180
    # times to create one directory that already existed. Remember the ones
    # we have made this run; if something deletes it underneath us the next
    # write fails loudly, which is better than paying for the check forever.
    d = os.path.join(config.DATA, "viewer")
    if d not in _root_made:
        os.makedirs(d, exist_ok=True)
        _root_made.add(d)
    return d


def cars_root():
    return os.path.join(_root(), "cars")


def assets_root():
    return os.path.join(_root(), "assets")


def package():
    """The client's content.kspkg, or None if the game was not found."""
    g = detect.game_dir()
    if not g:
        return None
    p = os.path.join(g, "content.kspkg")
    return p if os.path.isfile(p) else None


def packages():
    """Every package that can contain a viewable car.

    The base game plus each installed mod. A mod ships its car under the same
    `content\\cars\\<id>\\` layout, so nothing downstream has to care which
    package a car came from - but the picker must look in all of them or
    modded cars simply never appear.
    """
    out = []
    base = package()
    if base:
        out.append(("", base))
    seen = set()
    for which in ("client", "server"):
        try:
            d = install.client_mods_dir() if which == "client" else install.mods_dir()
        except Exception:
            continue
        if not d or not os.path.isdir(d):
            continue
        for f in sorted(os.listdir(d)):
            if not f.lower().endswith(".kspkg"):
                continue
            full = os.path.join(d, f)
            key = os.path.basename(full).lower()
            if key in seen:
                continue
            seen.add(key)
            out.append((os.path.splitext(f)[0], full))
    return out


def _stamp(path):
    st = os.stat(path)
    return f"{st.st_size}:{int(st.st_mtime)}"


def viewer_exe():
    """Where evoview.exe is, checking the setting then the usual places."""
    cfg = (config.CFG.get("viewer_exe") or "").strip()
    if cfg and os.path.isfile(cfg):
        return cfg
    names = ["evoview.exe"]
    roots = [
        config.BUNDLED_TOOLS,
        os.path.join(config.ROOT, "tools"),
        os.path.join(config.ROOT, "viewer"),
        _root(),
        # developing against the checkout next door
        os.path.join(os.path.dirname(config.ROOT), "evoview", "target", "release"),
    ]
    for r in roots:
        for n in names:
            p = os.path.join(r, n)
            if os.path.isfile(p):
                return p
    return None



def _looks_like_path(arg):
    """Is this argument a filesystem path rather than a flag or a value?"""
    a = str(arg)
    if not os.path.isabs(a):
        return False
    # an output file does not exist yet, but its directory does
    return os.path.exists(a) or os.path.isdir(os.path.dirname(a))


def viewer_cmd(cmd):
    """Turn an evoview argv into something this platform can actually run.

    evoview is a Windows binary. On Linux it goes through the same Proton
    prefix as the game — it reads the game's own .kspkg archives, so a
    prefix that cannot see them is no use — and every path argument has to be
    translated, because the exe resolves them on the Windows side of wine.

    Returns (argv, env). On Windows both are unchanged apart from the usual
    child-environment scrubbing.
    """
    from . import winproc
    env = winproc.child_env()
    if sys.platform == "win32":
        return list(cmd), env
    from . import proton
    appid = str(config.CFG.get("steam_appid") or "3058630")
    if not proton.available(appid):
        raise RuntimeError(
            "evoview needs the game's Proton prefix, which does not exist "
            "yet — launch Assetto Corsa EVO from Steam once, then try again.")
    args = [proton.to_windows_path(a, appid) if _looks_like_path(a) else str(a)
            for a in cmd[1:]]
    argv = proton.run_argv(appid, proton.to_windows_path(cmd[0], appid),
                           args, verb="runinprefix")
    return argv, proton.run_env(appid, env)


# ------------------------------------------------------------------ listing --

def _pretty(car_id):
    s = re.sub(r"^ks_", "", car_id)
    return " ".join(w.upper() if len(w) <= 3 and w.isalpha() else w.capitalize()
                    for w in s.split("_") if w)


def index(refresh=False):
    """Every car in the package that has geometry, cached.

    ⚠ Scanning the 64 MiB index takes a few seconds, so it is cached. Only
    cars with a meshes\\ entry are listed: the dedicated-server package has no
    render meshes at all and would produce a list of cars that cannot be shown.
    """
    pkgs = packages()
    if not pkgs:
        return {"error": "game install not found - set the game folder in "
                         "Settings", "cars": []}
    cache = os.path.join(_root(), "index.json")
    # ⚠ The mod FOLDERS are part of the stamp, not just the packages we found
    # in them. Otherwise a mod added or deleted while ACECM is open keeps the
    # old list - the car is on disk and ACECM still says it is missing, which
    # looks like a bug in the mod rather than a stale cache.
    dirs = []
    for which in ("client", "server"):
        try:
            d = install.client_mods_dir() if which == "client" else install.mods_dir()
        except Exception:
            continue
        if d and os.path.isdir(d):
            try:
                dirs.append(f"{d}:{int(os.stat(d).st_mtime)}:"
                            f"{len(os.listdir(d))}")
            except OSError:
                pass
    stamp = "|".join([f"{os.path.basename(p)}:{_stamp(p)}" for _m, p in pkgs]
                     + dirs)

    def _mark(cars):
        # ⚠ Recompute "already extracted" on every read. It changes whenever a
        # car is viewed, while the index itself only changes when the game is
        # patched - baking the flag into the cache means a car you just
        # extracted never shows as cached.
        for c in cars:
            c["extracted"] = os.path.isdir(os.path.join(cars_root(), c["id"]))
        return cars

    if not refresh and os.path.isfile(cache):
        try:
            got = json.load(open(cache, encoding="utf-8"))
            if got.get("stamp") == stamp:
                return {"cars": _mark(got["cars"]), "cached": True}
        except Exception:
            pass

    found = {}                      # car id -> (mod name, package path)
    bad = []
    for mod, pkg in pkgs:
        # ⚠ Never let one unreadable package empty the whole list. A partial
        # download or a file that is not really a kspkg is a problem with THAT
        # mod; the other cars are fine and must still be listed.
        try:
            for path, size, _off in kspkg.iter_entries(pkg):
                if not size:
                    continue
                low = path.lower()
                if not low.startswith("content\\cars\\") \
                        or "\\meshes\\" not in low:
                    continue
                if not low.endswith(".mesh"):
                    continue
                parts = path.split("\\")
                if len(parts) > 2 and not parts[2].lower().startswith("common"):
                    found.setdefault(parts[2], (mod, pkg))
        except Exception as ex:
            bad.append(os.path.basename(pkg))
            logs.LOG.warning("skipping unreadable package %s: %s", pkg, ex)

    # ⚠ A mod's real name is declared against its PRESET id
    # (preset_apex_ind_h_mech_1), not the folder id we list cars by, so
    # car_names() alone leaves mods showing as "Apex IND H". Take the label
    # from the mod's own manifest instead.
    labels = {}
    try:
        labels = dict(install.car_names())
    except Exception:
        pass
    for which in ("client", "server"):
        try:
            for m in install.installed(which).get("mods") or []:
                for c in m.get("cars") or []:
                    if c.get("label"):
                        labels.setdefault(m["name"], c["label"])
                        break
        except Exception:
            pass
    cars = sorted(
        ({"id": c, "label": labels.get(c) or _pretty(c),
          "mod": bool(mod), "pkg": pkg}
         for c, (mod, pkg) in found.items()),
        key=lambda c: (c["mod"], c["label"]))
    try:
        json.dump({"stamp": stamp, "cars": cars}, open(cache, "w", encoding="utf-8"))
    except Exception as ex:
        logs.LOG.warning("could not cache viewer index: %s", ex)
    # name the bad packages so a mod that cannot be read is visible, rather
    # than just quietly absent from the list
    return {"cars": _mark(cars), "cached": False,
            "unreadable": bad}


# --------------------------------------------------------------- extraction --

def _set(car_id, state, detail=""):
    with _LOCK:
        _JOBS[car_id] = {"state": state, "detail": detail}
    logs.LOG.info("viewer %s: %s %s", car_id, state, detail)


def job(car_id):
    with _LOCK:
        return dict(_JOBS.get(car_id) or {"state": "idle", "detail": ""})


def _tyre_sizes(car_dir):
    """Sizes named by the car's own .compatibletyres, front then rear."""
    out = []
    presets = os.path.join(car_dir, "presets")
    for f in sorted(os.listdir(presets)) if os.path.isdir(presets) else []:
        if not f.endswith(".compatibletyres"):
            continue
        d = open(os.path.join(presets, f), "rb").read()
        # ⚠ sizes are not all road-shaped: 235_45_17 but also 355_660_13
        for m in re.finditer(rb"(\d{2,3}_\d{2,3}_\d{2})\.tyre", d):
            s = m.group(1).decode()
            if s not in out:
                out.append(s)
    return out


def _family(car_dir):
    presets = os.path.join(car_dir, "presets")
    for f in sorted(os.listdir(presets)) if os.path.isdir(presets) else []:
        if not f.endswith(".compatibletyres"):
            continue
        d = open(os.path.join(presets, f), "rb").read().lower()
        m = re.search(rb"tyres[\\/]([a-z0-9_]+)[\\/]", d)
        if m:
            fam = m.group(1).decode()
            if fam.startswith("f1"):
                return "f1"
            if fam.startswith("racing") or "slick" in fam:
                return "racing"
    return "road"


def ensure_shared(car_id, pkg):
    """Pull the shared tyre and paint assets this car needs, once."""
    car_dir = os.path.join(cars_root(), car_id)
    sizes = _tyre_sizes(car_dir)
    fam = _family(car_dir)
    # ⚠ ONE pass over the index. Re-scanning it per file is O(entries x files)
    # against a 64 MiB table and turns a few seconds into minutes.
    tex = "road_1" if fam == "road" else fam
    want = []
    for path, size, off in kspkg.iter_entries(pkg):
        if not size:
            continue
        low = path.lower()
        if "\\parts\\tyres\\" in low:
            if low.endswith((".tyremesh", ".mesh")):
                if any(s in low for s in sizes):
                    want.append((path, size, off))
            elif "\\textures\\" in low and tex in low:
                want.append((path, size, off))
            elif "\\materials\\" in low:
                want.append((path, size, off))
        elif "\\customization\\" in low and low.endswith(".oemmultilayercolor"):
            want.append((path, size, off))
    if not want:
        return 0
    want.sort(key=lambda x: x[2])          # stream forwards through the file
    written = 0
    with open(pkg, "rb") as f:
        for path, size, off in want:
            dst = os.path.join(assets_root(), path.replace("\\", os.sep))
            if os.path.isfile(dst):
                continue
            os.makedirs(os.path.dirname(dst), exist_ok=True)
            with open(dst, "wb") as o:
                o.write(kspkg.read_entry(f, size, off, path))
            written += 1
    return written


def _package_for(car_id):
    """Which package holds this car - the base game, or a mod's own."""
    for c in index().get("cars", []):
        if c["id"] == car_id:
            return c.get("pkg") or package()
    return package()


def extract(car_id):
    """Extract one car plus its shared assets. Safe to call repeatedly."""
    pkg = _package_for(car_id)
    if not pkg:
        raise RuntimeError("game install not found")
    car_dir = os.path.join(cars_root(), car_id)
    done = os.path.join(car_dir, ".complete")
    if os.path.isfile(done):
        return car_dir

    _set(car_id, "extracting", "reading package index")
    n = kspkg.extract_prefix(
        pkg, f"content\\cars\\{car_id}", car_dir, strip=3,
        progress=lambda i, t: _set(car_id, "extracting", f"{i}/{t} files"))
    if not n:
        raise RuntimeError(f"{car_id} not found in the package")

    _set(car_id, "extracting", "shared tyres and paints")
    try:
        # ⚠ Shared tyres and paints live in the BASE package. A mod .kspkg
        # contains only its own car, so pulling them from `pkg` would find
        # nothing and the car would render on bare rims.
        ensure_shared(car_id, package() or pkg)
    except Exception as ex:
        # not fatal: the car still renders, just on bare rims
        logs.LOG.warning("shared assets for %s: %s", car_id, ex)

    open(done, "w").write("ok")
    _set(car_id, "ready", f"{n} files")
    return car_dir


def open_car(car_id, paint=""):
    """Launch the viewer on a car, reading it straight out of the package.

    ⚠ Nothing is extracted. evoview reads the .kspkg in place, so a car costs
    no disk at all - extracting one was 200-800 MB. `extract()` is kept for
    tooling that wants a real folder on disk, but the picker no longer needs it.
    """
    exe = viewer_exe()
    if not exe:
        raise RuntimeError(
            "evoview.exe not found. Put it in the tools folder next to ACECM, "
            "or set viewer_exe in Settings.")
    pkg = _package_for(car_id)
    if not pkg:
        raise RuntimeError("game install not found")
    cmd = [exe, pkg, "--car", car_id]
    base = package()
    # a mod package holds only its own car; the base game supplies the shared
    # tyre material and paints
    if base and os.path.abspath(base) != os.path.abspath(pkg):
        cmd += ["--base", base]

    # never hand a child our frozen bundle's PATH - see winproc.child_env
    cmd, env = viewer_cmd(cmd)
    if paint:
        env["EVOVIEW_PAINT"] = paint
    kw = {}
    if sys.platform == "win32":
        kw["creationflags"] = subprocess.CREATE_NEW_PROCESS_GROUP
    else:
        kw["start_new_session"] = True
    subprocess.Popen(cmd, env=env, cwd=os.path.dirname(exe), **kw)
    _set(car_id, "open", "")
    return {"ok": True, "package": pkg}


def open_track(folder):
    """Launch the viewer on a track, free-look, no studio floor.

    Loose imported folders (EvoForge / ACECM) win: they are already on disk.
    Stock circuits are read out of the client content.kspkg in place.
    """
    folder = (folder or "").strip()
    if not folder:
        raise RuntimeError("no track folder")
    exe = viewer_exe()
    if not exe:
        raise RuntimeError(
            "evoview.exe not found. Put it in the tools folder next to ACECM, "
            "or set viewer_exe in Settings.")
    loose = ""
    try:
        from . import contentsync
        cand = os.path.join(contentsync.tracks_dir(), folder)
        if os.path.isdir(cand):
            loose = cand
    except Exception:
        pass
    pkg = package()
    if loose:
        cmd = [exe, loose, "--track"]
        if pkg:
            cmd += ["--base", pkg]
        src = loose
    else:
        if not pkg:
            raise RuntimeError("game install not found")
        cmd = [exe, pkg, "--track", folder]
        src = pkg
    cmd, env = viewer_cmd(cmd)
    kw = {}
    if sys.platform == "win32":
        kw["creationflags"] = subprocess.CREATE_NEW_PROCESS_GROUP
    else:
        kw["start_new_session"] = True
    subprocess.Popen(cmd, env=env, cwd=os.path.dirname(exe), **kw)
    _set("track:" + folder, "open", "")
    logs.LOG.info("viewer track %s from %s", folder, src)
    return {"ok": True, "folder": folder, "source": src}


def start_open_track(folder):
    def run():
        try:
            open_track(folder)
        except Exception as ex:
            _set("track:" + (folder or ""), "error", str(ex))
            logs.LOG.exception("viewer open failed for track %s", folder)
    _set("track:" + (folder or ""), "starting", "")
    threading.Thread(target=run, daemon=True).start()
    return {"ok": True}


def start_open(car_id, paint=""):
    """Kick the whole thing off in the background; poll job() for progress."""
    def run():
        try:
            open_car(car_id, paint)
        except Exception as ex:
            _set(car_id, "error", str(ex))
            logs.LOG.exception("viewer open failed for %s", car_id)
    _set(car_id, "starting", "")
    threading.Thread(target=run, daemon=True).start()
    return {"ok": True}
