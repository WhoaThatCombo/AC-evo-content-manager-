"""Native desktop window, so ACECM is an app rather than a browser tab.

The UI is already a local web app, and rewriting it in a widget toolkit would
throw all of that away for no gain. Instead it is hosted in the operating
system's own webview - Edge WebView2 on Windows, which ships with Windows 11,
so nothing extra has to be installed or bundled.

On Linux there is no equivalent guarantee. The system webview is WebKitGTK
(or Qt WebEngine) and a given machine may have neither, plus the session may
be headless or remote. `available()` therefore PROVES a backend before we
commit to a window, and `app.serve` degrades to the browser rather than
exiting - the UI is a local web app, so a browser tab loses nothing but the
frame around it.

⚠ A GUI event loop must own the MAIN thread. The HTTP server therefore runs on
a background thread (see app.serve) and this blocks until the window closes.

⚠ Closing the window stops ACECM. Dedicated servers and trackers stay up on
purpose. The lobby proxy is different: if Settings → auto_proxy is on (the
default), the proxy starts with ACECM and is killed here when the window
closes.
"""
import os
import shutil
import sys


def _storage_path():
    """Per-build WebView2 profile under ACECM's own data dir.

    private_mode=False keeps cookies and session, but the default profile
    lives in %APPDATA%\\pywebview and is shared across every ACECM.exe that
    has ever run on this machine. After an in-app update the window then
    paints yesterday's JS/CSS from Chromium's disk + V8 code cache, which
    reads as a dead UI even though the new exe is serving the new files.
    """
    from . import config, version
    root = os.path.join(config.DATA, "webview")
    marker = os.path.join(root, "ace_version")
    try:
        have = open(marker, encoding="utf-8").read().strip()
    except OSError:
        have = ""
    if have != version.VERSION:
        if os.path.isdir(root):
            shutil.rmtree(root, ignore_errors=True)
        for leftover in _default_profiles():
            if leftover and os.path.isdir(leftover):
                shutil.rmtree(leftover, ignore_errors=True)
        os.makedirs(root, exist_ok=True)
        try:
            open(marker, "w", encoding="utf-8").write(version.VERSION)
        except OSError:
            pass
    os.makedirs(root, exist_ok=True)
    return root


def _default_profiles():
    """pywebview's own shared profile dirs, which we replace with our own."""
    if sys.platform == "win32":
        return [os.path.join(os.environ.get("APPDATA") or "", "pywebview")]
    home = os.path.expanduser("~")
    return [
        os.path.join(os.environ.get("XDG_DATA_HOME")
                     or os.path.join(home, ".local", "share"), "pywebview"),
        os.path.join(os.environ.get("XDG_CONFIG_HOME")
                     or os.path.join(home, ".config"), "pywebview"),
    ]


def _linux_backend():
    """Name of a usable pywebview GUI backend, or "".

    ⚠ Importing `webview` is not evidence of anything here. The package
    installs cleanly with no GUI bindings at all and only fails when
    `webview.start()` runs — on the MAIN thread, after the HTTP server is up
    and the browser fallback has already been passed by. Probe the bindings
    now, while there is still a choice to make.
    """
    if not (os.environ.get("DISPLAY") or os.environ.get("WAYLAND_DISPLAY")):
        return ""            # headless or a session we cannot draw into
    try:
        import gi
        gi.require_version("Gtk", "3.0")
        for ver in ("4.1", "4.0"):
            try:
                gi.require_version("WebKit2", ver)
                from gi.repository import WebKit2   # noqa: F401
                return "gtk"
            except (ValueError, ImportError):
                continue
    except (ImportError, ValueError):
        pass
    for mod in ("PyQt6.QtWebEngineWidgets", "PySide6.QtWebEngineWidgets",
                "PyQt5.QtWebEngineWidgets"):
        try:
            __import__(mod)
            return "qt"
        except ImportError:
            continue
    return ""


