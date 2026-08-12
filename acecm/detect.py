"""Find everything ACECM needs, without scanning the disk.

Guessing a handful of hardcoded paths only works on the machine it was written
on. Scanning every drive works everywhere and takes minutes. Neither is
acceptable in a shipped app, so ask the systems that already know:

  * **Steam** records its own location in the registry, and every library
    folder in `steamapps/libraryfolders.vdf`. From those, an app's install
    directory is one `appmanifest_<appid>.acf` lookup - exact, and instant,
    however many drives are involved.
  * **Windows** records where "Saved Games" is via the known-folder API. It is
    NOT always `%USERPROFILE%\\Saved Games`; people relocate it, and OneDrive
    moves it. Building the path by hand quietly points at nothing.

Only if those come back empty do we fall back to a short list of conventional
places, and even then we look for a distinguishing FILE rather than accepting
any folder with the right name.

Results are cached in `<data>/paths.json`. The cache is re-validated on every
use - a remembered path that no longer exists is dropped rather than trusted -
so a moved install heals itself on the next look.
"""
import glob
import json
import os
import re
import time

from . import config, logs

APPID_GAME = "3058630"           # Assetto Corsa EVO
# The dedicated server is distributed as a separate tool; its appid varies by
# branch, so it is found by its executable rather than by id.
SERVER_EXE_RE = re.compile(r"^AssettoCorsaEVOServer.*\.exe$", re.I)

CACHE = os.path.join(config.DATA, "paths.json")
_MEM = {}


# ------------------------------------------------------------------ steam --
def steam_root():
    """Steam's own install folder, from the registry."""
    try:
        import winreg
    except ImportError:
        return ""
    for root, key, name in (
        (winreg.HKEY_CURRENT_USER, r"Software\Valve\Steam", "SteamPath"),
        (winreg.HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Valve\Steam",
         "InstallPath"),
        (winreg.HKEY_LOCAL_MACHINE, r"SOFTWARE\Valve\Steam", "InstallPath"),
    ):
        try:
            with winreg.OpenKey(root, key) as k:
                p = winreg.QueryValueEx(k, name)[0]
                if p and os.path.isdir(p):
                    return os.path.normpath(p)
        except OSError:
            continue
    return ""


def steam_libraries():
    """Every Steam library folder on this machine."""
    root = steam_root()
    if not root:
        return []
    libs, seen = [], set()

    def add(p):
        p = os.path.normpath(p)
        if p.lower() not in seen and os.path.isdir(p):
            seen.add(p.lower())
            libs.append(p)

    add(root)
    vdf = os.path.join(root, "steamapps", "libraryfolders.vdf")
    try:
        text = open(vdf, encoding="utf-8", errors="replace").read()
    except OSError:
        return libs
    # The file is Valve's KeyValues, not JSON. Every library has a "path"
    # entry, and that is all we need - no need to parse the whole format.
    for m in re.finditer(r'"path"\s*"([^"]+)"', text):
        add(m.group(1).replace("\\\\", "\\"))
    return libs


def steam_app_dir(appid):
    """Install folder of a Steam app, via its manifest. Exact, and instant."""
    for lib in steam_libraries():
        acf = os.path.join(lib, "steamapps", f"appmanifest_{appid}.acf")
        if not os.path.isfile(acf):
            continue
        try:
            text = open(acf, encoding="utf-8", errors="replace").read()
        except OSError:
            continue
        m = re.search(r'"installdir"\s*"([^"]+)"', text)
        if not m:
            continue
        p = os.path.join(lib, "steamapps", "common", m.group(1))
        if os.path.isdir(p):
            return p
    return ""


# --------------------------------------------------------- known folders --
def saved_games():
    """The real "Saved Games" folder, which is not always under the profile."""
    try:
        import ctypes
        from ctypes import wintypes
        # FOLDERID_SavedGames
        guid = "{4C5C32FF-BB9D-43b0-B5B4-2D72E54EAAA4}"

        class GUID(ctypes.Structure):
            _fields_ = [("Data1", wintypes.DWORD), ("Data2", wintypes.WORD),
                        ("Data3", wintypes.WORD), ("Data4", ctypes.c_byte * 8)]

        g = GUID()
        if ctypes.windll.ole32.CLSIDFromString(guid, ctypes.byref(g)) == 0:
            out = ctypes.c_wchar_p()
            if ctypes.windll.shell32.SHGetKnownFolderPath(
                    ctypes.byref(g), 0, None, ctypes.byref(out)) == 0:
                p = out.value
                ctypes.windll.ole32.CoTaskMemFree(out)
                if p and os.path.isdir(p):
                    return p
    except Exception as ex:
        logs.LOG.debug("known-folder lookup failed: %s", ex)
    return os.path.join(os.path.expanduser("~"), "Saved Games")


