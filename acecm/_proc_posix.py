"""POSIX process helpers — the Linux implementation behind `winproc`.

Everything here answers the same questions the NT module does, but from
`/proc` and signals instead of Toolhelp / psapi. Two things make this more
than a mechanical translation:

  * **The processes we care about are Windows binaries running under Proton.**
    `AssettoCorsaEVOServer.exe` is not a Linux image name. Its `/proc/<pid>/comm`
    is the wine thread name, truncated to 15 bytes ("AssettoCorsaEVO" — the
    ".exe" is gone, and so is anything past it, which is exactly where the
    stock and `.percar` servers differ). The name we are looking for only
    survives intact in `/proc/<pid>/cmdline`, as a DOS path with backslashes.
    So match on the cmdline, splitting on both separators.

  * **A process tree is not a job object.** `taskkill /T` walks the tree for
    us; here we build the parent map ourselves. It has to be a real walk:
    Steam's reaper, the Proton python wrapper, `wine`, and the game are four
    generations, and killing the top leaves the game on screen — the same
    failure `/T` exists to prevent on Windows.
"""
import errno
import os
import signal
import socket
import time

# Kept so callers (and the NT module's signature) stay uniform; creationflags
# do not exist here and are dropped before reaching subprocess.
CREATE_NO_WINDOW = 0
TH32CS_SNAPPROCESS = 0
PROCESS_QUERY_LIMITED = 0
PROCESS_VM = 0

PAGESIZE = os.sysconf("SC_PAGE_SIZE")


def _norm(name):
    """Both spellings of `name`, lowercased.

    ⚠ The NT module can append ".exe" and be done, because on Windows every
    image really does end in it. Here a caller's name may resolve to either a
    Proton child ("AssettoCorsaEVOServer.exe") or a native helper with no
    suffix at all, and which one it is depends on the platform rather than on
    the call site. Accepting both keeps the shared callers honest.
    """
    n = (name or "").lower()
    bare = n[:-4] if n.endswith(".exe") else n
    return {bare, bare + ".exe"}


def _pids():
    for e in os.listdir("/proc"):
        if e.isdigit():
            yield int(e)


def _read(path, binary=False):
    try:
        if binary:
            with open(path, "rb") as fh:
                return fh.read()
        with open(path, "r", errors="replace") as fh:
            return fh.read()
    except OSError:
        return b"" if binary else ""


def _cmdline(pid):
    raw = _read(f"/proc/{pid}/cmdline", binary=True)
    return [a.decode("utf-8", "replace") for a in raw.split(b"\0") if a]


def _comm(pid):
    return _read(f"/proc/{pid}/comm").strip()


# The kernel stores only TASK_COMM_LEN-1 = 15 bytes of a process name.
_COMM_MAX = 15


def _basenames(pid):
    """Every plausible image name for `pid`, lowercased.

    ⚠ Windows paths inside a Proton cmdline use backslashes, which
    `os.path.basename` does not treat as a separator on Linux — splitting on
    "/" alone leaves the whole `Z:\\home\\...\\Server.exe` string and nothing
    ever matches. Split on both.

    ⚠ `comm` is IGNORED when it is 15 bytes long, because at that length it
    may be a truncation and there is no way to tell. This is not theoretical:
    `AssettoCorsaEVOServer.exe` truncates to exactly "AssettoCorsaEVO", which
    is the GAME's name. Trusting it made a running dedicated server look like
    a running game client, so Drive reported "launched" for a game that was
    never started and then waited forever for its inspector — and
    `kill_named("AssettoCorsaEVO.exe")` would have killed the server instead.
    The full name is always in the cmdline, so nothing is lost by dropping it.

    ⚠ ONLY argv[0]. A Windows program started under Proton has its own exe as
    argv[0] — that is what identifies it. Every other process in the launch
    chain (`srt-bwrap`, `pv-adverb`, `proton`) also carries the exe path
    somewhere in its arguments, so matching arguments at all reported the
    game as running when nothing but the wrapper had started: Drive said
    "direct launch ok", waited for an inspector that could never appear, and
    the game itself had already failed with no log. The wrappers are not the
    program.
    """
    out = set()
    c = _comm(pid)
    if c and len(c) < _COMM_MAX:
        out.add(c.lower())
    args = _cmdline(pid)
    if args:
        leaf = args[0].replace("\\", "/").rsplit("/", 1)[-1].strip().lower()
        if leaf:
            out.add(leaf)
    return out


