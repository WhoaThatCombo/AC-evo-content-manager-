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


def _arc(path):
    """Archive keys are ALWAYS lowercase.

    ⚠ The engine finds a file by hashing its path (FNV-1a-64 over UTF-16LE)
    and it lowercases before hashing: across a stock client and server archive,
    not ONE of 137,219 records contains a capital letter. A container read off
    disk as "layout_Brooklyn Park.scene" therefore went into the package with
    capitals, hashed to a key the engine never asks for, and the server died
    with "Failed to find file" for a file that was present, in-bounds and
    readable - which is about as misleading as an error can be.

    Only the archive KEY is lowered. The file is still read from its real name
    on disk, and table rows keep their own casing (layout matching there IS
    case-sensitive).
    """
    return path.lower()


def server_kspkg():
    """The archive of the server WE run - not whatever Steam has."""
    return os.path.join(config.server_dir(), "content.kspkg")


_IN_PKG = {"stamp": None, "folders": set()}


def in_server_package(folder):
    """Is this track already inside the server's own archive?

    Cached on the archive's size+mtime: the index is 64 MiB at the end of a
    multi-hundred-megabyte file, so asking once per profile per page render
    would be pointlessly expensive.
    """
    pkg = server_kspkg()
    if not folder or not os.path.isfile(pkg):
        return False
    try:
        st = os.stat(pkg)
        stamp = (st.st_size, int(st.st_mtime))
    except OSError:
        return False
    if _IN_PKG["stamp"] != stamp:
        found = set()
        try:
            from . import kspkg
            for p, _s, _o in kspkg.iter_entries(pkg):
                low = p.lower().replace("/", "\\")
                if low.startswith("content\\tracks\\"):
                    rest = low[len("content\\tracks\\"):]
                    if "\\" in rest:
                        found.add(rest.split("\\", 1)[0])
        except Exception as ex:
            logs.LOG.info("in_server_package: %s", ex)
            return False
        _IN_PKG.update(stamp=stamp, folders=found)
    return folder.lower() in _IN_PKG["folders"]


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
        out[_arc(f"content\\tracks\\{folder}\\{rel}")] = blob

    # ⚠ slot_map does NOT list the .track - a slot-borrow install inherits the
    # host's one and never needs its own. A native install does: tracks.table
    # points at content\tracks\<folder>\<folder>.track, and without this the
    # row names a file that is not in the archive. It went unnoticed because
    # the first archive we tried already had one left by an older injection.
    dst = _arc(f"content\\tracks\\{folder}\\{folder}.track")
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

    files = {_arc(f"content\\tracks\\{folder}\\{folder}.scene"): load(root),
             _arc(f"content\\tracks\\{folder}\\{folder}.track"): load(data)}
    containers, layout = {}, None
    for name in sorted(os.listdir(cdir)) if os.path.isdir(cdir) else []:
        if not name.endswith(".scene"):
            continue
        stem = name[:-len(".scene")]
        files[_arc(f"content\\tracks\\{folder}\\containers\\{name}")] = \
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

    # Loose AI lines. The archive cannot see a brand-new .aisplinedata
    # path, but the VFS will load one from disk. Barber already has both
    # files in its import folder; copy them next to the server.
    shipped = None
    try:
        from . import splines
        src_layouts = os.path.join(pkg_dir, "layouts")
        if os.path.isdir(src_layouts):
            n = 0
            for f in os.listdir(src_layouts):
                if not f.endswith(".aisplinedata"):
                    continue
                dest = splines.dest_path(folder, f)
                os.makedirs(os.path.dirname(dest), exist_ok=True)
                shutil.copy2(os.path.join(src_layouts, f), dest)
                n += 1
            shipped = {"copied": n, "from": src_layouts}
        else:
            shipped = splines.ship(folder)
    except Exception as ex:
        logs.LOG.warning("deploy spline copy %s: %s", folder, ex)
        shipped = {"ok": False, "error": str(ex)}

    return {**plan, "seconds": round(time.time() - t0, 1),
            "written": res, "verified": check, "published": published,
            "splines": shipped, **bak}


