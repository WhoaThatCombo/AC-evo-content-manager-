"""Our own lobby backend, and launching the client into it.

AC EVO has no direct-connect. The client asks Kunos's lobby to resolve a server
and authorise the join, so the way into a self-hosted server is not to bypass
the backend but to BE the backend - see acevo_localconnect/README.md for the
protocol. Two modes, both already written and proven:

  proxy      relays to the real Kunos backend and appends our servers to the
             list. Keeps the player's account, garage and the public list.
  standalone replaces the backend entirely. Fully offline, no Kunos at all.

This module only supervises those processes and reports state; the protocol
work lives in the tools directory.
"""
import json
import os
import shutil
import socket
import subprocess
import sys
import time

from . import config, detect, logs, netutil

MODES = {
    "proxy": "acevo_proxy.py",
    "standalone": "acevo_backend.py",
}
_procs = {}


def certs_dir():
    """Where OUR TLS keypair lives.

    ⚠ Generated on this machine and never shipped. A private key baked into a
    build is the same key for every user, which is no security at all - and the
    proxy terminates the client's TLS, so this key protects a real session.
    """
    d = os.path.join(config.DATA, "certs")
    os.makedirs(d, exist_ok=True)
    return d


def cert_paths():
    return (os.path.join(certs_dir(), "cert.pem"),
            os.path.join(certs_dir(), "key.pem"))


# The exact client slot. Stock ships this AND a localhost:448 URL; the
# patch overwrites THIS one. Presence of this string is the only honest
# "rdata still points at Kunos" signal — localhost:448 is in the table
# either way.
KUNOS_CLIENT_URL = b"wss://b.gk.sd:6990/9kB3F1CAx7mQ2zL5vN8pD4sT6yW0eU1r/000500"
_probe_cache = {}


def _wanted_san():
    """Names/IPs the cert must cover for local + LAN clients."""
    import ipaddress
    names = ["localhost", "b.gk.sd"]
    ips = ["127.0.0.1"]
    lan = netutil.lan_ipv4()
    if lan:
        ips.append(lan)
    return names, ips, lan


def _cert_sans(cert_path):
    try:
        from cryptography import x509
        crt = x509.load_pem_x509_certificate(open(cert_path, "rb").read())
        ext = crt.extensions.get_extension_for_class(x509.SubjectAlternativeName)
        names, ips = [], []
        for n in ext.value:
            if isinstance(n, x509.DNSName):
                names.append(n.value)
            elif isinstance(n, x509.IPAddress):
                ips.append(str(n.value))
        return names, ips
    except Exception:
        return [], []


def _cert_covers():
    cert, key = cert_paths()
    if not (os.path.exists(cert) and os.path.exists(key)):
        return False
    have_n, have_i = _cert_sans(cert)
    want_n, want_i, _ = _wanted_san()
    return set(want_n) <= set(have_n) and set(want_i) <= set(have_i)


def ensure_cert(force=False):
    """Create a self-signed keypair covering localhost and the LAN IP.

    Regenerates when the current LAN address is missing from the SAN — a
    laptop that moved networks would otherwise present a cert the guest
    client rejects.
    """
    cert, key = cert_paths()
    if not force and _cert_covers():
        return {"ok": True, "existing": True, "cert": cert,
                "lan_ip": netutil.lan_ipv4()}
    try:
        import datetime
        import ipaddress
        from cryptography import x509
        from cryptography.hazmat.primitives import hashes, serialization
        from cryptography.hazmat.primitives.asymmetric import rsa
        from cryptography.x509.oid import NameOID
    except Exception as ex:
        return {"ok": False, "error": f"cryptography unavailable: {ex}"}

    names, ips, lan = _wanted_san()
    k = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "localhost")])
    now = datetime.datetime.now(datetime.timezone.utc)
    san = [x509.DNSName(n) for n in names]
    san += [x509.IPAddress(ipaddress.ip_address(i)) for i in ips]
    crt = (x509.CertificateBuilder()
           .subject_name(name).issuer_name(name).public_key(k.public_key())
           .serial_number(x509.random_serial_number())
           .not_valid_before(now - datetime.timedelta(days=1))
           .not_valid_after(now + datetime.timedelta(days=3650))
           .add_extension(x509.SubjectAlternativeName(san), critical=False)
           .sign(k, hashes.SHA256()))
    with open(cert, "wb") as f:
        f.write(crt.public_bytes(serialization.Encoding.PEM))
    with open(key, "wb") as f:
        f.write(k.private_bytes(serialization.Encoding.PEM,
                                serialization.PrivateFormat.TraditionalOpenSSL,
                                serialization.NoEncryption()))
    logs.LOG.info("generated backend TLS keypair in %s (san names=%s ips=%s)",
                  certs_dir(), names, ips)
    return {"ok": True, "created": True, "cert": cert, "lan_ip": lan,
            "san": {"dns": names, "ips": ips}}


