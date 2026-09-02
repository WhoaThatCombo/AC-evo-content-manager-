"""Steam and Proton on Linux: where things are, and how to run a Windows exe.

ACECM's job does not change on Linux, but two of its assumptions do.

  * **There is no registry.** Steam's location is a handful of well-known
    directories instead of `HKCU\\Software\\Valve\\Steam`. Once found, the rest
    is identical: `libraryfolders.vdf` and `appmanifest_<appid>.acf` are the
    same text files on both platforms, so `detect` still resolves an install
    without scanning a disk.

  * **The game and the dedicated server are still Windows binaries.** They run
    under Proton, inside a prefix Steam keeps at
    `steamapps/compatdata/<appid>/pfx`. Anything ACECM starts has to go
    through the same Proton build and the same prefix the game uses, or it
    sees a different `Saved Games` and a different `content.kspkg` than the
    game does — which is the whole reason content installs land where they do.

⚠ Do not reach for system `wine`. Opening a Proton prefix with a different
wine version rewrites it, and the next Steam launch of the game inherits the
damage. Proton is found per-app from `config_info`, not guessed.
"""
import functools
import os
import re

from . import logs

# The Steam client's own directory, in the order Valve's own scripts try.
_STEAM_ROOTS = (
    "~/.steam/steam",
    "~/.steam/root",
    "~/.local/share/Steam",
    "~/.var/app/com.valvesoftware.Steam/data/Steam",   # flatpak
    "~/snap/steam/common/.local/share/Steam",
)


def _real(path):
    """Absolute, symlink-resolved, or "" if it does not exist.

    ⚠ Everything that compares or de-duplicates paths must go through this.
    On an rpm-ostree system (Bazzite, Silverblue) `/home` is a symlink to
    `/var/home`, so Steam writes `/var/home/you/...` into libraryfolders.vdf
    while `os.path.expanduser("~")` gives `/home/you/...`. The two name the
    same directory and compare unequal, which silently doubles every library
    and can make a "already installed here" check answer no.
    """
    if not path:
        return ""
    try:
        p = os.path.realpath(os.path.expanduser(path))
    except OSError:
        return ""
    return p if os.path.exists(p) else ""


def steam_root():
    """Steam's install folder."""
    for cand in _STEAM_ROOTS:
        p = _real(cand)
        if p and os.path.isdir(os.path.join(p, "steamapps")):
            return p
    return ""


def libraries():
    """Every Steam library folder, the root's own included."""
    root = steam_root()
    if not root:
        return []
    out, seen = [], set()

    def add(p):
        p = _real(p)
        if p and p not in seen and os.path.isdir(os.path.join(p, "steamapps")):
            seen.add(p)
            out.append(p)

    add(root)
    vdf = os.path.join(root, "steamapps", "libraryfolders.vdf")
    try:
        with open(vdf, encoding="utf-8", errors="replace") as fh:
            text = fh.read()
    except OSError:
        return out
    for m in re.finditer(r'"path"\s*"([^"]+)"', text):
        add(m.group(1).replace("\\\\", "/"))
    return out


def app_dir(appid):
    """A Steam app's install directory, via its appmanifest."""
    want = str(appid)
    for lib in libraries():
        acf = os.path.join(lib, "steamapps", f"appmanifest_{want}.acf")
        if not os.path.isfile(acf):
            continue
        try:
            with open(acf, encoding="utf-8", errors="replace") as fh:
                text = fh.read()
        except OSError:
            continue
        m = re.search(r'"installdir"\s*"([^"]+)"', text)
        if not m:
            continue
        p = os.path.join(lib, "steamapps", "common", m.group(1))
        if os.path.isdir(p):
            return p
    return ""


def compat_data(appid):
    """`steamapps/compatdata/<appid>` — the folder that holds the prefix."""
    want = str(appid)
    for lib in libraries():
        p = os.path.join(lib, "steamapps", "compatdata", want)
        if os.path.isdir(os.path.join(p, "pfx")):
            return p
    return ""


