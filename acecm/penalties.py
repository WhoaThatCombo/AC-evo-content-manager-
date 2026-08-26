"""Penalty toggle for the servers menu.

The core logic (which two files inside content.kspkg to clear, and the
protobuf field path to the embedded trigger list) is exactly
tools/penalties_tool.py - that script was already proven working and is kept
as the standalone/manual entry point. This module exists only to plug the
same logic into the Servers page as a live toggle: path resolution goes
through config.server_dir() / tracks.client_kspkg(), the same functions
everything else in ACECM uses, instead of penalties_tool's own independent
Steam-library scan - so this always acts on the install ACECM is actually
configured to use, never a different one it happened to find first.

⚠ A Kunos content update replaces content.kspkg wholesale, which silently
re-enables penalties on every server (and the client) - it looked broken but
nothing was; the archive was just reset along with everything else that
lives in it. There is no way to detect "this got wiped by an update" short
of checking state, so the Servers page just shows current state honestly
each time it loads rather than assuming yesterday's toggle still holds.
"""
import os
import struct


KSPKG_KEY = bytes.fromhex("c135117da921979f")
TABLE_SIZE = 64 * 1024 * 1024
BUCKET_SIZE = 256

TARGET1 = "system\\defaults\\timeattack.penaltyinvestigations"
TARGET2 = "content\\data\\practice.seasondefinition"
BAK1 = "timeattack_penaltyinvestigations.bak"
BAK2 = "practice_seasondefinition.bak"
NESTED_PATH = [10, 2, 4, 2, 25, 2, 22]


def xor_decrypt(buf, abs_offset):
    phase = abs_offset % 8
    key = KSPKG_KEY[phase:] + KSPKG_KEY[:phase]
    out = bytearray(len(buf))
    for i, b in enumerate(buf):
        out[i] = b ^ key[i % 8]
    return bytes(out)


def _read_varint(data, pos):
    result = shift = 0
    while True:
        b = data[pos]
        pos += 1
        result |= (b & 0x7F) << shift
        if not (b & 0x80):
            return result, pos
        shift += 7


def _write_varint(n):
    out = bytearray()
    while True:
        b = n & 0x7F
        n >>= 7
        if n:
            out.append(b | 0x80)
        else:
            out.append(b)
            return bytes(out)


def _pb_walk(data):
    out, p, n = [], 0, len(data)
    try:
        while p < n:
            tag, p = _read_varint(data, p)
            fn, wt = tag >> 3, tag & 7
            if fn == 0:
                return None
            if wt == 2:
                length, p = _read_varint(data, p)
                if p + length > n:
                    return None
                out.append((fn, 2, data[p:p + length]))
                p += length
            elif wt == 0:
                v, p = _read_varint(data, p)
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


def _pb_field(fn, wt, val):
    if wt == 2:
        return _write_varint((fn << 3) | 2) + _write_varint(len(val)) + val
    if wt == 0:
        return _write_varint((fn << 3) | 0) + _write_varint(val)
    return _write_varint((fn << 3) | wt) + val


def _pb_reencode(parsed):
    return b"".join(_pb_field(fn, wt, v) for fn, wt, v in parsed)


def _clear_nested_field(data, path):
    top = _pb_walk(data)
    if top is None:
        raise ValueError("not a valid message")
    fn_target = path[0]
    new_top, replaced = [], False
    for fn, wt, v in top:
        if fn == fn_target and wt == 2 and not replaced:
            new_top.append((fn, wt, b"" if len(path) == 1
                            else _clear_nested_field(v, path[1:])))
            replaced = True
        else:
            new_top.append((fn, wt, v))
    if not replaced:
        raise ValueError(f"field {fn_target} not found")
    return _pb_reencode(new_top)


def _nested_is_empty(data, path):
    node = _pb_walk(data)
    if node is None:
        raise ValueError("unreadable")
    for i, fn in enumerate(path):
        match = next((v for f, w, v in node if f == fn and w == 2), None)
        if match is None:
            raise ValueError("path not found (game version may differ)")
        if i == len(path) - 1:
            return len(match) == 0
        node = _pb_walk(match)
        if node is None:
            raise ValueError("nested parse failed")
    return False


def _read_table(kspkg_path):
    with open(kspkg_path, "rb") as f:
        f.seek(0, os.SEEK_END)
        table_start = f.tell() - TABLE_SIZE
        f.seek(table_start)
        return f.read(), table_start


