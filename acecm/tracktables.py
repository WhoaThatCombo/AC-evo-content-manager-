"""Editing the two system tables that make a track exist to the server.

On join the server does NOT send a track name - it sends three paths out of
`system\\tracks.table` (folder, root .scene, .track) plus the container list
from `system\\track_containers.table`. Whatever those point at is what every
client loads. Registering a track natively means writing both, instead of
repointing a stock track's row at borrowed slots.

    tracks.table             f2 rows: f1 display name, f3 folder, f4 .scene,
                             f5 country, f8 .track, f13 region
    track_containers.table   f2 wrapper -> repeated f3 rows -> f8 body:
                             f1 session name, f8 track length, f9 pit slots,
                             f10 display name, f11 repeated container path,
                             f14 layout, f18 bbox, f21 slots

⚠ track_containers holds ONE ROW PER GAME MODE, not one per track. Road Atlanta
ships twelve (GP / drift / layout x Race / Time Attack / Hotstint / No Game
Mode). Miss the mode the server starts in and it reports `containers: []` and
warns "we lack the right containers in this Gamemode" - with nothing naming the
real problem.

⚠ The layout name match is CASE SENSITIVE. A season asking for layout "Layout"
against rows carrying "layout" silently resolves no containers at all.
"""


def varint(n):
    out = bytearray()
    while True:
        b = n & 127
        n >>= 7
        out.append(b | (128 if n else 0))
        if not n:
            return bytes(out)


def tag(field, wire):
    return varint((field << 3) | wire)


def emit(field, wire, value):
    if wire == 2:
        return tag(field, 2) + varint(len(value)) + value
    if wire == 0:
        return tag(field, 0) + varint(value)
    return tag(field, wire) + value


def walk(d, start=0, end=None):
    """(field, wire, value) - length-delimited come back as bytes, varints int."""
    i, end = start, len(d) if end is None else end
    while i < end:
        key = 0
        shift = 0
        while True:
            b = d[i]; i += 1
            key |= (b & 127) << shift
            shift += 7
            if not b & 128:
                break
        field, wire = key >> 3, key & 7
        if wire == 2:
            n = 0
            shift = 0
            while True:
                b = d[i]; i += 1
                n |= (b & 127) << shift
                shift += 7
                if not b & 128:
                    break
            if i + n > end:
                return
            yield field, wire, d[i:i + n]
            i += n
        elif wire == 0:
            v = 0
            shift = 0
            while True:
                b = d[i]; i += 1
                v |= (b & 127) << shift
                shift += 7
                if not b & 128:
                    break
            yield field, wire, v
        elif wire == 5:
            yield field, wire, d[i:i + 4]; i += 4
        elif wire == 1:
            yield field, wire, d[i:i + 8]; i += 8
        else:
            raise ValueError(f"bad wire type {wire}")


def _unwrap(blob):
    """Both tables wrap their rows in a single field 2."""
    top = list(walk(blob))
    if len(top) != 1 or top[0][0] != 2:
        raise ValueError("unexpected table wrapper")
    return top[0][2]


def _paths(folder, scene, track):
    return folder, scene, track


def track_paths(folder):
    """The three paths a tracks.table row points at."""
    base = f"content\\tracks\\{folder}"
    return base, f"{base}\\{folder}.scene", f"{base}\\{folder}.track"


def registered_names(blob):
    """Display names already present in tracks.table.

    A Kunos content update replaces content.kspkg wholesale, so any row we
    upserted earlier is gone - there is no diff, no merge, just a fresh stock
    table. This is what lets a caller tell "still registered" from "wiped by
    the last update" without re-deploying tracks that don't need it.
    """
    out = set()
    for f, w, v in walk(_unwrap(blob)):
        if w != 2:
            continue
        for g, gw, gv in walk(v):
            if g != 2 or gw != 2:
                continue
            for h, hw, hv in walk(gv):
                if h == 1 and hw == 2:
                    out.add(hv.decode("utf-8", "replace"))
    return out


def rows_for_folder(blob, folder):
    """(display_name, folder) for every tracks.table row pointing at `folder`.

    Three different naming fallbacks existed across this codebase at once
    (deploy_native, pack_meta, and an early redeclare_tracks all disagreed),
    so the same folder can end up registered under two or three different
    display names before this is noticed. Finding rows by the thing that
    is actually unique - the folder path - is what makes cleanup possible;
    matching by name only would leave every wrong name in place.
    """
    out = []
    want = ("content\\tracks\\" + folder).lower()
    for f, w, v in walk(_unwrap(blob)):
        if w != 2:
            continue
        name = base = None
        for g, gw, gv in walk(v):
            if g == 2 and gw == 2:
                for h, hw, hv in walk(gv):
                    if h == 1 and hw == 2:
                        name = hv.decode("utf-8", "replace")
                    elif h == 3 and hw == 2:
                        base = hv.decode("utf-8", "replace")
        if base and base.lower() == want:
            out.append(name)
    return out


def remove_track_row(blob, display_name):
    """Drop the row for `display_name`, if present. No-op otherwise."""
    rows = _unwrap(blob)
    out = b""
    for f, w, v in walk(rows):
        if w != 2:
            out += emit(f, w, v)
            continue
        name = None
        for g, gw, gv in walk(v):
            if g == 2 and gw == 2:
                for h, hw, hv in walk(gv):
                    if h == 1 and hw == 2:
                        name = hv.decode("utf-8", "replace")
        if name == display_name:
            continue
        out += emit(f, w, v)
    return emit(2, 2, out)


