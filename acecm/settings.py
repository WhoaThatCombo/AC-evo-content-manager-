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
    return config.CFG.get("ace_dir") or ACE_DIR


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


def read(rel):
    """Decode one settings file to plain JSON."""
    from google.protobuf import json_format
    path = os.path.join(settings_dir(), rel.replace("/", os.sep))
    if not os.path.isfile(path):
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
        "values": json_format.MessageToDict(obj,
                                           preserving_proto_field_name=True,
                                           always_print_fields_with_no_presence=True),
    }


def write(rel, values):
    """Encode JSON back into the settings file, with a backup."""
    from google.protobuf import json_format
    path = os.path.join(settings_dir(), rel.replace("/", os.sep))
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


def restore(rel):
    path = os.path.join(settings_dir(), rel.replace("/", os.sep))
    bak = f"{path}.bak_acecm"
    if not os.path.isfile(bak):
        return {"ok": False, "error": "no ACECM backup for this file"}
    if _game_running():
        return {"ok": False, "error": "close the game first"}
    shutil.copy2(bak, path)
    return {"ok": True, "restored": rel}


def _game_running():
    import subprocess
    try:
        out = subprocess.run(
            ["powershell.exe", "-NoProfile", "-Command",
             "(Get-Process -Name 'AssettoCorsaEVO' -ErrorAction SilentlyContinue "
             "| Measure-Object).Count"],
            capture_output=True, text=True, timeout=15).stdout.strip()
        return out.isdigit() and int(out) > 0
    except Exception:
        return False


def state():
    return {"dir": settings_dir(), "game_running": _game_running()}
