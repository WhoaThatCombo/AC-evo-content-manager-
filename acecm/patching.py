"""Binary patching, done safely enough to trust.

Every ad-hoc patch in this project reinvented backup and verification, and the
failures were always the same three:

  * writing to a build the offsets were not derived from
  * writing without checking what was already there
  * "restoring" from a backup nobody verified

So patches here are DATA, not scripts. A patch declares the build it was made
for, the exact bytes it expects at each site, and the bytes to write. Applying
verifies every site first and writes nothing unless all of them match, so a
half-applied patch is not a state that can occur.

Hard-won rules encoded here:
  * `.text` is r-x at runtime. Do not plan to write to it - scratch space must
    be on the stack, and tables belong in .data page slack (which is BSS: past
    the raw size, zero-filled, writable, and needs no file bytes).
  * A patch is only valid for the build it was derived from. Game updates move
    everything; the MD5 gate makes a stale patch refuse rather than corrupt.
  * Back up once, before the first write, and keep the original untouched.
"""
import hashlib
import json
import os
import shutil
import struct
import time

from . import config

REGISTRY = os.path.join(config.DATA, "patches.json")


# --------------------------------------------------------------- PE helpers --
class PEInfo:
    """Just enough PE to find caves, slack and translate addresses."""

    def __init__(self, path):
        self.path = path
        self.data = open(path, "rb").read()
        d = self.data
        pe = struct.unpack_from("<I", d, 0x3C)[0]
        self.image_base = struct.unpack_from("<Q", d, pe + 24 + 24)[0]
        nsec = struct.unpack_from("<H", d, pe + 6)[0]
        optsz = struct.unpack_from("<H", d, pe + 20)[0]
        off = pe + 24 + optsz
        self.sections = []
        for i in range(nsec):
            b = d[off + i * 40:off + (i + 1) * 40]
            name = b[:8].rstrip(b"\0").decode(errors="replace")
            vsz, va, rsz, ptr = struct.unpack_from("<IIII", b, 8)
            chars = struct.unpack_from("<I", b, 36)[0]
            self.sections.append({
                "name": name, "va": self.image_base + va, "vsize": vsz,
                "raw": ptr, "rawsize": rsz,
                "write": bool(chars & 0x80000000),
                "exec": bool(chars & 0x20000000),
            })

    def va_to_off(self, va):
        for s in self.sections:
            if s["va"] <= va < s["va"] + s["vsize"]:
                o = s["raw"] + (va - s["va"])
                return o if o < s["raw"] + s["rawsize"] else None
        return None

    def off_to_va(self, off):
        for s in self.sections:
            if s["raw"] <= off < s["raw"] + s["rawsize"]:
                return s["va"] + (off - s["raw"])
        return None

    def code_caves(self, minimum=64):
        """Runs of int3 padding in executable sections - safe to put code in."""
        out = []
        for s in self.sections:
            if not s["exec"]:
                continue
            blob = self.data[s["raw"]:s["raw"] + min(s["rawsize"], s["vsize"])]
            run = 0
            for i, b in enumerate(blob):
                if b == 0xCC:
                    run += 1
                    continue
                if run >= minimum:
                    out.append({"va": s["va"] + i - run, "size": run,
                                "section": s["name"]})
                run = 0
            if run >= minimum:
                out.append({"va": s["va"] + len(blob) - run, "size": run,
                            "section": s["name"]})
        return sorted(out, key=lambda c: -c["size"])

    def bss_slack(self):
        """Writable space past a section's VIRTUAL size, to the page boundary.

        This is the only genuinely free writable region: zero-filled at load,
        writable, costs no file bytes, and belongs to no declared variable.

        ⚠ It is NOT the gap between raw size and virtual size. That gap is the
        section's BSS - where the program's own uninitialised globals live - and
        writing a patch table into it silently corrupts real variables. An
        earlier version of this function reported that gap (458 KB on the
        server) and would have handed a patch author a loaded gun; the true
        slack is the few KB after vsize.

        ⚠ `.text` is r-x at runtime, so it can never hold patch data. Scratch
        must go on the stack - a chat patch that put its buffer in .text
        crashed the server on the first message received.
        """
        out = []
        for s in self.sections:
            if not s["write"]:
                continue
            end_virt = s["va"] + s["vsize"]
            page_end = (end_virt + 0xFFF) & ~0xFFF
            if page_end - end_virt >= 64:
                out.append({"section": s["name"], "va": end_virt,
                            "size": page_end - end_virt,
                            "note": "free page tail (BSS proper is in use)"})
        return out


