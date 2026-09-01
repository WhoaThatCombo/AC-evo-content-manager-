"""Protobuf schemas, extracted from the user's OWN game install.

The game's message definitions are compiled into its executables as
FileDescriptorProtos. We deliberately do not redistribute them - they are
derived from Kunos' binary - so a shipped build extracts them on this machine,
from this user's copy, and caches the result under <data>/protos.

That is what makes Game settings work: the settings files under
`Saved Games\\ACE` are raw protobuf whose FILE EXTENSION is the message name
(`video.videosettings` -> `VideoSettings`), so without the schemas there is
nothing to decode and the page is empty.

⚠ The blob's length is NOT marked, and guessing it does not work. Probing by
powers of two overshoots the real size, the greedy parse then throws, and you
silently get a small number of TRUNCATED descriptors - which parse, look fine,
and then fail to satisfy anyone's dependencies. (Measured: guessing recovered
63 files of which only 17 could be loaded; walking the length recovers 90 with
zero failures.) The length is computed exactly, by walking top-level fields
until the wire stops making sense.

⚠ The name regex must match a length byte of 0x0a - a name of exactly ten
characters. `.` does not match a newline, which silently hid Math.proto and
with it everything that depends on Math.proto.
"""
import glob
import os
import re
import struct

from . import config, logs

CACHE = None            # lazily built {message name -> descriptor}


def cache_dir():
    d = os.path.join(config.DATA, "protos")
    os.makedirs(d, exist_ok=True)
    return d


def _sources():
    """Executables worth scanning, best first."""
    from . import detect
    out = []
    game = (config.CFG.get("game_exe") or "").strip() or detect.find("game_exe")
    if game and os.path.isfile(game):
        out.append(game)
    srv = config.server_exe()
    if srv and os.path.isfile(srv):
        out.append(srv)
    return out


def _varint(d, i):
    r = s = 0
    while True:
        b = d[i]
        i += 1
        r |= (b & 0x7F) << s
        if not b & 0x80:
            return r, i
        s += 7


def _blob_len(d, off, limit=1 << 22):
    """Exact length of the descriptor at off, by walking its top-level fields."""
    i, end = off, min(len(d), off + limit)
    known = {1, 2, 3, 4, 5, 6, 7, 8, 9, 12}      # FileDescriptorProto fields
    while i < end:
        start = i
        try:
            key, i = _varint(d, i)
        except Exception:
            return start - off
        fn, wt = key >> 3, key & 7
        if fn not in known or wt not in (0, 2):
            return start - off
        if wt == 0:
            try:
                _, i = _varint(d, i)
            except Exception:
                return start - off
        else:
            try:
                ln, i = _varint(d, i)
            except Exception:
                return start - off
            i += ln
        if i > end:
            return start - off
    return end - off


def _parse_at(d, off):
    """Parse one descriptor using its exact on-wire length."""
    from google.protobuf import descriptor_pb2 as dp
    n = _blob_len(d, off)
    for size in (n, n - 1, n - 2):               # tolerate a trailing byte
        if size < 8:
            continue
        fd = dp.FileDescriptorProto()
        try:
            fd.ParseFromString(bytes(d[off:off + size]))
            if fd.name:
                return fd
        except Exception:
            pass
    return None


