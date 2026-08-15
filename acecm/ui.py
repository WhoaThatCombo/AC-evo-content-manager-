"""Native desktop window, so ACECM is an app rather than a browser tab.

The UI is already a local web app, and rewriting it in a widget toolkit would
throw all of that away for no gain. Instead it is hosted in the operating
system's own webview - Edge WebView2 on Windows, which ships with Windows 11,
so nothing extra has to be installed or bundled.

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
        leftover = os.path.join(os.environ.get("APPDATA") or "", "pywebview")
        if leftover and os.path.isdir(leftover):
            shutil.rmtree(leftover, ignore_errors=True)
        os.makedirs(root, exist_ok=True)
        try:
            open(marker, "w", encoding="utf-8").write(version.VERSION)
        except OSError:
            pass
    os.makedirs(root, exist_ok=True)
    return root


def available():
    """Can we actually open a native window on this machine?"""
    try:
        import webview                                   # noqa: F401
    except Exception:
        return False
    if sys.platform != "win32":
        return True
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


def run(url, title="Assetto Corsa EVO Content Manager"):
    """Open the window. Blocks until it is closed."""
    import webview
    from . import version
    webview.create_window(f"{title}  v{version.VERSION}", url,
                          width=1360, height=900,
                          min_size=(1024, 680),
                          background_color="#0b0e13")
    # Persistent profile (so the window comes back quickly), but isolated
    # per ACECM version - see _storage_path.
    webview.start(private_mode=False, storage_path=_storage_path())
