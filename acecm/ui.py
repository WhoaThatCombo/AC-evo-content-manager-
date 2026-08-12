"""Native desktop window, so ACECM is an app rather than a browser tab.

The UI is already a local web app, and rewriting it in a widget toolkit would
throw all of that away for no gain. Instead it is hosted in the operating
system's own webview - Edge WebView2 on Windows, which ships with Windows 11,
so nothing extra has to be installed or bundled.

⚠ A GUI event loop must own the MAIN thread. The HTTP server therefore runs on
a background thread (see app.serve) and this blocks until the window closes.

⚠ Closing the window stops ACECM, but NOT the dedicated servers or trackers it
launched - those are separate processes on purpose, so a server keeps running
when you close the manager. Stop them from the Servers tab if that is not what
you want.
"""
import sys


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
    # private_mode=False keeps a persistent profile, so the webview does not
    # re-download and re-parse everything on every launch.
    webview.start(private_mode=False)