def _tool(rel):
    """A backend script or asset, wherever it actually lives."""
    if rel.endswith(".pem"):
        return os.path.join(certs_dir(), rel)
    return config.tool_script(rel)


def port_open(port, host="127.0.0.1"):
    s = socket.socket()
    s.settimeout(0.4)
    try:
        return s.connect_ex((host, port)) == 0
    finally:
        s.close()


def state():
    # Create the keypair up front rather than reporting "have_cert: false" and
    # making the user wonder what is broken. Cheap after the first time.
    try:
        ensure_cert()
    except Exception as ex:
        logs.LOG.warning("could not prepare the backend keypair: %s", ex)
    cert = os.path.exists(_tool("cert.pem")) and os.path.exists(_tool("key.pem"))
    running = {m: (p.poll() is None) for m, p in _procs.items()}
    probe = probe_client_url()
    lan = netutil.lan_ipv4()
    port = config.CFG["backend_port"]
    return {
        "port": port,
        "listening": port_open(port),
        "running": running,
        "have_cert": cert,
        "cert_covers_lan": _cert_covers(),
        "tools_dir": config.tools_dir(),
        "lan_ip": lan,
        "our_ips": sorted(netutil.our_ips(lan)),
        "launch_backend": netutil.backend_ws_url(port),
        # rdata probe — True only if the Kunos client URL slot is gone.
        # Launch now always passes -backend=, so this is no longer required.
        "client_patched": probe.get("rdata_patched"),
        "client_url": probe,
    }


def _game_exe():
    exe = (config.CFG.get("game_exe") or "").strip()
    if exe and os.path.isfile(exe):
        return exe
    return detect.find("game_exe") or ""


def probe_client_url():
    """Read the client's rdata slot. Do not trust the .bak_prebackend file.

    Restoring the official URL leaves that backup in place, which is why the
    old check reported 'patched' after --restore.
    """
    exe = _game_exe()
    if not exe:
        return {"rdata_patched": None, "error": "game exe not found"}
    try:
        st = os.stat(exe)
        key = (exe, st.st_mtime_ns, st.st_size)
    except OSError as ex:
        return {"rdata_patched": None, "error": str(ex)}
    hit = _probe_cache.get(key)
    if hit:
        return hit
    # 90 MB; we only need to know whether one 58-byte string is still there.
    data = open(exe, "rb").read()
    off, kind, matched = _find_url_slot(data)
    has_kunos = kind == "kunos"
    info = {
        "exe": exe,
        "rdata_patched": None if kind is None else (kind == "local"),
        "kunos_url_present": has_kunos,
        "slot": hex(off) if off is not None else None,
        "slot_kind": kind,
        "intended": redirect_url(),
        "size": st.st_size,
    }
    _probe_cache.clear()
    _probe_cache[key] = info
    return info


def patched():
    """Is the rdata client-URL slot overwritten? None if we cannot tell."""
    return probe_client_url().get("rdata_patched")


def redirect_url():
    """The URL we write into the client slot and pass as -backend=."""
    return netutil.backend_ws_url(config.CFG["backend_port"])


def _local_url_markers():
    port = int(config.CFG["backend_port"])
    return [
        redirect_url().encode(),
        f"wss://localhost:{port}/communicationNode/dev".encode(),
        b"wss://localhost:448/communicationNode/dev",
        b"wss://127.0.0.1:448/communicationNode/dev",
    ]


def _find_url_slot(data):
    """(offset, kind, matched_prefix) for the 58-byte client URL slot."""
    i = data.find(KUNOS_CLIENT_URL)
    if i >= 0:
        return i, "kunos", KUNOS_CLIENT_URL
    for m in _local_url_markers():
        i = data.find(m)
        if i >= 0:
            return i, "local", m
    return None, None, None


def _game_running(exe=None):
    """True if AssettoCorsaEVO.exe is alive — we must not write over it."""
    from . import winproc
    return bool(winproc.pids_named("AssettoCorsaEVO"))


