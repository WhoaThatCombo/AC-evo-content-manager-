"""Prove a ported route answers like the Python one it replaces.

    python native/tools/diff_api.py [--base http://127.0.0.1:8092] [ROUTE ...]

Run against a DEBUG native shell: each GET is sent twice, once normally
(answered in Rust if ported) and once with X-ACECM-Force-Python (always the
Python backend), and the JSON is diffed. Timing fields are ignored.
Exit status 1 if anything differs.
"""
import argparse
import json
import sys
import urllib.request

ROUTES = [
    "/api/config",
    "/api/detect",
    "/api/auth",
    "/api/version",
    "/api/logs?lines=200",
    "/api/logs?lines=50&file=native_backend.log",
    "/api/logs?file=..%5C..%5Cconfig.json",
    "/api/logs/files",
]
# values that legitimately differ between two calls
VOLATILE = {"took_ms", "ms", "found_at", "at", "age", "now"}
# known, correct differences: the hidden backend reports ITS private port
# (env overrides); the shell reports the public one, which is the real answer
EXPECTED = {"/api/config": {"ui_port", "listen"}}


def get(base, route, force):
    req = urllib.request.Request(base + route)
    if force:
        req.add_header("X-ACECM-Force-Python", "1")
    with urllib.request.urlopen(req, timeout=60) as r:
        return r.status, json.loads(r.read() or b"null")


def diff(a, b, at=""):
    out = []
    if isinstance(a, dict) and isinstance(b, dict):
        for k in sorted(set(a) | set(b)):
            if k in VOLATILE:
                continue
            if k not in a:
                out.append(f"{at}.{k}: only in python = {b[k]!r}")
            elif k not in b:
                out.append(f"{at}.{k}: only in rust = {a[k]!r}")
            else:
                out += diff(a[k], b[k], f"{at}.{k}")
    elif isinstance(a, list) and isinstance(b, list):
        if len(a) != len(b):
            out.append(f"{at}: length rust={len(a)} python={len(b)}")
        for i, (x, y) in enumerate(zip(a, b)):
            out += diff(x, y, f"{at}[{i}]")
    elif a != b:
        out.append(f"{at}: rust={a!r} python={b!r}")
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", default="http://127.0.0.1:8092")
    ap.add_argument("routes", nargs="*")
    a = ap.parse_args()
    bad = 0
    for route in a.routes or ROUTES:
        try:
            s1, rust = get(a.base, route, False)
            s2, py = get(a.base, route, True)
        except Exception as ex:                    # noqa: BLE001
            print(f"ERR  {route}: {ex}")
            bad += 1
            continue
        skip = EXPECTED.get(route, set())
        d = [x for x in diff(rust, py)
             if x.split(":")[0].lstrip(".") not in skip]
        if s1 != s2:
            d.insert(0, f"status rust={s1} python={s2}")
        print(("OK   " if not d else "DIFF ") + route)
        for line in d[:20]:
            print("       " + line)
        bad += bool(d)
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
