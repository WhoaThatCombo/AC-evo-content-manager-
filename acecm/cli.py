"""Entry point. One executable, several jobs.

⚠ A frozen build has no Python interpreter to shell out to - `sys.executable`
IS this exe. So the helper scripts cannot be launched as `python script.py`;
instead the exe re-invokes ITSELF with `--tool <name>` and runs that tool's
module in the child. Same binary, different job, no interpreter to ship.

    ACECM.exe                          the content manager, native window
    ACECM.exe --browser                same, but open in the default browser
    ACECM.exe --headless               serve the API only (for a headless box)
    ACECM.exe --tool server_telemetry  a telemetry tracker for one server
    ACECM.exe --tool start_vai_server  launch a dedicated server
    ACECM.exe --tool acevo_proxy       the lobby proxy backend
"""
import os
import sys

# Tools that may be run as a child process. Anything not listed cannot be
# invoked this way, so a bad --tool value fails loudly instead of importing
# something arbitrary.
TOOLS = ("server_telemetry", "start_vai_server", "acevo_proxy",
         "acevo_backend", "server_track_inject", "build_track_package")


def _run_tool(name, argv):
    if name not in TOOLS:
        print(f"unknown tool {name!r}; expected one of {', '.join(TOOLS)}")
        return 2
    sys.argv = [name] + argv
    import runpy
    from . import config

    # The injector is called as a function, not as a script - it takes the
    # archive and the package folder rather than parsing its own arguments.
    if name == "server_track_inject" and argv[:1] == ["--install"]:
        d = os.path.dirname(config.tool_script("server_track_inject.py"))
        if d not in sys.path:
            sys.path.insert(0, d)
        import server_track_inject as sti
        sti.install_package_to_server(argv[1], argv[2])
        return 0
    # Frozen or not, the tool scripts live somewhere findable; run them as
    # __main__ so their `if __name__ == "__main__"` blocks fire.
    # tool_script already resolves .py (dev) or .pyc (hardened release).
    path = config.tool_script(name + ".py")
    if not os.path.isfile(path):
        print(f"tool not found: {path}")
        return 3
    tool_dir = os.path.dirname(path)
    if tool_dir not in sys.path:
        sys.path.insert(0, tool_dir)
    runpy.run_path(path, run_name="__main__")
    return 0


def _take_opt(name):
    """Pull `--name value` out of argv so the rest of startup never sees it."""
    if name not in sys.argv:
        return None
    i = sys.argv.index(name)
    val = None
    if i + 1 < len(sys.argv) and not sys.argv[i + 1].startswith("-"):
        val = sys.argv[i + 1]
        del sys.argv[i:i + 2]
    else:
        del sys.argv[i]
    return val


# Modes that exist to be READ in a terminal. Everything else is the desktop
# app, which must never show a console window - see winproc.ensure_console.
_CONSOLE_ARGS = ("--tool", "--headless", "--no-ui", "--install", "--uninstall",
                 "--help", "-h", "--version", "--update-check")


def _console_bootstrap():
    """Decide whether this run gets a console, before anything prints.

    ⚠ FIRST thing in main(). The shipped exe is a GUI-subsystem binary with no
    stdout, so a bare print() anywhere before this raises AttributeError - and
    a --console build instead shows a black window behind the app for the whole
    startup, which is the thing this exists to stop.
    """
    try:
        from . import winproc
    except Exception:
        return
    wants = any(a in _CONSOLE_ARGS for a in sys.argv[1:])
    # a --tool child is a background process: attach to a terminal if one is
    # already there, but never conjure a window for it.
    tool = len(sys.argv) > 1 and sys.argv[1] == "--tool"
    try:
        if wants:
            winproc.ensure_console(alloc=not tool)
        winproc.safe_stdio()
    except Exception:
        pass


