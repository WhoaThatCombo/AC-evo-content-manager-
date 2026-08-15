"""Read and write the game's own settings files.

AC EVO stores its settings under `%USERPROFILE%\\Saved Games\\ACE\\` as raw
protobuf, and the FILE EXTENSION is the message name:

    video.videosettings              -> VideoSettings
    input_settings.inputsettings     -> InputSettings      (force feedback)
    input_devices.inputdeviceconfiguration -> InputDeviceConfiguration
    CarMirrors.carmirrorsusersettings-> CarMirrorsUserSettings

Since the 91 descriptors extracted from the client are in the pool, these can be
decoded to real field names, edited, and written back exactly - no guessing at
offsets, no risk of writing a malformed blob.

⚠ The game must be CLOSED when writing. It reads these at startup and rewrites
them on exit, so editing underneath a running client just loses your changes.

⚠ Every write backs up first. A corrupt settings file can stop the game booting,
and the only cheap way back is the copy we made.
"""
import glob
import json
import os
import re
import shutil
import time

from . import config, protos as protolib

ACE_DIR = os.path.join(os.path.expanduser("~"), "Saved Games", "ACE")

# Curated: the settings people actually want to change, in a sensible order.
# Anything not listed still appears, just after these.
FEATURED = [
    ("input_settings.inputsettings", "Force feedback & steering"),
    ("video.videosettings", "Graphics & display"),
    ("audio.audiosettings", "Audio"),
    ("input_devices.inputdeviceconfiguration", "Controller bindings"),
    ("input_keyboard.keyboardinputconfiguration", "Keyboard bindings"),
    ("vr.videosettings", "VR"),
]

_index = None


def _proto_index():
    """lowercase message name -> real name, from the extracted descriptors.

    ⚠ This used to scan .txt dumps in the tools folder. A shipped build has no
    such folder (we do not redistribute Kunos' schemas), so every settings file
    came back "decodable: false" and the page was empty. The names now come
    from descriptors extracted on this machine - see acecm/protos.py.
    """
    global _index
    if _index is not None:
        return _index
    _index = {}
    try:
        for name in protolib.message_names():
            _index.setdefault(name.lower(), name)
    except Exception as ex:
        from . import logs
        logs.LOG.warning("no protobuf schemas available: %s", ex)
    return _index


def _proto():
    """The descriptor pool, built from schemas extracted on this machine."""
    return protolib


def settings_dir():
    """Where the client keeps its settings.

    ⚠ Not necessarily under the user profile - Windows lets "Saved Games" be
    relocated, so this is detected via the known-folder API rather than built
    from %USERPROFILE%.
    """
    from . import detect
    return (config.CFG.get("ace_dir") or "").strip() or detect.find("ace_dir")         or ACE_DIR


def message_for(path):
    ext = os.path.splitext(path)[1].lstrip(".").lower()
    return _proto_index().get(ext)


def discover():
    """Every settings file we can decode, featured ones first."""
    d = settings_dir()
    if not os.path.isdir(d):
        return {"dir": d, "files": [], "error": "settings folder not found"}
    found = {}
    for p in glob.glob(os.path.join(d, "**", "*"), recursive=True):
        if not os.path.isfile(p):
            continue
        rel = os.path.relpath(p, d).replace("\\", "/")
        if rel.startswith(("Logs/", "crashdumps/", "mods/")):
            continue
        msg = message_for(p)
        found[rel] = {
            "file": rel, "message": msg, "size": os.path.getsize(p),
            "decodable": bool(msg and _proto().has(msg)),
        }
    order = []
    for name, label in FEATURED:
        if name in found:
            found[name]["label"] = label
            order.append(found.pop(name))
    order += [v for k, v in sorted(found.items())]
    return {"dir": d, "files": order,
            "decodable": sum(1 for f in order if f["decodable"])}


