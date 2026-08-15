"""Installing and removing server content.

Two very different jobs, because the game treats them differently:

CARS - the dedicated server reads mods from the USER PROFILE, not from its own
install directory:

    %USERPROFILE%\\Saved Games\\ACE-Server\\mods\\

and each mod needs BOTH files:
    <mod>.kspkg   the content itself
    <mod>.json    declares the cars inside it

⚠ The .json is what makes a car selectable as server content. A mod exported
without one installs "successfully" and then simply never appears in the
allowed-cars list, which looks like a server bug rather than a missing file.

TRACKS - two separate things, and conflating them is a mistake:
  * AI SPLINES (.aisplinedata) DO work as loose files under
    content/tracks/<track>/layouts/ - the server package ships none, and the
    vAI cars only run because the VFS falls back to loose files for these.
  * TRACK CONTENT itself does NOT. The dedicated server has no loose-file path
    for track logic; everything must live inside its content.kspkg, and the
    engine resolves by FNV-1a hash against the archive index so a brand new
    path cannot be found at all. Custom tracks are therefore installed by
    borrowing an existing track's slots - see tracks.py.
"""
import json
import os
import shutil
import zipfile

from . import config

# Fallbacks only. The real locations are detected - "Saved Games" is a known
# folder and can be relocated, so building these paths by hand quietly points
# at nothing on a machine that moved it.
MODS = os.path.join(os.path.expanduser("~"), "Saved Games", "ACE-Server", "mods")
# The CLIENT keeps its mods somewhere else entirely. Both sides need the same
# .kspkg + .json pair, and they drift apart easily: a car present on the server
# but missing its .json on the client cannot be selected by the player, which
# looks like the mod "not working" rather than one 244-byte file being absent.
CLIENT_MODS = os.path.join(os.path.expanduser("~"), "Saved Games", "ACE", "mods")


def mods_dir(create=False):
    from . import detect
    d = ((config.CFG.get("mods_dir") or "").strip()
         or detect.find("server_mods") or MODS)
    # ⚠ On a fresh machine this folder does not exist until the server has run
    # once. Installing a mod is exactly when it should be created.
    if create and d:
        os.makedirs(d, exist_ok=True)
    return d


def client_mods_dir():
    from . import detect
    return ((config.CFG.get("client_mods_dir") or "").strip()
            or detect.find("client_mods") or CLIENT_MODS)


def _size(path):
    try:
        return os.path.getsize(path)
    except OSError:
        return 0


def installed(which="server"):
    """Every car mod in a mod folder, with health.

    which="server" -> the dedicated server's folder
    which="client" -> the game client's folder
    """
    d = mods_dir() if which == "server" else client_mods_dir()
    if not os.path.isdir(d):
        return {"dir": d, "mods": [], "note": "mod folder does not exist yet"}
    names = set()
    for f in os.listdir(d):
        base, ext = os.path.splitext(f)
        if ext.lower() in (".kspkg", ".json"):
            names.add(base)
    out = []
    for base in sorted(names):
        pkg = os.path.join(d, base + ".kspkg")
        meta = os.path.join(d, base + ".json")
        has_pkg, has_meta = os.path.exists(pkg), os.path.exists(meta)
        cars, err = [], None
        if has_meta:
            try:
                for c in json.load(open(meta, encoding="utf-8")).get("cars", []):
                    cars.append({"id": c.get("name"),
                                 "label": c.get("display_name") or c.get("name"),
                                 "pi": c.get("performance_indicator")})
            except Exception as ex:
                err = f"unreadable .json: {ex}"
        out.append({
            "name": base,
            "kspkg": has_pkg, "json": has_meta,
            "size_mb": round(_size(pkg) / 1048576, 1) if has_pkg else 0,
            "cars": cars,
            "error": err,
            # the failure mode that looks like a server bug
            "usable": has_pkg and has_meta and not err,
            "why": (None if (has_pkg and has_meta) else
                    "missing .kspkg - nothing to load" if not has_pkg else
                    "missing .json - the car will NOT appear in the car list"),
        })
    return {"dir": d, "side": which, "mods": out, "total": len(out),
            "usable": sum(1 for m in out if m["usable"])}


def audit():
    """Compare both sides and name what is actually wrong.

    Three distinct problems, each fixed differently:
      * broken pair  - a .kspkg with no .json (or vice versa) on one side
      * client-only  - installed for you, but the server cannot host it
      * server-only  - the server offers it, but you cannot load it
    """
    srv = installed("server")
    cli = installed("client")
    by = {"server": {m["name"]: m for m in srv["mods"]},
          "client": {m["name"]: m for m in cli["mods"]}}
    rows = []
    for name in sorted(set(by["server"]) | set(by["client"])):
        s_, c_ = by["server"].get(name), by["client"].get(name)
        issues = []
        for side, m in (("server", s_), ("client", c_)):
            if m and m["why"]:
                issues.append(f"{side}: {m['why']}")
        if s_ and not c_:
            issues.append("only on the server - you cannot load this car")
        if c_ and not s_:
            issues.append("only on the client - the server cannot host it")
        rows.append({
            "name": name,
            "server": bool(s_), "client": bool(c_),
            "server_ok": bool(s_ and s_["usable"]),
            "client_ok": bool(c_ and c_["usable"]),
            "cars": (s_ or c_ or {}).get("cars", []),
            "size_mb": (s_ or c_ or {}).get("size_mb", 0),
            "issues": issues,
            "ok": not issues,
            # the exact file to copy, when that is all that is missing
            "fix": _fix_hint(name, s_, c_),
        })
    return {"server_dir": srv["dir"], "client_dir": cli["dir"],
            "mods": rows, "total": len(rows),
            "problems": sum(1 for r in rows if not r["ok"])}


