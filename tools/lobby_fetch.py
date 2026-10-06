"""Fetch the public server list straight from the AC EVO lobby - no game.

    ACECM.exe --tool lobby_fetch --out <file.json> [--game-dir DIR]

The lobby only answers a client that proves it owns the game, with a Steam
ticket in the first frame. ACECM used to get that ticket only by launching
EVO and relaying the game's own session through the proxy, which is why
"Refresh list" took a minute. This mints the SAME kind of ticket the game
does - GetAuthTicketForWebApi with the game's identity string, through the
game's own steam_api64.dll under its app id - then speaks the lobby protocol
the proxy already relays: login text frame, RegisterRequest, then
MultiplayerServerListRequestServerList.

⚠ A SEPARATE, SHORT-LIVED PROCESS on purpose. While steam_api is initialised
in a process, Steam treats that process as AC EVO running. Inside ACECM that
would last until ACECM closed - and block launching the real game. Here it
lasts the ~2 s of one fetch, and SteamAPI_Shutdown + exit end it.

⚠ Do not run while the game itself is running (ACECM checks): the proxy
captures the list from the game then, and two "EVO" processes on one account
is a state better not to create.

Output: the same flattened shape the proxy's capture writes, so every
consumer of server_list.json reads it unchanged. Prints one JSON summary line.
"""
import argparse
import asyncio
import ctypes
import json
import os
import secrets
import ssl
import sys
import time

APP_ID = 3058630
# the string the game itself passes (it is in AssettoCorsaEVO.exe)
IDENTITY = b"evoBackendMini"
CB_WEBAPI_TICKET = 168          # GetTicketForWebApiResponse_t
TICKET_WAIT = 6.0


def _t():
    return time.perf_counter()


class Steam:
    """The handful of steam_api64 flat calls this needs, nothing more."""

    def __init__(self, game_dir):
        os.environ["SteamAppId"] = str(APP_ID)
        os.environ["SteamGameId"] = str(APP_ID)
        if hasattr(os, "add_dll_directory"):
            os.add_dll_directory(game_dir)
        self.dll = ctypes.WinDLL(os.path.join(game_dir, "steam_api64.dll"))
        d = self.dll
        d.SteamAPI_InitFlat.argtypes = [ctypes.c_char_p]
        d.SteamAPI_InitFlat.restype = ctypes.c_int
        d.SteamAPI_SteamUser_v023.restype = ctypes.c_void_p
        d.SteamAPI_GetHSteamPipe.restype = ctypes.c_int
        d.SteamAPI_ISteamUser_GetSteamID.argtypes = [ctypes.c_void_p]
        d.SteamAPI_ISteamUser_GetSteamID.restype = ctypes.c_uint64
        d.SteamAPI_ISteamUser_GetAuthTicketForWebApi.argtypes = [
            ctypes.c_void_p, ctypes.c_char_p]
        d.SteamAPI_ISteamUser_GetAuthTicketForWebApi.restype = ctypes.c_uint32
        d.SteamAPI_ISteamUser_CancelAuthTicket.argtypes = [
            ctypes.c_void_p, ctypes.c_uint32]
        d.SteamAPI_ManualDispatch_RunFrame.argtypes = [ctypes.c_int]
        d.SteamAPI_ManualDispatch_GetNextCallback.argtypes = [
            ctypes.c_int, ctypes.c_void_p]
        d.SteamAPI_ManualDispatch_GetNextCallback.restype = ctypes.c_bool
        d.SteamAPI_ManualDispatch_FreeLastCallback.argtypes = [ctypes.c_int]
        self.user = None
        self.handle = 0

    def ticket(self):
        d = self.dll
        err = ctypes.create_string_buffer(1024)
        r = d.SteamAPI_InitFlat(err)
        if r != 0:
            raise RuntimeError("Steam is not running or not logged in "
                               f"(SteamAPI_InitFlat {r}: {err.value.decode(errors='replace')})")
        d.SteamAPI_ManualDispatch_Init()
        pipe = d.SteamAPI_GetHSteamPipe()
        self.user = d.SteamAPI_SteamUser_v023()
        if not self.user:
            raise RuntimeError("Steam user interface unavailable")
        sid = d.SteamAPI_ISteamUser_GetSteamID(self.user)
        self.handle = d.SteamAPI_ISteamUser_GetAuthTicketForWebApi(
            self.user, IDENTITY)
        if not self.handle:
            raise RuntimeError("Steam refused to issue a ticket")
        # CallbackMsg_t: int32 user, int32 id, ptr param, int32 size  (24 B)
        msg = ctypes.create_string_buffer(24)
        until = time.time() + TICKET_WAIT
        while time.time() < until:
            d.SteamAPI_ManualDispatch_RunFrame(pipe)
            while d.SteamAPI_ManualDispatch_GetNextCallback(pipe, msg):
                cb = int.from_bytes(msg.raw[4:8], "little", signed=True)
                par = int.from_bytes(msg.raw[8:16], "little")
                if cb == CB_WEBAPI_TICKET and par:
                    # GetTicketForWebApiResponse_t: handle, EResult, size, bytes
                    head = ctypes.string_at(par, 12)
                    eres = int.from_bytes(head[4:8], "little")
                    size = int.from_bytes(head[8:12], "little")
                    raw = ctypes.string_at(par + 12, size)
                    d.SteamAPI_ManualDispatch_FreeLastCallback(pipe)
                    if eres != 1:
                        raise RuntimeError(f"Steam ticket failed (EResult {eres})")
                    return sid, raw.hex().upper()
                d.SteamAPI_ManualDispatch_FreeLastCallback(pipe)
            time.sleep(0.01)          # 10 ms: the ticket is usually ~100 ms
        raise RuntimeError("Steam did not deliver a ticket in time")

    def close(self):
        try:
            if self.user and self.handle:
                self.dll.SteamAPI_ISteamUser_CancelAuthTicket(self.user,
                                                              self.handle)
        finally:
            self.dll.SteamAPI_Shutdown()