def tcp_listen_pids(port):
    """PIDs with a TCP LISTEN socket on `port`.

    Two steps, because /proc/net/tcp knows the socket's inode but not its
    owner: collect listening inodes for the port, then find which process has
    one of them open. Sockets we are not allowed to inspect are skipped, not
    fatal — an unprivileged ACECM can still see its own.
    """
    want = int(port)
    inodes = set()
    for table in ("/proc/net/tcp", "/proc/net/tcp6"):
        for line in _read(table).splitlines()[1:]:
            f = line.split()
            if len(f) < 10:
                continue
            if f[3] != "0A":            # TCP_LISTEN
                continue
            try:
                if int(f[1].rsplit(":", 1)[1], 16) == want:
                    inodes.add(f[9])
            except (ValueError, IndexError):
                continue
    if not inodes:
        return []
    out = []
    for pid in _pids():
        try:
            fds = os.listdir(f"/proc/{pid}/fd")
        except OSError:
            continue                     # gone, or not ours to look at
        for fd in fds:
            try:
                link = os.readlink(f"/proc/{pid}/fd/{fd}")
            except OSError:
                continue
            if link.startswith("socket:[") and link[8:-1] in inodes:
                out.append(pid)
                break
    return out


def ppid(pid):
    """Parent pid, or 0 if unknown."""
    return _ppid(int(pid))


def cmdline(pid):
    """Full command line of `pid` as one string, or "" if unreadable."""
    return " ".join(_cmdline(int(pid)))


def pids_named(*names):
    """PIDs whose image name matches any of `names` (with or without .exe)."""
    want = set().union(*(_norm(n) for n in names)) if names else set()
    return [p for p in _pids() if _basenames(p) & want]


def pids_named_prefix(*prefixes):
    """PIDs whose image name starts with any of `prefixes`."""
    want = [p.lower() for p in prefixes]
    # a prefix is matched as given; do not synthesise a ".exe" variant here
    out = []
    for pid in _pids():
        if any(b.startswith(p) for b in _basenames(pid) for p in want):
            out.append(pid)
    return out


def _state(pid):
    """Single-letter process state from /proc/<pid>/stat, or "" if gone."""
    tail = _read(f"/proc/{int(pid)}/stat").rpartition(")")[2].split()
    return tail[0] if tail else ""


def alive(pid):
    """True if `pid` is a process that still exists AND has not exited.

    ⚠ A zombie is not alive. `/proc/<pid>` outlives the process itself until
    the parent reaps it, and ACECM is that parent — it starts the dedicated
    server with Popen and does not always wait(). Counting "Z" as alive makes
    a server we just killed look like it is still holding 9700 forever: Stop
    reports failure, and the stale pid in runtime.json makes Drive skip Start.
    That is the Windows "reused PID" hazard from the other direction, and the
    same feature breaks.
    """
    if not pid:
        return False
    return _state(pid) not in ("", "Z", "X", "x")


def working_set(pid):
    """Resident set in bytes, or None if the process is gone.

    RSS is the closest analogue to a Windows working set: both count the
    physical pages currently mapped in.
    """
    fields = _read(f"/proc/{int(pid)}/statm").split()
    if len(fields) < 2:
        return None
    try:
        return int(fields[1]) * PAGESIZE
    except ValueError:
        return None


