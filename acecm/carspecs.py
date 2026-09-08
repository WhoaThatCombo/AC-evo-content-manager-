"""Headline specs for a car, read out of the game's own physics files.

Nothing here is a lookup table of manufacturer figures - every number comes
from the files the game itself drives with, so a modded or retuned car reports
what it will actually do rather than what its readme claims.

Where each number lives, per car folder in content.kspkg:

    *.car          CarData   - general.totalMass, general.fuel, screenName
    *.drivetrain   DrivetrainData.tractionType  - RWD / FWD / AWD
    *.gearbox      GearBoxData.gear_count, final_ratio
    *.aicardata    AiCarData - redline, and the AI's top-speed hint
    *.carengine    EngineData - limiter, and the path to the power curve
    *.curve        the power curve itself

⚠ POWER IS DERIVED, not stated. The curve holds engine torque BEFORE boost, so
peak power is max over the curve of `Nm x (1 + max_boost) x rpm / 7120.6`.
Checked against cars whose real output is known:

    AE86    123 hp computed vs 128 real   (naturally aspirated)
    MX5 NA  113 hp computed vs 116 real   (naturally aspirated)
    M2      481 hp computed vs 473 real   (1.55 bar turbo)

- within a few percent, which is close enough to print. It is not exact: boost
builds with rpm rather than arriving flat, and this uses max_boost throughout.

⚠ TOP SPEED IS A HINT, not a measurement. AiCarData.physics_hints.topspeed_kmh
is what the AI assumes, and it ignores a manufacturer limiter - the M2 reports
335 km/h against a real 250. It is labelled as an estimate in the UI for that
reason, and it is the game's own number rather than one we invented.
"""
import json
import os
import struct
import time

from . import config, kspkg, logs, viewer

CACHE = os.path.join(config.DATA, "car_specs.json")
# ⚠ Bump when the shape changes, or an existing cache is served forever - the
# archive mtime alone cannot know that WE started reporting something new.
CACHE_V = 2

_mem = {"key": None, "data": None}

# hp = Nm x rpm / this.  kW = Nm.rpm/9549, and one metric hp is 0.7457 kW,
# so the divisor is 9549 x 0.7457 = 7121.
_NM_RPM_TO_HP = 9549.0 * 0.7457


def _varint(b, i):
    r = s = 0
    while True:
        x = b[i]
        i += 1
        r |= (x & 0x7F) << s
        s += 7
        if not x & 0x80:
            return r, i


def _fields(b):
    """Every top-level field as (number, value), without needing a schema."""
    i, out = 0, []
    while i < len(b):
        try:
            k, i = _varint(b, i)
        except IndexError:
            break
        fn, wt = k >> 3, k & 7
        try:
            if wt == 2:
                ln, i = _varint(b, i)
                out.append((fn, b[i:i + ln]))
                i += ln
            elif wt == 5:
                out.append((fn, struct.unpack_from("<f", b, i)[0]))
                i += 4
            elif wt == 0:
                v, i = _varint(b, i)
                out.append((fn, v))
            elif wt == 1:
                i += 8
            else:
                break
        except (IndexError, struct.error):
            break
    return out


def _curve_points(blob):
    """(rpm, value) pairs from a .curve.

    ⚠ Parsed by hand rather than as CurveData. A curve entry carries its value
    in field 10, but CurveData's Vector2Data declares only x/y (1/2), so
    parsing it with the schema silently returned every y as zero.
    """
    pts = []
    for fn, val in _fields(blob):
        if fn != 1 or not isinstance(val, bytes):
            continue
        d = dict(_fields(val))
        x, y = d.get(1), d.get(10)
        if isinstance(x, float) and isinstance(y, float):
            pts.append((x, y))
    return pts


