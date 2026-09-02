"""A native WebKitGTK window for ACECM, hosted by the SYSTEM python.

⚠ This exists because a frozen ACECM cannot use the desktop's own webview.
A one-file build carries its own interpreter, and the distro's PyGObject is
compiled for the distro's Python — importing it into ours is an ABI mismatch,
not a missing dependency, so no amount of installing fixes it from inside.

Running it the other way round does work: ACECM keeps serving HTTP and spawns
the system python, which already has `gi` and WebKit2, to draw the window. No
pip, no virtualenv, no network, nothing for the user to install.

Deliberately dependency-free beyond PyGObject: it must run under whatever
python3 the distro ships. Usage:

    python3 acecm_window.py <url> [title]
"""
import sys

import gi

gi.require_version("Gtk", "3.0")
for _v in ("4.1", "4.0"):
    try:
        gi.require_version("WebKit2", _v)
        break
    except ValueError:
        continue
from gi.repository import Gtk, WebKit2  # noqa: E402


def main(argv):
    if len(argv) < 2:
        print("usage: acecm_window.py <url> [title]", file=sys.stderr)
        return 2
    url = argv[1]
    title = argv[2] if len(argv) > 2 else "Assetto Corsa EVO Content Manager"

    win = Gtk.Window(title=title)
    win.set_default_size(1360, 900)
    win.set_size_request(1024, 680)

    view = WebKit2.WebView()
    # ⚠ The UI is a local app that talks to its own HTTP server, so it needs
    # scripting and local storage. It is not browsing the web; the only origin
    # it ever loads is 127.0.0.1.
    settings = view.get_settings()
    settings.set_property("enable-developer-extras", True)
    settings.set_property("enable-write-console-messages-to-stdout", True)
    view.load_uri(url)
    win.add(view)

    # ⚠ Closing the window must end the PROCESS, not just hide it. ACECM
    # waits on this child to know the user is done, and stops the lobby proxy
    # when it exits — a window that merely hid would leave ACECM running with
    # nothing on screen and no way back to it.
    win.connect("destroy", Gtk.main_quit)
    win.show_all()
    Gtk.main()
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