def _fix_hint(name, s_, c_):
    """A concrete copy command when one side is only missing its .json."""
    if c_ and c_["kspkg"] and not c_["json"] and s_ and s_["json"]:
        return {"what": "copy the .json from the server to the client",
                "src": os.path.join(mods_dir(), name + ".json"),
                "dst": os.path.join(client_mods_dir(), name + ".json")}
    if s_ and s_["kspkg"] and not s_["json"] and c_ and c_["json"]:
        return {"what": "copy the .json from the client to the server",
                "src": os.path.join(client_mods_dir(), name + ".json"),
                "dst": os.path.join(mods_dir(), name + ".json")}
    return None


def apply_fix(name):
    """Perform the copy that _fix_hint describes."""
    a = audit()
    row = next((r for r in a["mods"] if r["name"] == name), None)
    if not row or not row.get("fix"):
        return {"ok": False, "error": "nothing automatically fixable for " + name}
    fix = row["fix"]
    if not os.path.isfile(fix["src"]):
        return {"ok": False, "error": "source file missing: " + fix["src"]}
    os.makedirs(os.path.dirname(fix["dst"]), exist_ok=True)
    shutil.copy2(fix["src"], fix["dst"])
    return {"ok": True, "copied": fix["dst"], "what": fix["what"]}


def _copy_file(src, dst, force=False):
    """Copy one file. Skip if dest exists and matches, unless force."""
    if not os.path.isfile(src):
        return "missing"
    os.makedirs(os.path.dirname(dst) or ".", exist_ok=True)
    if os.path.isfile(dst) and not force:
        if _size(src) == _size(dst):
            return "same"
        return "differs"
    shutil.copy2(src, dst)
    return "copied"


def sync(direction="to_server", force=False, names=None):
    """Make client and server mod folders agree.

    direction:
      to_server  copy complete client pairs onto the dedicated server
      to_client  copy complete server pairs onto the game client
      both       fill gaps both ways; never overwrite a different-sized
                 file unless force=True

    A join needs the same <mod>.kspkg + <mod>.json on both sides. After
    the copy we refresh lobby.json so the DRIVE list includes the new ids.
    """
    direction = (direction or "to_server").strip().lower()
    if direction not in ("to_server", "to_client", "both"):
        return {"ok": False, "error": f"unknown direction {direction}"}
    srv = mods_dir(create=True)
    cli = client_mods_dir()
    if not cli:
        return {"ok": False, "error": "client mods folder not found"}
    os.makedirs(cli, exist_ok=True)

    au = audit()
    wanted = names or [r["name"] for r in au["mods"]]
    copied, skipped, errors = [], [], []

    def one(name, src_dir, dst_dir, label):
        for ext in (".kspkg", ".json"):
            src, dst = os.path.join(src_dir, name + ext), os.path.join(dst_dir, name + ext)
            if not os.path.isfile(src):
                continue
            try:
                st = _copy_file(src, dst, force=force)
            except OSError as ex:
                errors.append(f"{name}{ext}: {ex}")
                continue
            rec = f"{label}:{name}{ext}"
            if st == "copied":
                copied.append(rec)
            else:
                skipped.append({"file": rec, "why": st})

    for name in wanted:
        row = next((r for r in au["mods"] if r["name"] == name), None)
        if direction in ("to_server", "both"):
            one(name, cli, srv, "->server")
        if direction in ("to_client", "both"):
            one(name, srv, cli, "->client")
        if row is None and direction != "both":
            # audit only lists names present on at least one side; a
            # brand-new pair on the source is already covered.
            pass

    lobby_info = None
    try:
        lobby_info = _refresh_lobby()
    except Exception as ex:
        errors.append(f"lobby refresh: {ex}")

    again = audit()
    return {
        "ok": not errors,
        "direction": direction,
        "copied": copied,
        "skipped": skipped,
        "errors": errors,
        "server_dir": srv,
        "client_dir": cli,
        "problems_before": au.get("problems"),
        "problems_after": again.get("problems"),
        "lobby": lobby_info,
    }


def _refresh_lobby():
    """Rebuild the advertised car list from whatever profile we last used."""
    from . import lobby, servers
    items = servers.load()
    current = lobby.read()
    chosen = None
    if current.get("server_id"):
        chosen = next((p for p in items if p.get("id") == current["server_id"]),
                      None)
    if chosen is None and items:
        chosen = items[0]
    if chosen is None:
        return lobby.write({})
    return lobby.write(chosen)


def car_names():
    """preset_<code>_mech_<n> -> display name, from installed mod manifests.

    This is the only truthful source for those ids: the mod that ships the car
    declares its own display name. Kunos presets are not covered - their names
    are not in any file we have.
    """
    names = {}
    for m in installed().get("mods", []):
        for c in m["cars"]:
            if c["id"]:
                names[c["id"]] = c["label"]
    return names


