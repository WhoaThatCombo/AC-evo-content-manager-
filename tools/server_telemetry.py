"""Server-side live telemetry for AC EVO - identified positions, no client.

The dedicated server holds every car's world position (it runs the physics), but
never publishes coordinates: they only travel over the netcode UDP path, which
is not logged. The client's HUD knows them, but reading it needs the game
running - see track_viewer.py's /live.

This gets the same data from the SERVER alone, with no binary patch:

  identity   the log already prints, at join:
             "searchForSpawn <pguid> p:<x> <y> <z>"
  position   lives in the ODE body objects on the heap, readable with
             ReadProcessMemory

So: watch the log for a spawn, find the physics body sitting at that exact
position, and bind pguid -> body address. Body addresses are stable for the
object's lifetime, so one binding at join yields identified positions for the
whole session.

    python server_telemetry.py            auto-detect the server, serve :8091
    python server_telemetry.py --once     bind, print one sample, exit

⚠ Bindings can only be made as cars JOIN. Cars already driving when this starts
cannot be identified retroactively - restart the server with this already
running to catch every spawn.
"""
import argparse
import ctypes as C
import glob
import json
import os
import re
import struct
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import numpy as np

k32 = C.WinDLL("kernel32", use_last_error=True)

PROCESS_VM_READ = 0x0010
PROCESS_QUERY_INFORMATION = 0x0400
MEM_COMMIT = 0x1000
PAGE_GUARD = 0x100
PAGE_NOACCESS = 0x01
READABLE = 0x02 | 0x04 | 0x20 | 0x40 | 0x80

# ⚠ Same trap as start_vai_server: a frozen ACECM unpacks this into a temp
# folder, so __file__ is not the server folder. Take it from the environment.
SRV = (os.environ.get("SERVER_DIR")
       or os.path.dirname(os.path.abspath(__file__)))
LOGDIR = os.path.join(SRV, "serverConfig")
PORT = int(os.environ.get("TELEM_PORT", "8091"))
# how close a body must be to the logged spawn point to count as that car
BIND_TOL = float(os.environ.get("BIND_TOL", "3.0"))
# Sampling a bound car costs ~0.007 ms (measured ceiling ~150 kHz), so the
# poll rate is free. 30 Hz gives the UI dense enough samples to interpolate
# between without it ever having to extrapolate.
POLL_HZ = float(os.environ.get("POLL_HZ", "30"))
# seconds to wait after a join before diffing movers - the car has to be
# driving to show up, and it sits stationary for a moment after spawning
JOIN_SETTLE = float(os.environ.get("JOIN_SETTLE", "10"))

SPAWN_RE = re.compile(r"searchForSpawn (\S+) p:(-?[\d.]+) (-?[\d.]+) (-?[\d.]+)")
# "<name> connected (<bool>) on car <model>, with new carId <pguid>"
# The pguid in searchForSpawn IS this carId (verified: 29/29 overlap), so this
# line names every car we can bind. vAI are named VAI-nnnnnn; a real player
# shows their Steam ID instead - that is how players are told from AI.
CONN_RE = re.compile(r"(\S+) connected \(\w+\) on car (\S+), with new carId (\S+)")
# ⚠ A binding MUST be dropped when its car leaves. Without this a stale binding
# outlives the disconnect, the freed body gets recycled by another car, and the
# label migrates onto a stranger - observed live: a player who left at 14:00:00
# was still drawn as a gold "player" minutes later, sitting on someone else's
# car, while their real car showed as an anonymous bot.
LEAVE_RE = re.compile(r"Disconnected carId (\S+)")
# ⭐ The RELIABLE identity line. "connected ... with new carId" fires only on a
# client's FIRST connect, but clients reconnect repeatedly (observed: 4 times in
# one minute), and every one of those emits only this line plus searchForSpawn.
# Keying identity off the first-connect line therefore left rejoining players
# permanently anonymous. This carries the Steam ID *and* the display name:
#   connecting gamecar <carId> (<display name> | <steamid64>)
GAMECAR_RE = re.compile(r"connecting gamecar (\S+) \((.*?) \| (\d+)\)")


def car_identities(log_path):
    """carId -> {name, model, ai} for the current session."""
    try:
        txt = open(log_path, encoding="utf-8", errors="replace").read()
    except OSError:
        return {}
    starts = [m.start() for m in re.finditer(r"Build release", txt)]
    if starts:
        txt = txt[starts[-1]:]
    out = {}
    for name, model, cid in CONN_RE.findall(txt):
        out[cid] = {"name": name, "model": model, "display": None,
                    "ai": name.upper().startswith("VAI-")}
    # Fills in every reconnect the line above misses, and adds the display name.
    for cid, disp, steam in GAMECAR_RE.findall(txt):
        rec = out.setdefault(cid, {"name": steam, "model": None, "ai": False})
        rec["name"] = steam
        rec["display"] = disp.strip() or None
        rec["ai"] = False          # only real clients get a steamid64 here
    return out


