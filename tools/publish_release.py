"""Publish a GitHub release for the current version, with the built exe.

    python tools/publish_release.py [--notes-file FILE]

Auth comes from the credential git already uses for this remote (Git
Credential Manager), so there is no second token to manage and nothing is
printed. This is what the `gh` CLI would do; it just is not installed here.

⚠ The in-app updater reads GitHub Releases and expects BOTH assets:
ACECM.exe and ACECM.exe.sha256. A release missing the checksum leaves the
updater unable to verify what it downloaded.
"""
import json
import os
import subprocess
import sys
import urllib.error
import urllib.request

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, HERE)
from acecm.version import VERSION                              # noqa: E402

REPO = "WhoaThatCombo/AC-evo-content-manager-"
API = "https://api.github.com"

# What every release says about the Windows warning. Kept here so the note
# cannot drift from what the build actually is.
_DOWNLOAD_HELP = r"""### Downloading

Windows will warn about this download. ACECM is not code-signed, so
SmartScreen reports an unknown publisher on every new release. That is a
reputation warning, not a virus detection.

**Install:** download **ACECM.zip** (browsers block a bare `.exe`), extract
it, run `ACECM.exe`, then choose **More info -> Run anyway**.

**Verify what you downloaded** - this must match exactly:

```
{digest}
```

```powershell
Get-FileHash .\ACECM.exe -Algorithm SHA256
```
"""


def token():
    """The password git stores for github.com. Never logged."""
    p = subprocess.run(["git", "credential", "fill"],
                       input="protocol=https\nhost=github.com\n\n",
                       capture_output=True, text=True, cwd=HERE)
    for line in p.stdout.splitlines():
        if line.startswith("password="):
            return line.split("=", 1)[1].strip()
    raise SystemExit("no stored github.com credential - push once first")


def call(method, url, tok, data=None, ctype="application/json", raw=False):
    req = urllib.request.Request(url, method=method)
    req.add_header("Authorization", f"Bearer {tok}")
    req.add_header("Accept", "application/vnd.github+json")
    req.add_header("User-Agent", f"ACECM/{VERSION}")
    body = None
    if data is not None:
        body = data if raw else json.dumps(data).encode()
        req.add_header("Content-Type", ctype)
    try:
        with urllib.request.urlopen(req, body, timeout=300) as r:
            return json.loads(r.read() or b"{}")
    except urllib.error.HTTPError as e:
        detail = e.read().decode("utf-8", "replace")[:400]
        raise SystemExit(f"{method} {url.split('?')[0]} -> {e.code}: {detail}")


def main():
    tag = f"v{VERSION}"
    exe = os.path.join(HERE, "dist", "ACECM.exe")
    sha = exe + ".sha256"
    if not os.path.isfile(exe):
        raise SystemExit(f"missing {exe} - run build.py first")
    # ⚠ ALWAYS recompute from the exe we are about to upload. Reading a
    # checksum file left on disk publishes the hash of whatever was built
    # LAST TIME: the exe is then correct, the checksum is not, and the in-app
    # updater rejects every download of this release as corrupt. Shipped
    # exactly that once - the release looked complete and the update failed.
    import hashlib
    digest = hashlib.sha256(open(exe, "rb").read()).hexdigest()
    with open(sha, "w", encoding="utf-8") as f:
        f.write(f"{digest}  {os.path.basename(exe)}\n")
    print(f"sha256 {digest}")

    # ⚠ Ship a ZIP as well as the bare exe. Chrome and Edge add their own
    # "isn't commonly downloaded / may be dangerous" block on a raw .exe
    # regardless of what is inside it, and that is where people were giving
    # up. The same bytes inside a zip download without the interstitial. The
    # exe stays because the in-app updater fetches ACECM.exe by name.
    import zipfile
    zip_path = os.path.join(HERE, "dist", "ACECM.zip")
    with zipfile.ZipFile(zip_path, "w", zipfile.ZIP_DEFLATED) as z:
        z.write(exe, "ACECM.exe")
    print(f"zip {os.path.getsize(zip_path) / 1e6:.1f} MB")

    notes = ""
    if "--notes-file" in sys.argv:
        notes = open(sys.argv[sys.argv.index("--notes-file") + 1],
                     encoding="utf-8").read()
    # ⚠ Always append the download guidance. ACECM is not code-signed, so
    # Windows reports an unknown publisher on every new release. That is a
    # reputation warning, not a detection - Defender scans the build clean.
    # Saying so on every release, with a checksum to check against, is the
    # difference between someone continuing and someone assuming the worst.
    notes = (notes.rstrip() + "\n\n" if notes.strip() else "") + _DOWNLOAD_HELP.format(digest=digest)

    tok = token()
    # ⚠ Do not DELETE the release. Recreating the same tag leaves
    # github.com/.../releases/download/<tag>/ACECM.exe pointing at a
    # dead blob ("The specified blob does not exist") until the CDN
    # catches up — which is what the in-app updater was hitting.
    rel = None
    try:
        rel = call("GET", f"{API}/repos/{REPO}/releases/tags/{tag}", tok)
        print(f"{tag} already exists, replacing assets")
        for a in rel.get("assets") or []:
            call("DELETE",
                 f"{API}/repos/{REPO}/releases/assets/{a['id']}", tok)
            print(f"  removed old {a['name']}")
        if notes:
            call("PATCH", f"{API}/repos/{REPO}/releases/{rel['id']}", tok,
                 {"body": notes, "name": tag})
    except SystemExit:
        rel = None

    if not rel:
        rel = call("POST", f"{API}/repos/{REPO}/releases", tok, {
            "tag_name": tag, "name": tag, "body": notes,
            "draft": False, "prerelease": False,
        })
        print(f"created {rel['html_url']}")
    else:
        print(f"updating {rel['html_url']}")

    upload = rel["upload_url"].split("{")[0]
    for path in (exe, sha, zip_path):
        name = os.path.basename(path)
        with open(path, "rb") as f:
            blob = f.read()
        call("POST", f"{upload}?name={name}", tok, blob,
             ctype="application/octet-stream", raw=True)
        print(f"  uploaded {name} ({len(blob)/1e6:.1f} MB)")
    print("done")


if __name__ == "__main__":
    main()