# Pythons that might carry the distro's PyGObject. Ordered: whatever is on
# PATH first, then explicit versioned names for a distro whose `python3` is
# something else.
_SYSTEM_PYTHONS = ("/usr/bin/python3", "python3", "/usr/bin/python3.14",
                   "/usr/bin/python3.13", "/usr/bin/python3.12",
                   "/usr/bin/python3.11")

_PROBE = (
    "import gi;gi.require_version('Gtk','3.0');"
    "ok=0\n"
    "for v in ('4.1','4.0'):\n"
    "    try:\n"
    "        gi.require_version('WebKit2',v);ok=1;break\n"
    "    except ValueError: pass\n"
    "raise SystemExit(0 if ok else 1)"
)


def system_gtk_python():
    """A system python that can host a WebKitGTK window, or "".

    ⚠ Probed by RUNNING it, never by importing. The whole point is that this
    interpreter is not ours — a frozen ACECM carries its own Python and the
    distro's `gi` is built for a different one, so `import gi` here would fail
    even on a machine that has everything. Asking the other interpreter is the
    only honest test.
    """
    import shutil
    import subprocess
    if sys.platform == "win32":
        return ""
    if not (os.environ.get("DISPLAY") or os.environ.get("WAYLAND_DISPLAY")):
        return ""
    seen = set()
    for name in _SYSTEM_PYTHONS:
        exe = name if os.path.isabs(name) else (shutil.which(name) or "")
        if not exe or exe in seen or not os.path.exists(exe):
            continue
        seen.add(exe)
        try:
            r = subprocess.run([exe, "-c", _PROBE], capture_output=True,
                               timeout=20)
        except (OSError, subprocess.SubprocessError):
            continue
        if r.returncode == 0:
            return exe
    return ""


def _run_system_gtk(url, title):
    """Host the window in the system python. Blocks until it closes."""
    import subprocess
    global APP_PROC
    exe = system_gtk_python()
    if not exe:
        return False
    from . import config, logs, winproc
    script = config.tool_script("acecm_window.py")
    if not script or not os.path.isfile(script):
        logs.LOG.warning("acecm_window.py is missing from this build")
        return False
    logs.LOG.info("native window via %s", exe)
    # ⚠ A clean environment. A frozen build exports its own library paths and
    # the system python must not inherit them, or it loads OUR bundled
    # libssl/libstdc++ instead of the distro's and dies before drawing.
    env = winproc.child_env()
    env.update(_webkit_env())
    # ⚠ Keep the window's own output. It is a separate process, so a crash in
    # WebKit (or a missing typelib) otherwise leaves nothing behind at all -
    # the window just vanishes and ACECM exits, which is indistinguishable
    # from the user closing it.
    wlog = os.path.join(config.DATA, "window.log")
    try:
        os.makedirs(config.DATA, exist_ok=True)
        sink = open(wlog, "w", encoding="utf-8", errors="replace")
    except OSError:
        sink = None
    APP_PROC = subprocess.Popen([exe, script, url, title], env=env,
                                stdout=sink or None,
                                stderr=subprocess.STDOUT if sink else None)
    try:
        APP_PROC.wait()
    except KeyboardInterrupt:
        pass
    finally:
        proc, APP_PROC = APP_PROC, None
        if proc and proc.poll() is None:
            proc.terminate()
        if sink:
            try:
                sink.close()
            except OSError:
                pass
    rc = proc.returncode if proc else 0
    if rc:
        tail = ""
        try:
            with open(wlog, encoding="utf-8", errors="replace") as fh:
                tail = " | ".join(fh.read().strip().splitlines()[-4:])
        except OSError:
            pass
        # ⚠ A non-zero exit is a CRASH, not the user closing the window, and
        # the caller must be told so it can fall back instead of shutting
        # ACECM down as if the session were over.
        logs.LOG.warning("the native window exited with code %s: %s", rc,
                         tail or f"see {wlog}")
        return False
    return True