def scan_source(path):
    """Look at a folder or .zip and report the mods it could install."""
    if not path or not os.path.exists(path):
        return {"ok": False, "error": "path not found"}
    found = {}
    if os.path.isfile(path) and path.lower().endswith(".zip"):
        try:
            with zipfile.ZipFile(path) as z:
                for n in z.namelist():
                    base, ext = os.path.splitext(os.path.basename(n))
                    if ext.lower() in (".kspkg", ".json"):
                        found.setdefault(base, set()).add(ext.lower())
        except Exception as ex:
            return {"ok": False, "error": f"bad zip: {ex}"}
    elif os.path.isdir(path):
        for f in os.listdir(path):
            base, ext = os.path.splitext(f)
            if ext.lower() in (".kspkg", ".json"):
                found.setdefault(base, set()).add(ext.lower())
    else:
        return {"ok": False, "error": "expected a folder or a .zip"}
    mods = [{"name": b,
             "kspkg": ".kspkg" in e, "json": ".json" in e,
             "complete": {".kspkg", ".json"} <= e} for b, e in sorted(found.items())]
    return {"ok": True, "path": path, "mods": mods,
            "complete": sum(1 for m in mods if m["complete"])}


def _mod_dests(create=True):
    """Server and client folders. A join needs the pair on both sides."""
    out, seen = [], set()
    for d in (mods_dir(create=create), client_mods_dir()):
        if not d:
            continue
        key = os.path.normcase(os.path.abspath(d))
        if key in seen:
            continue
        seen.add(key)
        if create:
            os.makedirs(d, exist_ok=True)
        out.append(d)
    return out


def _write_mod_file(name, ext, src_fh=None, src_path=None):
    """Write one half of a car pair onto every side we install to."""
    if ext.lower() not in (".kspkg", ".json"):
        return []
    dests = _mod_dests(create=True)
    if not dests:
        return []
    first = os.path.join(dests[0], name + ext)
    if src_path:
        if os.path.abspath(src_path) != os.path.abspath(first):
            shutil.copy2(src_path, first)
    elif src_fh is not None:
        with open(first, "wb") as fh:
            shutil.copyfileobj(src_fh, fh)
    else:
        return []
    written = [first]
    for dest in dests[1:]:
        dst = os.path.join(dest, name + ext)
        if os.path.abspath(dst) != os.path.abspath(first):
            shutil.copy2(first, dst)
        written.append(dst)
    return written


def _car_incoming_sizes(path, wanted):
    """name -> {ext: size} for the pair about to be installed."""
    out = {}
    names = {m["name"] for m in wanted}
    if os.path.isfile(path) and path.lower().endswith(".zip"):
        with zipfile.ZipFile(path) as z:
            for n in z.namelist():
                base, ext = os.path.splitext(os.path.basename(n))
                ext = ext.lower()
                if ext not in (".kspkg", ".json") or base not in names:
                    continue
                out.setdefault(base, {})[ext] = z.getinfo(n).file_size
        return out
    for m in wanted:
        for ext in (".kspkg", ".json"):
            src = os.path.join(path, m["name"] + ext)
            if os.path.isfile(src):
                out.setdefault(m["name"], {})[ext] = _size(src)
    return out


def _car_conflict(incoming):
    """Already installed? And are the bytes the same size as what we have?"""
    dests = []
    for side, fn in (("server", mods_dir), ("client", client_mods_dir)):
        try:
            d = fn()
        except Exception:
            d = None
        if d:
            dests.append((side, d))
    conflicts = []
    any_exist = False
    for name, files in incoming.items():
        for side, d in dests:
            for ext in (".kspkg", ".json"):
                dp = os.path.join(d, name + ext)
                if not os.path.isfile(dp):
                    continue
                any_exist = True
                inc = files.get(ext)
                conflicts.append({
                    "name": name, "side": side, "file": name + ext,
                    "same": inc is not None and inc == _size(dp),
                    "have": _size(dp), "incoming": inc,
                })
    if not any_exist:
        return None
    same = bool(conflicts) and all(c["same"] for c in conflicts)
    for name, files in incoming.items():
        for ext in files:
            for _side, d in dests:
                if not os.path.isfile(os.path.join(d, name + ext)):
                    same = False
    names = list(incoming)
    label = names[0] if len(names) == 1 else f"{len(names)} car mods"
    return {
        "ok": False,
        "need_confirm": True,
        "exists": True,
        "same": same,
        "kind": "car",
        "name": names[0] if len(names) == 1 else ", ".join(names),
        "label": label,
        "names": names,
        "conflicts": conflicts,
    }