def _display_name_for(folder):
    """One canonical display name per folder, used everywhere a track gets
    (re)registered.

    Three different fallbacks used to disagree - deploy_native stripped the
    trailing random suffix before title-casing, pack_meta title-cased the
    whole folder name including the suffix, and an early redeclare_tracks
    used the raw folder untouched. The same track ended up registered under
    two or three different names across the client and server archives,
    which is indistinguishable from "different tracks" to anything that
    looks it up by name (a saved profile, the picker, _custom_track_problem).

    ⚠ Deliberately does NOT prefer whatever tracks.table already calls this
    folder. That table is exactly what a content update wipes and what the
    naming bug above wrongly re-populated - trusting it here just picks
    whichever wrong name got written most recently instead of fixing it.

    ⚠ Stripping only the trailing "_xxxx" EvoForge suffix collides for real:
    shutoko_r_vq and shutoko_r_x0 (Tatsumi PA and Daishi PA - two different
    tracks) both strip down to "Shutoko R". upsert_track_row keys by display
    name, so a collision here is not a cosmetic problem, it is one track's
    row silently overwriting the other's on the next write. Any folder whose
    stripped name is not unique among all locally known folders keeps its
    full name instead, even though that is uglier.
    """
    def strip(f):
        return f.rsplit("_", 1)[0].replace("_", " ").title()

    name = strip(folder)
    others = [f for f in client_track_folders()
             if f != folder and f != "common_assets" and strip(f) == name]
    if others:
        return folder.replace("_", " ").title()
    return name


def fix_track_naming(dry_run=False):
    """Consolidate every folder onto ONE display name on BOTH archives.

    Finds every row - correctly named or not - for each loose track folder,
    removes all of them, and re-registers exactly once under
    _display_name_for()'s canonical name. Cheap to run any time; a folder
    already registered correctly under the right name is a no-op.
    """
    canonical = {f: _display_name_for(f) for f in client_track_folders()
                if f != "common_assets"}
    plan = []
    changes = {"server": {}, "client": {}}
    for side, kspkg_path in (("server", server_kspkg()), ("client", client_kspkg())):
        if not kspkg_path or not os.path.isfile(kspkg_path):
            continue
        tk = kspkg_write.read_entry(kspkg_path, "system\\tracks.table")
        tc = kspkg_write.read_entry(kspkg_path, "system\\track_containers.table")
        if tk is None or tc is None:
            continue
        for folder, want in canonical.items():
            have = tracktables.rows_for_folder(tk, folder)
            stray = [n for n in have if n != want]
            if not stray and want in have:
                continue
            plan.append({"side": side, "folder": folder, "want": want,
                        "removing": stray})
            for name in stray:
                tk = tracktables.remove_track_row(tk, name)
                tc = tracktables.remove_container_rows(tc, name)
            changes[side][folder] = (tk, tc)
    if dry_run:
        return {"ok": True, "dry_run": True, "plan": plan}
    if not plan:
        return {"ok": True, "plan": [], "note": "every folder already has one consistent name"}
    if servers._server_pids():
        return {"ok": False, "error": "stop the server before fixing track "
                                      "names - it holds content.kspkg open"}
    from . import winproc
    if winproc.pids_named("AssettoCorsaEVO"):
        return {"ok": False, "needs_close": True,
                "error": "close the game before fixing client track names"}

    # Persist the stray-removed tables FIRST - deploy_native() and
    # register_client_track() each re-read tracks.table fresh off disk, so
    # the in-memory removal computed above is invisible to them until it is
    # actually written. Skipping this step silently discards every removal.
    if changes["server"]:
        tk_final, tc_final = None, None
        for tk, tc in changes["server"].values():
            tk_final, tc_final = tk, tc      # each folder's edit builds on the last
        kspkg_write.write_archive(server_kspkg(), server_kspkg() + ".fixnames_tmp",
                                  {"system\\tracks.table": tk_final,
                                   "system\\track_containers.table": tc_final})
        os.replace(server_kspkg() + ".fixnames_tmp", server_kspkg())
    if changes["client"]:
        tk_final, tc_final = None, None
        for tk, tc in changes["client"].values():
            tk_final, tc_final = tk, tc
        kspkg_write.write_inplace(client_kspkg(),
                                  {"system\\tracks.table": tk_final,
                                   "system\\track_containers.table": tc_final})

    results = []
    for folder, want in canonical.items():
        if folder in changes["server"]:
            src = os.path.join(os.path.expanduser("~"), "Saved Games", "ACE",
                               "mods", "content", "tracks", folder)
            r = deploy_native(src, folder=folder, display_name=want)
            results.append({"side": "server", "folder": folder, "ok": r.get("ok", False)})
        if folder in changes["client"]:
            r = register_client_track(folder, {"display_name": want})
            results.append({"side": "client", "folder": folder, "ok": r.get("ok", False)})
    ok = all(r["ok"] for r in results)
    return {"ok": ok, "fixed": [r for r in results if r["ok"]],
            "failed": [r for r in results if not r["ok"]]}


