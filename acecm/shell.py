"""Hand a URL to the desktop, on either platform.

`os.startfile` does not exist outside Windows, and it is what ACECM uses both
to open its own UI in a browser and to hand Steam a `steam://` verb. The Linux
equivalent is `xdg-open`, with one wrinkle worth spelling out: `steam://` only
resolves if Steam registered its handler, and on a Flatpak or Snap install
that registration is often missing or points at a wrapper that is not running.
Falling back to the Steam client directly is what makes "Launch game" work on
a machine where the URL handler was never set up.
"""
import os
import shutil
import subprocess
import sys

from . import logs

IS_WINDOWS = sys.platform == "win32"

# Steam clients, in the order we prefer them: a native install first, then the
# sandboxed ones, which need their own runner.
_STEAM_RUNNERS = (
    ("steam", []),
    ("flatpak", ["run", "com.valvesoftware.Steam"]),
    ("snap", ["run", "steam"]),
)


def _xdg_open(url):
    exe = shutil.which("xdg-open")
    if not exe:
        return False
    try:
        # ⚠ Detached, and output discarded. xdg-open delegates to a desktop
        # helper that can outlive us and chatters on stderr; inheriting our
        # pipes has it write into ACECM's log for the rest of the session.
        subprocess.Popen([exe, url], start_new_session=True,
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        return True
    except OSError as ex:
        logs.LOG.info("xdg-open %s: %s", url, ex)
        return False


def _steam_client(url):
    """Give a steam:// verb to the Steam client itself."""
    for name, pre in _STEAM_RUNNERS:
        exe = shutil.which(name)
        if not exe:
            continue
        try:
            subprocess.Popen([exe] + pre + [url], start_new_session=True,
                             stdout=subprocess.DEVNULL,
                             stderr=subprocess.DEVNULL)
            return True
        except OSError:
            continue
    return False


def open_url(url):
    """Open `url` with whatever the desktop uses. True if something took it.

    ⚠ Report the OUTCOME. `os.startfile` raises when there is no handler, and
    callers rely on that to fall back; a Linux version that always returned
    None would make "started through Steam" print on a machine where nothing
    happened at all.
    """
    url = str(url or "")
    if not url:
        return False
    if IS_WINDOWS:
        os.startfile(url)      # noqa: S606
        return True
    steamish = url.startswith("steam://")
    # For steam:// prefer the client: xdg-open "succeeds" by launching a
    # handler that may itself do nothing.
    if steamish and _steam_client(url):
        return True
    if _xdg_open(url):
        return True
    if steamish:
        return False
    logs.LOG.warning("no way to open %s — is xdg-utils installed?", url)
    return False
