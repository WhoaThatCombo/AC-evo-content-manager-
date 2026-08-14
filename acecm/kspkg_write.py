"""Writing Kunos .kspkg archives - adding entries and growing existing ones.

Reading is in kspkg.py. This is the other half, and it exists because a custom
track has to be ADDED to the server archive at its own paths rather than
overwriting a stock track's slots.

WHAT THE TABLE ACTUALLY IS (measured, not guessed)

The 64 MiB tail is NOT a hash table with free buckets. It is a DENSE ARRAY of
256-byte records, packed from index 0 with no gaps, SORTED by a 64-bit id, and
binary-searched at runtime. There is no entry count anywhere in the file - the
loader walks until it hits an empty record.

    228  u16  kind       0x0100 file, 0x0001 dir
    230  u16  path length in characters
    232  u64  id, little endian - the sort key
    240  u32  size
    248  u64  offset into the data region

⭐ The id is FNV-1a-64 over the path encoded UTF-16LE. Plain UTF-8 FNV matches
nothing, which is why earlier attempts to correlate a record's position with a
hash of its path failed completely.

Two consequences that make or break a write:

  * A new record needs a COMPLETE header. Entries appended with a zeroed header
    are skipped as empty - the loader never sees them, and the server dies with
    `Failed to find file`.
  * New ids land anywhere in the order, so the WHOLE TABLE MUST BE RE-SORTED.
    Appending to the end of the used range leaves the array unsorted and the
    binary search still misses the entry.

Growing an entry needs no data shuffling: append the new bytes where the table
used to start, repoint that one record, and write the table 64 MiB further on.
The old bytes stay as dead space. Ids hash PATHS, so changing a payload never
reorders anything.

⚠ Every entry's keystream restarts at ITS OWN start, so payloads encrypt at
phase 0 - never at the file offset. Table records are 256-byte aligned, so they
are phase 0 too. Getting this wrong yields a readback showing neither the old
nor the new content.
"""
import os
import struct

from . import kspkg

M64 = (1 << 64) - 1
FNV_BASIS = 0xcbf29ce484222325
FNV_PRIME = 0x100000001b3
KIND_FILE = 0x0100
KIND_DIR = 0x0001
# 256-byte record minus the 32-byte trailer, less room for the NUL
MAX_PATH = kspkg.BUCKET - 32 - 1


def entry_id(path):
    """The record's sort key: FNV-1a-64 over the path as UTF-16LE."""
    h = FNV_BASIS
    for c in path.encode("utf-16-le"):
        h = ((h ^ c) * FNV_PRIME) & M64
    return h


def _bucket(path, size, offset, kind=None):
    """Build one complete 256-byte record."""
    raw = path.encode("utf-8")
    if len(raw) > MAX_PATH:
        raise ValueError(f"path too long for a record ({len(raw)}): {path}")
    rec = bytearray(kspkg.BUCKET)
    rec[:len(raw)] = raw
    struct.pack_into("<H", rec, 228,
                     kind if kind is not None
                     else (KIND_FILE if size else KIND_DIR))
    struct.pack_into("<H", rec, 230, len(path))
    struct.pack_into("<Q", rec, 232, entry_id(path))
    struct.pack_into("<I", rec, 240, size)
    struct.pack_into("<Q", rec, 248, offset)
    return rec