def apply_redirect():
    """Overwrite the Kunos client URL slot with our local backend URL.

    Same rules as patching.py: verify the site, back up once, write nothing
    unless the expected bytes are there. The slot is found by string, not
    offset, so a game update that keeps the URL still works.
    """
    exe = _game_exe()
    if not exe:
        return {"ok": False, "error": "game exe not found"}
    if _game_running(exe):
        return {"ok": False,
                "error": "the game is running — close it before patching"}
    new = redirect_url().encode()
    if len(new) > len(KUNOS_CLIENT_URL):
        return {"ok": False, "error": f"URL too long ({len(new)} > "
                                      f"{len(KUNOS_CLIENT_URL)})"}
    data = bytearray(open(exe, "rb").read())
    off, kind, matched = _find_url_slot(data)
    if off is None:
        return {"ok": False,
                "error": "neither the Kunos URL nor a local backend URL "
                         "was found — the game may have updated"}
    slot = len(KUNOS_CLIENT_URL)
    want = new + b"\x00" * (slot - len(new))
    if data[off:off + slot] == want:
        _probe_cache.clear()
        return {"ok": True, "already": True, "off": hex(off),
                "url": redirect_url()}
    if kind == "kunos" and data[off:off + slot] != KUNOS_CLIENT_URL:
        return {"ok": False, "error": "Kunos URL site is not intact — refusing"}
    # ⚠ The game usually lives under Program Files, where writing needs
    # elevation. Unelevated this raised a bare PermissionError that surfaced as
    # "patch failed" with no cause - and the patch is OPTIONAL anyway, so say
    # both things rather than leaving someone stuck on a step they do not need.
    bak = exe + ".bak_prebackend"
    try:
        if not os.path.exists(bak):
            shutil.copy2(exe, bak)
            logs.LOG.info("backend URL backup: %s", bak)
        data[off:off + slot] = want
        with open(exe, "wb") as fh:
            fh.write(data)
    except PermissionError:
        return {"ok": False, "needs_admin": True,
                "error": f"no permission to modify {exe}. Run ACECM as "
                         f"administrator, or skip this entirely and start the "
                         f"game with ACECM's Launch game button - that passes "
                         f"-backend= and needs no patch"}
    except OSError as ex:
        return {"ok": False, "error": f"could not write the client: {ex}"}
    _probe_cache.clear()
    logs.LOG.info("backend URL patched at %s -> %s", hex(off), redirect_url())
    return {"ok": True, "off": hex(off), "was": kind,
            "url": redirect_url(), "backup": bak}


def restore_redirect():
    """Put the official Kunos client URL back."""
    exe = _game_exe()
    if not exe:
        return {"ok": False, "error": "game exe not found"}
    if _game_running(exe):
        return {"ok": False,
                "error": "the game is running — close it before restoring"}
    data = bytearray(open(exe, "rb").read())
    if data.find(KUNOS_CLIENT_URL) >= 0:
        _probe_cache.clear()
        return {"ok": True, "already": True, "url": KUNOS_CLIENT_URL.decode()}
    off, kind, matched = _find_url_slot(data)
    if off is None:
        return {"ok": False,
                "error": "no local backend URL found to restore"}
    slot = len(KUNOS_CLIENT_URL)
    data[off:off + slot] = KUNOS_CLIENT_URL
    open(exe, "wb").write(data)
    _probe_cache.clear()
    logs.LOG.info("backend URL restored at %s", hex(off))
    return {"ok": True, "off": hex(off), "url": KUNOS_CLIENT_URL.decode()}


