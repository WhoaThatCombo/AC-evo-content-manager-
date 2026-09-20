"""Shrink the mods folder on disk with transparent NTFS compression.

EVO content is enormous - roughly 600 MB per car and multiple GB per track -
so a modest collection runs to tens of gigabytes. Measured on a real folder:
43.1 GB of mods stored in 11 GB, a ratio of 4.0 to 1, and every file still
read back byte for byte.

This is Windows' own `compact /exe:LZX` (the WOF overlay used for CompactOS).
The files stay ordinary files: Windows decompresses them on read, so the game
and ACECM neither know nor care. Nothing about the content changes - only how
many blocks it occupies.

⚠ Windows only. `compact` does not exist under Proton/Linux and the whole
feature reports unsupported there rather than pretending.

⚠ WOF is built for read-mostly data. Writing to a compressed file rehydrates
THAT FILE, so this is a good trade for a mods folder (many files, each read
far more often than written) and a poor one for a single huge archive that
gets edited - writing one byte of a 69 GB content.kspkg would decompress all
69 GB. That is why this points at the mods folder and nothing else.
"""
import ctypes
import os
import subprocess
import sys
import threading
import time

from . import install
from . import logs

_state = {"active": False}
_lock = threading.Lock()


def supported():
    return sys.platform == "win32"


def _mods_dir():
    """The folder the game loads loose mod content from."""
    try:
        d = install.client_mods_dir()
    except Exception:                              # noqa: BLE001
        return ""
    return d if d and os.path.isdir(d) else ""


def _on_disk(path):
    """Actual allocated size, which is the whole point of the readout.

    os.path.getsize reports the LOGICAL size and is identical before and
    after compression - reporting it as "after" would make the feature look
    broken. GetCompressedFileSizeW is what the filesystem really spends.
    """
    fn = ctypes.windll.kernel32.GetCompressedFileSizeW
    fn.argtypes = [ctypes.c_wchar_p, ctypes.POINTER(ctypes.c_ulong)]
    fn.restype = ctypes.c_ulong
    high = ctypes.c_ulong(0)
    low = fn(path, ctypes.byref(high))
    if low == 0xFFFFFFFF and ctypes.get_last_error() not in (0,):
        raise OSError("GetCompressedFileSize failed")
    return (high.value << 32) | low


_cache = {"at": 0.0, "root": "", "val": None}
TTL = 90.0


def measure(path="", force=False):
    """Logical vs on-disk for the mods folder.

    ⚠ Cached. Walking ~90k files costs seconds, and this is drawn at the top
    of Settings - re-measuring on every visit made the page sit there. The
    number only moves when content is added or a run finishes, and both of
    those invalidate it explicitly.
    """
    if not supported():
        return {"ok": False, "supported": False,
                "error": "NTFS compression is Windows only"}
    root = path or _mods_dir()
    if not root:
        return {"ok": False, "supported": True, "error": "no mods folder yet"}
    now = time.time()
    if (not force and _cache["val"] and _cache["root"] == root
            and now - _cache["at"] < TTL):
        return {**_cache["val"], "cached": True}
    logical = disk = files = 0
    # scandir hands back the logical size from the directory entry, so only
    # the on-disk figure needs a syscall per file - half the work os.walk +
    # getsize was doing.
    stack = [root]
    while stack:
        d = stack.pop()
        try:
            with os.scandir(d) as it:
                for e in it:
                    try:
                        if e.is_dir(follow_symlinks=False):
                            stack.append(e.path)
                            continue
                        logical += e.stat(follow_symlinks=False).st_size
                        disk += _on_disk(e.path)
                        files += 1
                    except OSError:
                        continue
        except OSError:
            continue
    val = {"ok": True, "supported": True, "path": root, "files": files,
           "logical": logical, "on_disk": disk,
           "ratio": (logical / disk) if disk else 0.0,
           "saved": max(0, logical - disk)}
    _cache.update(at=now, root=root, val=val)
    return val


def invalidate():
    _cache.update(at=0.0, val=None)


def status():
    with _lock:
        return {"ok": True, **_state}


def _set(**kw):
    with _lock:
        _state.update(kw)


def start(undo=False):
    """Run compact over the mods folder in the background."""
    if not supported():
        return {"ok": False, "error": "NTFS compression is Windows only"}
    root = _mods_dir()
    if not root:
        return {"ok": False, "error": "no mods folder yet"}
    with _lock:
        if _state.get("active"):
            return {"ok": False, "error": "already running"}
        _state.clear()
        _state.update(active=True, phase="measuring", undo=bool(undo))
    threading.Thread(target=_run, args=(root, bool(undo)),
                     daemon=True).start()
    return {"ok": True, "started": True}


def _run(root, undo):
    try:
        before = measure(root, force=True)
        _set(phase="working", before=before)
        # /c compress  /u uncompress  /s recurse  /i ignore errors
        # /f force (revisit files a previous run skipped)
        args = ["compact", "/u" if undo else "/c", "/s", "/i", "/f"]
        if not undo:
            args.append("/exe:LZX")
        args.append(os.path.join(root, "*"))
        p = subprocess.run(args, capture_output=True, text=True,
                           shell=False, cwd=root,
                           creationflags=getattr(subprocess,
                                                 "CREATE_NO_WINDOW", 0))
        tail = (p.stdout or "").strip().splitlines()[-3:]
        after = measure(root, force=True)
        _set(active=False, phase="done", before=before, after=after,
             summary=" ".join(t.strip() for t in tail))
    except Exception as ex:                        # noqa: BLE001
        logs.LOG.warning("compact failed: %s", ex)
        _set(active=False, phase="error", error=str(ex))
