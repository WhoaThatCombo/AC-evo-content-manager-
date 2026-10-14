"""Every list-row thumbnail, Rust vs Python: same status, same bytes.

    python native/tools/diff_thumbs.py [--base http://127.0.0.1:8092]
"""
import argparse
import hashlib
import json
import sys
import urllib.error
import urllib.parse
import urllib.request


def get(base, route, force):
    req = urllib.request.Request(base + route)
    if force:
        req.add_header("X-ACECM-Force-Python", "1")
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            return r.status, hashlib.sha1(r.read()).hexdigest()[:12]
    except urllib.error.HTTPError as e:
        return e.code, ""


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", default="http://127.0.0.1:8092")
    a = ap.parse_args()
    cars = json.load(urllib.request.urlopen(a.base + "/api/cars"))["cars"]
    tracks = json.load(urllib.request.urlopen(a.base + "/api/tracks"))["tracks"]
    q = urllib.parse.quote
    routes = [f"/api/thumb/car?id={q(c.get('model') or c['id'])}&preset={q(c['id'])}" for c in cars]
    routes += [f"/api/thumb/car?id={q(c['id'])}" for c in cars[:20]]
    routes += [f"/api/thumb/track?folder={q(t['track'])}" for t in tracks]
    bad = hits = 0
    for r in routes:
        x, y = get(a.base, r, False), get(a.base, r, True)
        hits += x[0] == 200
        if x != y:
            bad += 1
            print(f"DIFF {r}: rust={x} python={y}")
    print(f"{len(routes)} thumbnails, {hits} images, {bad} differ")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
