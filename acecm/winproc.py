"""Silent process helpers. Never shell out to powershell.exe.

ACECM used to spawn powershell.exe for Get-Process / Get-CimInstance. Each
call flashed a console and Windows toasted it. Toolhelp / psapi stay
in-process.

The module name is historical: this is the process API the rest of ACECM
calls, and it dispatches to `_proc_win` (Toolhelp / psapi / taskkill) or
`_proc_posix` (/proc and signals). Call sites do not branch on platform —
if you find yourself adding `if sys.platform` in a feature module, the
difference belongs down here instead.
"""
import os
import sys

IS_WINDOWS = sys.platform == "win32"


def child_env(env=None):
    """A copy of the environment safe to hand to a program that is not us.

    ⚠ A FROZEN ACECM unpacks itself into a temp folder and that folder ends up
    on the library search path, so every child we start searches OUR bundle
    before its own. Proven from a crash dump of somebody's dedicated server:
    it had loaded

        C:\\Users\\<them>\\AppData\\Local\\Temp\\_MEI133842\\VCRUNTIME140.dll

    - ACECM's C runtime, not the one beside the server exe - and died with an
    unhandled C++ exception. The same server started from Kunos's own
    launcher on the same machine, with the same ports, was fine. Nothing was
    wrong with their firewall; we were poisoning the child's DLL search path.

    ⚠ Matched by CONTENT, not by comparing with sys._MEIPASS. A user can be
    running a second copy of ACECM, and an entry from ITS bundle is just as
    wrong for our child as one from ours.

    ⚠ On Linux the same bug wears a different name. A one-file build exports
    LD_LIBRARY_PATH pointing at its own unpacked libs, and PyInstaller saves
    whatever was there before into LD_LIBRARY_PATH_ORIG. A Proton child that
    inherits it loads our bundled libstdc++/libssl instead of the runtime's,
    which is the identical failure with a different loader. Restore the
    original and strip our bundle from every search path we hand on.
    """
    out = dict(os.environ if env is None else env)
    for key in ("PATH", "Path", "LD_LIBRARY_PATH"):
        path = out.get(key) or ""
        if path:
            keep = [p for p in path.split(os.pathsep)
                    if p and "_MEI" not in p.upper()]
            if keep:
                out[key] = os.pathsep.join(keep)
            else:
                out.pop(key, None)
    # PyInstaller stashes the pre-launch value here; a child wants that one.
    for key in ("LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH"):
        orig = out.pop(key + "_ORIG", None)
        if orig:
            out[key] = orig
    # PyInstaller's own breadcrumbs. A frozen child of a frozen parent reads
    # these and can decide it is running inside our bundle.
    for key in ("_MEIPASS", "_MEIPASS2", "_PYI_APPLICATION_HOME_DIR",
                "_PYI_ARCHIVE_FILE", "_PYI_PARENT_PROCESS_LEVEL"):
        out.pop(key, None)
    return out


if IS_WINDOWS:
    from ._proc_win import (                                   # noqa: F401
        CREATE_NO_WINDOW, PROCESS_QUERY_LIMITED, PROCESS_VM,
        TH32CS_SNAPPROCESS, alive, cmdline, exe_path,
        hidden_console_popen,
        hidden_popen,
        hidden_run, hide_console, kill, kill_denied, kill_named,
        pids_named,
        pids_named_prefix, ppid, tcp_listen_pids, working_set,
    )
else:
    from ._proc_posix import (                                 # noqa: F401
        CREATE_NO_WINDOW, PROCESS_QUERY_LIMITED, PROCESS_VM,
        TH32CS_SNAPPROCESS, alive, cmdline, exe_path,
        hidden_console_popen,
        hidden_popen,
        hidden_run, hide_console, kill, kill_denied, kill_named,
        pids_named,
        pids_named_prefix, ppid, tcp_listen_pids, working_set,
    )


# --------------------------------------------------------------------------
# Console, on demand.
#
# ⚠ The shipped exe is built --windowed (GUI subsystem), so Windows never
# allocates a console for it. That is the ONLY reliable way to stop a black
# window appearing behind the app: a --console build gets its console from the
# OS before a single line of our code runs, so hiding it later always leaves a
# visible flash - and for a double-clicked exe it is on screen for the whole
# multi-second startup.
#
# The cost is that a GUI-subsystem process has no stdout at all, which matters
# because this is also a server tool. So the console is created on demand:
# attach to the terminal that launched us if there is one, otherwise allocate
# our own window - but only for the modes that actually need to be read.
# --------------------------------------------------------------------------

class _NullStream:
    """Somewhere for print() to go when there is no console.

    A --windowed build sets sys.stdout/sys.stderr to None, and then any bare
    print() raises AttributeError. Logging still gets everything: the log file
    handler is independent of stdout."""

    def write(self, s):
        return len(s) if s else 0

    def flush(self):
        pass

    def isatty(self):
        return False

    def fileno(self):
        raise OSError("no console")

    def close(self):
        pass

    @property
    def closed(self):
        return False


def ensure_console(alloc=True):
    """Give this process a readable console. True if one is available.

    Attaches to the launching terminal when there is one, so `ACECM.exe
    --headless` in a shell prints where the user is looking.

    ⚠ alloc=False for background children (--tool). Those are started with
    CREATE_NO_WINDOW precisely so nothing appears on screen; calling
    AllocConsole in one would hand it a brand-new VISIBLE console window - the
    proxy and every telemetry tracker popping a black box, which is worse than
    the single console this whole change exists to remove. They log to files.
    """
    import sys
    if sys.platform != "win32":
        return True
    import ctypes
    from ctypes import wintypes
    _k32 = ctypes.WinDLL("kernel32", use_last_error=True)
    ATTACH_PARENT_PROCESS = ctypes.c_uint(-1).value
    ok = False
    try:
        _k32.GetConsoleWindow.restype = wintypes.HWND
        if _k32.GetConsoleWindow():
            ok = True                      # already have one
        elif _k32.AttachConsole(ATTACH_PARENT_PROCESS):
            ok = True                      # started from a terminal
        elif alloc and _k32.AllocConsole():
            ok = True                      # double-clicked: make our own
    except Exception:
        return False
    if not ok:
        return False
    # Rebind the streams; without this they stay None and print() still fails.
    for name, dev, mode in (("stdout", "CONOUT$", "w"),
                            ("stderr", "CONOUT$", "w"),
                            ("stdin", "CONIN$", "r")):
        try:
            cur = getattr(sys, name, None)
            if cur is not None and not isinstance(cur, _NullStream):
                try:
                    if cur.fileno() >= 0:
                        continue           # already a real stream
                except Exception:
                    pass
            f = open(dev, mode, buffering=1 if mode == "w" else -1,
                     encoding="utf-8", errors="replace")
            setattr(sys, name, f)
        except Exception:
            continue
    return True


def safe_stdio():
    """Make print() harmless when there is no console (the window build)."""
    import sys
    for name in ("stdout", "stderr"):
        if getattr(sys, name, None) is None:
            setattr(sys, name, _NullStream())
