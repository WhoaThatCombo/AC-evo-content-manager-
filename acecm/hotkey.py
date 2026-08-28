"""Fire an in-game action from a wheel, a button box, or a controller.

Why this exists: the game's "Go to Pitlane" lives in the pause menu and its
button is hidden while you are on track - so the one moment you want it is
the one moment you cannot reach it. The command underneath has no such gate
(see gameui.back_to_pit); it just needs something to call it.

⚠ TWO INPUT APIS, because one is not enough. Wheels and button boxes are
DirectInput devices and answer joyGetPosEx; an Xbox pad is XInput and does
NOT report its buttons there - measured on this machine, the pad enumerated
as joystick 0 and reported an all-zero button mask no matter what was
pressed, while XInput saw the same press immediately. Reading only one API
would silently work for half the hardware people own.

⚠ Read-only, always. This polls which buttons are held; it never synthesises
input and never acquires a device exclusively, so the game keeps full use of
the wheel.

⚠ joyGetPosEx reports only the first 32 buttons of the first 16 devices.
Some wheels expose the rest through DirectInput proper, so a button box with
more than 32 controls may have buttons that cannot be seen here.
"""
import ctypes
import threading
import time
from ctypes import wintypes

from . import config, logs

_JOY_RETURNBUTTONS = 0x00000080
_JOY_RETURNALL = 0x000000FF
_MAX_JOY = 16
_MAX_PADS = 4

# XInput names its buttons; DirectInput just numbers them.
XINPUT_BUTTONS = [
    (0x0001, "DPad Up"), (0x0002, "DPad Down"), (0x0004, "DPad Left"),
    (0x0008, "DPad Right"), (0x0010, "Start"), (0x0020, "Back"),
    (0x0040, "L Stick"), (0x0080, "R Stick"), (0x0100, "LB"),
    (0x0200, "RB"), (0x1000, "A"), (0x2000, "B"), (0x4000, "X"),
    (0x8000, "Y"),
]


class _JOYINFOEX(ctypes.Structure):
    _fields_ = [("dwSize", wintypes.DWORD), ("dwFlags", wintypes.DWORD),
                ("dwXpos", wintypes.DWORD), ("dwYpos", wintypes.DWORD),
                ("dwZpos", wintypes.DWORD), ("dwRpos", wintypes.DWORD),
                ("dwUpos", wintypes.DWORD), ("dwVpos", wintypes.DWORD),
                ("dwButtons", wintypes.DWORD), ("dwButtonNumber", wintypes.DWORD),
                ("dwPOV", wintypes.DWORD), ("dwReserved1", wintypes.DWORD),
                ("dwReserved2", wintypes.DWORD)]


class _XINPUT_GAMEPAD(ctypes.Structure):
    _fields_ = [("wButtons", wintypes.WORD), ("bLeftTrigger", ctypes.c_ubyte),
                ("bRightTrigger", ctypes.c_ubyte), ("sThumbLX", ctypes.c_short),
                ("sThumbLY", ctypes.c_short), ("sThumbRX", ctypes.c_short),
                ("sThumbRY", ctypes.c_short)]


class _XINPUT_STATE(ctypes.Structure):
    _fields_ = [("dwPacketNumber", wintypes.DWORD),
                ("Gamepad", _XINPUT_GAMEPAD)]


_DLLS = {}


def _winmm():
    if "mm" not in _DLLS:
        try:
            _DLLS["mm"] = ctypes.WinDLL("winmm")
        except Exception:
            _DLLS["mm"] = None
    return _DLLS["mm"]


def _xinput():
    if "xi" not in _DLLS:
        _DLLS["xi"] = None
        for name in ("XInput1_4.dll", "xinput1_3.dll", "XInput9_1_0.dll"):
            try:
                _DLLS["xi"] = ctypes.WinDLL(name)
                break
            except Exception:
                continue
    return _DLLS["xi"]


def read_joy(joy_id):
    """DirectInput button bitmask, or None if that device is not there."""
    mm = _winmm()
    if mm is None:
        return None
    info = _JOYINFOEX()
    info.dwSize = ctypes.sizeof(_JOYINFOEX)
    info.dwFlags = _JOY_RETURNBUTTONS
    if mm.joyGetPosEx(int(joy_id), ctypes.byref(info)) != 0:
        return None
    return int(info.dwButtons)


def read_pad(pad_id):
    """XInput button bitmask, or None if that pad is not connected."""
    xi = _xinput()
    if xi is None:
        return None
    st = _XINPUT_STATE()
    if xi.XInputGetState(int(pad_id), ctypes.byref(st)) != 0:
        return None
    return int(st.Gamepad.wButtons)


def read(kind, dev):
    return read_pad(dev) if kind == "pad" else read_joy(dev)


def devices():
    """Everything answering right now, and what it is currently holding.

    Both APIs, because a machine can easily have a wheel on one and a pad on
    the other - and the same physical pad may appear on both while only
    reporting its buttons on XInput.
    """
    out = []
    for jid in range(_MAX_JOY):
        mask = read_joy(jid)
        if mask is None:
            continue
        out.append({"kind": "joy", "id": jid, "buttons": mask,
                    "label": f"Joystick {jid}",
                    "pressed": [i + 1 for i in range(32) if mask & (1 << i)]})
    for pid in range(_MAX_PADS):
        mask = read_pad(pid)
        if mask is None:
            continue
        out.append({"kind": "pad", "id": pid, "buttons": mask,
                    "label": f"Controller {pid + 1}",
                    "pressed": [n for b, n in XINPUT_BUTTONS if mask & b]})
    return out


