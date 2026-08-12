"""
AC EVO - Penalty Toggle  (singleplayer client and/or dedicated server)

Turns the penalty-investigation system (wrong-way DQ, track limits, speeding,
etc.) OFF or back ON by patching two files inside a content.kspkg:

  1. system\\defaults\\timeattack.penaltyinvestigations  -> emptied
  2. content\\data\\practice.seasondefinition             -> the embedded
     penalty trigger list (a second, session-specific copy of the same kind
     of data) emptied

Both targets have to be cleared: clearing only #1 is not enough, because the
per-session copy inside practice.seasondefinition overrides it.

WHICH INSTALL DOES WHAT
  * CLIENT install  -> affects your own singleplayer Practice / Time Attack.
    It does NOT affect multiplayer: penalties in multiplayer are enforced by
    whichever server you join, from that server's own files.
  * SERVER install   -> affects everyone racing on servers you host with that
    dedicated-server copy. Nobody needs to change anything on their end.

Disabling writes a backup of the original bytes next to the content.kspkg
(timeattack_penaltyinvestigations.bak / practice_seasondefinition.bak), and
re-enabling restores from those backups - so keep them if you ever want
penalties back.

Usage: double-click penalties_tool.bat  (close the game / stop the server
first). Everything is menu-driven; nothing is written until you pick.
"""

import os
import re
import struct
import subprocess
import sys

KSPKG_KEY = bytes.fromhex("c135117da921979f")
TABLE_SIZE = 64 * 1024 * 1024
BUCKET_SIZE = 256

TARGET1 = "system\\defaults\\timeattack.penaltyinvestigations"
TARGET2 = "content\\data\\practice.seasondefinition"
BAK1 = "timeattack_penaltyinvestigations.bak"
BAK2 = "practice_seasondefinition.bak"

# field path to the penalty trigger list embedded in practice.seasondefinition
NESTED_PATH = [10, 2, 4, 2, 25, 2, 22]


def xor_decrypt(buf, abs_offset):
    phase = abs_offset % 8
    key = KSPKG_KEY[phase:] + KSPKG_KEY[:phase]
    out = bytearray(len(buf))
    for i, b in enumerate(buf):
        out[i] = b ^ key[i % 8]
    return bytes(out)


# ---------------------------------------------------------------------------
# minimal protobuf varint / walk / re-encode (stdlib only)
# ---------------------------------------------------------------------------
def read_varint(data, pos):
    result = shift = 0
    while True:
        b = data[pos]
        pos += 1
        result |= (b & 0x7F) << shift
        if not (b & 0x80):
            return result, pos
        shift += 7


def write_varint(n):
    out = bytearray()
    while True:
        b = n & 0x7F
        n >>= 7
        if n:
            out.append(b | 0x80)
        else:
            out.append(b)
            return bytes(out)


def pb_walk(data):
    out, p, n = [], 0, len(data)
    try:
        while p < n:
            tag, p = read_varint(data, p)
            fn, wt = tag >> 3, tag & 7
            if fn == 0:
                return None
            if wt == 2:
                length, p = read_varint(data, p)
                if p + length > n:
                    return None
                out.append((fn, 2, data[p:p + length]))
                p += length
            elif wt == 0:
                v, p = read_varint(data, p)
                out.append((fn, 0, v))
            elif wt == 5:
                if p + 4 > n:
                    return None
                out.append((fn, 5, data[p:p + 4]))
                p += 4
            elif wt == 1:
                if p + 8 > n:
                    return None
                out.append((fn, 1, data[p:p + 8]))
                p += 8
            else:
                return None
    except IndexError:
        return None
    return out


def pb_field(fn, wt, val):
    if wt == 2:
        return write_varint((fn << 3) | 2) + write_varint(len(val)) + val
    if wt == 0:
        return write_varint((fn << 3) | 0) + write_varint(val)
    return write_varint((fn << 3) | wt) + val


def pb_reencode(parsed):
    return b"".join(pb_field(fn, wt, v) for fn, wt, v in parsed)


