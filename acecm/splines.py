"""Find and ship AI spline files so telemetry and vAI work on every track.

The dedicated server's content.kspkg does not include .aisplinedata. The
client archive does — every stock layout. Custom imports (Barber, Highlands)
keep theirs loose under Saved Games. The engine (and this app) read loose
files from:

    <server>/content/tracks/<folder>/layouts/<layout>.ideal_line.aisplinedata
    <server>/content/tracks/<folder>/layouts/<layout>.pitlane.aisplinedata

So: discover wherever they already are, copy them to that server folder.
Barber is not special — it is an EvoForge folder (`barber_mo_ra`) that
already ships both files; they just never got copied next to the server.
"""
import json
import os
import shutil

from . import config, logs

CACHE = os.path.join(config.DATA, "spline_index.json")


def server_tracks():
    return os.path.join(config.server_dir() or "", "content", "tracks")


def _kind(name):
    n = (name or "").lower()
    if ".ideal_line." in n:
        return "ideal_line"
    if ".pitlane." in n:
        return "pitlane"
    return "other"


def _layout_stem(name):
    return (name or "").split(".")[0]


def _parse_archive_path(path):
    parts = (path or "").replace("/", "\\").split("\\")
    if len(parts) < 5:
        return None
    if parts[0].lower() != "content" or parts[1].lower() != "tracks":
        return None
    fname = parts[-1]
    if not fname.lower().endswith(".aisplinedata"):
        return None
    return {
        "folder": parts[2],
        "layout": _layout_stem(fname),
        "kind": _kind(fname),
        "file": fname,
        "archive": path,
        "size": 0,
        "source": "archive",
    }


def archive_index(refresh=False):
    """Stock splines inside the client's content.kspkg, cached on size+mtime."""
    from . import viewer
    pkg = viewer.package()
    if not pkg:
        return []
    try:
        st = os.stat(pkg)
        stamp = f"{st.st_size}:{int(st.st_mtime)}"
    except OSError:
        return []
    if not refresh:
        try:
            got = json.load(open(CACHE, encoding="utf-8"))
            if got.get("stamp") == stamp:
                return got.get("entries") or []
        except Exception:
            pass
    from . import kspkg
    out = []
    for path, size, _off in kspkg.iter_entries(pkg):
        rec = _parse_archive_path(path)
        if not rec or rec["kind"] == "other":
            continue
        rec["size"] = size
        out.append(rec)
    try:
        os.makedirs(config.DATA, exist_ok=True)
        json.dump({"stamp": stamp, "entries": out},
                  open(CACHE, "w", encoding="utf-8"))
    except OSError:
        pass
    return out


def _walk_layouts(root, source):
    out = []
    if not root or not os.path.isdir(root):
        return out
    for folder in sorted(os.listdir(root)):
        ldir = os.path.join(root, folder, "layouts")
        if not os.path.isdir(ldir):
            continue
        for f in os.listdir(ldir):
            kind = _kind(f)
            if kind == "other":
                continue
            p = os.path.join(ldir, f)
            try:
                size = os.path.getsize(p)
            except OSError:
                size = 0
            out.append({
                "folder": folder, "layout": _layout_stem(f),
                "kind": kind, "file": f, "path": p, "size": size,
                "source": source,
            })
    return out


def imported():
    from . import contentsync
    return _walk_layouts(contentsync.tracks_dir(), "import")


def on_server():
    return _walk_layouts(server_tracks(), "server")


def dest_path(folder, filename):
    folder = (folder or "").strip().replace("/", os.sep).replace("\\", os.sep)
    filename = os.path.basename(filename or "")
    if not folder or not filename or ".." in folder.split(os.sep):
        raise ValueError("bad spline dest")
    return os.path.join(server_tracks(), folder, "layouts", filename)


def _same(src_size, dest):
    try:
        return os.path.isfile(dest) and os.path.getsize(dest) == src_size
    except OSError:
        return False


def _copy(src, dest, size):
    if _same(size, dest):
        return "same"
    os.makedirs(os.path.dirname(dest), exist_ok=True)
    shutil.copy2(src, dest)
    return "copied"


