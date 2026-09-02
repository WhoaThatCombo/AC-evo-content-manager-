"""Wheel, pad and keyboard state on Linux, read from evdev.

The Windows module asks three APIs (joyGetPosEx, XInput, GetAsyncKeyState).
Linux has one: `/dev/input/event*`. That simplifies the device side and
complicates two other things, both of which matter for correctness rather
than convenience.

  * **Permissions.** Reading an input device needs membership of the `input`
    group (or a udev rule). Without it every device is simply invisible —
    there is no error, so a silent empty list would look like "no wheel
    connected". `permission_problem()` exists so the UI can say which it is.

  * **Focus.** `GetAsyncKeyState` sees a key wherever it was pressed, so the
    Windows build checks the foreground window before honouring a keyboard
    combo — ctrl+y is redo in most editors, and firing on that would teleport
    a car in a session running behind it. Wayland deliberately has no API to
    ask which window is focused. We therefore treat "cannot determine focus"
    as NOT focused, which disables the keyboard hotkey rather than making it
    dangerous. Bound wheel and pad buttons are unaffected: nothing else on
    the desktop is listening to them.
"""
import os
import time

from . import logs

_WARNED = {}


def _warn_once(key, msg, *args):
    if key not in _WARNED:
        _WARNED[key] = True
        logs.LOG.warning(msg, *args)


def _evdev():
    try:
        import evdev
        return evdev
    except ImportError:
        _warn_once("evdev", "python-evdev is not installed, so wheel/pad "
                            "buttons and keyboard hotkeys are unavailable "
                            "(pip install evdev)")
        return None


def _all_devices():
    ev = _evdev()
    if ev is None:
        return []
    out = []
    for path in ev.list_devices():
        try:
            out.append(ev.InputDevice(path))
        except OSError:
            continue                 # unreadable: covered by permission_problem
    return out


def permission_problem():
    """True when input devices exist but none of them can be opened.

    ⚠ Distinguishes "no wheel plugged in" from "not in the `input` group".
    Both look identical from an empty device list, and only one of them is
    fixed by plugging something in.
    """
    ev = _evdev()
    if ev is None:
        return False
    try:
        nodes = [n for n in os.listdir("/dev/input") if n.startswith("event")]
    except OSError:
        return False
    if not nodes:
        return False
    for n in nodes:
        if os.access(os.path.join("/dev/input", n), os.R_OK):
            return False
    return True


def _buttons(dev):
    """Button key-codes this device reports, in a stable order.

    ⚠ Sorted by key code, not by capability order. The bit a button occupies
    is stored in config when the user binds it, so the order has to be the
    same on the next launch — capability order is whatever the kernel
    returned and is not guaranteed to be.
    """
    ev = _evdev()
    caps = dev.capabilities().get(ev.ecodes.EV_KEY) or []
    return sorted(c for c in caps
                  if ev.ecodes.BTN_MISC <= c <= ev.ecodes.BTN_GEAR_UP
                  or c >= ev.ecodes.BTN_TRIGGER_HAPPY)


def _is_controller(dev):
    """A wheel, pad or stick.

    ⚠ Buttons + an absolute axis is not enough on its own. RGB gaming
    keyboards expose BTN_* codes and an ABS axis through their consumer
    controls, so a keyboard was showing up in the wheel picker - and binding
    the pit control to "button 3" of your keyboard is not something anyone
    wants to discover mid-session. Require a real stick/wheel axis, and never
    accept something that is also a full keyboard.
    """
    ev = _evdev()
    caps = dev.capabilities()
    axes = set(caps.get(ev.ecodes.EV_ABS) or [])
    if hasattr(axes, "__iter__"):
        axes = {a[0] if isinstance(a, tuple) else a for a in axes}
    stick = {ev.ecodes.ABS_X, ev.ecodes.ABS_Y, ev.ecodes.ABS_RX,
             ev.ecodes.ABS_RY, ev.ecodes.ABS_RZ, ev.ecodes.ABS_THROTTLE,
             ev.ecodes.ABS_WHEEL, ev.ecodes.ABS_GAS, ev.ecodes.ABS_BRAKE}
    return bool(_buttons(dev)) and bool(axes & stick) and not _is_keyboard(dev)


def _is_keyboard(dev):
    ev = _evdev()
    caps = dev.capabilities().get(ev.ecodes.EV_KEY) or []
    return ev.ecodes.KEY_A in caps and ev.ecodes.KEY_Z in caps


def _mask(dev):
    """Current button bitmask for one device."""
    try:
        held = set(dev.active_keys())
    except OSError:
        return None
    mask = 0
    for i, code in enumerate(_buttons(dev)):
        if code in held and i < 64:
            mask |= 1 << i
    return mask


def _controllers():
    return [d for d in _all_devices() if _is_controller(d)]


def read(kind, dev_id):
    """Button bitmask for controller `dev_id`, or None if it is not there.

    `kind` ("joy"/"pad") is kept for signature compatibility with the Windows
    module; evdev makes no such distinction, so both resolve the same way.
    """
    devs = _controllers()
    try:
        dev = devs[int(dev_id)]
    except (IndexError, ValueError, TypeError):
        return None
    try:
        return _mask(dev)
    finally:
        _close(devs)


