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
    for f in (exe, sha):
        if not os.path.isfile(f):
            raise SystemExit(f"missing {f} - run build.py first")

    notes = ""
    if "--notes-file" in sys.argv:
        notes = open(sys.argv[sys.argv.index("--notes-file") + 1],
                     encoding="utf-8").read()

    tok = token()
    # a re-run should replace the release, not fail or duplicate it
    try:
        old = call("GET", f"{API}/repos/{REPO}/releases/tags/{tag}", tok)
        print(f"{tag} already exists, replacing it")
        call("DELETE", f"{API}/repos/{REPO}/releases/{old['id']}", tok)
    except SystemExit:
        pass

    rel = call("POST", f"{API}/repos/{REPO}/releases", tok, {
        "tag_name": tag, "name": tag, "body": notes,
        "draft": False, "prerelease": False,
    })
    print(f"created {rel['html_url']}")

    upload = rel["upload_url"].split("{")[0]
    for path in (exe, sha):
        name = os.path.basename(path)
        with open(path, "rb") as f:
            blob = f.read()
        call("POST", f"{upload}?name={name}", tok, blob,
             ctype="application/octet-stream", raw=True)
        print(f"  uploaded {name} ({len(blob)/1e6:.1f} MB)")
    print("done")


if __name__ == "__main__":
    main()