def _build():
    """One pass over every car package, collecting what each car needs.

    ⚠ ALL packages, not just the base one. A mod ships its physics in its own
    .kspkg, so scanning content.kspkg alone gave specs to stock cars and none
    to any modded one. viewer.packages() finds them from the mods folder, so
    nothing here names a particular mod.
    """
    out, pre, seen_pkgs = {}, {}, []
    try:
        pkgs = [pk for _m, pk in viewer.packages()]
    except Exception:
        pkgs = [viewer.package()]
    # ⚠ A mod's engine can point at a curve that lives in the BASE package -
    # a cloned car keeps the original's data paths - so a mod-only curve
    # lookup found nothing and the car reported a fraction of its real power
    # (the M8 clone read 196 hp against ~617). Index the base curves once and
    # let every package fall back to them.
    base_pkg = viewer.package()
    base_curves = {}
    if base_pkg and os.path.isfile(base_pkg):
        try:
            base_curves = _curve_index(base_pkg)
        except Exception as ex:
            logs.LOG.info("carspecs base curves: %s", ex)
    for pk in pkgs:
        if not pk or pk in seen_pkgs or not os.path.isfile(pk):
            continue
        seen_pkgs.append(pk)
        try:
            cars, presets = _scan_pkg(pk, base_pkg, base_curves)
            for model, spec in cars.items():
                # a mod owns its model outright; base entries never collide
                if spec:
                    out.setdefault(model, spec)
            for pid, spec in presets.items():
                pre.setdefault(pid, spec)
        except Exception as ex:
            logs.LOG.info("carspecs %s: %s", os.path.basename(pk), ex)
    return {"cars": out, "presets": pre, "built": int(time.time()),
            "packages": len(seen_pkgs), "v": CACHE_V}


_SPEC_KEYS = {
    "POWER": ("power_hp", 1.0),
    "TORQUE": ("torque_nm", 1.0),
    "WEIGHT": ("mass_kg", 1.0),
    "MAX_SPEED": ("top_speed_kmh", 1.0),
    "ACCELERATION": ("accel_s", 1.0),
    "GEARS": ("gears", 1.0),
    "DISPLACEMENT": ("displacement_l", 1.0),
    "CYLINDERS": ("cylinders", 1.0),
    "FUEL_TANK_CAPACITY": ("fuel_l", 1.0),
    "MAX_RPM": ("redline_rpm", 1.0),
}
# only the conversions we might actually meet; anything else is left alone and
# the value is reported in whatever unit the file used rather than mangled
_UNIT_FIX = {
    ("power_hp", "POWER_KILOWATT"): 1.34102,
    ("power_hp", "POWER_HORSEPOWER_METRIC"): 0.98632,
    ("mass_kg", "WEIGHT_POUNDS"): 0.453592,
    ("top_speed_kmh", "SPEED_MILES_PER_HOUR"): 1.60934,
}


def _spec_sheet(cs):
    """CarSpecsData -> our field names, as the GAME states them."""
    out = {}
    try:
        items = list(cs.items)
    except Exception:
        return out
    if not items:
        return out
    kd = cs.DESCRIPTOR.fields_by_name["items"].message_type
    kenum = kd.fields_by_name["key"].enum_type
    uenum = kd.fields_by_name["units"].enum_type
    for it in items:
        kv = kenum.values_by_number.get(it.key)
        if not kv:
            continue
        key = kv.name.replace("CarSpecs_KeyEnum_", "")
        if key not in _SPEC_KEYS:
            continue
        name, scale = _SPEC_KEYS[key]
        val = float(it.value) * scale
        uv = uenum.values_by_number.get(it.units)
        unit = uv.name.replace("CarSpecs_UnitsEnum_", "") if uv else ""
        val *= _UNIT_FIX.get((name, unit), 1.0)
        if name in ("displacement_l", "accel_s"):
            out[name] = round(val, 1)
        else:
            out[name] = round(val)
    if out:
        out["exact"] = True
    return out


def _curve_index(pkg):
    """basename -> (size, offset, path) for every .curve in a package."""
    sep = chr(92)
    out = {}
    for path, size, off in kspkg.iter_entries(pkg):
        low = path.lower().replace("/", sep)
        if low.endswith(".curve"):
            out.setdefault(low.split(sep)[-1], (size, off, path))
    return out