def extract(path=None, force=False):
    """Pull FileDescriptorProtos out of an executable into <data>/protos."""
    from google.protobuf import descriptor_pb2 as dp
    d = cache_dir()
    srcs = [path] if path else _sources()
    srcs = [s for s in srcs if s and os.path.isfile(s)]
    # ⚠ The cache is keyed on the SOURCE exes, not just "do .desc files
    # exist". A game update rewrites the executable and changes the schemas
    # inside it - 9.0 -> 9.1 changed 36 of them, including the multiplayer
    # protocol - but the old code served whatever was already extracted, so
    # ACECM silently kept parsing with last version's layout. The stamp is
    # (path, size, mtime) of every source; when it moves, re-extract.
    stampfile = os.path.join(d, ".sources")
    stamp = chr(10).join(
        f"{os.path.abspath(x)}|{os.path.getsize(x)}|{int(os.path.getmtime(x))}"
        for x in srcs)
    if not force and glob.glob(os.path.join(d, "*.desc")):
        try:
            if open(stampfile, encoding="utf-8").read() == stamp:
                return {"ok": True, "cached": True, "dir": d,
                        "count": len(glob.glob(os.path.join(d, "*.desc")))}
        except OSError:
            pass
        # stale: the game changed under us, so clear before re-reading
        for old_desc in glob.glob(os.path.join(d, "*.desc")):
            try:
                os.remove(old_desc)
            except OSError:
                pass
        logs.LOG.info("game executable changed - re-extracting schemas")
    if not srcs:
        return {"ok": False, "error": "no game or server executable found to "
                                      "read the schemas from - set game_exe in "
                                      "Settings"}
    found = {}
    for src in srcs:
        try:
            data = open(src, "rb").read()
        except OSError as ex:
            logs.LOG.warning("cannot read %s: %s", src, ex)
            continue
        # [\s\S] not . - see the module docstring about the 0x0a length byte
        for m in re.finditer(rb"\x0a([\s\S])([A-Za-z0-9_/]{3,60}\.proto)", data):
            if m.group(1)[0] != len(m.group(2)):
                continue
            fd = _parse_at(data, m.start())
            # ⚠ Do NOT require message_type. Options.proto declares only
            # extensions, so skipping "empty" descriptors dropped it - and 35
            # files depend on it, which cascaded into only 23 of 90 loading and
            # an empty Game settings page. Keep anything that parses.
            if fd and fd.name:
                prev = found.get(fd.name)
                if (not prev or len(fd.SerializeToString())
                        > len(prev.SerializeToString())):
                    found[fd.name] = fd
        logs.LOG.info("extracted %d descriptor(s) so far from %s",
                      len(found), os.path.basename(src))
    for name, fd in found.items():
        safe = name.replace("/", "_").replace("\\", "_") + ".desc"
        with open(os.path.join(d, safe), "wb") as f:
            f.write(fd.SerializeToString())
    if found:
        try:
            open(stampfile, "w", encoding="utf-8").write(stamp)
        except OSError as ex:
            logs.LOG.info("could not write the schema stamp: %s", ex)
    return {"ok": bool(found), "dir": d, "count": len(found),
            "sources": [os.path.basename(s) for s in srcs],
            "error": None if found else "no descriptors found in those files"}


def _dirs():
    """Where .desc files might live: our cache, then any configured tools dir."""
    out = [cache_dir()]
    t = config.tools_dir()
    for cand in (os.path.join(t, "protos"), t):
        if os.path.isdir(cand):
            out.append(cand)
    return out


def _pool():
    """A descriptor pool built from every .desc we can find."""
    global CACHE
    if CACHE is not None:
        return CACHE
    from google.protobuf import descriptor_pb2 as dp
    from google.protobuf import descriptor_pool, message_factory

    # Provision on demand. Nothing else triggers extraction, so without this
    # a fresh install shows an empty Game settings page and never explains why.
    if not glob.glob(os.path.join(cache_dir(), "*.desc")):
        r = extract()
        logs.LOG.info("first-run schema extraction: %s", r)

    pool = descriptor_pool.DescriptorPool()
    files = {}
    for d in _dirs():
        for p in glob.glob(os.path.join(d, "*.desc")):
            try:
                fd = dp.FileDescriptorProto()
                fd.ParseFromString(open(p, "rb").read())
                files.setdefault(fd.name, fd)
            except Exception:
                continue
    # Dependencies must be added before dependents, and the extraction order is
    # arbitrary - so keep retrying until a pass adds nothing new.
    pending = dict(files)
    while pending:
        progressed = False
        for name, fd in list(pending.items()):
            try:
                pool.Add(fd)
                pending.pop(name)
                progressed = True
            except Exception:
                continue
        if not progressed:
            for name in pending:
                logs.LOG.debug("descriptor not loadable (missing deps): %s", name)
            break

    # ⚠ Look messages up in the POOL by name. Enumerating
    # file.message_types_by_name only sees files whose FindFileByName succeeds,
    # so anything whose dependencies failed to load silently disappeared -
    # VideoSettings and friends went missing and Game settings stayed empty.
    index = {}
    for name in files:
        try:
            f = pool.FindFileByName(name)
        except Exception:
            continue
        for msg in f.message_types_by_name:
            index[msg] = name
    CACHE = (pool, index, message_factory)
    logs.LOG.info("protobuf pool: %d file(s) offered, %d loaded, %d message(s)",
                  len(files), len(files) - len(pending), len(index))
    return CACHE


def has(name):
    pool, _index, _f = _pool()
    try:
        pool.FindMessageTypeByName(name)
        return True
    except Exception:
        return False


def new(name):
    pool, _index, factory = _pool()
    return factory.GetMessageClass(pool.FindMessageTypeByName(name))()


def message_names():
    _pool_, index, _f = _pool()
    return sorted(index)


def state():
    d = cache_dir()
    n = len(glob.glob(os.path.join(d, "*.desc")))
    try:
        msgs = len(message_names())
    except Exception:
        msgs = 0
    return {"dir": d, "descriptors": n, "messages": msgs,
            "sources": [os.path.basename(s) for s in _sources()]}
