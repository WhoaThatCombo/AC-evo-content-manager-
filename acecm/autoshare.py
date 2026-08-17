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


def needs(profile):
    """What is modded about this server: {"tracks": [...], "mods": [...]}.

    Stock content is deliberately excluded - every player already has it, and
    listing it would make joiners download a copy of what they own.
    """
    imported = _imported_tracks()
    by_car = _mod_by_car()

    tracks = []
    folder = (profile.get("custom_track") or "").strip()
    if folder and folder in imported:
        tracks.append(folder)

    mods = []
    for car in (profile.get("cars") or []):
        name = by_car.get(car)
        if name and name not in mods:
            mods.append(name)
    return {"tracks": tracks, "mods": sorted(mods)}


def _auto_entry(profile):
    for e in registry.load():
        if e.get("auto") and e.get("profile_id") == profile.get("id"):
            return e
    return None


def publish(profile):
    """Make the share list match this profile. Returns what changed."""
    want = needs(profile)
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
    """Drop automatic entries whose profile is gone. Hand-made ones stay."""
    alive = {p.get("id") for p in profiles}
    gone = [e for e in registry.load()
            if e.get("auto") and e.get("profile_id") not in alive]
    for e in gone:
        registry.remove(e["id"])
    return [e["id"] for e in gone]


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
    folder = (profile.get("custom_track") or "").strip()
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


def fill_server(profile):
    """Copy anything the server is missing from the client side.

    Only the car mods: putting a track into the server package rewrites its
    index and is a deliberate, slow operation, so that stays an explicit
    action with its own progress and confirmation.
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
    return {"ok": not failed, "copied": copied, "failed": failed,
            "track_missing": gaps["track"]}


def sync(profile, fill=True):
    """Everything at once: fill the server, then publish what it needs."""
    filled = fill_server(profile) if fill else {}
    shared = publish(profile)
    return {"ok": shared.get("ok", False), "filled": filled, "shared": shared}
