"""Choosing a car's colours without opening the game's paint shop.

Everything here was established by experiment against a real profile; the
findings that matter are recorded as comments because none of them is obvious
from the data alone.

WHAT A CAR LOOKS LIKE, on disk:

    ProfileData\\<profile>\\OpenData\\SavedCars\\<model>_<uuid>
        .carfinalstatewithconsumable          - protobuf, CarFinalStateWithConsumable

    multilayer_states {                        - a map keyed by SLOT NAME
      key: "EXT SKIN"                          - the same names the .design lists
      value {
        material_replacement_path: ...
        primary_channel_state {
          color { x y z }                      - copied from the colour file
          roughness / clear_coat / ...         - likewise, when it has them
          oem_color_path: ...oemmultilayercolor
        }
      }
    }

⚠ The record is AUTHORITATIVE - the game reads it at startup, and a bad one
crashes it on a resource worker thread before any menu appears.

⚠ There is NO checksum. A decode/re-encode round trip is accepted even though
it reorders fields, and the `a`/`b` uint64 pair beside the guid is simply that
guid big-endian, not a hash of anything.

⚠ The channel state must AGREE with the path. Changing oem_color_path alone -
even to a colour the car allows - crashes the game. The game itself writes the
state one change late (it stores the previous colour's values beside the new
path), which is a quirk to ignore, not to copy: we write values that match.

⚠ Only offer colours from the CAR's own .design files. The brand's colour
folder is a superset; ks_renault has 45 while the A110 S allows 12 on its
exterior skin.
"""
import os
import re
import struct
import uuid as _uuid

from . import config, kspkg, logs, protos, viewer

RECORD = "CarFinalStateWithConsumable"


def _saved_cars_dir():
    """<Saved Games>\\ACE\\ProfileData\\<profile>\\OpenData\\SavedCars."""
    from . import detect
    root = os.path.join(detect.saved_games(), "ACE", "ProfileData")
    if not os.path.isdir(root):
        return ""
    for name in sorted(os.listdir(root)):
        d = os.path.join(root, name, "OpenData", "SavedCars")
        if os.path.isdir(d):
            return d
    return ""


def garage():
    """Every owned car: model, instance id, and the colour of each slot."""
    d = _saved_cars_dir()
    out = []
    if not d:
        return out
    for name in sorted(os.listdir(d)):
        if not name.endswith(".carfinalstatewithconsumable"):
            continue
        path = os.path.join(d, name)
        stem = name[:-len(".carfinalstatewithconsumable")]
        # <model>_<uuid>, and the uuid is the last five dash-joined groups
        m = re.match(r"^(.*)_([0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12})$",
                     stem, re.I)
        model, inst = (m.group(1), m.group(2)) if m else (stem, "")
        entry = {"model": model, "instance": inst, "file": path, "slots": {}}
        try:
            rec = _load(path)
            for slot, st in _slots(rec).items():
                entry["slots"][slot] = _current_color(st)
        except Exception as ex:
            logs.LOG.warning("livery: reading %s: %s", name, ex)
            entry["error"] = str(ex)
        out.append(entry)
    return out


def _load(path):
    msg = protos.new(RECORD)
    if msg is None:
        raise RuntimeError("the game's message schemas are not available")
    with open(path, "rb") as f:
        raw = f.read()
    used = msg.ParseFromString(raw)
    if used != len(raw):
        raise ValueError("record did not decode cleanly")
    return msg


def _slots(rec):
    """The multilayer_states map, wherever it sits in the record.

    ⚠ It is nested several levels down, not on the root message, so this
    walks rather than guessing at a path - the layout differs between a car
    with consumables and one without.
    """
    for holder, _depth in _walk(rec):
        states = getattr(holder, "multilayer_states", None)
        if states:
            return states
    return {}


def _current_color(state):
    ch = getattr(state, "primary_channel_state", None)
    return getattr(ch, "oem_color_path", "") if ch else ""