def _scan_pkg(pkg, base_pkg=None, base_curves=None):
    """Specs for every car in one package."""
    from . import protos as ap
    sep = chr(92)
    # model -> {kind: blob}; curves are kept by basename
    want = (".car", ".drivetrain", ".gearbox", ".aicardata", ".carengine")
    got, curves, presets = {}, {}, {}
    t0 = time.time()
    with open(pkg, "rb") as fh:
        for path, size, off in kspkg.iter_entries(pkg):
            low = path.lower().replace("/", sep)
            parts = low.split(sep)
            if len(parts) < 3 or parts[0] != "content" or parts[1] != "cars":
                continue
            model = parts[2]
            if low.endswith(".curve"):
                curves.setdefault(model, {})[parts[-1]] = (size, off, path)
                continue
            if low.endswith(".mechanicalcarpreset"):
                presets[parts[-1].rsplit(".", 1)[0]] = (size, off, path)
                continue
            for kind in want:
                if not low.endswith(kind):
                    continue
                # first of each kind wins; a car can ship alternative engines
                # under data/parts/ and the base one is the stock fitment
                got.setdefault(model, {}).setdefault(kind, (size, off, path))
                break

        # opened lazily: most packages resolve every curve locally
        base_fh = [None]
        out = {}
        for model, blobs in got.items():
            try:
                out[model] = _one(fh, ap, model, blobs, curves.get(model, {}),
                                  base_pkg, base_curves or {}, base_fh)
            except Exception as ex:
                logs.LOG.debug("carspecs %s: %s", model, ex)
        # ⚠ The game's OWN per-trim spec sheet, and it beats anything derived:
        # MechanicalCarPresetData.car_specs states power, weight, top speed and
        # 0-100 for THAT trim. Without it every trim of a car reported the base
        # car's numbers - the M2 CS Racing's 350 and 450 read identically.
        pre = {}
        for pid, ent in presets.items():
            try:
                m = ap.new("MechanicalCarPresetData")
                m.ParseFromString(kspkg.read_entry(fh, ent[0], ent[1], ent[2]))
                got_spec = _spec_sheet(m.car_specs)
                if got_spec:
                    pre[pid] = got_spec
            except Exception as ex:
                logs.LOG.debug("carspecs preset %s: %s", pid, ex)
        if base_fh[0] is not None:
            base_fh[0].close()
    return out, pre


def _one(fh, ap, model, blobs, curves, base_pkg=None, base_curves=None,
         base_fh=None):
    def read(kind):
        ent = blobs.get(kind)
        return kspkg.read_entry(fh, ent[0], ent[1], ent[2]) if ent else None

    def read_curve(name):
        """A curve from this package, else from the base game."""
        ent = curves.get(name)
        if ent:
            return kspkg.read_entry(fh, ent[0], ent[1], ent[2])
        ent = (base_curves or {}).get(name)
        if ent and base_pkg and base_fh is not None:
            if base_fh[0] is None:
                base_fh[0] = open(base_pkg, "rb")
            return kspkg.read_entry(base_fh[0], ent[0], ent[1], ent[2])
        return None

    spec = {}
    car = read(".car")
    engine_msg = None
    if car:
        m = ap.new("CarData")
        m.ParseFromString(car)
        g = m.general
        if g.totalMass:
            spec["mass_kg"] = round(g.totalMass)
        if g.fuel:
            spec["fuel_l"] = round(g.fuel)
        if g.screenName:
            spec["name"] = g.screenName
        engine_msg = m.engine

    dt = read(".drivetrain")
    if dt:
        d = ap.new("DrivetrainData")
        d.ParseFromString(dt)
        fld = d.DESCRIPTOR.fields_by_name["tractionType"]
        val = fld.enum_type.values_by_number.get(d.tractionType)
        if val:
            # FourWD / AWD / AWDF all read as all-wheel drive to a human
            spec["drive"] = {"RWD": "RWD", "FWD": "FWD"}.get(val.name, "AWD")

    gb = read(".gearbox")
    if gb:
        g = ap.new("GearBoxData")
        g.ParseFromString(gb)
        if g.gear_count:
            spec["gears"] = g.gear_count

    ai = read(".aicardata")
    if ai:
        a = ap.new("AiCarData")
        a.ParseFromString(ai)
        if a.gears.redline_rpm:
            spec["redline_rpm"] = round(a.gears.redline_rpm)
        if a.physics_hints.topspeed_kmh:
            # ⚠ an AI hint, not a measured or governed figure - see module doc
            spec["top_speed_kmh_est"] = round(a.physics_hints.topspeed_kmh)

    # power: the curve is pre-boost engine torque
    if engine_msg is None:
        eng = read(".carengine")
        if eng:
            engine_msg = ap.new("EngineData")
            engine_msg.ParseFromString(eng)
    if engine_msg is not None:
        name = (engine_msg.powerCurve or "").replace(chr(92), "/")
        name = name.rsplit("/", 1)[-1].lower()
        blob = read_curve(name)
        if blob:
            pts = _curve_points(blob)
            # ⚠ The WASTEGATE is the real ceiling, not max_boost. The Alpine
            # A110 S declares max_boost 1.3 with a wastegate at 1.1, and
            # taking max_boost alone over-read it by a fifth. Where a turbo
            # sets no wastegate, max_boost stands.
            boost = 0.0
            for t in engine_msg.turbos:
                b = float(t.max_boost or 0.0)
                wg = float(getattr(t, "wastegate", 0.0) or 0.0)
                if wg > 0:
                    b = min(b, wg)
                boost = max(boost, b)
            if pts:
                mult = 1.0 + (boost or 0.0)
                hp = max(y * mult * x / _NM_RPM_TO_HP for x, y in pts)
                nm = max(y * mult for _x, y in pts)
                if hp > 0:
                    spec["power_hp"] = round(hp)
                    spec["torque_nm"] = round(nm)
                    spec["boost_bar"] = round(boost, 2) if boost else 0
        if engine_msg.limiter and not spec.get("redline_rpm"):
            spec["redline_rpm"] = round(engine_msg.limiter)

    if spec.get("power_hp") and spec.get("mass_kg"):
        spec["hp_per_tonne"] = round(spec["power_hp"] * 1000.0 / spec["mass_kg"])
    return spec