def _safe_settings_path(rel):
    """A file under Saved Games\\ACE, or None if the path walks out."""
    rel = (rel or "").replace("\\", "/").lstrip("/")
    parts = [p for p in rel.split("/") if p and p != "."]
    if not parts or ".." in parts:
        return None
    root = os.path.abspath(settings_dir() or "")
    if not root:
        return None
    path = os.path.abspath(os.path.join(root, *parts))
    try:
        if os.path.commonpath([root, path]) != root:
            return None
    except ValueError:
        return None
    return path


def read(rel):
    """Decode one settings file to plain JSON."""
    from google.protobuf import json_format
    path = _safe_settings_path(rel)
    if not path or not os.path.isfile(path):
        return {"ok": False, "error": "not found"}
    msg_name = message_for(path)
    ap = _proto()
    if not msg_name or not ap.has(msg_name):
        return {"ok": False, "error": f"no schema for '{rel}'",
                "message": msg_name}
    obj = ap.new(msg_name)
    try:
        obj.ParseFromString(open(path, "rb").read())
    except Exception as ex:
        return {"ok": False, "error": f"cannot parse: {ex}"}
    return {
        "ok": True, "file": rel, "message": msg_name,
        # preserving_proto_field_name keeps snake_case, which matches what the
        # schemas and the game's own logs use - easier to correlate.
        # ⚠ descriptor_pool matters: some settings embed an Any (e.g. a
        # TimeAttack.Specialization), and without our pool json_format cannot
        # resolve the type_url and raises instead of decoding.
        "values": json_format.MessageToDict(
            obj, preserving_proto_field_name=True,
            always_print_fields_with_no_presence=True,
            descriptor_pool=protolib._pool()[0]),
    }


def write(rel, values):
    """Encode JSON back into the settings file, with a backup."""
    from google.protobuf import json_format
    path = _safe_settings_path(rel)
    if not path:
        return {"ok": False, "error": "bad settings path"}
    msg_name = message_for(path)
    ap = _proto()
    if not msg_name or not ap.has(msg_name):
        return {"ok": False, "error": "no schema"}
    if _game_running():
        return {"ok": False, "error":
                "close the game first - it rewrites these files on exit and "
                "would overwrite your changes"}
    obj = ap.new(msg_name)
    try:
        json_format.ParseDict(values, obj)
    except Exception as ex:
        return {"ok": False, "error": f"invalid values: {ex}"}
    blob = obj.SerializeToString()

    bak = f"{path}.bak_acecm"
    if os.path.exists(path) and not os.path.exists(bak):
        shutil.copy2(path, bak)
    # timestamped copy too, so repeated edits are all recoverable
    hist = f"{path}.{time.strftime('%Y%m%d-%H%M%S')}.bak"
    if os.path.exists(path):
        shutil.copy2(path, hist)
    with open(path, "wb") as fh:
        fh.write(blob)
    return {"ok": True, "file": rel, "bytes": len(blob),
            "backup": os.path.basename(bak), "history": os.path.basename(hist)}


BUNDLE_KIND = "acecm.gamesettings"


def export_bundle(files=None):
    """Decoded settings, as one shareable JSON document.

    Values, not bytes: a raw settings file is opaque, tied to the exact schema
    of one build, and impossible to review before applying. Decoded JSON can be
    read, diffed and edited by hand, and re-encoded against whatever schema the
    receiving machine actually has.

    ⚠ Bindings reference DEVICES. A wheel someone else does not own will not
    map onto their hardware, so device configuration is exported but flagged,
    and the importer skips those files unless asked for them explicitly.
    """
    from . import version
    picked, skipped = {}, {}
    for info in discover().get("files", []):
        rel = info["file"]
        if files and rel not in files:
            continue
        if not info.get("decodable"):
            if files:
                skipped[rel] = "no schema for this file"
            continue
        try:
            r = read(rel)
        except Exception as ex:
            # Export is a bulk operation over 1000+ files; one that cannot be
            # decoded is a note in the bundle, not a failed backup.
            skipped[rel] = f"{type(ex).__name__}: {ex}"
            continue
        if r.get("ok"):
            picked[rel] = {"message": r["message"], "values": r["values"]}
        else:
            skipped[rel] = r.get("error", "could not read")
    return {
        "kind": BUNDLE_KIND, "bundle_version": 1,
        "created": time.strftime("%Y-%m-%d %H:%M:%S"),
        "acecm": version.VERSION,
        "files": picked,
        "skipped": skipped,
        "note": "device bindings only make sense on identical hardware",
    }