# ------------------------------------------------------------- colours --
def color_info(path, pkg=None):
    """Name, rgb and material params out of a .oemmultilayercolor.

    The file is a small protobuf: a display name, then a sub-message holding
    rgb followed by whichever material scalars that paint defines. Read by
    field number, because the values map straight onto the channel state and
    a missing one must stay missing rather than become a default.
    """
    pkg = pkg or viewer.package()
    want = path.lower().replace("/", "\\")
    blob = None
    with open(pkg, "rb") as f:
        for p, s, o in kspkg.iter_entries(pkg):
            if p.lower().replace("/", "\\") == want:
                blob = bytes(kspkg.read_entry(f, s, o, p))
                break
    if blob is None:
        return {}
    out = {"path": path, "name": "", "rgb": None}
    # ⚠ Walk the fields properly. The first version searched for the 0x1a tag
    # byte with blob.find(), which silently matched a NAME LENGTH instead:
    # "Alpine Carpaint Abyss Blue" is 26 chars, and 26 == 0x1a. That colour
    # then read as having no rgb, and applying it failed while its neighbours
    # worked - a bug that only appears for names of one particular length.
    i, fields = 0, {}
    while i < len(blob):
        tag, i = _varint(blob, i)
        if tag is None:
            break
        num, wire = tag >> 3, tag & 0x07
        if wire == 2:                       # length-delimited
            ln, i = _varint(blob, i)
            if ln is None or i + ln > len(blob):
                break
            chunk, i = blob[i:i + ln], i + ln
            if num == 2:
                out["name"] = chunk.decode("utf-8", "replace")
            elif num == 3:                  # the colour sub-message
                j = 0
                while j + 4 < len(chunk) + 1 and j < len(chunk):
                    t = chunk[j]
                    if t & 0x07 != 5:       # 32-bit floats only
                        break
                    fields[t >> 3] = struct.unpack_from("<f", chunk, j + 1)[0]
                    j += 5
        elif wire == 5:
            # ⚠ The material params live at TOP level, beside the colour
            # sub-message rather than inside it.
            fields.setdefault(num, struct.unpack_from("<f", blob, i)[0])
            i += 4
        elif wire == 1:
            i += 8
        elif wire == 0:
            _, i = _varint(blob, i)
        else:
            break
    # ⚠ A zero component is OMITTED (proto3 does not write defaults), so a
    # colour with no red - "Alpine Blue" - had only two of the three fields
    # and read as unparseable. Missing means 0.0, not missing.
    if any(k in fields for k in (1, 2, 3)):
        out["rgb"] = (fields.get(1, 0.0), fields.get(2, 0.0),
                      fields.get(3, 0.0))
    # field numbers observed on real paints; absent ones stay absent
    for num, key in ((4, "metalness"), (5, "roughness"),
                     (6, "clear_coat"), (7, "normal_intensity")):
        if num in fields:
            out[key] = fields[num]
    return out


def designs(model, pkg=None):
    """Every design this car ships, decoded.

    ⚠ Decoded as DesignData, NOT scraped. The first version of this regexed
    slot-shaped words and colour paths out of the raw bytes and paired them by
    position; it found nothing at all on a real car, and the failure mode of a
    scraper that half-works is worse - it would offer colours the car does not
    allow, which is precisely what crashes the game.
    """
    pkg = pkg or viewer.package()
    prefix = f"content\\cars\\{model}\\skins\\".lower()
    out = []
    with open(pkg, "rb") as f:
        found = [(p, s, o) for p, s, o in kspkg.iter_entries(pkg)
                 if p.lower().replace("/", "\\").startswith(prefix)
                 and p.lower().endswith(".design")]
        for p, s, o in found:
            blob = bytes(kspkg.read_entry(f, s, o, p))
            msg = protos.new("DesignData")
            if msg is None:
                break
            try:
                if msg.ParseFromString(blob) != len(blob):
                    continue
            except Exception as ex:
                logs.LOG.info("livery: %s did not decode: %s", p, ex)
                continue
            slots = {}
            for cm in msg.customizable_materials:
                # `slot` is the key the garage record uses; `description` is
                # what the game shows. They differ ("EXT_SKIN" / "EXT SKIN"),
                # and the record is keyed by the DESCRIPTION.
                colours = list(getattr(cm.multilayer, "oem_color_paths", []))
                slots[cm.description or cm.slot] = {
                    "slot": cm.slot,
                    "material": cm.material_replacement_path,
                    "colors": colours,
                }
            out.append({"path": p,
                        "name": msg.description or os.path.basename(p),
                        "slots": slots})
    return out