def prefix(appid):
    """The wine prefix itself."""
    base = compat_data(appid)
    return os.path.join(base, "pfx") if base else ""


def _config_info(appid):
    """Lines of `compatdata/<appid>/config_info`, written by Proton itself.

    This is the record of which Proton build last set the prefix up. Picking
    the newest Proton on disk instead would be a guess, and a wrong one for
    anybody who pinned a version in the game's compatibility settings.
    """
    base = compat_data(appid)
    if not base:
        return []
    try:
        with open(os.path.join(base, "config_info"),
                  encoding="utf-8", errors="replace") as fh:
            return [ln.strip() for ln in fh.read().splitlines() if ln.strip()]
    except OSError:
        return []


def proton_dir(appid):
    """The Proton install that owns this app's prefix.

    config_info's second line points inside the Proton tree
    (`.../Proton - Experimental/files/share/fonts/`); walk up to the folder
    that actually holds the `proton` script.
    """
    for line in _config_info(appid):
        if "/files/" not in line:
            continue
        root = line.split("/files/")[0]
        if os.path.isfile(os.path.join(root, "proton")):
            return root
    # Fall back to whatever is installed, newest first by mtime.
    cands = []
    for lib in libraries():
        common = os.path.join(lib, "steamapps", "common")
        try:
            names = os.listdir(common)
        except OSError:
            continue
        for n in names:
            p = os.path.join(common, n)
            if os.path.isfile(os.path.join(p, "proton")):
                cands.append(p)
    for lib in (steam_root(),):
        extra = os.path.join(lib, "compatibilitytools.d") if lib else ""
        try:
            for n in os.listdir(extra):
                p = os.path.join(extra, n)
                if os.path.isfile(os.path.join(p, "proton")):
                    cands.append(p)
        except OSError:
            pass
    if not cands:
        return ""
    cands.sort(key=lambda p: os.path.getmtime(p), reverse=True)
    return cands[0]


def runtime_entry(appid):
    """The Steam Linux Runtime entry point Proton expects to run inside.

    Modern Proton is built against the runtime container, not the host. Some
    builds start anyway and then fail on a host library version mismatch —
    which surfaces as the exe dying seconds in with no useful message, the
    same shape as the Windows "server prints Start Server then dies" bug. If
    the runtime the toolmanifest asks for is present, go through it.
    """
    tool = os.path.join(proton_dir(appid) or "", "toolmanifest.vdf")
    need = ""
    try:
        with open(tool, encoding="utf-8", errors="replace") as fh:
            m = re.search(r'"require_tool_appid"\s*"(\d+)"', fh.read())
            if m:
                need = m.group(1)
    except OSError:
        pass
    if need:
        d = app_dir(need)
        entry = os.path.join(d or "", "_v2-entry-point")
        if os.path.isfile(entry):
            return entry
    # Named lookup, for a runtime that is present but has no manifest match.
    for lib in libraries():
        common = os.path.join(lib, "steamapps", "common")
        try:
            names = sorted(os.listdir(common), reverse=True)
        except OSError:
            continue
        for n in names:
            if not n.startswith("SteamLinuxRuntime"):
                continue
            entry = os.path.join(common, n, "_v2-entry-point")
            if os.path.isfile(entry):
                return entry
    return ""


# ------------------------------------------------------------ path mapping --
@functools.lru_cache(maxsize=8)
def _drives(pfx):
    """DOS drive letter -> host directory, longest host path first.

    Read from `dosdevices` rather than assumed. `z:` is `/` on every prefix,
    but Steam also adds `s:` for its own install. Prefer the most specific
    mapping (longest host prefix) so a file under the Steam library comes out
    as `S:\\steamapps\\...` rather than `Z:\\var\\home\\...`: both name the
    same bytes, but the shorter-looking one is what Steam's own tooling uses,
    and a config the game rewrites should not flip between the two.
    """
    out = []
    dd = os.path.join(pfx, "dosdevices")
    try:
        names = os.listdir(dd)
    except OSError:
        return out
    for n in names:
        if len(n) != 2 or not n.endswith(":"):
            continue
        target = _real(os.path.join(dd, n))
        if target:
            out.append((n.upper(), target))
    out.sort(key=lambda kv: len(kv[1]), reverse=True)
    return tuple(out)


