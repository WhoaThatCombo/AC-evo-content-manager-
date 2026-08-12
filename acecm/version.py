"""Build identity and the update check.

Updating a running .exe on Windows is the awkward part: you cannot overwrite a
file that is currently executing. The trick used here is the standard one -
download beside the current exe, then hand off to a tiny batch script that
waits for this process to exit, swaps the files, and relaunches. The old build
is kept as .old so a bad update can be rolled back by hand.

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
import urllib.error
import urllib.request

from . import config

VERSION = "0.4.0"
NAME = "Assetto Corsa EVO Content Manager"


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
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read())


def check():
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
            asset = next((a for a in rel.get("assets", [])
                          if a.get("name", "").lower() == "acecm.exe"), None)
            # GitHub publishes a digest as "sha256:<hex>" when it has one.
            digest = (asset or {}).get("digest") or ""
            return {"ok": True, "current": VERSION, "checked": True,
                    "source": f"github:{repo}",
                    "latest": latest, "available": _newer(latest, VERSION),
                    "notes": (rel.get("body") or "")[:4000],
                    "published": rel.get("published_at"),
                    "url": (asset or {}).get("browser_download_url", ""),
                    "sha256": digest.split(":")[-1] if digest else "",
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


def apply(url=None, sha256=None):
    """Download a new build and schedule the swap on exit."""
    if not config.FROZEN:
        return {"ok": False,
                "error": "running from source - update with git, not this"}
    info = check()
    url = url or info.get("url")
    sha256 = sha256 or info.get("sha256")
    if not url:
        return {"ok": False, "error": "no download url"}

    exe = sys.executable
    new = exe + ".new"
    try:
        req = urllib.request.Request(url, headers={
            **_headers(),
            # asset downloads need the octet-stream Accept, not the JSON one
            "Accept": "application/octet-stream"})
        with urllib.request.urlopen(req, timeout=300) as r, open(new, "wb") as f:
            h = hashlib.sha256()
            while True:
                chunk = r.read(1 << 20)
                if not chunk:
                    break
                h.update(chunk)
                f.write(chunk)
        got = h.hexdigest()
    except Exception as ex:
        return {"ok": False, "error": f"download failed: {ex}"}

    # ⚠ Verify BEFORE scheduling the swap. A truncated or tampered download
    # that gets moved into place leaves the user with a broken install and no
    # obvious way back.
    if sha256 and got.lower() != sha256.lower():
        os.remove(new)
        return {"ok": False,
                "error": f"checksum mismatch (got {got[:12]}, "
                         f"expected {sha256[:12]})"}

    bat = os.path.join(config.DATA, "_update.bat")
    with open(bat, "w", encoding="utf-8") as f:
        f.write(
            "@echo off\r\n"
            "rem wait for ACECM to exit before touching its own exe\r\n"
            ":wait\r\n"
            f'tasklist /fi "PID eq {os.getpid()}" | find "{os.getpid()}" >nul\r\n'
            "if not errorlevel 1 (timeout /t 1 /nobreak >nul & goto wait)\r\n"
            f'move /y "{exe}" "{exe}.old" >nul\r\n'
            f'move /y "{new}" "{exe}" >nul\r\n'
            f'start "" "{exe}"\r\n'
            'del "%~f0"\r\n')
    subprocess.Popen(["cmd", "/c", bat], creationflags=0x00000008)  # DETACHED
    return {"ok": True, "version": info.get("latest"),
            "note": "downloaded and verified; close ACECM to finish the update "
                    "- it will relaunch itself"}
