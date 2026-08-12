"""Push the client straight into a server - "go to server".

Found in the client: alongside the ordinary server-list commands there is a
`go_to_server` pair,

    MultiplayerServerListCommands.request_go_to_server   = 9
    MultiplayerServerListCommands.response_go_to_server  = 1008
    MultiplayerResponseGoToServer { response = 1; string url = 2; }

and the client parses that url expecting a `join:` prefix:

    "Invalid go to server server string '{}': expected prefix 'join:'"

We are the backend, so we can send that response and the client should walk
itself into the server - no menus, no browser.

⚠ Do NOT confuse this with `-direct`. That flag maps to
`request_direct_server -> MultiplayerServerListRequestConnectToSimgrid`, i.e.
Simgrid integration, which is why it aborts with "Connect To direct server
requirement not met". It is not a general direct-connect and never was.

Two wire shapes are tried, because the client accepts bare command messages as
the Any payload (that is how the existing server-list responses work) but the
schema also defines a combined wrapper:

    1. Any = MultiplayerResponseGoToServer
    2. Any = MultiplayerServerListCommands { response_go_to_server = ... }
"""
import sys

import acevo_proto as ap

TYPE_PREFIX = "type.googleapis.com/"


def _wrap(msg, name):
    env = ap.new("BackendMessage")
    env.msg.type_url = TYPE_PREFIX + name
    env.msg.value = msg.SerializeToString()
    env.unwrap_message = False
    return env.SerializeToString()


def build(server_id, shape="bare"):
    """Serialised BackendMessage that tells the client to join `server_id`."""
    url = server_id if str(server_id).startswith("join:") else f"join:{server_id}"
    inner = ap.new("MultiplayerResponseGoToServer")
    inner.url = url
    try:
        inner.response = 0            # ..._OK
    except Exception:
        pass                          # enum name/type varies; url is the payload

    if shape == "bare":
        return _wrap(inner, "MultiplayerResponseGoToServer")

    cmds = ap.new("MultiplayerServerListCommands")
    cmds.response_go_to_server.CopyFrom(inner)
    return _wrap(cmds, "MultiplayerServerListCommands")


async def push(ws, server_id, shape="bare"):
    """Send it to one connected client."""
    raw = build(server_id, shape)
    print(f"  PUSH go_to_server [{shape}] {len(raw)}B -> join:{server_id}",
          flush=True)
    await ws.send(raw)
    return {"sent": True, "server_id": server_id, "shape": shape,
            "bytes": len(raw)}


if __name__ == "__main__":
    sid = sys.argv[1] if len(sys.argv) > 1 else "local-0000-0000-0000-000000000001"
    for shape in ("bare", "wrapped"):
        try:
            raw = build(sid, shape)
            print(f"{shape:8} {len(raw):4} bytes  {raw[:64].hex()}")
        except Exception as ex:
            print(f"{shape:8} FAILED: {type(ex).__name__}: {ex}")
