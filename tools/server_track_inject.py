"""Inject a custom track into the AC EVO DEDICATED SERVER's content.kspkg.

Why this exists
---------------
The dedicated server does NOT read a mods folder and has no loose-file
support at all (no fastStorage / "non-packed" code paths in its exe, and a
mods\\*.kspkg probe left the startup "Sha unique content" hash byte-identical).
Everything it loads must live inside its own content.kspkg.

The server also needs far less of a track than the client: no meshes, no
textures - just the logic files (root .scene, .track, and the container
scenes: layout, spawnpoints, timelines...). For Alabama that's ~7.8 MB.

Track identity lives in system\\tracks.table: top-level field 2, holding a
repeated field 3, one entry per track (display name, folder, .scene, nation,
coords, .track, continent). EvoForge registers a client-side track by
rewriting exactly this table inside the client's content.kspkg - we do the
same thing on the server.

Everything is backed up before writing, and --restore puts it back.
"""

import argparse
import json
import os
import struct
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import penalties_tool as pt

TRACKS_TABLE = r"system\tracks.table"
BAK_SUFFIX = ".trackinject.bak"


# ---------------------------------------------------------------- kspkg I/O
def decrypt_table(raw):
    key = pt.KSPKG_KEY * (pt.TABLE_SIZE // len(pt.KSPKG_KEY))
    return (int.from_bytes(raw, "big") ^ int.from_bytes(key, "big")).to_bytes(
        pt.TABLE_SIZE, "big")


def read_table_raw(kspkg):
    with open(kspkg, "rb") as f:
        f.seek(0, os.SEEK_END)
        table_start = f.tell() - pt.TABLE_SIZE
        f.seek(table_start)
        return f.read(pt.TABLE_SIZE), table_start


def find_bucket_fast(dec, path):
    """Bucket index for `path`, using the wholesale-decrypted table."""
    needle = path.encode("utf-8")
    for i in range(0, pt.TABLE_SIZE, pt.BUCKET_SIZE):
        b = dec[i:i + pt.BUCKET_SIZE]
        if b[0] == 0:
            continue
        z = b.find(0)
        if z > 0 and b[:z].lower() == needle.lower():
            return i // pt.BUCKET_SIZE
    return None


def write_entry_relocating(kspkg, path, content):
    """Replace `path`'s content with `content` of ANY size.

    The new bytes are parked where the hash table currently starts and the
    table is rewritten after them, so the archive grows by len(content).
    The bucket keeps its slot - only size/offset change - which matters
    because the game finds entries by their own hash of the path, so a
    brand-new bucket would not be found."""
    raw, table_start = read_table_raw(kspkg)
    dec = bytearray(decrypt_table(raw))
    bi = find_bucket_fast(dec, path)
    if bi is None:
        raise SystemExit(f"!! {path} not found in {kspkg}")

    off = bi * pt.BUCKET_SIZE
    struct.pack_into("<I", dec, off + 240, len(content))
    struct.pack_into("<Q", dec, off + 248, table_start)

    key = pt.KSPKG_KEY * (pt.TABLE_SIZE // len(pt.KSPKG_KEY))
    enc = (int.from_bytes(bytes(dec), "big") ^ int.from_bytes(key, "big")).to_bytes(
        pt.TABLE_SIZE, "big")

    with open(kspkg, "r+b") as f:
        f.seek(table_start)
        f.write(pt.xor_decrypt(content, 0))
        f.write(enc)
        f.truncate()


# ------------------------------------------------------------ tracks.table
def get_entry(kspkg, path):
    idx = pt.build_index(kspkg)
    rec = idx.get(path.lower())
    if not rec:
        raise SystemExit(f"!! {path} missing from {kspkg}")
    return pt.read_at(kspkg, rec[1], rec[2])


def track_entries(table_bytes):
    """[(entry_bytes)] - the repeated field 3 inside top-level field 2."""
    top = pt.pb_walk(table_bytes)
    inner = next(v for f, w, v in top if f == 2 and w == 2)
    return [v for f, w, v in pt.pb_walk(inner) if f == 3 and w == 2]


def build_table(entries):
    inner = b"".join(pt.pb_field(3, 2, e) for e in entries)
    return pt.pb_field(2, 2, inner)


def entry_name(e):
    """Display name of a track entry.

    Layout: entry -> f2 -> { f1 display name, f3 folder, f4 .scene,
    f5 nation, f8 .track, f13 continent }."""
    inner = next((v for f, w, v in (pt.pb_walk(e) or []) if f == 2 and w == 2), None)
    if inner is None:
        return "?"
    name = next((v for f, w, v in (pt.pb_walk(inner) or []) if f == 1 and w == 2), None)
    try:
        return name.decode("utf-8") if name else "?"
    except UnicodeDecodeError:
        return "?"


def entry_paths(e):
    """(folder, scene, track) for a track entry - what the server must hold."""
    inner = next((v for f, w, v in (pt.pb_walk(e) or []) if f == 2 and w == 2), None)
    if inner is None:
        return None, None, None
    got = {}
    for f, w, v in (pt.pb_walk(inner) or []):
        if w == 2 and f in (3, 4, 8):
            try:
                got[f] = v.decode("utf-8")
            except UnicodeDecodeError:
                pass
    return got.get(3), got.get(4), got.get(8)


CONTAINERS_TABLE = r"system\track_containers.table"

# Alabama's logic files -> Road Atlanta slots that already exist in the
# server's hash table. New paths cannot be added: the engine finds entries by
# hashing the path to a bucket, and appended files land in the wrong slot
# (proved by "Failed to find file: ...alabama_r_4e.scene" even though our own
# linear-scan index saw them). Overwriting an existing path always works.
RA_CONTAINERS = r"content\tracks\road_atlanta\containers"

# Which Road Atlanta container slot each kind of container borrows. Matched on
# the source filename's prefix, so layout_layout.scene / layout_drift.scene /
# layout_gp.scene all land in the same slot.
_SLOT_RULES = [
    ("spawnpoints_grid",    RA_CONTAINERS + r"\spawnpoints_grid_gp.scene"),
    ("spawnpoints_pitlane", RA_CONTAINERS + r"\spawnpoints_pitlane_gp.scene"),
    ("spawnpoints_hotlap",  RA_CONTAINERS + r"\spawnpoints_hotlap_gp.scene"),
    ("timelines",           RA_CONTAINERS + r"\timelines_gp.scene"),
    ("layout",              RA_CONTAINERS + r"\layout_gp.scene"),
    ("ground",              RA_CONTAINERS + r"\race_scenery_gp.scene"),
]
# spare slots for any container the rules don't name
_SPARE_SLOTS = [RA_CONTAINERS + r"\marshalls_gp.scene",
                RA_CONTAINERS + r"\pitlane_zones_gp.scene",
                RA_CONTAINERS + r"\tv1_cameras_gp.scene",
                RA_CONTAINERS + r"\tv2_cameras_gp.scene",
                RA_CONTAINERS + r"\big_screens.scene",
                RA_CONTAINERS + r"\starting_positions.scene"]


def build_slot_map(src_dir):
    """{relative source file -> Road Atlanta slot} for one track folder.

    Derived from what the track actually ships rather than hardcoded, because
    the layout container is named after the layout (layout_drift.scene,
    layout_layout.scene, ...) and tracks carry different container sets."""
    folder = os.path.basename(src_dir.rstrip("\\/"))
    smap = {}
    root_scene = folder + ".scene"
    if os.path.isfile(os.path.join(src_dir, root_scene)):
        smap[root_scene] = r"content\tracks\road_atlanta\road_atlanta.scene"

    cdir = os.path.join(src_dir, "containers")
    if not os.path.isdir(cdir):
        return smap
    names = sorted(f for f in os.listdir(cdir) if f.lower().endswith(".scene"))
    used, spares = set(), list(_SPARE_SLOTS)
    for fn in names:
        low = fn.lower()
        target = None
        for prefix, slot in _SLOT_RULES:
            if low.startswith(prefix) and slot not in used:
                target = slot
                break
        if target is None:
            if not spares:
                print(f"   !! no slot left for {fn} - skipped")
                continue
            target = spares.pop(0)
        used.add(target)
        smap[os.path.join("containers", fn)] = target
    return smap


# default kept so older calls still work; real runs use build_slot_map()
SLOT_MAP = {}
RA_FOLDER = r"content\tracks\road_atlanta"
RA_SCENE = r"content\tracks\road_atlanta\road_atlanta.scene"
RA_TRACK = r"content\tracks\road_atlanta\road_atlanta.track"


def repoint_strings(buf, mapping):
    """Rewrite length-delimited string fields via `mapping`, re-encoding as we
    go so differing lengths are fine. Recurses into sub-messages.

    Only descends into a field when it parses AND re-serialises back to the
    exact same bytes - otherwise binary payloads that happen to look like
    valid protobuf would be silently rewritten and corrupted."""
    parsed = pt.pb_walk(buf)
    if parsed is None:
        return buf, 0
    out, n = [], 0
    for f, w, v in parsed:
        if w == 2:
            try:
                s = v.decode("utf-8")
            except UnicodeDecodeError:
                s = None
            if s is not None and s in mapping:
                out.append((f, w, mapping[s].encode("utf-8")))
                n += 1
                continue
            inner = pt.pb_walk(v) if len(v) > 1 else None
            if inner is not None and pt.pb_reencode(inner) == v:
                sub, k = repoint_strings(v, mapping)
                out.append((f, w, sub))
                n += k
            else:
                out.append((f, w, v))
        else:
            out.append((f, w, v))
    return pt.pb_reencode(out), n


def container_entries(table_bytes):
    top = pt.pb_walk(table_bytes)
    inner = next(v for f, w, v in top if f == 2 and w == 2)
    return [v for f, w, v in pt.pb_walk(inner) if f == 3 and w == 2]


def install_package_to_server(server, package_dir):
    """Install a build_track_package.py package straight onto the SERVER's
    content.kspkg, without needing this machine's own client to have
    EvoForge-imported the track first (unlike do_override(), which reads the
    container entries live off a client kspkg). Needs the package's
    containers.bin (added alongside tracks_entry.bin/manifest.json/override/
    - older packages built before that won't have it, see build_track_package.py).

    Server must be STOPPED while this runs - same rule as every other
    content.kspkg write in this toolkit."""
    with open(os.path.join(package_dir, "manifest.json"), encoding="utf-8") as f:
        manifest = json.load(f)
    display_name = manifest["display_name"]
    folder = manifest["folder"]
    slot_map = manifest["slot_map"]
    ala_to_ra = {os.path.join("content", "tracks", folder, rel): tgt
                 for rel, tgt in slot_map.items()}

    # ---- 1. content slots ------------------------------------------------
    print(f"-- content ({folder}: {len(slot_map)} files)")
    for rel, target in slot_map.items():
        src = os.path.join(package_dir, "override", target)
        if not os.path.isfile(src):
            print(f"   !! missing {rel} in package - skipped")
            continue
        with open(src, "rb") as f:
            data = f.read()
        # build_track_package.py's override/ files are only RENAMED to their
        # host slot on disk - their own internal path references still say
        # content\tracks\<folder>\..., same as do_override()'s live-folder
        # source files, and need the same repoint before writing or the game
        # fails to find its own container files at startup (confirmed via a
        # live crash: "Failed to find file: content\tracks\<folder>\...").
        data, nref = repoint_strings(data, ala_to_ra)
        bak = server + f".slot.{os.path.basename(target)}.bak"
        if not os.path.isfile(bak):
            cur = get_entry(server, target)
            with open(bak, "wb") as f:
                f.write(cur)
        write_entry_relocating(server, target, data)
        print(f"   OK  {rel}  ->  {os.path.basename(target)}  "
              f"({len(data)} bytes, {nref} refs repointed)")

    # ---- 2. tracks.table: register (repointed at Road Atlanta) -----------
    print("-- tracks.table")
    entry_path = os.path.join(package_dir, "tracks_entry.bin")
    if not os.path.isfile(entry_path):
        print("   !! no tracks_entry.bin in package - can't register the track name")
    else:
        with open(entry_path, "rb") as f:
            raw_entry = f.read()
        inner = next(v for f, w, v in pt.pb_walk(raw_entry) if f == 2 and w == 2)
        folder_p, scene_p, track_p = entry_paths(raw_entry)
        m = {k: v for k, v in {folder_p: RA_FOLDER, scene_p: RA_SCENE,
                               track_p: RA_TRACK}.items() if k}
        new_inner, n = repoint_strings(inner, m)
        repointed_entry = pt.pb_field(2, 2, new_inner)

        tbl = get_entry(server, TRACKS_TABLE)
        entries = track_entries(tbl)
        if any(display_name.lower() == entry_name(e).lower() for e in entries):
            print(f"   OK  '{display_name}' already registered")
        else:
            entries.append(repointed_entry)
            write_entry_relocating(server, TRACKS_TABLE, build_table(entries))
            print(f"   OK  registered '{display_name}' ({n} paths repointed at road_atlanta)")

    # ---- 3. track_containers.table ----------------------------------------
    print("-- track_containers.table")
    containers_path = os.path.join(package_dir, "containers.bin")
    if not os.path.isfile(containers_path):
        print("   !! no containers.bin in package (older package format) - "
              "the track will register but likely fail to load in-game")
        return
    with open(containers_path, "rb") as f:
        raw = f.read()
    pkg_entries = []
    off = 0
    while off < len(raw):
        ln = int.from_bytes(raw[off:off + 4], "little")
        off += 4
        pkg_entries.append(raw[off:off + ln])
        off += ln

    bak = server + ".containers.bak"
    if not os.path.isfile(bak):
        srv_ct = get_entry(server, CONTAINERS_TABLE)
        with open(bak, "wb") as f:
            f.write(srv_ct)
        print(f"   OK  backed up original -> {os.path.basename(bak)}")
    with open(bak, "rb") as f:
        srv_entries = container_entries(f.read())

    # srv_entries is always freshly rebuilt from the pristine backup above,
    # so appending here and replacing the whole live table is idempotent
    # across repeat calls (same pattern as do_override()'s container step) -
    # no separate "already present" check needed.
    added = 0
    for e in pkg_entries:
        new_e, n = repoint_strings(e, ala_to_ra)
        srv_entries.append(new_e)
        added += 1
    write_entry_relocating(server, CONTAINERS_TABLE, build_table(srv_entries))
    print(f"   OK  server container table: {len(srv_entries)} entries (+{added})")


def do_override(server, client, track_sub, src_dir):
    """Point the server's custom-track entry at Road Atlanta's slots and fill
    those slots with the custom track's logic files."""
    slot_map = build_slot_map(src_dir)
    folder = os.path.basename(src_dir.rstrip("\\/"))
    ala_to_ra = {os.path.join("content", "tracks", folder, rel): tgt
                 for rel, tgt in slot_map.items()}

    # ---- 1. content: overwrite Road Atlanta's slots -------------------------
    print(f"-- content ({folder}: {len(slot_map)} files)")
    for rel, target in slot_map.items():
        src = os.path.join(src_dir, rel)
        if not os.path.isfile(src):
            print(f"   !! missing source {rel} - skipped")
            continue
        with open(src, "rb") as f:
            data = f.read()
        # the copied file still refers to the track's own folder - point those
        # references at the Road Atlanta slots they now actually live in
        data, nref = repoint_strings(data, ala_to_ra)
        bak = server + f".slot.{os.path.basename(target)}.bak"
        if not os.path.isfile(bak):
            cur = get_entry(server, target)
            with open(bak, "wb") as f:
                f.write(cur)
        write_entry_relocating(server, target, data)
        print(f"   OK  {rel}  ->  {os.path.basename(target)}  "
              f"({len(data)} bytes, {nref} refs repointed)")

    # ---- 2. tracks.table: repoint the custom entry at Road Atlanta ----------
    print("-- tracks.table")
    tbl = get_entry(server, TRACKS_TABLE)
    entries = track_entries(tbl)
    rebuilt, hit = [], False
    for e in entries:
        if track_sub.lower() in entry_name(e).lower():
            inner = next(v for f, w, v in pt.pb_walk(e) if f == 2 and w == 2)
            folder, scene, trackp = entry_paths(e)
            m = {folder: RA_FOLDER, scene: RA_SCENE, trackp: RA_TRACK}
            m = {k: v for k, v in m.items() if k}
            new_inner, n = repoint_strings(inner, m)
            rebuilt.append(pt.pb_field(2, 2, new_inner))
            hit = True
            print(f"   OK  '{entry_name(e)}' repointed at road_atlanta ({n} paths)")
        else:
            rebuilt.append(e)
    if not hit:
        raise SystemExit(f"!! no server entry matching {track_sub!r} - run without "
                         f"--override first to register it")
    write_entry_relocating(server, TRACKS_TABLE, build_table(rebuilt))

    # ---- 3. track_containers.table: copy entries, repoint containers --------
    print("-- track_containers.table")
    cli_ct = get_entry(client, CONTAINERS_TABLE)
    srv_ct = get_entry(server, CONTAINERS_TABLE)
    bak = server + ".containers.bak"
    if not os.path.isfile(bak):
        with open(bak, "wb") as f:
            f.write(srv_ct)
        print(f"   OK  backed up original -> {os.path.basename(bak)}")

    cli_entries = container_entries(cli_ct)
    # always rebuild from the pristine backup so re-running can't stack duplicates
    with open(bak, "rb") as f:
        srv_entries = container_entries(f.read())
    # match on the track's FOLDER, not its display name: a name like "Drift"
    # also appears in unrelated base-game entries and would copy those too
    needle = os.path.basename(src_dir.rstrip("\\/")).lower().encode()
    wanted = [e for e in cli_entries if needle in bytes(e).lower()]
    if not wanted:
        raise SystemExit(f"!! client track_containers.table has no entries "
                         f"referencing {needle.decode()}")

    added = 0
    for e in wanted:
        new_e, n = repoint_strings(e, ala_to_ra)
        srv_entries.append(new_e)
        added += 1
        print(f"   OK  copied a layout entry ({n} container paths repointed)")
    write_entry_relocating(server, CONTAINERS_TABLE, build_table(srv_entries))
    print(f"   OK  server container table: {len(srv_entries)} entries (+{added})")


def deploy_client_track(server, client, track_sub, src_dir):
    """One-shot version of the two manual steps (register, then --override)
    for a track this machine's CLIENT has already EvoForge-imported. Safe to
    call even if the track's raw entry is already registered - it just skips
    that step and goes straight to the slot override."""
    srv_tbl = get_entry(server, TRACKS_TABLE)
    cli_tbl = get_entry(client, TRACKS_TABLE)
    srv_entries = track_entries(srv_tbl)
    cli_entries = track_entries(cli_tbl)

    wanted = [e for e in cli_entries if track_sub.lower() in entry_name(e).lower()]
    if not wanted:
        raise ValueError(f"no client track matching {track_sub!r}. "
                         f"Client has: {[entry_name(e) for e in cli_entries]}")
    have = {entry_name(e).lower() for e in srv_entries}
    todo = [e for e in wanted if entry_name(e).lower() not in have]
    if todo:
        bak = server + BAK_SUFFIX
        if not os.path.isfile(bak):
            with open(bak, "wb") as f:
                f.write(srv_tbl)
        new_tbl = build_table(srv_entries + todo)
        write_entry_relocating(server, TRACKS_TABLE, new_tbl)
        print(f"OK  registered {[entry_name(e) for e in todo]}")

    do_override(server, client, track_sub, src_dir)


def do_client_kspkg(client, src_dir):
    """Put the custom track into the CLIENT's road_atlanta slots.

    Loose files do NOT override the archive: when a path exists both packed
    and loose, the packed copy wins (proved by a join where zero loose
    road_atlanta files were opened and the drift map loaded instead). Only
    paths that exist *only* loose - like the track's own art folder - get
    picked up. So the entry points have to be written into the archive, while
    meshes/materials/textures keep their own paths and resolve loose."""
    slot_map = build_slot_map(src_dir)
    folder = os.path.basename(src_dir.rstrip("\\/"))
    ala_to_ra = {os.path.join("content", "tracks", folder, rel): tgt
                 for rel, tgt in slot_map.items()}

    for rel, target in slot_map.items():
        src = os.path.join(src_dir, rel)
        if not os.path.isfile(src):
            print(f"   !! missing {rel} - skipped")
            continue
        with open(src, "rb") as f:
            data = f.read()
        data, nref = repoint_strings(data, ala_to_ra)
        bak = client + f".slot.{os.path.basename(target)}.bak"
        if not os.path.isfile(bak):
            with open(bak, "wb") as f:
                f.write(get_entry(client, target))
        write_entry_relocating(client, target, data)
        print(f"   OK  {rel} -> {os.path.basename(target)} "
              f"({len(data)} bytes, {nref} refs repointed)")

    tsrc = os.path.join(src_dir, os.path.basename(src_dir) + ".track")
    if os.path.isfile(tsrc):
        bak = client + ".slot.road_atlanta.track.bak"
        if not os.path.isfile(bak):
            with open(bak, "wb") as f:
                f.write(get_entry(client, RA_TRACK))
        with open(tsrc, "rb") as f:
            write_entry_relocating(client, RA_TRACK, f.read())
        print("   OK  .track -> road_atlanta.track")
    print("\n   (per-slot .bak files hold whatever was in those slots before -")
    print("    that includes the AC1 drift-map build, so it is recoverable.)")


def do_client(src_dir, mods_dir):
    """Client side of the same override.

    The server broadcasts the track's PATHS (folder_path / file_path /
    track_data_path), not just its name, so a joining client loads whatever
    lives at content\\tracks\\road_atlanta. Dropping the custom track's
    entry-point files there as LOOSE files wins over the archive - the client
    (unlike the server) does support non-packed files.

    The copies keep their internal references to the track's own folder on
    purpose: that folder is already installed loose on the client, so its
    meshes, materials and textures resolve normally. Only the entry points
    need to answer to Road Atlanta's names."""
    dst = os.path.join(mods_dir, "content", "tracks", "road_atlanta")
    os.makedirs(os.path.join(dst, "containers"), exist_ok=True)
    n = 0
    for rel, target in build_slot_map(src_dir).items():
        src = os.path.join(src_dir, rel)
        if not os.path.isfile(src):
            print(f"   !! missing {rel} - skipped")
            continue
        out = os.path.join(mods_dir, target)
        os.makedirs(os.path.dirname(out), exist_ok=True)
        with open(src, "rb") as f:
            data = f.read()
        with open(out, "wb") as f:
            f.write(data)
        print(f"   OK  {rel}  ->  {target}  ({len(data)} bytes)")
        n += 1
    # the .track carries dynamic-track settings and is tiny
    t_src = os.path.join(src_dir, os.path.basename(src_dir) + ".track")
    if os.path.isfile(t_src):
        out = os.path.join(mods_dir, RA_TRACK)
        with open(t_src, "rb") as f:
            data = f.read()
        with open(out, "wb") as f:
            f.write(data)
        print(f"   OK  .track  ->  {RA_TRACK}  ({len(data)} bytes)")
        n += 1
    print(f"\nOK  {n} loose files written under {dst}")
    print("    Delete that road_atlanta folder to undo (real Road Atlanta returns).")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--track", default="alabama",
                    help="substring of the track's display name in the client table")
    ap.add_argument("--restore", action="store_true",
                    help="put the server's original tracks.table back")
    ap.add_argument("--override", action="store_true",
                    help="map the track onto Road Atlanta's existing slots")
    ap.add_argument("--src", default=os.path.join(os.path.expanduser("~"), "Saved Games", "ACE", "mods", "content", "tracks"),
                    help="the track's loose folder on the client")
    ap.add_argument("--client", action="store_true",
                    help="loose-file override (does NOT work - packed wins; kept for reference)")
    ap.add_argument("--client-kspkg", action="store_true",
                    help="patch the CLIENT's content.kspkg slots (this is the one that works)")
    ap.add_argument("--mods", default=os.path.join(os.path.expanduser("~"), "Saved Games", "ACE", "mods"),
                    help="the client's mods folder")
    args = ap.parse_args()

    if args.client:
        print("-- client loose override (superseded: packed wins over loose)")
        do_client(args.src, args.mods)
        return

    if args.client_kspkg:
        cli = pt.find_client_kspkg()
        if not cli:
            raise SystemExit("!! could not locate the client's content.kspkg")
        print(f"-- client content.kspkg slot override\n   {cli}")
        do_client_kspkg(cli, args.src)
        print("\nOK  client patched - rejoin the server to test.")
        return

    server = pt.find_server_kspkg()
    client = pt.find_client_kspkg()
    if not server or not client:
        raise SystemExit("!! could not locate the client and/or server content.kspkg")
    print(f"server: {server}\nclient: {client}\n")

    bak = server + BAK_SUFFIX

    if args.restore:
        if not os.path.isfile(bak):
            raise SystemExit(f"!! no backup at {bak}")
        with open(bak, "rb") as f:
            original = f.read()
        write_entry_relocating(server, TRACKS_TABLE, original)
        print(f"OK  restored original tracks.table ({len(original)} bytes)")
        return

    if args.override:
        do_override(server, client, args.track, args.src)
        print("\nOK  override complete - start the server on this track to test.")
        return

    srv_tbl = get_entry(server, TRACKS_TABLE)
    cli_tbl = get_entry(client, TRACKS_TABLE)
    srv_entries = track_entries(srv_tbl)
    cli_entries = track_entries(cli_tbl)
    print(f"server tracks: {len(srv_entries)}   client tracks: {len(cli_entries)}")

    wanted = [e for e in cli_entries
              if args.track.lower() in entry_name(e).lower()]
    if not wanted:
        raise SystemExit(f"!! no client track matching {args.track!r}. "
                         f"Client has: {[entry_name(e) for e in cli_entries]}")
    have = {entry_name(e).lower() for e in srv_entries}
    todo = [e for e in wanted if entry_name(e).lower() not in have]
    if not todo:
        print(f"OK  server already registers: {[entry_name(e) for e in wanted]}")
        return

    if not os.path.isfile(bak):
        with open(bak, "wb") as f:
            f.write(srv_tbl)
        print(f"OK  backed up original tracks.table -> {bak}")

    new_tbl = build_table(srv_entries + todo)
    write_entry_relocating(server, TRACKS_TABLE, new_tbl)
    print(f"OK  registered {[entry_name(e) for e in todo]} "
          f"({len(srv_tbl)} -> {len(new_tbl)} bytes)")

    check = track_entries(get_entry(server, TRACKS_TABLE))
    print(f"OK  server now lists {len(check)} tracks; "
          f"last = {entry_name(check[-1])!r}")


if __name__ == "__main__":
    main()