def clear_nested_field(data, path):
    top = pb_walk(data)
    if top is None:
        raise ValueError("not a valid message")
    fn_target = path[0]
    new_top, replaced = [], False
    for fn, wt, v in top:
        if fn == fn_target and wt == 2 and not replaced:
            new_top.append((fn, wt, b"" if len(path) == 1
                            else clear_nested_field(v, path[1:])))
            replaced = True
        else:
            new_top.append((fn, wt, v))
    if not replaced:
        raise ValueError(f"field {fn_target} not found")
    return pb_reencode(new_top)


def nested_is_empty(data, path):
    """True if the embedded trigger list at `path` is already cleared."""
    node = pb_walk(data)
    if node is None:
        raise ValueError("unreadable")
    for i, fn in enumerate(path):
        match = next((v for f, w, v in node if f == fn and w == 2), None)
        if match is None:
            raise ValueError("path not found (game version may differ)")
        if i == len(path) - 1:
            return len(match) == 0
        node = pb_walk(match)
        if node is None:
            raise ValueError("nested parse failed")
    return False


# ---------------------------------------------------------------------------
# kspkg read / write
# ---------------------------------------------------------------------------
def read_table(kspkg_path):
    with open(kspkg_path, "rb") as f:
        f.seek(0, os.SEEK_END)
        table_start = f.tell() - TABLE_SIZE
        f.seek(table_start)
        return f.read(), table_start