def install(path, only=None, overwrite=False):
    """Copy mod pairs from a folder or .zip onto the server AND the client.

    A car that lands only on the dedicated server is hostable but not
    selectable in Drive; only on the client, and friends cannot be offered
    it. Both sides get the same .kspkg + .json.

    Refuses a second copy of the same name unless overwrite=True.
    """
    scan = scan_source(path)
    if not scan.get("ok"):
        return scan
    wanted = [m for m in scan["mods"]
              if (not only or m["name"] in only)]
    if not wanted:
        return {"ok": False, "error": "nothing selected to install"}
    if not overwrite:
        hit = _car_conflict(_car_incoming_sizes(path, wanted))
        if hit:
            return hit

    done, skipped = [], []
    if os.path.isfile(path):
        with zipfile.ZipFile(path) as z:
            for n in z.namelist():
                base, ext = os.path.splitext(os.path.basename(n))
                if ext.lower() not in (".kspkg", ".json"):
                    continue
                if not any(m["name"] == base for m in wanted):
                    continue
                with z.open(n) as src:
                    _write_mod_file(base, ext, src_fh=src)
                done.append(base + ext)
    else:
        for m in wanted:
            for ext in (".kspkg", ".json"):
                src = os.path.join(path, m["name"] + ext)
                if os.path.exists(src):
                    _write_mod_file(m["name"], ext, src_path=src)
                    done.append(m["name"] + ext)
                else:
                    skipped.append(m["name"] + ext)
    incomplete = [m for m in wanted if not m["complete"]]
    # Say which file is actually missing: "no .json" and "no .kspkg" fail in
    # completely different ways, and a generic message sends you looking in the
    # wrong place.
    notes = []
    for m in incomplete:
        if not m["json"]:
            notes.append(f"{m['name']}: no .json - the car will NOT appear in "
                         f"the car list")
        elif not m["kspkg"]:
            notes.append(f"{m['name']}: no .kspkg - there is no content to load")
    try:
        _refresh_lobby()
    except Exception:
        pass
    return {"ok": True, "kind": "car", "installed": done, "missing": skipped,
            "incomplete": [m["name"] for m in incomplete],
            "sides": _mod_dests(create=False),
            "warning": "; ".join(notes) or None}


def remove(name, sides=None):
    """Delete a car pair. Default is both sides — leftover half-pairs
    are how a 'removed' mod still shows up in one list."""
    name = _safe_base(name)
    if not name:
        return {"ok": False, "error": "bad name"}
    dirs = []
    want = sides or ("server", "client")
    if "server" in want:
        dirs.append(mods_dir())
    if "client" in want:
        dirs.append(client_mods_dir())
    gone = []
    for d in dirs:
        if not d or not os.path.isdir(d):
            continue
        for ext in (".kspkg", ".json"):
            p = os.path.join(d, name + ext)
            if os.path.exists(p):
                os.remove(p)
                gone.append(p)
    _unshare_name(mod=name)
    try:
        _refresh_lobby()
    except Exception:
        pass
    return {"ok": bool(gone), "removed": gone}


# ------------------------------------------------------------------ tracks --
def tracks_installed():
    """Track folders on the server, and whether they have the loose AI splines.

    ⚠ The server's content.kspkg ships NEITHER spline file. Without them the
    VirtualAIProvider cannot run, so a track with missing splines can be hosted
    but will have no AI - a failure that otherwise shows up as an empty grid.
    """
    root = os.path.join(config.server_dir(), "content", "tracks")
    out = []
    if not os.path.isdir(root):
        return {"root": root, "tracks": [], "note": "no content/tracks folder"}
    for track in sorted(os.listdir(root)):
        ldir = os.path.join(root, track, "layouts")
        if not os.path.isdir(ldir):
            continue
        layouts = {}
        for f in os.listdir(ldir):
            if not f.endswith(".aisplinedata"):
                continue
            name = f.split(".")[0]
            kind = "ideal_line" if ".ideal_line." in f else (
                "pitlane" if ".pitlane." in f else "other")
            layouts.setdefault(name, set()).add(kind)
        out.append({
            "track": track,
            "layouts": [{"layout": k,
                         "ideal_line": "ideal_line" in v,
                         "pitlane": "pitlane" in v,
                         "ai_ready": {"ideal_line", "pitlane"} <= v}
                        for k, v in sorted(layouts.items())],
        })
    return {"root": root, "tracks": out}


# ----------------------------------------------------------- drop / library --
# Cars: a .kspkg + .json pair (zip, folder, or the two files dropped together).
# Tracks: the same uncompressed tar Get content fetches (acecm_track.json
# inside), or a zip of that tree. Both land the same way a join install does.


def _safe_base(name):
    name = os.path.basename((name or "").replace("\\", "/")).strip()
    if not name or name in (".", "..") or any(c in name for c in '/:*?"<>|\0'):
        return ""
    return name


def _safe_folder(name):
    name = _safe_base(name)
    if not name or name.lower().endswith((".tar", ".zip", ".kspkg", ".json",
                                          ".tgz", ".gz")):
        # a filename is fine as a folder only after stripping the pack suffix
        stem, ext = os.path.splitext(name)
        if ext.lower() in (".tar", ".zip", ".tgz", ".gz"):
            name = stem
            if name.lower().endswith(".tar"):
                name = name[:-4]
    if not name or name in (".", ".."):
        return ""
    return name


def _dir_bytes(root):
    total = 0
    if not os.path.isdir(root):
        return 0
    for base, _dirs, files in os.walk(root):
        for f in files:
            try:
                total += os.path.getsize(os.path.join(base, f))
            except OSError:
                pass
    return total