def start(mode="proxy"):
    if mode not in MODES:
        return {"ok": False, "error": f"unknown mode {mode}"}
    script = _tool(MODES[mode])
    if not os.path.exists(script):
        return {"ok": False, "error": f"missing {script}"}
    # Make the keypair rather than refusing: a shipped build has no
    # gencert.sh to run, and telling a user to go find one is not a feature.
    made = ensure_cert()
    if not made.get("ok"):
        return {"ok": False, "error": f"cannot create a TLS keypair: "
                                      f"{made.get('error')}"}
    stop()
    log = open(os.path.join(config.DATA, f"backend_{mode}.log"), "w",
               encoding="utf-8", errors="replace")
    # mode is "proxy" or "standalone"; both are our own scripts, launched
    # through the dispatcher so a frozen build re-invokes itself rather than
    # looking for a Python that is not there.
    tool = os.path.splitext(os.path.basename(script))[0]
    env = dict(os.environ)
    # The proxy loads the game's protobuf schemas and its TLS keypair; in a
    # shipped build neither sits next to the script, so point at both.
    from . import lobby, protos as protolib
    from . import servers as _servers
    if not os.path.exists(lobby.PATH):
        items = _servers.load()
        lobby.write(items[0] if items else {})
    env["ACECM_PROTOS"] = protolib.cache_dir()
    env["ACECM_CERTS"] = certs_dir()
    env["LOBBY_JSON"] = lobby.PATH
    lan = netutil.lan_ipv4()
    if lan:
        env["SERVER_IP"] = lan
        env["LAN_IP"] = lan
    env["OUR_IPS"] = ",".join(sorted(netutil.our_ips(lan)))
    env["PORT"] = str(config.CFG["backend_port"])
    # ⚠ Without this the log file is EMPTY on every user machine. Python
    # block-buffers stdout when it is a file rather than a console, so ~8 KB
    # of diagnostics sit in the buffer and are lost outright when the process
    # is killed. Asked a user for this log to debug an empty server browser
    # and got a zero-byte file - the one artefact that could explain the
    # failure could never contain anything.
    env["PYTHONUNBUFFERED"] = "1"
    cmd = config.tool_cmd(tool, [])
    p = subprocess.Popen(cmd, cwd=os.path.dirname(script), env=env,
                         stdout=log, stderr=subprocess.STDOUT)
    logs.launched(f"backend ({mode})", cmd, p.pid, log=log.name)
    _procs[mode] = p
    # ⚠ Spawning is not starting. If the port is taken, or an import blows up
    # (any_pb2 did exactly this in shipped builds), the child is dead within a
    # second and we would still hand back ok:True with a pid. Wait for it to
    # actually listen, and if it never does, say so with the reason.
    # ⚠ "something answers on the port" is NOT the test - that is true even
    # when our child died on bind and a stale backend owns the socket, which
    # is precisely the case that made this look healthy while nothing worked.
    # The listener must be OUR child.
    deadline = time.time() + 8.0
    port = config.CFG["backend_port"]
    owner = []
    while time.time() < deadline:
        owner = _listener_pids(port)
        if any(_is_descendant(o, p.pid) for o in owner):
            return {"ok": True, "pid": p.pid, "mode": mode, "port": port}
        if p.poll() is not None:
            break
        time.sleep(0.5)
    tail = [ln for ln in log_tail_text(mode).splitlines() if ln.strip()][-6:]
    if owner:
        why = (f"port {port} is already held by another process (pid "
               f"{owner[0]}), so this backend could not bind. Close the other "
               f"ACECM or backend window and start it again.")
    elif p.poll() is not None:
        why = (f"the backend exited immediately without listening on port "
               f"{port} - see the detail below")
    else:
        why = (f"the backend is running but never listened on port {port}")
    return {"ok": False, "pid": p.pid, "mode": mode, "port": port,
            "error": why, "detail": tail}


def _listener_pids(port):
    """Every pid listening on a local port (empty if none)."""
    out = []
    try:
        r = subprocess.run(["powershell", "-NoProfile", "-NonInteractive",
                            "-Command",
                            f"(Get-NetTCPConnection -State Listen -LocalPort "
                            f"{int(port)} -ErrorAction SilentlyContinue)"
                            f".OwningProcess"],
                           capture_output=True, text=True, timeout=10)
        for line in (r.stdout or "").split():
            if line.strip().isdigit():
                out.append(int(line))
    except Exception as ex:
        logs.LOG.info("listener lookup on %s: %s", port, ex)
    return out


def _is_descendant(pid, ancestor, depth=6):
    """Is pid the ancestor process, or a child/grandchild of it?

    ⚠ Needed because a onefile PyInstaller exe is TWO processes: the bootloader
    we spawn, and the real Python child it unpacks and runs. The socket belongs
    to the child, so testing the pid we launched says "not listening" for every
    frozen build - which is the shipped configuration.
    """
    seen = pid
    for _ in range(depth):
        if seen == ancestor:
            return True
        try:
            r = subprocess.run(
                ["powershell", "-NoProfile", "-NonInteractive", "-Command",
                 f"(Get-CimInstance Win32_Process -Filter "
                 f"'ProcessId={int(seen)}').ParentProcessId"],
                capture_output=True, text=True, timeout=10)
            txt = (r.stdout or "").strip()
        except Exception:
            return False
        if not txt.isdigit():
            return False
        seen = int(txt)
        if seen in (0, 4):
            return False
    return False


