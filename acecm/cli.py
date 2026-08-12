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
    path = config.tool_script(name + ".py")
    if not os.path.isfile(path):
        print(f"tool not found: {path}")
        return 3
    tool_dir = os.path.dirname(path)
    if tool_dir not in sys.path:
        sys.path.insert(0, tool_dir)
    runpy.run_path(path, run_name="__main__")
    return 0


def main():
    if len(sys.argv) > 2 and sys.argv[1] == "--tool":
        raise SystemExit(_run_tool(sys.argv[2], sys.argv[3:]))
    # A native window is the default, but this is also a server tool - it has
    # to be runnable on a box with no desktop at all.
    mode = "window"
    if "--browser" in sys.argv:
        mode = "browser"
    elif "--headless" in sys.argv or "--no-ui" in sys.argv:
        mode = "headless"
    from .app import main as app_main
    try:
        app_main(mode)
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