def find_bucket(table, needle):
    for i in range(TABLE_SIZE // BUCKET_SIZE):
        raw = table[i * BUCKET_SIZE:(i + 1) * BUCKET_SIZE]
        dec = xor_decrypt(raw, i * BUCKET_SIZE)
        if dec.startswith(needle):
            return i, dec
    return None, None


def read_entry(kspkg_path, table, rel_path):
    idx, dec = find_bucket(table, rel_path.encode("utf-8") + b"\x00")
    if idx is None:
        return None, None
    size = struct.unpack("<I", dec[240:244])[0]
    offset = struct.unpack("<Q", dec[248:256])[0]
    with open(kspkg_path, "rb") as f:
        f.seek(offset)
        raw = f.read(size)
    return xor_decrypt(raw, 0), size


def write_entry(kspkg_path, rel_path, new_content):
    table, table_start = read_table(kspkg_path)
    idx, dec = find_bucket(table, rel_path.encode("utf-8") + b"\x00")
    if idx is None:
        raise ValueError(f"{rel_path} not found in archive")
    bucket = bytearray(dec)
    struct.pack_into("<I", bucket, 240, len(new_content))
    struct.pack_into("<Q", bucket, 248, table_start)
    new_table = bytearray(table)
    new_table[idx * BUCKET_SIZE:(idx + 1) * BUCKET_SIZE] = \
        xor_decrypt(bytes(bucket), idx * BUCKET_SIZE)
    with open(kspkg_path, "r+b") as f:
        f.seek(table_start)
        f.write(xor_decrypt(new_content, 0))
        f.write(bytes(new_table))
        f.truncate()


# ---------------------------------------------------------------------------
# state / patch / restore
# ---------------------------------------------------------------------------
def penalty_state(kspkg_path):
    """Return 'off', 'on', or a short problem description."""
    try:
        table, _ = read_table(kspkg_path)
        d1, size1 = read_entry(kspkg_path, table, TARGET1)
        d2, _ = read_entry(kspkg_path, table, TARGET2)
        if d1 is None or d2 is None:
            return "target files not found (version mismatch?)"
        return "off" if (size1 == 0 and nested_is_empty(d2, NESTED_PATH)) else "on"
    except Exception as e:
        return f"unreadable ({e})"


def patch_kspkg(kspkg_path):
    """Disable penalties. Returns status lines. Idempotent."""
    status = []
    table, _ = read_table(kspkg_path)

    d1, size1 = read_entry(kspkg_path, table, TARGET1)
    if d1 is None:
        status.append(f"!! {TARGET1} not found - skipped.")
    elif size1 == 0:
        status.append(f"OK  {TARGET1} already empty.")
    else:
        bak = os.path.join(os.path.dirname(kspkg_path), BAK1)
        with open(bak, "wb") as f:
            f.write(d1)
        write_entry(kspkg_path, TARGET1, b"")
        status.append(f"OK  cleared {TARGET1} ({size1} -> 0 bytes). Backup: {bak}")

    table, _ = read_table(kspkg_path)
    d2, _ = read_entry(kspkg_path, table, TARGET2)
    if d2 is None:
        status.append(f"!! {TARGET2} not found - skipped.")
    else:
        try:
            if nested_is_empty(d2, NESTED_PATH):
                status.append(f"OK  {TARGET2} triggers already cleared.")
            else:
                bak = os.path.join(os.path.dirname(kspkg_path), BAK2)
                with open(bak, "wb") as f:
                    f.write(d2)
                write_entry(kspkg_path, TARGET2, clear_nested_field(d2, NESTED_PATH))
                status.append(f"OK  cleared embedded triggers in {TARGET2}. Backup: {bak}")
        except ValueError as e:
            status.append(f"!! skipped {TARGET2}: {e}")
    return status


def restore_kspkg(kspkg_path):
    """Re-enable penalties by restoring the original bytes from the backups."""
    folder = os.path.dirname(kspkg_path)
    status = []
    for target, bakname in ((TARGET1, BAK1), (TARGET2, BAK2)):
        bak = os.path.join(folder, bakname)
        if not os.path.isfile(bak):
            status.append(f"!! no backup for {target} ({bakname}) - cannot restore "
                          f"this one. Verify game files in Steam to get it back.")
            continue
        with open(bak, "rb") as f:
            original = f.read()
        write_entry(kspkg_path, target, original)
        status.append(f"OK  restored {target} ({len(original)} bytes) from {bakname}")
    return status


# ---------------------------------------------------------------------------
# DAMAGE
#
# There is no global damage switch in this build - not in the assist presets,
# not in any .seasondefinition. Gameplay damage is per-car physics data, in
# two places (field numbers taken from the protobuf descriptors embedded in
# AssettoCorsaEVO.exe, not guessed):
#
#   .car        f2 SuspensionsData -> f4 SuspensionDamageData
#                  f1 minVelocity   f2 gain   f3 maxDamage   f4 debugLog
#   .carengine  f27 EngineDamageData
#                  f1 turboBoostDamageThreshold  f2 turboBoostDamageK
#                  f3 rpmDamageThreshold         f4 rpmDamageK
#                  f5 bovThreshold
#               plus legacy EngineData f16/f17 (turbo boost threshold / K)
#
# Body and glass damage (DamagePartData) is visual only - bone deformation -
# so it is deliberately left alone; it has no effect on how the car drives.
#
# Disabling zeroes the *rate* fields (gain / K), which stops damage
# accumulating, and pushes minVelocity out of reach. Every edit replaces one
# 4-byte float with another, so each file stays exactly the same size and can
# be written back in place - no archive restructuring.
# ---------------------------------------------------------------------------
DMG_BACKUP = "damage_backup.bin"
DMG_MAGIC = b"ACEDMG01"
BIG = 1.0e9


def _f32(x):
    return struct.pack("<f", x)


def _set_floats(msg, changes):
    """Replace float fields in `msg` per {field_no: value}. Only rewrites
    fields that already exist as 32-bit floats, so the length never changes."""
    parsed = pb_walk(msg)
    if parsed is None:
        return None, 0
    out, n = [], 0
    for fn, wt, v in parsed:
        if wt == 5 and fn in changes:
            nv = _f32(changes[fn])
            if nv != v:
                n += 1
            out.append((fn, wt, nv))
        else:
            out.append((fn, wt, v))
    if n == 0:
        return None, 0
    return pb_reencode(out), n


def _replace_sub(buf, path, changes):
    """Descend `path` (field numbers), apply float `changes` at the leaf,
    and re-encode back up. Returns (new_bytes, n_changed)."""
    parsed = pb_walk(buf)
    if parsed is None:
        return None, 0
    fn_t = path[0]
    out, total = [], 0
    done = False
    for fn, wt, v in parsed:
        if fn == fn_t and wt == 2 and not done:
            done = True
            if len(path) == 1:
                nv, n = _set_floats(v, changes)
            else:
                nv, n = _replace_sub(v, path[1:], changes)
            total += n
            out.append((fn, wt, nv if nv is not None else v))
        else:
            out.append((fn, wt, v))
    if total == 0:
        return None, 0
    return pb_reencode(out), total


def build_index(kspkg_path):
    """Full {lowercase path: (path, size, offset)} index.

    Every bucket starts at a multiple of 256 and 256 % 8 == 0, so the whole
    64 MiB table shares one XOR phase and can be decrypted in one shot -
    which makes this ~0.3s instead of a per-file linear scan."""
    with open(kspkg_path, "rb") as f:
        f.seek(0, os.SEEK_END)
        table_start = f.tell() - TABLE_SIZE
        f.seek(table_start)
        raw = f.read(TABLE_SIZE)
    key = KSPKG_KEY * (TABLE_SIZE // len(KSPKG_KEY))
    dec = (int.from_bytes(raw, "big") ^ int.from_bytes(key, "big")).to_bytes(
        TABLE_SIZE, "big")
    idx = {}
    for i in range(0, TABLE_SIZE, BUCKET_SIZE):
        b = dec[i:i + BUCKET_SIZE]
        if b[0] == 0:
            continue
        z = b.find(0)
        if z <= 0:
            continue
        try:
            path = b[:z].decode("utf-8")
        except UnicodeDecodeError:
            continue
        size = struct.unpack_from("<I", b, 240)[0]
        offset = struct.unpack_from("<Q", b, 248)[0]
        idx[path.lower()] = (path, size, offset)
    return idx


def read_at(kspkg_path, size, offset):
    with open(kspkg_path, "rb") as f:
        f.seek(offset)
        return xor_decrypt(f.read(size), 0)


def write_at(kspkg_path, offset, content):
    """In-place write. Only valid when the new content is the same length as
    the old, which is guaranteed here (float-for-float replacement)."""
    with open(kspkg_path, "r+b") as f:
        f.seek(offset)
        f.write(xor_decrypt(content, 0))


SUSP_PATH = [2, 4]                       # .car  -> SuspensionsData -> damage
SUSP_OFF = {1: BIG, 2: 0.0, 3: 0.0}      # minVelocity, gain, maxDamage
ENG_SUB_OFF = {2: 0.0, 4: 0.0}           # f27: turboBoostDamageK, rpmDamageK
ENG_TOP_OFF = {17: 0.0}                  # legacy EngineData turboBoostDamageK

# Cosmetic damage is driven from ONE file shared by every car:
#   content\cars\common_assets\car_shading_settings.carshadingglobalsetting
# CarShadingGlobalSetting f13/f14 = damage_min/full_damage_speed (body panels,
# scratches, dents) and f150/f151 = glass_min/full_damage_speed (cracks).
# Pushing the "min speed" out of reach means the effect never kicks in.
SHADING_FILE = ("content\\cars\\common_assets\\"
                "car_shading_settings.carshadingglobalsetting")
VIS_OFF = {13: BIG, 14: BIG, 150: BIG, 151: BIG}


def _damage_edits(kspkg_path, idx, physical=True, visual=False):
    """Yield (path, size, offset, original_bytes, new_bytes) for each file
    that has damage data to neutralise."""
    if physical:
        for low, (path, size, offset) in sorted(idx.items()):
            if size == 0:
                continue
            if low.endswith(".car"):
                data = read_at(kspkg_path, size, offset)
                new, n = _replace_sub(data, SUSP_PATH, SUSP_OFF)
                if new and n:
                    yield path, size, offset, data, new
            elif low.endswith(".carengine"):
                data = read_at(kspkg_path, size, offset)
                cur, total = data, 0
                sub, n = _replace_sub(cur, [27], ENG_SUB_OFF)
                if sub:
                    cur, total = sub, total + n
                top, n2 = _set_floats(cur, ENG_TOP_OFF)
                if top:
                    cur, total = top, total + n2
                if total:
                    yield path, size, offset, data, cur

    if visual:
        rec = idx.get(SHADING_FILE.lower())
        if rec:
            path, size, offset = rec
            data = read_at(kspkg_path, size, offset)
            new, n = _set_floats(data, VIS_OFF)
            if new and n:
                yield path, size, offset, data, new


def damage_state(kspkg_path, idx=None):
    """'off', 'on', or a problem string - decided from the .car files."""
    try:
        idx = idx or build_index(kspkg_path)
        checked = zeroed = 0
        for low, (path, size, offset) in idx.items():
            if not low.endswith(".car") or size == 0:
                continue
            data = read_at(kspkg_path, size, offset)
            susp = pb_walk(data)
            if susp is None:
                continue
            s = next((v for f, w, v in susp if f == 2 and w == 2), None)
            if s is None:
                continue
            d = next((v for f, w, v in (pb_walk(s) or []) if f == 4 and w == 2), None)
            if d is None:
                continue
            vals = {f: struct.unpack("<f", v)[0]
                    for f, w, v in (pb_walk(d) or []) if w == 5}
            checked += 1
            if vals.get(2, 1) == 0.0 and vals.get(3, 1) == 0.0:
                zeroed += 1
        if checked == 0:
            return "no car damage data found (version mismatch?)"
        return "off" if zeroed == checked else "on"
    except Exception as e:
        return f"unreadable ({e})"


def visual_damage_state(kspkg_path, idx=None):
    """'off', 'on', or a problem string - from the shared shading settings."""
    try:
        idx = idx or build_index(kspkg_path)
        rec = idx.get(SHADING_FILE.lower())
        if not rec:
            return "shading settings not found (version mismatch?)"
        path, size, offset = rec
        data = read_at(kspkg_path, size, offset)
        vals = {f: struct.unpack("<f", v)[0]
                for f, w, v in (pb_walk(data) or []) if w == 5}
        return "off" if vals.get(13, 0) >= BIG / 2 else "on"
    except Exception as e:
        return f"unreadable ({e})"


def _load_backup(bak):
    """{(path, offset): original_bytes} from the backup blob, or {}."""
    if not os.path.isfile(bak):
        return {}
    out = {}
    with open(bak, "rb") as f:
        if f.read(len(DMG_MAGIC)) != DMG_MAGIC:
            return {}
        count = struct.unpack("<I", f.read(4))[0]
        for _ in range(count):
            plen = struct.unpack("<H", f.read(2))[0]
            path = f.read(plen).decode("utf-8", "replace")
            offset = struct.unpack("<Q", f.read(8))[0]
            size = struct.unpack("<I", f.read(4))[0]
            out[(path, offset)] = f.read(size)
    return out


def _save_backup(bak, entries):
    with open(bak, "wb") as f:
        f.write(DMG_MAGIC)
        f.write(struct.pack("<I", len(entries)))
        for (path, offset), original in entries.items():
            pb = path.encode("utf-8")
            f.write(struct.pack("<H", len(pb)))
            f.write(pb)
            f.write(struct.pack("<Q", offset))
            f.write(struct.pack("<I", len(original)))
            f.write(original)


def disable_damage(kspkg_path, physical=True, visual=False):
    """Neutralise damage. `physical` = suspension + engine (affects how the
    car drives); `visual` = body/glass damage appearance, which comes from a
    single shared settings file.

    Originals are merged into one backup blob, so enabling later restores
    exactly - including entries added by a later run."""
    idx = build_index(kspkg_path)
    edits = list(_damage_edits(kspkg_path, idx, physical, visual))
    if not edits:
        return ["OK  nothing left to change - already disabled."]

    bak = os.path.join(os.path.dirname(kspkg_path), DMG_BACKUP)
    backup = _load_backup(bak)
    added = 0
    for path, size, offset, original, _new in edits:
        key = (path, offset)
        if key not in backup:            # never overwrite a known-good original
            backup[key] = original
            added += 1

    cars = engines = shading = 0
    for path, size, offset, original, new in edits:
        if len(new) != len(original):    # must never happen; guard anyway
            continue
        write_at(kspkg_path, offset, new)
        low = path.lower()
        if low.endswith(".car"):
            cars += 1
        elif low.endswith(".carengine"):
            engines += 1
        else:
            shading += 1

    _save_backup(bak, backup)
    out = []
    if cars or engines:
        out.append(f"OK  suspension damage disabled on {cars} car files")
        out.append(f"OK  engine damage disabled on {engines} engine files")
    if shading:
        out.append("OK  cosmetic (body + glass) damage disabled - 1 shared file")
    out.append(f"OK  backup now holds {len(backup)} originals "
               f"({added} added this run): {bak}")
    return out


def enable_damage(kspkg_path):
    """Put every original damage value back from the backup blob."""
    bak = os.path.join(os.path.dirname(kspkg_path), DMG_BACKUP)
    backup = _load_backup(bak)
    if not backup:
        return [f"!! no usable {DMG_BACKUP} next to content.kspkg - cannot "
                f"restore. Verify the game files in Steam to get the originals back."]
    for (path, offset), original in backup.items():
        write_at(kspkg_path, offset, original)
    return [f"OK  restored original damage values in {len(backup)} files",
            "    (backup kept - delete damage_backup.bin if you want it gone)"]


# ---------------------------------------------------------------------------
# install discovery
# ---------------------------------------------------------------------------
CLIENT_DIRS = ["Assetto Corsa EVO"]
SERVER_DIRS = ["Assetto Corsa EVO Dedicated Server"]


def _steam_libraries():
    """Steam library paths, read from libraryfolders.vdf where available.
    Never scans whole drives - a recursive search makes the tool look hung."""
    roots = []
    for base in (os.environ.get("ProgramFiles(x86)"), os.environ.get("ProgramFiles"),
                 os.environ.get("ProgramW6432")):
        if base:
            roots.append(os.path.join(base, "Steam"))
    for d in "CDEFGH":
        roots += [f"{d}:\\Steam", f"{d}:\\SteamLibrary", f"{d}:\\Games\\Steam",
                  f"{d}:\\Games\\SteamLibrary",
                  f"{d}:\\Program Files (x86)\\Steam", f"{d}:\\Program Files\\Steam"]

    libs, seen = [], set()

    def add(p):
        if p and os.path.normcase(p) not in seen:
            seen.add(os.path.normcase(p))
            libs.append(p)

    for root in roots:
        add(root)
        vdf = os.path.join(root, "steamapps", "libraryfolders.vdf")
        if os.path.isfile(vdf):
            try:
                with open(vdf, "r", encoding="utf-8", errors="replace") as f:
                    txt = f.read()
                for m in re.finditer(r'"path"\s*"([^"]+)"', txt):
                    add(m.group(1).replace("\\\\", "\\"))
            except OSError:
                pass
    return libs


def _find(subdirs):
    for lib in _steam_libraries():
        for sub in subdirs:
            for cand in (os.path.join(lib, "steamapps", "common", sub),
                         os.path.join(lib, "common", sub),
                         os.path.join(lib, sub)):
                p = os.path.join(cand, "content.kspkg")
                if os.path.isfile(p):
                    return p
    return None


def find_client_kspkg():
    return _find(CLIENT_DIRS)


def find_server_kspkg():
    return _find(SERVER_DIRS)


def process_running(names):
    try:
        out = subprocess.run(["tasklist"], capture_output=True, text=True, timeout=10)
        low = out.stdout.lower()
        return [n for n in names if n.lower() in low]
    except Exception:
        return []


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------
def _ask(prompt, valid):
    while True:
        c = input(prompt).strip().lower()
        if c in valid:
            return c
        print(f"   please enter one of: {', '.join(valid)}")


def _state_line(kspkg):
    idx = build_index(kspkg)

    def s(v):
        return {"off": "OFF", "on": "on"}.get(v, v)

    return (f"penalties: {s(penalty_state(kspkg))}   |   "
            f"damage: {s(damage_state(kspkg, idx))}   |   "
            f"cosmetic damage: {s(visual_damage_state(kspkg, idx))}")


def main():
    print("=" * 66)
    print(" AC EVO - Penalty & Damage Toggle")
    print("=" * 66)

    targets = []
    client = find_client_kspkg()
    server = find_server_kspkg()
    if client:
        targets.append(("Singleplayer (game client)", client, "AssettoCorsaEVO.exe"))
    if server:
        targets.append(("Multiplayer (dedicated server you host)", server,
                        "AssettoCorsaEVOServer.exe"))

    if not targets:
        print("\nCouldn't find an AC EVO install automatically.")
        p = input("Paste the full path to a content.kspkg (or Enter to quit): ").strip().strip('"')
        if not p or not os.path.isfile(p):
            print("Nothing to do.")
            input("\nPress Enter to exit...")
            return
        targets.append(("Manual path", p, ""))

    print("\nInstalls found (reading current state, this takes a moment)...\n")
    for i, (label, path, _) in enumerate(targets, 1):
        print(f"  [{i}] {label}")
        print(f"      {path}")
        print(f"      {_state_line(path)}\n")

    choices = [str(i) for i in range(1, len(targets) + 1)]
    if len(targets) > 1:
        choices.append("a")
        pick = _ask(f"Which install? ({'/'.join(choices)}, a = all, q = quit): ",
                    choices + ["q"])
    else:
        pick = _ask("Use this install? (1 = yes, q = quit): ", ["1", "q"])
    if pick == "q":
        return
    chosen = targets if pick == "a" else [targets[int(pick) - 1]]

    print("\nWhat do you want to change?")
    print("   p = penalties            (wrong-way DQ, track limits, speeding)")
    print("   d = damage               (suspension + engine - how it drives)")
    print("   c = cosmetic damage      (dents, scratches, cracked glass)")
    print("   b = all of the above")
    what = _ask("   choice (p/d/c/b, q = quit): ", ["p", "d", "c", "b", "q"])
    if what == "q":
        return
    action = _ask("Turn it off or back on?  (o = off, n = on, q = quit): ",
                  ["o", "n", "q"])
    if action == "q":
        return
    off = (action == "o")

    # never write to a kspkg the game or server currently has open
    busy = process_running([n for _, _, n in chosen if n])
    if busy:
        print(f"\n!! Still running: {', '.join(busy)}")
        print("!! Close the game / stop the server first, then run this again.")
        input("\nPress Enter to exit...")
        return

    print()
    for label, path, _ in chosen:
        print(f"--- {label}")
        try:
            if what in ("p", "b"):
                for l in (patch_kspkg(path) if off else restore_kspkg(path)):
                    print("   " + l)
            if what in ("d", "c", "b"):
                print("   (scanning car files...)")
                if off:
                    lines = disable_damage(path,
                                           physical=what in ("d", "b"),
                                           visual=what in ("c", "b"))
                else:
                    # one backup blob covers both, so this restores everything
                    lines = enable_damage(path)
                for l in lines:
                    print("   " + l)
        except Exception as e:
            print(f"   !! failed: {e}")
        print(f"   now -> {_state_line(path)}")
        print()

    if off:
        print("Done. Reminder: changing the CLIENT only affects your own")
        print("singleplayer - on other people's servers, their rules apply.")
    else:
        print("Done - originals restored from the backup files.")
    input("\nPress Enter to exit...")


if __name__ == "__main__":
    main()