class MBI(C.Structure):
    _fields_ = [("BaseAddress", C.c_void_p), ("AllocationBase", C.c_void_p),
                ("AllocationProtect", C.c_uint32), ("__a", C.c_uint32),
                ("RegionSize", C.c_size_t), ("State", C.c_uint32),
                ("Protect", C.c_uint32), ("Type", C.c_uint32), ("__b", C.c_uint32)]


def find_server_pid():
    import subprocess
    out = subprocess.run(
        ["powershell.exe", "-NoProfile", "-Command",
         "Get-Process -Name 'AssettoCorsaEVOServer*' -ErrorAction SilentlyContinue "
         "| Sort-Object WorkingSet64 -Descending "
         "| Select-Object -First 1 -ExpandProperty Id"],
        capture_output=True, text=True).stdout.strip()
    return int(out) if out.isdigit() else None


class Proc:
    def __init__(self, pid):
        self.pid = pid
        self.h = k32.OpenProcess(
            PROCESS_VM_READ | PROCESS_QUERY_INFORMATION, False, pid)
        if not self.h:
            raise OSError(f"OpenProcess({pid}) failed: {C.get_last_error()}")

    def regions(self):
        addr, mbi = 0, MBI()
        while k32.VirtualQueryEx(self.h, C.c_void_p(addr), C.byref(mbi), C.sizeof(mbi)):
            size, base = mbi.RegionSize, (mbi.BaseAddress or 0)
            if (mbi.State == MEM_COMMIT and (mbi.Protect & READABLE)
                    and not (mbi.Protect & (PAGE_GUARD | PAGE_NOACCESS))
                    and size <= 64 << 20):
                yield base, size
            addr = base + size
            if addr > 0x7FFFFFFFFFFF:
                return

    def read(self, base, size):
        buf = (C.c_char * size)()
        got = C.c_size_t(0)
        if not k32.ReadProcessMemory(self.h, C.c_void_p(base), buf, size, C.byref(got)):
            return None
        return bytes(buf[:got.value])

    def read_pos(self, addr):
        b = self.read(addr, 12)
        if not b or len(b) < 12:
            return None
        return struct.unpack("<fff", b)

    def close(self):
        if self.h:
            k32.CloseHandle(self.h)
            self.h = None


# Track bounding box. Anything outside this is not a car on this circuit.
BB = (500.0, 7000.0, 250.0, 700.0, 500.0, 6000.0)   # xlo,xhi,ylo,yhi,zlo,zhi
# ⚠ That default is NURBURGRING's extents. Every other circuit sits somewhere
# else entirely (Barber: x[-329,576] y[17,39] z[-518,208]), and a car outside
# the box is discarded before the movement diff runs - so the tracker reports
# zero cars while the server log plainly shows someone driving. ACECM passes
# the right box per server; keep the default only as a fallback.
# Track centreline, for rejecting off-track junk. ACECM passes a JSON file of
# [[x,z], ...]; without it only the origin is rejected.
TRACK_XZ = None
MAX_OFFTRACK = float(os.environ.get("TELEM_MAX_OFFTRACK", "30"))
# consecutive observations before an unidentified mover is reported
ANON_MIN_SAMPLES = int(os.environ.get("TELEM_ANON_MIN_SAMPLES", "4"))
_track_env = os.environ.get("TELEM_TRACK", "").strip()

_bb_env = os.environ.get("TELEM_BBOX", "").strip()
if _bb_env:
    try:
        _vals = tuple(float(v) for v in _bb_env.split(","))
        if len(_vals) == 6:
            BB = _vals
        else:
            print(f"!! TELEM_BBOX needs 6 numbers, got {len(_vals)} - using default")
    except ValueError:
        print(f"!! TELEM_BBOX not numeric ({_bb_env!r}) - using default")


