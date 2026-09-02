"""Build identity and the update check.

Updating a running .exe on Windows is the awkward part: you cannot overwrite a
file that is currently executing. The trick used here is the standard one -
download beside the current exe, then hand off to a tiny batch script that
waits for this process to exit, swaps the files, and relaunches. The new exe
must write an --okflag file once it is actually serving; if it does not, the
script puts .old back. A dead update no longer leaves the Start Menu broken.

The update source is a JSON manifest the user configures; nothing phones home
by default, and `update_url` empty means the check is skipped entirely.

    {"version": "0.3.0", "url": "https://.../ACECM.exe",
     "sha256": "…", "notes": "what changed"}
"""
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import time
import uuid
import urllib.error
import urllib.parse
import urllib.request

from . import config, logs

VERSION = "0.16.0"
_ROLLBACK = None
NAME = "Assetto Corsa EVO Content Manager"


class _DropAuthOnHop(urllib.request.HTTPRedirectHandler):
    """Do not forward the GitHub API token onto the asset CDN.

    A signed release-asset URL answers 404 "The specified blob does not
    exist" if the API Authorization header is still attached. urllib
    forwards it by default.
    """

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        new = super().redirect_request(req, fp, code, msg, headers, newurl)
        if new is None:
            return None
        old = urllib.parse.urlparse(req.full_url).netloc.lower()
        hop = urllib.parse.urlparse(new.full_url).netloc.lower()
        if hop != old:
            try:
                new.remove_header("Authorization")
            except Exception:
                pass
        return new


_CTX = None


def _ssl_context():
    """Trust the Windows store AND a bundled CA list, not one or the other.

    ⚠ A frozen build cannot rely on the machine's certificates alone. On a
    Windows install that has never had to fetch an intermediate, verifying
    github.com fails with

        [SSL: CERTIFICATE_VERIFY_FAILED] unable to get local issuer certificate

    and the updater simply stops working - which is exactly how one server box
    got stuck on an old build with no way to move.

    ⚠ Nor can it rely on the bundled list alone: a machine behind a corporate
    proxy or an antivirus that intercepts TLS presents its own root, which is
    in the Windows store and never in certifi. Loading both means either route
    can satisfy the chain, and neither case needs a human to diagnose it.
    """
    global _CTX
    if _CTX is not None:
        return _CTX
    import ssl
    ctx = ssl.create_default_context()      # the OS store
    try:
        import certifi
        ctx.load_verify_locations(certifi.where())
    except Exception as ex:                 # no bundle: the OS store stands
        logs.LOG.info("no bundled CA list (%s); using the system store", ex)
    _CTX = ctx
    return ctx


def _opener():
    import ssl as _ssl                      # noqa: F401  (context type only)
    return urllib.request.build_opener(
        _DropAuthOnHop,
        urllib.request.HTTPSHandler(context=_ssl_context()))


# What this platform's build is called in a GitHub release. Windows keeps the
# historical name so existing installs keep updating; Linux has no extension.
ASSET_NAME = "ACECM.exe" if sys.platform == "win32" else "ACECM"


def _newer(a, b):
    """Is version a newer than b? Compares numerically, not as text."""
    def parts(v):
        out = []
        for chunk in str(v).split("."):
            digits = "".join(c for c in chunk if c.isdigit())
            out.append(int(digits) if digits else 0)
        return out
    pa, pb = parts(a), parts(b)
    pa += [0] * (len(pb) - len(pa))
    pb += [0] * (len(pa) - len(pb))
    return pa > pb


def _headers():
    h = {"Accept": "application/vnd.github+json",
         # GitHub rejects requests with no User-Agent
         "User-Agent": f"ACECM/{VERSION}"}
    # ⚠ A PRIVATE repo answers 404, not 403, to an unauthenticated request -
    # indistinguishable from "no such repo". A token is required to see it at
    # all, and also to download its release assets.
    tok = (config.CFG.get("update_token") or
           os.environ.get("ACECM_GITHUB_TOKEN") or "").strip()
    if tok:
        h["Authorization"] = f"Bearer {tok}"
    return h


def _get_json(url, timeout=20):
    req = urllib.request.Request(url, headers=_headers())
    with _opener().open(req, timeout=timeout) as r:
        return json.loads(r.read())


def _open_download(url, timeout=300):
    """GET a release asset. API asset URLs need octet-stream Accept."""
    req = urllib.request.Request(url, headers={
        **_headers(),
        "Accept": "application/octet-stream",
    })
    return _opener().open(req, timeout=timeout)