def _unshare_name(mod=None, track=None):
    """Drop a thing from every registry entry that listed it."""
    try:
        from . import registry
    except Exception:
        return
    items = registry.load()
    changed = False
    keep = []
    for e in items:
        mods = list(e.get("required_mods") or [])
        tracks = list(e.get("required_tracks") or [])
        if mod and mod in mods:
            mods = [m for m in mods if m != mod]
            changed = True
        if track and track in tracks:
            tracks = [t for t in tracks if t != track]
            changed = True
        e = {**e, "required_mods": mods, "required_tracks": tracks}
        if mods or tracks:
            keep.append(e)
        else:
            changed = True
    if changed:
        registry.save(keep)


def drop_root():
    d = os.path.join(config.DATA, "drop")
    os.makedirs(d, exist_ok=True)
    return d


def drop_staging(did, create=True):
    import re
    did = (did or "").strip()
    if not re.fullmatch(r"[0-9a-fA-F-]{8,40}", did):
        return None
    d = os.path.join(drop_root(), did)
    if create:
        os.makedirs(d, exist_ok=True)
    return d if os.path.isdir(d) or create else None


def drop_part(did, filename, stream, size=None, limit=8 * 1024 * 1024 * 1024):
    """Stream one dropped file into a staging folder."""
    name = _safe_base(filename)
    if not name:
        return {"ok": False, "error": "bad filename"}
    dest_dir = drop_staging(did, create=True)
    if not dest_dir:
        return {"ok": False, "error": "bad drop id"}
    dest = os.path.join(dest_dir, name)
    wrote = 0
    try:
        with open(dest, "wb") as fh:
            while True:
                chunk = stream.read(1 << 20)
                if not chunk:
                    break
                wrote += len(chunk)
                if wrote > limit:
                    fh.close()
                    try:
                        os.remove(dest)
                    except OSError:
                        pass
                    return {"ok": False, "error": "file is larger than 8 GB"}
                fh.write(chunk)
    except OSError as ex:
        return {"ok": False, "error": str(ex)}
    return {"ok": True, "name": name, "bytes": wrote,
            "size": size or wrote}


def drop_cleanup(did):
    d = drop_staging(did, create=False)
    if d and os.path.isdir(d):
        shutil.rmtree(d, ignore_errors=True)


def _read_json_member(zf_or_tar, name):
    try:
        if hasattr(zf_or_tar, "extractfile"):
            fh = zf_or_tar.extractfile(name)
            if not fh:
                return {}
            return json.loads(fh.read().decode("utf-8"))
        return json.loads(zf_or_tar.read(name).decode("utf-8"))
    except Exception:
        return {}


def _archive_meta(path):
    """acecm_track.json from a tar or zip, if this is a multiplayer pack."""
    low = path.lower()
    try:
        if low.endswith(".zip"):
            with zipfile.ZipFile(path) as z:
                names = z.namelist()
                hit = next((n for n in names
                            if os.path.basename(n.replace("\\", "/"))
                            == "acecm_track.json"), None)
                return _read_json_member(z, hit) if hit else {}
        import tarfile
        with tarfile.open(path, "r:*") as tar:
            hit = None
            for info in tar.getmembers():
                if os.path.basename(info.name.replace("\\", "/")) == "acecm_track.json":
                    hit = info
                    break
            return _read_json_member(tar, hit) if hit else {}
    except Exception:
        return {}


def _looks_like_track_dir(path):
    if not os.path.isdir(path):
        return False
    names = set(os.listdir(path))
    if "acecm_track.json" in names:
        return True
    if "containers" in names and os.path.isdir(os.path.join(path, "containers")):
        return True
    for n in names:
        low = n.lower()
        if low.endswith(".scene") or low.endswith(".track"):
            return True
    return False


def _archive_rel(rel):
    rel = (rel or "").replace("\\", "/").lstrip("/")
    if not rel or rel.endswith("/") or ".." in rel.split("/"):
        return ""
    if os.path.basename(rel) == "acecm_track.json":
        return "acecm_track.json"
    parts = [p for p in rel.split("/") if p]
    if parts and parts[0].lower() == "tracks" and len(parts) > 2:
        parts = parts[2:]
    return "/".join(parts)


def _archive_file_sizes(path):
    out = {}
    low = (path or "").lower()
    if low.endswith(".zip"):
        with zipfile.ZipFile(path) as z:
            for info in z.infolist():
                if info.is_dir():
                    continue
                rel = _archive_rel(info.filename)
                if rel:
                    out[rel] = info.file_size
        return out
    import tarfile
    with tarfile.open(path, "r:*") as tar:
        for info in tar.getmembers():
            if not info.isfile():
                continue
            rel = _archive_rel(info.name)
            if rel:
                out[rel] = info.size
    return out


def _dir_file_sizes(root):
    out = {}
    if not os.path.isdir(root):
        return out
    for base, _dirs, files in os.walk(root):
        for f in files:
            p = os.path.join(base, f)
            rel = os.path.relpath(p, root).replace("\\", "/")
            if rel.startswith(".."):
                continue
            out[rel] = _size(p)
    return out


