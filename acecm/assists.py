"""Read and write the game's driving assists.

The client keeps assists in the user profile, not in content:

    %USERPROFILE%\\Saved Games\\ACE\\AssistSettings\\
        AssistPresetListType_CUSTOM.racingsettingsassistpreset   the CUSTOM preset
        current.assistpresetsave                                 which preset is active

Both hold a `RacingSettingsAssistPreset`. Writing them is how the in-game
Assists page persists a change, so editing them here is the same operation the
game performs - no patching, nothing in content.kspkg, and joining a server is
unaffected (the server hashes CAR CONTENT, not the profile).

⚠ damage_rate is stored OFFSET BY ONE: the game computes
`displayed% = (damage_rate + 1) * 100`, so 0.0 means 100% damage (normal) and
-1.0 means none. Verified in the client's own UI code, which converts both ways
(`value/100 - 1` on write, `(value+1)*100` on read). Do not "fix" a stored 0.

⚠ RacingSetting is 0=OFF, 1=ON, 2=FREE. A preset built from scratch would
therefore force every assist OFF, so we always PARSE the existing file and
change only the fields asked for.
"""
import os
import shutil
import time

from . import logs, protos

MSG = "RacingSettingsAssistPreset"

# field name -> ("toggle" | "float", label)
# (field, label, inverted) - LABELS AND ORDER MATCH THE GAME'S OWN ASSISTS
# SCREEN, so what you set here reads the same as what you see in game.
#
# ⚠ manual_ignition is INVERTED. The game shows "AUTOMATIC IGNITION" while the
# field stores the opposite, and the built-in presets prove it: BEGINNER has
# every assist ON (1) yet manual_ignition 0, and a profile showing "AUTOMATIC
# IGNITION: ON" stores 0. Presenting the raw field meant ACECM said "Manual
# ignition: Off" for what the game calls "Automatic ignition: On" - true, but
# impossible to reconcile with the screen you are copying from.
TOGGLES = [
    ("manual_ignition", "Automatic ignition", True),
    ("automatic_gearbox", "Automatic gearbox", False),
    ("automatic_pit_limiter", "Automatic pit limiter", False),
    ("automatic_clutch", "Automatic clutch", False),
    ("automatic_clutch_on_start", "Automatic clutch on start", False),
    ("standing_start_assist", "Standing start assist", False),
    ("automatic_blip", "Automatic throttle blip", False),
    ("automatic_lights", "Automatic headlights", False),
    ("automatic_wipers", "Automatic wipers", False),
    ("ideal_line", "Ideal line", False),
]


def _flip(v, inverted):
    """on <-> off for an inverted field; FREE has no opposite and is kept."""
    if not inverted:
        return v
    return {"on": "off", "off": "on"}.get(v, v)
RACING_SETTING = {0: "off", 1: "on", 2: "free"}
RACING_SETTING_REV = {v: k for k, v in RACING_SETTING.items()}


def dir_path():
    return os.path.join(os.path.expanduser("~"), "Saved Games", "ACE",
                        "AssistSettings")


def _custom():
    return os.path.join(dir_path(),
                        "AssistPresetListType_CUSTOM.racingsettingsassistpreset")


def _current():
    return os.path.join(dir_path(), "current.assistpresetsave")


def _load(path, msgname):
    m = protos.new(msgname)
    if os.path.isfile(path):
        with open(path, "rb") as f:
            m.ParseFromString(f.read())
    return m


def _preset_of(save):
    """current.assistpresetsave wraps the preset in field `preset`."""
    return save.preset


def read():
    """Current assist values, plus how the game would display damage."""
    d = dir_path()
    if not os.path.isdir(d):
        return {"ok": False, "error": "no AssistSettings folder - run the game once",
                "dir": d}
    try:
        p = _load(_custom(), MSG)
    except Exception as ex:
        logs.LOG.info("assists: cannot read preset: %s", ex)
        return {"ok": False, "error": f"unreadable preset: {ex}", "dir": d}
    out = {"ok": True, "dir": d, "toggles": [], }
    for name, label, inv in TOGGLES:
        try:
            v = int(getattr(p, name))
        except Exception:
            v = 0
        out["toggles"].append({"name": name, "label": label,
                               "value": _flip(RACING_SETTING.get(v, "off"), inv)})
    out["stability_control"] = round(float(getattr(p, "stability_control", 0.0)) * 100)
    # stored -1..0  ->  shown 0..100 %
    out["damage_pct"] = round((float(getattr(p, "damage_rate", 0.0)) + 1.0) * 100)
    return out


def write(values):
    """Apply only the keys present in `values`; everything else is preserved."""
    d = dir_path()
    if not os.path.isdir(d):
        return {"ok": False, "error": "no AssistSettings folder - run the game once"}
    stamp = time.strftime("%Y%m%d-%H%M%S")
    changed = []
    for path, wrap in ((_custom(), False), (_current(), True)):
        if not os.path.isfile(path):
            continue
        try:
            msg = _load(path, "AssistPresetSave" if wrap else MSG)
        except Exception as ex:
            return {"ok": False, "error": f"unreadable {os.path.basename(path)}: {ex}"}
        p = _preset_of(msg) if wrap else msg
        for name, _label, inv in TOGGLES:
            if name in values:
                v = RACING_SETTING_REV.get(
                    _flip(str(values[name]).lower(), inv))
                if v is not None:
                    setattr(p, name, v)
                    changed.append(name)
        if "stability_control" in values:
            try:
                p.stability_control = max(0.0, min(1.0,
                                                   float(values["stability_control"]) / 100.0))
                changed.append("stability_control")
            except (TypeError, ValueError):
                pass
        if "damage_pct" in values:
            try:
                pct = max(0.0, min(100.0, float(values["damage_pct"])))
                p.damage_rate = pct / 100.0 - 1.0      # see module docstring
                changed.append("damage_pct")
            except (TypeError, ValueError):
                pass
        # ⚠ Changing anything makes it a CUSTOM preset, exactly as the game's
        # own Assists page does - otherwise the game would show e.g. "PRO"
        # while the values no longer match it.
        try:
            p.type = 4      # AssistPresetListType_CUSTOM
        except Exception:
            pass
        blob = msg.SerializeToString()
        shutil.copyfile(path, f"{path}.{stamp}.bak")
        with open(path, "wb") as f:
            f.write(blob)
    if not changed:
        return {"ok": False, "error": "nothing to change"}
    logs.LOG.info("assists updated: %s", ", ".join(sorted(set(changed))))
    r = read()
    r["saved"] = sorted(set(changed))
    return r