def _download_file(urls, dest, timeout=300, retries=4):
    """Write the first URL that works. 404s are retried: GitHub's CDN
    lags behind a just-replaced release and answers 'blob does not exist'."""
    if isinstance(urls, str):
        urls = [urls]
    urls = [u for u in urls if u]
    if not urls:
        raise RuntimeError("no download url")
    last = None
    for url in urls:
        for attempt in range(retries):
            try:
                with _open_download(url, timeout=timeout) as r, open(dest, "wb") as f:
                    h = hashlib.sha256()
                    while True:
                        chunk = r.read(1 << 20)
                        if not chunk:
                            break
                        h.update(chunk)
                        f.write(chunk)
                return h.hexdigest()
            except urllib.error.HTTPError as ex:
                last = ex
                if ex.code in (404, 502, 503) and attempt + 1 < retries:
                    time.sleep(1.5 * (attempt + 1))
                    continue
                break
            except Exception as ex:
                last = ex
                break
    raise last


_CHECK_CACHE = {"at": 0.0, "result": None}
_CHECK_TTL = 30 * 60      # seconds


def check(force=False):
    """Cached wrapper around _check_now.

    ⚠ The dashboard calls this on EVERY render, and the dashboard re-renders
    itself on a ~1s timer after any action. That made a ~600 ms network
    round-trip to GitHub part of drawing the page - the single biggest source
    of the UI feeling slow. It also burned the unauthenticated rate limit (60
    requests an hour) within a minute, after which the update check itself
    started failing with 403 and reported nonsense.

    A release does not appear more than once every half hour, so serve a
    cached answer and let the Updater page pass force=True.
    """
    import time as _time
    now = _time.monotonic()
    hit = _CHECK_CACHE["result"]
    if hit is not None and not force and (now - _CHECK_CACHE["at"]) < _CHECK_TTL:
        return {**hit, "cached": True}
    out = _check_now()
    # Never cache a failure: a transient network blip would otherwise pin the
    # UI to "check failed" for half an hour.
    if out.get("ok"):
        _CHECK_CACHE.update(at=now, result=out)
    return out


def _check_now():
    """Look for a newer build. Never raises - the UI shows whatever comes back.

    Reads the latest GitHub Release of `update_repo` and looks for an
    `ACECM.exe` asset. A tag of `v0.4.0` or `0.4.0` both work; the comparison
    is numeric, so `0.10.0` correctly beats `0.9.0` (string compare would not).
    """
    repo = (config.CFG.get("update_repo") or "").strip()
    url = (config.CFG.get("update_url") or "").strip()
    if not repo and not url:
        return {"ok": True, "current": VERSION, "checked": False,
                "hint": "set update_repo (owner/name) in Settings to check "
                        "GitHub for new builds"}
    try:
        if repo:
            rel = _get_json(f"https://api.github.com/repos/{repo}/releases/latest")
            latest = str(rel.get("tag_name") or rel.get("name") or "").lstrip("vV")
            assets = rel.get("assets") or []
            # ⚠ The asset name is per-platform. A release carries both builds,
            # so a Linux ACECM that matched "acecm.exe" would download the
            # WINDOWS binary, verify its checksum happily, and replace itself
            # with something it cannot execute - a self-update that bricks the
            # install and passes every check on the way.
            want = ASSET_NAME.lower()
            asset = next((a for a in assets
                          if a.get("name", "").lower() == want), None)
            # Prefer the ACECM.exe.sha256 we upload. GitHub's own digest is
            # often missing, and when it is present it is not always the
            # same string we published.
            sha = ""
            digest = (asset or {}).get("digest") or ""
            if digest:
                sha = digest.split(":")[-1]
            sum_asset = next((a for a in assets
                              if a.get("name", "").lower()
                              == want + ".sha256"), None)
            if sum_asset:
                sum_url = (sum_asset.get("url")
                           or sum_asset.get("browser_download_url"))
                if sum_url:
                    try:
                        with _open_download(sum_url, timeout=20) as r:
                            txt = r.read().decode("ascii", "replace")
                        hexpart = txt.strip().split()[0]
                        if len(hexpart) == 64:
                            sha = hexpart
                    except Exception:
                        pass
            # Prefer the API asset URL. browser_download_url 404s after a
            # tag is deleted and recreated — the CDN keeps the old path
            # and answers "The specified blob does not exist".
            api_url = (asset or {}).get("url") or ""
            cdn_url = (asset or {}).get("browser_download_url") or ""
            return {"ok": True, "current": VERSION, "checked": True,
                    "source": f"github:{repo}",
                    "latest": latest, "available": _newer(latest, VERSION),
                    "notes": (rel.get("body") or "")[:4000],
                    "published": rel.get("published_at"),
                    "url": api_url or cdn_url,
                    "browser_url": cdn_url,
                    "sha256": sha,
                    "error": None if asset else
                             "that release has no ACECM.exe asset attached"}
        man = _get_json(url)
        latest = str(man.get("version", ""))
        return {"ok": True, "current": VERSION, "checked": True,
                "source": url,
                "latest": latest, "available": _newer(latest, VERSION),
                "notes": man.get("notes", ""), "url": man.get("url", ""),
                "sha256": man.get("sha256", "")}
    except urllib.error.HTTPError as ex:
        hint = ""
        if ex.code == 404:
            hint = ("either that repo has no releases yet, or it is PRIVATE - "
                    "a private repo returns 404 to an unauthenticated check. "
                    "Make it public, or set update_token to a GitHub token "
                    "with 'repo' scope.")
        elif ex.code in (401, 403):
            hint = "the token was rejected, or the API rate limit was hit."
        return {"ok": False, "current": VERSION, "repo": repo,
                "error": f"HTTP {ex.code}", "hint": hint}
    except Exception as ex:
        return {"ok": False, "current": VERSION,
                "error": f"{type(ex).__name__}: {ex}"}