def _sizes_match(incoming, have):
    """Same payload? Ignore the sidecar — hosts regenerate it."""
    skip = {"acecm_track.json"}
    inc = {k: v for k, v in (incoming or {}).items() if k not in skip}
    hv = {k: v for k, v in (have or {}).items() if k not in skip}
    if not inc:
        return False
    if set(inc) != set(hv):
        return False
    return all(inc[k] == hv[k] for k in inc)


def _track_has_files(dest):
    if not dest or not os.path.isdir(dest):
        return False
    try:
        return any(os.scandir(dest))
    except OSError:
        return False


def _track_conflict(folder, dest, incoming_sizes, label=None):
    if not _track_has_files(dest):
        return None
    same = _sizes_match(incoming_sizes, _dir_file_sizes(dest))
    return {
        "ok": False,
        "need_confirm": True,
        "exists": True,
        "same": same,
        "kind": "track",
        "name": folder,
        "label": label or folder,
        "folder": folder,
        "path": dest,
    }


def _extract_archive(path, dest):
    """Unpack a track pack into dest. Members cannot escape dest."""
    from .contentsync import _under
    os.makedirs(dest, exist_ok=True)
    low = path.lower()
    if low.endswith(".zip"):
        with zipfile.ZipFile(path) as z:
            for info in z.infolist():
                rel = info.filename.replace("\\", "/").lstrip("/")
                if not rel or rel.endswith("/") or ".." in rel.split("/"):
                    continue
                if os.path.basename(rel) == "acecm_track.json":
                    target = _under(dest, "acecm_track.json")
                else:
                    # packs are stored with files at the track-folder root
                    parts = [p for p in rel.split("/") if p]
                    if parts and parts[0].lower() == "tracks" and len(parts) > 2:
                        parts = parts[2:]
                    if not parts:
                        continue
                    target = _under(dest, *parts)
                os.makedirs(os.path.dirname(target), exist_ok=True)
                with z.open(info) as src, open(target, "wb") as dst:
                    shutil.copyfileobj(src, dst)
        return dest
    import tarfile
    with tarfile.open(path, "r:*") as tar:
        try:
            tar.extractall(dest, filter="data")
        except TypeError:
            for info in tar.getmembers():
                rel = (info.name or "").replace("\\", "/").lstrip("/")
                if not rel or ".." in rel.split("/"):
                    continue
                tar.extract(info, dest)
    return dest


def install_track_pack(path, folder=None, overwrite=False):
    """Install a multiplayer track tar/zip the same way Get content does."""
    from . import contentsync
    from . import tracks as trackmod
    if not path or not os.path.isfile(path):
        return {"ok": False, "error": "track pack not found"}
    meta = _archive_meta(path)
    folder = _safe_folder(folder or meta.get("folder")
                          or os.path.splitext(os.path.basename(path))[0])
    if not folder:
        return {"ok": False, "error": "could not tell the track folder name"}
    dest = contentsync._under(contentsync.tracks_dir(), folder)
    if not overwrite:
        hit = _track_conflict(folder, dest, _archive_file_sizes(path),
                              meta.get("display_name") or folder)
        if hit:
            return hit
    if overwrite and _track_has_files(dest):
        shutil.rmtree(dest)
    os.makedirs(dest, exist_ok=True)
    _extract_archive(path, dest)
    # meta may have been extracted; prefer the one on disk
    side = os.path.join(dest, "acecm_track.json")
    if os.path.isfile(side):
        try:
            meta = {**meta, **json.load(open(side, encoding="utf-8"))}
        except Exception:
            pass
    if meta.get("folder") and _safe_folder(meta["folder"]) != folder:
        # pack named the folder differently than the archive file
        want = _safe_folder(meta["folder"])
        if want:
            dest2 = contentsync._under(contentsync.tracks_dir(), want)
            if dest2 != dest:
                if os.path.isdir(dest2):
                    shutil.rmtree(dest2)
                shutil.move(dest, dest2)
                dest, folder = dest2, want
    reg = trackmod.register_client_track(folder, meta)
    return {
        "ok": True,
        "kind": "track",
        "folder": folder,
        "display_name": (meta.get("display_name") or folder),
        "path": dest,
        "registered": bool(reg.get("ok")),
        "register": reg,
        "warning": None if reg.get("ok") else (reg.get("error") or
                   "files are in place; register the track when the game is closed"),
    }


def install_track_folder(path, folder=None, overwrite=False):
    """Copy an already-unpacked track tree into the client's tracks folder."""
    from . import contentsync
    from . import tracks as trackmod
    if not path or not os.path.isdir(path):
        return {"ok": False, "error": "track folder not found"}
    meta = {}
    side = os.path.join(path, "acecm_track.json")
    if os.path.isfile(side):
        try:
            meta = json.load(open(side, encoding="utf-8"))
        except Exception:
            pass
    folder = _safe_folder(folder or meta.get("folder") or os.path.basename(path))
    if not folder:
        return {"ok": False, "error": "bad track folder name"}
    dest = contentsync._under(contentsync.tracks_dir(), folder)
    same_dir = os.path.abspath(path) == os.path.abspath(dest)
    if not overwrite and not same_dir:
        hit = _track_conflict(folder, dest, _dir_file_sizes(path),
                              meta.get("display_name") or folder)
        if hit:
            return hit
    if os.path.abspath(path) != os.path.abspath(dest):
        if os.path.isdir(dest):
            if not overwrite:
                return _track_conflict(folder, dest, _dir_file_sizes(path),
                                       meta.get("display_name") or folder) \
                       or {"ok": False, "error": "track already exists"}
            shutil.rmtree(dest)
        shutil.copytree(path, dest)
    if meta:
        try:
            json.dump(meta, open(os.path.join(dest, "acecm_track.json"), "w",
                                 encoding="utf-8"), indent=2)
        except OSError:
            pass
    reg = trackmod.register_client_track(folder, meta)
    return {
        "ok": True,
        "kind": "track",
        "folder": folder,
        "display_name": meta.get("display_name") or folder,
        "path": dest,
        "registered": bool(reg.get("ok")),
        "register": reg,
        "warning": None if reg.get("ok") else (reg.get("error") or None),
    }


