#!/usr/bin/env bash
# Set ACECM up on Linux. Safe to re-run.
#
# ⚠ --system-site-packages is deliberate and load-bearing. The best window
# backend, WebKitGTK, can only come from the distro's own python3-gobject: a
# sealed venv cannot see it, and pip cannot supply it. Inheriting system
# packages is what lets a native window work without layering anything.
set -euo pipefail

cd "$(dirname "$(readlink -f "$0")")"
VENV="${ACECM_VENV:-.venv}"

echo "==> creating $VENV"
python3 -m venv --system-site-packages "$VENV"
"$VENV/bin/python" -m pip install --quiet --upgrade pip
echo "==> installing dependencies"
"$VENV/bin/python" -m pip install --quiet -r requirements-linux.txt

echo
echo "==> checking the window backend"
"$VENV/bin/python" - <<'PY'
import sys
sys.path.insert(0, ".")
from acecm import ui

gui = ui._linux_backend()
app = ui._app_window_argv("about:blank", "probe")
if gui:
    print(f"  native embedded window : YES ({gui})")
elif app:
    print(f"  native embedded window : no")
    print(f"  browser app window     : YES ({app[0]})")
    print("    A real window with no tabs or address bar. For a fully")
    print("    embedded webview instead, install WebKitGTK:")
    print("      rpm-ostree install python3-gobject webkit2gtk4.1  (reboot)")
    print("    or uncomment the PySide6 lines in requirements-linux.txt.")
else:
    print("  native embedded window : no")
    print("  browser app window     : no")
    print("    ACECM will open in your default browser. To get a window:")
    print("      rpm-ostree install python3-gobject webkit2gtk4.1  (reboot)")
    print("    or uncomment the PySide6 lines in requirements-linux.txt and")
    print("    re-run this script.")

try:
    import evdev  # noqa: F401
    print("  pit button / hotkeys   : available")
except ImportError:
    print("  pit button / hotkeys   : evdev missing")
PY

echo
echo "==> done. Start ACECM with:"
echo "     $VENV/bin/python -m acecm"
