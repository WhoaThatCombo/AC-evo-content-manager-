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
import struct
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
    # ⚠ Do NOT probe 448 with a plain TCP connect. That port is TLS, so a
    # health-check socket sits in websockets' CONNECTING state forever and
    # looks identical to "the game reached us and TLS hung". Ask the OS
    # who is listening instead.
    return {
        "port": port,
        "listening": bool(_listener_pids(port)),
        "running": running,
        "have_cert": cert,
        "cert_covers_lan": _cert_covers(),
        "tools_dir": config.tools_dir(),
        "lan_ip": lan,
        "our_ips": sorted(netutil.our_ips(lan)),
        "launch_backend": netutil.backend_ws_url(port),
        # rdata probe — True only if the Kunos client URL slot is gone.
        # Steam relaunches the exe with Arguments: 1, so -backend= does
        # not reach the process. This slot is what actually steers it.
        "client_patched": probe.get("rdata_patched"),
        "client_url": probe,
        "inspector_patched": probe_inspector().get("inspector_patched"),
        "inspector_port": INSPECTOR_PORT,
        "game_backend": game_backend_seen(),
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
    # "patch failed" with no cause. This rewrite is required (Steam drops
    # -backend=), so say that rather than offering a skip that does not work.
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
                         f"administrator once — Steam drops -backend=, so "
                         f"this rewrite is what actually points the client "
                         f"at us"}
    except OSError as ex:
        return {"ok": False, "error": f"could not write the client: {ex}"}
    _probe_cache.clear()
    logs.LOG.info("backend URL patched at %s -> %s", hex(off), redirect_url())
    return {"ok": True, "off": hex(off), "was": kind,
            "url": redirect_url(), "backup": bak}


# Gameface/cohtml inspector. Stock sets DebuggerPort to 0xFFFFFFFF (invalid)
# and the enable flag to 0, so :9444 never listens. Drive presses Start/Join
# through that port — without this rewrite a fresh install launches and sits
# on the home menu forever. The two writes sit 7 bytes apart in the same
# instruction run, so re-deriving after an update is a single combined-pattern
# search rather than two independent ones. Re-derived 2026-08-25 against
# buildid 24331595 (was FUN_140ce58d0 in 0.8.1; function moved +0x106b10).
INSPECTOR_PORT = int(os.environ.get("INSPECTOR_PORT", "9444"))
_INSPECTOR_PORT_VA = 0x140DEC69C
_INSPECTOR_PORT_ORIG = bytes.fromhex("c74538ffffffff")  # mov [rbp+0x38], -1
_INSPECTOR_FLAG_VA = 0x140DEC6A3
_INSPECTOR_FLAG_ORIG = bytes.fromhex("66c7453c0001")    # flag 0, next byte 1
_INSPECTOR_FLAG_NEW = bytes.fromhex("66c7453c0101")     # flag 1
_inspector_cache = {}


def _va_to_file(data, va):
    e = struct.unpack_from("<I", data, 0x3C)[0]
    n = struct.unpack_from("<H", data, e + 6)[0]
    opt = struct.unpack_from("<H", data, e + 20)[0]
    ib = struct.unpack_from("<Q", data, e + 24 + 24)[0]
    rva = va - ib
    for i in range(n):
        vs, va_s, rs, ra = struct.unpack_from(
            "<IIII", data, e + 24 + opt + i * 40 + 8)
        if va_s <= rva < va_s + max(vs, rs):
            return ra + (rva - va_s)
    raise ValueError(f"VA {va:#x} is not in any section")


def _inspector_want_port():
    return _INSPECTOR_PORT_ORIG[:3] + struct.pack("<I", INSPECTOR_PORT)