def _varint(buf, i):
    """One protobuf varint, or (None, i) at the end."""
    shift = val = 0
    while i < len(buf):
        b = buf[i]
        i += 1
        val |= (b & 0x7F) << shift
        if not b & 0x80:
            return val, i
        shift += 7
        if shift > 63:
            break
    return None, i


def allowed(model, pkg=None):
    """Slot -> [colour paths] this CAR permits, across all its designs."""
    out = {}
    for d in designs(model, pkg):
        for slot, info in d["slots"].items():
            have = out.setdefault(slot, [])
            for c in info["colors"]:
                if c not in have:
                    have.append(c)
    return out


# --------------------------------------------------------------- apply --
def apply_color(car_file, slot, color_path, dry_run=False):
    """Set one slot's colour, writing a channel state that AGREES with it."""
    info = color_info(color_path)
    if not info or not info.get("rgb"):
        return {"ok": False, "error": f"could not read {color_path}"}
    rec = _load(car_file)
    states = _slots(rec)
    if slot not in states:
        return {"ok": False,
                "error": f"this car has no slot called {slot!r}",
                "slots": sorted(states.keys())}
    ch = states[slot].primary_channel_state
    ch.oem_color_path = color_path
    ch.color.x, ch.color.y, ch.color.z = info["rgb"]
    for key in ("metalness", "roughness", "clear_coat", "normal_intensity"):
        if key in info:
            setattr(ch, key, info[key])
        elif ch.HasField(key) if _optional(ch, key) else False:
            ch.ClearField(key)

    # ⚠ A fresh id, in BOTH forms: the guid string and the same 128 bits as
    # two big-endian uint64s. They must not disagree.
    new = _uuid.uuid4()
    _set_guid(rec, new)

    blob = rec.SerializeToString()
    if dry_run:
        return {"ok": True, "dry_run": True, "bytes": len(blob),
                "color": info, "guid": str(new)}
    bak = car_file + ".bak_livery"
    if not os.path.exists(bak):
        with open(car_file, "rb") as src, open(bak, "wb") as dst:
            dst.write(src.read())
    with open(car_file, "wb") as f:
        f.write(blob)
    logs.LOG.info("livery: %s -> %s", os.path.basename(car_file),
                  os.path.basename(color_path))
    return {"ok": True, "bytes": len(blob), "color": info,
            "guid": str(new), "backup": bak}


def _optional(msg, field):
    try:
        return msg.DESCRIPTOR.fields_by_name[field].has_presence
    except Exception:
        return False


def _set_guid(rec, val):
    """Write a uuid everywhere the record keeps one, keeping both forms equal."""
    hi, lo = struct.unpack(">QQ", val.bytes)
    done = []
    for holder, _ in _walk(rec):
        if hasattr(holder, "guid") and isinstance(
                getattr(holder, "guid", None), str) and getattr(holder, "guid"):
            holder.guid = str(val)
            done.append("guid")
        for name in ("id", "uid", "hash"):
            sub = getattr(holder, name, None)
            if sub is not None and hasattr(sub, "a") and hasattr(sub, "b"):
                sub.a, sub.b = hi, lo
                done.append(name)
    return done


def _walk(msg, depth=0):
    """Every sub-message, depth-first.

    ⚠ Duck-typed on purpose. FieldDescriptor.label does not exist in the upb
    (C) protobuf implementation, and using it raised an AttributeError that
    got swallowed by a caller's try/except - so every lookup silently found
    nothing instead of failing loudly.
    """
    if depth > 6:
        return
    yield msg, depth
    for f, v in msg.ListFields():
        if f.message_type is None:
            continue
        if hasattr(v, "ListFields"):          # single message
            items = [v]
        elif hasattr(v, "values"):            # map field
            items = list(v.values())
        else:
            try:
                items = list(v)               # repeated
            except TypeError:
                items = []
        for it in items:
            if hasattr(it, "ListFields"):
                yield from _walk(it, depth + 1)