def rollback_flag_path():
    return os.path.join(config.DATA, "update-rollback.flag")


def pending_path():
    return os.path.join(config.DATA, "update-pending.flag")


def swap_pending():
    """True when a downloaded exe is waiting for this process to exit."""
    return os.path.isfile(pending_path())



def _linux_swap(exe, new, relaunch=True, ver=""):
    """Replace the running executable, then restart it.

    ⚠ Far simpler than the Windows path, and for a real reason: Linux
    refuses to WRITE to a running binary (ETXTBSY) but is perfectly happy to
    RENAME it. The inode stays alive for this process while the directory
    entry points at the new file, so the swap needs no waiting, no retry
    loop, and no helper script watching for our pid to disappear.

    ⚠ os.replace, not shutil.move, and the temporary must already be on the
    same filesystem as the exe. os.replace is atomic within a filesystem and
    raises across one; a copy-based move could be interrupted and leave a
    half-written ACECM behind with the original already renamed away.
    """
    import shutil
    old = exe + ".old"
    staged = exe + ".new"
    # Stage beside the exe so the two renames below are same-filesystem.
    shutil.copyfile(new, staged)
    shutil.copymode(exe, staged)
    os.chmod(staged, os.stat(staged).st_mode | 0o111)
    try:
        os.replace(exe, old)
    except OSError as ex:
        os.remove(staged)
        raise RuntimeError(f"could not move the old build aside: {ex}")
    try:
        os.replace(staged, exe)
    except OSError as ex:
        os.replace(old, exe)          # put it back; we are still runnable
        raise RuntimeError(f"could not move the new build into place: {ex}")
    try:
        os.remove(new)
    except OSError:
        pass
    logs.LOG.info("update: swapped in %s (previous build kept as %s)",
                  ver or "new build", os.path.basename(old))
    if relaunch:
        _linux_relaunch(exe)
    return old


def _linux_relaunch(exe):
    """Start the new build once this process has released the port.

    ⚠ Cannot simply Popen and exit: the new copy would race us for the UI
    port and die with "address in use", which looks exactly like ACECM
    closing and never coming back. A tiny detached shell waits for our pid
    the way the Windows batch file does.
    """
    import subprocess
    script = (
        f'while kill -0 {os.getpid()} 2>/dev/null; do sleep 0.5; done; '
        f'sleep 0.5; exec "{exe}" --relaunch'
    )
    subprocess.Popen(["/bin/sh", "-c", script], start_new_session=True,
                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                     cwd=os.path.dirname(exe) or None)


