"""A small cache for answers that are expensive to compute and rarely change.

Most of what the UI asks for on every page switch is derived from files that
sit still for hours at a time - content.kspkg, a profile, the mods folder. The
pages were recomputing all of it on every navigation, which is what made
switching tabs feel like work.

⚠ Everything here is keyed on the STAT of the files the answer came from, not
on a timer. A stale toggle state is worse than a slow one: if the archive
changes - we patch it, or a Kunos update replaces it - the key changes with it
and the next read recomputes. Nothing needs to remember to invalidate, which
is the only version of this that stays correct as pages get added.

Deliberately NOT cached: anything about a running process (server state, drive
status, telemetry). Those change on their own with no file to notice it by.
"""
import os
import threading


_lock = threading.Lock()
_store = {}


def stat_key(*paths):
    """A key that changes whenever any of these files does.

    Missing files are part of the key too - "not there yet" is a real state,
    and an answer computed while a path was missing must not outlive it
    appearing.
    """
    parts = []
    for p in paths:
        if not p:
            parts.append(None)
            continue
        try:
            st = os.stat(p)
            parts.append((int(st.st_mtime_ns), st.st_size))
        except OSError:
            parts.append(None)
    return tuple(parts)


def dir_key(*paths, depth=1):
    """A key that changes when a FOLDER's contents change.

    ⚠ stat_key is wrong for anything derived from a folder. Installing or
    deleting a mod does not touch the files we already looked at, and on a
    deletion there may be no file left to stat at all - so a file-stat key
    happily serves a list that still contains something the user removed.
    This keys on the listing itself: names, sizes and mtimes.

    `depth` is how many levels down to look. 1 is the folder's own entries,
    which is enough when what you cache is "which mods exist". Raise it only
    when the answer really depends on file contents further down - each level
    costs a full listing every time the key is computed.
    """
    parts = []
    for p in paths:
        if not p:
            parts.append(None)
            continue
        parts.append(_walk_key(p, depth))
    return tuple(parts)


def _walk_key(path, depth):
    try:
        entries = []
        with os.scandir(path) as it:
            for e in sorted(it, key=lambda x: x.name):
                try:
                    st = e.stat()
                except OSError:
                    entries.append((e.name, None))
                    continue
                if e.is_dir() and depth > 1:
                    entries.append((e.name, _walk_key(e.path, depth - 1)))
                elif e.is_dir():
                    entries.append((e.name, "d", int(st.st_mtime_ns)))
                else:
                    entries.append((e.name, st.st_size, int(st.st_mtime_ns)))
        return tuple(entries)
    except OSError:
        # missing or unreadable is itself a state worth keying on
        return None


def get(name, key, compute):
    """Cached `compute()`, recomputed whenever `key` differs from last time.

    ⚠ compute() runs OUTSIDE the lock. It can take a second and can touch the
    disk; holding the lock across it would make every other page wait on the
    slowest one - the opposite of the point. Two callers racing may both
    compute, which costs a little work but cannot produce a wrong answer.
    """
    with _lock:
        hit = _store.get(name)
        if hit is not None and hit[0] == key:
            return hit[1]
    value = compute()
    with _lock:
        _store[name] = (key, value)
    return value


def drop(*names):
    """Forget entries by name; no argument forgets everything.

    The stat key already handles files changing underneath us. This is for the
    cases it cannot see - a setting that changes where we look, rather than
    the content of what we find.
    """
    with _lock:
        if not names:
            _store.clear()
        for n in names:
            _store.pop(n, None)
