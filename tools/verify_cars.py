"""Load and render EVERY car the viewer can see, and report what broke.

    python tools/verify_cars.py [--out DIR] [--only SUBSTR]

Cars are read straight out of their .kspkg - the base game plus every
installed mod - so the sweep writes nothing but the thumbnails. A car is
flagged only when something is measurably wrong, not merely ugly:

    no geometry            nothing loaded
    tyres != 4             a wheel failed to place
    unbound materials      a batch referenced a material we did not build
    load/render failure    crash, timeout, bad asset
"""
import argparse
import json
import os
import re
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from acecm import viewer                                    # noqa: E402

SCENE = re.compile(r"scene: (\d+) mesh\(es\), (\d+) triangles, (\d+) fallback")
CONTENT = re.compile(r"content: tyres=(\d+) rims=(\d+) anims=(\d+) bones=(\d+)")
PAINTS = re.compile(r"paints: (\d+)")
LIVERY = re.compile(r"liveries: (\d+)")
LOADED = re.compile(r"(\d+) mesh\(es\), (\d+) texture\(s\), (\d+) material\(s\)")


def render(pkg, car_id, out_png, exe, base=None, timeout=300):
    cmd = [exe, pkg, "--car", car_id, "--shot", out_png, "--size", "640x430",
           "--yaw", "2.35", "--pitch", "0.17"]
    # a mod package holds only its own car; the base game supplies the shared
    # tyre material and paints
    if base and os.path.abspath(base) != os.path.abspath(pkg):
        cmd += ["--base", base]
    try:
        p = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout,
                           cwd=os.path.dirname(exe))
    except subprocess.TimeoutExpired:
        return {"ok": False, "why": "render timed out"}
    txt = (p.stdout or "") + (p.stderr or "")
    if p.returncode != 0:
        lines = [l for l in txt.splitlines() if l.strip()]
        why = next((l for l in lines if "panic" in l.lower()), lines[-1] if lines else "")
        return {"ok": False, "why": f"exit {p.returncode}: {why[:110]}"}
    got = {"ok": True}
    m = SCENE.search(txt)
    if m:
        got["meshes"] = int(m.group(1))
        got["tris"] = int(m.group(2))
        got["fallback"] = int(m.group(3))
    m = CONTENT.search(txt)
    if m:
        got["tyres"], got["rims"] = int(m.group(1)), int(m.group(2))
        got["anims"], got["bones"] = int(m.group(3)), int(m.group(4))
    m = LOADED.search(txt)
    if m:
        got["textures"], got["materials"] = int(m.group(2)), int(m.group(3))
    m = PAINTS.search(txt)
    got["paints"] = int(m.group(1)) if m else 0
    m = LIVERY.search(txt)
    got["liveries"] = int(m.group(1)) if m else 0
    return got


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default="verify_out")
    ap.add_argument("--only", default="")
    a = ap.parse_args()

    exe = viewer.viewer_exe()
    if not exe:
        raise SystemExit("evoview.exe not found")
    # ⚠ absolute: the viewer runs with its own working directory, so a
    # relative output path silently lands somewhere else (or fails)
    a.out = os.path.abspath(a.out)
    os.makedirs(a.out, exist_ok=True)

    cars = viewer.index().get("cars", [])
    if a.only:
        cars = [c for c in cars if a.only.lower() in c["id"].lower()]
    print(f"{len(cars)} car(s)\n")

    results = []
    for i, car in enumerate(cars, 1):
        cid = car["id"]
        png = os.path.join(a.out, cid + ".png")
        t0 = time.time()
        row = {"id": cid, "label": car["label"], "mod": car["mod"]}
        row.update(render(car["pkg"], cid, png, exe, base=viewer.package()))
        row["seconds"] = round(time.time() - t0, 1)

        flags = []
        if not row.get("ok"):
            flags.append(row.get("why", "render failed"))
        else:
            if not row.get("tris"):
                flags.append("no geometry")
            if row.get("tyres", 0) != 4:
                flags.append(f"{row.get('tyres', 0)} tyres")
            if row.get("rims", 0) != 4:
                flags.append(f"{row.get('rims', 0)} rims")
            if not row.get("paints") and not row.get("liveries"):
                flags.append("no colour: not painted and no livery")
            if row.get("fallback", 0):
                flags.append(f"{row['fallback']} fallback batch(es)")
        row["flags"] = flags
        results.append(row)
        print(f"[{i:>2}/{len(cars)}] {'ok ' if not flags else '!! '}{cid:36} "
              f"{row.get('tris', 0):>9,} tris {row.get('anims', 0):>3} anim "
              f"{row['seconds']:>5}s  {'; '.join(flags)}")

    with open(os.path.join(a.out, "results.json"), "w", encoding="utf-8") as f:
        json.dump(results, f, indent=1)
    bad = [r for r in results if r["flags"]]
    print(f"\n{len(results) - len(bad)}/{len(results)} clean, {len(bad)} flagged")
    for r in bad:
        print(f"  {'MOD ' if r['mod'] else '    '}{r['id']:36} {'; '.join(r['flags'])}")


if __name__ == "__main__":
    main()
