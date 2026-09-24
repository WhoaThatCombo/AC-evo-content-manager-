"""Car damage for hosted servers.

There is no damage setting anywhere in the server config or the command-line
season blob (BuildSeasonDefinitionRequest has no assists field). The server
builds each session from a TEMPLATE season definition inside its own
content.kspkg, and SeasonDefinition field 50 - assists_preset, a
RacingSettingsAssistPreset - is the one place a server can impose assists on
every car that joins. Its damage_rate (field 14) is stored as a multiplier
offset: the game shows (damage_rate + 1) x 100 %, so 0 is normal damage and
-1 is none.

What we write is a NEUTRAL block: every on/off assist is FREE (the player's
own choice stands), stability control is left open across 0..1, and only
damage_rate is imposed. At 100 % no block is written at all - the factory
template goes back byte for byte.

⚠ Global to the install. Every server started from this dedicated-server
folder reads the same templates, so this is a per-install setting, not
per-profile.

⚠ A Kunos update replaces content.kspkg and silently resets damage to 100 %.
status() reads the archive each time rather than trusting a saved toggle.

⚠ Never overwrite an assists block we did not write. If a template already
carries one (a future Kunos default, or another tool), refuse and say so.
"""
import os
import struct

from . import config
from . import kspkg
from . import kspkg_write
from . import logs

TEMPLATES = (
    "content\\data\\practice.seasondefinition",
    "content\\data\\race_weekend.seasondefinition",
)
FIELD_ASSISTS = 50
FREE = 2
TYPE_CUSTOM = 4
# RacingSettingsAssistPreset enum fields - all set FREE so nothing but
# damage is forced on the player
_ENUM_FIELDS = (1, 2, 3, 4, 6, 7, 8, 11, 12, 13)
F_TYPE, F_STAB_MIN, F_STAB_MAX, F_DAMAGE = 10, 16, 17, 14

FACTORY_DIR = os.path.join(config.DATA, "damage_factory")


# --- tiny protobuf helpers ---------------------------------------------------
def _varint(n):
    out = bytearray()
    while True:
        b = n & 0x7F
        n >>= 7
        if n:
            out.append(b | 0x80)
        else:
            out.append(b)
            return bytes(out)


def _read_varint(d, p):
    r = s = 0
    while True:
        b = d[p]
        p += 1
        r |= (b & 0x7F) << s
        if not b & 0x80:
            return r, p
        s += 7


def _fields(d):
    """[(field, wiretype, raw_bytes_of_whole_field, value)] or raise."""
    out, p = [], 0
    while p < len(d):
        start = p
        tag, p = _read_varint(d, p)
        fn, wt = tag >> 3, tag & 7
        if fn == 0:
            raise ValueError("bad tag")
        if wt == 0:
            v, p = _read_varint(d, p)
        elif wt == 1:
            v, p = d[p:p + 8], p + 8
        elif wt == 5:
            v, p = d[p:p + 4], p + 4
        elif wt == 2:
            n, p = _read_varint(d, p)
            v, p = d[p:p + n], p + n
        else:
            raise ValueError("unsupported wire type %d" % wt)
        if p > len(d):
            raise ValueError("truncated")
        out.append((fn, wt, d[start:p], v))
    return out


def _block(pct):
    """Our neutral RacingSettingsAssistPreset imposing only damage."""
    rate = max(0.0, min(1.0, pct / 100.0)) - 1.0
    b = bytearray()
    b += _varint(F_TYPE << 3) + _varint(TYPE_CUSTOM)
    for fn in _ENUM_FIELDS:
        b += _varint(fn << 3) + _varint(FREE)
    b += _varint((F_STAB_MIN << 3) | 5) + struct.pack("<f", 0.0)
    b += _varint((F_STAB_MAX << 3) | 5) + struct.pack("<f", 1.0)
    b += _varint((F_DAMAGE << 3) | 5) + struct.pack("<f", rate)
    return bytes(b)


def _parse_block(raw):
    """Our block -> damage %, a foreign block -> None."""
    try:
        f = {fn: v for fn, wt, _r, v in _fields(raw)}
    except Exception:
        return None
    if f.get(F_TYPE) != TYPE_CUSTOM:
        return None
    if any(f.get(fn) != FREE for fn in _ENUM_FIELDS):
        return None
    if set(f) - set(_ENUM_FIELDS) - {F_TYPE, F_STAB_MIN, F_STAB_MAX, F_DAMAGE}:
        return None
    rate = struct.unpack("<f", f[F_DAMAGE])[0] if F_DAMAGE in f else 0.0
    return round((rate + 1.0) * 100)


def _split(template):
    """(template without field 50, the field-50 payload or None)."""
    kept, block = [], None
    for fn, _wt, raw, v in _fields(template):
        if fn == FIELD_ASSISTS:
            block = v
        else:
            kept.append(raw)
    return b"".join(kept), block