def _close(devs):
    for d in devs:
        try:
            d.close()
        except OSError:
            pass


def devices():
    """Everything answering right now, and what it is currently holding."""
    devs = _controllers()
    out = []
    try:
        for i, dev in enumerate(devs):
            mask = _mask(dev)
            if mask is None:
                continue
            names = _buttons(dev)
            # ⚠ Say how many buttons it has. Some keyboards publish a
            # minimal gamepad interface (BTN_SELECT/BTN_START and a full set
            # of axes), so the list can legitimately contain a device nobody
            # would want to bind. Excluding it on a button-count threshold
            # would risk hiding somebody's real, minimal wheel; naming the
            # count lets the user tell them apart instead.
            #
            # ⚠ `id` is the INDEX and it is what gets saved as pit_btn_id, so
            # this list must never be reordered - not even to put the most
            # likely device first.
            label = (dev.name or f"Controller {i}").strip()
            out.append({
                "kind": "joy", "id": i, "buttons": mask,
                "label": f"{label} ({len(names)} buttons)",
                "pressed": [n + 1 for n in range(len(names))
                            if mask & (1 << n)],
            })
    finally:
        _close(devs)
    return out


def capture(seconds=6.0):
    """Wait for a button and report which one, so binding is 'press it'.

    Anything already held when this starts is ignored - otherwise binding
    from a wheel whose paddle is resting against a switch captures that
    instead of the button the user then presses.
    """
    devs = _controllers()
    if not devs:
        if permission_problem():
            return {"ok": False,
                    "error": "no input device could be opened. Add yourself "
                             "to the 'input' group and log out and in again: "
                             "sudo usermod -aG input $USER"}
        return {"ok": False, "error": "no wheel or controller found"}
    try:
        base = {i: (_mask(d) or 0) for i, d in enumerate(devs)}
        deadline = time.time() + float(seconds)
        while time.time() < deadline:
            for i, dev in enumerate(devs):
                mask = _mask(dev)
                if mask is None:
                    continue
                fresh = mask & ~base[i]      # ignore what was already held
                if fresh:
                    bit = (fresh & -fresh).bit_length() - 1
                    return {"ok": True, "kind": "joy", "id": i,
                            "mask": 1 << bit, "button": bit + 1,
                            "label": f"{(dev.name or '').strip()} "
                                     f"button {bit + 1}".strip()}
                base[i] &= mask              # a released button can be bound
            time.sleep(0.03)
    finally:
        _close(devs)
    return {"ok": False, "error": "nothing pressed"}


# ------------------------------------------------------------- keyboard --
def _key_codes(ev):
    table = {"ctrl": (ev.ecodes.KEY_LEFTCTRL, ev.ecodes.KEY_RIGHTCTRL),
             "shift": (ev.ecodes.KEY_LEFTSHIFT, ev.ecodes.KEY_RIGHTSHIFT),
             "alt": (ev.ecodes.KEY_LEFTALT, ev.ecodes.KEY_RIGHTALT)}
    for c in "abcdefghijklmnopqrstuvwxyz0123456789":
        code = getattr(ev.ecodes, f"KEY_{c.upper()}", None)
        if code is not None:
            table[c] = (code,)
    for n in range(1, 13):
        code = getattr(ev.ecodes, f"KEY_F{n}", None)
        if code is not None:
            table[f"f{n}"] = (code,)
    return table


def combo_held(text):
    """Is this key combination held right now?

    ⚠ Only ever answers True while the game has focus — see the module note.
    Nothing is grabbed or reserved: this reads current key state, so the game
    and everything else still receive the keys normally.
    """
    ev = _evdev()
    if ev is None:
        return False
    parts = [p.strip().lower() for p in str(text or "").split("+") if p.strip()]
    if not parts:
        return False
    table = _key_codes(ev)
    if any(p not in table for p in parts):
        return False
    if not game_focused():
        return False
    devs = [d for d in _all_devices() if _is_keyboard(d)]
    try:
        held = set()
        for d in devs:
            try:
                held.update(d.active_keys())
            except OSError:
                continue
        return all(any(c in held for c in table[p]) for p in parts)
    finally:
        _close(devs)


def game_focused():
    """Is the game the window in front?

    ⚠ Returns False when focus cannot be determined, and that is the whole
    point. Wayland exposes no way to ask which window is focused, so guessing
    "yes" would make a keyboard hotkey fire from inside a text editor. False
    disables the keyboard binding and leaves wheel buttons working.
    """
    import shutil
    import subprocess
    if not shutil.which("xdotool"):
        _warn_once("focus", "cannot tell which window has focus (xdotool is "
                            "not installed), so the keyboard pit hotkey is "
                            "disabled. A wheel or pad button still works.")
        return False
    try:
        r = subprocess.run(["xdotool", "getactivewindow", "getwindowname"],
                           capture_output=True, text=True, timeout=5)
        if r.returncode != 0:
            return False
        return "assetto corsa evo" in (r.stdout or "").lower()
    except (OSError, subprocess.SubprocessError):
        return False