def _webkit_env():
    """Environment tweaks WebKitGTK needs on some systems.

    ⚠ The DMABUF renderer is the one that matters. On NVIDIA — proprietary
    driver, Wayland, or both — WebKitGTK's DMABUF path commonly fails and
    takes the web process down with it a second or two after the page loads,
    which looks exactly like the window closing itself. Disabling it costs a
    little compositing performance in a UI that is mostly static text, and
    buys a window that stays on screen.
    """
    env = {}
    if sys.platform == "win32":
        return env
    env.setdefault("WEBKIT_DISABLE_DMABUF_RENDERER", "1")
    # ⚠ Do NOT also disable compositing mode. It was in here as a second
    # belt-and-braces workaround and it is not free: without a compositing
    # layer WebKit cannot render the things that need one, so overlays and
    # menus lose their backdrop and the page behind a dropdown comes out
    # washed out and over-bright while the dropdown itself looks fine.
    # DMABUF is the setting that stops the crash; this one only broke the UI.
    return env


# Chromium-family browsers can open a frameless, tab-less window that looks
# and behaves like an application window. It is not an embedded webview — we
# do not own the process — but it is a real window rather than a tab, and it
# needs nothing installed beyond a browser the user already has.
#
# ⚠ Firefox and its forks are deliberately absent. `--kiosk` is fullscreen
# with no decorations and no way out but a keystroke, which is worse than a
# tab, and there is no Firefox equivalent of --app.
_APP_BROWSERS = (
    ("chromium", []), ("chromium-browser", []), ("google-chrome", []),
    ("google-chrome-stable", []), ("brave-browser", []), ("vivaldi", []),
    ("microsoft-edge", []),
)
_APP_FLATPAKS = (
    "org.chromium.Chromium", "com.google.Chrome", "com.brave.Browser",
    "com.microsoft.Edge", "com.vivaldi.Vivaldi",
)


def _app_window_argv(url, title):
    """argv for a browser app-mode window, or None.

    The profile is ACECM's own: sharing the user's default profile would put
    the app window in the same browser session they are using, so quitting
    the browser would close ACECM and vice versa.
    """
    import shutil
    profile = os.path.join(_storage_path(), "appwindow")
    flags = [f"--app={url}", f"--user-data-dir={profile}",
             "--window-size=1360,900", "--no-first-run",
             "--no-default-browser-check",
             f"--class={title}"]
    for name, pre in _APP_BROWSERS:
        exe = shutil.which(name)
        if exe:
            return [exe] + pre + flags
    flatpak = shutil.which("flatpak")
    if flatpak:
        for app in _APP_FLATPAKS:
            try:
                import subprocess
                r = subprocess.run([flatpak, "info", app],
                                   capture_output=True, timeout=10)
                if r.returncode == 0:
                    # ⚠ The profile lives outside the sandbox, so it has to be
                    # granted explicitly or Chromium falls back to its own and
                    # the isolation above is silently lost.
                    return [flatpak, "run", f"--filesystem={profile}",
                            app] + flags
            except (OSError, subprocess.SubprocessError):
                continue
    return ""


def available():
    """Can we actually open a native window on this machine?"""
    try:
        import webview                                   # noqa: F401
    except Exception:
        return False
    if sys.platform != "win32":
        return (bool(_linux_backend()) or bool(system_gtk_python())
                or bool(_app_window_argv("about:blank", "probe")))
    # On Windows the binding is useless without the WebView2 runtime.
    try:
        import winreg
        for root, key in (
            (winreg.HKEY_LOCAL_MACHINE,
             r"SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients"
             r"\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"),
            (winreg.HKEY_CURRENT_USER,
             r"SOFTWARE\Microsoft\EdgeUpdate\Clients"
             r"\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"),
        ):
            try:
                with winreg.OpenKey(root, key) as k:
                    if winreg.QueryValueEx(k, "pv")[0]:
                        return True
            except OSError:
                continue
        return False
    except Exception:
        return False


WINDOW = None


def _raise_app_window():
    """Ask the window manager to raise the browser app window."""
    import shutil
    import subprocess
    from . import version
    title = f"Assetto Corsa EVO Content Manager  v{version.VERSION}"
    for argv in (["wmctrl", "-a", title],
                 ["xdotool", "search", "--name", title, "windowactivate"]):
        if not shutil.which(argv[0]):
            continue
        try:
            if subprocess.run(argv, capture_output=True,
                              timeout=10).returncode == 0:
                return True
        except (OSError, subprocess.SubprocessError):
            continue
    return False