# --- archive access ----------------------------------------------------------
def _archive():
    from . import tracks
    k = tracks.server_kspkg()
    return k if k and os.path.isfile(k) else ""


def _read(archive, path):
    size, off = kspkg.find_entry(archive, path)
    if size is None:
        return None
    with open(archive, "rb") as f:
        f.seek(off)
        return kspkg.decrypt(f.read(size), 0)


def _factory_path(path):
    return os.path.join(FACTORY_DIR, os.path.basename(path))


def _factory(path, current):
    """The untouched template, kept outside the archive.

    Refreshed whenever the archive holds a template with NO assists block -
    that is the factory file, including a newer one after a game update.
    """
    os.makedirs(FACTORY_DIR, exist_ok=True)
    fp = _factory_path(path)
    _rest, block = _split(current)
    if block is None:
        if not os.path.isfile(fp) or open(fp, "rb").read() != current:
            with open(fp, "wb") as f:
                f.write(current)
        return current
    # Our own block: the factory template is simply the rest of the file.
    # Rebuilding it this way needs no backup, so a lost backup (or a Kunos
    # update that left our block in place) cannot strand the archive.
    if _parse_block(block) is not None:
        return _split(current)[0]
    return None


def status():
    archive = _archive()
    if not archive:
        return {"ok": False, "error": "server content.kspkg not found"}
    out = {"ok": True, "path": archive, "templates": {}}
    pcts = set()
    for path in TEMPLATES:
        try:
            cur = _read(archive, path)
            if cur is None:
                out["templates"][path] = {"state": "missing"}
                continue
            _rest, block = _split(cur)
            if block is None:
                st = {"state": "factory", "damage": 100}
            else:
                pct = _parse_block(block)
                st = ({"state": "ours", "damage": pct} if pct is not None
                      else {"state": "foreign"})
            out["templates"][path] = st
            if "damage" in st:
                pcts.add(st["damage"])
        except Exception as ex:                    # noqa: BLE001
            out["templates"][path] = {"state": "unreadable", "error": str(ex)}
    states = {t["state"] for t in out["templates"].values()}
    out["foreign"] = "foreign" in states
    out["damage"] = pcts.pop() if len(pcts) == 1 else None
    return out


def set_damage(pct):
    """pct 0..100. 100 restores the factory templates."""
    from . import servers
    pct = int(max(0, min(100, round(float(pct)))))
    archive = _archive()
    if not archive:
        return {"ok": False, "error": "server content.kspkg not found"}
    if servers._server_pids():
        return {"ok": False, "error": "stop every server first - the server "
                                      "holds content.kspkg open"}
    changes, notes = {}, []
    for path in TEMPLATES:
        cur = _read(archive, path)
        if cur is None:
            notes.append(f"{path} not in this archive - skipped")
            continue
        try:
            _rest, block = _split(cur)
        except ValueError as ex:
            # ⚠ garbage here means the archive is not the encryption we
            # assumed; writing now would corrupt it
            return {"ok": False, "error": f"{path} does not parse ({ex}); "
                                          "refusing to write"}
        if block is not None and _parse_block(block) is None:
            return {"ok": False, "foreign": True,
                    "error": f"{os.path.basename(path)} already carries an "
                             "assists block ACECM did not write - leaving it "
                             "alone"}
        base = _factory(path, cur)
        if base is None:
            return {"ok": False, "error": f"cannot rebuild {path}; verify "
                                          "the server files in Steam first"}
        want = base if pct == 100 else (
            base + _varint((FIELD_ASSISTS << 3) | 2)
            + _varint(len(_block(pct))) + _block(pct))
        if want != cur:
            changes[path] = want
    if not changes:
        return {"ok": True, "unchanged": True, "notes": notes, **status()}
    try:
        res = kspkg_write.write_inplace(archive, changes)
        ver = kspkg_write.verify(archive)
        if not ver.get("ok"):
            raise ValueError(f"archive failed verification: {ver}")
        for path, blob in changes.items():
            if _read(archive, path) != blob:
                raise ValueError(f"readback of {path} does not match")
    except Exception as ex:                        # noqa: BLE001
        logs.LOG.error("damage write failed: %s", ex)
        return {"ok": False, "error": str(ex)}
    logs.LOG.info("server damage set to %d%% (%s)", pct,
                  ", ".join(os.path.basename(p) for p in changes))
    return {"ok": True, "written": list(changes), "notes": notes,
            "records": res.get("records"), **status()}


def apply_for(profile):
    """Called by servers.start: make the archive match THIS profile.

    ⚠ The templates are shared by every server in the install, so with
    another server already running we cannot rewrite them (it holds the
    archive open) - and it is already using whatever is there.
    """
    want = int(profile.get("damage_pct", 100) if profile.get("damage_pct")
               is not None else 100)
    cur = status()
    if cur.get("ok") and cur.get("damage") == want:
        return {"ok": True, "unchanged": True, "damage": want}
    return set_damage(want)
