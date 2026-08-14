"""Putting ACECM somewhere permanent, with shortcuts.

The exe comes out of a build folder or a download, and running it from there
means hunting for it every time - and worse, a downloaded copy tends to get
deleted or moved, taking the shortcut with it. So this copies the running
executable into a stable install folder and makes a Start Menu (and optionally
Desktop) shortcut pointing at THAT copy.

    %LOCALAPPDATA%\\Programs\\ACECM\\ACECM.exe

Per-user on purpose: no admin prompt, and nothing to clean out of Program Files
if someone just deletes the folder.

⚠ The data folder is deliberately NOT touched. A frozen build already keeps its
profiles, registry and caches under %LOCALAPPDATA%\\ACECM, so installing does
not move, copy or endanger anything you have configured - and reinstalling over
the top keeps it all.

⚠ Only meaningful for a frozen build. Run from a source checkout there is no
single file to install, so this reports that rather than copying a script that
would not run without Python.
"""
import os
import shutil
import subprocess
import sys

from . import config, logs, version

APP = "ACECM"


def install_dir():
    base = (os.environ.get("LOCALAPPDATA")
            or os.path.expanduser(r"~\AppData\Local"))
    return os.path.join(base, "Programs", APP)


def installed_exe():
    return os.path.join(install_dir(), f"{APP}.exe")


def _start_menu():
    base = (os.environ.get("APPDATA")
            or os.path.expanduser(r"~\AppData\Roaming"))
    return os.path.join(base, r"Microsoft\Windows\Start Menu\Programs",
                        f"{APP}.lnk")


def _desktop():
    # ⚠ Ask the shell. A OneDrive-redirected profile has no ~\Desktop at all,
    # and writing there fails with DirectoryNotFound.
    from . import detect
    return os.path.join(detect.desktop(), f"{APP}.lnk")


def running_exe():
    """The file to install, or None when running from source."""
    return os.path.abspath(sys.executable) if config.FROZEN else None


def status():
    exe = installed_exe()
    here = running_exe()
    out = {
        "frozen": bool(config.FROZEN),
        "install_dir": install_dir(),
        "installed": os.path.isfile(exe),
        "installed_exe": exe,
        "running_exe": here,
        "running_installed": bool(here and os.path.normcase(here)
                                  == os.path.normcase(exe)),
        "start_menu": os.path.isfile(_start_menu()),
        "desktop": os.path.isfile(_desktop()),
        "version": version.VERSION,
    }
    if out["installed"]:
        try:
            out["installed_size"] = os.path.getsize(exe)
        except OSError:
            pass
    if not out["frozen"]:
        out["note"] = ("running from source - build the exe first "
                       "(python build.py), then install from the built copy")
    return out


def _shortcut(link, target, args="", desc=""):
    """Write a .lnk.

    Windows has no plain-file shortcut format we can emit safely, so this asks
    the shell to make one. PowerShell is always present; pywin32 is not, and
    adding a dependency for four lines is not worth it.
    """
    ps = (
        "$s = (New-Object -ComObject WScript.Shell).CreateShortcut('%s');"
        "$s.TargetPath = '%s';"
        "$s.Arguments = '%s';"
        "$s.WorkingDirectory = '%s';"
        "$s.Description = '%s';"
        "$s.Save()"
    ) % (link.replace("'", "''"), target.replace("'", "''"),
         args.replace("'", "''"), os.path.dirname(target).replace("'", "''"),
         desc.replace("'", "''"))
    r = subprocess.run(["powershell", "-NoProfile", "-NonInteractive",
                        "-Command", ps], capture_output=True, text=True)
    if r.returncode != 0:
        raise RuntimeError((r.stderr or r.stdout or "shortcut failed").strip())
    return link


def installed_version():
    """The version of the installed copy, from the marker written beside it.

    ⚠ Read from a marker rather than the exe: there is no way to ask a frozen
    binary its version without running it, and running an unknown exe to decide
    whether to overwrite it is worse than keeping a text file.
    """
    try:
        with open(os.path.join(install_dir(), "version.txt"),
                  encoding="utf-8") as f:
            return f.read().strip()
    except OSError:
        return ""


