"""Build a loose content/tracks/eifel folder from shipped Nürburgring kit.

Official Free Roam / Cruise point at content\\tracks\\eifel\\eifel.scene,
which is not in content.kspkg. The world that *did* ship is nurburgring
containers (base + Adenau + boulevard + industrial + parks).

This writes a tiny catalog scene whose ContainerActorData paths point at
those existing files. evoview (and the game, via loose-file absence fill)
can then open `eifel` without copying a gigabyte of meshes.

    python tools/build_eifel_map.py [--kspkg PATH] [--out DIR]

Default out: %%USERPROFILE%%\\Saved Games\\ACE\\mods\\content\\tracks\\eifel
"""
from __future__ import annotations

import argparse
import json
import os
import struct
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "..", "acecm"))
from acecm import kspkg  # noqa: E402
from acecm.tracktables import emit  # noqa: E402

DEFAULT_KSPKG = os.path.join(
    os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)"),
    "Steam", "steamapps", "common", "Assetto Corsa EVO", "content.kspkg",
)
DEFAULT_OUT = os.path.join(
    os.path.expanduser("~"), "Saved Games", "ACE", "mods", "content", "tracks", "eifel",
)
NURB_SCENE = r"content\tracks\nurburgring\nurburgring.scene"
NURB_TRACK = r"content\tracks\nurburgring\nurburgring.track"


def varint(d, i):
    r = 0
    s = 0
    while i < len(d):
        b = d[i]
        i += 1
        r |= (b & 0x7F) << s
        if b & 0x80 == 0:
            return r, i
        s += 7
        if s > 63:
            return None, i
    return None, i


def fields(d, start, end):
    i = start
    end = min(end, len(d))
    while i < end:
        key, i = varint(d, i)
        if key is None:
            return
        fnum, wt = key >> 3, key & 7
        if wt == 0:
            v, i = varint(d, i)
            if v is None:
                return
            yield fnum, 0, v
        elif wt == 1:
            if i + 8 > end:
                return
            yield fnum, 1, d[i : i + 8]
            i += 8
        elif wt == 2:
            n, i = varint(d, i)
            if n is None or i + n > end:
                return
            yield fnum, 2, (i, i + n)
            i += n
        elif wt == 5:
            if i + 4 > end:
                return
            yield fnum, 5, struct.unpack_from("<f", d, i)[0]
            i += 4
        else:
            return


def text(d, r):
    a, b = r
    return d[a:b].decode("utf-8", "replace")


def skip_logic(path: str) -> bool:
    l = path.lower()
    return any(
        k in l
        for k in (
            "spawnpoint",
            "timeline",
            "camera_sequence",
            "common_car_cam",
            "tv1_camera",
            "tv2_camera",
            "tv_camera",
            "fmod_",
            "dlp_",
            "physics_20",
            "drs_zone",
        )
    )


def keep_container(path: str, on_demand: bool) -> bool:
    """Always-on kit + Eifel world. Skip race-only GP variants and logic."""
    if skip_logic(path):
        return False
    n = path.replace("/", "\\").rsplit("\\", 1)[-1].lower()
    if n.endswith(".scene"):
        n = n[: -len(".scene")]
    world = any(
        k in n
        for k in (
            "boulevard",
            "adenau",
            "industrial",
            "bistrot",
            "people_",
            "npc_",
            "temp_people",
            "spawns_ow",
            "park_entrance_open",
            "park_nordschleife_open",
            "park_new_exit_open",
            "cruise",
            "tourist",
            "nurburgring_base",
            "custom_vegetation",
            "big_screens",
            "bridges_nords",
            "bridges_gp",
            "grass_",
            "clover_",
            "flowers_",
            "misc_",
            "nordschleife_race",
            "race_scenery_nordschleife",
            "layout_nordschleife",
        )
    )
    if n.startswith("layout_") and "nordschleife" not in n and "cruise" not in n:
        return False
    if "park" in n and "closed" in n:
        return False
    if not on_demand:
        return True
    return world


def list_nurb_containers(scene: bytes):
    always, demand = [], []
    for f, wt, v in fields(scene, 0, len(scene)):
        if f != 2 or wt != 2:
            continue
        actor = None
        for af, awt, av in fields(scene, v[0], v[1]):
            if af == 50 and awt == 2:
                actor = av
        if not actor:
            continue
        for kf, kwt, kv in fields(scene, actor[0], actor[1]):
            if kf != 121 or kwt != 2:
                continue
            path, on_demand = "", False
            for mf, mwt, mv in fields(scene, kv[0], kv[1]):
                if mf == 1 and mwt == 2:
                    path = text(scene, mv)
                elif mf == 3 and mwt == 0:
                    on_demand = bool(mv)
            if not path:
                continue
            (demand if on_demand else always).append(path)
    return always, demand


def actor_container(name: str, path: str) -> bytes:
    cont = emit(1, 2, path.encode()) + emit(3, 0, 0)  # loaded_on_demand = false
    actor = emit(121, 2, cont)
    body = emit(1, 2, name.encode()) + emit(2, 0, 1) + emit(50, 2, actor)
    return emit(2, 2, body)


def build_scene(paths: list[str]) -> bytes:
    gfx = emit(20, 0, 1)  # SceneGraphicsSettings.is_open_world
    actors = b"".join(
        actor_container(os.path.basename(p).rsplit(".", 1)[0], p) for p in paths
    )
    return actors + emit(3, 2, gfx)


def read_kspkg_file(pkg: str, want: str) -> bytes | None:
    want = want.lower()
    for path, size, off in kspkg.iter_entries(pkg):
        if path.lower() == want and size:
            with open(pkg, "rb") as f:
                return kspkg.read_entry(f, size, off, path)
    return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--kspkg", default=DEFAULT_KSPKG)
    ap.add_argument("--out", default=DEFAULT_OUT)
    args = ap.parse_args()
    if not os.path.isfile(args.kspkg):
        raise SystemExit(f"no kspkg at {args.kspkg}")

    raw = read_kspkg_file(args.kspkg, NURB_SCENE)
    if not raw:
        raise SystemExit("nurburgring.scene not in kspkg")
    always, demand = list_nurb_containers(raw)
    picked = []
    seen = set()
    for p in always + demand:
        if p.lower() in seen:
            continue
        on_demand = p in demand
        if not keep_container(p, on_demand):
            continue
        seen.add(p.lower())
        picked.append(p)
    print(f"catalog: {len(picked)} containers ({len(always)} always + world)")
    for p in picked:
        print(" ", p)

    os.makedirs(args.out, exist_ok=True)
    scene = build_scene(picked)
    scene_path = os.path.join(args.out, "eifel.scene")
    open(scene_path, "wb").write(scene)
    print(f"wrote {scene_path} ({len(scene)} bytes)")

    track = read_kspkg_file(args.kspkg, NURB_TRACK)
    if track:
        open(os.path.join(args.out, "eifel.track"), "wb").write(track)
        print(f"wrote eifel.track ({len(track)} bytes) cloned from nurburgring")

    man = {
        "name": "eifel",
        "display_name": "Eifel (ACECM freeroam)",
        "folder": "eifel",
        "source": "nurburgring containers",
        "containers": picked,
        "note": (
            "Catalog only. Meshes stay in content.kspkg. evoview needs --base. "
            "The game may resolve this via loose-file absence fill when Free "
            "Roam asks for content\\tracks\\eifel\\eifel.scene."
        ),
    }
    open(os.path.join(args.out, "manifest.json"), "w", encoding="utf-8").write(
        json.dumps(man, indent=2)
    )
    print("out", args.out)


if __name__ == "__main__":
    main()
