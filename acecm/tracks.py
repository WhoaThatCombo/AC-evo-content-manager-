"""Deploying custom tracks onto the dedicated server.

Very different from cars, and worth stating plainly because the difference is
the whole reason this module exists:

  * A car mod is a pair of loose files in the user profile. Drop them in, done.
  * A custom TRACK must go INSIDE the server's content.kspkg. The dedicated
    server has no loose-file path for track logic, so the files themselves have
    to live in the archive.

There are two ways to do that, and `deploy_native` is the one to prefer.

NATIVE (deploy_native) installs the track at its OWN paths. It was long assumed
impossible because added archive entries seemed invisible, but that turned out
to be a malformed record header plus a table that must stay sorted - see
kspkg_write. Proven end to end: the server broadcasts the track's real paths
and a joining client resolves them out of its loose EvoForge import, because a
brand new path is an ABSENCE and loose files fill absences. Stock tracks stay
stock, several custom tracks coexist, and joiners install nothing extra.

BORROWED (deploy) is the old way, kept as a fallback: the package maps its
files onto a host track's paths via
`server_track_inject.install_package_to_server()`. While installed, Road
Atlanta IS the custom track, only one can exist at a time, and every joining
client must run install_track.py against its own archive.

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

from . import config, kspkg_write, logs, servers, tracktables

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


def importable():
    """Tracks already imported on this machine, ready to deploy as-is.

    ⚠ These are the obvious thing to host and were not offered anywhere. The
    deploy list only ever showed slot-borrow PACKAGES built by
    build_track_package.py, so a track imported in EvoForge - the normal way
    anyone gets one - could not be deployed from the UI at all, while the
    error for a missing track said "deploy it from Content".

    A native install needs nothing more than this folder: root .scene, .track
    and containers\\ are all here.
    """
    out = []
    try:
        from . import contentsync
        root = contentsync.tracks_dir()
        known = {f: n for n, f in (contentsync.track_map() or {}).items()}
        for folder in contentsync.installed_tracks():
            src = os.path.join(root, folder)
            item = {"path": src, "folder": folder, "source": "imported",
                    "display_name": known.get(folder) or folder}
            try:
                info = read_track_folder(src)
                item["layout"] = info["layout"]
                item["files"] = len(info["files"])
                item["ok"] = True
            except Exception as ex:
                item["ok"] = False
                item["error"] = str(ex)
            out.append(item)
    except Exception as ex:
        logs.LOG.warning("importable tracks: %s", ex)
    return out


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


# ------------------------------------------------------------------ native --
# The package ships its files twice over: `track/<folder>/` holds them at their
# own paths (this is what a player imports client-side) and `override/` holds
# the same bytes under the HOST track's paths for a slot-borrow install. The
# file CONTENTS already reference native paths either way - only the archive
# slot differed - so a native deploy needs no content rewriting at all.
TEMPLATE_TRACK = "Road Atlanta"


def client_track_folders():
    """Track folders players have imported loose (EvoForge writes these)."""
    d = os.path.join(os.path.expanduser("~"), "Saved Games", "ACE", "mods",
                     "content", "tracks")
    if not os.path.isdir(d):
        return []
    return sorted(n for n in os.listdir(d) if os.path.isdir(os.path.join(d, n)))


def _package_files(pkg_dir, folder, own_folder):
    """archive path -> bytes, for the track's logic files.

    ⚠ A .scene names its containers INTERNALLY. Installing under a different
    folder therefore has to rewrite the file contents too - miss that and the
    server crashes with `Failed to find file` on a path nothing else mentions.
    Only an equal-length rename is safe as a byte substitution; anything else
    would need the protobuf length prefixes rebuilt, so it is refused rather
    than silently corrupting a 1.4 MB scene.
    """
    man = json.load(open(os.path.join(pkg_dir, "manifest.json"),
                        encoding="utf-8"))
    rename = folder != own_folder
    if rename and len(folder) != len(own_folder):
        raise ValueError(
            f"cannot install {own_folder!r} as {folder!r}: names differ in "
            f"length ({len(own_folder)} vs {len(folder)}), and scene files "
            f"reference their own folder internally")
    out = {}
    for rel, host in (man.get("slot_map") or {}).items():
        src = os.path.join(pkg_dir, "override", host.replace("\\", os.sep))
        if not os.path.isfile(src):
            src = os.path.join(pkg_dir, "track", own_folder,
                               rel.replace("\\", os.sep))
        if not os.path.isfile(src):
            raise FileNotFoundError(f"package is missing {rel}")
        blob = open(src, "rb").read()
        if rename:
            blob = blob.replace(own_folder.encode(), folder.encode())
            # ⚠ the root .scene/.track are NAMED after the folder, so the
            # relative path has to be renamed too - installing
            # drift_0y3j31.scene inside drift_2hwp80\ leaves the table row
            # pointing at a file that is not there
            rel = rel.replace(own_folder, folder)
        out[f"content\\tracks\\{folder}\\{rel}"] = blob

    # ⚠ slot_map does NOT list the .track - a slot-borrow install inherits the
    # host's one and never needs its own. A native install does: tracks.table
    # points at content\tracks\<folder>\<folder>.track, and without this the
    # row names a file that is not in the archive. It went unnoticed because
    # the first archive we tried already had one left by an older injection.
    dst = f"content\\tracks\\{folder}\\{folder}.track"
    if dst not in out:
        host = man.get("host_track") or ""
        cands = [os.path.join(pkg_dir, "track", own_folder,
                              own_folder + ".track")]
        if host:
            cands.insert(0, os.path.join(pkg_dir, "override",
                                         host.replace("\\", os.sep)))
        for src in cands:
            if os.path.isfile(src):
                blob = open(src, "rb").read()
                if rename:
                    blob = blob.replace(own_folder.encode(), folder.encode())
                out[dst] = blob
                break
        else:
            raise FileNotFoundError(
                f"package has no .track for {own_folder} (looked in "
                f"override\\{host} and track\\{own_folder}\\)")
    return out


# A container's role, from its file name, for tracks imported as loose folders
# (which carry no slot_map to state it).
_ROLE_BY_NAME = {
    "ground_grass": "race_scenery",
    "spawnpoints_grid": "spawnpoints_grid",
    "spawnpoints_hotlap": "spawnpoints_hotlap",
    "spawnpoints_pitlane": "spawnpoints_pitlane",
    "timelines": "timelines",
}


def _package_roles(man):
    """role -> container file name, for a slot-borrow package.

    ⚠ Take the role from the HOST path the package borrowed, not from the
    track's own file name. The package already states the correspondence:
    `containers\\ground_grass.scene -> ...\\containers\\race_scenery_gp.scene`
    says ground_grass IS the scenery. Deriving the role from "ground_grass"
    instead invents a role no template row mentions, and the container is
    silently dropped - the track loads with no scenery.
    """
    out = {}
    for rel, host in (man.get("slot_map") or {}).items():
        if not rel.startswith("containers\\") or not rel.endswith(".scene"):
            continue
        role = tracktables._role(host.rsplit("\\", 1)[-1][:-len(".scene")])
        out[role] = rel.rsplit("\\", 1)[-1][:-len(".scene")]
    return out


def read_track_folder(src_dir, folder=None):
    """Treat an imported track folder as an installable source.

    EvoForge writes exactly what the server needs - root .scene, .track and
    containers\\ - so a native install can read it straight off disk. No
    slot-borrow package, and no chance of the folder name disagreeing with what
    clients hold, because it IS what clients hold.
    """
    own = os.path.basename(os.path.normpath(src_dir))
    folder = folder or own
    root = os.path.join(src_dir, own + ".scene")
    data = os.path.join(src_dir, own + ".track")
    cdir = os.path.join(src_dir, "containers")
    if not os.path.isfile(root) or not os.path.isfile(data):
        raise FileNotFoundError(f"{src_dir} has no {own}.scene / {own}.track")
    if folder != own and len(folder) != len(own):
        raise ValueError(f"cannot install {own!r} as {folder!r}: names differ "
                         f"in length and scenes reference their own folder")

    def load(p):
        b = open(p, "rb").read()
        return b.replace(own.encode(), folder.encode()) if folder != own else b

    files = {f"content\\tracks\\{folder}\\{folder}.scene": load(root),
             f"content\\tracks\\{folder}\\{folder}.track": load(data)}
    containers, layout = {}, None
    for name in sorted(os.listdir(cdir)) if os.path.isdir(cdir) else []:
        if not name.endswith(".scene"):
            continue
        stem = name[:-len(".scene")]
        files[f"content\\tracks\\{folder}\\containers\\{name}"] = \
            load(os.path.join(cdir, name))
        if stem.startswith("layout_"):
            # layout_layout_drift.scene -> the layout is "layout_drift"
            containers["layout"] = stem
            layout = stem[len("layout_"):]
        elif stem in _ROLE_BY_NAME:
            containers[_ROLE_BY_NAME[stem]] = stem
    if not layout:
        raise ValueError(f"{src_dir} has no layout_*.scene container")
    return {"folder": folder, "own_folder": own, "layout": layout,
            "files": files, "containers": containers}


def deploy_native(pkg_dir, dry_run=False, folder=None, display_name=None):
    """Install a track at its OWN paths - no stock track is overwritten.

    The clean alternative to slot-borrowing, proven end to end: the server
    broadcasts the track's real paths, and a joining client resolves them from
    its loose EvoForge import because a brand new path is an ABSENCE, and loose
    files fill absences. Joiners install nothing extra and every stock track
    survives.

    ⚠ The folder name must match what clients already have on disk. The server
    broadcasts paths, not names, so a server saying `barber_mo_ra` and a client
    holding `alabama_r_4e` can never meet.
    """
    if os.path.isfile(os.path.join(pkg_dir, "manifest.json")):
        v = validate(pkg_dir)
        if not v.get("ok"):
            return {"ok": False, "error": "package is not valid", "detail": v}
    elif not os.path.isdir(pkg_dir):
        return {"ok": False, "error": f"not a folder: {pkg_dir}"}
    if servers._server_pids():
        return {"ok": False,
                "error": "stop the server before deploying a track - it holds "
                         "content.kspkg open and writing under it corrupts both"}

    # Either a slot-borrow package, or a track folder as EvoForge imported it.
    is_pkg = os.path.isfile(os.path.join(pkg_dir, "manifest.json"))
    if is_pkg:
        man = json.load(open(os.path.join(pkg_dir, "manifest.json"),
                             encoding="utf-8"))
        own_folder = man.get("folder")
        display = display_name or man.get("display_name")
        layout = man.get("layout") or "layout"
        if not own_folder or not display:
            return {"ok": False,
                    "error": "manifest needs folder and display_name"}
    else:
        try:
            src = read_track_folder(pkg_dir, folder)
        except Exception as ex:
            return {"ok": False, "error": str(ex)}
        own_folder = src["own_folder"]
        layout = src["layout"]
        # ⚠ Default to the name CLIENTS already use for this folder, from their
        # own tracks.table. The lobby advertises whatever name the server was
        # given, and joiners match on it - so deploying "Highlands" when every
        # client calls it "Highlands Drift" tells them they are missing a track
        # they have. Guessing a name from the folder has the same failure.
        display = display_name or _client_name(own_folder) \
            or own_folder.rsplit("_", 1)[0].replace("_", " ").title()
    # ⚠ The server broadcasts PATHS, so its folder name must be the one clients
    # already hold. A package built as drift_0y3j31 cannot serve a client that
    # imported drift_2hwp80 - same track, different folder, no join.
    folder = folder or own_folder
    installed = client_track_folders()
    mismatch = bool(installed) and folder not in installed

    kspkg = server_kspkg()
    if not os.path.isfile(kspkg):
        return {"ok": False, "error": f"server archive not found: {kspkg}"}

    if not is_pkg:
        files, containers = src["files"], src["containers"]
    else:
        try:
            files = _package_files(pkg_dir, folder, own_folder)
        except Exception as ex:
            return {"ok": False, "error": str(ex)}
        containers = _package_roles(man)

    # role -> this track's container file name.

    tk = kspkg_write.read_entry(kspkg, "system\\tracks.table")
    tc = kspkg_write.read_entry(kspkg, "system\\track_containers.table")
    if tk is None or tc is None:
        return {"ok": False, "error": "archive has no system tables"}
    try:
        new_tk = tracktables.upsert_track_row(tk, display, folder,
                                              TEMPLATE_TRACK)
        new_tc, modes = tracktables.upsert_container_rows(
            tc, display, folder, layout, containers, TEMPLATE_TRACK)
    except Exception as ex:
        return {"ok": False, "error": f"table edit failed: {ex}"}

    plan = {
        "ok": True, "folder": folder, "package_folder": own_folder,
        "renamed": folder != own_folder,
        "client_folders": installed,
        "warning": (f"no client here has imported {folder!r} - joiners whose "
                    f"folder differs cannot load it") if mismatch else None,
        "display_name": display,
        "layout": layout, "files": sorted(files),
        "containers": containers, "modes": modes,
        "tracks_table": f"{len(tk)} -> {len(new_tk)} B",
        "containers_table": f"{len(tc)} -> {len(new_tc)} B",
    }
    if dry_run:
        return plan

    bak = _backup(kspkg)
    changes = dict(files)
    changes["system\\tracks.table"] = new_tk
    changes["system\\track_containers.table"] = new_tc

    tmp = kspkg + ".native_tmp"
    t0 = time.time()
    try:
        res = kspkg_write.write_archive(kspkg, tmp, changes)
        check = kspkg_write.verify(tmp)
        if not check["ok"]:
            os.remove(tmp)
            return {"ok": False, "error": "written archive failed verification",
                    "detail": check, **bak}
        os.replace(tmp, kspkg)
    except Exception as ex:
        if os.path.isfile(tmp):
            os.remove(tmp)
        return {"ok": False, "error": str(ex), **bak,
                "hint": "the archive was backed up first - use Restore to undo"}
    # Publish it, so joining players can actually get the content.
    # ⚠ Automatic on purpose. The host knows the folder, the display name and
    # the layout at exactly this moment; expecting them to retype it into a
    # registry entry afterwards means most servers would advertise a track
    # nobody can download, and the failure would surface on someone ELSE'S
    # machine as a join they cannot explain.
    published = None
    try:
        published = _publish(display, folder)
    except Exception as ex:
        logs.LOG.warning("could not publish %s for download: %s", folder, ex)

    return {**plan, "seconds": round(time.time() - t0, 1),
            "written": res, "verified": check, "published": published, **bak}


def _client_name(folder):
    """What the local client's tracks.table calls this folder, if anything."""
    try:
        from . import contentsync
        for name, f in contentsync.track_map().items():
            if f == folder:
                return name
    except Exception as ex:
        logs.LOG.info("client track name for %s: %s", folder, ex)
    return ""


def _publish(display, folder):
    """Add or refresh this track in the share registry, keeping it deduped."""
    from . import registry
    for e in registry.load():
        if folder in (e.get("required_tracks") or []):
            return {"id": e["id"], "new": False}
    entry = registry.upsert({
        "name": f"{display} (hosted here)",
        "description": f"Content for {display} - installed by ACECM",
        "ip": "", "port": 9700,
        "required_tracks": [folder],
        "public": True,
    })
    return {"id": entry["id"], "new": True}


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