def ingest(path, overwrite=False):
    """Install whatever was dropped: car pair, car zip, or track pack."""
    if not path or not os.path.exists(path):
        return {"ok": False, "error": "nothing to install"}
    if os.path.isdir(path):
        if _looks_like_track_dir(path):
            return install_track_folder(path, overwrite=overwrite)
        scan = scan_source(path)
        if scan.get("ok") and scan.get("mods"):
            return install(path, overwrite=overwrite)
        return {"ok": False,
                "error": "that folder is not a car mod (.kspkg + .json) "
                         "or a track pack"}
    low = path.lower()
    if low.endswith((".tar", ".tar.gz", ".tgz")):
        return install_track_pack(path, overwrite=overwrite)
    if low.endswith(".zip"):
        meta = _archive_meta(path)
        scan = scan_source(path)
        if scan.get("ok") and scan.get("complete"):
            return install(path, overwrite=overwrite)
        if meta or not (scan.get("mods") or []):
            return install_track_pack(path, overwrite=overwrite)
        return install(path, overwrite=overwrite)
    if low.endswith((".kspkg", ".json")):
        # a single half-pair: look next to it, else wait for the other half
        folder = os.path.dirname(path)
        scan = scan_source(folder) if os.path.isdir(folder) else {"ok": False}
        if scan.get("ok") and scan.get("complete"):
            return install(folder, overwrite=overwrite)
        return {"ok": False,
                "error": "drop the matching .kspkg and .json together "
                         "(or a zip of both)"}
    return {"ok": False,
            "error": "drop a car zip (.kspkg + .json) or a track pack (.tar)"}


def ingest_staging(did, overwrite=False):
    """Finish a drag-drop: staging holds the files the window received.

    On a duplicate the files stay staged so the UI can ask and retry
    with overwrite=True without uploading again.
    """
    d = drop_staging(did, create=False)
    if not d or not os.path.isdir(d):
        return {"ok": False, "error": "nothing was dropped"}
    names = [n for n in os.listdir(d)
             if os.path.isfile(os.path.join(d, n))]
    if not names:
        drop_cleanup(did)
        return {"ok": False, "error": "drop was empty"}
    if len(names) == 1:
        r = ingest(os.path.join(d, names[0]), overwrite=overwrite)
    elif any(n.lower() == "acecm_track.json" for n in names) \
            or any(n.lower().endswith((".scene", ".track")) for n in names):
        r = install_track_folder(d, overwrite=overwrite)
    else:
        scan = scan_source(d)
        if scan.get("ok") and scan.get("mods"):
            r = install(d, overwrite=overwrite)
        else:
            r = {"ok": False,
                 "error": "could not tell if that was a car or a track pack"}
    if r.get("need_confirm"):
        r["id"] = did
        return r
    drop_cleanup(did)
    return r


def remove_track(folder):
    """Delete an imported track's files. Stock content.kspkg is untouched."""
    from . import contentsync
    folder = _safe_folder(folder)
    if not folder:
        return {"ok": False, "error": "bad track folder"}
    dest = contentsync._under(contentsync.tracks_dir(), folder)
    if not os.path.isdir(dest):
        return {"ok": False, "error": "that track is not installed"}
    shutil.rmtree(dest)
    _unshare_name(track=folder)
    cache = os.path.join(config.DATA, "track_map.json")
    try:
        if os.path.isfile(cache):
            os.remove(cache)
    except OSError:
        pass
    return {"ok": True, "removed": dest, "folder": folder}


def export_car(name, dest_dir=None):
    """Zip the .kspkg + .json the way a drop can install it again."""
    name = _safe_base(name)
    if not name:
        return {"ok": False, "error": "bad name"}
    srcs = {}
    for d in (client_mods_dir(), mods_dir()):
        if not d:
            continue
        for ext in (".kspkg", ".json"):
            p = os.path.join(d, name + ext)
            if ext not in srcs and os.path.isfile(p):
                srcs[ext] = p
    if ".kspkg" not in srcs:
        return {"ok": False, "error": f"no {name}.kspkg on this machine"}
    dest_dir = dest_dir or os.path.join(os.path.expanduser("~"), "Downloads")
    os.makedirs(dest_dir, exist_ok=True)
    dest = os.path.join(dest_dir, name + ".zip")
    with zipfile.ZipFile(dest, "w", compression=zipfile.ZIP_DEFLATED) as z:
        for ext, p in srcs.items():
            z.write(p, name + ext)
    return {"ok": True, "kind": "car", "name": name, "path": dest,
            "files": list(srcs)}


