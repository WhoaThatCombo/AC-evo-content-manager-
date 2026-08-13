"""Car and track inventory for the content manager.

The dedicated server's own view of content is what matters here - a car the
server cannot load is a broken join, no matter what the client has. So:

  cars    from the server's cars.json, split into genuine Kunos presets and
          mods. Kunos names are always "<code>_mech_<n>"; mods come through
          from a modded client as names truncated to 13 chars with no preset
          suffix, and the server's content.kspkg does not contain them.
  tracks  from events_practice.json, which is the same list the launcher
          indexes with EVENT_IDX - so the index here IS the launch index.
"""
import json
import os
import re

from . import config, install

MECH = re.compile(r".+_mech_\d+$")

# Human names for the manufacturer codes that appear in preset ids, so the UI
# can group by brand without a hand-written table per car.
_BRAND_FIX = {"ks": "", "bmw": "BMW", "amg": "Mercedes-AMG", "rs": "Audi Sport"}


def _pretty(name):
    """ks_bmw_m4_gt3_mech_0 -> BMW M4 GT3."""
    s = re.sub(r"_mech_\d+$", "", name)
    # Two id spaces exist. cars.json mostly uses preset_<code> (93 of 107),
    # while the server log records full model names (ks_bmw_m4_gt3). The codes
    # are not expandable without a lookup we do not have, so strip the prefix
    # and show the code honestly rather than inventing a name for it.
    s = re.sub(r"^(ks|preset)_", "", s)
    parts = [p for p in s.split("_") if p]
    out = []
    for p in parts:
        if len(p) <= 3 and p.isalpha() and p.lower() not in ("the", "and"):
            out.append(p.upper())
        else:
            out.append(p.capitalize())
    return " ".join(out)


def cars():
    """Every car the server knows about, annotated."""
    path = os.path.join(config.server_dir(), "cars.json")
    try:
        raw = json.load(open(path, encoding="utf-8"))["cars"]
    except Exception as ex:
        return {"error": f"cannot read cars.json: {ex}", "cars": []}
    # A mod's own .json declares display_name for the preset ids it ships.
    # Kunos presets are bound to a model folder via *.mechanicalcarpreset /
    # *.carfinalstate paths inside content.kspkg — see carsmap.py.
    declared = {}
    try:
        declared = install.car_names()
    except Exception:
        pass
    try:
        from . import carsmap
        cmap = carsmap.table()
        presets = cmap.get("presets") or {}
    except Exception:
        presets = {}
    # ⚠ carsmap is built from the BASE archive, so a mod's presets are never in
    # it and every modded car came back with no model at all. That is the one
    # group where it matters most: no model means no thumbnail, and mods are
    # exactly the cars someone picks by sight. Fall back to the ids the viewer
    # found inside the mod packages themselves.
    # ⚠ NOT called `known` - the mod loop below already uses that name for the
    # ids it has emitted, and shadowing it there would be a silent mess.
    viewer_ids = set()
    try:
        from . import viewer
        viewer_ids = {c["id"] for c in viewer.index().get("cars", [])}
    except Exception:
        pass

    def guess_model(name):
        """The model folder a preset belongs to, when carsmap cannot say.

        preset_mazda_rx_s_mech_1 -> ks_mazda_rx_s. Only ids the viewer actually
        found are returned, so this can name a model that does not exist.
        """
        if name in viewer_ids:
            return name
        code = re.sub(r"^preset_|_mech_\d+$", "", name)
        for cand in (f"ks_{code}", code):
            if cand in viewer_ids:
                return cand
        hits = [k for k in viewer_ids if code and code in k]
        return hits[0] if len(hits) == 1 else ""

    out = []
    for c in raw:
        name = c.get("name", "")
        is_kunos = bool(MECH.match(name))
        model = (presets.get(name) or presets.get(name.lower())
                 or guess_model(name))
        if name in declared:
            label = declared[name]
        elif model:
            label = _pretty(model)
        else:
            label = _pretty(name)
        out.append({
            "id": name,
            "model": model,
            "label": label,
            "named": name in declared or bool(model),
            "brand": (label.split(" ") or [""])[0],
            "kunos": is_kunos,
            # a mod is anything the server's own package cannot resolve; letting
            # a player pick one is a broken join, so the UI flags it loudly
            "mod": not is_kunos,
        })
    # ⚠ Cars shipped by an INSTALLED mod are not in cars.json at all (that file
    # came from a client dump), so they were missing from the allowed-cars list
    # entirely - the mod installs fine and is then unselectable. Merge them in.
    known = {c["id"] for c in out}
    for cid, label in declared.items():
        if cid not in known:
            # ⚠ Same fallback as above. Cars declared by a mod's own manifest
            # are appended HERE, not in the loop over cars.json - so fixing the
            # model only up there left every modded car without one, which is
            # precisely the group that needs a picture.
            model = (presets.get(cid) or presets.get(cid.lower())
                     or guess_model(cid))
            out.append({"id": cid, "model": model, "label": label,
                        "brand": label.split(" ")[0],
                        "kunos": False, "mod": True, "named": True,
                        "from_mod": True})
    out.sort(key=lambda c: (not c["kunos"], c["label"]))
    return {"cars": out, "total": len(out),
            "kunos": sum(1 for c in out if c["kunos"]),
            "mods": sum(1 for c in out if c["mod"]),
            "from_mods": sum(1 for c in out if c.get("from_mod"))}


def tracks():
    """Track/layout list, indexed exactly as the launcher's EVENT_IDX."""
    path = os.path.join(config.server_dir(), "events_practice.json")
    try:
        evs = json.load(open(path, encoding="utf-8"))["events"]
    except Exception as ex:
        return {"error": f"cannot read events_practice.json: {ex}", "tracks": []}
    out = []
    for i, e in enumerate(evs):
        out.append({
            "index": i,                      # this IS EVENT_IDX
            "track": e.get("track", "?"),
            "layout": e.get("layout", "?"),
            "name": e.get("event_name") or _pretty(e.get("track", "")),
            "label": f"{_pretty(e.get('track',''))} - "
                     f"{_pretty(re.sub('^layout_', '', e.get('layout','')))}",
            "length_m": e.get("track_length"),
        })
    return {"tracks": out, "total": len(out)}


def models_seen():
    """Real car model names harvested from the server log.

    Every join records "on car <model>", so this is a truthful catalogue of what
    has actually run on the server - unlike the preset_<code> ids in cars.json,
    these are the full names (ks_bmw_m4_gt3). The two id spaces are NOT mapped
    to each other here; doing so by guesswork would put wrong names on cars.
    """
    log = config.server_log()
    try:
        txt = open(log, encoding="utf-8", errors="replace").read()
    except OSError:
        return {"models": [], "note": "no server log yet"}
    seen = sorted(set(re.findall(r"on car ([a-z0-9_]+),", txt)))
    return {"models": [{"id": m, "label": _pretty(m)} for m in seen],
            "total": len(seen)}


def installed_tracks():
    """Track folders actually present on disk, so the UI can flag a profile
    pointing at content the server does not have."""
    d = os.path.join(config.server_dir(), "content", "tracks")
    try:
        return sorted(x for x in os.listdir(d)
                      if os.path.isdir(os.path.join(d, x)))
    except OSError:
        return []