def scan_bbox(proc):
    """All float triplets inside the track bounding box -> {addr: (x,y,z)}.

    Takes ~0.4 s for the whole process and yields ~270k slots, almost all of
    them static scenery/spline data. Cheap enough to run repeatedly.
    """
    xlo, xhi, ylo, yhi, zlo, zhi = BB
    out = {}
    for base, size in proc.regions():
        raw = proc.read(base, size)
        if not raw or len(raw) < 16:
            continue
        n = len(raw) // 4
        a = np.frombuffer(raw[:n * 4], dtype="<f4")
        if a.size < 3:
            continue
        with np.errstate(invalid="ignore"):
            idx = np.flatnonzero((a[:-2] > xlo) & (a[:-2] < xhi))
            if idx.size == 0:
                continue
            idx = idx[(a[idx + 1] > ylo) & (a[idx + 1] < yhi)
                      & (a[idx + 2] > zlo) & (a[idx + 2] < zhi)]
        # ⚠ Build this vectorised. The old per-element loop (int()/float() per
        # candidate, tens of thousands of them) held the GIL long enough to
        # starve the 30 Hz sampler thread: position samples showed a ~290 ms
        # hole every 4 s when the refresher ran, which outran the UI's
        # interpolation buffer and made the map jump.
        ii = idx.tolist()
        xs = a[idx].tolist()
        ys = a[idx + 1].tolist()
        zs = a[idx + 2].tolist()
        for j, i in enumerate(ii):
            out[base + i * 4] = (xs[j], ys[j], zs[j])
        # Give the sampler a chance to run between regions.
        time.sleep(0)
    return out


def movers(proc, gap=1.5, min_move=2.0):
    """Addresses whose position changed like a driving car -> {addr: (x,y,z)}.

    Two bbox scans and a diff. This is the identification primitive: cars are
    the only things in the box that move.
    """
    a = scan_bbox(proc)
    time.sleep(gap)
    b = scan_bbox(proc)
    out = {}
    for addr, p1 in a.items():
        p2 = b.get(addr)
        if not p2:
            continue
        d = ((p2[0] - p1[0]) ** 2 + (p2[2] - p1[2]) ** 2) ** 0.5
        if min_move < d < 200.0 and abs(p2[1] - p1[1]) < 15.0:
            out[addr] = p2
    return {a: p for a, p in out.items() if on_track(p)}


def on_track(p, maxoff=None):
    """Is this position actually on the circuit?

    ⚠ The bounding box alone is not enough. On a track near the world origin
    (Barber spans x[-329,576] z[-518,208]) the box contains (0,0,0), and memory
    is full of zero and near-zero triplets; junk drifting between small values
    reads as a car moving a few metres and gets reported as a phantom. The old
    Nurburgring box only avoided this by accident, because it started at x=500.

    So require a detection to be near the track centreline. Applied to MOVERS
    only - a handful of points - never to the ~270k bbox candidates, which
    would be far too slow.
    """
    if TRACK_XZ is None:
        # No centreline available: still reject the origin, the common case.
        return abs(p[0]) > 1.0 or abs(p[2]) > 1.0
    maxoff = MAX_OFFTRACK if maxoff is None else maxoff
    dx = TRACK_XZ[:, 0] - p[0]
    dz = TRACK_XZ[:, 1] - p[2]
    return float(np.min(dx * dx + dz * dz)) <= maxoff * maxoff


def cluster_movers(mv, tol=1.0):
    """One car can expose several addresses (duplicate copies of its position).

    Group by near-identical coordinates and keep one representative each.
    """
    reps = []
    for addr, (x, y, z) in sorted(mv.items()):
        for r in reps:
            rx, ry, rz = mv[r]
            if abs(rx - x) < tol and abs(ry - y) < tol and abs(rz - z) < tol:
                break
        else:
            reps.append(addr)
    return reps


def scan_for_points(proc, points, tol):
    """One pass over the process, matching MANY spawn points at once.

    Scanning per spawn does not work: the whole grid spawns inside about a
    second, while a single 340 MB numpy pass takes a few seconds, so every car
    after the first has already driven off by the time its scan starts. Here one
    pass answers all of them.

    points: {key: (x, y, z)}.  Returns {key: [addresses]}, nearest-first.
    """
    out = {k: [] for k in points}
    keys = list(points)
    px = np.array([points[k][0] for k in keys], dtype="f4")
    py = np.array([points[k][1] for k in keys], dtype="f4")
    pz = np.array([points[k][2] for k in keys], dtype="f4")
    for base, size in proc.regions():
        raw = proc.read(base, size)
        if not raw or len(raw) < 16:
            continue
        n = len(raw) // 4
        a = np.frombuffer(raw[:n * 4], dtype="<f4")
        if a.size < 3:
            continue
        with np.errstate(invalid="ignore"):
            # candidate x positions: anything near ANY spawn x
            near = np.zeros(a.size - 2, dtype=bool)
            for v in px:
                near |= np.abs(a[:-2] - v) < tol
            idx = np.flatnonzero(near)
            if idx.size == 0:
                continue
            ax, ay, az = a[idx], a[idx + 1], a[idx + 2]
            for j, k in enumerate(keys):
                m = ((np.abs(ax - px[j]) < tol) & (np.abs(ay - py[j]) < tol)
                     & (np.abs(az - pz[j]) < tol))
                for i in np.flatnonzero(m):
                    out[k].append(base + int(idx[i]) * 4)
    return out