def redeclare_tracks(dry_run=False):
    """Re-register every loose custom track that a content update wiped.

    A Kunos update replaces content.kspkg wholesale - it doesn't merge, it
    overwrites - so any tracks.table / track_containers.table rows deploy_native
    wrote are gone even though the track's own files are untouched (they live
    under their own path, not inside a stock folder). EvoForge re-injects rows
    like this on every update; without it a server that survived ten patches
    can go silently trackless on the eleventh. This is the same fix, built in,
    for people who don't have EvoForge installed.

    Only loose folders under the CLIENT's import path are candidates - that is
    where EvoForge writes finished conversions, and it is the only place we
    can recover a display name and layout from without asking the user.
    """
    kspkg = server_kspkg()
    if not os.path.isfile(kspkg):
        return {"ok": False, "error": f"server archive not found: {kspkg}"}
    if servers._server_pids():
        return {"ok": False,
                "error": "stop the server before redeclaring tracks - it "
                         "holds content.kspkg open and writing under it "
                         "corrupts both"}

    tk = kspkg_write.read_entry(kspkg, "system\\tracks.table")
    if tk is None:
        return {"ok": False, "error": "archive has no system\\tracks.table"}
    try:
        registered = tracktables.registered_names(tk)
    except Exception as ex:
        return {"ok": False, "error": f"could not read tracks.table: {ex}"}

    missing = []
    for folder in client_track_folders():
        if folder == "common_assets":
            continue
        display = _display_name_for(folder)
        if display in registered:
            continue
        src = os.path.join(os.path.expanduser("~"), "Saved Games", "ACE",
                           "mods", "content", "tracks", folder)
        try:
            info = read_track_folder(src, folder)
        except Exception as ex:
            missing.append({"folder": folder, "display_name": display,
                            "ok": False, "error": str(ex)})
            continue
        missing.append({"folder": folder, "display_name": display,
                        "own_folder": info["own_folder"],
                        "layout": info["layout"], "ok": None})

    if dry_run:
        return {"ok": True, "dry_run": True, "checked": len(missing),
                "candidates": missing}

    results = []
    for m in missing:
        if m["ok"] is False:
            results.append(m)
            continue
        r = deploy_native(os.path.join(os.path.expanduser("~"), "Saved Games",
                                       "ACE", "mods", "content", "tracks",
                                       m["folder"]),
                          folder=m["folder"], display_name=m["display_name"])
        results.append({**m, "ok": r.get("ok", False), "detail": r})

    ok = all(r["ok"] is not False for r in results)
    return {"ok": ok, "redeclared": [r for r in results if r["ok"]],
            "failed": [r for r in results if r["ok"] is False],
            "already_registered": len(client_track_folders()) - len(missing)}