def _orphan_on_backend_port():
    """An ACECM backend from a PREVIOUS ACECM session still holding the port.

    ⚠ _procs only knows about children of THIS process. Close ACECM (or update
    it) while the proxy is running and the proxy keeps living, keeps port 448,
    and is invisible to us. Start the proxy again and the new one dies on bind
    while ACECM cheerfully reports the pid it just spawned - so the UI says
    "running", the log says "launched backend (proxy)", and the listener is
    actually the stale build. Every symptom points at the client.

    ⚠ Identify it by COMMAND LINE, not by exe name. Downloading the app twice
    gives you "ACECM(3).exe" and "ACECM(4).exe", and a name test misses the
    orphan in exactly the situation that creates one - updating.
    """
    port = config.CFG["backend_port"]
    tools = tuple(os.path.splitext(v)[0] for v in MODES.values())
    for pid in _listener_pids(port):
        if pid == os.getpid():
            continue
        try:
            r = subprocess.run(
                ["powershell", "-NoProfile", "-NonInteractive", "-Command",
                 f"(Get-CimInstance Win32_Process -Filter "
                 f"'ProcessId={pid}').CommandLine"],
                capture_output=True, text=True, timeout=10)
            cmd = (r.stdout or "").strip()
        except Exception as ex:
            logs.LOG.info("backend port owner lookup: %s", ex)
            continue
        if "--tool" in cmd and any(t in cmd for t in tools):
            return pid
    return None


def stop():
    for mode, p in list(_procs.items()):
        if p.poll() is None:
            p.terminate()
        _procs.pop(mode, None)
    # Reclaim the port from a backend we no longer have a handle to, or the
    # next start() silently loses the bind and everything downstream lies.
    pid = _orphan_on_backend_port()
    if pid:
        logs.LOG.info("reclaiming backend port %s from orphaned ACECM pid %s "
                      "(left over from a previous ACECM session)",
                      config.CFG["backend_port"], pid)
        subprocess.run(["taskkill", "/PID", str(pid), "/T", "/F"],
                       capture_output=True)
    return {"ok": True}


def log_tail_text(mode="proxy"):
    path = os.path.join(config.DATA, f"backend_{mode}.log")
    try:
        return open(path, encoding="utf-8", errors="replace").read()
    except OSError:
        return ""


def log(mode="proxy", lines=80):
    path = os.path.join(config.DATA, f"backend_{mode}.log")
    if not os.path.exists(path):
        return {"lines": []}
    txt = open(path, encoding="utf-8", errors="replace").read()
    return {"lines": txt.splitlines()[-lines:]}


CONTROL = "http://127.0.0.1:8093"


def join_state():
    """Is a game client attached to our backend and joinable?"""
    import urllib.request
    try:
        with urllib.request.urlopen(CONTROL + "/", timeout=1.0) as r:
            return {"control": True, **json.loads(r.read())}
    except Exception as ex:
        return {"control": False, "error": f"{type(ex).__name__}",
                "hint": "start the proxy backend to enable direct join"}


def server_list():
    """Every public EVO server, as captured by the proxy.

    ⚠ The proxy only sees this when a game client asks for it, so the list is a
    snapshot from the last time the browser was opened in-game - not a live
    query. Kunos will not answer us directly: the first frame of the session is
    a Steam auth ticket bound to a real client session, which we relay rather
    than possess.
    """
    import urllib.request
    cache = os.path.join(config.DATA, "server_list.json")
    try:
        with urllib.request.urlopen(CONTROL + "/servers", timeout=4) as r:
            got = {"ok": True, **json.loads(r.read())}
        # ⚠ Keep it. The list only passes through while the game is open, and
        # everything you would DO with it - see which servers need content you
        # lack, and download it - is work you want to do with the game CLOSED,
        # so the mods are in place before it next starts.
        try:
            json.dump(got, open(cache, "w", encoding="utf-8"))
        except OSError as ex:
            logs.LOG.warning("could not cache the server list: %s", ex)
        return got
    except Exception:
        pass
    try:
        got = json.load(open(cache, encoding="utf-8"))
        st = os.stat(cache)
        got.update({"ok": True, "cached": True,
                    "captured_at": got.get("captured_at") or int(st.st_mtime),
                    "note": "from the last time the in-game browser was open - "
                            "player counts and pings will be stale, but it is "
                            "enough to see what content a server needs"})
        return got
    except Exception as ex:
        return {"ok": False, "error": f"{type(ex).__name__}",
                "hint": "start the proxy backend, then open Multiplayer "
                        "in-game once so the list passes through - after that "
                        "it is remembered and usable with the game closed"}


