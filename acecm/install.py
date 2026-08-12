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

MODS = os.path.join(os.path.expanduser("~"), "Saved Games", "ACE-Server", "mods")
# The CLIENT keeps its mods somewhere else entirely. Both sides need the same
# .kspkg + .json pair, and they drift apart easily: a car present on the server
# but missing its .json on the client cannot be selected by the player, which
# looks like the mod "not working" rather than one 244-byte file being absent.
CLIENT_MODS = os.path.join(os.path.expanduser("~"), "Saved Games", "ACE", "mods")


def mods_dir():
    return config.CFG.get("mods_dir") or MODS


def client_mods_dir():
    return config.CFG.get("client_mods_dir") or CLIENT_MODS


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


def install(path, only=None):
    """Copy mod pairs from a folder or .zip into the server's mod folder."""
    scan = scan_source(path)
    if not scan.get("ok"):
        return scan
    dest = mods_dir()
    os.makedirs(dest, exist_ok=True)
    wanted = [m for m in scan["mods"]
              if (not only or m["name"] in only)]
    if not wanted:
        return {"ok": False, "error": "nothing selected to install"}

    done, skipped = [], []
    if os.path.isfile(path):
        with zipfile.ZipFile(path) as z:
            for n in z.namelist():
                base, ext = os.path.splitext(os.path.basename(n))
                if ext.lower() not in (".kspkg", ".json"):
                    continue
                if not any(m["name"] == base for m in wanted):
                    continue
                with z.open(n) as src, open(os.path.join(dest, base + ext),
                                            "wb") as dst:
                    shutil.copyfileobj(src, dst)
                done.append(base + ext)
    else:
        for m in wanted:
            for ext in (".kspkg", ".json"):
                src = os.path.join(path, m["name"] + ext)
                if os.path.exists(src):
                    shutil.copy2(src, os.path.join(dest, m["name"] + ext))
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
    return {"ok": True, "installed": done, "missing": skipped,
            "incomplete": [m["name"] for m in incomplete],
            "warning": "; ".join(notes) or None}


def remove(name):
    d = mods_dir()
    gone = []
    for ext in (".kspkg", ".json"):
        p = os.path.join(d, name + ext)
        if os.path.exists(p):
            os.remove(p)
            gone.append(name + ext)
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