def _find_bucket(table, needle):
    for i in range(TABLE_SIZE // BUCKET_SIZE):
        raw = table[i * BUCKET_SIZE:(i + 1) * BUCKET_SIZE]
        dec = xor_decrypt(raw, i * BUCKET_SIZE)
        if dec.startswith(needle):
            return i, dec
    return None, None


def _read_entry(kspkg_path, rel_path):
    """Read one entry. Binary search, not a scan - see kspkg.find_entry.

    This used to walk the table decrypting every bucket byte-at-a-time in
    Python: ~112k decrypt calls for the four lookups behind /api/penalties,
    1.8s per request, which is what made opening the Servers page feel stuck.
    The table is sorted, so the answer was always ~18 seeks away.
    """
    from . import kspkg
    size, offset = kspkg.find_entry(kspkg_path, rel_path)
    if size is None:
        return None, None
    with open(kspkg_path, "rb") as f:
        f.seek(offset)
        raw = f.read(size)
    return xor_decrypt(raw, 0), size


def _write_entry(kspkg_path, rel_path, new_content):
    table, table_start = _read_table(kspkg_path)
    idx, dec = _find_bucket(table, rel_path.encode("utf-8") + b"\x00")
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


def _state(kspkg_path):
    """'off', 'on', or a short problem description."""
    try:
        d1, size1 = _read_entry(kspkg_path, TARGET1)
        d2, _ = _read_entry(kspkg_path, TARGET2)
        if d1 is None or d2 is None:
            return "target files not found (version mismatch?)"
        return "off" if (size1 == 0 and _nested_is_empty(d2, NESTED_PATH)) else "on"
    except Exception as ex:
        return f"unreadable ({ex})"


def _patch(kspkg_path):
    status = []
    d1, size1 = _read_entry(kspkg_path, TARGET1)
    if d1 is None:
        status.append(f"{TARGET1} not found - skipped")
    elif size1 == 0:
        status.append(f"{TARGET1} already empty")
    else:
        bak = os.path.join(os.path.dirname(kspkg_path), BAK1)
        with open(bak, "wb") as f:
            f.write(d1)
        _write_entry(kspkg_path, TARGET1, b"")
        status.append(f"cleared {TARGET1} ({size1} -> 0 bytes)")

    d2, _ = _read_entry(kspkg_path, TARGET2)
    if d2 is None:
        status.append(f"{TARGET2} not found - skipped")
    else:
        try:
            if _nested_is_empty(d2, NESTED_PATH):
                status.append(f"{TARGET2} triggers already cleared")
            else:
                bak = os.path.join(os.path.dirname(kspkg_path), BAK2)
                with open(bak, "wb") as f:
                    f.write(d2)
                _write_entry(kspkg_path, TARGET2, _clear_nested_field(d2, NESTED_PATH))
                status.append(f"cleared embedded triggers in {TARGET2}")
        except ValueError as ex:
            status.append(f"skipped {TARGET2}: {ex}")
    return status


def _restore(kspkg_path):
    folder = os.path.dirname(kspkg_path)
    status = []
    for target, bakname in ((TARGET1, BAK1), (TARGET2, BAK2)):
        bak = os.path.join(folder, bakname)
        if not os.path.isfile(bak):
            status.append(f"no backup for {target} ({bakname}) - cannot "
                          f"restore this one; verify game files in Steam")
            continue
        with open(bak, "rb") as f:
            original = f.read()
        _write_entry(kspkg_path, target, original)
        status.append(f"restored {target} ({len(original)} bytes)")
    return status


def _server_kspkg():
    from . import tracks
    return tracks.server_kspkg()


def _client_kspkg():
    from . import tracks
    return tracks.client_kspkg()


def status():
    """State of both installs, for the Servers page to show on load.

    Cached on the two archives' stat, so opening Servers repeatedly costs one
    read rather than four - and a toggle (or a Kunos update) changes the file,
    which changes the key, so the next call recomputes on its own.
    """
    from . import cache
    server = _server_kspkg()
    client = _client_kspkg()

    def compute():
        out = {}
        if server and os.path.isfile(server):
            out["server"] = {"path": server, "state": _state(server)}
        if client and os.path.isfile(client):
            out["client"] = {"path": client, "state": _state(client)}
        return {"ok": True, **out}

    return cache.get("penalties.status", cache.stat_key(server, client),
                     compute)


def set_penalties(side, off):
    """side: 'server' or 'client'. off=True disables, False restores."""
    from . import servers, winproc
    kspkg = _server_kspkg() if side == "server" else _client_kspkg()
    if not kspkg or not os.path.isfile(kspkg):
        return {"ok": False, "error": f"{side} content.kspkg not found"}

    if side == "server" and servers._server_pids():
        return {"ok": False, "error": "stop the server before changing "
                                      "penalties - it holds content.kspkg open"}
    if side == "client" and winproc.pids_named("AssettoCorsaEVO"):
        return {"ok": False, "needs_close": True,
                "error": "close the game before changing penalties"}

    try:
        lines = _patch(kspkg) if off else _restore(kspkg)
    except Exception as ex:
        return {"ok": False, "error": str(ex)}
    return {"ok": True, "detail": lines, "state": _state(kspkg)}
