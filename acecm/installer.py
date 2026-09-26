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

from . import config, logs, version, winproc

APP = "ACECM"


IS_WINDOWS = sys.platform == "win32"


def install_dir():
    if not IS_WINDOWS:
        # ~/.local/bin is on PATH for a normal login shell and needs no root,
        # which matters on an immutable OS where /usr is read-only.
        return os.path.join(os.path.expanduser("~"), ".local", "bin")
    base = (os.environ.get("LOCALAPPDATA")
            or os.path.expanduser(r"~\AppData\Local"))
    return os.path.join(base, "Programs", APP)


def installed_exe():
    if not IS_WINDOWS:
        return os.path.join(install_dir(), APP)
    return os.path.join(install_dir(), f"{APP}.exe")


def _start_menu():
    if not IS_WINDOWS:
        # The XDG equivalent of the Start Menu: an application entry the
        # desktop's launcher picks up.
        base = os.path.join(os.environ.get("XDG_DATA_HOME")
                            or os.path.join(os.path.expanduser("~"),
                                            ".local", "share"),
                            "applications")
        return os.path.join(base, f"{APP}.desktop")
    base = (os.environ.get("APPDATA")
            or os.path.expanduser(r"~\AppData\Roaming"))
    return os.path.join(base, r"Microsoft\Windows\Start Menu\Programs",
                        f"{APP}.lnk")


def _desktop():
    # ⚠ Ask the shell. A OneDrive-redirected profile has no ~\Desktop at all,
    # and writing there fails with DirectoryNotFound.
    from . import detect
    ext = ".lnk" if IS_WINDOWS else ".desktop"
    return os.path.join(detect.desktop(), f"{APP}{ext}")


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


def _desktop_entry(link, target, args="", desc=""):
    """Write an XDG .desktop launcher.

    ⚠ Must be executable, and on a desktop file placed in ~/Desktop it must
    also be trusted by the file manager, or GNOME/KDE show it as a text file
    rather than an icon. Nothing we can do about the trust bit from here —
    the user right-clicks "Allow launching" once — but without the exec bit
    it does not even offer that.
    """
    exe = target if not args else f'{target} {args}'
    icon = os.path.join(os.path.dirname(os.path.abspath(target)), "acecm.ico")
    body = [
        "[Desktop Entry]",
        "Type=Application",
        f"Name={APP}",
        f"Comment={desc or APP}",
        # ⚠ Quote the path: an install under a directory with a space in it
        # otherwise parses as a command plus an argument and fails silently.
        f'Exec="{target}"' + (f" {args}" if args else ""),
        f"Path={os.path.dirname(os.path.abspath(target))}",
        "Terminal=false",
        "Categories=Game;Utility;",
        "StartupNotify=true",
    ]
    if os.path.isfile(icon):
        body.insert(5, f"Icon={icon}")
    os.makedirs(os.path.dirname(link), exist_ok=True)
    with open(link, "w", encoding="utf-8") as fh:
        fh.write("\n".join(body) + "\n")
    os.chmod(link, 0o755)
    return link