# ------------------------------------------------------------- detectors --
def _first_dir(cands, contains=None):
    """First candidate that exists - and, if asked, holds a telling file."""
    for c in cands:
        if not c or not os.path.isdir(c):
            continue
        if contains is None:
            return c
        try:
            names = os.listdir(c)
        except OSError:
            continue
        if any(contains(n) for n in names):
            return c
    return ""


def game_dir():
    d = steam_app_dir(APPID_GAME)
    if d:
        return d
    return _first_dir(
        [os.path.join(lib, "steamapps", "common", "Assetto Corsa EVO")
         for lib in steam_libraries()]
        + [r"C:\Program Files (x86)\Steam\steamapps\common\Assetto Corsa EVO",
           r"C:\Program Files\Steam\steamapps\common\Assetto Corsa EVO"],
        contains=lambda n: n.lower() == "assettocorsaevo.exe")


def game_exe():
    d = game_dir()
    p = os.path.join(d, "AssettoCorsaEVO.exe") if d else ""
    return p if p and os.path.isfile(p) else ""


def server_dir():
    """A dedicated-server install: any folder holding a server executable."""
    home = os.path.expanduser("~")
    cands = []
    # Steam libraries first - a server installed as a Steam tool lands there
    for lib in steam_libraries():
        common = os.path.join(lib, "steamapps", "common")
        cands += [os.path.join(common, "Assetto Corsa EVO Dedicated Server"),
                  os.path.join(common, "Assetto Corsa EVO Server")]
    cands += [os.path.join(home, "Downloads", "ACE_server_portable"),
              os.path.join(home, "Downloads", "AssettoCorsaEVOServer"),
              os.path.join(home, "ACE_server_portable"),
              r"C:\ACE_server", r"C:\AssettoCorsaEVOServer"]
    return _first_dir(cands, contains=lambda n: bool(SERVER_EXE_RE.match(n)))


def ace_dir():
    """Where the client keeps settings and mods."""
    return _first_dir([os.path.join(saved_games(), "ACE"),
                       os.path.join(os.path.expanduser("~"), "Saved Games", "ACE")])


def ace_server_dir():
    return _first_dir([os.path.join(saved_games(), "ACE-Server"),
                       os.path.join(os.path.expanduser("~"), "Saved Games",
                                    "ACE-Server")])


def client_mods():
    d = ace_dir()
    return os.path.join(d, "mods") if d else ""


def server_mods():
    d = ace_server_dir()
    return os.path.join(d, "mods") if d else ""


# ----------------------------------------------------------------- cache --
FINDERS = {
    "game_dir": game_dir,
    "game_exe": game_exe,
    "server_dir": server_dir,
    "ace_dir": ace_dir,
    "ace_server_dir": ace_server_dir,
    "client_mods": client_mods,
    "server_mods": server_mods,
}


def _load_cache():
    if _MEM:
        return _MEM
    try:
        _MEM.update(json.load(open(CACHE, encoding="utf-8")))
    except Exception:
        pass
    return _MEM


def _save_cache(d):
    _MEM.update(d)
    try:
        json.dump(_MEM, open(CACHE, "w", encoding="utf-8"), indent=2)
    except OSError as ex:
        logs.LOG.warning("could not write the path cache: %s", ex)


def find(what, refresh=False):
    """A detected path, cached. Never returns something that is not there."""
    if what not in FINDERS:
        raise KeyError(what)
    # An explicit setting always wins over detection.
    override = (config.CFG.get(what) or "").strip()
    if override and os.path.exists(override):
        return override

    cache = _load_cache()
    hit = (cache.get(what) or {}).get("path", "")
    # ⚠ Re-validate. A cached path that has since been moved or uninstalled is
    # worse than no path at all: it makes every downstream feature fail with a
    # confusing error instead of simply looking again.
    if hit and not refresh and os.path.exists(hit):
        return hit

    t0 = time.perf_counter()
    found = FINDERS[what]() or ""
    ms = (time.perf_counter() - t0) * 1000
    _save_cache({what: {"path": found, "found_at": int(time.time()),
                        "ms": round(ms, 1)}})
    logs.LOG.info("detect %-15s %-6s %s", what, f"{ms:.0f}ms",
                  found or "(not found)")
    return found


def all_paths(refresh=False):
    """Everything, for the Settings page."""
    out, t0 = {}, time.perf_counter()
    for what in FINDERS:
        p = find(what, refresh)
        out[what] = {"path": p, "exists": bool(p and os.path.exists(p)),
                     "source": "setting" if (config.CFG.get(what) or "").strip()
                               else "detected"}
    return {"paths": out,
            "steam": steam_root(),
            "libraries": steam_libraries(),
            "saved_games": saved_games(),
            "took_ms": round((time.perf_counter() - t0) * 1000, 1)}