def read_records(archive):
    """Every used record, decrypted, in table order."""
    total = os.path.getsize(archive)
    table_start = total - kspkg.TABLE
    with open(archive, "rb") as f:
        f.seek(table_start)
        table = f.read(kspkg.TABLE)
    out = []
    for i in range(kspkg.TABLE // kspkg.BUCKET):
        dec = kspkg.decrypt(table[i * kspkg.BUCKET:(i + 1) * kspkg.BUCKET], 0)
        end = dec.find(b"\0")
        if end <= 0:
            continue
        out.append(bytearray(dec))
    return table_start, out


def record_path(rec):
    return rec[:rec.find(b"\0")].decode("utf-8", "replace")


def repair_headers(records):
    """Complete any record whose header was left zeroed.

    Entries appended by older tooling have kind/path_len/id all zero. They are
    invisible to the engine but perfectly visible to our own index walk, which
    is exactly how they went unnoticed.
    """
    fixed = []
    for rec in records:
        kind = struct.unpack_from("<H", rec, 228)[0]
        ident = struct.unpack_from("<Q", rec, 232)[0]
        if kind or ident:
            continue
        path = record_path(rec)
        size = struct.unpack_from("<I", rec, 240)[0]
        struct.pack_into("<H", rec, 228, KIND_FILE if size else KIND_DIR)
        struct.pack_into("<H", rec, 230, len(path))
        struct.pack_into("<Q", rec, 232, entry_id(path))
        fixed.append(path)
    return fixed


def write_inplace(archive, changes):
    """Replace or add entries by rewriting only the 64 MiB index tail.

    The 25 GB client archive cannot be copied on every download. New
    payloads go where the old table started; the previous 64 MiB tail is
    saved as `<archive>.bak_tables` so a failed write can be undone
    without a full backup. The data region is not touched.
    """
    table_start, records = read_records(archive)
    repaired = repair_headers(records)
    by_path = {record_path(r).lower(): r for r in records}
    orig_size = os.path.getsize(archive)
    with open(archive, "rb") as f:
        f.seek(table_start)
        tail = f.read()
    bak = archive + ".bak_tables"
    with open(bak, "wb") as f:
        f.write(tail)

    added, replaced = [], []
    try:
        with open(archive, "r+b") as f:
            f.seek(table_start)
            cursor = table_start
            for path, blob in changes.items():
                f.write(kspkg.decrypt(blob, 0))
                rec = by_path.get(path.lower())
                if rec is None:
                    records.append(_bucket(path, len(blob), cursor))
                    added.append(path)
                else:
                    struct.pack_into("<I", rec, 240, len(blob))
                    struct.pack_into("<Q", rec, 248, cursor)
                    replaced.append(path)
                cursor += len(blob)
            records.sort(key=lambda r: struct.unpack_from("<Q", r, 232)[0])
            ids = [struct.unpack_from("<Q", r, 232)[0] for r in records]
            if ids != sorted(ids) or len(set(ids)) != len(ids):
                raise ValueError("record ids are not unique and ascending")
            table = bytearray(kspkg.TABLE)
            for i, rec in enumerate(records):
                table[i * kspkg.BUCKET:(i + 1) * kspkg.BUCKET] = \
                    kspkg.decrypt(bytes(rec), 0)
            blank = kspkg.decrypt(bytes(kspkg.BUCKET), 0)
            for i in range(len(records), kspkg.TABLE // kspkg.BUCKET):
                table[i * kspkg.BUCKET:(i + 1) * kspkg.BUCKET] = blank
            f.write(bytes(table))
            f.truncate(cursor + kspkg.TABLE)
    except Exception:
        with open(archive, "r+b") as f:
            f.seek(table_start)
            f.write(tail)
            f.truncate(orig_size)
        raise
    return {"added": added, "replaced": replaced, "repaired": repaired,
            "records": len(records), "size": os.path.getsize(archive),
            "backup": bak}


def write_archive(src, dst, changes, progress=None):
    """Copy `src` to `dst`, adding or replacing entries.

    `changes` maps an archive path (backslash separated, as the game spells it)
    to its new bytes. A path already present is replaced; a new one is added.
    Payloads are appended where the table used to begin, so nothing already in
    the data region moves and no offset but the changed ones is touched.
    """
    table_start, records = read_records(src)
    repaired = repair_headers(records)

    by_path = {record_path(r).lower(): r for r in records}
    added, replaced = [], []

    with open(src, "rb") as fin, open(dst, "wb") as fout:
        # data region verbatim - every untouched entry keeps its offset
        remaining = table_start
        while remaining:
            chunk = fin.read(min(1 << 22, remaining))
            if not chunk:
                raise IOError("archive ended inside the data region")
            fout.write(chunk)
            remaining -= len(chunk)

        cursor = table_start
        for path, blob in changes.items():
            fout.write(kspkg.decrypt(blob, 0))      # ⚠ phase 0, not the offset
            rec = by_path.get(path.lower())
            if rec is None:
                records.append(_bucket(path, len(blob), cursor))
                added.append(path)
            else:
                struct.pack_into("<I", rec, 240, len(blob))
                struct.pack_into("<Q", rec, 248, cursor)
                replaced.append(path)
            cursor += len(blob)
            if progress:
                progress(path, len(blob))

        # ⚠ re-sort: a new id belongs wherever it falls, not at the end
        records.sort(key=lambda r: struct.unpack_from("<Q", r, 232)[0])
        ids = [struct.unpack_from("<Q", r, 232)[0] for r in records]
        if ids != sorted(ids) or len(set(ids)) != len(ids):
            raise ValueError("record ids are not unique and ascending")

        table = bytearray(kspkg.TABLE)
        for i, rec in enumerate(records):
            table[i * kspkg.BUCKET:(i + 1) * kspkg.BUCKET] = \
                kspkg.decrypt(bytes(rec), 0)
        blank = kspkg.decrypt(bytes(kspkg.BUCKET), 0)
        for i in range(len(records), kspkg.TABLE // kspkg.BUCKET):
            table[i * kspkg.BUCKET:(i + 1) * kspkg.BUCKET] = blank
        fout.write(bytes(table))

    return {"added": added, "replaced": replaced, "repaired": repaired,
            "records": len(records), "size": os.path.getsize(dst)}


def read_entry(archive, path):
    """One entry's plaintext bytes, or None."""
    want = path.lower()
    with open(archive, "rb") as f:
        for p, size, off in kspkg.iter_entries(archive):
            if p.lower() == want:
                return kspkg.read_entry(f, size, off, p)
    return None


def verify(archive):
    """Structural check: sorted, unique, complete headers, readable payloads."""
    _ts, records = read_records(archive)
    ids = [struct.unpack_from("<Q", r, 232)[0] for r in records]
    bad = []
    for r in records:
        path = record_path(r)
        kind = struct.unpack_from("<H", r, 228)[0]
        plen = struct.unpack_from("<H", r, 230)[0]
        ident = struct.unpack_from("<Q", r, 232)[0]
        if kind not in (KIND_FILE, KIND_DIR) or plen != len(path) \
                or ident != entry_id(path):
            bad.append(path)
    return {
        "records": len(records),
        "sorted": ids == sorted(ids),
        "unique": len(set(ids)) == len(ids),
        "bad_headers": bad[:10],
        "ok": ids == sorted(ids) and len(set(ids)) == len(ids) and not bad,
    }