def focus():
    """Bring this instance's window to the front.

    ⚠ Why a second process must NOT just open its own window: both would use
    the same WebView2 user-data folder, which is not shared-safe. The newcomer
    takes the profile, the FIRST instance's browser process dies, its window
    closes, and main() unwinds - so launching ACECM twice killed the copy that
    was already running. Asking the live instance to show itself is the only
    version of this that leaves one healthy app.
    """
    w = WINDOW
    if w is None:
        # A browser app window is somebody else's process, so there is no
        # handle to raise. Ask the desktop, and settle for False if it cannot
        # be done — the caller then opens a tab, which is better than nothing.
        if APP_PROC is not None and APP_PROC.poll() is None:
            return _raise_app_window()
        return False
    try:
        w.restore()
    except Exception:
        pass
    try:
        w.show()
        # on_top briefly, or the window rises behind whatever has focus
        w.on_top = True
        w.on_top = False
        return True
    except Exception:
        return False


APP_PROC = None


def _run_app_window(url, title):
    """Open a browser app-mode window and block until it closes.

    ⚠ Blocking is the contract `run` promises its caller: ACECM exits when the
    window closes, and the lobby proxy is stopped on the way out. Returning
    early here would look like the user closed the window immediately and
    shut ACECM down while the window was still on screen.
    """
    import subprocess
    global APP_PROC
    argv = _app_window_argv(url, title)
    if not argv:
        return False
    from . import logs
    logs.LOG.info("opening the UI as a browser app window: %s", argv[0])
    APP_PROC = subprocess.Popen(argv, env=_child_env())
    try:
        APP_PROC.wait()
    except KeyboardInterrupt:
        pass
    finally:
        proc, APP_PROC = APP_PROC, None
        if proc and proc.poll() is None:
            proc.terminate()
    return True


def _child_env():
    from . import winproc
    return winproc.child_env()


def run(url, title="Assetto Corsa EVO Content Manager"):
    """Open the window. Blocks until it is closed.

    Three routes, best first:

      1. An embedded system webview (WebView2 / WebKitGTK / Qt WebEngine).
         A real application window we own and can raise on request.
      2. A WebKitGTK window hosted by the SYSTEM python. Used by frozen
         builds, which cannot import the distro's PyGObject themselves.
      3. A Chromium-family browser in `--app` mode. Not embedded, but a
         frameless window with no tabs or address bar, on its own profile.
      4. Nothing — the caller falls back to a browser tab.

    ⚠ Route 2 exists because route 1 is not a given on Linux. A KDE machine
    may have no WebKitGTK at all, and telling somebody to layer packages onto
    an immutable OS and reboot, just to get a window, is not a fix.
    """
    from . import version
    global WINDOW
    name = f"{title}  v{version.VERSION}"
    gui = None if sys.platform == "win32" else _linux_backend()

    try:
        from . import winproc
        winproc.hide_console()
    except Exception:
        pass

    if sys.platform == "win32" or gui:
        import webview
        WINDOW = webview.create_window(name, url,
                              width=1360, height=900,
                              min_size=(1024, 680),
                              background_color="#0b0e13")
        # Persistent profile (so the window comes back quickly), but isolated
        # per ACECM version - see _storage_path.
        kw = {"private_mode": False, "storage_path": _storage_path()}
        if gui:
            # Name the backend rather than letting pywebview choose. Its
            # autodetect imports each toolkit in turn, and a half-installed
            # Qt earlier in that order wins over a working GTK.
            kw["gui"] = gui
        webview.start(**kw)
        return

    # A frozen build cannot import the distro's PyGObject, but it can ask the
    # system python to draw the window for it. Same result for the user: a
    # real application window, nothing to install.
    if _run_system_gtk(url, name):
        return
    if _run_app_window(url, name):
        return
    raise RuntimeError("no window backend available")