def export_track(folder, dest_dir=None):
    """The same uncompressed tar friends pull with Get content."""
    from . import registry
    folder = _safe_folder(folder)
    if not folder:
        return {"ok": False, "error": "bad track folder"}
    packed = registry.ensure_track_pack(folder)
    if not packed or not os.path.isfile(packed):
        return {"ok": False, "error": "no files for that track"}
    dest_dir = dest_dir or os.path.join(os.path.expanduser("~"), "Downloads")
    os.makedirs(dest_dir, exist_ok=True)
    dest = os.path.join(dest_dir, folder + ".tar")
    if os.path.abspath(packed) != os.path.abspath(dest):
        shutil.copy2(packed, dest)
    return {"ok": True, "kind": "track", "name": folder, "path": dest,
            "pack": packed}


def export_item(kind, name, dest_dir=None):
    kind = (kind or "").strip().lower()
    if kind == "car":
        return export_car(name, dest_dir)
    if kind == "track":
        return export_track(name, dest_dir)
    return {"ok": False, "error": "kind must be car or track"}


def remove_item(kind, name):
    kind = (kind or "").strip().lower()
    if kind == "car":
        return remove(name)
    if kind == "track":
        return remove_track(name)
    return {"ok": False, "error": "kind must be car or track"}


def clip_item(kind, name):
    """Text a host actually pastes: share name, folder, path, share URL."""
    from . import contentsync, registry
    kind = (kind or "").strip().lower()
    name = _safe_base(name) if kind == "car" else _safe_folder(name)
    if not name:
        return {"ok": False, "error": "bad name"}
    lan = ""
    try:
        info = contentsync.share_info()
        lan = info.get("lan_url") or ""
    except Exception:
        pass
    shared = False
    path = ""
    label = name
    if kind == "car":
        for d in (client_mods_dir(), mods_dir()):
            p = os.path.join(d or "", name + ".kspkg")
            if d and os.path.isfile(p):
                path = p
                break
        for e in registry.load():
            if name in (e.get("required_mods") or []):
                shared = True
                break
    else:
        try:
            path = contentsync._under(contentsync.tracks_dir(), name)
        except ValueError:
            path = ""
        try:
            from . import tracks as trackmod
            for t in trackmod.importable():
                if t.get("folder") == name:
                    label = t.get("display_name") or name
                    break
        except Exception:
            pass
        for e in registry.load():
            if name in (e.get("required_tracks") or []):
                shared = True
                break
    text = name
    return {
        "ok": True, "kind": kind, "name": name, "text": text,
        "label": label, "path": path, "share_url": lan if shared else "",
        "shared": shared,
        "variants": {
            "name": name,
            "label": label,
            "path": path,
            "share": lan if shared else "",
        },
    }


def library():
    """Everything the Content page can manage."""
    from . import contentsync, registry
    from . import tracks as trackmod
    au = audit()
    shared_mods, shared_tracks = {}, {}
    for e in registry.load():
        for m in e.get("required_mods") or []:
            shared_mods[m] = e.get("id")
        for t in e.get("required_tracks") or []:
            shared_tracks[t] = e.get("id")
    cars = []
    for row in au.get("mods") or []:
        cars.append({
            "kind": "car",
            "name": row["name"],
            "label": row["name"],
            "size_mb": row.get("size_mb") or 0,
            "cars": row.get("cars") or [],
            "server_ok": row.get("server_ok"),
            "client_ok": row.get("client_ok"),
            "issues": row.get("issues") or [],
            "ok": row.get("ok"),
            "shared": row["name"] in shared_mods,
            "path": os.path.join(au.get("client_dir") or au.get("server_dir")
                                 or "", row["name"] + ".kspkg"),
        })
    tracks = []
    packs = os.path.join(config.DATA, "packs")
    for t in trackmod.importable():
        folder = t.get("folder") or ""
        # Do not walk a 1 GB tree just to draw the list. The cached
        # multiplayer pack is the size that actually gets shared.
        packed = os.path.join(packs, folder + ".tar")
        size_mb = 0
        if os.path.isfile(packed):
            size_mb = round(_size(packed) / 1048576, 1)
        tracks.append({
            "kind": "track",
            "name": folder,
            "label": t.get("display_name") or folder,
            "folder": folder,
            "path": t.get("path") or "",
            "layout": t.get("layout") or "",
            "files": t.get("files") or 0,
            "size_mb": size_mb,
            "ok": bool(t.get("ok")),
            "error": t.get("error"),
            "shared": folder in shared_tracks,
        })
    lan = ""
    try:
        lan = (contentsync.share_info() or {}).get("lan_url") or ""
    except Exception:
        pass
    return {
        "ok": True,
        "cars": cars,
        "tracks": tracks,
        "server_dir": au.get("server_dir"),
        "client_dir": au.get("client_dir"),
        "tracks_dir": contentsync.tracks_dir(),
        "share_url": lan,
        "total": len(cars) + len(tracks),
    }
