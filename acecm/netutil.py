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
