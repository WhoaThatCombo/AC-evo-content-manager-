"""Sharing follows the servers you run, instead of being a separate chore.

The old arrangement asked the host to work out, by hand, which of their content
a joining player would be missing, and then tick each piece on a Content page.
That is a question ACECM can answer itself: a server profile already names its
track and its allowed cars, and we already know which of those are modded. So
the share list is not something to maintain - it is a CONSEQUENCE of how the
servers are configured, and it can be kept correct automatically.

    configure a server  ->  we work out what is modded about it
                        ->  make sure the SERVER has those files
                        ->  publish exactly those, for joiners to fetch

⚠ Entries created this way carry `profile_id` and `auto: True`. Anything a user
shared by hand has neither, and is never touched here - automatic bookkeeping
must not quietly delete somebody's deliberate choice.
"""
import os

from . import config, install, logs, registry

AUTO_NOTE = "shared automatically because a server here needs it"


def _mod_by_car():
    """car id -> the mod that provides it, for every installed car mod."""
    out = {}
    for side in ("client", "server"):
        for mod in (install.installed(side).get("mods") or []):
            for car in (mod.get("cars") or []):
                if car.get("id"):
                    out[car["id"]] = mod["name"]
    return out


def _imported_tracks():
    """Track folders that came from a mod, i.e. are not in the stock game."""
    from . import contentsync
    try:
        return set(contentsync.local().get("tracks") or [])
    except Exception as ex:
        logs.LOG.warning("autoshare: could not list imported tracks: %s", ex)
        return set()


def track_folder(profile):
    """The FOLDER a profile's custom track lives in.

    ⚠ `custom_track` holds what a player sees - "Drift", "Highlands Drift" -
    not the folder it lives in (drift_2hwp80). Treating it as a folder made
    every check miss: the track was never shared, every server was told its
    track was "not in the content package", and the button meant to fix that
    could not find the files. The client's own tracks.table is the mapping.
    """
    name = (profile.get("custom_track") or "").strip()
    if not name:
        return ""
    from . import contentsync
    imported = _imported_tracks()
    if name in imported:          # already a folder
        return name
    try:
        table = contentsync.track_map()
    except Exception as ex:
        logs.LOG.info("autoshare: track map: %s", ex)
        return ""
    hit = table.get(name)
    if not hit:                   # names differ in case more often than not
        low = name.lower()
        hit = next((v for k, v in table.items() if k.lower() == low), "")
    return hit or ""


def needs(profile):
    """What is modded about this server: {"tracks": [...], "mods": [...]}.

    Stock content is deliberately excluded - every player already has it, and
    listing it would make joiners download a copy of what they own.
    """
    imported = _imported_tracks()
    by_car = _mod_by_car()

    tracks = []
    folder = track_folder(profile)
    if folder and folder in imported:
        tracks.append(folder)

    mods = []
    for car in (profile.get("cars") or []):
        name = by_car.get(car)
        if name and name not in mods:
            mods.append(name)
    return {"tracks": tracks, "mods": sorted(mods)}


def share_needs(profile):
    """What we actually advertise. A host can unshare the hosted track."""
    want = needs(profile)
    skip = {t for t in (profile.get("unshared_tracks") or []) if t}
    want["tracks"] = [t for t in want["tracks"] if t not in skip]
    return want


def _auto_entry(profile):
    for e in registry.load():
        if e.get("auto") and e.get("profile_id") == profile.get("id"):
            return e
    return None


def publish(profile):
    """Make the share list match this profile. Returns what changed."""
    want = share_needs(profile)
    existing = _auto_entry(profile)

    if not want["tracks"] and not want["mods"]:
        # nothing modded about it any more - stop advertising it
        if existing:
            registry.remove(existing["id"])
            return {"ok": True, "shared": False, "removed": True, **want}
        return {"ok": True, "shared": False, **want}

    entry = {
        "id": (existing or {}).get("id") or "",
        "name": f"{profile.get('name') or 'server'} (content)",
        "description": AUTO_NOTE,
        "profile_id": profile.get("id"),
        "auto": True,
        "public": True,
        "port": profile.get("tcp_port") or 9700,
        "required_tracks": want["tracks"],
        "required_mods": want["mods"],
    }
    if not entry["id"]:
        entry.pop("id")
    saved = registry.upsert(entry)
    return {"ok": True, "shared": True, "id": saved.get("id"), **want}


def prune(profiles):
    """Drop automatic entries whose profile is gone. Hand-made ones stay.

    Also drops leftover '(hosted here)' rows that track deploy used to create
    for every import. Those advertised every imported track whether a server
    here actually ran it, and autoshare never touched them because they had
    no `auto` / `profile_id`.
    """
    alive = {p.get("id") for p in profiles}
    drop, keep = [], []
    for e in registry.load():
        leftover = ((e.get("name") or "").endswith("(hosted here)")
                    and not e.get("auto")
                    and not (e.get("profile_id") or "").strip())
        stale_auto = e.get("auto") and e.get("profile_id") not in alive
        if leftover or stale_auto:
            drop.append(e)
        else:
            keep.append(e)
    if drop:
        registry.save(keep)
        try:
            registry.forget_public_sizes()
        except Exception:
            pass
    return [e["id"] for e in drop]


