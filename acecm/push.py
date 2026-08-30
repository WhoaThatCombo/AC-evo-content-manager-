"""Send a server the content it needs, from this install.

The remote half of this already worked: a headless ACECM accepts uploads by
streaming them into a staging folder and ingesting them (see auth.py). What
was missing was the other end - deciding WHAT a server is short of, and
putting it there without anyone copying files by hand.

ACECM already answers the first part. A server profile names its track and its
allowed cars, and `autoshare.needs` works out which of those are modded. This
just walks that list and uploads it.

    profile -> autoshare.needs -> the mod packages + one track tar -> remote

⚠ Uploads STREAM. A track is about a gigabyte, and reading it into memory to
post it would spike this process by that much for no reason - the same reason
the receiving side streams to disk rather than buffering a request body.

⚠ The track goes as ONE tar, not as three thousand files. registry already
builds exactly that pack for content delivery, so this reuses it rather than
inventing a second archive format the receiver would have to learn.
"""
import json
import os
import urllib.error
import urllib.parse
import urllib.request
import uuid

from . import autoshare, install, logs, registry

CHUNK = 4 * 1024 * 1024


def _headers(token):
    h = {"User-Agent": "ACECM"}
    if token:
        h["X-ACECM-Token"] = token
    return h


def _post_json(base, path, token, obj, timeout=600):
    data = json.dumps(obj).encode()
    req = urllib.request.Request(base.rstrip("/") + path, data=data,
                                 method="POST",
                                 headers={**_headers(token),
                                          "Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read() or b"{}")


class _FileBody:
    """A file as a request body, read in chunks rather than all at once."""

    def __init__(self, path, progress=None, sent=0, total=0):
        self.f = open(path, "rb")
        self.progress = progress
        self.sent = sent
        self.total = total

    def read(self, n=-1):
        b = self.f.read(CHUNK if n is None or n < 0 else min(n, CHUNK))
        if b:
            self.sent += len(b)
            if self.progress:
                self.progress(self.sent, self.total)
        else:
            self.f.close()
        return b


def _send_file(base, token, did, path, progress=None, sent=0, total=0):
    """Stream one file into the remote's staging folder."""
    name = os.path.basename(path)
    size = os.path.getsize(path)
    url = (base.rstrip("/") + "/api/drop/part?id=" + did
           + "&name=" + urllib.parse.quote(name))
    body = _FileBody(path, progress, sent, total)
    req = urllib.request.Request(url, data=body, method="POST",
                                 headers={**_headers(token),
                                          "Content-Type":
                                              "application/octet-stream"})
    # ⚠ Content-Length by hand. urllib will not size a file-like body, and
    # without it the request goes out chunked - which the receiver rejects,
    # because it reads exactly Content-Length to avoid a keep-alive deadlock.
    req.add_header("Content-Length", str(size))
    with urllib.request.urlopen(req, timeout=3600) as r:
        return json.loads(r.read() or b"{}"), body.sent


def plan(profile):
    """What this server is short of, as files on this machine.

    ⚠ Reports what is MISSING remotely only in the sense of what the profile
    needs; the receiver decides whether it already has it. Sending a car the
    server already has is wasteful but harmless, and asking first would cost
    a round trip per file for a case that is rare.
    """
    want = autoshare.needs(profile)
    mods, missing = [], []
    src = install.client_mods_dir()
    for name in want["mods"]:
        pair = [os.path.join(src, name + ext) for ext in (".kspkg", ".json")]
        here = [p for p in pair if os.path.isfile(p)]
        if any(p.endswith(".kspkg") for p in here):
            mods.append((name, here))
        else:
            missing.append(name)
    return {"mods": mods, "tracks": want["tracks"], "missing": missing}


def check(base, token=""):
    """Is there an ACECM there, and will it take our uploads?"""
    base = (base or "").strip().rstrip("/")
    if not base.startswith(("http://", "https://")):
        return {"ok": False, "error": "give the server's ACECM address, "
                                      "like http://100.x.y.z:8092"}
    try:
        req = urllib.request.Request(base + "/api/auth",
                                     headers=_headers(token))
        with urllib.request.urlopen(req, timeout=15) as r:
            got = json.loads(r.read() or b"{}")
    except urllib.error.HTTPError as ex:
        return {"ok": False, "error": f"the server answered HTTP {ex.code}"}
    except Exception as ex:
        return {"ok": False,
                "error": f"no ACECM answering at {base} ({ex})"}
    if got.get("authorised"):
        return {"ok": True, "remote_admin": got.get("remote_admin")}
    if not got.get("remote_admin"):
        return {"ok": False,
                "error": "that ACECM is not accepting remote administration "
                         "- start it with --headless, or turn remote admin on "
                         "in its Settings"}
    return {"ok": False,
            "error": "the server did not accept that admin token"}


def send(profile, base, token="", overwrite=True, progress=None):
    """Upload everything `profile` needs to the ACECM at `base`."""
    base = (base or "").strip().rstrip("/")
    if not base.startswith(("http://", "https://")):
        return {"ok": False, "error": "give the server's ACECM address, "
                                      "like http://100.x.y.z:8092"}
    # ⚠ Check the token BEFORE sending anything. The receiver rejects an
    # unauthorised request without reading its body, so a wrong token part way
    # through a gigabyte surfaces as "[WinError 10053] connection aborted" -
    # a socket error that says nothing about the actual problem. One cheap GET
    # turns that into a sentence.
    reach = check(base, token)
    if not reach.get("ok"):
        return reach

    p = plan(profile)
    if not p["mods"] and not p["tracks"]:
        return {"ok": False, "error": "nothing modded about this server - "
                                      "there is nothing to send"}

    # size everything first, so a progress bar can mean something
    files = []
    for _name, paths in p["mods"]:
        files.extend(paths)
    packs = []
    for folder in p["tracks"]:
        try:
            tar = registry.ensure_track_pack(folder)
        except Exception as ex:
            logs.LOG.exception("packing %s to send", folder)
            return {"ok": False, "error": f"could not pack {folder}: {ex}"}
        if not tar or not os.path.isfile(tar):
            return {"ok": False, "error": f"no files found for track {folder}"}
        packs.append(tar)
    total = sum(os.path.getsize(f) for f in files + packs)

    sent_total = 0
    done = []
    # ⚠ One drop per THING, not one for everything. A car is a .kspkg plus its
    # .json and has to be ingested together; a track pack is ingested on its
    # own. Putting them in one staging folder would leave the receiver
    # guessing what it had been handed.
    for name, paths in p["mods"]:
        did = str(uuid.uuid4())
        for path in paths:
            try:
                _r, sent_total = _send_file(base, token, did, path,
                                            progress, sent_total, total)
            except urllib.error.HTTPError as ex:
                return {"ok": False, "error": _why(ex, name)}
            except Exception as ex:
                return {"ok": False, "error": f"sending {name}: {ex}"}
        try:
            r = _post_json(base, "/api/drop", token,
                           {"id": did, "overwrite": overwrite})
        except urllib.error.HTTPError as ex:
            return {"ok": False, "error": _why(ex, name)}
        if not r.get("ok"):
            return {"ok": False, "error": f"{name}: {r.get('error')}"}
        done.append(name)

    for folder, tar in zip(p["tracks"], packs):
        did = str(uuid.uuid4())
        try:
            _r, sent_total = _send_file(base, token, did, tar,
                                        progress, sent_total, total)
            r = _post_json(base, "/api/drop", token,
                           {"id": did, "overwrite": overwrite}, timeout=3600)
        except urllib.error.HTTPError as ex:
            return {"ok": False, "error": _why(ex, folder)}
        except Exception as ex:
            return {"ok": False, "error": f"sending {folder}: {ex}"}
        if not r.get("ok"):
            return {"ok": False, "error": f"{folder}: {r.get('error')}"}
        done.append(folder)

    logs.LOG.info("sent %s to %s", ", ".join(done), base)
    return {"ok": True, "sent": done, "bytes": sent_total,
            "missing_here": p["missing"]}


def _why(ex, what):
    """Turn an HTTP failure into something the user can act on."""
    if ex.code in (401, 403):
        return (f"{what}: the server refused the upload - check the admin "
                "token, and that it was started with --headless")
    try:
        body = json.loads(ex.read() or b"{}")
        if body.get("error"):
            return f"{what}: {body['error']}"
    except Exception:
        pass
    return f"{what}: HTTP {ex.code}"
