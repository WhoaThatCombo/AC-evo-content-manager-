"""LAN address helpers for the local lobby.

The dedicated server binds 0.0.0.0. What we *advertise* has to be the address
the joining client can actually reach: loopback for a client on this machine,
the LAN IPv4 for everyone else. Guessing 127.0.0.1 for a second PC is why LAN
join looked like 'the backend works but the server is unreachable'.
"""
import socket


def lan_ipv4():
    """Best-effort primary IPv4 on a real interface, not 127.0.0.1.

    UDP connect to a public address does not send a packet; it just asks the
    routing table which source IP would be used. Falls back to hostname
    resolution, then empty string if the machine is offline.
    """
    try:
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        try:
            s.connect(("1.1.1.1", 80))
            ip = s.getsockname()[0]
        finally:
            s.close()
        if ip and not ip.startswith("127."):
            return ip
    except OSError:
        pass
    try:
        for info in socket.getaddrinfo(socket.gethostname(), None,
                                       socket.AF_INET, socket.SOCK_DGRAM):
            ip = info[4][0]
            if ip and not ip.startswith("127."):
                return ip
    except OSError:
        pass
    return ""


def is_loopback(host):
    if not host:
        return False
    h = str(host).strip("[]").split("%")[0]
    return h in ("127.0.0.1", "::1", "localhost") or h.startswith("127.")


def peer_host(ws_or_addr):
    """Remote host from a websocket, a (host, port) tuple, or a string."""
    if ws_or_addr is None:
        return ""
    if isinstance(ws_or_addr, (tuple, list)) and ws_or_addr:
        return str(ws_or_addr[0] or "")
    if isinstance(ws_or_addr, str):
        return ws_or_addr
    addr = getattr(ws_or_addr, "remote_address", None)
    if isinstance(addr, (tuple, list)) and addr:
        return str(addr[0] or "")
    return ""


def advertise_ip(peer=None, lan=None, loopback="127.0.0.1"):
    """IP to put on a MultiplayerServerListEntry for this peer."""
    lan = lan if lan is not None else lan_ipv4()
    host = peer_host(peer)
    if not host or is_loopback(host):
        return loopback
    return lan or loopback


def our_ips(lan=None):
    """Every address we should treat as 'this machine's server'."""
    out = {"127.0.0.1", "localhost", "::1"}
    lan = lan if lan is not None else lan_ipv4()
    if lan:
        out.add(lan)
    return out


def backend_ws_url(port, host="127.0.0.1"):
    """The -backend= value Kunos's own help text describes."""
    return f"wss://{host}:{int(port)}/communicationNode/dev"


# ---------------------------------------------------------------- public IP --
# ⚠ This is the ONLY outbound request in this module, and it exists because a
# LAN address is useless to the person you are sharing with. Someone on another
# network needs the address your router answers on, and nothing on this machine
# knows it - the PC only ever sees its private 192.168.x.x. So we ask an
# outside service what address our traffic appears to come from.
#
# Nothing is uploaded: the request carries no data beyond itself, and the reply
# is a bare IP. It runs only when the share panel is opened, never on startup,
# and a failure degrades to "we could not work it out" rather than an error.
_PUBLIC = {"ip": "", "at": 0.0}
_PUBLIC_TTL = 15 * 60

# Two providers, so one being down is not the end of it. Both return a bare
# address in the body.
_PUBLIC_URLS = ("https://api.ipify.org", "https://ifconfig.me/ip")


def public_ipv4(refresh=False, timeout=4.0):
    """The address the outside world sees, or "" if it cannot be determined."""
    import re
    import time
    import urllib.request

    now = time.monotonic()
    if not refresh and _PUBLIC["ip"] and (now - _PUBLIC["at"]) < _PUBLIC_TTL:
        return _PUBLIC["ip"]
    for url in _PUBLIC_URLS:
        try:
            req = urllib.request.Request(url, headers={"User-Agent": "ACECM"})
            with urllib.request.urlopen(req, timeout=timeout) as r:
                txt = r.read(64).decode("ascii", "replace").strip()
        except Exception:
            continue
        # Only accept something that really is a dotted IPv4 - a captive
        # portal or an error page would otherwise become "your address".
        if re.fullmatch(r"(?:\d{1,3}\.){3}\d{1,3}", txt) and \
                all(0 <= int(p) <= 255 for p in txt.split(".")):
            _PUBLIC.update(ip=txt, at=now)
            return txt
    return ""