def schedule_relaunch(exe, pid, bat=None):
    """Start ACECM again only after this PID is gone.

    Launching a second copy first fights for port 8092; the child dies
    with 'port in use' and Restart looks like it closed and never came back.
    """
    if sys.platform != "win32":
        _linux_relaunch(exe)
        return ""
    bat = bat or os.path.join(config.DATA, "_relaunch.bat")
    inst = os.path.dirname(exe)
    os.makedirs(config.DATA, exist_ok=True)
    lines = [
        "@echo off\r\n",
        ":wait\r\n",
        f'tasklist /fi "PID eq {pid}" 2>nul | find "{pid}" >nul\r\n',
        "if not errorlevel 1 (ping -n 2 127.0.0.1 >nul & goto wait)\r\n",
        "ping -n 2 127.0.0.1 >nul\r\n",
        # ⚠ --relaunch tells the new copy it is REPLACING us, so it waits for
        # the port instead of deferring to the instance still letting go of
        # it. Without it, Restart looked like it closed ACECM for good.
        f'start "ACECM" /d "{inst}" "{exe}" --relaunch\r\n',
        'del "%~f0"\r\n',
    ]
    with open(bat, "w", encoding="utf-8") as f:
        f.writelines(lines)
    CREATE_NO_WINDOW = 0x08000000
    CREATE_NEW_PROCESS_GROUP = 0x00000200
    subprocess.Popen(["cmd", "/c", bat],
                     creationflags=CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP,
                     close_fds=True, cwd=config.DATA)
    return bat


def last_rollback():
    """Set on this process if the previous swap had to put .old back."""
    return _ROLLBACK


def consume_rollback():
    """If the last update was rolled back, remember it for the dashboard."""
    global _ROLLBACK
    path = rollback_flag_path()
    if not os.path.isfile(path):
        return None
    try:
        os.remove(path)
    except OSError:
        pass
    _ROLLBACK = {
        "what": "The last update did not start, so the previous build was restored",
        "do": "You can try updating again. The broken download was discarded.",
    }
    logs.LOG.warning("previous update was rolled back — restored the previous exe")
    return _ROLLBACK


def confirm_update(okflag):
    """The new exe is serving. Tell the swap script it can keep us."""
    if not okflag:
        return
    try:
        parent = os.path.dirname(okflag)
        if parent:
            os.makedirs(parent, exist_ok=True)
        with open(okflag, "w", encoding="utf-8") as f:
            f.write("ok\n")
        logs.LOG.info("update handshake: wrote %s", okflag)
    except OSError as ex:
        logs.LOG.warning("update handshake could not write %s: %s", okflag, ex)