def capture(seconds=6.0):
    """Wait for a button and report which one, so binding is 'press it'.

    Anything already held when this starts is ignored - otherwise binding
    from a wheel whose paddle is resting against a switch captures that
    instead of the button the user then presses.
    """
    start = {}
    for d in devices():
        start[(d["kind"], d["id"])] = d["buttons"]
    deadline = time.time() + max(0.5, float(seconds))
    while time.time() < deadline:
        for d in devices():
            key = (d["kind"], d["id"])
            was = start.get(key, 0)
            fresh = d["buttons"] & ~was
            if fresh:
                bit = fresh & -fresh          # lowest newly-pressed bit
                if d["kind"] == "pad":
                    name = next((n for b, n in XINPUT_BUTTONS if b == bit), hex(bit))
                    return {"ok": True, "kind": "pad", "id": d["id"],
                            "mask": bit, "label": f"{d['label']} · {name}"}
                num = bit.bit_length()
                return {"ok": True, "kind": "joy", "id": d["id"],
                        "mask": bit, "label": f"{d['label']} · button {num}"}
            start.setdefault(key, d["buttons"])
        time.sleep(0.03)
    return {"ok": False, "error": "no button pressed"}


# ---- keyboard ------------------------------------------------------------
_VK = {"ctrl": 0x11, "shift": 0x10, "alt": 0x12}
for _c in "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789":
    _VK[_c.lower()] = ord(_c)
for _n in range(1, 13):
    _VK[f"f{_n}"] = 0x6F + _n


def parse_combo(text):
    """'ctrl+y' -> ([0x11], 0x59). Unknown names give (None, None)."""
    parts = [p.strip().lower() for p in str(text or "").split("+") if p.strip()]
    if not parts:
        return None, None
    mods, key = [], None
    for p in parts:
        if p in ("ctrl", "shift", "alt"):
            mods.append(_VK[p])
        elif p in _VK:
            key = _VK[p]
        else:
            return None, None
    return (mods, key) if key else (None, None)


def _down(vk):
    try:
        user32 = ctypes.WinDLL("user32", use_last_error=True)
        return bool(user32.GetAsyncKeyState(int(vk)) & 0x8000)
    except Exception:
        return False


def combo_held(text):
    """Is this key combination held right now?

    ⚠ GetAsyncKeyState, not RegisterHotKey. RegisterHotKey would take the
    combination system-wide and away from everything else, and needs a
    message loop; this only asks what is currently down, so nothing is
    reserved and the game is unaffected.
    """
    mods, key = parse_combo(text)
    if not key:
        return False
    return all(_down(m) for m in mods) and _down(key)


def game_focused():
    """Is the game the window in front?

    ⚠ The keyboard combination is only honoured when it is. GetAsyncKeyState
    is not focus-aware - it reports the key wherever it was pressed - and
    ctrl+y is 'redo' in most editors, so without this, hitting redo in a text
    editor would teleport the car of a session running behind it. A bound
    wheel/controller button needs no such check: nothing else on the desktop
    is listening to it.
    """
    try:
        user32 = ctypes.WinDLL("user32", use_last_error=True)
        hwnd = user32.GetForegroundWindow()
        if not hwnd:
            return False
        pid = wintypes.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
        buf = ctypes.create_unicode_buffer(256)
        user32.GetWindowTextW(hwnd, buf, 256)
        return "assetto corsa evo" in (buf.value or "").lower()
    except Exception:
        return False


_STATE = {"last": {}, "started": False}


def _fire():
    from . import gameui
    r = gameui.back_to_pit()
    logs.LOG.info("pit button: %s", "sent" if r.get("ok") else r.get("error"))
    return r


def watch():
    """Poll the bound button and fire on the press.

    ⚠ Edge, not level. At 20 Hz a button held for half a second reads as ten
    presses, and ten teleports for one press is worse than it not working -
    so only a not-held -> held transition counts.
    """
    if _STATE["started"]:
        return
    _STATE["started"] = True

    def run():
        while True:
            try:
                kind = config.CFG.get("pit_btn_kind")
                dev = config.CFG.get("pit_btn_id")
                mask = config.CFG.get("pit_btn_mask")
                if kind and mask and dev is not None and dev != "":
                    now_mask = read(kind, dev)
                    if now_mask is not None:
                        held = bool(now_mask & int(mask))
                        key = f"{kind}:{dev}:{mask}"
                        if held and not _STATE["last"].get(key):
                            _fire()
                        _STATE["last"][key] = held
                # ⚠ The keyboard is checked even with a button bound, not
                # instead of it. They are alternatives, not a choice: the
                # default combination has to keep working for someone who
                # later binds a wheel button, and for anyone with no
                # controller plugged in at all.
                combo = config.CFG.get("pit_key")
                if combo:
                    held = combo_held(combo) and game_focused()
                    if held and not _STATE["last"].get("kbd"):
                        _fire()
                    _STATE["last"]["kbd"] = held
            except Exception as ex:
                logs.LOG.debug("pit button: %s", ex)
            time.sleep(0.05)

    threading.Thread(target=run, daemon=True).start()