def browser_chain():
    """Why the server browser is empty, as a checklist.

    ⚠ The list is not fetched - it is CAPTURED from the game's own lobby
    traffic, which only passes through us if the client is patched to use our
    backend. Every link can be missing silently: an unpatched client simply
    talks to Kunos and the browser stays empty forever, with nothing failing
    and nothing to see. "I followed the steps and it does not populate" is the
    expected experience when one link is off, so name them.
    """
    import os

    try:
        st = state()
    except Exception as ex:
        st = {"error": str(ex)}
    cu = st.get("client_url") or {}
    cached = os.path.join(config.DATA, "server_list.json")
    have_cache = os.path.isfile(cached)
    captured = 0
    if have_cache:
        try:
            captured = len(json.load(open(cached, encoding="utf-8"))
                           .get("servers") or [])
        except Exception:
            pass

    exe = _game_exe()
    try:
        live = join_state()
    except Exception:
        live = {}
    steps = [
        # ⚠ First, because without it "Launch game" quietly hands off to Steam,
        # which starts the game with no -backend= - so the client talks to
        # Kunos, nothing passes through the proxy, and every other link can be
        # green while the browser stays empty forever.
        {"ok": bool(exe),
         "what": "game executable located",
         "fix": "Set game_exe in Settings to AssettoCorsaEVO.exe. Without it "
                "ACECM has to start the game through Steam, which drops the "
                "-backend= flag and the list never reaches us",
         "detail": exe or ""},
        {"ok": bool(st.get("listening")),
         "what": f"backend proxy listening on :{st.get('port')}",
         "fix": "Start it on the Backend page"},
        {"ok": bool(st.get("have_cert")),
         "what": "TLS certificate present",
         "fix": "Generate it on the Backend page - the client will not "
                "connect to the proxy without one"},
        # ⚠ TWO ways to satisfy this, and the easy one was never mentioned.
        # ACECM's own Launch game passes -backend=, which does the same job
        # with no patching and no admin rights. The rdata patch only matters
        # when the game is started from Steam, which drops the flag.
        # ⚠ Ask the proxy whether a client is ACTUALLY on the socket. This used
        # to report the rdata patch instead, which is only one of the two ways
        # to satisfy it: launching with -backend= connects a client without
        # ever touching rdata, so a perfectly working setup read "missing"
        # here and sent people off patching a file they did not need. The
        # patch state stays as a fallback for when the game is not running.
        {"ok": bool(live.get("client_connected")
                    or st.get("client_patched") or cu.get("rdata_patched")),
         "what": "game client talks to the local backend",
         "fix": "Easiest: start the game with ACECM's 'Launch game' button - "
                "it passes -backend= and needs no patch. Starting from Steam "
                "drops that flag, and only then do you need 'Point client at "
                "us' on the Backend page (which needs ACECM running as "
                "administrator if the game is in Program Files)",
         "detail": ("a client is connected to the proxy right now"
                    if live.get("client_connected") else
                    "rdata patched, so a Steam-started game reaches us"
                    if (st.get("client_patched") or cu.get("rdata_patched"))
                    else "no client on the proxy socket"
                         + ("" if live.get("control") else
                            " (and the proxy's control port is not answering, "
                            "so it may have started only partly)"))},
        {"ok": captured > 0,
         "what": f"server list captured ({captured} servers)"
                 if captured else "server list captured",
         "fix": ("The client IS connected to us, so the link is fine - open "
                 "Multiplayer in-game now and the list is captured as it "
                 "loads. If it stays empty with the client connected, the "
                 "proxy could not reach Kunos upstream: check the backend log "
                 "on this page."
                 if live.get("client_connected") else
                 "With the three above done, open Multiplayer in-game once. "
                 "After that it is remembered and works with the game closed")},
    ]
    first = next((s for s in steps if not s["ok"]), None)
    return {"ok": first is None, "steps": steps,
            "blocked_on": first["what"] if first else None,
            "captured": captured}


