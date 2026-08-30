"""Paths and settings for Assetto Corsa EVO Content Manager.

Everything the app touches lives outside its own folder (the game install, the
dedicated server, the reverse-engineered backend tools), so all of it is
resolved here rather than hardcoded across modules. `config.json` next to the
package overrides any of it, so a second machine only needs that one file.
"""
import json
import os
import sys

# Frozen (shipped .exe) vs running from source.
#
# ⚠ A packaged build must NOT write beside its executable: that is Program
# Files for an installed copy, and a one-file build unpacks to a temp folder
# that is deleted on exit - profiles and telemetry state would vanish every
# run. User data goes to %LOCALAPPDATA%\ACECM; read-only assets ride inside
# the bundle at sys._MEIPASS.
FROZEN = getattr(sys, "frozen", False)
HERE = (sys._MEIPASS if FROZEN                      # noqa: SLF001
        else os.path.dirname(os.path.abspath(__file__)))
if FROZEN:
    ROOT = os.path.join(os.environ.get("LOCALAPPDATA")
                        or os.path.expanduser("~"), "ACECM")
    WEB = os.path.join(HERE, "acecm", "web")
    # helper scripts we wrote, shipped inside the bundle
    BUNDLED_TOOLS = os.path.join(HERE, "tools")
else:
    ROOT = os.path.dirname(HERE)
    WEB = os.path.join(HERE, "web")
    BUNDLED_TOOLS = os.path.join(ROOT, "tools")
# ⚠ ACECM_DATA pins the data directory. Windows MSIX/Store virtualization can
# redirect %LOCALAPPDATA% into a per-package LocalCache, so the same build
# launched two different ways can use two different folders and look like it
# lost your profiles. Set ACECM_DATA to end the ambiguity.
DATA = os.environ.get("ACECM_DATA") or os.path.join(ROOT, "data")

DEFAULTS = {
    # ⚠ Remembered, not re-tested every launch. Starting the game ourselves is
    # the only way to pass it flags, but Steam refuses it when our token
    # cannot claim ownership (elevated, or Family Share) - and it refuses by
    # letting the process die a few seconds in, so finding out costs a wait.
    # Paying that once is fine; paying it on every launch is not.
    "direct_launch_refused": False,
    # EVO has no units setting; the speedometer is hard-wired to km/h. This
    # redraws it in mph in the running HUD - nothing on disk changes, so a
    # launch straight from Steam is unaffected.
    # Remote administration. Off for a desktop install; --headless turns it
    # on, because a server you cannot reach is not a server. See auth.py.
    "remote_admin": False,
    # opt-in: see backend.fast_boot_flags
    "fast_boot": False,
    "hud_mph": False,
    # A wheel/box/controller button that sends the car to the pitlane. The
    # game's own "Go to Pitlane" is in the pause menu and its button is
    # hidden while you are on track, so it cannot be reached when it matters.
    "pit_btn_kind": "",      # "joy" (DirectInput) or "pad" (XInput)
    "pit_btn_id": "",
    "pit_btn_mask": 0,
    "pit_btn_label": "",
    # ⚠ On by default, and deliberately an awkward combination. It is read
    # with GetAsyncKeyState rather than reserved system-wide, so it still
    # fires while the game has focus without being taken away from anything
    # else - but that also means it is seen wherever you press it, so a
    # single letter would go off while typing. Empty disables it.
    "pit_key": "ctrl+y",
    # Dedicated server install. Empty = look for it on first run; a shipped
    # build cannot assume the developer's own folder layout.
    "server_dir": "",
    # STOCK exe by default. AssettoCorsaEVOServer.percar.exe is the binary
    # patched for virtual-AI cars; we deliberately do not ship AI bots, so the
    # unpatched server is what a build runs. (That patched copy also carried
    # the chat-logging patch - see notes if you want that back on a stock base.)
    # ⚠ Leave EMPTY. A stock install has exactly AssettoCorsaEVOServer.exe;
    # any other name is a copy someone made. Defaulting to a name that only
    # existed on the development machine made real installs report the
    # executable as missing. Set this only to force a specific binary.
    "server_exe": "",
    # our own lobby backend + client patches; empty = use the bundled copy
    "tools_dir": "",
    # game client (for direct launch); empty = auto-detect via Steam
    # Leave these EMPTY to auto-detect (Steam libraries + known folders).
    # Anything set here wins over detection.
    "game_exe": "",
    "ace_dir": "",
    "mods_dir": "",
    "client_mods_dir": "",
    "steam_appid": "3058630",
    # Where to look for new builds. update_repo is a GitHub "owner/name" and
    # is checked against that project's latest Release; update_url is the
    # older manual-manifest route and is only used if update_repo is empty.
    "update_repo": "WhoaThatCombo/AC-evo-content-manager-",
    "update_url": "",
    # only needed if update_repo is PRIVATE (a token with repo scope)
    "update_token": "",
    # ports we own
    "ui_port": 8092,
    # 0.0.0.0 = LAN/internet can fetch content. 127.0.0.1 hid the share.
    "listen": "0.0.0.0",
    "telemetry_port": 8091,
    "backend_port": 448,
    # Lobby TLS. 127.0.0.1 is the game on this PC. 0.0.0.0 would put a
    # Kunos session MITM on the LAN — only set that if you know you need it.
    "backend_listen": "127.0.0.1",
    # Start the lobby proxy with ACECM and kill it when the window closes.
    # Dedicated servers are separate and are not touched.
    "auto_proxy": True,
    # default server settings for new profiles
    "default_tcp_port": 9700,
    "default_http_port": 8080,
}


