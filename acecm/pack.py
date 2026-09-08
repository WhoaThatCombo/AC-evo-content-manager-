"""Shareable server packages: one zip holding a server's track and car mods.

The point is the case ACECM's own delivery cannot serve: a joiner on bad
internet, or one too far away for a direct fetch to be pleasant. Build the
package once, put it on Google Drive or a USB stick, and the other person drags
it onto their ACECM.

⚠ The contents are decided by exactly the same code that decides what a joiner
would DOWNLOAD from this host - autoshare.share_needs() plus registry's file
finders. A package is therefore the same set of bytes the network path would
have sent, and the two cannot drift apart:

  * the hosted track, only when it is an imported/mod track;
  * the mod cars in that profile's allowed-cars list;
  * nothing stock - the other person already has it;
  * nothing the host has unshared for that server.

The zip carries `acecm-package.json` at its root, which is what tells a
dropped file apart from an ordinary mod zip. Paths inside are the same
manifest-relative paths the network manifest uses (`mods/x.kspkg`,
`tracks/<folder>/...`), so installing reuses contentsync.destination() rather
than inventing a second layout to keep in step.
"""
import hashlib
import json
import os
import time
import zipfile

from . import config, logs

MARKER = "acecm-package.json"
FORMAT = 1


def _digest(path, chunk=1024 * 1024):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(chunk), b""):
            h.update(block)
    return h.hexdigest()


def contents(profile):
    """(files, want, missing) - what this server's package would hold.

    files: [{path, src, kind, size}] with `path` the manifest-relative name.
    """
    from . import autoshare, registry
    want = autoshare.share_needs(profile)
    files, missing = [], []
    for kind, names, finder in (("mod", want.get("mods") or [], registry._mod_files),
                                ("track", want.get("tracks") or [], registry._track_files)):
        for name in names:
            got = finder(name)
            if not got:
                missing.append(f"{kind} {name}")
                continue
            for rel, path in got:
                try:
                    files.append({"path": rel, "src": path, "kind": kind,
                                  "size": os.path.getsize(path)})
                except OSError:
                    missing.append(f"{kind} {name}: {rel}")
    return files, want, missing


def preview(profile):
    """What a package would contain, without building it."""
    files, want, missing = contents(profile)
    return {"ok": True,
            "tracks": want.get("tracks") or [],
            "mods": want.get("mods") or [],
            "files": len(files),
            "bytes": sum(f["size"] for f in files),
            "missing": missing}


def default_dir():
    """Downloads - it is where people look for something to share."""
    d = os.path.join(os.path.expanduser("~"), "Downloads")
    return d if os.path.isdir(d) else config.DATA


def _safe_name(s):
    keep = "".join(c if (c.isalnum() or c in " -_") else "_" for c in str(s))
    return "_".join(keep.split()) or "server"


def build(profile, dest=None, progress=None, digests=True):
    """Write the package. Returns {ok, path, files, bytes} or {ok: False}."""
    from . import install as installmod
    files, want, missing = contents(profile)
    if not files:
        return {"ok": False,
                "error": "this server needs no custom content - its track and "
                         "cars are all stock, so there is nothing to share",
                "missing": missing}
    name = _safe_name(profile.get("name") or "server")
    out = dest or os.path.join(default_dir(),
                               f"ACECM {name} package.zip")
    if os.path.isdir(out):
        out = os.path.join(out, f"ACECM {name} package.zip")
    os.makedirs(os.path.dirname(out) or ".", exist_ok=True)

    man = {
        "format": FORMAT,
        "built": int(time.time()),
        "built_by": "ACECM",
        "server": {"name": profile.get("name") or "",
                   "track": profile.get("track_label")
                            or profile.get("custom_track") or "",
                   "tracks": want.get("tracks") or [],
                   "mods": want.get("mods") or []},
        "files": [],
    }
    total = sum(f["size"] for f in files)
    installmod.ingest_begin(f"packaging {name}", total)
    tmp = out + ".part"
    done = 0
    try:
        # ⚠ ZIP_DEFLATED, not stored. A track folder is mostly small text and
        # protobuf and compresses hard, which is the whole point when the
        # person receiving it is the one with bad internet.
        with zipfile.ZipFile(tmp, "w", zipfile.ZIP_DEFLATED,
                             allowZip64=True) as z:
            for f in files:
                rel = f["path"].replace("\\", "/")
                man["files"].append({
                    "path": rel, "size": f["size"], "kind": f["kind"],
                    "sha256": _digest(f["src"]) if digests else "",
                })
                z.write(f["src"], rel)
                done += f["size"]
                installmod.ingest_step(f["size"], os.path.basename(rel))
                if progress:
                    progress(done, total)
            z.writestr(MARKER, json.dumps(man, indent=1))
        os.replace(tmp, out)
    except Exception as ex:
        for junk in (tmp,):
            try:
                os.remove(junk)
            except OSError:
                pass
        logs.exception("pack build", ex)
        return {"ok": False, "error": f"could not build the package: {ex}"}
    finally:
        installmod.ingest_end("")
    logs.LOG.info("built server package %s (%d file(s), %.1f MB)",
                  out, len(files), os.path.getsize(out) / 1e6)
    return {"ok": True, "path": out, "files": len(files),
            "bytes": os.path.getsize(out), "raw_bytes": total,
            "missing": missing, "server": man["server"]}