def _extract_many(needed):
    """One pass over the client archive; needed is [{archive, dest, size}]."""
    if not needed:
        return [], []
    from . import kspkg, viewer
    pkg = viewer.package()
    if not pkg:
        return [], []
    want = {n["archive"].lower(): n for n in needed}
    copied, skipped = [], []
    with open(pkg, "rb") as fh:
        for path, size, offset in kspkg.iter_entries(pkg):
            rec = want.get(path.lower())
            if not rec:
                continue
            dest = rec["dest"]
            tag = f"archive:{rec['folder']}/{rec['file']}"
            if _same(size, dest):
                skipped.append(tag)
                continue
            os.makedirs(os.path.dirname(dest), exist_ok=True)
            data = kspkg.read_entry(fh, size, offset, path)
            tmp = dest + ".part"
            with open(tmp, "wb") as out:
                out.write(data)
            os.replace(tmp, dest)
            copied.append(tag)
    return copied, skipped


def ship(folder=None, layout=None):
    """Copy missing splines for one track (or every track) onto the server."""
    folder = (folder or "").strip()
    layout = (layout or "").strip().lower()
    copied, skipped, errors = [], [], []

    # Custom imports first — Barber / Highlands live only here.
    for rec in imported():
        if folder and rec["folder"] != folder:
            continue
        if layout and layout not in rec["layout"].lower() \
                and layout not in rec["file"].lower():
            continue
        try:
            dest = dest_path(rec["folder"], rec["file"])
            st = _copy(rec["path"], dest, rec["size"])
        except Exception as ex:
            errors.append(f"{rec['folder']}/{rec['file']}: {ex}")
            continue
        (skipped if st == "same" else copied).append(
            f"import:{rec['folder']}/{rec['file']}")

    # Stock layouts from the client archive.
    need = []
    for rec in archive_index():
        if folder and rec["folder"] != folder:
            continue
        if layout and layout not in rec["layout"].lower() \
                and layout not in rec["file"].lower():
            continue
        try:
            dest = dest_path(rec["folder"], rec["file"])
        except ValueError as ex:
            errors.append(str(ex))
            continue
        if _same(rec["size"], dest):
            skipped.append(f"archive:{rec['folder']}/{rec['file']}")
            continue
        need.append({**rec, "dest": dest})
    ac, sk = _extract_many(need)
    copied.extend(ac)
    skipped.extend(sk)

    return {
        "ok": not errors,
        "folder": folder or None,
        "layout": layout or None,
        "copied": copied,
        "skipped": skipped,
        "errors": errors,
        "copied_n": len(copied),
        "skipped_n": len(skipped),
    }


def ship_all():
    return ship()


def folders_for(name):
    """Display name or folder -> possible folder ids."""
    name = (name or "").strip()
    if not name:
        return []
    out = []
    try:
        from . import contentsync
        tm = contentsync.track_map() or {}
    except Exception:
        tm = {}
    if name in tm:
        out.append(tm[name])
    slug = name.lower().replace(" ", "_")
    out.append(slug)
    for disp, folder in tm.items():
        if disp.lower() == name.lower() or folder.lower() == slug:
            out.append(folder)
    seen, uniq = set(), []
    for f in out:
        if f and f not in seen:
            seen.add(f)
            uniq.append(f)
    return uniq