def config_path():
    """Where settings live.

    ⚠ Normally beside the data folder's parent, but when ACECM_DATA is set
    explicitly the config follows it - otherwise "run with a clean data dir"
    silently keeps the old settings, which made a fresh-install test look like
    it passed when it had not.
    """
    if os.environ.get("ACECM_DATA"):
        return os.path.join(DATA, "config.json")
    return os.path.join(ROOT, "config.json")


def _load():
    cfg = dict(DEFAULTS)
    path = config_path()
    if os.path.exists(path):
        try:
            cfg.update(json.load(open(path, encoding="utf-8")))
        except Exception as ex:
            print(f"config.json ignored ({ex})")
    return cfg


CFG = _load()


def save(patch):
    """Persist a settings change (only keys we know about)."""
    cfg = _load()
    for k, v in (patch or {}).items():
        if k not in DEFAULTS:
            continue
        # Empty token from the form means "leave the stored one".
        if k == "update_token" and not str(v or "").strip():
            continue
        cfg[k] = v
    path = config_path()
    json.dump(cfg, open(path, "w", encoding="utf-8"), indent=2)
    CFG.update(cfg)
    return public_cfg()


def public_cfg():
    """Settings for the UI. Never echo the GitHub token."""
    out = dict(CFG)
    tok = str(out.get("update_token") or "").strip()
    out["update_token"] = ""
    out["update_token_set"] = bool(tok)
    return out


def find_server_dir():
    """Locate a dedicated-server install.

    ⚠ Imported lazily: acecm.detect imports this module, so a top-level import
    here would be circular.
    """
    from . import detect
    return detect.find("server_dir")


def server_dir():
    """The dedicated-server folder: the setting if it is real, else detected."""
    d = (CFG.get("server_dir") or "").strip()
    if d and os.path.isdir(d):
        return d
    if d:
        from . import logs
        logs.LOG.warning("configured server_dir %r does not exist - detecting "
                         "instead", d)
    found = find_server_dir()
    if found:
        save({"server_dir": found})
    return found


def catalog_path(name):
    """cars.json / events_*.json: the server folder, else the bundled copy.

    A stock Steam dedicated server does not ship those catalogues. Older
    ACECM required the portable extras next to the exe, which a fresh
    install does not have. The copies in tools/ are the stock lists.
    """
    n = os.path.basename(name)
    for base in (server_dir(), BUNDLED_TOOLS):
        if not base:
            continue
        p = os.path.join(base, n)
        if os.path.isfile(p):
            return p
    return ""


def server_exe():
    """Full path to the dedicated-server executable.

    ⚠ A configured value only wins while it is TRUE. Early builds saved
    `server_exe` as a filename that existed only on the development machine, so
    upgrading left that stale value in config.json - and because an explicit
    setting outranks detection, a perfectly good install kept reporting the
    executable as missing even after the default was fixed. A setting that
    points at nothing is dropped in favour of what is actually on disk.
    """
    from . import detect, logs
    name = (CFG.get("server_exe") or "").strip()
    if name:
        # An explicit setting may be a bare filename or a full path.
        p = name if os.path.isabs(name) else os.path.join(server_dir(), name)
        if os.path.isfile(p):
            return p
        logs.LOG.warning("configured server_exe %r does not exist - ignoring it "
                         "and detecting instead", name)
        save({"server_exe": ""})          # stop it biting again next launch
    return detect.find("server_exe")


def tools_dir():
    """Our own helper scripts: the configured copy, else the bundled one."""
    d = CFG.get("tools_dir") or ""
    if d and os.path.isdir(d):
        return d
    return BUNDLED_TOOLS


def tool_cmd(name, args):
    """argv for running one of our helper tools as a child process.

    ⚠ Frozen builds have no interpreter to call. `sys.executable` is the ACECM
    exe itself, so `[sys.executable, "script.py"]` would just start a second
    copy of the app. Re-invoke this exe with `--tool <name>` instead; from
    source, run it with the Python that is already running us.
    """
    if FROZEN:
        return [sys.executable, "--tool", name] + list(args)
    exe = sys.executable
    # pythonw cannot be the parent of AssettoCorsaEVOServer.exe: the
    # launcher returns and the server dies with it, so 9700 never stays
    # open. python.exe + CREATE_NO_WINDOW (hidden_popen) is the same
    # on every machine that started ACECM via pythonw / run.bat.
    if name == "start_vai_server" and exe.lower().endswith("pythonw.exe"):
        cand = exe[:-5] + ".exe"  # pythonw.exe -> python.exe
        if os.path.isfile(cand):
            exe = cand
    return [exe, "-u", tool_script(name + ".py")] + list(args)


# Scripts this build owns. A leftover copy next to the dedicated server
# used to win, so a fix in ACECM never ran on machines that already had
# an older start_vai_server.py in the server folder.
_OWNED_TOOLS = (
    "start_vai_server.py",
    "acevo_proxy.py",
    "acevo_backend.py",
    "server_telemetry.py",
)


def tool_script(name):
    """Absolute path to one of our scripts, wherever it actually lives.

    ACECM-owned launchers always use the copy shipped with this build, so
    every machine gets the same start/hide/bind behaviour. Other helpers
    still prefer a file next to the dedicated server (parse_spline etc.).
    """
    ours = [BUNDLED_TOOLS, os.path.join(BUNDLED_TOOLS, "backend"),
            tools_dir()]
    if name not in _OWNED_TOOLS:
        ours.insert(0, server_dir())
    for base in ours:
        if not base:
            continue
        p = os.path.join(base, name)
        if os.path.isfile(p):
            return p
    return os.path.join(BUNDLED_TOOLS, name)


def server_log(name="vai_server.log"):
    return os.path.join(server_dir(), "serverConfig", name)


os.makedirs(DATA, exist_ok=True)