def _is_device_file(rel):
    low = rel.lower()
    return "device" in low or "input_devices" in low


def import_bundle(bundle, only=None, include_devices=False):
    """Apply a bundle produced by export_bundle. Every file is backed up first."""
    if not isinstance(bundle, dict) or bundle.get("kind") != BUNDLE_KIND:
        return {"ok": False,
                "error": "that is not an ACECM game-settings bundle"}
    if _game_running():
        return {"ok": False, "error":
                "close the game first - it rewrites these files on exit and "
                "would overwrite whatever we import"}
    applied, failed, skipped = [], {}, {}
    for rel, entry in (bundle.get("files") or {}).items():
        if only and rel not in only:
            continue
        if _is_device_file(rel) and not include_devices:
            skipped[rel] = ("device bindings are tied to specific hardware - "
                            "tick 'include device bindings' to apply anyway")
            continue
        # ⚠ Re-encode against THIS machine's schema rather than trusting the
        # sender's. If a field no longer exists here, that is a clear error on
        # one file instead of a corrupt settings file.
        r = write(rel, entry.get("values") or {})
        if r.get("ok"):
            applied.append(rel)
        else:
            failed[rel] = r.get("error")
    return {"ok": not failed, "applied": applied, "failed": failed,
            "skipped": skipped}


def backups(rel):
    """Every timestamped copy we have taken of one settings file."""
    path = _safe_settings_path(rel)
    if not path:
        return {"ok": False, "file": rel, "backups": []}
    d, base = os.path.dirname(path), os.path.basename(path)
    out = []
    if os.path.isdir(d):
        for f in sorted(os.listdir(d), reverse=True):
            if f.startswith(base + ".") and f.endswith((".bak", ".bak_acecm")):
                p = os.path.join(d, f)
                out.append({"name": f, "size": os.path.getsize(p),
                            "mtime": int(os.path.getmtime(p))})
    return {"ok": True, "file": rel, "backups": out}


def restore_backup(rel, name):
    """Put a specific timestamped backup back."""
    path = _safe_settings_path(rel)
    if not path:
        return {"ok": False, "error": "bad settings path"}
    src = os.path.join(os.path.dirname(path), os.path.basename(name))
    if not os.path.isfile(src):
        return {"ok": False, "error": "no such backup"}
    if _game_running():
        return {"ok": False, "error": "close the game first"}
    # keep the current state too, so restoring is itself undoable
    if os.path.exists(path):
        shutil.copy2(path, f"{path}.{time.strftime('%Y%m%d-%H%M%S')}.bak")
    shutil.copy2(src, path)
    return {"ok": True, "restored": rel, "from": os.path.basename(src)}


def restore(rel):
    path = _safe_settings_path(rel)
    if not path:
        return {"ok": False, "error": "bad settings path"}
    bak = f"{path}.bak_acecm"
    if not os.path.isfile(bak):
        return {"ok": False, "error": "no ACECM backup for this file"}
    if _game_running():
        return {"ok": False, "error": "close the game first"}
    shutil.copy2(bak, path)
    return {"ok": True, "restored": rel}


def _game_running():
    from . import winproc
    return bool(winproc.pids_named("AssettoCorsaEVO"))


def state():
    return {"dir": settings_dir(), "game_running": _game_running()}