def resolve(folder, layout=None, display=None):
    """The ideal-line (and pitlane) to use for this track.

    Looks at the server copy, then the client import, then the archive
    index. Ships a missing pair so the next start finds them locally.
    """
    folders = []
    if folder:
        folders.append(folder)
    folders.extend(folders_for(display or folder or ""))
    seen, order = set(), []
    for f in folders:
        if f and f not in seen:
            seen.add(f)
            order.append(f)
    layout = (layout or "").strip().lower()
    want = layout.replace(" ", "_")

    pools = [("server", on_server()), ("import", imported()),
             ("archive", archive_index())]
    ideal = pit = None
    used_folder = order[0] if order else folder
    for f in order or [folder]:
        cands = []
        for src, rows in pools:
            for r in rows:
                if r["folder"] != f or r["kind"] != "ideal_line":
                    continue
                cands.append({**r, "src": src})
        if not cands:
            continue
        if want:
            hit = [c for c in cands if want in c["layout"].lower()
                   or want in c["file"].lower()]
            pick = (hit or cands)[0]
        else:
            pick = cands[0]
        used_folder = f
        ideal = pick
        # matching pitlane next to it
        pit_name = pick["file"].replace(".ideal_line.", ".pitlane.")
        for src, rows in pools:
            for r in rows:
                if r["folder"] == f and r["file"] == pit_name:
                    pit = {**r, "src": src}
                    break
            if pit:
                break
        break

    if not ideal:
        return {"ok": False, "folder": folder, "layout": layout,
                "error": f"no ideal-line spline for {display or folder}"}

    # Make sure the server has a loose copy — vAI and the tracker both
    # read that folder, not the 25 GB client archive.
    try:
        if ideal.get("source") == "archive" or ideal.get("src") == "archive":
            ship(used_folder, ideal.get("layout"))
        elif ideal.get("path") and not _same(
                ideal.get("size") or 0,
                dest_path(used_folder, ideal["file"])):
            ship(used_folder, ideal.get("layout"))
    except Exception as ex:
        logs.LOG.warning("ship spline %s: %s", used_folder, ex)

    dest = dest_path(used_folder, ideal["file"])
    if os.path.isfile(dest):
        ideal_path = dest
    else:
        ideal_path = ideal.get("path")
    pit_path = None
    if pit:
        pd = dest_path(used_folder, pit["file"])
        pit_path = pd if os.path.isfile(pd) else pit.get("path")
    if not ideal_path or not os.path.isfile(ideal_path):
        return {"ok": False, "folder": used_folder,
                "error": "spline resolved but the file is not on disk"}
    return {
        "ok": True,
        "folder": used_folder,
        "layout": ideal.get("layout"),
        "ideal": ideal_path,
        "pitlane": pit_path,
        "file": ideal["file"],
        "source": ideal.get("src") or ideal.get("source"),
    }


def ship_for_profile(profile):
    """Splines for whatever this server profile is about to host."""
    profile = profile or {}
    custom = (profile.get("custom_track") or "").strip()
    if custom:
        folders = folders_for(custom)
        folder = folders[0] if folders else ""
        layout = ""
        try:
            from . import contentsync, tracks
            src = os.path.join(contentsync.tracks_dir(), folder)
            layout = tracks.read_track_folder(src).get("layout") or ""
        except Exception:
            pass
        return ship(folder, layout)
    try:
        from . import content
        idx = int(profile.get("track_index") or 0)
        evs = content.tracks().get("tracks") or []
        ev = next((t for t in evs if t.get("index") == idx), None) \
            or (evs[idx] if 0 <= idx < len(evs) else {})
        folder = (ev.get("track") or "").lower().replace(" ", "_")
        return ship(folder, ev.get("layout") or "")
    except Exception as ex:
        return {"ok": False, "error": str(ex)}


def status():
    """What the Content page shows."""
    srv = on_server()
    imp = imported()
    try:
        arc = archive_index()
    except Exception as ex:
        logs.LOG.warning("spline archive index: %s", ex)
        arc = []
    on = {(r["folder"], r["file"]) for r in srv}
    missing_imp = [r for r in imp if (r["folder"], r["file"]) not in on]
    missing_arc = [r for r in arc if (r["folder"], r["file"]) not in on]
    folders = sorted({r["folder"] for r in srv + imp + arc})
    ready = 0
    for f in folders:
        kinds = {r["kind"] for r in srv if r["folder"] == f}
        if "ideal_line" in kinds and "pitlane" in kinds:
            ready += 1
    return {
        "ok": True,
        "server": len(srv),
        "imported": len(imp),
        "archive": len(arc),
        "missing": len(missing_imp) + len(missing_arc),
        "missing_imports": [
            f"{r['folder']}/{r['file']}" for r in missing_imp],
        "folders": len(folders),
        "ready_folders": ready,
        "note": ("Stock lines come from the game archive. Custom tracks "
                 "(Barber, Highlands) keep theirs in the import folder. "
                 "Ship copies both next to the dedicated server."),
    }