def join(server_id, shape="bare", tcp=None, udp=None, password=""):
    """Push the client into a dedicated server.

    The url is join:<ip>:<tcp>[ :<udp>], NOT join:<profile-id>. A uuid here
    is how we previously connected to ':0'.
    """
    import urllib.request
    body = json.dumps({
        "id": server_id, "shape": shape,
        "tcp": tcp, "udp": udp or tcp, "password": password or "",
    }).encode()
    req = urllib.request.Request(CONTROL + "/join", data=body,
                                 headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=8) as r:
            return json.load(r)
    except urllib.error.HTTPError as ex:
        try:
            return json.load(ex)
        except Exception:
            return {"ok": False, "error": f"HTTP {ex.code}"}
    except Exception as ex:
        return {"ok": False,
                "error": f"{type(ex).__name__}: {ex}",
                "hint": "is the proxy backend running?"}


# Two je-if-flag-zero sites. NOPping them forces LocalAI on the *showroom*
# car at boot and the client then stops pumping lobby messages.
# Keep the original jumps; flip the live BSS flag after the menu is up.
_AI_FLAG_SITES = (
    (0x140FB358E, bytes([0x0F, 0x84, 0x3F, 0x01, 0x00, 0x00]), bytes([0x90] * 6)),
    (0x140FB38E8, bytes([0x74, 0x6C]), bytes([0x90] * 2)),
)
# cmp byte [rip+disp], 0 just before both sites resolves here.
_AI_FLAG_VA = 0x145D7BD78
_IMAGE_BASE = 0x140000000


def _text_off(va):
    return 0x600 + (va - 0x140001000)


def restore_ai_flag_jumps():
    """Put the original je-if-flag-zero bytes back.

    The NOP experiment made the paint-shop car LocalAI and froze the
    lobby reader (log stuck at 288KiB, go_to_server never Incoming).
    """
    exe = _game_exe()
    if not exe:
        return {"ok": False, "error": "game exe not found"}
    d = bytearray(open(exe, "rb").read())
    already = restored = 0
    need_write = False
    for va, orig, nop in _AI_FLAG_SITES:
        o = _text_off(va)
        have = bytes(d[o:o + len(orig)])
        if have == orig:
            already += 1
        elif have == nop:
            d[o:o + len(orig)] = orig
            restored += 1
            need_write = True
        else:
            return {"ok": False, "error": f"unexpected bytes at {va:#x}: {have.hex()}"}
    if need_write:
        if _game_running(exe):
            return {"ok": False, "error": "close the game first"}
        open(exe, "wb").write(d)
        logs.LOG.info("ai_player_car jumps restored (%s sites)", restored)
    return {"ok": True, "restored": restored, "already": already}


def apply_ai_player_flag():
    """Compatibility name — we restore jumps now, not NOP them."""
    return restore_ai_flag_jumps()


def _game_pids():
    from . import winproc
    return winproc.pids_named("AssettoCorsaEVO")


def poke_ai_player_flag(value=True):
    """Set FLAGS_ai_player_car in the running process after the menu exists.

    The flag is one BSS byte. Setting it at launch makes the showroom car
    LocalAI and the client stops handling go_to_server. Setting it here,
    once the dealership car already exists, only affects the next spawn
    (the dedicated-server join).
    """
    import ctypes
    from ctypes import wintypes

    pids = _game_pids()
    if not pids:
        return {"ok": False, "error": "game is not running"}
    k32 = ctypes.WinDLL("kernel32", use_last_error=True)
    psapi = ctypes.WinDLL("psapi", use_last_error=True)
    PROCESS_VM = 0x0438  # QUERY_INFO | VM_READ | VM_WRITE | VM_OP
    k32.OpenProcess.restype = wintypes.HANDLE
    k32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    psapi.EnumProcessModules.argtypes = [
        wintypes.HANDLE, ctypes.POINTER(ctypes.c_void_p),
        wintypes.DWORD, ctypes.POINTER(wintypes.DWORD)]
    k32.ReadProcessMemory.argtypes = [
        wintypes.HANDLE, ctypes.c_void_p, ctypes.c_void_p,
        ctypes.c_size_t, ctypes.POINTER(ctypes.c_size_t)]
    k32.WriteProcessMemory.argtypes = [
        wintypes.HANDLE, ctypes.c_void_p, ctypes.c_void_p,
        ctypes.c_size_t, ctypes.POINTER(ctypes.c_size_t)]
    want = b"\x01" if value else b"\x00"
    done = []
    for pid in pids:
        h = k32.OpenProcess(PROCESS_VM, False, pid)
        if not h:
            done.append({"pid": pid, "ok": False, "error": "OpenProcess"})
            continue
        try:
            mods = (ctypes.c_void_p * 8)()
            need = wintypes.DWORD()
            if not psapi.EnumProcessModules(
                    h, mods, ctypes.sizeof(mods), ctypes.byref(need)):
                done.append({"pid": pid, "ok": False, "error": "EnumProcessModules"})
                continue
            base = mods[0]
            addr = base + (_AI_FLAG_VA - _IMAGE_BASE)
            before = ctypes.create_string_buffer(1)
            n = ctypes.c_size_t()
            if not k32.ReadProcessMemory(h, ctypes.c_void_p(addr), before, 1,
                                         ctypes.byref(n)):
                done.append({"pid": pid, "ok": False, "error": "ReadProcessMemory",
                             "addr": hex(addr)})
                continue
            buf = ctypes.create_string_buffer(want)
            if not k32.WriteProcessMemory(h, ctypes.c_void_p(addr), buf, 1,
                                          ctypes.byref(n)):
                done.append({"pid": pid, "ok": False, "error": "WriteProcessMemory",
                             "addr": hex(addr), "before": before.raw.hex()})
                continue
            after = ctypes.create_string_buffer(1)
            k32.ReadProcessMemory(h, ctypes.c_void_p(addr), after, 1,
                                  ctypes.byref(n))
            rec = {"pid": pid, "ok": after.raw == want, "addr": hex(addr),
                   "base": hex(base), "before": before.raw.hex(),
                   "after": after.raw.hex()}
            done.append(rec)
            logs.LOG.info("poked FLAGS_ai_player_car pid=%s %s", pid, rec)
        finally:
            k32.CloseHandle(h)
    ok = any(x.get("ok") for x in done)
    return {"ok": ok, "pokes": done}