def main():
    _console_bootstrap()
    if len(sys.argv) > 2 and sys.argv[1] == "--tool":
        raise SystemExit(_run_tool(sys.argv[2], sys.argv[3:]))
    # Written by the update swap script. The new exe must create this
    # file once it is serving, or the previous build is restored.
    _okflag = _take_opt("--okflag")
    # ⚠ A scheduled relaunch (Restart, or the update swap) is handing the port
    # over from a copy that is still exiting - see app.serve.
    _relaunch = "--relaunch" in sys.argv
    if _relaunch:
        sys.argv.remove("--relaunch")
    _take_opt("--updated")

    # Put a downloaded exe somewhere permanent, with a shortcut, so it never
    # has to be found again. Runs and exits - it is a setup step, not the app.
    if "--install" in sys.argv or "--uninstall" in sys.argv:
        from . import installer
        if "--uninstall" in sys.argv:
            r = installer.uninstall(remove_exe="--purge" in sys.argv)
            for f in r.get("removed", []):
                print(f"removed {f}")
            print(f"your settings and profiles are untouched: {r['data_kept']}")
            raise SystemExit(0 if r.get("ok") else 1)
        r = installer.install(desktop="--no-desktop" not in sys.argv)
        if not r.get("ok"):
            print(f"install failed: {r.get('error')}")
            raise SystemExit(1)
        print(f"installed to {r['exe']}")
        for s in r.get("shortcuts", []):
            print(f"  shortcut: {s}")
        print("launch it from the Start Menu from now on")
        raise SystemExit(0)
    # A support/diagnostic switch: print what the updater decides and exit,
    # without showing the dialog. "it just boots and never offers" is
    # otherwise invisible from outside the exe.
    if "--update-check" in sys.argv:
        from . import installer
        import json as _json
        try:
            print(_json.dumps(installer.pending_update(), indent=2))
        except Exception as ex:
            print(f"pending_update() raised: {type(ex).__name__}: {ex}")
            import traceback
            traceback.print_exc()
            raise SystemExit(2)
        raise SystemExit(0)

    # ----------------------------------------------------------------------
    # Self-update on launch. A build run from OUTSIDE the install folder (i.e.
    # a fresh download posted to Patreon) offers to replace an older installed
    # copy, then relaunches it and exits. The installed copy never reaches
    # this - installer.pending_update() short-circuits on running_installed,
    # so there is no updater inside the app you launch normally.
    #
    # --no-update-prompt skips it (used by the swap relaunch, and handy for
    # testing). Any failure here must fall through to a normal boot, never
    # strand the user without an app.
    # ----------------------------------------------------------------------
    from . import config as _config
    if _config.FROZEN and "--no-update-prompt" not in sys.argv and not _relaunch:
        # ⚠ Say what was decided, and why. This branch used to swallow every
        # exception into `pu = None` and run before logs.setup(), so a launch
        # that quietly declined to offer an update left NO evidence anywhere -
        # which is exactly what happened: the download just booted in place
        # and the install was untouched, with nothing to explain it.
        try:
            from . import logs as _logs
            _logs.setup()
        except Exception:
            _logs = None
        pu = None
        try:
            from . import installer
            pu = installer.pending_update()
            if _logs:
                _logs.LOG.info(
                    "update check: this=%s installed=%s running_installed=%s "
                    "installed_flag=%s -> offer=%s offer_install=%s",
                    pu.get("this_version"), pu.get("installed_version"),
                    pu.get("running_installed"), pu.get("installed"),
                    pu.get("offer"), pu.get("offer_install"))
        except Exception as ex:
            if _logs:
                _logs.exception("update check", ex)
        if pu and (pu.get("offer") or pu.get("offer_install")):
            update = pu.get("offer")
            want = (installer.prompt_update(pu) if update
                    else installer.prompt_install(pu))
            if _logs:
                _logs.LOG.info("update prompt answered: %s", want)
            if want:
                r = installer.apply_update(relaunch=update)
                if r.get("ok"):
                    # replaced (and, for an update, relaunched) the installed
                    # copy - this download's job is done
                    if not update:
                        print(f"installed to {r['exe']}")
                    raise SystemExit(0)
                # the swap failed (most often: install still running and would
                # not close). Say why, then boot this copy in place so the user
                # is not left with nothing.
                if _logs:
                    _logs.LOG.warning("update failed: %s", r.get("error"))
                installer.warn(r.get("error") or "update failed")

    # A native window is the default, but this is also a server tool - it has
    # to be runnable on a box with no desktop at all.
    mode = "window"
    if "--browser" in sys.argv:
        mode = "browser"
    elif "--headless" in sys.argv or "--no-ui" in sys.argv:
        mode = "headless"
    from .app import main as app_main
    try:
        app_main(mode, okflag=_okflag, relaunch=_relaunch)
    except SystemExit:
        raise
    except BaseException as ex:
        # Anything fatal gets written down before the window disappears - a
        # packaged app has no console to read the traceback from.
        try:
            from . import logs
            logs.setup()
            logs.exception("startup", ex)
            print(f"FATAL: {type(ex).__name__}: {ex}")
            print(f"full traceback in {logs.log_dir()}")
        finally:
            raise