def redeclare_client_tracks(dry_run=False):
    """The client-side twin of redeclare_tracks().

    A content update wipes the CLIENT's own tracks.table exactly the same
    way it wipes the server's - they are two separate archives, so fixing one
    says nothing about the other. Single-player Practice/Custom Session reads
    the client's own table to resolve a track name to paths; a row missing
    there is not a "not found" in the UI, it is "Trying to load a message
    with an empty path" and a crash, because the game already committed to
    starting before discovering the path is empty.
    """
    pkg = client_kspkg()
    if not pkg:
        return {"ok": False, "error": "client content.kspkg not found"}
    from . import winproc
    if winproc.pids_named("AssettoCorsaEVO"):
        return {"ok": False, "needs_close": True,
                "error": "close the game before redeclaring client tracks - "
                         "content.kspkg is locked while it is running"}

    tk = kspkg_write.read_entry(pkg, "system\\tracks.table")
    if tk is None:
        return {"ok": False, "error": "client archive has no system\\tracks.table"}
    try:
        registered = tracktables.registered_names(tk)
    except Exception as ex:
        return {"ok": False, "error": f"could not read tracks.table: {ex}"}

    missing = [f for f in client_track_folders()
              if f != "common_assets" and (_client_name(f) or f) not in registered]

    if dry_run:
        return {"ok": True, "dry_run": True, "candidates": missing}

    results = []
    for folder in missing:
        r = register_client_track(folder)
        results.append({"folder": folder, "ok": r.get("ok", False), "detail": r})

    ok = all(r["ok"] for r in results)
    return {"ok": ok, "redeclared": [r for r in results if r["ok"]],
            "failed": [r for r in results if not r["ok"]],
            "already_registered": len(client_track_folders()) - len(missing)}


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


def client_kspkg():
    from . import viewer
    return viewer.package() or ""


def pack_meta(folder):
    """Sidecar the joiner needs to register this track in THEIR tables."""
    display = _display_name_for(folder)
    layout, containers = "layout", {}
    try:
        from . import contentsync
        src = read_track_folder(os.path.join(contentsync.tracks_dir(), folder),
                                folder)
        layout = src.get("layout") or layout
        containers = src.get("containers") or {}
        display = _client_name(folder) or src.get("display_name") or display
    except Exception as ex:
        logs.LOG.info("pack meta for %s: %s", folder, ex)
    return {"folder": folder, "display_name": display,
            "layout": layout, "containers": containers}


def missing_from_client(folder=None):
    """Imported tracks whose FILES are here but which the client table lost.

    A game update replaces content.kspkg, and with it the tracks.table rows
    that made imported tracks selectable. The art is untouched in the mods
    folder, so the track is not gone - the game has just forgotten it. This is
    what tells the difference: on disk under mods, absent from the table.
    """
    from . import contentsync
    try:
        known = set(contentsync.track_map().values())
    except Exception:
        known = set()
    here = [t.get("folder") for t in importable() if t.get("folder")]
    miss = [f for f in here if f not in known]
    return [f for f in miss if not folder or f == folder]


def restore_imported(only=None, progress=None):
    """Re-register every imported track the client table has forgotten.

    The one action a game update needs for modded content: it walks the tracks
    already in the mods folder and writes each back into the fresh client
    table. Idempotent - a track the table still knows is skipped, so running it
    when nothing is wrong does nothing.

    ⚠ Client side only. Putting a track back into the SERVER package is
    deploy_native and happens per hosted track at server start - re-deploying
    all nineteen here would rewrite a 64 MiB index nineteen times for tracks
    most people do not host.
    """
    from . import winproc
    if winproc.pids_named("AssettoCorsaEVO"):
        return {"ok": False, "needs_close": True,
                "error": "close the game first - restoring tracks writes "
                         "content.kspkg, which is locked while it runs"}
    want = missing_from_client()
    if only:
        want = [f for f in want if f in set(only)]
    done, failed = [], []
    for i, folder in enumerate(want):
        if progress:
            progress(i, len(want), folder)
        r = register_client_track(folder)
        (done if r.get("ok") else failed).append(
            folder if r.get("ok") else {"folder": folder,
                                        "error": r.get("error")})
    if want:
        try:
            from . import contentsync
            contentsync.track_map(refresh=True)
        except Exception:
            pass
    return {"ok": not failed, "restored": done, "failed": failed,
            "checked": len(want)}


