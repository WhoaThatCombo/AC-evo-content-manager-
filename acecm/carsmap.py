"""preset_<code>_mech_<n>  ->  ks_<model>  from the live archive.

Two id spaces exist in this game. cars.json and the lobby speak presets
(`preset_m4gt3_mech_1`); the server log and the kspkg folder speak models
(`ks_bmw_m4_gt3`). The binding is on disk: every mechanical preset and
car-final-state lives under content\\cars\\<model>\\presets\\<preset id>....

Walking the 64 MiB kspkg index is seconds, not minutes, and we cache against
the archive mtime so the Cars page does not pay that on every refresh.
"""
import json
import os
import re
import time

from . import config, detect, install, kspkg

CACHE = os.path.join(config.DATA, "preset_map.json")
# preset_718gt4rs_mech_2  OR  ks_caterham_485_csr_mech_2
_PRESET = re.compile(r"(?:^|_)((?:preset_)?[a-z0-9]+_mech_\d+)(?:_|\.|$)", re.I)
_MODEL_DIR = re.compile(r"^content\\cars\\([^\\]+)\\", re.I)

_mem = {"key": None, "data": None}


def _kspkg_path():
    for c in (
        os.path.join(config.server_dir() or "", "content.kspkg"),
        os.path.join(detect.find("game_dir") or "", "content.kspkg"),
    ):
        if c and os.path.isfile(c):
            return c
    return ""


def _key(path):
    try:
        st = os.stat(path)
        return f"{path}|{st.st_mtime_ns}|{st.st_size}"
    except OSError:
        return path


def _preset_ids(filename):
    """Mechanical-preset ids encoded in a filename.

    carfinalstate names are long compounds
    (`ks_foo_preset_r5gt_mech_1_preset_r5gt_visual_1`); only the `preset_*_mech_N`
    tokens are ids. mechanicalcarpreset files are the id itself
    (`preset_695b_mech_1` or `ks_caterham_485_csr_mech_2`).
    """
    found = []
    for m in re.finditer(r"preset_[a-z0-9]+_mech_\d+", filename, re.I):
        found.append(m.group(0).lower())
    if found:
        return found
    stem = filename.rsplit(".", 1)[0]
    if re.fullmatch(r"[a-z0-9_]+_mech_\d+", stem, re.I):
        found.append(stem.lower())
    return found


def _scan(path):
    presets = {}
    models = {}
    n = 0
    for entry, _size, _off in kspkg.iter_entries(path):
        n += 1
        low = entry.lower()
        m = _MODEL_DIR.match(low)
        if not m:
            continue
        model = m.group(1)
        if low.endswith(".mechanicalcarpreset") or low.endswith(".carfinalstate"):
            base = kspkg.entry_name(low)
            for pid in _preset_ids(base):
                # mechanicalcarpreset is the authoritative id; a later
                # carfinalstate for the same preset must not overwrite it
                # with a different model (it won't, they share a folder).
                presets.setdefault(pid, model)
                # Caterham etc. ship as ks_<model>_mech_N. Don't invent a
                # preset_ks_* key — cars.json uses the ks_* id as-is.
        models.setdefault(model, kspkg.entry_dir(entry))
    return {
        "kspkg": path,
        "built": int(time.time()),
        "entries_seen": n,
        "presets": presets,
        "models": sorted(models),
    }


def _load_cache():
    try:
        return json.load(open(CACHE, encoding="utf-8"))
    except Exception:
        return None


def table(refresh=False):
    path = _kspkg_path()
    if not path:
        return {"presets": {}, "models": [], "error": "content.kspkg not found"}
    key = _key(path)
    if not refresh and _mem["key"] == key and _mem["data"]:
        return _mem["data"]
    cached = None if refresh else _load_cache()
    if cached and cached.get("kspkg") == path and cached.get("key") == key:
        _mem["key"], _mem["data"] = key, cached
        return cached
    data = _scan(path)
    data["key"] = key
    # mods declare their own ids; they win on collision because they are
    # what the player actually installed under that preset name
    try:
        for pid, label in install.car_names().items():
            data.setdefault("mod_names", {})[pid] = label
    except Exception:
        data["mod_names"] = {}
    os.makedirs(config.DATA, exist_ok=True)
    json.dump(data, open(CACHE, "w", encoding="utf-8"))
    _mem["key"], _mem["data"] = key, data
    return data


def model_for(preset_id):
    if not preset_id:
        return ""
    presets = table().get("presets") or {}
    return presets.get(preset_id) or presets.get(preset_id.lower()) or ""


def label_for(preset_id, fallback=""):
    """A name we are willing to show in the UI.

    Mod manifests are trusted. For Kunos cars we show the model folder
    pretty-printed (ks_bmw_m4_gt3 -> BMW M4 GT3), never a guessed expansion
    of the short preset code.
    """
    mods = (table().get("mod_names") or {})
    if preset_id in mods:
        return mods[preset_id]
    model = model_for(preset_id)
    return fallback or model