class Tracker:
    def __init__(self, proc):
        self.proc = proc
        self.bound = {}      # pguid -> {addr, hist}
        self.baseline = set()
        self.last_movers = {}
        self.tracked = {}      # addr -> misses; every car we can see
        self.anon_hist = {}
        self.log_path = None
        self._ident = {}
        self._ident_t = 0.0
        self.baseline_is_ai = False
        self._last_out = []
        self.pending = {}
        self.lock = threading.Lock()

    def ident_for(self, pguid):
        now = time.time()
        if now - self._ident_t > 5.0:
            self._ident = car_identities(self.log_path)
            self._ident_t = now
        i = self._ident.get(pguid)
        if not i:
            return {"name": None, "model": None, "display": None, "ai": None}
        return {"name": i["name"], "model": i.get("model"),
                "display": i.get("display"), "ai": i["ai"]}

    def refresh_baseline(self):
        mv = movers(self.proc)
        self.baseline = set(cluster_movers(mv))
        self.last_movers = mv
        with self.lock:
            self.tracked = dict.fromkeys(self.baseline, 0)
        return self.baseline

    def refresh_tracked(self):
        """Keep the full set of car addresses fresh, identified or not.

        Identity only arrives for cars that join while we are watching, but
        EVERY car is visible as a mover - so the view can show the whole field
        and simply label the ones we know. Without this the map would sit empty
        until somebody happened to rejoin.
        """
        mv = movers(self.proc)
        reps = cluster_movers(mv)
        with self.lock:
            for a in reps:
                self.tracked.setdefault(a, 0)
            # forget addresses that have stopped behaving like cars
            for a in list(self.tracked):
                if a not in reps:
                    self.tracked[a] += 1
                    if self.tracked[a] > 3:
                        del self.tracked[a]
                else:
                    self.tracked[a] = 0
        return len(self.tracked)

    def bind_by_join(self, pguids):
        """pguids: {pguid: (x,y,z) spawn point or None}."""
        spawns = pguids if isinstance(pguids, dict) else {}
        pguids = list(pguids)
        """Identify newly-joined cars as the movers that were not there before.

        This replaces spawn-coordinate matching, which cannot work: the logged
        spawn point comes from a static SLOT TABLE (stride 0x108) and no car
        body is ever at those coordinates - probed it, 596 triplets in the
        spawn area, all permanently static.

        A car is instead identified by becoming a NEW mover shortly after its
        join appears in the log. Unambiguous when one car joins at a time,
        which is the normal case for real players.
        """
        mv = movers(self.proc)
        reps = cluster_movers(mv)
        new = [a for a in reps if a not in self.baseline]
        self.baseline = set(reps)
        self.last_movers = mv

        # One car often exposes SEVERAL address slots holding near-identical
        # positions (different copies updated a frame apart), and a 1 m cluster
        # tolerance does not always merge them. If a single car joined and the
        # new movers are all bunched within a car-length or two, they are the
        # same car - bind it instead of refusing as "ambiguous", which was
        # leaving real players permanently unidentified.
        if len(pguids) == 1 and len(new) > 1:
            xs = [mv[a] for a in new]
            spread = max(((p[0] - q[0]) ** 2 + (p[2] - q[2]) ** 2) ** 0.5
                         for p in xs for q in xs)
            if spread < 8.0:
                print(f"  {len(new)} slots within {spread:.1f} m - same car")
                new = new[:1]

        # Several people joining at once used to be refused outright, which on
        # a busy server meant real players were never named. Each joiner spawns
        # in their OWN pit slot (a few metres apart) and the log gives that
        # point, so a new mover can be attributed to the joiner whose spawn it
        # started nearest to. Only accept a clear winner - if two candidates are
        # comparably close, stay silent rather than mislabel someone.
        if len(new) >= 1 and len(pguids) > 1 and any(spawns.get(p) for p in pguids):
            claimed, n = set(), 0
            for a in new:
                px, _, pz = mv[a]
                d = sorted(((((px - s[0]) ** 2 + (pz - s[2]) ** 2) ** 0.5), p)
                           for p in pguids
                           if (s := spawns.get(p)) and p not in claimed)
                if not d:
                    continue
                best, who = d[0]
                second = d[1][0] if len(d) > 1 else float("inf")
                if best < 400.0 and second > best * 2.5:
                    claimed.add(who)
                    with self.lock:
                        self.bound[who] = {"addr": a, "hist": []}
                    n += 1
                    print(f"  bound {who[:16]}... -> 0x{a:012x} "
                          f"({best:.0f} m from its spawn slot)")
            if n:
                return n

        if len(new) == 1 and len(pguids) == 1:
            a = new[0]
            with self.lock:
                self.bound[pguids[0]] = {"addr": a, "hist": []}
            print(f"  bound {pguids[0][:16]}... -> 0x{a:012x} {mv[a]}")
            return 1
        if not new:
            print(f"  no new mover appeared for {len(pguids)} join(s) - "
                  f"car may not be driving yet")
            return 0
        print(f"  ambiguous: {len(new)} new movers for {len(pguids)} join(s); "
              f"not guessing")
        return 0

    def bind_batch(self, pending):
        """pending: {pguid: (x,y,z)} - bind the whole grid from one scan."""
        pending = {k: v for k, v in pending.items() if k not in self.bound}
        if not pending:
            return 0
        t0 = time.time()
        cand = scan_for_points(self.proc, pending, BIND_TOL)
        print(f"  scan took {time.time() - t0:.1f}s")

        # Do NOT commit to an address here. Several things sit at a spawn point:
        # the persistent physics body, the static spawn-slot definition it was
        # placed from, and short-lived scratch buffers the spawn search used.
        # Committing at spawn picked scratch, which is freed moments later and
        # then reads as garbage. Instead keep every candidate and let resolve()
        # decide once the car drives - only the real body both stays readable
        # and moves along the track.
        n = 0
        for k, addrs in cand.items():
            if not addrs:
                print(f"  {k[:16]}... no candidate at spawn point")
                continue
            with self.lock:
                self.bound[k] = {"addr": None, "cands": list(addrs),
                                 "spawn": pending[k], "hist": []}
            n += 1
            print(f"  {k[:16]}... {len(addrs)} candidate(s), pending movement")
        return n

    def resolve(self):
        """Narrow candidates to the one real body, using movement."""
        with self.lock:
            items = [(k, r) for k, r in self.bound.items() if r["addr"] is None]
        for k, rec in items:
            sx, sy, sz = rec["spawn"]
            alive = []
            for a in rec["cands"]:
                p = self.proc.read_pos(a)
                if p is None:
                    continue                       # freed scratch
                x, y, z = p
                if not (abs(x) < 20000 and abs(y) < 5000 and abs(z) < 20000):
                    continue                       # reused for something else
                d = ((x - sx) ** 2 + (z - sz) ** 2) ** 0.5
                alive.append((a, d))
            if not alive:
                with self.lock:
                    self.bound.pop(k, None)
                print(f"  {k[:16]}... lost (all candidates died)")
                continue
            movers = [a for a, d in alive if d > 5.0]
            with self.lock:
                if k not in self.bound:
                    continue
                if movers:
                    self.bound[k]["addr"] = movers[0]
                    print(f"  resolved {k[:16]}... -> 0x{movers[0]:012x}")
                else:
                    self.bound[k]["cands"] = [a for a, _ in alive]

    def prune(self):
        """Drop bindings whose address stopped being a plausible position."""
        with self.lock:
            for k in list(self.bound):
                p = self.proc.read_pos(self.bound[k]["addr"])
                if p is None or not (abs(p[0]) < 20000 and abs(p[2]) < 20000):
                    del self.bound[k]

    def unbind(self, pguid):
        with self.lock:
            return self.bound.pop(pguid, None) is not None

    def snapshot(self):
        """Last computed sample. HTTP must use THIS, not sample().

        Letting request threads call sample() meant several threads appended to
        the same history list microseconds apart, which both corrupted the
        speed calculation and raced on the list itself.
        """
        return list(self._last_out)

    def sample(self):
        out = []
        now = time.time()
        with self.lock:
            items = [(k, r) for k, r in self.bound.items() if r["addr"]]
        for pguid, rec in items:
            p = self.proc.read_pos(rec["addr"])
            if not p:
                continue
            x, y, z = p
            # A bound address can be freed and reused by something else, after
            # which it reads as garbage (denormals like 3.2e-32) or teleports,
            # producing nonsense like 39000 km/h. Validate against the track
            # box and against a plausible step, and retire a binding that keeps
            # failing rather than emitting rubbish.
            xlo, xhi, ylo, yhi, zlo, zhi = BB
            ok = (xlo < x < xhi and ylo < y < yhi and zlo < z < zhi)
            if ok and rec["hist"]:
                t0, x0, _, z0 = rec["hist"][-1]
                dt = now - t0
                # ⚠ Only judge speed over a MEANINGFUL interval. sample() runs
                # on the poller thread AND on every HTTP request, so two calls
                # can land microseconds apart; dividing by that dt turned a
                # normal 0.2 m step into 200 m/s, tripped this gate and retired
                # perfectly good bindings ("freed/reused" was a false positive -
                # measured, the addresses stay valid for minutes).
                if dt > 0.05 and \
                        ((x - x0) ** 2 + (z - z0) ** 2) ** 0.5 / dt > 130.0:
                    ok = False          # >468 km/h: genuinely not a car
            if not ok:
                rec["bad"] = rec.get("bad", 0) + 1
                if rec["bad"] > 100:
                    with self.lock:
                        self.bound.pop(pguid, None)
                    print(f"  dropped {pguid[:16]}... - address no longer a car "
                          f"(freed/reused)")
                continue
            rec["bad"] = 0
            h = rec["hist"]
            h.append((now, x, y, z))
            del h[:-24]
            speed = heading = None
            if len(h) >= 2:
                t0, x0, _, z0 = h[0]
                dt = now - t0
                if dt > 0.05:
                    dx, dz = x - x0, z - z0
                    speed = (dx * dx + dz * dz) ** 0.5 / dt * 3.6
                    if abs(dx) + abs(dz) > 0.05:
                        import math
                        heading = (math.degrees(math.atan2(dx, dz))) % 360
            out.append({"id": pguid, "x": x, "y": y, "z": z, "t": now,
                        # ⭐ The whole recent TRAIL, with server timestamps, not
                        # just the latest point. We sample at 30 Hz but the UI
                        # polls far slower; handing over the trail means the UI
                        # interpolates on OUR clock and a late or jittery poll
                        # changes nothing - it just receives the samples a
                        # little later and still has every one of them.
                        "trail": [[round(t0, 3), round(px, 2), round(pz, 2)]
                                  for t0, px, _py, pz in h],
                        "kmh": round(speed, 1) if speed is not None else None,
                        "heading": round(heading, 1) if heading is not None else None,
                        "addr": f"0x{rec['addr']:x}",
                        **self.ident_for(pguid)})

        # unidentified cars: everything else that is moving like a car
        bound_addrs = {r["addr"] for r in self.bound.values() if r.get("addr")}
        with self.lock:
            others = [a for a in self.tracked if a not in bound_addrs]
        # Prune history for addresses that stopped being movers, or a phantom
        # that flickers away and returns comes back with its old sample count
        # already past the threshold and is reported immediately.
        for _a in list(self.anon_hist):
            if _a not in others:
                del self.anon_hist[_a]
        xlo, xhi, ylo, yhi, zlo, zhi = BB
        for a in others:
            p = self.proc.read_pos(a)
            if not p:
                continue
            x, y, z = p
            if not (xlo < x < xhi and ylo < y < yhi and zlo < z < zhi):
                continue
            h = self.anon_hist.setdefault(a, [])
            h.append((now, x, y, z))
            del h[:-24]
            kmh = hd = None
            if len(h) >= 2:
                t0, x0, _, z0 = h[0]
                dt = now - t0
                dx, dz = x - x0, z - z0
                v = (dx * dx + dz * dz) ** 0.5 / dt * 3.6 if dt > 0.05 else None
                if v is not None and v < 500:
                    kmh = round(v, 1)
                    if abs(dx) + abs(dz) > 0.05:
                        import math
                        hd = round(math.degrees(math.atan2(dx, dz)) % 360, 1)
            # ⚠ An anonymous mover must PERSIST before we believe it. A real
            # car updates continuously; the false positives that survive the
            # box and the on-track test are transient - they appear for a
            # sample or two and vanish (observed: two phantoms alongside one
            # real player, each seen twice, one of them at x=1). Requiring a
            # few consecutive observations costs a second of latency on a
            # genuinely new car and removes the flicker entirely.
            if len(h) < ANON_MIN_SAMPLES:
                continue
            out.append({"id": None, "x": x, "y": y, "z": z, "kmh": kmh,
                        "t": now,
                        "trail": [[round(t0, 3), round(px, 2), round(pz, 2)]
                                  for t0, px, _py, pz in h],
                        "heading": hd, "addr": f"0x{a:x}",
                        "name": None, "model": None, "display": None,
                        "ai": True if (self.baseline_is_ai
                                       and a in self.baseline) else None})
        self._last_out = out
        return out


