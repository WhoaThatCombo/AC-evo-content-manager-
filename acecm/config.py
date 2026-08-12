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
    # Dedicated server install. Empty = look for it on first run; a shipped
    # build cannot assume the developer's own folder layout.
    "server_dir": "",
    # STOCK exe by default. AssettoCorsaEVOServer.percar.exe is the binary
    # patched for virtual-AI cars; we deliberately do not ship AI bots, so the
    # unpatched server is what a build runs. (That patched copy also carried
    # the chat-logging patch - see notes if you want that back on a stock base.)
    "server_exe": "AssettoCorsaEVOServer.stock.exe",
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
    "telemetry_port": 8091,
    "backend_port": 448,
    # default server settings for new profiles
    "default_tcp_port": 9700,
    "default_http_port": 8080,
}


def _load():
    cfg = dict(DEFAULTS)
    path = os.path.join(ROOT, "config.json")
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
    cfg.update({k: v for k, v in patch.items() if k in DEFAULTS})
    path = os.path.join(ROOT, "config.json")
    json.dump(cfg, open(path, "w", encoding="utf-8"), indent=2)
    CFG.update(cfg)
    return cfg


def find_server_dir():
    """Locate a dedicated-server install.

    ⚠ Imported lazily: acecm.detect imports this module, so a top-level import
    here would be circular.
    """
    from . import detect
    return detect.find("server_dir")


def server_dir():
    d = CFG.get("server_dir") or ""
    if not d:
        d = find_server_dir()
        if d:
            save({"server_dir": d})
    return d


def server_exe():
    return os.path.join(server_dir(), CFG["server_exe"])


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
    return [sys.executable, "-u", tool_script(name + ".py")] + list(args)


def tool_script(name):
    """Absolute path to one of our scripts, wherever it actually lives.

    Prefer a copy in the user's server folder (they may have edited it), fall
    back to the copy shipped inside the build. Without this a frozen exe looks
    for `server_telemetry.py` next to a server install that has never seen it.
    """
    for base in (server_dir(), BUNDLED_TOOLS,
                 os.path.join(BUNDLED_TOOLS, "backend"), tools_dir()):
        if not base:
            continue
        p = os.path.join(base, name)
        if os.path.isfile(p):
            return p
    return os.path.join(BUNDLED_TOOLS, name)


def server_log(name="vai_server.log"):
    return os.path.join(server_dir(), "serverConfig", name)


os.makedirs(DATA, exist_ok=True)