def remove_container_rows(blob, display_name):
    """Drop every track_containers.table row for `display_name`.

    Mirrors container_rows()'s own field path exactly: _unwrap, then the
    display name sits inside the row's field 8 wrapper at field 10.
    """
    out = b""
    for f, w, v in walk(_unwrap(blob)):
        if w != 2:
            out += emit(f, w, v)
            continue
        name = None
        for g, gw, gv in walk(v):
            if g != 8 or gw != 2:
                continue
            for h, hw, hv in walk(gv):
                if h == 10 and hw == 2:
                    name = hv.decode("utf-8", "replace")
        if name == display_name:
            continue
        out += emit(f, w, v)
    return emit(2, 2, out)


def upsert_track_row(blob, display_name, folder, template_name=None):
    """Point `display_name` at `folder`'s own paths, adding the row if absent.

    An existing row is rewritten rather than duplicated - a track deployed
    twice must not appear twice. When the name is new, a row is cloned from
    `template_name` (any stock track) so every field we do not understand keeps
    a sane value.
    """
    base, scene, trackf = track_paths(folder)
    rows = _unwrap(blob)
    out = b""
    found = False
    template = None
    for f, w, v in walk(rows):
        if w != 2:
            out += emit(f, w, v)
            continue
        name = None
        for g, gw, gv in walk(v):
            if g == 2 and gw == 2:
                for h, hw, hv in walk(gv):
                    if h == 1 and hw == 2:
                        name = hv.decode("utf-8", "replace")
        if template is None and name and (template_name is None
                                          or name == template_name):
            template = (f, v)
        if name != display_name:
            out += emit(f, w, v)
            continue
        out += emit(f, w, _rewrite_track_row(v, display_name, base, scene, trackf))
        found = True
    if not found:
        if template is None:
            raise ValueError("no template row to clone")
        tf, tv = template
        out += emit(tf, 2, _rewrite_track_row(tv, display_name, base, scene,
                                              trackf))
    return emit(2, 2, out)


def _rewrite_track_row(row, display_name, base, scene, trackf):
    inner = b""
    for f, w, v in walk(row):
        if f == 2 and w == 2:
            body = b""
            for g, gw, gv in walk(v):
                if gw == 2 and g == 1:
                    gv = display_name.encode()
                elif gw == 2 and g == 3:
                    gv = base.encode()
                elif gw == 2 and g == 4:
                    gv = scene.encode()
                elif gw == 2 and g == 8:
                    gv = trackf.encode()
                body += emit(g, gw, gv)
            inner += emit(f, 2, body)
        else:
            inner += emit(f, w, v)
    return inner


def container_rows(blob, display_name):
    """Every (outer_field, body) row registered for a display name."""
    out = []
    for f, w, v in walk(_unwrap(blob)):
        if w != 2:
            continue
        for g, gw, gv in walk(v):
            if g != 8 or gw != 2:
                continue
            for h, hw, hv in walk(gv):
                if h == 10 and hw == 2 and hv.decode("utf-8", "replace") == display_name:
                    out.append((f, gv))
    return out


def upsert_container_rows(blob, display_name, folder, layout, containers,
                          template_name):
    """Register `containers` for every game mode `template_name` supports.

    `containers` maps a template container's role - its file name with the
    layout suffix stripped - to this track's own file name. Roles the track has
    no equivalent for are dropped; shared common_assets containers are kept.
    """
    rows = _unwrap(blob)
    template = container_rows(blob, template_name)
    if not template:
        raise ValueError(f"no container rows for template {template_name!r}")

    kept = b""
    for f, w, v in walk(rows):
        drop = False
        if w == 2:
            for g, gw, gv in walk(v):
                if g != 8 or gw != 2:
                    continue
                for h, hw, hv in walk(gv):
                    if h == 10 and hw == 2 \
                            and hv.decode("utf-8", "replace") == display_name:
                        drop = True          # replace, never duplicate
        kept += b"" if drop else emit(f, w, v)

    made = 0
    for outer_f, body in template:
        # ⚠ The row's SESSION NAME is part of the match, not just the track and
        # layout. Template rows are named "<template layout> <mode>" ("GP
        # Race"), and the season asks for "<our layout> <mode>" ("layout
        # Race"). Leave f1 alone and the server finds no row and reports
        # containers: [] with nothing explaining why.
        src_layout = None
        for f, w, v in walk(body):
            if f == 14 and w == 2:
                src_layout = v.decode("utf-8", "replace")
        buf = b""
        for f, w, v in walk(body):
            if f == 11 and w == 2:
                path = v.decode("utf-8", "replace")
                name = path.rsplit("\\", 1)[-1]
                if name.endswith(".scene"):
                    name = name[:-len(".scene")]
                if "common_assets" in path:
                    buf += emit(11, 2, v)            # shared camera sequences
                    continue
                role = _role(name)
                if role in containers:
                    buf += emit(11, 2, (f"content\\tracks\\{folder}\\containers"
                                        f"\\{containers[role]}.scene").encode())
                continue
            if f == 1 and w == 2 and src_layout:
                name = v.decode("utf-8", "replace")
                if name.startswith(src_layout + " "):
                    v = (layout + name[len(src_layout):]).encode()
            elif f == 10 and w == 2:
                v = display_name.encode()
            elif f == 14 and w == 2:
                v = layout.encode()
            buf += emit(f, w, v)
        kept += emit(outer_f, 2, emit(8, 2, buf))
        made += 1
    return emit(2, 2, kept), made


# Container file names carry a layout suffix on stock tracks (layout_gp,
# timelines_gp) but not on imported ones. Strip any trailing _<word> that is a
# known layout so both spellings reduce to the same role.
_SUFFIXES = ("_gp", "_layout", "_drift", "_national", "_club", "_short")


def _role(name):
    for suf in _SUFFIXES:
        if name.endswith(suf) and len(name) > len(suf):
            return name[:-len(suf)]
    return name