def _shortcut(link, target, args="", desc=""):
    """Write a .lnk (or a .desktop entry on Linux).

    Windows has no plain-file shortcut format we can emit safely, so this asks
    the shell to make one. PowerShell is always present; pywin32 is not, and
    adding a dependency for four lines is not worth it.
    """
    if not IS_WINDOWS:
        return _desktop_entry(link, target, args, desc)
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
    r = winproc.hidden_run(["powershell", "-NoProfile", "-NonInteractive",
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


def headless_shortcut(desktop=True):
    """A shortcut that starts ACECM as a server, with no window.

    ⚠ Points at the INSTALLED copy, not at wherever this process happens to be
    running from. A shortcut to a file in Downloads (or to a build in a source
    tree) breaks the moment that file moves, and the whole point of this one is
    that somebody double-clicks it months later on a machine nobody sits at.
    """
    target = installed_exe()
    if not os.path.isfile(target):
        # not installed yet - a shortcut to a copy that may be deleted is
        # worse than none, so say what to do instead of making a dud
        running = running_exe()
        if not running:
            return {"ok": False,
                    "error": "no installed ACECM.exe to point at - use "
                             "Install to a permanent folder first"}
        target = running
    made, failed = [], []
    for link, want in ((_start_menu().replace(f"{APP}.lnk",
                                              f"{APP} Server.lnk"), True),
                       (_desktop().replace(f"{APP}.lnk",
                                           f"{APP} Server.lnk"), desktop)):
        if not want:
            continue
        try:
            _shortcut(link, target, args="--headless",
                      desc=f"{APP} as a headless server (remote admin)")
            made.append(link)
        except Exception as ex:
            logs.LOG.warning("headless shortcut %s: %s", link, ex)
            failed.append(str(ex))
    if not made:
        return {"ok": False,
                "error": "; ".join(failed) or "could not write the shortcut"}
    logs.LOG.info("headless shortcut(s): %s", ", ".join(made))
    return {"ok": True, "made": made, "target": target}


def restart(delay=0.8):
    """Quit this process. Relaunch after this PID is gone.

    ⚠ Never start a second ACECM.exe while this one is still alive. The
    child cannot bind port 8092, exits immediately, then this window
    closes — Restart looks like it shut ACECM down and never came back.

    After Download & install the swap batch is already waiting for this
    PID, so we only exit. Otherwise a small wait-then-start script does
    the same job without fighting for the port.
    """
    exe = running_exe()
    if not exe:
        return {"ok": False,
                "error": "running from source - restart it the way you "
                         "started it"}
    pending = version.swap_pending()
    if not pending:
        try:
            version.schedule_relaunch(exe, os.getpid())
        except Exception as ex:
            return {"ok": False, "error": f"could not schedule relaunch: {ex}"}

    def bye():
        import time
        time.sleep(0.3 if pending else max(0.3, delay))
        logs.LOG.info("restarting - %s",
                      "swap will relaunch" if pending else "relaunch after exit")
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


# --------------------------------------------------------------------------
# Self-updating without an auto-updater.
#
# The update channel is a person: you post ACECM.exe to Patreon, someone
# downloads it and runs it. If it finds an OLDER installed copy it offers to
# replace that copy and relaunch it from the install folder; then this
# download exits. No polling, no manifest, no server.
#
# ⚠ This NEVER fires for the installed copy itself. pending_update() short-
# circuits on running_installed, so the app you launch from the Start Menu
# has no update logic in it at all - only a file run from OUTSIDE the install
# folder (i.e. a fresh download) can offer anything.
# --------------------------------------------------------------------------

def pending_update():
    """What a just-launched exe should offer, if anything.

    offer          -> replace an older installed copy (the update case)
    offer_install  -> no install yet; put this one in place (first run)
    Both are False for the installed copy, for a downgrade, and for a
    same-version re-download - so the common cases stay silent.
    """
    st = status()
    installed_v = installed_version()
    frozen = st["frozen"]
    # the guard the whole feature rests on: the installed copy is never asked
    running_installed = st["running_installed"]
    offer = bool(frozen and st["installed"] and not running_installed
                 and _older(installed_v, version.VERSION))
    offer_install = bool(frozen and not st["installed"] and not running_installed)
    return {
        "offer": offer,
        "offer_install": offer_install,
        "installed": st["installed"],
        "installed_version": installed_v,
        "this_version": version.VERSION,
        "install_dir": install_dir(),
        "install_exe": st["installed_exe"],
        "running_installed": running_installed,
    }


def _exe_locked(path):
    """True while `path` is a running image Windows will not let us overwrite.

    A running exe is opened FILE_SHARE_READ only, so asking for write access
    fails - which is exactly the condition os.replace would hit."""
    if not os.path.isfile(path):
        return False
    try:
        with open(path, "r+b"):
            return False
    except OSError:
        return True


def running_installed_pids():
    """PIDs actually running the INSTALLED exe - not some other ACECM."""
    try:
        from . import winproc
    except Exception:
        return []
    dst = os.path.normcase(os.path.abspath(installed_exe()))
    out = []
    for pid in winproc.pids_named(APP + ".exe"):
        try:
            got = winproc.exe_path(pid) or ""
            if got and os.path.normcase(os.path.abspath(got)) == dst:
                out.append(int(pid))
        except Exception:
            continue
    return out


def helper_pids():
    """Background helpers running from the INSTALLED exe (`ACECM.exe --tool
    ...`): the lobby proxy, telemetry trackers, render jobs.

    ⚠ Each one holds the exe image open exactly like the main window does,
    so any of them blocks an update. The lobby proxy exits with its parent
    window (verified 1.0.18 -> 1.0.19), but a telemetry tracker had no exit
    condition at all: one outlived its server by a day, and the update
    refused with "the installed ACECM is still running (process 56328)"
    while no ACECM window was open.
    """
    try:
        from . import winproc
    except Exception:
        return []
    out = []
    for pid in running_installed_pids():
        cmd = winproc.cmdline(pid) or ""
        if " --tool " in f" {cmd} ":
            out.append(pid)
    return out


def _stop_helpers(reason):
    """Stop our own --tool helpers so the installed exe unlocks."""
    from . import winproc
    killed = []
    for pid in helper_pids():
        try:
            if winproc.kill(pid):
                killed.append(pid)
        except Exception:
            pass
    if killed:
        logs.LOG.info("update: stopped %d leftover helper(s) %s (%s)",
                      len(killed), killed, reason)
    return killed


def _ask_installed_to_quit(any_acecm=False):
    """Ask a running copy to exit.

    ⚠ TWO different intents, and they must not share a rule:
      * the UPDATER (default) may only ever quit the INSTALLED copy - quitting
        something else achieves nothing and can kill an unrelated instance;
      * STARTUP TAKEOVER (any_acecm=True) just wants the PORT back from a
        leftover windowless ACECM, whichever copy that happens to be.
    Applying the updater's identity check to the takeover path stopped ACECM
    starting at all whenever any other copy held the port - the app reported
    "already running without a window and did not exit when asked" and there
    was no way through but killing the other process by hand.

    ⚠ For the updater, check WHO is answering first. The port is not proof of
    identity: a copy run from a source tree during development owned it,
    obediently quit when asked, and the actual installed copy kept its lock -
    so the update failed with "still running" having just killed something
    else entirely.
    """
    import urllib.request
    port = config.CFG.get("ui_port")
    if not port:
        return False
    mine = set(running_installed_pids())
    try:
        from . import winproc
        owners = set(winproc.tcp_listen_pids(int(port)) or [])
    except Exception:
        owners = set()
    if not any_acecm and owners and mine and not (owners & mine):
        logs.LOG.info("update: port %s is held by pid(s) %s, which are not the "
                      "installed copy %s - not sending quit there",
                      port, sorted(owners), sorted(mine))
        return False
    try:
        req = urllib.request.Request(
            f"http://127.0.0.1:{port}/api/app/quit", data=b"{}",
            headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=5) as r:
            return bool((__import__("json").loads(r.read()) or {}).get("ok"))
    except Exception as ex:
        logs.LOG.info("quit request to running install: %s", ex)
        return False


def _wait_unlocked(path, timeout=20.0):
    import time
    end = time.time() + timeout
    while time.time() < end:
        if not _exe_locked(path):
            return True
        time.sleep(0.4)
    return not _exe_locked(path)


def _spawn(exe, args):
    flags = 0
    if os.name == "nt":
        # DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP: the new app must outlive
        # this exiting updater and not share its (dying) console.
        flags = 0x00000008 | 0x00000200
    # A onefile build unpacks itself into %TEMP%\_MEInnnnnn and tells its
    # children where that is via _MEIPASS2. A frozen child that sees it does
    # NOT unpack its own copy - it runs out of ours. So the app we are
    # launching here held our extraction folder open, and this exiting
    # updater could not delete it:
    #   Failed to remove temporary directory: ...\Temp\_MEI270842
    # a message box, on every single update. child_env() drops those
    # breadcrumbs so the new copy unpacks its own directory and owns it.
    from . import winproc
    subprocess.Popen([exe] + list(args), close_fds=True,
                     creationflags=flags, env=winproc.child_env(),
                     cwd=os.path.dirname(exe) or None)


def apply_update(relaunch=True):
    """Replace the installed copy with THIS exe, then relaunch it.

    Sequence: if the installed copy is running, ask it to quit and wait for
    its exe to unlock; copy ourselves into place (force, because the marker
    may equal or trail this build); relaunch the installed copy and let the
    caller exit. The ACECM data folder is never touched.
    """
    # ⚠ Set logging up HERE. The updater replaces the exe and exits long
    # before serve() would have called logs.setup(), so without this the whole
    # swap - the one operation that overwrites a user's install - happened with
    # nothing written down anywhere.
    try:
        logs.setup()
    except Exception:
        pass
    st = status()
    if not st["frozen"]:
        return {"ok": False, "error": st.get("note") or "not a frozen build"}
    dst = installed_exe()
    logs.LOG.info("self-update: v%s -> v%s  (this exe: %s)",
                  installed_version() or "?", version.VERSION, running_exe())
    if _exe_locked(dst):
        # the window first (so it cannot respawn its proxy), then whatever
        # helpers are still holding the exe - its own, or orphans
        _ask_installed_to_quit()
        if not _wait_unlocked(dst, timeout=8.0):
            _stop_helpers("installed exe still locked after quit")
        if not _wait_unlocked(dst):
            pids = running_installed_pids()
            who = (" (process " + ", ".join(str(p) for p in pids) + ")"
                   if pids else "")
            return {"ok": False, "running": True, "pids": pids,
                    "error": f"the installed ACECM is still running{who} - "
                             "close that window, then run this update again"}
    r = install(desktop=st.get("desktop", True), force=True)
    if not r.get("ok"):
        return r
    if relaunch:
        try:
            _spawn(dst, ["--relaunch"])
        except Exception as ex:
            return {"ok": False,
                    "error": f"installed v{version.VERSION} but could not "
                             f"relaunch it: {ex}. Start it from the Start Menu.",
                    "exe": dst}
    return {"ok": True, "exe": dst, "version": version.VERSION,
            "replaced": r.get("replaced"), "relaunched": relaunch}


def _messagebox(text, title, yesno=True):
    """A native yes/no (or OK) box, so a windowless download can still ask.

    Falls back to the console when there is no user32 (non-Windows, or a
    genuinely headless box), and to False if even that is unavailable."""
    if os.name == "nt":
        try:
            import ctypes
            MB_YESNO, MB_OK = 0x4, 0x0
            MB_ICONQUESTION = 0x20
            MB_SETFOREGROUND, MB_TOPMOST = 0x10000, 0x40000
            style = (MB_YESNO if yesno else MB_OK) | MB_ICONQUESTION \
                | MB_SETFOREGROUND | MB_TOPMOST
            return ctypes.windll.user32.MessageBoxW(0, text, title, style) == 6
        except Exception:
            pass
    if not yesno:
        print(f"{title}: {text}")
        return True
    try:
        return input(f"{title}\n{text}\n[y/N] ").strip().lower().startswith("y")
    except Exception:
        return False


def prompt_update(pu):
    return _messagebox(
        "A newer ACECM is ready to install.\n\n"
        f"Installed:  v{pu['installed_version']}\n"
        f"This file:   v{pu['this_version']}\n\n"
        "Replace the installed copy and restart it now?",
        "ACECM update", yesno=True)


def prompt_install(pu):
    return _messagebox(
        f"ACECM v{pu['this_version']} is not installed yet.\n\n"
        f"Install it to\n{pu['install_dir']}\nand add Start Menu / Desktop "
        "shortcuts?",
        "Install ACECM", yesno=True)


def warn(msg):
    _messagebox(str(msg), "ACECM", yesno=False)


def quit_now(delay=0.4):
    """Exit this process so its exe unlocks. No relaunch - the updater that
    asked will start the replacement copy itself."""
    def bye():
        import time
        time.sleep(delay)
        # Take our helpers down with us. os._exit does not stop children;
        # the proxy happens to exit with us, but anything that does not
        # would keep the exe open and fail the update that asked us to quit.
        try:
            from . import backend, telemetry
            stops = (backend.stop, telemetry.stop)
        except Exception:
            stops = ()
        for stop in stops:
            try:
                stop()
            except Exception:
                pass
        try:
            from . import ui
            ui.destroy()
        except Exception:
            pass
        time.sleep(1.5)
        os._exit(0)                       # noqa: SLF001
    import threading
    threading.Thread(target=bye, daemon=True).start()
    return {"ok": True, "quitting": True}


# ------------------------------------------------------- temp housekeeping --

# A file that exists in OUR bundle and is vanishingly unlikely to exist in
# another PyInstaller app's. Never delete a _MEI directory without it: other
# frozen programs use the same %TEMP%\_MEInnnnnn naming, and one of them may
# be running right now.
# ⚠ Two markers: the UI was one web/app.js until 1.0.18 and is web/js/ since.
# With only the new one, every leftover folder from an OLDER build would
# never be recognised as ours again - and never cleaned.
_OURS_MARKERS = (os.path.join("acecm", "web", "app.js"),
                 os.path.join("acecm", "web", "js", "15-nav.js"))


def stale_bundles(older_than=1800.0):
    """Leftover onefile extraction folders that are definitely ours.

    Every build before 1.2.0 leaked one of these per update: the relaunched
    copy inherited _MEIPASS2 and ran out of the *old* process's folder, so the
    old process could not delete it and said so in a message box. One user had
    139 directories and 4.7 GB of them.
    """
    import tempfile
    import time
    live = ""
    if getattr(sys, "frozen", False):
        live = os.path.normcase(getattr(sys, "_MEIPASS", "") or "")
    root = tempfile.gettempdir()
    out = []
    try:
        names = os.listdir(root)
    except OSError:
        return out
    now = time.time()
    for name in names:
        if not name.startswith("_MEI"):
            continue
        d = os.path.join(root, name)
        if not os.path.isdir(d) or os.path.normcase(d) == live:
            continue
        if not any(os.path.isfile(os.path.join(d, m)) for m in _OURS_MARKERS):
            continue
        try:
            # ⚠ mtime, so a copy that is merely OLD but still running is not a
            # candidate the moment it is unpacked. A running app touches
            # nothing, so pair this with the delete-failure check below - the
            # real guard is that an in-use file cannot be removed at all.
            if now - os.path.getmtime(d) < older_than:
                continue
        except OSError:
            continue
        out.append(d)
    return out


def sweep_bundles(older_than=1800.0):
    """Delete what stale_bundles() found. Never raises.

    Deletion IS the lock test: a directory another live copy is running from
    refuses to go, and it is simply left behind.
    """
    import shutil
    freed, gone, kept = 0, 0, 0
    for d in stale_bundles(older_than):
        size = 0
        try:
            for base, _dirs, files in os.walk(d):
                for f in files:
                    try:
                        size += os.path.getsize(os.path.join(base, f))
                    except OSError:
                        pass
        except OSError:
            pass
        # ⚠ RENAME FIRST, and only delete what the rename won. rmtree on a
        # live bundle is not safely reversible: it deletes the unlocked files
        # one by one and only fails when it reaches a loaded DLL, which would
        # gut a running copy of ACECM instead of skipping it. Renaming a
        # directory is atomic and Windows refuses it outright while any file
        # inside is open - so the rename IS the in-use test, and a copy that
        # has merely been running a long time is left completely untouched.
        tmp = d + ".stale"
        try:
            if os.path.exists(tmp):
                shutil.rmtree(tmp, ignore_errors=True)
            os.rename(d, tmp)
        except OSError:
            kept += 1
            continue
        try:
            shutil.rmtree(tmp)
            freed += size
            gone += 1
        except OSError:
            kept += 1
    if gone:
        logs.LOG.info("removed %d stale bundle folder(s), %.1f MB freed "
                      "(%d still in use)", gone, freed / 1e6, kept)
    return {"removed": gone, "bytes": freed, "in_use": kept}