def _write_swap_script(exe, new, pid, bat=None, blog=None, relaunch=True,
                       ver=None, okflag=None):
    """The script that replaces a running exe once it exits.

    Split out so it can be tested without downloading anything - the swap is
    the one part of updating that cannot be exercised in normal use.

    After the swap the new exe is launched with --okflag. If that file is
    not created (the binary never got as far as serving), .old is copied
    back and a rollback flag is left for the restored build to show.
    """
    bat = bat or os.path.join(config.DATA, "_update.bat")
    blog = blog or os.path.join(config.DATA, "_update.log")
    inst = os.path.dirname(exe)
    marker = os.path.join(inst, "version.txt")
    prev_marker = marker + ".prev"
    rolled = rollback_flag_path()
    if relaunch and not okflag:
        okflag = os.path.join(tempfile.gettempdir(),
                              "acecm-upok-" + uuid.uuid4().hex + ".flag")
    args = ""
    if relaunch:
        if ver:
            args += f" --updated {ver}"
        if okflag:
            args += f' --okflag "{okflag}"'
        # ⚠ Keep the launch MODE across an update. A headless server that
        # updated itself used to relaunch WITHOUT --headless: it came back in
        # window mode, which on a box nobody sits at either opens a stray
        # window or fails to start, and window mode does not turn remote
        # administration on - so the server went dark and its operator had to
        # restart it by hand. The running process still carries the flag it
        # was started with, so carry it through to the copy that replaces it.
        for flag in ("--headless", "--no-ui", "--browser"):
            if flag in sys.argv and flag not in args:
                args += f" {flag}"
    lines = [
        "@echo off\r\n",
        # ⚠ Delayed expansion is required: %VAR% inside a parenthesised
        # block is expanded when the block is PARSED, so a retry counter
        # written that way never changes and the loop never ends.
        "setlocal enabledelayedexpansion\r\n",
        f'set LOG="{blog}"\r\n',
        f'echo [%date% %time%] waiting for pid {pid} > %LOG%\r\n',
        "rem Wait for ACECM to exit - Windows will not let a running exe be\r\n",
        "rem replaced. ⚠ Use ping, not timeout: timeout needs a console and\r\n",
        "rem fails outright in a windowless process.\r\n",
        ":wait\r\n",
        f'tasklist /fi "PID eq {pid}" 2>nul | find "{pid}" >nul\r\n',
        "if not errorlevel 1 (ping -n 2 127.0.0.1 >nul & goto wait)\r\n",
        # ⚠ Waiting for the main pid is NOT enough. ACECM spawns children
        # from its own exe (--tool acevo_proxy, the telemetry trackers), and
        # Windows keeps the image file locked while any of them lives. The
        # move then failed with "being used by another process", retried ten
        # times, gave up and relaunched the OLD build - so every update
        # silently did nothing and Restart came back on the same version,
        # with the reason buried in _update.log.
        #
        # ⚠ Match on the EXE PATH, not the image name. `taskkill /IM
        # ACECM.exe` ends every ACECM on the machine, which would take down
        # an unrelated one - a headless server on another port, or a second
        # install - as collateral of updating this one. Only the copies
        # running the file we are about to replace are holding it open.
        "rem clear helpers still running the exe we are replacing\r\n",
        ('powershell -NoProfile -NonInteractive -Command '
         '"Get-Process -Name ACECM -ErrorAction SilentlyContinue | '
         'Where-Object { $_.Path -eq \'%s\' } | '
         'Stop-Process -Force -ErrorAction SilentlyContinue" >>%%LOG%% 2>&1\r\n'
         % exe.replace("'", "''")),
        "rem the file can stay locked for a moment after the process goes\r\n",
        "ping -n 4 127.0.0.1 >nul\r\n",
        "set TRIES=0\r\n",
        ":swap\r\n",
        f'move /y "{exe}" "{exe}.old" >>%LOG% 2>&1\r\n',
        "if errorlevel 1 (\r\n",
        "  set /a TRIES+=1\r\n",
        "  if !TRIES! lss 20 (ping -n 2 127.0.0.1 >nul & goto swap)\r\n",
        "  echo could not replace the exe - it is still locked >>%LOG%\r\n",
        "  goto giveup\r\n",
        ")\r\n",
        f'move /y "{new}" "{exe}" >>%LOG% 2>&1\r\n',
        "if errorlevel 1 (\r\n",
        "  echo could not move the new exe into place - restoring >>%LOG%\r\n",
        f'  copy /y "{exe}.old" "{exe}" >>%LOG% 2>&1\r\n',
        "  goto giveup\r\n",
        ")\r\n",
        "echo swapped ok >>%LOG%\r\n",
        f'if exist "{marker}" copy /y "{marker}" "{prev_marker}" >>%LOG% 2>&1\r\n',
    ]
    if ver:
        lines.append(f'echo {ver}> "{marker}"\r\n')
    if relaunch:
        lines.extend([
            f'start "ACECM" /d "{inst}" "{exe}"{args} --relaunch\r\n',
            "set W=0\r\n",
            ":waitok\r\n",
            "ping -n 3 127.0.0.1 >nul\r\n",
            (f'if exist "{okflag}" goto upok\r\n' if okflag
             else "goto upok\r\n"),
            "set /a W+=1\r\n",
            # ~60s. PyInstaller + WebView2 is slow the first time.
            "if !W! lss 20 goto waitok\r\n",
            # A live ACECM.exe after the swap is success even if the flag
            # file was lost. Rolling back then fights that process and
            # leaves the user with a closed window.
            f'tasklist /fi "IMAGENAME eq ACECM.exe" 2>nul | find /i "ACECM.exe" >nul\r\n',
            "if not errorlevel 1 (\r\n",
            "  echo new exe is running without a flag - keeping it >>%LOG%\r\n",
            "  goto upok\r\n",
            ")\r\n",
            "echo new exe did not start - rolling back >>%LOG%\r\n",
            ":rollback\r\n",
            f'copy /y "{exe}.old" "{exe}" >>%LOG% 2>&1\r\n',
            f'if exist "{prev_marker}" copy /y "{prev_marker}" "{marker}" >>%LOG% 2>&1\r\n',
            f'echo rollback> "{rolled}"\r\n',
            f'start "ACECM" /d "{inst}" "{exe}" --relaunch\r\n',
            "goto done\r\n",
            ":upok\r\n",
            (f'del "{okflag}" >nul 2>&1\r\n' if okflag else ""),
            f'del "{pending_path()}" >nul 2>&1\r\n',
            "echo handshake ok >>%LOG%\r\n",
            "goto done\r\n",
        ])
    lines.extend([
        ":giveup\r\n",
        f'echo giveup >>%LOG%\r\n',
        (f'start "" /d "{inst}" "{exe}"\r\n' if relaunch else ""),
        ":done\r\n",
        'del "%~f0"\r\n',
    ])
    with open(bat, "w", encoding="utf-8") as f:
        f.writelines(lines)
    return bat, blog


