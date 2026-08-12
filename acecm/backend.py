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
import socket
import subprocess
import sys

from . import config, logs

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


def ensure_cert(force=False):
    """Create a self-signed keypair if we do not have one yet."""
    cert, key = cert_paths()
    if not force and os.path.exists(cert) and os.path.exists(key):
        return {"ok": True, "existing": True, "cert": cert}
    try:
        import datetime
        from cryptography import x509
        from cryptography.hazmat.primitives import hashes, serialization
        from cryptography.hazmat.primitives.asymmetric import rsa
        from cryptography.x509.oid import NameOID
    except Exception as ex:
        return {"ok": False, "error": f"cryptography unavailable: {ex}"}

    k = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "localhost")])
    now = datetime.datetime.now(datetime.timezone.utc)
    crt = (x509.CertificateBuilder()
           .subject_name(name).issuer_name(name).public_key(k.public_key())
           .serial_number(x509.random_serial_number())
           .not_valid_before(now - datetime.timedelta(days=1))
           .not_valid_after(now + datetime.timedelta(days=3650))
           .add_extension(x509.SubjectAlternativeName([
               x509.DNSName("localhost"),
               x509.DNSName("b.gk.sd"),
               x509.IPAddress(__import__("ipaddress").ip_address("127.0.0.1")),
           ]), critical=False)
           .sign(k, hashes.SHA256()))
    with open(cert, "wb") as f:
        f.write(crt.public_bytes(serialization.Encoding.PEM))
    with open(key, "wb") as f:
        f.write(k.private_bytes(serialization.Encoding.PEM,
                                serialization.PrivateFormat.TraditionalOpenSSL,
                                serialization.NoEncryption()))
    logs.LOG.info("generated a self-signed backend keypair in %s", certs_dir())
    return {"ok": True, "created": True, "cert": cert}


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
    return {
        "port": config.CFG["backend_port"],
        "listening": port_open(config.CFG["backend_port"]),
        "running": running,
        "have_cert": cert,
        "tools_dir": config.tools_dir(),
        # the client only talks to us once its backend URL is redirected
        "client_patched": patched(),
    }


def patched():
    """Is the client currently pointed at our backend?

    patch_backend_url.py writes a marker backup next to the exe; its presence
    is the honest signal, since we cannot read the client's memory here.
    """
    game = config.CFG.get("game_exe") or ""
    if game and os.path.exists(game + ".bak_prebackend"):
        return True
    return None          # unknown - game path not configured


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
    from . import protos as protolib
    env["ACECM_PROTOS"] = protolib.cache_dir()
    env["ACECM_CERTS"] = certs_dir()
    cmd = config.tool_cmd(tool, [])
    p = subprocess.Popen(cmd, cwd=os.path.dirname(script), env=env,
                         stdout=log, stderr=subprocess.STDOUT)
    logs.launched(f"backend ({mode})", cmd, p.pid, log=log.name)
    _procs[mode] = p
    return {"ok": True, "pid": p.pid, "mode": mode}


def stop():
    for mode, p in list(_procs.items()):
        if p.poll() is None:
            p.terminate()
        _procs.pop(mode, None)
    return {"ok": True}


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
    try:
        with urllib.request.urlopen(CONTROL + "/servers", timeout=4) as r:
            return {"ok": True, **json.loads(r.read())}
    except Exception as ex:
        return {"ok": False, "error": f"{type(ex).__name__}",
                "hint": "start the proxy backend, then open Multiplayer "
                        "in-game once so the list passes through"}


def join(server_id, shape="bare"):
    """Push the client straight into a server, skipping the menus.

    The client exposes a `go_to_server` command whose url is `join:<server_id>`
    (see backend/join_push.py). Since we are the backend, we can send it.
    ⚠ This is NOT the `-direct` flag - that one is Simgrid integration and
    always fails with "requirement not met".
    """
    import urllib.request
    body = json.dumps({"id": server_id, "shape": shape}).encode()
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


def launch_game():
    """Start the game client. With the backend redirect in place, our servers
    appear in its normal Multiplayer browser."""
    exe = config.CFG.get("game_exe")
    if exe and os.path.exists(exe):
        subprocess.Popen([exe], cwd=os.path.dirname(exe))
        return {"ok": True, "via": "exe"}
    appid = config.CFG.get("steam_appid")
    if appid:
        os.startfile(f"steam://rungameid/{appid}")   # noqa: S606
        return {"ok": True, "via": "steam"}
    return {"ok": False, "error": "set game_exe in config.json"}