def _key():
    pkg = viewer.package()
    try:
        st = os.stat(pkg)
        return f"{pkg}|{st.st_mtime_ns}|{st.st_size}|{CACHE_V}"
    except Exception:
        return f"{pkg}|{CACHE_V}"


def table(refresh=False):
    key = _key()
    if not refresh and _mem["key"] == key and _mem["data"]:
        return _mem["data"]
    if not refresh:
        try:
            cached = json.load(open(CACHE, encoding="utf-8"))
            if cached.get("key") == key:
                _mem["key"], _mem["data"] = key, cached
                return cached
        except Exception:
            pass
    data = _build()
    data["key"] = key
    try:
        os.makedirs(config.DATA, exist_ok=True)
        json.dump(data, open(CACHE, "w", encoding="utf-8"))
    except OSError as ex:
        logs.LOG.info("carspecs cache: %s", ex)
    _mem["key"], _mem["data"] = key, data
    return data


def for_model(model):
    """Specs for one car folder, or {} when we have nothing truthful to say."""
    if not model:
        return {}
    cars = table().get("cars") or {}
    return cars.get(model) or cars.get(str(model).lower()) or {}


def for_preset(preset_id):
    """Specs for a preset id, resolved to its model first.

    ⚠ Resolved through the car list, not carsmap alone. carsmap indexes the
    BASE archive only, so a mod's preset was never in it and every modded car
    resolved to nothing - the strip appeared for stock cars and no other.
    content.cars() already maps every preset to a model, mods included
    (it consults the mods' own manifests), so this reuses that rather than
    inventing a second rule.
    """
    if not preset_id:
        return {}
    model = ""
    try:
        from . import carsmap
        model = (carsmap.table().get("presets") or {}).get(preset_id, "")
    except Exception:
        pass
    if not model:
        try:
            from . import content
            for c in content.cars().get("cars", []):
                if c.get("id") == preset_id:
                    model = c.get("model") or ""
                    break
        except Exception:
            pass
    base = dict(for_model(model or preset_id))
    # ⚠ The TRIM's own sheet wins. It is what the game itself displays for
    # that preset, so the M2 CS Racing's 350 and 450 stop reading alike; the
    # model-level numbers stay underneath to fill what a sheet omits (drive
    # type, redline) and to cover mods, which almost never ship one.
    sheet = (table().get("presets") or {}).get(preset_id) or {}
    if sheet:
        base.pop("top_speed_kmh_est", None) if sheet.get("top_speed_kmh") else None
        base.update(sheet)
    if base.get("power_hp") and base.get("mass_kg"):
        base["hp_per_tonne"] = round(base["power_hp"] * 1000.0 / base["mass_kg"])
    return base
