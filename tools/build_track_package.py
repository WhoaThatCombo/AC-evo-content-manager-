"""Build a shareable installer package for a custom AC EVO track.

Run this on the machine that already has the track working. It produces a
folder (and optionally a .zip) containing everything another player needs:

    <out>/
      install_track.py      the installer they run
      install_track.bat     double-click launcher
      manifest.json         track name, folder, slot map
      tracks_entry.bin      their tracks.table entry (for singleplayer)
      override/             the entry-point files renamed to the host slot
      track/<folder>/       the track's own art + data

Why the override exists: a server broadcasts the track's PATHS, not its name,
and the server can only host a custom track by borrowing an existing track's
slots (new paths are invisible to the engine's hash lookup). So a joining
client must find the custom track at the HOST slot's paths - loose files,
which win over the archive.
"""

import argparse
import json
import os
import shutil
import sys
import zipfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import penalties_tool as pt
import server_track_inject as sti

HERE = os.path.dirname(os.path.abspath(__file__))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--src", default=os.path.join(os.path.expanduser("~"), "Saved Games", "ACE", "mods", "content", "tracks"))
    ap.add_argument("--name", default="Alabama Racing Course")
    ap.add_argument("--layout", default="layout")
    ap.add_argument("--out", default=os.path.join(os.path.expanduser("~"), "Downloads", "track_package"))
    ap.add_argument("--zip", action="store_true", help="also produce a .zip")
    args = ap.parse_args()

    folder = os.path.basename(args.src.rstrip("\\/"))
    out = args.out
    if os.path.isdir(out):
        shutil.rmtree(out)
    os.makedirs(out)

    # ---- 1. the track's own files (art + data) ----------------------------
    dst_track = os.path.join(out, "track", folder)
    print(f"copying track art -> track/{folder} ...")
    shutil.copytree(args.src, dst_track)
    total = sum(os.path.getsize(os.path.join(r, f))
                for r, _, fs in os.walk(dst_track) for f in fs)
    print(f"  {total/1024/1024:.0f} MB")

    # ---- 2. the host-slot override ----------------------------------------
    print("building override/ ...")
    n = 0
    slot_map = sti.build_slot_map(args.src)
    for rel, target in slot_map.items():
        src = os.path.join(args.src, rel)
        if not os.path.isfile(src):
            print(f"  !! missing {rel}")
            continue
        dst = os.path.join(out, "override", target)
        os.makedirs(os.path.dirname(dst), exist_ok=True)
        shutil.copy2(src, dst)
        n += 1
    tsrc = os.path.join(args.src, folder + ".track")
    if os.path.isfile(tsrc):
        dst = os.path.join(out, "override", sti.RA_TRACK)
        os.makedirs(os.path.dirname(dst), exist_ok=True)
        shutil.copy2(tsrc, dst)
        n += 1
    print(f"  {n} files")

    # ---- 3. tracks.table entry (so it also shows in singleplayer) ---------
    print("extracting tracks.table entry ...")
    cli = pt.find_client_kspkg()
    entry = None
    if cli:
        tbl = sti.get_entry(cli, sti.TRACKS_TABLE)
        for e in sti.track_entries(tbl):
            if args.name.lower() in sti.entry_name(e).lower():
                entry = e
                break
    if entry:
        with open(os.path.join(out, "tracks_entry.bin"), "wb") as f:
            f.write(entry)
        print(f"  OK ({len(entry)} bytes)")
    else:
        print("  !! not found - installer will skip singleplayer registration")

    # ---- 3b. track_containers.table entries (for server deploy) -----------
    # Without these a receiving SERVER can't build the layout/spawnpoints/
    # timelines containers for this track - do_override() normally pulls
    # them live from a client that's already imported the track via
    # EvoForge, which a package recipient may not have. Bundling them here
    # makes the package fully self-installable on a server that has never
    # seen this track before.
    print("extracting track_containers.table entries ...")
    n_containers = 0
    if cli:
        ct = sti.get_entry(cli, sti.CONTAINERS_TABLE)
        needle = folder.lower().encode()
        wanted = [e for e in sti.container_entries(ct) if needle in bytes(e).lower()]
        if wanted:
            with open(os.path.join(out, "containers.bin"), "wb") as f:
                for e in wanted:
                    f.write(len(e).to_bytes(4, "little"))
                    f.write(e)
            n_containers = len(wanted)
            print(f"  OK ({n_containers} entries)")
        else:
            print("  !! no matching container entries - server deploy won't have "
                  "layout/spawnpoints/timelines for this track")
    else:
        print("  !! no client kspkg found - skipped")

    # ---- 4. manifest + installer ------------------------------------------
    manifest = {
        "display_name": args.name,
        "layout": args.layout,
        "folder": folder,
        "slot_map": dict(slot_map),
        "host_track": sti.RA_TRACK,
        "has_containers": n_containers > 0,
    }
    with open(os.path.join(out, "manifest.json"), "w", encoding="utf-8") as f:
        json.dump(manifest, f, indent=2)

    for fn in ("install_track.py", "install_track.bat"):
        shutil.copy2(os.path.join(HERE, fn), os.path.join(out, fn))

    print(f"\nOK  package at {out}")
    if args.zip:
        zp = out.rstrip("\\/") + ".zip"
        print(f"zipping -> {zp} (this takes a minute) ...")
        with zipfile.ZipFile(zp, "w", zipfile.ZIP_DEFLATED, compresslevel=6) as z:
            for r, _, fs in os.walk(out):
                for fn in fs:
                    full = os.path.join(r, fn)
                    z.write(full, arcname=os.path.relpath(full, out))
        print(f"OK  {os.path.getsize(zp)/1024/1024:.0f} MB")


if __name__ == "__main__":
    main()