def launch_game(extra_args=None):
    """Start the client with -backend= so the rdata patch is optional.

    Steam Launch Options never reach this process (the game logs
    `Arguments: 1`). Starting the exe ourselves does. SteamAppId is set so
    steam_api64.dll still finds the running Steam client.
    extra_args: more CLI flags (e.g. -ai_enable_evo_next).
    """
    exe = _game_exe()
    if not exe:
        # ⚠ NOT ok. Steam launches the game without our -backend=, so the
        # client talks to Kunos and nothing reaches the proxy - the browser
        # stays empty. This used to return ok:True with a warning nobody saw,
        # so "Launch game" looked like it had worked and the real problem
        # (we cannot find the exe) never surfaced.
        appid = config.CFG.get("steam_appid")
        if appid:
            os.startfile(f"steam://rungameid/{appid}")   # noqa: S606
            return {"ok": False, "via": "steam", "started": True,
                    "error": "the game exe could not be found, so it was "
                             "started through Steam WITHOUT -backend= - the "
                             "server browser will not fill. Set game_exe in "
                             "Settings to AssettoCorsaEVO.exe and launch "
                             "again."}
        return {"ok": False, "error": "set game_exe in Settings"}
    url = netutil.backend_ws_url(config.CFG["backend_port"])
    env = dict(os.environ)
    appid = str(config.CFG.get("steam_appid") or "3058630")
    env["SteamAppId"] = appid
    env["SteamGameId"] = appid
    # gflags: extra argv is often dropped (log still says Arguments: 2).
    # The binary documents env: export FLAGS_flag1=value
    extra_args = list(extra_args or [])
    if any("ai_player_car" in a for a in extra_args):
        env["FLAGS_ai_player_car"] = "true"
    if any("ai_enable_evo_next" in a for a in extra_args):
        env["FLAGS_ai_enable_evo_next"] = "true"
    cwd = os.path.dirname(exe)
    flagfile = os.path.join(config.DATA, "evo.flags")
    try:
        lines = []
        if env.get("FLAGS_ai_player_car"):
            lines.append("--ai_player_car")
        if env.get("FLAGS_ai_enable_evo_next"):
            lines.append("--ai_enable_evo_next")
        if lines:
            open(flagfile, "w", encoding="ascii").write("\n".join(lines) + "\n")
            extra_args.append(f"--flagfile={flagfile}")
            extra_args.append(f"-flagfile={flagfile}")
    except OSError:
        pass
    cmd = [exe, f"-backend={url}"]
    cmd.extend(extra_args)
    subprocess.Popen(cmd, cwd=cwd, env=env)
    logs.launched("game client", cmd, None, backend=url,
                  flags_env={k: env[k] for k in env if k.startswith("FLAGS_")})
    return {"ok": True, "via": "exe", "backend": url,
            "rdata_patched": probe_client_url().get("rdata_patched"),
            "flags_env": {k: env[k] for k in env if k.startswith("FLAGS_")}}