# ------------------------------------------------------------------ patches --
class Patch:
    """A named, verifiable, reversible modification of one file.

    sites: [{"va" or "off": int, "expect": hex str, "write": hex str}]
    """

    def __init__(self, pid, title, target, build_md5, sites, description=""):
        self.id = pid
        self.title = title
        self.target = target
        self.build_md5 = build_md5
        self.sites = sites
        self.description = description

    # ---- state ----------------------------------------------------------
    def _resolve(self, pe, site):
        if "off" in site:
            return site["off"]
        return pe.va_to_off(site["va"])

    def status(self):
        """clean | applied | mismatch | wrong-build | missing"""
        if not os.path.isfile(self.target):
            return {"state": "missing", "detail": "target not found"}
        pe = PEInfo(self.target)
        cur = hashlib.md5(pe.data).hexdigest()
        applied = clean = 0
        for s in self.sites:
            off = self._resolve(pe, s)
            if off is None:
                return {"state": "mismatch", "detail": "address not in image"}
            have = pe.data[off:off + len(bytes.fromhex(s["write"]))].hex()
            if have == s["write"].lower():
                applied += 1
            elif have == s["expect"].lower():
                clean += 1
        if applied == len(self.sites):
            state = "applied"
        elif clean == len(self.sites):
            state = "clean"
        else:
            state = "mismatch"
        # the build gate is advisory once applied (the file has changed by
        # definition), but decisive when clean
        wrong = (state == "clean" and self.build_md5
                 and cur != self.build_md5)
        return {"state": "wrong-build" if wrong else state,
                "md5": cur, "expected_md5": self.build_md5,
                "sites_applied": applied, "sites_clean": clean,
                "sites": len(self.sites)}

    # ---- apply / restore -------------------------------------------------
    def backup_path(self):
        return f"{self.target}.bak_{self.id}"

    def apply(self, force=False):
        st = self.status()
        if st["state"] == "applied":
            return {"ok": True, "already": True, **st}
        if st["state"] == "missing":
            return {"ok": False, "error": "target not found"}
        if st["state"] == "mismatch":
            return {"ok": False, "error":
                    "bytes at one or more sites match neither the expected nor "
                    "the patched value - refusing to write", **st}
        if st["state"] == "wrong-build" and not force:
            return {"ok": False, "error":
                    f"this patch was made for build {self.build_md5}, target is "
                    f"{st['md5']} - offsets will not match", **st}

        pe = PEInfo(self.target)
        data = bytearray(pe.data)
        # verify EVERY site before writing ANY - no half-applied states
        writes = []
        for s in self.sites:
            off = self._resolve(pe, s)
            exp = bytes.fromhex(s["expect"])
            if bytes(data[off:off + len(exp)]) != exp:
                return {"ok": False, "error": f"verify failed at 0x{off:x}"}
            writes.append((off, bytes.fromhex(s["write"])))

        if not os.path.exists(self.backup_path()):
            shutil.copy2(self.target, self.backup_path())
        for off, new in writes:
            data[off:off + len(new)] = new
        with open(self.target, "wb") as fh:
            fh.write(bytes(data))
        return {"ok": True, "backup": self.backup_path(),
                "sites": len(writes)}

    def restore(self):
        bak = self.backup_path()
        if os.path.isfile(bak):
            shutil.copy2(bak, self.target)
            return {"ok": True, "restored_from": bak}
        # no backup: put the expected bytes back, which is equally correct
        pe = PEInfo(self.target)
        data = bytearray(pe.data)
        n = 0
        for s in self.sites:
            off = self._resolve(pe, s)
            if off is None:
                continue
            data[off:off + len(bytes.fromhex(s["expect"]))] = \
                bytes.fromhex(s["expect"])
            n += 1
        with open(self.target, "wb") as fh:
            fh.write(bytes(data))
        return {"ok": True, "reverted_sites": n, "note": "no backup; wrote "
                "the expected bytes back"}

    @classmethod
    def from_dict(cls, d):
        """JSON uses "id"; the constructor takes "pid" to avoid shadowing the
        builtin. Map it here rather than at every call site."""
        return cls(d.get("id") or d.get("pid"), d.get("title", ""),
                   d.get("target", ""), d.get("build_md5", ""),
                   d.get("sites", []), d.get("description", ""))

    def to_dict(self):
        return {"id": self.id, "title": self.title, "target": self.target,
                "build_md5": self.build_md5, "sites": self.sites,
                "description": self.description}


# ----------------------------------------------------------------- registry --
def load():
    try:
        raw = json.load(open(REGISTRY, encoding="utf-8"))
    except Exception:
        raw = []
    return [Patch.from_dict(p) for p in raw]


def save(patches):
    json.dump([p.to_dict() for p in patches], open(REGISTRY, "w",
              encoding="utf-8"), indent=2)


def upsert(d):
    items = [p for p in load() if p.id != d.get("id")]
    items.append(Patch.from_dict(d))
    save(items)
    return d


def remove(pid):
    save([p for p in load() if p.id != pid])
    return {"ok": True}


def overview():
    out = []
    for p in load():
        st = p.status()
        out.append({**p.to_dict(), "status": st})
    return {"patches": out, "registry": REGISTRY}


def _allowed_inspect_target(target):
    """Only the configured game / dedicated-server binaries."""
    target = os.path.abspath(target or "")
    if not target or not os.path.isfile(target):
        return False
    allowed = []
    for p in (config.server_exe(), config.CFG.get("game_exe"),
              config.CFG.get("server_exe")):
        if p:
            allowed.append(os.path.abspath(p))
    try:
        from . import detect
        for key in ("game_exe", "server_exe"):
            found = detect.find(key)
            if found:
                allowed.append(os.path.abspath(found))
    except Exception:
        pass
    roots = []
    for p in allowed:
        d = os.path.dirname(p)
        if d:
            roots.append(d)
    try:
        if target in allowed:
            return True
        return any(os.path.commonpath([r, target]) == r for r in roots)
    except ValueError:
        return False


def inspect(target):
    """What a binary offers a patch author: caves, slack, identity."""
    if not _allowed_inspect_target(target):
        return {"ok": False, "error": "that file is not the game or server"}
    if not os.path.isfile(target):
        return {"ok": False, "error": "not found"}
    pe = PEInfo(target)
    return {
        "ok": True,
        "target": target,
        "md5": hashlib.md5(pe.data).hexdigest(),
        "size": len(pe.data),
        "image_base": hex(pe.image_base),
        "sections": [{k: (hex(v) if k == "va" else v) for k, v in s.items()}
                     for s in pe.sections],
        "code_caves": [{"va": hex(c["va"]), "size": c["size"],
                        "section": c["section"]} for c in pe.code_caves()[:5]],
        "bss_slack": [{"section": s["section"], "va": hex(s["va"]),
                       "size": s["size"]} for s in pe.bss_slack()],
    }