def newest_log():
    logs = glob.glob(os.path.join(LOGDIR, "*.log"))
    return max(logs, key=os.path.getmtime) if logs else None


def tail_spawns(tracker, path, from_start=False):
    """Follow the log, binding each car as it spawns."""
    # Re-open per poll rather than holding a buffered reader: the server keeps
    # its own handle on this file, and a long-lived Python reader stops seeing
    # appends (it saw 1 of 43 spawn lines). Track a byte offset instead.
    offset = 0 if from_start else os.path.getsize(path)
    print(f"watching {os.path.basename(path)} for spawns"
          f"{' (from start)' if from_start else ''}, offset {offset}")
    pending = {}
    last_spawn = {}
    last_seen = 0.0
    while True:
        # The server truncates and reuses the same filename on restart. Seeking
        # to the end of the PREVIOUS run's (50 MB) log leaves the offset past
        # EOF forever, so nothing is ever read. Detect the shrink and rewind.
        try:
            size = os.path.getsize(path)
        except OSError:
            time.sleep(0.3)
            continue
        if size < offset:
            print("log truncated (server restarted) - rewinding")
            offset = 0
            pending.clear()
        if size > offset:
            with open(path, "rb") as fh:
                fh.seek(offset)
                chunk = fh.read(size - offset)
                offset = size
            text = chunk.decode("utf-8", "replace")
            for m in SPAWN_RE.finditer(text):
                pguid = m.group(1)
                spawn = tuple(map(float, m.groups()[1:]))
                last_spawn[pguid] = spawn
                if pguid not in tracker.bound:
                    pending[pguid] = spawn
                    last_seen = time.time()
            # A reconnect emits `connecting gamecar` too; treat it as a join so
            # a player who rejoins gets re-bound instead of staying anonymous.
            for m in GAMECAR_RE.finditer(text):
                pguid = m.group(1)
                if pguid not in tracker.bound and pguid not in pending:
                    # keep any spawn point already seen for this car, so the
                    # nearest-spawn disambiguation still has a reference
                    pending[pguid] = last_spawn.get(pguid)
                    last_seen = time.time()
            # Retire bindings for cars that have left, before their body can be
            # recycled by someone else and carry the old label with it.
            for m in LEAVE_RE.finditer(text):
                gone = m.group(1)
                pending.pop(gone, None)
                if tracker.unbind(gone):
                    print(f"  unbound {gone[:16]}... (disconnected)")
        # Wait for the car to actually start driving - it is identified by
        # becoming a new mover, which cannot happen while it sits in the pits.
        if pending and time.time() - last_seen > JOIN_SETTLE:
            print(f"resolving {len(pending)} join(s) by mover diff...")
            tracker.bind_by_join(dict(pending))
            print(f"  {len(tracker.bound)} car(s) identified")
            pending.clear()
        time.sleep(0.15)