async def fetch_list(upstream, get_ticket, timeout=20.0, times=None):
    """Connect and mint the ticket AT THE SAME TIME.

    ⚠ Measured: TLS connect to the lobby ~0.6 s, Steam ticket 0.7-1.3 s.
    Run back to back they cost both; overlapped, only the longer one.
    """
    import websockets
    import acevo_backend as b
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
    ctx.check_hostname = False
    ctx.verify_mode = ssl.CERT_NONE      # same stance as the proxy's upstream
    nonce = secrets.token_hex(8)
    t0 = _t()
    tk_task = asyncio.get_running_loop().run_in_executor(None, get_ticket)
    async with websockets.connect(
            upstream, ssl=ctx, max_size=None, ping_interval=None,
            compression=None, subprotocols=["wss", "ws"],
            user_agent_header="WebSocket++/0.8.2",
            open_timeout=timeout) as ws:
        if times is not None:
            times["connect_ms"] = round((_t() - t0) * 1000)
        sid, ticket = await tk_task
        if times is not None:
            times["ticket_ms"] = round((_t() - t0) * 1000)
        await ws.send(f"||42|4|{sid}|1|{nonce}|{ticket}")
        reg = b.ap.new("RegisterRequest")
        reg.identity_id = str(sid)
        reg.platform_type = 1                    # STEAM
        reg.server_version = b.SERVER_VERSION
        reg.procol_version = b.PROTOCOL_VERSION
        await ws.send(b.wrap(reg, "RegisterRequest"))
        deadline = time.time() + timeout
        asked = False
        while time.time() < deadline:
            raw = await asyncio.wait_for(ws.recv(), max(0.1, deadline - time.time()))
            if isinstance(raw, str):
                continue
            name, msg = b.unwrap(raw)
            if name == "RegisterResponse" and not asked:
                if msg is not None and hasattr(msg, "is_registered") \
                        and not msg.is_registered:
                    raise RuntimeError("the lobby refused the login")
                req = b.ap.new("MultiplayerServerListRequestServerList")
                req.request_no = 1
                await ws.send(b.wrap(req, "MultiplayerServerListRequestServerList"))
                asked = True
            elif name == "MultiplayerServerListResponseServerList" and msg is not None:
                return msg
        raise RuntimeError("the lobby did not send a server list in time")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    ap.add_argument("--game-dir", default=os.environ.get("ACECM_GAME_DIR", ""))
    a = ap.parse_args()
    t0 = _t()
    times = {}
    try:
        if not a.game_dir or not os.path.isfile(
                os.path.join(a.game_dir, "steam_api64.dll")):
            raise RuntimeError("AC EVO's folder (with steam_api64.dll) not found")
        steam = Steam(a.game_dir)
        try:
            import acevo_proxy as px
            t1 = _t()
            msg = asyncio.run(fetch_list(px.UPSTREAM, steam.ticket,
                                         times=times))
            times["lobby_ms"] = round((_t() - t1) * 1000)
        finally:
            steam.close()
        t2 = _t()
        px._capture_list(msg)             # same flattening the proxy uses
        servers = px.SERVER_LIST["servers"]
        blob = {"ok": True, "count": len(servers),
                "captured_at": int(time.time()), "servers": servers,
                "source": "lobby"}
        tmp = a.out + ".part"
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump(blob, f)
        os.replace(tmp, a.out)
        times["write_ms"] = round((_t() - t2) * 1000)
        print(json.dumps({"ok": True, "count": len(servers),
                          "ms": round((_t() - t0) * 1000), **times}))
        return 0
    except Exception as ex:                       # noqa: BLE001
        print(json.dumps({"ok": False, "error": str(ex),
                          "ms": round((_t() - t0) * 1000), **times}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