# ------------------------------------------------------------- install --

def read_manifest(path):
    """The package manifest, or {} when this is not one of ours."""
    try:
        with zipfile.ZipFile(path) as z:
            with z.open(MARKER) as f:
                man = json.loads(f.read().decode("utf-8"))
    except Exception:
        return {}
    return man if isinstance(man, dict) and man.get("files") is not None else {}


def is_package(path):
    """Cheap enough to call on every dropped file."""
    if not str(path).lower().endswith(".zip") or not os.path.isfile(path):
        return False
    try:
        with zipfile.ZipFile(path) as z:
            return MARKER in z.namelist()
    except Exception:
        return False


def install(path, progress=None):
    """Unpack a server package into this machine's content."""
    from . import contentsync, install as installmod
    man = read_manifest(path)
    if not man:
        return {"ok": False, "error": "not an ACECM server package"}
    if int(man.get("format") or 0) > FORMAT:
        return {"ok": False,
                "error": "this package was made by a newer ACECM - update, "
                         "then drop it again"}
    entries = man.get("files") or []
    total = sum(int(e.get("size") or 0) for e in entries) or 1
    installmod.ingest_begin("installing server package", total)
    written, skipped, folders, bad = 0, 0, set(), []
    done = 0
    try:
        with zipfile.ZipFile(path) as z:
            for e in entries:
                rel = (e.get("path") or "").replace("\\", "/")
                try:
                    # ⚠ destination() validates the path (no .., only content
                    # extensions) AND is the same mapping the network install
                    # uses, so a package lands exactly where a download would.
                    dest = contentsync.destination(rel)
                except ValueError as ex:
                    bad.append(f"{rel}: {ex}")
                    continue
                size = int(e.get("size") or 0)
                want_sha = e.get("sha256") or ""
                if (os.path.isfile(dest) and os.path.getsize(dest) == size
                        and (not want_sha
                             or contentsync.registry.file_digest(dest) == want_sha)):
                    skipped += 1
                else:
                    os.makedirs(os.path.dirname(dest), exist_ok=True)
                    tmp = dest + ".part"
                    with z.open(rel) as src, open(tmp, "wb") as out:
                        while True:
                            chunk = src.read(1024 * 1024)
                            if not chunk:
                                break
                            out.write(chunk)
                    if want_sha and _digest(tmp) != want_sha:
                        os.remove(tmp)
                        bad.append(f"{rel}: checksum mismatch")
                        continue
                    os.replace(tmp, dest)
                    written += 1
                parts = rel.split("/")
                if len(parts) >= 3 and parts[0] == "tracks":
                    folders.add(parts[1])
                done += size
                installmod.ingest_step(size, os.path.basename(rel))
                if progress:
                    progress(done, total)
        # ⚠ A track on disk is not yet a track the game can load - it has to
        # go into the client's tables too. Same call the network path makes.
        for folder in sorted(folders):
            try:
                contentsync._register_downloaded_track(folder)
            except Exception as ex:
                logs.LOG.warning("could not register %s: %s", folder, ex)
    except Exception as ex:
        logs.exception("pack install", ex)
        return {"ok": False, "error": f"could not install the package: {ex}"}
    finally:
        installmod.ingest_end("")
        try:
            installmod._after_content_change()
        except Exception:
            pass
    logs.LOG.info("installed server package %s: %d written, %d already had, "
                  "%d track(s)", os.path.basename(path), written, skipped,
                  len(folders))
    return {"ok": True, "written": written, "already_had": skipped,
            "tracks": sorted(folders), "problems": bad,
            "server": man.get("server") or {}}