def to_windows_path(path, appid):
    """A host path as the Windows side of this prefix sees it."""
    pfx = prefix(appid)
    p = os.path.realpath(os.path.expanduser(path))
    if not pfx:
        return p
    for letter, target in _drives(pfx):
        if p == target:
            return letter + "\\"
        if p.startswith(target.rstrip("/") + "/"):
            rest = p[len(target.rstrip("/")) + 1:]
            return letter + "\\" + rest.replace("/", "\\")
    return p


def from_windows_path(path, appid):
    """The inverse: a `C:\\...` or `Z:\\...` path as a host path."""
    p = (path or "").strip()
    pfx = prefix(appid)
    if not pfx or len(p) < 2 or p[1] != ":":
        return p
    letter = p[:2].upper()
    rest = p[2:].lstrip("\\/").replace("\\", "/")
    for cand, target in _drives(pfx):
        if cand == letter:
            return os.path.join(target, rest)
    return p


def saved_games(appid):
    """`Saved Games` inside the prefix — where the ACE profile lives.

    The Windows build asks the known-folder API because a real user can
    relocate the folder or let OneDrive move it. Neither happens in a Proton
    prefix: it is created by wine at a fixed place under the prefix user.
    """
    pfx = prefix(appid)
    if not pfx:
        return ""
    for user in ("steamuser", os.environ.get("USER") or ""):
        if not user:
            continue
        p = os.path.join(pfx, "drive_c", "users", user, "Saved Games")
        if os.path.isdir(p):
            return p
    return ""


# ---------------------------------------------------------------- running --
def available(appid):
    """True if we have everything needed to start a Windows exe."""
    return bool(prefix(appid)) and bool(proton_dir(appid))


def run_env(appid, env=None):
    """Environment for a Proton child.

    ⚠ Both STEAM_COMPAT_* variables are mandatory; Proton exits immediately
    without them and the caller only sees a dead process. They are what pins
    the child to the GAME's prefix rather than letting Proton mint a new one.
    """
    from . import winproc
    out = winproc.child_env(env)
    base = compat_data(appid)
    root = steam_root()
    if base:
        out["STEAM_COMPAT_DATA_PATH"] = base
    if root:
        out["STEAM_COMPAT_CLIENT_INSTALL_PATH"] = root
    out.setdefault("STEAM_COMPAT_APP_ID", str(appid))
    out.setdefault("SteamAppId", str(appid))
    # Headless helpers must never bring up an overlay or a gamepad UI.
    out.setdefault("PROTON_NO_ESYNC", out.get("PROTON_NO_ESYNC", "0"))
    return out


def run_argv(appid, exe, args=(), verb="run"):
    """argv that runs a Windows `exe` inside this app's prefix.

    `run` is the verb Steam itself uses for a game. `runinprefix` skips the
    steam.exe shim and is the right one for a plain console tool like the
    dedicated server — it keeps our process the direct parent, so the pid we
    remember is the pid we can later kill.
    """
    proton = proton_dir(appid)
    if not proton:
        raise RuntimeError("no Proton install found for appid %s" % appid)
    inner = [os.path.join(proton, "proton"), verb, exe] + [str(a) for a in args]
    entry = runtime_entry(appid)
    if entry:
        return [entry, "--verb=" + verb, "--"] + inner
    logs.LOG.warning("Steam Linux Runtime not found; running Proton on the "
                     "host. If %s dies immediately, install the runtime.",
                     os.path.basename(exe))
    return inner