def probe_inspector():
    """Is the Gameface inspector enabled in the exe? None if we cannot tell."""
    exe = _game_exe()
    if not exe:
        return {"inspector_patched": None, "error": "game exe not found"}
    try:
        st = os.stat(exe)
        key = (exe, st.st_mtime_ns, st.st_size, INSPECTOR_PORT)
    except OSError as ex:
        return {"inspector_patched": None, "error": str(ex)}
    hit = _inspector_cache.get(key)
    if hit:
        return hit
    try:
        data = open(exe, "rb").read()
        pf = _va_to_file(data, _INSPECTOR_PORT_VA)
        gf = _va_to_file(data, _INSPECTOR_FLAG_VA)
    except (OSError, ValueError, struct.error) as ex:
        info = {"exe": exe, "inspector_patched": None, "error": str(ex)}
        _inspector_cache.clear()
        _inspector_cache[key] = info
        return info
    port_b = bytes(data[pf:pf + len(_INSPECTOR_PORT_ORIG)])
    flag_b = bytes(data[gf:gf + len(_INSPECTOR_FLAG_ORIG)])
    want = _inspector_want_port()
    info = {
        "exe": exe,
        "inspector_patched": port_b == want and flag_b == _INSPECTOR_FLAG_NEW,
        "port_site": hex(_INSPECTOR_PORT_VA),
        "flag_site": hex(_INSPECTOR_FLAG_VA),
        "port_bytes": port_b.hex(),
        "flag_bytes": flag_b.hex(),
        "port": INSPECTOR_PORT,
        "known": (port_b in (want, _INSPECTOR_PORT_ORIG)
                  and flag_b in (_INSPECTOR_FLAG_ORIG, _INSPECTOR_FLAG_NEW)),
    }
    _inspector_cache.clear()
    _inspector_cache[key] = info
    return info