def set_track(folder, share, label=""):
    """Share or unshare one track, including the one a server is hosting.

    Unsharing a hosted track is remembered on the profile as
    `unshared_tracks`, so the next save/start does not publish it again.
    The server still gets the files - this only changes what joiners can
    download.
    """
    from . import servers
    folder = (folder or "").strip()
    if not folder:
        return {"ok": False, "error": "no track"}
    share = bool(share)
    items = servers.load()
    for p in items:
        skip = [t for t in (p.get("unshared_tracks") or []) if t]
        uses = track_folder(p) == folder
        if share:
            p["unshared_tracks"] = [t for t in skip if t != folder]
        elif uses and folder not in skip:
            p["unshared_tracks"] = skip + [folder]
    servers.save_all(items)
    for p in items:
        publish(p)
    if share:
        already = any(folder in (e.get("required_tracks") or [])
                      for e in registry.load())
        if not already:
            registry.upsert({
                "name": f"{(label or folder)} (track)",
                "description": f"Track {folder} shared by this host",
                "required_tracks": [folder],
                "public": True,
            })
    else:
        kept = []
        for e in registry.load():
            tracks = [t for t in (e.get("required_tracks") or []) if t != folder]
            if tracks or (e.get("required_mods") or []):
                kept.append({**e, "required_tracks": tracks})
        registry.save(kept)
    try:
        registry.forget_public_sizes()
    except Exception:
        pass
    return {"ok": True, "shared": share, "folder": folder}


def server_gaps(profile):
    """What this server is configured to use but does not actually have.

    ⚠ The client and the dedicated server keep car mods in DIFFERENT folders.
    A car you can drive is not automatically a car the server can host, and
    that mismatch is invisible until a player picks the car and cannot join.
    """
    want = needs(profile)
    have = {m["name"] for m in (install.installed("server").get("mods") or [])}
    missing_mods = [m for m in want["mods"] if m not in have]

    missing_track = ""
    folder = track_folder(profile)
    if folder:
        from . import tracks as trackmod
        try:
            pkg = trackmod.server_kspkg()
            if pkg and not trackmod.in_server_package(folder):
                missing_track = folder
        except Exception as ex:
            logs.LOG.info("autoshare: server package check: %s", ex)
    return {"mods": missing_mods, "track": missing_track,
            "ok": not missing_mods and not missing_track}


def deploy_track(folder):
    """Put an imported track into the SERVER's own content package.

    ⚠ This rewrites the package's 64 MiB index, so it is done at START, not on
    every save - once, when the user is already waiting for the server to come
    up. Skipped entirely when the track is already there.
    """
    if not folder:
        return {"ok": True, "skipped": "no custom track"}
    from . import tracks as trackmod
    if trackmod.in_server_package(folder):
        return {"ok": True, "skipped": "already in the package"}
    hit = next((x for x in trackmod.importable()
                if x.get("folder") == folder), None)
    if not hit or not hit.get("path"):
        return {"ok": False,
                "error": f"no imported files found for {folder}"}
    logs.LOG.info("deploying %s into the server package", folder)
    try:
        r = trackmod.deploy_native(hit["path"])
    except Exception as ex:
        logs.LOG.exception("deploying %s", folder)
        return {"ok": False, "error": str(ex)}
    if r.get("ok"):
        logs.LOG.info("deployed %s into the server package", folder)
    else:
        logs.LOG.warning("deploying %s: %s", folder, r.get("error"))
    return r


def fill_server(profile):
    """Copy anything the server is missing from the client side.

    Car mods are files in a folder; the track has to go INTO the server's
    content package, which is slower - both happen here so that starting a
    server is all it takes to make it hostable.
    """
    gaps = server_gaps(profile)
    copied, failed = [], []
    src_dir = install.client_mods_dir()
    dst_dir = install.mods_dir(create=True)
    for name in gaps["mods"]:
        ok = True
        for ext in (".kspkg", ".json"):
            src = os.path.join(src_dir, name + ext)
            dst = os.path.join(dst_dir, name + ext)
            if not os.path.isfile(src):
                ok = False
                break
            try:
                install._copy_file(src, dst, force=True)
            except Exception as ex:
                logs.LOG.warning("autoshare: copying %s: %s", name + ext, ex)
                ok = False
                break
        (copied if ok else failed).append(name)
    track = deploy_track(gaps["track"]) if gaps["track"] else {}
    return {"ok": not failed and track.get("ok", True),
            "copied": copied, "failed": failed,
            "track": gaps["track"], "track_deploy": track,
            "track_missing": "" if track.get("ok", True) else gaps["track"]}


def sync(profile, fill=True):
    """Everything at once: fill the server, then publish what it needs."""
    filled = fill_server(profile) if fill else {}
    shared = publish(profile)
    return {"ok": shared.get("ok", False), "filled": filled, "shared": shared}