class Server(ThreadingHTTPServer):
    # Threaded on purpose: refresh_tracked() runs a ~3.4 s mover scan every few
    # seconds, and on a single-threaded server that blocked every request for
    # its whole duration. The viewer timed out, its feed went empty and the map
    # looked FROZEN. Requests only read a cached snapshot, so serving them
    # concurrently is safe.
    daemon_threads = True
    # MUST be a class attribute: HTTPServer binds inside __init__, so setting
    # this on the instance afterwards is too late. Windows happily allows
    # several processes to bind the same port, and requests then land on an
    # arbitrary stale instance - that is how 18 trackers ended up sharing :8091
    # while the one holding the real bindings served nobody.
    allow_reuse_address = False


class Handler(BaseHTTPRequestHandler):
    tracker = None

    def do_GET(self):
        data = json.dumps({"cars": self.tracker.snapshot(),
                           "bound": len(self.tracker.bound)}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *a):
        pass


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--pid", type=int)
    ap.add_argument("--log")
    ap.add_argument("--from-start", action="store_true",
                    help="replay the whole log (binds cars that spawned earlier, "
                         "which usually fails - they have driven away)")
    ap.add_argument("--once", action="store_true")
    ap.add_argument("--wait", type=float, default=0, metavar="SEC",
                    help="wait up to SEC for the server process to appear, so "
                         "this can be launched together with the server")
    ap.add_argument("--baseline-ai", action="store_true",
                    help="treat the cars already driving at startup as vAI. "
                         "True when this is started with the server: the AI "
                         "grid spawns first and all at once, so anything that "
                         "appears later is a real player.")
    a = ap.parse_args()

    pid = a.pid or find_server_pid()
    if not pid and a.wait:
        print("waiting for the dedicated server to start...")
        t0 = time.time()
        while not pid and time.time() - t0 < a.wait:
            time.sleep(0.5)
            pid = find_server_pid()
    if not pid:
        sys.exit("no AssettoCorsaEVOServer process found")
    log = a.log or newest_log()
    if not log:
        sys.exit(f"no log in {LOGDIR}")
    print(f"server pid {pid}\nlog {log}")
    # State the coordinate filter. A box from the wrong track silently reports
    # zero cars, which is indistinguishable from "nobody is driving".
    print(f"bbox x[{BB[0]:.0f},{BB[1]:.0f}] y[{BB[2]:.0f},{BB[3]:.0f}] "
          f"z[{BB[4]:.0f},{BB[5]:.0f}]"
          f"{'  (from TELEM_BBOX)' if _bb_env else '  (DEFAULT - Nurburgring)'}")
    global TRACK_XZ
    if _track_env and os.path.isfile(_track_env):
        try:
            pts = json.load(open(_track_env, encoding="utf-8"))
            TRACK_XZ = np.array(pts, dtype="f4")
            print(f"centreline {len(TRACK_XZ)} pts, rejecting anything over "
                  f"{MAX_OFFTRACK:.0f} m off track")
        except Exception as ex:
            print(f"!! could not load TELEM_TRACK ({ex}) - origin filter only")
    else:
        print("no centreline (TELEM_TRACK unset) - origin filter only")

    proc = Proc(pid)
    tr = Tracker(proc)
    tr.log_path = log

    if a.once:
        d = open(log, encoding="utf-8", errors="replace").read()
        for m in list(SPAWN_RE.finditer(d))[-24:]:
            tr.bind(m.group(1), *map(float, m.groups()[1:]))
        print(json.dumps(tr.sample(), indent=1))
        return

    t = threading.Thread(target=tail_spawns, args=(tr, log, a.from_start),
                         daemon=True)
    t.start()

    def poller():
        # Speed and heading come from position history, so something has to
        # sample continuously - otherwise history only ever has the single
        # point taken by whatever HTTP request happened to arrive, and both
        # come back null.
        period = 1.0 / POLL_HZ
        while True:
            try:
                tr.sample()
            except Exception as ex:
                print(f"poll: {type(ex).__name__}: {ex}")
            time.sleep(period)

    threading.Thread(target=poller, daemon=True).start()

    def refresher():
        while True:
            time.sleep(4.0)
            try:
                tr.refresh_tracked()
            except Exception as ex:
                print(f"refresh: {type(ex).__name__}: {ex}")

    tr.baseline_is_ai = a.baseline_ai
    if a.baseline_ai:
        # --baseline-ai claims "everything already driving is a bot". That is
        # only true when this starts WITH the server. If real players are
        # already connected they would be silently painted as bots, which is
        # worse than admitting we do not know - so refuse and say why.
        pre = [v for v in car_identities(log).values() if not v["ai"]]
        if pre:
            print(f"  ! --baseline-ai ignored: {len(pre)} real player(s) are "
                  f"already connected ({', '.join(p['name'] for p in pre[:4])}"
                  f"{'…' if len(pre) > 4 else ''}), so the cars already driving "
                  f"are NOT all bots. Restart the server and this together to "
                  f"use it.")
            tr.baseline_is_ai = False
    # Wait for the AI grid to be MOVING rather than sleeping a fixed guess.
    # The grid connects within ~12 s of launch but takes a while to get rolling;
    # a blind 75 s sleep let real players join first and spoiled the baseline.
    # Poll instead and settle as soon as the mover count stops growing.
    if a.wait:
        print("waiting for the AI grid to connect and start moving...")
        t0, last, stable = time.time(), -1, 0
        while time.time() - t0 < 240:
            # ⚠ Do NOT scan while the server is still loading: the address space
            # churns and the diff reports hundreds of bogus "movers" (seen: 218
            # at t+1s), which sailed past any count threshold and produced a
            # garbage baseline. Wait for the bots to actually be connected.
            expect = sum(1 for v in car_identities(log).values() if v["ai"])
            if not expect:
                time.sleep(1.0)
                continue
            n = len(cluster_movers(movers(proc, gap=0.6)))
            # a plausible reading is bounded by the grid size; anything wildly
            # larger is still load noise
            sane = 0 < n <= expect * 2
            stable = stable + 1 if (sane and abs(n - last) <= 1) else 0
            last = n
            print(f"   {expect} bot(s) connected, {n} moving")
            # Never demand the FULL grid: some bots sit in the pits and never
            # move, so n == expect may never happen and the wait would hang.
            # Most of the grid rolling, a settled count, or a time cap will do.
            if sane and (n >= expect * 0.7 or stable >= 2
                         or time.time() - t0 > 45):
                break
        print(f"grid ready after {time.time() - t0:.0f}s")

    print("establishing mover baseline...")
    base = tr.refresh_baseline()
    print(f"  {len(base)} car(s) already driving (unidentified - they were "
          f"here before we attached)")
    threading.Thread(target=refresher, daemon=True).start()

    Handler.tracker = tr
    try:
        srv = Server(("127.0.0.1", PORT), Handler)
    except OSError as ex:
        sys.exit(f"port {PORT} already in use ({ex}) - another tracker "
                 f"is running; kill it first")
    print(f"telemetry JSON on http://127.0.0.1:{PORT}/")
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        proc.close()


if __name__ == "__main__":
    main()