def apply_inspector():
    """Turn on Gameface DevTools on :9444 so Drive can press Start/Join.

    Same rules as apply_redirect: verify the site, back up once, write
    nothing unless the expected bytes are there. Steam drops argv, so
    there is no launch-flag substitute.
    """
    exe = _game_exe()
    if not exe:
        return {"ok": False, "error": "game exe not found"}
    if not (1 <= INSPECTOR_PORT <= 65536):
        return {"ok": False, "error": f"inspector port {INSPECTOR_PORT} "
                                      "is not a valid TCP port"}
    try:
        data = bytearray(open(exe, "rb").read())
        pf = _va_to_file(data, _INSPECTOR_PORT_VA)
        gf = _va_to_file(data, _INSPECTOR_FLAG_VA)
    except (OSError, ValueError, struct.error) as ex:
        return {"ok": False,
                "error": f"could not locate the inspector sites: {ex}"}
    port_b = bytes(data[pf:pf + len(_INSPECTOR_PORT_ORIG)])
    flag_b = bytes(data[gf:gf + len(_INSPECTOR_FLAG_ORIG)])
    want = _inspector_want_port()
    if port_b == want and flag_b == _INSPECTOR_FLAG_NEW:
        _inspector_cache.clear()
        return {"ok": True, "already": True, "port": INSPECTOR_PORT}
    if port_b not in (want, _INSPECTOR_PORT_ORIG):
        return {"ok": False,
                "error": f"unexpected bytes at inspector port site "
                         f"{_INSPECTOR_PORT_VA:#x}: {port_b.hex()} "
                         f"(expected {_INSPECTOR_PORT_ORIG.hex()}). "
                         f"the game may have updated — Drive cannot press "
                         f"Start until this is re-derived"}
    if flag_b not in (_INSPECTOR_FLAG_ORIG, _INSPECTOR_FLAG_NEW):
        return {"ok": False,
                "error": f"unexpected bytes at inspector flag site "
                         f"{_INSPECTOR_FLAG_VA:#x}: {flag_b.hex()}"}
    if _game_running(exe):
        return {"ok": False,
                "error": "the game is running — close it before enabling "
                         "the menu inspector"}
    bak = exe + ".bak_preinspector"
    try:
        if not os.path.exists(bak):
            shutil.copy2(exe, bak)
            logs.LOG.info("inspector backup: %s", bak)
        data[pf:pf + len(want)] = want
        data[gf:gf + len(_INSPECTOR_FLAG_NEW)] = _INSPECTOR_FLAG_NEW
        with open(exe, "wb") as fh:
            fh.write(data)
    except PermissionError:
        return {"ok": False, "needs_admin": True,
                "error": f"no permission to modify {exe}. Run ACECM as "
                         f"administrator once — Drive talks to the menu "
                         f"on :{INSPECTOR_PORT}, and stock EVO leaves that "
                         f"inspector off"}
    except OSError as ex:
        return {"ok": False, "error": f"could not write the client: {ex}"}
    _inspector_cache.clear()
    logs.LOG.info("inspector enabled: DebuggerPort %s, flag 1 (sites %s / %s)",
                  INSPECTOR_PORT, hex(_INSPECTOR_PORT_VA),
                  hex(_INSPECTOR_FLAG_VA))
    return {"ok": True, "port": INSPECTOR_PORT, "backup": bak,
            "off": hex(pf)}


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
    # ⚠ The proxy needs the game's protobuf schemas to understand lobby traffic,
    # and they are extracted from the user's own exe rather than shipped. That
    # extraction only ever happened as a side effect of opening Game Settings,
    # which nobody has to do - so on a machine where it never ran, the proxy
    # starts, binds, relays raw bytes, and captures NOTHING, while every
    # indicator reads healthy. Extract here, and say so plainly if we cannot.
    env["ACECM_PROTOS"] = protolib.cache_dir()
    try:
        if not protolib.has("BackendMessage"):
            got = protolib.extract()
            logs.LOG.info("schema extraction before backend start: %s", got)
    except Exception as ex:
        logs.LOG.warning("schema extraction before backend start failed: %s", ex)
    if not protolib.has("BackendMessage"):
        return {"ok": False, "error":
                "the game's message schemas are missing, so the backend could "
                "read lobby traffic but never understand it - the server "
                "browser would stay empty with everything looking fine. They "
                "are read from your own AssettoCorsaEVO.exe: set game_dir in "
                "Settings to your Assetto Corsa EVO install and start it again",
                "protos": protolib.cache_dir()}
    env["ACECM_CERTS"] = certs_dir()
    env["LOBBY_JSON"] = lobby.PATH
    lan = netutil.lan_ipv4()
    if lan:
        env["SERVER_IP"] = lan
        env["LAN_IP"] = lan
    env["OUR_IPS"] = ",".join(sorted(netutil.our_ips(lan)))
    env["PORT"] = str(config.CFG["backend_port"])
    env["BACKEND_LISTEN"] = (
        (config.CFG.get("backend_listen") or "127.0.0.1").strip()
        or "127.0.0.1")
    # ⚠ Without this the log file is EMPTY on every user machine. Python
    # block-buffers stdout when it is a file rather than a console, so ~8 KB
    # of diagnostics sit in the buffer and are lost outright when the process
    # is killed. Asked a user for this log to debug an empty server browser
    # and got a zero-byte file - the one artefact that could explain the
    # failure could never contain anything.
    env["PYTHONUNBUFFERED"] = "1"
    cmd = config.tool_cmd(tool, [])
    from . import winproc
    p = winproc.hidden_popen(cmd, cwd=os.path.dirname(script), env=env,
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
    # Generous, because the child is another onefile exe: it unpacks ~70 MB and
    # Defender scans it on first run. 8s was fine on a warm SSD and short on a
    # cold machine, which is the machine that needs the check to be right.
    deadline = time.time() + 30.0
    port = config.CFG["backend_port"]
    owner = []
    while time.time() < deadline:
        owner = _listener_pids(port)
        if any(_is_descendant(o, p.pid) for o in owner):
            return {"ok": True, "pid": p.pid, "mode": mode, "port": port}
        if p.poll() is not None:
            break
        time.sleep(0.5)
    # ⚠ Do not leave a late child running after calling it a failure. On a slow
    # machine (onefile unpack + a Defender scan of a 70 MB exe) it can bind
    # AFTER we time out; we then report "never listened" while it quietly takes
    # the port, and the obvious retry fails with "already held" - blaming the
    # user for our own leftover. Either it started in time or it does not run.
    if p.poll() is None:
        try:
            subprocess.run(["taskkill", "/PID", str(p.pid), "/T", "/F"],
                           capture_output=True, timeout=15)
        except Exception:
            p.terminate()
        _procs.pop(mode, None)
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
    try:
        from . import winproc
        return winproc.tcp_listen_pids(int(port))
    except Exception as ex:
        logs.LOG.info("listener lookup on %s: %s", port, ex)
        return []


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
        # ⚠ Match the tool NAME only. Requiring "--tool" as well matched the
        # frozen form ("ACECM.exe --tool acevo_proxy") but missed the source
        # form ("python -u tools/acevo_proxy.py"), so running from a git
        # checkout never reclaimed the port and start() just failed instead.
        if any(t in cmd for t in tools):
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
        from . import winproc
        if not winproc.tcp_listen_pids(8093):
            return {"control": False, "error": "proxy not listening",
                    "hint": "start the proxy backend to enable direct join"}
        with urllib.request.urlopen(CONTROL + "/", timeout=0.4) as r:
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
    # Proxy control is :8093. urlopen's 4s timeout ran on every Server
    # browser visit when the backend was down, which is how that menu
    # felt frozen after the UI script started working again.
    listening = False
    try:
        from . import winproc
        listening = bool(winproc.tcp_listen_pids(8093))
    except Exception:
        listening = True
    try:
        if not listening:
            raise OSError("proxy control is not listening")
        with urllib.request.urlopen(CONTROL + "/servers", timeout=1.5) as r:
            got = {"ok": True, **json.loads(r.read())}
        # ⚠ Keep it. The list only passes through while the game is open, and
        # everything you would DO with it - see which servers need content you
        # lack, and download it - is work you want to do with the game CLOSED,
        # so the mods are in place before it next starts.
        # Do not clobber a good snapshot with an empty live reply (game closed
        # or browser not opened this session).
        live = got.get("servers") or []
        if live:
            try:
                json.dump(got, open(cache, "w", encoding="utf-8"))
            except OSError as ex:
                logs.LOG.warning("could not cache the server list: %s", ex)
            return got
        raise OSError("proxy returned an empty server list")
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
        # ⚠ The only honest "the game will talk to us" signal is either a
        # live socket or a rewritten rdata URL. -backend= is passed on
        # Launch, but Steam relaunches the exe with Arguments: 1, so the
        # flag never arrives. Claiming Launch was enough made a fresh
        # (unpatched) install look configured while the client still
        # dialled Kunos and the browser stayed empty.
        {"ok": bool(live.get("client_connected")
                    or st.get("client_patched") or cu.get("rdata_patched")),
         "what": "game client talks to the local backend",
         "fix": "Close the game, then Launch from ACECM. Launch rewrites the "
                "lobby URL in the exe first — that is required. Steam drops "
                "-backend=, so an unpatched client never reaches this proxy. "
                "If the rewrite says it needs administrator, run ACECM as "
                "admin once (Program Files). Steam 'Verify integrity' undoes "
                "the rewrite.",
         "detail": ("a client is connected to the proxy right now"
                    if live.get("client_connected") else
                    "rdata rewritten, so the next game start reaches us"
                    if (st.get("client_patched") or cu.get("rdata_patched"))
                    else "rdata still points at Kunos and no client is on "
                         "the proxy"
                         + ("" if live.get("control") else
                            " (and the proxy's control port is not answering, "
                            "so it may have started only partly)")
                         + (_backend_seen_note(st.get("game_backend") or {})))},
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
# Re-derived 2026-08-26 against buildid 24331595 (was 0x140FB358E/0x140FB38E8;
# found by tracing the ai_player_car gflag registration call to its BSS
# storage byte, then finding which of 216 gap-matched je pairs actually reads
# it - bytes at the new sites are identical to the old ones, just relocated).
_AI_FLAG_SITES = (
    (0x1410B171E, bytes([0x0F, 0x84, 0x3F, 0x01, 0x00, 0x00]), bytes([0x90] * 6)),
    (0x1410B1A78, bytes([0x74, 0x6C]), bytes([0x90] * 2)),
)
# cmp byte [rip+disp], 0 just before both sites resolves here.
_AI_FLAG_VA = 0x1467120A9
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


def _backend_seen_note(seen):
    url = (seen or {}).get("url") or ""
    if not url:
        return ""
    if "127.0.0.1" in url or "localhost" in url:
        return f" (game log: {url})"
    return (f" (game log still shows {url} — Steam dropped -backend= "
            "and rdata was not rewritten)")


_game_backend_cache = {}


def game_backend_seen():
    """Last 'Connecting to backend at …' line from the newest ACE log.

    That line is the only proof of where the *running* client actually
    dialled. ACECM can pass -backend= and still watch the game talk to
    Kunos if Steam ate the flag.
    """
    from . import detect
    ace = detect.find("ace_dir") or ""
    folder = os.path.join(ace, "Logs") if ace else ""
    if not folder or not os.path.isdir(folder):
        return {"url": None, "log": None}
    try:
        names = [n for n in os.listdir(folder)
                 if n.lower().startswith("log-") and n.lower().endswith(".txt")]
    except OSError:
        return {"url": None, "log": None}
    if not names:
        return {"url": None, "log": None}
    names.sort(reverse=True)
    path = os.path.join(folder, names[0])
    try:
        key = (path, os.path.getmtime(path), os.path.getsize(path))
    except OSError:
        return {"url": None, "log": path}
    hit = _game_backend_cache.get(key)
    if hit:
        return hit
    url = None
    try:
        # The connecting line is near the start; 2 MB is plenty.
        text = open(path, encoding="utf-8", errors="replace").read(2_000_000)
    except OSError:
        return {"url": None, "log": path}
    needle = "Connecting to backend at "
    for line in text.splitlines():
        i = line.find(needle)
        if i >= 0:
            url = line[i + len(needle):].strip()
    info = {"url": url, "log": path}
    _game_backend_cache.clear()
    _game_backend_cache[key] = info
    return info


def launch_game(extra_args=None):
    """Start the client, rewriting rdata first so Steam cannot skip us.

    Steam Launch Options never reach this process (the game logs
    `Arguments: 1`). Starting the exe ourselves still often ends the same
    way: steam_api hands off to Steam, Steam starts a fresh process, and
    -backend= is gone. The rewritten URL in the exe is what actually
    steers a stock client. SteamAppId is set so steam_api64.dll still
    finds the running Steam client.
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
                             "started through Steam WITHOUT a rewritten "
                             "lobby URL - the server browser will not fill. "
                             "Set game_exe in Settings to AssettoCorsaEVO.exe "
                             "and launch again."}
        return {"ok": False, "error": "set game_exe in Settings"}

    # Rewrite the exe before we start anything. On a fresh install the
    # stock client still talks to Kunos AND has the menu inspector off,
    # so Drive launches the game and then sits on the home screen.
    # Cannot write while the process is running.
    patch = None
    inspector = None
    if not probe_inspector().get("inspector_patched"):
        if _game_running(exe):
            return {"ok": False,
                    "error": "the game is already running and its menu "
                             "inspector is still off. Close it completely, "
                             "then Launch again — ACECM will enable the "
                             "inspector so Drive can press Start."}
        inspector = apply_inspector()
        if not inspector.get("ok"):
            extra = ""
            if inspector.get("needs_admin"):
                extra = " Run ACECM as administrator once to allow the write."
            return {"ok": False,
                    "error": (inspector.get("error") or "could not enable "
                              "the menu inspector") + extra,
                    "needs_admin": inspector.get("needs_admin"),
                    "inspector": inspector}
    if not probe_client_url().get("rdata_patched"):
        if _game_running(exe):
            return {"ok": False,
                    "error": "the game is already running and still points "
                             "at the official lobby. Close it completely, "
                             "then Launch again — ACECM will rewrite the "
                             "URL first. -backend= is dropped by Steam and "
                             "is not enough on its own."}
        patch = apply_redirect()
        if not patch.get("ok"):
            extra = ""
            if patch.get("needs_admin"):
                extra = " Run ACECM as administrator once to allow the write."
            return {"ok": False,
                    "error": (patch.get("error") or "could not rewrite the "
                              "client lobby URL") + extra,
                    "needs_admin": patch.get("needs_admin"),
                    "patch": patch}

    # A Kunos content update replaces the client's content.kspkg wholesale,
    # wiping tracks.table rows we wrote even though the track's own loose
    # files survive - indistinguishable from "never registered" until the
    # game commits to loading it and dies on an empty path. Same fix as the
    # inspector/redirect patches above: catch it before Launch, not after a
    # crash. Cheap when there is nothing to fix - it is a table diff, not a
    # rewrite, when every track is already registered.
    if not _game_running(exe):
        try:
            from . import tracks as trackdeploy
            redec = trackdeploy.redeclare_client_tracks()
            if redec.get("redeclared"):
                logs.LOG.info("auto-redeclared %d client track(s) wiped by "
                              "an update before launch: %s",
                              len(redec["redeclared"]),
                              ", ".join(r["folder"] for r in redec["redeclared"]))
        except Exception as ex:
            logs.LOG.warning("auto-redeclare client tracks before launch: %s", ex)

    url = netutil.backend_ws_url(config.CFG["backend_port"])
    env = dict(os.environ)
    appid = str(config.CFG.get("steam_appid") or "3058630")
    # Belt and braces: still pass the flag AND FLAGS_backend. They only
    # help if Steam does not relaunch the process; rdata is the real fix.
    env["FLAGS_backend"] = url
    extra_args = list(extra_args or [])
    if any("ai_player_car" in a for a in extra_args):
        env["FLAGS_ai_player_car"] = "true"
    if any("ai_enable_evo_next" in a for a in extra_args):
        env["FLAGS_ai_enable_evo_next"] = "true"
    # Steam relaunches with Arguments: 1 and drops argv. gflags still
    # reads FLAGS_* from the environment, so string flags like
    # startup_gamemode have to live here, not only on the command line.
    for a in extra_args:
        if a.startswith("--"):
            a = a[2:]
        elif a.startswith("-"):
            a = a[1:]
        if "=" in a:
            k, v = a.split("=", 1)
            if k and v and k.isidentifier():
                env["FLAGS_" + k] = v
    cwd = os.path.dirname(exe)
    flagfile = os.path.join(config.DATA, "evo.flags")
    try:
        lines = [f"--backend={url}"]
        if env.get("FLAGS_ai_player_car"):
            lines.append("--ai_player_car")
        if env.get("FLAGS_ai_enable_evo_next"):
            lines.append("--ai_enable_evo_next")
        for key, val in env.items():
            if key.startswith("FLAGS_") and key not in (
                    "FLAGS_backend", "FLAGS_ai_player_car",
                    "FLAGS_ai_enable_evo_next"):
                lines.append(f"--{key[6:]}={val}")
        open(flagfile, "w", encoding="ascii").write("\n".join(lines) + "\n")
        extra_args.append(f"--flagfile={flagfile}")
        extra_args.append(f"-flagfile={flagfile}")
        extra_args.append("--fromenv=startup_gamemode,load_single_car,backend")
        extra_args.append("--tryfromenv=startup_gamemode,load_single_car,backend")
    except OSError:
        pass
    # steam:// is a no-op (or a permission toast) if Steam is still
    # starting. Open the client first and give it ~20s rather than
    # handing Drive a "launched" game that never appears.
    steam = ensure_steam()
    if not steam.get("ok"):
        return {"ok": False,
                "error": steam.get("error") or "Steam is not running — "
                         "open Steam, then Drive again",
                "steam": steam}

    # Do NOT Popen the game exe from ACECM. steam_api then checks ownership
    # on THIS process's token. If ACECM was run as admin to patch rdata, or
    # the friend is on Family Share, Steam answers
    # "User has not permission to run this product" and the game never
    # opens. steam:// goes through the unelevated Steam client, which is
    # the same path as the Play button. rdata already has our lobby URL;
    # Steam still drops argv.
    via, cmd = _start_via_steam(appid)
    if not via:
        env["SteamAppId"] = appid
        env["SteamGameId"] = appid
        cmd = [exe, f"-backend={url}", f"--backend={url}"]
        cmd.extend(extra_args)
        via = "exe"
        subprocess.Popen(cmd, cwd=cwd, env=env)
    logs.launched("game client", cmd, None, backend=url, via=via,
                  rdata_patched=probe_client_url().get("rdata_patched"),
                  inspector_patched=probe_inspector().get("inspector_patched"),
                  steam=steam,
                  flags_env={k: env[k] for k in env if k.startswith("FLAGS_")})
    return {"ok": True, "via": via, "backend": url,
            "rdata_patched": probe_client_url().get("rdata_patched"),
            "inspector_patched": probe_inspector().get("inspector_patched"),
            "patch": patch, "inspector": inspector, "steam": steam,
            "flags_env": {k: env[k] for k in env if k.startswith("FLAGS_")}}


def _steam_running():
    from . import winproc
    return bool(winproc.pids_named("steam"))


def _steam_client_ready():
    """steam.exe plus the helper that comes up after the client is usable."""
    from . import winproc
    return bool(winproc.pids_named("steam")) and bool(
        winproc.pids_named("steamwebhelper"))


def ensure_steam(wait=20.0):
    """Make sure Steam is actually up before steam://rungameid.

    A cold start of steam.exe is not ready to own a launch: the URL is
    dropped or Steam toasts a permission error. If the client is already
    signed in this returns immediately. Otherwise start steam.exe -silent
    and wait up to `wait` seconds for steamwebhelper.
    """
    if _steam_client_ready():
        return {"ok": True, "already": True}
    root = detect.steam_root()
    steam = os.path.join(root or "", "steam.exe")
    started = False
    if not _steam_running():
        if not os.path.isfile(steam):
            return {"ok": False,
                    "error": "Steam is not running and steam.exe was not "
                             "found. Open Steam, then try again"}
        try:
            subprocess.Popen([steam, "-silent"], cwd=root or None)
            started = True
            logs.LOG.info("started Steam; waiting up to %.0fs for the client",
                          wait)
        except OSError as ex:
            return {"ok": False, "error": f"could not start Steam: {ex}"}
    else:
        logs.LOG.info("Steam is up but still starting; waiting up to %.0fs",
                      wait)
    deadline = time.time() + wait
    while time.time() < deadline:
        if _steam_client_ready():
            time.sleep(0.8)
            return {"ok": True, "started": started, "ready": True,
                    "waited": round(wait - max(0.0, deadline - time.time()), 1)}
        time.sleep(0.4)
    if _steam_running():
        logs.LOG.warning("Steam did not finish starting in %.0fs — "
                         "launching the game anyway", wait)
        return {"ok": True, "started": started, "ready": False,
                "waited": wait}
    return {"ok": False,
            "error": f"Steam did not open within {int(wait)}s — "
                     "open Steam yourself, then Drive again"}


def _start_via_steam(appid):
    """Open the game as the logged-in Steam user, not as ACECM's token."""
    url = f"steam://rungameid/{appid}"
    # explorer.exe is not elevated even when ACECM is. That is what
    # makes Family Share / "run as admin to patch" actually launch.
    windir = os.environ.get("WINDIR", r"C:\Windows")
    explorer = os.path.join(windir, "explorer.exe")
    if os.path.isfile(explorer):
        try:
            subprocess.Popen([explorer, url])
            return "steam-url", [explorer, url]
        except OSError:
            pass
    try:
        os.startfile(url)  # noqa: S606
        return "steam-url", [url]
    except OSError:
        pass
    root = detect.steam_root()
    steam = os.path.join(root or "", "steam.exe")
    if os.path.isfile(steam):
        try:
            subprocess.Popen([steam, "-applaunch", str(appid)])
            return "steam", [steam, "-applaunch", str(appid)]
        except OSError:
            pass
    return "", []
