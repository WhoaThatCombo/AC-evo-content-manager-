"""Reading Kunos .kspkg archives.

Format (see memory kspkg-format-cracked):

    [data region: every file's bytes, XOR'd with the engine key. Each file's
     key phase RESETS TO 0 at its own start, not at the archive's offset 0]
    [fixed 64 MiB table: one 256-byte bucket per path, same phase reset]

Bucket layout, after decrypting with the phase for its own offset:

    [0:N]      null-terminated path, backslash-separated
    [240:244]  uint32 size (0 for a directory entry)
    [248:256]  uint64 offset into the data region

⚠ `.texturemips` are the one entry type stored UNENCRYPTED - the engine
streams mip pages straight off disk. Decrypting them like everything else
turns them into noise that looks exactly like a de-tiling bug, so they are
skipped here. See memory acevo-texturemips-not-encrypted.
"""
import os
import struct

from . import logs

KEY = bytes.fromhex("c135117da921979f")
BUCKET = 256
TABLE = 64 * 1024 * 1024


def _keystream(n, phase):
    return bytes(KEY[(i + phase) % 8] for i in range(n))


def decrypt(buf, offset=0):
    """XOR with the engine key, the key phase taken from `offset`.

    ⚠ For table buckets `offset` is relative to the TABLE'S OWN start, not the
    absolute file offset - the two only agree when the table happens to begin
    on an 8-byte boundary, and getting it wrong yields an index with zero
    readable entries rather than an obvious error.

    Done as one big-integer XOR: byte-at-a-time Python over a 64 MiB table is
    minutes, this is seconds.
    """
    n = len(buf)
    if not n:
        return b""
    ks = _keystream(n, offset % 8)
    return (int.from_bytes(buf, "big") ^ int.from_bytes(ks, "big")).to_bytes(n, "big")


def _plain(path):
    return path.lower().endswith(".texturemips")


def entry_name(entry):
    """The file name of an archive entry.

    ⚠ Not os.path.basename. Entries inside a .kspkg are stored with Windows
    separators (`content\\cars\\ks_foo\\presets\\bar.mechanicalcarpreset`), and
    on Linux a backslash is an ordinary filename character — basename returns
    the WHOLE path unchanged. Everything downstream that expected a bare name
    then quietly sees a path: it cost every `ks_*_mech_N` car id in carsmap,
    which dropped six real Kunos cars out of Drive with no error anywhere.
    """
    return str(entry).replace("\\", "/").rsplit("/", 1)[-1]


def entry_dir(entry):
    """The directory part of an archive entry, in archive form."""
    norm = str(entry).replace("\\", "/")
    return norm.rsplit("/", 1)[0].replace("/", "\\") if "/" in norm else ""


def iter_entries(kspkg_path):
    """Yield (path, size, offset) for every named entry in the archive.

    ⚠ Yields nothing for a file too small to BE an archive. The table is the
    last 64 MiB, so a partial download or a mis-named file gives a negative
    table offset and seek() raises - which used to abort the caller's whole
    scan. One junk file in a mods folder then read as "you have no cars at
    all", sending people hunting for a missing mod that was never the problem.
    """
    total = os.path.getsize(kspkg_path)
    table_start = total - TABLE
    if table_start < 0:
        logs.LOG.warning("%s is %d bytes - too small to be a kspkg, skipping",
                         os.path.basename(kspkg_path), total)
        return
    # Most buckets are empty and hold one repeating pattern; skipping them
    # before decrypting is what keeps a 64 MiB table to a few seconds.
    fillers = {_keystream(BUCKET, ph) for ph in range(8)}
    with open(kspkg_path, "rb") as f:
        f.seek(table_start)
        rel = 0
        while True:
            chunk = f.read(BUCKET * 4096)
            if not chunk:
                return
            for i in range(0, len(chunk) - BUCKET + 1, BUCKET):
                raw = chunk[i:i + BUCKET]
                pos = rel + i
                if raw in fillers:
                    continue
                dec = decrypt(raw, pos)
                end = dec.find(b"\0")
                if end <= 0:
                    continue
                try:
                    path = dec[:end].decode("utf-8")
                except UnicodeDecodeError:
                    continue
                if "\\" not in path:
                    continue
                size = struct.unpack("<I", dec[240:244])[0]
                # ⚠ the high dword is load-bearing: Kunos's own content.kspkg
                # is ~74 GB and its offsets exceed 2^32
                offset = struct.unpack("<Q", dec[248:256])[0]
                yield path, size, offset
            rel += len(chunk)