# ------------------------------------------------------------- populate --
def states_in_package(pkg=None):
    """model -> the .carfinalstate files the GAME ships for it.

    These are the game's own complete states for each car and preset combo -
    same car_data, actor, mechanical/visual preset and material slots a real
    saved car holds. Verified against an owned car: the shipped state for
    ks_alpine_a110_s decodes as CarFinalStateData cleanly and carries the
    identical paths. So a car nobody owns can still be given a truthful
    record, rather than one invented field by field.
    """
    pkg = pkg or viewer.package()
    sep = chr(92)
    out = {}
    for path, _s, _o in kspkg.iter_entries(pkg):
        low = path.lower().replace("/", sep)
        if not low.endswith(".carfinalstate"):
            continue
        parts = low.split(sep)
        if len(parts) > 2 and parts[0] == "content" and parts[1] == "cars":
            out.setdefault(parts[2], []).append(path)
    for v in out.values():
        v.sort()
    return out


def populate(dry_run=False, pkg=None):
    """Give every car in the game a saved car, so its liveries can be picked.

    ⚠ The garage only ever held cars you OWN - nine here against a hundred in
    the game - and the livery picker is driven by the garage, so there was no
    way to choose a colour for anything else. This writes the missing records
    from the game's own shipped state for that car.

    Existing files are never touched: a car you already own keeps whatever you
    have done to it.
    """
    d = _saved_cars_dir()
    if not d:
        return {"ok": False, "error": "no SavedCars folder - launch the game "
                                      "once so it creates your profile"}
    pkg = pkg or viewer.package()
    if not pkg:
        return {"ok": False, "error": "content.kspkg not found"}
    have = {e["model"] for e in garage()}
    shipped = states_in_package(pkg)
    todo = sorted(m for m in shipped if m not in have)
    made, failed = [], []
    if dry_run:
        return {"ok": True, "dry_run": True, "would_add": todo,
                "already": sorted(have), "count": len(todo)}

    with open(pkg, "rb") as f:
        offs = {}
        for path, size, off in kspkg.iter_entries(pkg):
            if path.lower().endswith(".carfinalstate"):
                offs[path] = (size, off)
        for model in todo:
            try:
                src = shipped[model][0]
                size, off = offs[src]
                blob = bytes(kspkg.read_entry(f, size, off, src))
                data = protos.new("CarFinalStateData")
                if data is None:
                    raise RuntimeError("CarFinalStateData schema missing")
                if data.ParseFromString(blob) != len(blob):
                    raise ValueError("shipped state did not decode cleanly")
                rec = protos.new(RECORD)
                rec.final_state.CopyFrom(data)
                new = _uuid.uuid4()
                _set_guid(rec, new)
                out = rec.SerializeToString()
                # never write something we cannot read back
                check = protos.new(RECORD)
                if check.ParseFromString(out) != len(out):
                    raise ValueError("record did not round-trip")
                name = f"{model}_{new}.carfinalstatewithconsumable"
                with open(os.path.join(d, name), "wb") as fh:
                    fh.write(out)
                made.append(model)
            except Exception as ex:
                logs.LOG.warning("livery: could not add %s: %s", model, ex)
                failed.append({"model": model, "error": str(ex)})
    logs.LOG.info("livery: added %d saved car(s)", len(made))
    return {"ok": True, "added": made, "failed": failed,
            "count": len(made), "already": len(have)}


def depopulate():
    """Remove ONLY the records populate() created - anything with no mileage.

    ⚠ Judged by consumable status, not by a list we keep: a car you have
    actually driven has wear on it, and must survive this.
    """
    d = _saved_cars_dir()
    if not d:
        return {"ok": False, "error": "no SavedCars folder"}
    gone, kept = [], []
    for e in garage():
        try:
            rec = _load(e["file"])
            body = getattr(rec.car_consumable_status, "body", None)
            driven = bool(getattr(body, "hundred_meters", 0))
        except Exception:
            driven = True          # unreadable: leave it alone
        if driven:
            kept.append(e["model"])
            continue
        try:
            os.remove(e["file"])
            gone.append(e["model"])
        except OSError as ex:
            logs.LOG.warning("livery: removing %s: %s", e["model"], ex)
    return {"ok": True, "removed": gone, "kept": kept}
