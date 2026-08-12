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


def iter_entries(kspkg_path):
    """Yield (path, size, offset) for every named entry in the archive."""
    total = os.path.getsize(kspkg_path)
    table_start = total - TABLE
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