def register_client_track(folder, meta=None):
    """Write this track into the CLIENT's tracks.table + containers table.

    Fetch used to drop only the art folder, so ACECM still said the track
    was missing (track_map reads the table) and the game never listed it
    like an EvoForge import. Same upsert as a native deploy, applied to
    the client's content.kspkg, in-place so we do not copy 25 GB.
    """
    from . import winproc
    folder = (folder or "").strip()
    if not folder:
        return {"ok": False, "error": "no folder"}
    meta = dict(meta or {})
    if not meta.get("display_name") or not meta.get("containers"):
        try:
            meta = {**pack_meta(folder), **{k: v for k, v in meta.items() if v}}
        except Exception:
            pass
    display = (meta.get("display_name") or "").strip()
    layout = (meta.get("layout") or "layout").strip()
    containers = meta.get("containers") or {}
    if not display:
        display = _display_name_for(folder)

    pkg = client_kspkg()
    if not pkg:
        return {"ok": False, "error": "client content.kspkg not found"}
    if winproc.pids_named("AssettoCorsaEVO"):
        return {"ok": False, "needs_close": True,
                "error": "close the game, then fetch again — content.kspkg "
                         "is locked while it is running"}

    tk = kspkg_write.read_entry(pkg, "system\\tracks.table")
    tc = kspkg_write.read_entry(pkg, "system\\track_containers.table")
    if tk is None or tc is None:
        return {"ok": False, "error": "client archive has no system tables"}
    try:
        new_tk = tracktables.upsert_track_row(tk, display, folder,
                                              TEMPLATE_TRACK)
        new_tc, modes = tracktables.upsert_container_rows(
            tc, display, folder, layout, containers or {"layout": "layout"},
            TEMPLATE_TRACK)
    except Exception as ex:
        return {"ok": False, "error": f"table edit failed: {ex}"}

    try:
        written = kspkg_write.write_inplace(
            pkg, {"system\\tracks.table": new_tk,
                  "system\\track_containers.table": new_tc})
        check = kspkg_write.verify(pkg)
        # ⚠ verify() checks all ~120k records, most of which we never touched.
        # A pre-existing header quirk on some unrelated texturemips (seen on
        # this build's own content, before any of our writes) used to fail
        # this whole call even though sorted/unique - the properties an
        # in-place table write can actually break - both held. Only the sites
        # we just wrote are grounds to call OUR write bad; anything else is
        # someone else's problem to log, not ours to block on.
        ours = {"system\\tracks.table", "system\\track_containers.table"}
        our_bad = [p for p in check.get("bad_headers", []) if p in ours]
        if not check.get("sorted") or not check.get("unique") or our_bad:
            return {"ok": False, "error": "client archive failed verification "
                    "after the table write", "detail": check}
        if check.get("bad_headers"):
            logs.LOG.warning("client content.kspkg has %d pre-existing "
                             "header quirk(s) unrelated to this write: %s",
                             len(check["bad_headers"]), check["bad_headers"])
    except Exception as ex:
        return {"ok": False, "error": str(ex)}

    cache = os.path.join(config.DATA, "track_map.json")
    try:
        if os.path.isfile(cache):
            os.remove(cache)
    except OSError:
        pass
    logs.LOG.info("registered client track %r as %r (modes=%s)",
                  folder, display, modes)
    return {"ok": True, "folder": folder, "display_name": display,
            "layout": layout, "modes": modes, "written": written}


def _publish(display, folder):
    """Add or refresh this track in the share registry, keeping it deduped."""
    from . import registry
    for e in registry.load():
        if folder in (e.get("required_tracks") or []):
            return {"id": e["id"], "new": False}
    # ⚠ No ip/port here. Hardcoding them wrote ip='' and port=9700 into every
    # entry a track import created, so a host with twenty tracks published
    # twenty entries with no address and the same port - registry.upsert
    # fills both in from this machine and its server profile.
    entry = registry.upsert({
        "name": f"{display} (hosted here)",
        "description": f"Content for {display} - installed by ACECM",
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