def _ppid(pid):
    """Parent pid, read from the tail of /proc/<pid>/stat.

    ⚠ Field 2 is the comm, in parentheses, and it may itself contain spaces
    or a ')' — splitting the whole line on whitespace mis-indexes every field
    after it. Everything fixed-position starts after the LAST ')'.
    """
    stat = _read(f"/proc/{int(pid)}/stat")
    tail = stat.rpartition(")")[2].split()
    try:
        return int(tail[1])              # ppid is the 2nd field after state
    except (IndexError, ValueError):
        return 0


def _tree(pid):
    """`pid` and every descendant, parents before children."""
    kids = {}
    for p in _pids():
        kids.setdefault(_ppid(p), []).append(p)
    out, stack = [], [int(pid)]
    while stack:
        cur = stack.pop()
        out.append(cur)
        stack.extend(kids.get(cur, ()))
    return out


def _signal_tree(pid, sig):
    """Signal a whole tree. True if we were refused by permissions."""
    denied = False
    # ⚠ Snapshot the tree BEFORE signalling. Killing a parent reparents its
    # children to init, so a walk done afterwards can no longer find them —
    # that is the Linux shape of "killed the wrapper, game still on screen".
    for p in reversed(_tree(pid)):       # children first
        try:
            os.kill(p, sig)
        except ProcessLookupError:
            continue
        except PermissionError:
            denied = True
        except OSError as ex:
            if ex.errno == errno.EPERM:
                denied = True
    return denied


def kill(pid):
    """Force-kill a process tree. True if it is gone afterwards.

    Same contract as the NT version: report the OUTCOME. A server started by
    another user (or a root-owned one) cannot be signalled, and reporting
    success there would leave Stop claiming a port that is still held.

    SIGTERM first — the dedicated server flushes and releases 9700 on a clean
    exit, where SIGKILL can leave the socket in TIME_WAIT and make the next
    start look like a port conflict.
    """
    if not pid:
        return True
    _signal_tree(pid, signal.SIGTERM)
    for _ in range(20):
        time.sleep(0.1)
        if not alive(pid):
            return True
    _signal_tree(pid, signal.SIGKILL)
    for _ in range(20):
        time.sleep(0.1)
        if not alive(pid):
            return True
    return not alive(pid)


def kill_denied(pid):
    """True when this process exists but we lack the rights to kill it."""
    if not alive(pid):
        return False
    try:
        os.kill(int(pid), 0)
    except PermissionError:
        return True
    except OSError as ex:
        return ex.errno == errno.EPERM
    return False


def kill_named(*names):
    """Force-kill every process whose image matches, plus its children."""
    out = []
    for name in names:
        n = (name or "").lower()
        pids = pids_named(name)
        ok = all(kill(p) for p in pids) if pids else True
        out.append((n, 0 if ok else 1))
    return out


def _strip_win_kw(kw):
    """Drop Windows-only subprocess keywords a shared caller may pass."""
    for k in ("creationflags", "startupinfo"):
        kw.pop(k, None)
    return kw


def hidden_run(cmd, **kw):
    """subprocess.run. There is no console to flash."""
    import subprocess
    return subprocess.run(cmd, **_strip_win_kw(kw))


def hidden_popen(cmd, **kw):
    """Start a helper without a terminal of its own."""
    import subprocess
    _strip_win_kw(kw)
    # Its own session, so a Ctrl-C in the terminal that started ACECM does not
    # take the helper down with it.
    kw.setdefault("start_new_session", True)
    return subprocess.Popen(cmd, **kw)


def hidden_console_popen(cmd, **kw):
    """Start a console program.

    The NT twin has to allocate a hidden console or the dedicated server never
    finishes CRT startup. Nothing equivalent is needed here: the wine console
    a Proton child gets is virtual and never appears on screen. The caller's
    contract — a log file it keeps open until the exe exits — is unchanged.
    """
    return hidden_popen(cmd, **kw)


def hide_console():
    """No-op: a Linux build is not attached to a console it must hide."""
    return False