def apply(url=None, sha256=None):
    """Download a new build and schedule the swap on exit."""
    if not config.FROZEN:
        return {"ok": False,
                "error": "running from source - update with git, not this"}
    info = check()
    # Ignore caller url/sha256. Those used to come from the POST body, so
    # anyone who could reach the API could feed a binary of their own.
    sha256 = info.get("sha256") or ""
    urls = []
    for u in (info.get("url"), info.get("browser_url")):
        if u and u not in urls:
            urls.append(u)
    if not urls:
        return {"ok": False, "error": "no download url"}
    if not sha256 or len(str(sha256)) != 64:
        return {"ok": False,
                "error": "that release has no checksum — refusing to install"}

    exe = sys.executable
    # ⚠ Do not write ACECM.exe.new next to the running exe. A previous
    # failed swap leaves that file locked and apply() then crashes with
    # WinError 32 before any download starts.
    os.makedirs(config.DATA, exist_ok=True)
    new = os.path.join(config.DATA, "ACECM-update.new")
    try:
        if os.path.isfile(new):
            try:
                os.remove(new)
            except OSError:
                new = os.path.join(config.DATA, f"ACECM-update-{uuid.uuid4().hex[:8]}.new")
        got = _download_file(urls, new, timeout=300)
    except Exception as ex:
        msg = str(ex)
        if "404" in msg or "blob does not exist" in msg.lower():
            return {"ok": False,
                    "error": "download failed: GitHub is still publishing "
                             "the file (404). Wait a minute and try again, "
                             "or close ACECM and run the new ACECM.exe from "
                             "the GitHub release."}
        return {"ok": False, "error": f"download failed: {ex}"}

    # ⚠ Verify BEFORE scheduling the swap. A truncated or tampered download
    # that gets moved into place leaves the user with a broken install and no
    # obvious way back.
    if sha256 and got.lower() != sha256.lower():
        try:
            os.remove(new)
        except OSError:
            pass
        return {"ok": False,
                "error": f"checksum mismatch (got {got[:12]}, "
                         f"expected {sha256[:12]})"}

    if sys.platform != "win32":
        # ⚠ Swap NOW rather than scheduling it. Renaming a running binary is
        # legal here, and this process keeps executing the old inode until it
        # exits, so there is nothing to wait for and no script to go wrong.
        try:
            old = _linux_swap(exe, new, relaunch=True,
                              ver=info.get("latest") or VERSION)
        except (OSError, RuntimeError) as ex:
            return {"ok": False, "error": f"update failed: {ex}"}
        try:
            with open(pending_path(), "w", encoding="utf-8") as f:
                f.write(info.get("latest") or VERSION)
        except OSError:
            pass
        return {"ok": True, "version": info.get("latest"),
                "note": "installed and verified; close ACECM to finish - it "
                        f"will relaunch itself. The previous build is kept "
                        f"as {os.path.basename(old)}."}

    bat, blog = _write_swap_script(exe, new, os.getpid(),
                                   ver=info.get("latest") or VERSION)
    try:
        with open(pending_path(), "w", encoding="utf-8") as f:
            f.write(info.get("latest") or VERSION)
    except OSError:
        pass
    # ⚠ CREATE_NO_WINDOW, not DETACHED_PROCESS. Detached leaves the batch with
    # no console at all, and the commands it needs (timeout, and reliable
    # redirection) fail outright there - the first version scheduled a swap
    # that silently never happened. CREATE_NEW_PROCESS_GROUP keeps it alive
    # when this process and its group go away.
    CREATE_NO_WINDOW = 0x08000000
    CREATE_NEW_PROCESS_GROUP = 0x00000200
    subprocess.Popen(["cmd", "/c", bat],
                     creationflags=CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP,
                     close_fds=True, cwd=config.DATA)
    logs.LOG.info("update to %s downloaded and verified; swap scheduled (%s)",
                  info.get("latest"), blog)
    return {"ok": True, "version": info.get("latest"), "log": blog,
            "note": "downloaded and verified; close ACECM to finish the update "
                    "- it will relaunch itself. If the new build does not "
                    "start, the previous one is put back."}
