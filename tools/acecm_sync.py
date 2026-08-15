"""ACECM Sync - install what a modded EVO server requires, then join it.

Give it the address of someone's ACECM and it will list their servers, download
only the content you are missing, put each file where the game actually looks
for it, and print the join link.

    python acecm_sync.py http://their-host:8092            list servers
    python acecm_sync.py http://their-host:8092 srv123     sync that server

Why this exists: AC EVO cannot deliver content itself. A client without the
right car or track is simply rejected (CONTENT_UNAVAILABLE), and no field in the
protocol carries a download URL. So the content has to arrive out-of-band, and
this is that step - the same shape Content Manager uses for AC1.

Nothing here touches the game's own files: car mods go to your mods folder,
track packages to Downloads for you to deploy deliberately.
"""
import hashlib
import json
import os
import sys
import urllib.parse
import urllib.request

CLIENT_MODS = os.path.join(os.path.expanduser("~"), "Saved Games", "ACE", "mods")
TRACK_DEST = os.path.join(os.path.expanduser("~"), "Downloads")


def get_json(url):
    with urllib.request.urlopen(url, timeout=20) as r:
        return json.load(r)


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def human(n):
    for unit in ("B", "KB", "MB", "GB"):
        if n < 1024:
            return f"{n:.0f} {unit}"
        n /= 1024
    return f"{n:.1f} TB"


def destination(entry):
    """Where a manifest path belongs on THIS machine."""
    p = (entry.get("path") or "").replace("\\", "/").lstrip("/")
    parts = [x for x in p.split("/") if x and x != "."]
    if not parts or ".." in parts:
        raise ValueError("bad content path")
    def under(root, extra):
        root = os.path.abspath(root)
        dest = os.path.abspath(os.path.join(root, *extra))
        if os.path.commonpath([root, dest]) != root:
            raise ValueError("path escapes content folder")
        return dest
    if parts[0] == "mods":
        return under(CLIENT_MODS, parts[1:2])
    if parts[0] == "tracks":
        return under(TRACK_DEST, parts[1:])
    return under(TRACK_DEST, [parts[-1]])


def download(url, dest, expect, size):
    os.makedirs(os.path.dirname(dest), exist_ok=True)
    tmp = dest + ".part"
    done = 0
    with urllib.request.urlopen(url, timeout=60) as r, open(tmp, "wb") as fh:
        while True:
            chunk = r.read(1 << 20)
            if not chunk:
                break
            fh.write(chunk)
            done += len(chunk)
            pct = (done / size * 100) if size else 0
            print(f"\r      {human(done)} / {human(size)}  {pct:5.1f}%",
                  end="", flush=True)
    print()
    got = sha256(tmp)
    if expect and got != expect:
        os.remove(tmp)
        return False, f"checksum mismatch (got {got[:12]}, expected {expect[:12]})"
    os.replace(tmp, dest)
    return True, "ok"


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        return 1
    base = sys.argv[1].rstrip("/")
    listing = get_json(base + "/api/registry/list")
    servers = listing.get("servers", [])

    if len(sys.argv) < 3:
        if not servers:
            print("no public servers listed there")
            return 0
        print(f"{len(servers)} server(s) at {base}\n")
        for s in servers:
            print(f"  {s['id']:12} {s['name']}")
            if s.get("description"):
                print(f"               {s['description']}")
            print(f"               {s['ip']}:{s['port']}  "
                  f"content {human(s['content_bytes'])}  "
                  f"mods {len(s['required_mods'])} tracks {len(s['required_tracks'])}")
        print("\nrun again with a server id to sync it")
        return 0

    sid = sys.argv[2]
    man = get_json(f"{base}/api/registry/manifest?id={urllib.parse.quote(sid)}")
    if not man.get("ok"):
        print("error:", man.get("error"))
        return 1
    srv = man["server"]
    print(f"{srv['name']}  ({srv['ip']}:{srv['port']})")
    if man.get("missing_locally"):
        print("  ! the host is missing:", ", ".join(man["missing_locally"]))

    need = []
    for f in man["files"]:
        dest = destination(f)
        if os.path.isfile(dest) and os.path.getsize(dest) == f["size"]:
            if not f.get("sha256") or sha256(dest) == f["sha256"]:
                continue                      # already have it
        need.append((f, dest))

    if not need:
        print("\n  everything already installed")
    else:
        total = sum(f["size"] for f, _ in need)
        print(f"\n  {len(need)} file(s) to fetch, {human(total)}")
        for f, dest in need:
            print(f"   {f['path']}")
            ok, msg = download(f["url"], dest, f.get("sha256"), f["size"])
            if not ok:
                print(f"      FAILED: {msg}")
                return 1

    print(f"\njoin link (copy to clipboard, then use the clipboard button in "
          f"the server browser):\n  join:{srv['ip']}:{srv['port']}")
    try:
        import subprocess
        subprocess.run(["powershell.exe", "-NoProfile", "-Command",
                        f"Set-Clipboard -Value 'join:{srv['ip']}:{srv['port']}'"],
                       capture_output=True, timeout=15)
        print("  (copied to your clipboard)")
    except Exception:
        pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
