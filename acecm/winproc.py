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
        TH32CS_SNAPPROCESS, alive, cmdline, hidden_console_popen,
        hidden_popen,
        hidden_run, hide_console, kill, kill_denied, kill_named,
        pids_named,
        pids_named_prefix, ppid, tcp_listen_pids, working_set,
    )
else:
    from ._proc_posix import (                                 # noqa: F401
        CREATE_NO_WINDOW, PROCESS_QUERY_LIMITED, PROCESS_VM,
        TH32CS_SNAPPROCESS, alive, cmdline, hidden_console_popen,
        hidden_popen,
        hidden_run, hide_console, kill, kill_denied, kill_named,
        pids_named,
        pids_named_prefix, ppid, tcp_listen_pids, working_set,
    )