def read_entry(fh, size, offset, path):
    fh.seek(offset)
    raw = fh.read(size)
    return raw if _plain(path) else decrypt(raw, 0)


def extract_prefix(kspkg_path, prefix, out_dir, strip=3, progress=None):
    """Extract every entry under `prefix` into `out_dir`.

    One pass over the index, then reads sorted by offset so a 74 GB file
    streams forwards instead of seeking back and forth. `strip` drops that
    many leading path components (content\\cars\\<id>\\ by default).
    """
    prefix = prefix.lower().rstrip("\\") + "\\"
    wanted = [(p, s, o) for p, s, o in iter_entries(kspkg_path)
              if s and p.lower().startswith(prefix)]
    if not wanted:
        return 0
    wanted.sort(key=lambda x: x[2])
    total = 0
    with open(kspkg_path, "rb") as f:
        for n, (path, size, offset) in enumerate(wanted):
            parts = path.split("\\")
            rel = os.path.join(*parts[strip:]) if len(parts) > strip else parts[-1]
            dst = os.path.join(out_dir, rel)
            os.makedirs(os.path.dirname(dst), exist_ok=True)
            with open(dst, "wb") as o:
                o.write(read_entry(f, size, offset, path))
            total += size
            if progress and n % 50 == 0:
                progress(n, len(wanted))
    return len(wanted)


def extract_one(kspkg_path, entry_path, dst):
    """Extract a single entry by exact path (case-insensitive)."""
    target = entry_path.lower()
    for path, size, offset in iter_entries(kspkg_path):
        if size and path.lower() == target:
            os.makedirs(os.path.dirname(dst), exist_ok=True)
            with open(kspkg_path, "rb") as f:
                data = read_entry(f, size, offset, path)
            with open(dst, "wb") as o:
                o.write(data)
            return True
    return False


def find_entry(kspkg_path, entry_path):
    """(size, offset) for one entry, or (None, None) - WITHOUT scanning.

    The table is a dense array sorted by FNV-1a-64 of the path as UTF-16LE
    (see kspkg_write for the measured layout), so a single entry is a binary
    search: ~18 probes of 256 bytes each instead of decrypting a quarter of a
    million buckets. iter_entries is still the right call when you genuinely
    want everything; this is for "is this one file in here, and where".

    ⚠ Buckets are 256-byte aligned, so every record decrypts at key phase 0
    regardless of where the table starts.
    """
    from . import kspkg_write
    total = os.path.getsize(kspkg_path)
    table_start = total - TABLE
    if table_start < 0:
        return None, None
    want = kspkg_write.entry_id(entry_path)
    target = entry_path.lower()

    with open(kspkg_path, "rb") as f:
        def rec(i):
            f.seek(table_start + i * BUCKET)
            raw = f.read(BUCKET)
            return decrypt(raw, 0) if len(raw) == BUCKET else None

        def used(dec):
            # ⚠ NOT the `kind` field: 0x0000 shows up on plenty of real
            # entries (irradiance volumes, shadow caches), so testing kind
            # put the end of the array at index 3 and lost ~a quarter of all
            # lookups. A zero id is what actually marks the empty tail.
            return dec is not None and struct.unpack("<Q", dec[232:240])[0] != 0

        # the array is packed from 0, so the used region is a prefix - find
        # its length by bisecting on "is this record still a real one"
        lo, hi = 0, TABLE // BUCKET
        while lo < hi:
            mid = (lo + hi) // 2
            if used(rec(mid)):
                lo = mid + 1
            else:
                hi = mid
        count = lo

        lo, hi = 0, count
        while lo < hi:
            mid = (lo + hi) // 2
            dec = rec(mid)
            got = struct.unpack("<Q", dec[232:240])[0]
            if got < want:
                lo = mid + 1
            else:
                hi = mid

        # ⚠ walk equal ids rather than trusting the first hit: the id is a
        # hash, and a collision would otherwise silently return the wrong file
        i = lo
        while i < count:
            dec = rec(i)
            if struct.unpack("<Q", dec[232:240])[0] != want:
                break
            end = dec.find(b"\0")
            if end > 0:
                try:
                    if dec[:end].decode("utf-8").lower() == target:
                        return (struct.unpack("<I", dec[240:244])[0],
                                struct.unpack("<Q", dec[248:256])[0])
                except UnicodeDecodeError:
                    pass
            i += 1
    return None, None
