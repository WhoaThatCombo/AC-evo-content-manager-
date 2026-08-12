"""Deploying custom tracks onto the dedicated server.

Very different from cars, and worth stating plainly because the difference is
the whole reason this module exists:

  * A car mod is a pair of loose files in the user profile. Drop them in, done.
  * A custom TRACK must go INSIDE the server's content.kspkg. The dedicated
    server has no loose-file path for track logic, and the engine resolves
    content by FNV-1a hash against the archive index - so a brand new track
    path simply cannot be found, no matter where the files sit.

Which is why a custom track is hosted by BORROWING an existing track's slots:
the package maps each of its files onto a host track's paths, and the server
broadcasts those paths to joining clients. The heavy lifting is already done by
`server_track_inject.install_package_to_server()`; this module supervises it and
enforces the safety rules that tool assumes you know.

⚠ Three of those rules bite hard:
  1. The server MUST be stopped - this rewrites a 300 MB archive it holds open.
  2. `penalties_tool.find_server_kspkg()` resolves to the STEAM install, which
     is NOT the portable server we run. The path is passed explicitly here;
     letting it auto-detect patches a server you are not using and leaves the
     real one untouched (a very confusing "nothing happened").
  3. The archive is backed up before the first write, because a failure part
     way through leaves it inconsistent.
"""
import json
import os
import shutil
import subprocess
import sys
import time

from . import config, servers

REQUIRED = ["manifest.json", "containers.bin", "tracks_entry.bin"]


def server_kspkg():
    """The archive of the server WE run - not whatever Steam has."""
    return os.path.join(config.server_dir(), "content.kspkg")


def validate(pkg_dir):
    """Is this a usable track package?"""
    if not pkg_dir or not os.path.isdir(pkg_dir):
        return {"ok": False, "error": "not a folder"}
    missing = [f for f in REQUIRED
               if not os.path.isfile(os.path.join(pkg_dir, f))]
    has_override = os.path.isdir(os.path.join(pkg_dir, "override"))
    info = {"path": pkg_dir, "missing": missing, "override": has_override}
    try:
        man = json.load(open(os.path.join(pkg_dir, "manifest.json"),
                             encoding="utf-8"))
        info["display_name"] = man.get("display_name")
        info["folder"] = man.get("folder")
        info["files"] = len(man.get("slot_map") or {})
        info["host_slots"] = sorted({os.path.dirname(v) for v
                                     in (man.get("slot_map") or {}).values()})[:3]
    except Exception as ex:
        info["error"] = f"manifest unreadable: {ex}"
    if "containers.bin" in missing:
        # the specific failure worth naming: older packages predate it
        info["note"] = ("containers.bin missing - this package was built before "
                        "that file existed and cannot be installed directly; "
                        "rebuild it with build_track_package.py")
    info["ok"] = not missing and has_override and "error" not in info
    return info


def packages(extra=None):
    """Find track packages in the usual places."""
    roots = [os.path.join(config.DATA, "track_packages"),
             os.path.join(config.server_dir(), "track_packages"),
             os.path.expanduser(r"~\Downloads")]
    if extra:
        roots.insert(0, extra)
    seen, out = set(), []
    for root in roots:
        if not os.path.isdir(root):
            continue
        for name in sorted(os.listdir(root)):
            d = os.path.join(root, name)
            if d in seen or not os.path.isdir(d):
                continue
            if os.path.isfile(os.path.join(d, "manifest.json")):
                seen.add(d)
                out.append(validate(d))
    return {"packages": out, "roots": roots}


def _backup(kspkg):
    bak = kspkg + ".bak_pretrack"
    if os.path.exists(bak):
        return {"backup": bak, "made": False}
    shutil.copy2(kspkg, bak)
    return {"backup": bak, "made": True}


def deploy(pkg_dir):
    """Install a track package into the running server's content.kspkg."""
    v = validate(pkg_dir)
    if not v.get("ok"):
        return {"ok": False, "error": "package is not valid", "detail": v}

    # ⚠ Guard first: the server holds content.kspkg open, and writing under it
    # corrupts the archive AND the running session.
    running = [p for p in servers.load() if servers.status(p)["running"]]
    if running or servers._server_pids():
        return {"ok": False,
                "error": "stop the server before deploying a track - it holds "
                         "content.kspkg open and writing under it corrupts both"}

    kspkg = server_kspkg()
    if not os.path.isfile(kspkg):
        return {"ok": False, "error": f"server archive not found: {kspkg}"}
    bak = _backup(kspkg)

    # Run the injector out-of-process: it does sys.path surgery and expects to
    # live next to penalties_tool, so importing it into this app is fragile.
    srv_dir = config.server_dir()
    code = (
        "import sys, json;"
        f"sys.path.insert(0, r'{srv_dir}');"
        "import server_track_inject as sti;"
        f"sti.install_package_to_server(r'{kspkg}', r'{pkg_dir}')"
    )
    t0 = time.time()
    try:
        # ⚠ `python -c` does not exist in a frozen build - sys.executable is
        # ACECM itself and has no -c. Frozen goes through the dispatcher, which
        # imports the injector and calls it with these two paths.
        cmd = (config.tool_cmd("server_track_inject",
                               ["--install", kspkg, pkg_dir])
               if config.FROZEN else [sys.executable, "-c", code])
        r = subprocess.run(cmd, cwd=srv_dir,
                           capture_output=True, text=True, timeout=900)
    except subprocess.TimeoutExpired:
        return {"ok": False, "error": "injector timed out", **bak}
    out = (r.stdout or "") + (r.stderr or "")
    ok = r.returncode == 0
    return {
        "ok": ok,
        "seconds": round(time.time() - t0, 1),
        "output": out.splitlines()[-40:],
        "error": None if ok else f"injector exited {r.returncode}",
        **bak,
        "hint": (None if ok else
                 "the archive was backed up first - use Restore to undo"),
    }


def restore():
    """Put the pre-deploy archive back."""
    kspkg = server_kspkg()
    bak = kspkg + ".bak_pretrack"
    if not os.path.isfile(bak):
        return {"ok": False, "error": "no pre-deploy backup exists"}
    if servers._server_pids():
        return {"ok": False, "error": "stop the server first"}
    shutil.copy2(bak, kspkg)
    return {"ok": True, "restored_from": bak}


def state():
    kspkg = server_kspkg()
    return {
        "kspkg": kspkg,
        "exists": os.path.isfile(kspkg),
        "size_mb": round(os.path.getsize(kspkg) / 1048576, 1)
        if os.path.isfile(kspkg) else 0,
        "backup": os.path.isfile(kspkg + ".bak_pretrack"),
        "server_running": bool(servers._server_pids()),
        # named so the UI can warn before someone patches the wrong install
        "note": "ACECM patches the server it runs, not the Steam copy",
    }