def install(desktop=True, force=False):
    """Copy this exe into the install folder and make the shortcuts.

    ⚠ Refuses to go backwards unless forced. The updater replaces the INSTALLED
    copy, so an older download left in Downloads stays old - and installing
    from it would quietly overwrite a newer build with an older one, which
    reads as "the update did not stick".
    """
    src = running_exe()
    if not src:
        return {"ok": False, "error": status()["note"]}
    dst = installed_exe()
    there = installed_version()
    if there and not force and _older(version.VERSION, there):
        return {"ok": False, "downgrade": True,
                "error": f"the installed copy is newer (v{there}) than this "
                         f"one (v{version.VERSION}) - this exe is an old "
                         f"download. Launch ACECM from the Start Menu, or "
                         f"install again with force to go back to "
                         f"v{version.VERSION}."}
    if os.path.normcase(src) == os.path.normcase(dst):
        # already the installed copy - just make sure the shortcuts exist
        made = _shortcuts(dst, desktop)
        return {"ok": True, "already": True, "exe": dst, "shortcuts": made}

    os.makedirs(install_dir(), exist_ok=True)
    try:
        # ⚠ Copy to a temp name and replace, so a failed copy cannot leave a
        # half-written exe where the shortcut points.
        tmp = dst + ".new"
        shutil.copy2(src, tmp)
        os.replace(tmp, dst)
    except PermissionError:
        return {"ok": False,
                "error": "the installed copy is running - close that window "
                         "first, then install again"}
    except OSError as ex:
        return {"ok": False, "error": f"could not copy: {ex}"}

    # tools ship beside the exe in a source layout; a frozen build carries them
    # inside itself, so there is nothing else to bring
    try:
        with open(os.path.join(install_dir(), "version.txt"), "w",
                  encoding="utf-8") as f:
            f.write(version.VERSION)
    except OSError as ex:
        logs.LOG.warning("could not write the version marker: %s", ex)
    made = _shortcuts(dst, desktop)
    logs.LOG.info("installed to %s", dst)
    return {"ok": True, "exe": dst, "shortcuts": made,
            "version": version.VERSION, "replaced": there or None}


def _older(a, b):
    """Is version a older than b? Unparseable versions never block."""
    def parts(v):
        return [int(x) for x in str(v).lstrip("v").split(".")
                if x.isdigit()]
    try:
        pa, pb = parts(a), parts(b)
        return bool(pa) and bool(pb) and pa < pb
    except Exception:
        return False


def _shortcuts(target, desktop=True):
    made = []
    for link, want in ((_start_menu(), True), (_desktop(), desktop)):
        if not want:
            continue
        try:
            _shortcut(link, target, desc=f"{APP} {version.VERSION}")
            made.append(link)
        except Exception as ex:
            logs.LOG.warning("shortcut %s: %s", link, ex)
    return made


def restart(delay=0.8):
    """Quit this process. Relaunch unless an update swap is already waiting.

    ⚠ DETACHED_PROCESS is what made Restart look like it closed ACECM and
    never came back: the child has no console and often no visible window.
    `cmd /c start` is how the update script launches us and it does show.

    ⚠ After Download & install the swap batch is already waiting for this
    PID. Launching another ACECM.exe here holds the file and port 8092, so
    the new build never writes --okflag and the script rolls back. Just
    exit and let the batch swap + start.
    """
    exe = running_exe()
    if not exe:
        return {"ok": False,
                "error": "running from source - restart it the way you "
                         "started it"}
    pending = version.swap_pending()
    if not pending:
        inst = os.path.dirname(exe)
        try:
            # start's first quoted token is the window TITLE, not the exe.
            subprocess.Popen(
                ["cmd", "/c", "start", "ACECM", "/d", inst, exe],
                close_fds=True,
                creationflags=0x00000200,  # CREATE_NEW_PROCESS_GROUP
                cwd=inst)
        except Exception as ex:
            return {"ok": False, "error": f"could not relaunch: {ex}"}

    def bye():
        import time
        time.sleep(0.3 if pending else delay)
        logs.LOG.info("restarting - %s",
                      "swap will relaunch" if pending else "new instance launched")
        os._exit(0)                      # noqa: SLF001 - immediate, no cleanup

    import threading
    threading.Thread(target=bye, daemon=True).start()
    return {"ok": True, "restarting": True, "exe": exe,
            "pending_update": pending}


def uninstall(remove_exe=False):
    """Remove the shortcuts, and optionally the installed copy.

    ⚠ Never touches the data folder. Someone removing a shortcut does not mean
    they want their server profiles deleted.
    """
    gone = []
    for link in (_start_menu(), _desktop()):
        try:
            if os.path.isfile(link):
                os.remove(link)
                gone.append(link)
        except OSError as ex:
            logs.LOG.warning("could not remove %s: %s", link, ex)
    if remove_exe:
        exe = installed_exe()
        try:
            if os.path.isfile(exe):
                os.remove(exe)
                gone.append(exe)
        except OSError as ex:
            return {"ok": False, "removed": gone,
                    "error": f"could not remove {exe}: {ex} "
                             f"(is it running?)"}
    return {"ok": True, "removed": gone, "data_kept": config.DATA}
