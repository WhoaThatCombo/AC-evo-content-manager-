"""Talk to the game's Gameface inspector (Chrome DevTools on :9444).

The UI is cohtml. Stock EVO leaves the inspector off (DebuggerPort
0xFFFFFFFF, enable flag 0). backend.launch_game enables it in the exe
before Steam starts the game. Steam still drops every launch flag
except Launch Options, so Drive cannot start a session from argv — it
has to press Start the same way the menu does:

    ksUI.goTo('singleplayer.html', 'singleplayer/main')
    GAMEMODESELECTION.start()
"""
import base64
import json
import os
import socket
import struct
import time
import urllib.error
import urllib.request

HOST = "127.0.0.1"
PORT = int(os.environ.get("INSPECTOR_PORT", "9444"))


def listening():
    try:
        s = socket.create_connection((HOST, PORT), timeout=0.4)
        s.close()
        return True
    except OSError:
        return False


def list_views():
    with urllib.request.urlopen(f"http://{HOST}:{PORT}/json/list", timeout=8) as r:
        return json.load(r)


def _recv_exact(sock, n):
    buf = b""
    while len(buf) < n:
        c = sock.recv(n - len(buf))
        if not c:
            raise ConnectionError("socket closed")
        buf += c
    return buf


def _send_raw(sock, opcode, data=b""):
    hdr = bytearray([0x80 | opcode])
    n = len(data)
    if n < 126:
        hdr.append(0x80 | n)
    elif n < 65536:
        hdr.append(0x80 | 126)
        hdr += struct.pack(">H", n)
    else:
        hdr.append(0x80 | 127)
        hdr += struct.pack(">Q", n)
    mask = os.urandom(4)
    hdr += mask
    sock.sendall(bytes(hdr) + bytes(b ^ mask[i % 4] for i, b in enumerate(data)))


def _recv_raw(sock):
    b1, b2 = _recv_exact(sock, 2)
    op = b1 & 0x0F
    ln = b2 & 0x7F
    if ln == 126:
        ln = struct.unpack(">H", _recv_exact(sock, 2))[0]
    elif ln == 127:
        ln = struct.unpack(">Q", _recv_exact(sock, 8))[0]
    if b2 & 0x80:
        mask = _recv_exact(sock, 4)
        data = bytearray(_recv_exact(sock, ln))
        for i in range(ln):
            data[i] ^= mask[i % 4]
        return op, bytes(data)
    return op, _recv_exact(sock, ln)


def _recv_frame(sock):
    while True:
        op, payload = _recv_raw(sock)
        if op == 0x1:
            return payload
        if op == 0x8:
            raise ConnectionError("server closed the websocket")
        if op == 0x9:
            _send_raw(sock, 0xA, payload)


def _send_frame(sock, payload):
    data = payload.encode()
    hdr = bytearray([0x81])
    n = len(data)
    if n < 126:
        hdr.append(0x80 | n)
    elif n < 65536:
        hdr.append(0x80 | 126)
        hdr += struct.pack(">H", n)
    else:
        hdr.append(0x80 | 127)
        hdr += struct.pack(">Q", n)
    mask = os.urandom(4)
    hdr += mask
    sock.sendall(bytes(hdr) + bytes(b ^ mask[i % 4] for i, b in enumerate(data)))


def evaluate(expr, page="0", timeout=15, attempts=4, user_gesture=False):
    last = None
    for i in range(attempts):
        try:
            r = _evaluate_once(expr, page, timeout, user_gesture)
            if r is not None:
                return r
        except (ConnectionError, OSError, TimeoutError) as ex:
            last = ex
            if not listening():
                break
            time.sleep(0.3)
        except Exception as ex:
            last = ex
    raise last or RuntimeError("no reply from the inspector")


def _evaluate_once(expr, page="0", timeout=15, user_gesture=False):
    path = f"/devtools/page/{page}"
    s = socket.create_connection((HOST, PORT), timeout=timeout)
    key = base64.b64encode(os.urandom(16)).decode()
    s.sendall((f"GET {path} HTTP/1.1\r\nHost: {HOST}:{PORT}\r\n"
               "Upgrade: websocket\r\nConnection: Upgrade\r\n"
               f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
               ).encode())
    resp = b""
    while b"\r\n\r\n" not in resp:
        resp += s.recv(4096)
    if b"101" not in resp.split(b"\r\n")[0]:
        raise ConnectionError("upgrade failed: "
                              + resp.split(b"\r\n")[0].decode())
    _send_frame(s, json.dumps({"id": 1, "method": "Runtime.enable"}))
    ctx = None
    s.settimeout(4)
    try:
        while ctx is None:
            msg = json.loads(_recv_frame(s))
            if msg.get("method") == "Runtime.executionContextCreated":
                ctx = msg["params"]["context"]["id"]
    except Exception:
        pass
    if ctx is None:
        s.close()
        return None
    params = {"expression": expr, "returnByValue": True,
              "awaitPromise": True, "userGesture": bool(user_gesture),
              "contextId": ctx}
    s.settimeout(timeout)
    _send_frame(s, json.dumps({"id": 2, "method": "Runtime.evaluate",
                               "params": params}))
    while True:
        msg = json.loads(_recv_frame(s))
        if msg.get("id") == 2:
            s.close()
            return msg


def js_value(msg):
    if not msg:
        return None
    res = msg.get("result") or msg
    if res.get("exceptionDetails"):
        ex = res["exceptionDetails"]
        text = (ex.get("text") or "") + " " + str(
            (ex.get("exception") or {}).get("description") or "")
        return {"ok": False, "error": text.strip()}
    r = (res.get("result") or {})
    return {"ok": True, "value": r.get("value", r.get("description"))}


def menu_page():
    """The inspector page that is actually the menu, not HUD."""
    try:
        views = list_views()
    except Exception:
        return "0"
    for v in views:
        url = (v.get("url") or "").lower()
        if any(x in url for x in ("menu.html", "singleplayer.html", "ingame.html")):
            return str(v.get("id") or "0")
    return "0"


def ready():
    """Cheap. A 6s×2 evaluate on every poll is what delayed Start."""
    if not listening():
        return False
    try:
        r = js_value(evaluate(
            "!!(window.ksUI && window.GAMEMODESELECTION)",
            page=menu_page(), timeout=2, attempts=1))
        return bool(r and r.get("ok") and r.get("value"))
    except Exception:
        return False


_BOOT = """
(function(){
  var href = (location.href || '').toLowerCase();
  var path = '';
  try { path = ((ksUI.menuState.get() || {}).path || '').toLowerCase(); } catch (e) {}
  var car = !!(window.CurrentCar && CurrentCar.model && CurrentCar.model.name);
  var dest = '';
  try { dest = localStorage.getItem('loadingDestination') || ''; } catch (e) {}
  var cls = '';
  try { cls = String(document.body.className || ''); } catch (e) {}
  var paint = /(?:^|\\s)paintshop(?:\\s|$)/.test(cls) ? 'paint' : 'nopaint';
  var locked = cls.indexOf('ui-locked') >= 0 ? 'locked' : 'open';
  var page = 'other';
  if (href.indexOf('ingame.html') >= 0) page = 'ingame';
  else if (href.indexOf('singleplayer') >= 0) page = 'sp';
  else if (href.indexOf('multiplayer') >= 0) page = 'mp';
  else if (href.indexOf('menu.html') >= 0) page = car ? 'home' : 'menu-nocar';
  else if (href.indexOf('intro') >= 0) page = 'intro';
  return [page, path, car ? 'car' : 'nocar', dest, paint, locked].join('|');
})()
"""


def boot_parts(hint=None):
    s = hint if hint is not None else boot_state()
    return str(s or "").lower().split("|")


def boot_page(hint=None):
    p = boot_parts(hint)
    return p[0] if p else ""


def paintshop_up(hint=None):
    p = boot_parts(hint)
    return len(p) > 4 and p[4] == "paint"


def boot_state():
    """Live boot gate. 'home' = MainPage + CurrentCar (last thing before
    Start). The on-disk log is not flushed until quit, so do not use it."""
    if not listening():
        return ""
    try:
        r = js_value(evaluate(_BOOT, page=menu_page(), timeout=2, attempts=1))
        if r and r.get("ok"):
            return str(r.get("value") or "")
    except Exception:
        pass
    return ""


def home_ready(hint=None):
    """True on the home or single-player page with a current car.

    Paintshop still overlaying home is NOT ready: Start from there is
    what the game answers with `goto menu.html main/main override`.
    """
    s = hint if hint is not None else boot_state()
    if paintshop_up(s):
        return False
    page = boot_page(s)
    return page in ("home", "sp")


def session_loading(hint=None):
    s = (hint if hint is not None else boot_state()).lower()
    return "ingame" in s or "|session" in s


_GOTO = """
(function(){
  if (!window.ksUI) return 'no-ksUI';
  if ((location.href||'').indexOf('singleplayer') >= 0) return 'already-sp';
  ksUI.goTo('singleplayer.html', 'singleplayer/main');
  return 'goto-sp';
})()
"""

# ---- speedometer in mph -------------------------------------------------
# EVO has no units setting: "mph" appears nowhere in the client binary and
# nowhere in its UI code, and the speedo is hard-wired as
#
# <div class="value" data-bind-value="{{ModelCurrentCar.display_speed_kmh}}">
# <div class="unit">km/h</div>
#
# ⚠ DISPLAY ONLY. display_speed_kmh is also read by the pit limiter
# (`slow-down` above 60, `moving` above 20). Converting the underlying value
# would quietly move the pit-limiter warnings to 37 and 12 mph, so the number
# the engine binds is left exactly as it is and only what is drawn changes.
#
# ⚠ The engine owns the bound div and rewrites it every frame. Rather than
# fight it - which means either a write loop or reading back our own output
# and converting twice - the bound div is hidden and a clone we own is shown
# next to it. The engine keeps writing km/h where nobody can see it, and we
# read that and write mph. Same element, same classes, so it inherits the
# same styling and looks unchanged.
_MPH = """
(function(){
  var host = document.querySelector('.speed .value[data-bind-value]');
  if (!host) return 'no-speedo';
  var box = host.parentNode;
  if (box.querySelector('.acecm-mph')) return 'already';
  var mirror = host.cloneNode(false);
  mirror.className = host.className + ' acecm-mph';
  mirror.removeAttribute('data-bind-value');
  host.style.display = 'none';
  box.insertBefore(mirror, host.nextSibling);
  var unit = box.querySelector('.unit');
  if (unit) unit.textContent = 'mph';
  var paint = function(){
    var n = parseFloat((host.textContent || '').trim());
    mirror.textContent = isFinite(n) ? String(Math.round(n * 0.621371)) : '';
  };
  paint();
  // the engine writes into host, never into mirror, so this cannot see its
  // own output and needs no re-entry guard
  new MutationObserver(paint).observe(
      host, {childList: true, characterData: true, subtree: true});
  return 'mph-on';
})()
"""

_MPH_OFF = """
(function(){
  var box = document.querySelector('.speed');
  if (!box) return 'no-speedo';
  var m = box.querySelector('.acecm-mph');
  if (m) m.remove();
  var host = box.querySelector('.value[data-bind-value]');
  if (host) host.style.display = '';
  var unit = box.querySelector('.unit');
  if (unit) unit.textContent = 'km/h';
  return 'mph-off';
})()
"""


def hud_page():
    """The inspector page that is the HUD, specifically.

    ⚠ NOT menu_page(). That returns the first view matching menu.html,
    singleplayer.html OR ingame.html, so with the menu still alive it hands
    back the menu - and the HUD script would silently find no speedometer.

    ⚠ TWO documents, not one. The view is ingame.html while you are in the
    pit menu and becomes hud.html once you are actually driving - same view
    id, different page. Matching only ingame.html found the HUD in the pits
    and lost it the moment it mattered.

    ⚠ Also not the car's dashboard. A car's own displays are separate views
    (content/cars/<car>/displays/display.html) with their own speed readouts,
    and those are part of the car's modelled interior - not ours to rewrite.
    """
    try:
        views = list_views()
    except Exception:
        return None
    for v in views:
        url = (v.get("url") or "").lower()
        if "uiresources/hud.html" in url or "uiresources/ingame.html" in url:
            return str(v.get("id") or "0")
    return None


def set_mph(on=True):
    """Show the speedometer in mph. Returns what the page reported."""
    page = hud_page()
    if not page:
        return {"ok": False, "error": "the HUD is not open"}
    r = js_value(evaluate(_MPH if on else _MPH_OFF, page=page,
                          timeout=8, attempts=2))
    if not r or not r.get("ok"):
        return {"ok": False, "error": (r or {}).get("error") or "no answer"}
    return {"ok": True, "state": r.get("value")}


_START = """
(async function(){
  if ((location.href||'').indexOf('singleplayer') < 0) return 'not-on-sp';
  if (document.body && document.body.classList
      && document.body.classList.contains('paintshop')) return 'paintshop';
  if (!window.GAMEMODESELECTION) return 'no-GAMEMODESELECTION';
  try {
    if (window.ksUI && typeof ksUI.setOriginAsReturnPage === 'function')
      ksUI.setOriginAsReturnPage();
  } catch (e) {}
  try {
    await GAMEMODESELECTION.start();
    return 'started';
  } catch (e) {
    return 'fail:' + String((e && (e.response || e.message)) || e);
  }
})()
"""

_WHERE = """
(function(){
  var href = location.href || '';
  var path = '';
  try { path = (ksUI.menuState.get() || {}).path || ''; } catch (e) {}
  return path + '|' + href;
})()
"""


def enter_singleplayer():
    page = menu_page()
    return js_value(evaluate(_GOTO, page=page, timeout=10, user_gesture=False))


_GOTO_MP = """
(function(){
  if (!window.ksUI) return 'no-ksUI';
  if ((location.href||'').indexOf('multiplayer') >= 0) return 'already-mp';
  ksUI.goTo('multiplayer.html', 'main/serverlist');
  return 'goto-mp';
})()
"""


def enter_multiplayer(ip="", tcp=0, password=""):
    """Open the in-game public list once.

    Do not stuff ip:port into the path. That is not a real submenu, so
    the router rejects it and snaps back to main/main — which is the
    bounce both machines were seeing. Join picks the row after the
    list is up.
    """
    page = menu_page()
    return js_value(evaluate(_GOTO_MP, page=page, timeout=10,
                             user_gesture=False))


_JOIN_PUBLIC = """
(function(want){
  var key = String(want.ip) + ':' + String(want.tcp);
  if (window.__acecmJoin === key) return 'already-sent';
  var page = document.querySelector('ks-page-serverlist');
  if (!page) return 'no-page';
  if (page.classList && page.classList.contains('loading')) return 'loading';
  var list = page.ServerList || [];
  if (!list.length) return 'waiting-list';
  var hit = null;
  for (var i = 0; i < list.length; i++) {
    var s = list[i];
    if (s.server_ip == want.ip && String(s.server_tcp_port) == String(want.tcp)) {
      hit = s; break;
    }
    if (want.id && String(s.server_id) == String(want.id)) { hit = s; break; }
  }
  if (!hit) return 'not-in-list:' + list.length;
  if (!window.CurrentCar || !CurrentCar.model) return 'no-car';
  try { page.setSelectedServer(hit.server_id); } catch (e) {}
  if (page.txtPassword) page.txtPassword.value = want.password || '';
  page.selectedCar = CurrentCar.model;
  page.selectedCarPguid = CurrentCar.model.car_pguid;
  if (!page.selectedServerId) return 'select-fail:' + hit.server_id;
  window.__acecmJoin = key;
  try {
    page.connectToServer(false);
    return 'connect:' + hit.server_id + ':' + (hit.server_name || '');
  } catch (e) {
    window.__acecmJoin = '';
    return 'connect-fail:' + String(e && e.message || e);
  }
})
"""


def join_public(ip, tcp, password="", server_id=""):
    """Select the row in the in-game list and press Join once."""
    page = menu_page()
    want = json.dumps({
        "ip": ip or "",
        "tcp": int(tcp or 0),
        "password": password or "",
        "id": server_id or "",
    })
    expr = "(" + _JOIN_PUBLIC + ")(" + want + ")"
    return js_value(evaluate(expr, page=page, timeout=8, attempts=1))


_REFRESH_LIST = """
(function(){
  var el = document.querySelector('ks-page-serverlist');
  if (el && typeof el.getServerList === 'function') {
    el.getServerList();
    return 'refresh';
  }
  if (!window.ksUI) return 'no-ksUI';
  ksUI.goTo('multiplayer.html', 'main/serverlist');
  return 'goto-mp';
})()
"""


def refresh_server_list():
    # Short: a hung MP page used to block 40s and take the inspector down,
    # after which QuitGame could not run.
    page = menu_page()
    return js_value(evaluate(_REFRESH_LIST, page=page, timeout=4, attempts=1,
                             user_gesture=False))


_QUIT = """
(function(){
  if (!window.ksUI) return 'no-ksUI';
  try {
    // MP / loading pages ignore QuitGame. Home first, then quit.
    try { ksUI.goTo('menu.html', 'main/main'); } catch (e) {}
    ksUI.requestNoResponse('GameMode', 'QuitGame');
    return 'quit';
  } catch (e) {
    return 'fail:' + String(e && e.message || e);
  }
})()
"""


def quit_game():
    if not listening():
        return {"ok": False, "error": "inspector not listening"}
    page = menu_page()
    return js_value(evaluate(_QUIT, page=page, timeout=3, attempts=1,
                             user_gesture=True))


def close_window():
    """WM_CLOSE the EVO window. Works when the inspector is already dead."""
    import ctypes
    from ctypes import wintypes
    user32 = ctypes.WinDLL("user32", use_last_error=True)
    user32.FindWindowW.argtypes = [wintypes.LPCWSTR, wintypes.LPCWSTR]
    user32.FindWindowW.restype = wintypes.HWND
    user32.GetWindowTextW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
    user32.PostMessageW.argtypes = [wintypes.HWND, wintypes.UINT,
                                    wintypes.WPARAM, wintypes.LPARAM]
    WM_CLOSE = 0x0010
    hwnd = user32.FindWindowW(None, "Assetto Corsa EVO")
    if not hwnd:
        found = wintypes.HWND()

        @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
        def each(h, _):
            buf = ctypes.create_unicode_buffer(256)
            user32.GetWindowTextW(h, buf, 256)
            if "assetto corsa evo" in buf.value.lower():
                found.value = h
                return False
            return True
        user32.EnumWindows(each, 0)
        hwnd = found.value
    if not hwnd:
        return False
    user32.PostMessageW(hwnd, WM_CLOSE, 0, 0)
    return True


def window_open():
    """True if an EVO window is still on screen."""
    import ctypes
    from ctypes import wintypes
    user32 = ctypes.WinDLL("user32", use_last_error=True)
    user32.FindWindowW.argtypes = [wintypes.LPCWSTR, wintypes.LPCWSTR]
    user32.FindWindowW.restype = wintypes.HWND
    user32.GetWindowTextW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
    hwnd = user32.FindWindowW(None, "Assetto Corsa EVO")
    if hwnd:
        return True
    found = ctypes.c_int(0)

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def each(h, _):
        buf = ctypes.create_unicode_buffer(256)
        user32.GetWindowTextW(h, buf, 256)
        if "assetto corsa evo" in buf.value.lower():
            found.value = 1
            return False
        return True
    user32.EnumWindows(each, 0)
    return bool(found.value)


def press_start():
    # Short timeout: a dying inspector hangs on executionContextCreated.
    # Drive retries once :9444 is healthy again.
    page = menu_page()
    return js_value(evaluate(_START, page=page, timeout=8, attempts=2,
                             user_gesture=True))


def focus_game():
    """Put EVO in front. CDP / ACECM steal focus and the game pauses
    in an occluded state until you alt-tab back."""
    import ctypes
    from ctypes import wintypes
    user32 = ctypes.WinDLL("user32", use_last_error=True)
    user32.FindWindowW.argtypes = [wintypes.LPCWSTR, wintypes.LPCWSTR]
    user32.FindWindowW.restype = wintypes.HWND
    user32.GetWindowTextW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
    user32.ShowWindow.argtypes = [wintypes.HWND, ctypes.c_int]
    user32.SetForegroundWindow.argtypes = [wintypes.HWND]
    hwnd = user32.FindWindowW(None, "Assetto Corsa EVO")
    if not hwnd:
        found = wintypes.HWND()

        @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
        def each(h, _):
            buf = ctypes.create_unicode_buffer(256)
            user32.GetWindowTextW(h, buf, 256)
            if "assetto corsa evo" in buf.value.lower():
                found.value = h
                return False
            return True
        user32.EnumWindows(each, 0)
        hwnd = found.value
    if not hwnd:
        return False
    user32.ShowWindow(hwnd, 9)  # SW_RESTORE
    user32.SetForegroundWindow(hwnd)
    return True


_SELECT = """
(async function(want){
  if (!window.VEHICLES) return 'no-VEHICLES';
  var model = String(want.model || '');
  var preset = String(want.preset || '');
  function blob(m){
    if (!m) return '';
    var s = [m.name, m.display_name, m.car_guid].join(' ');
    var ps = (m.__uimetadata && m.__uimetadata.presets) || [];
    for (var i = 0; i < ps.length; i++) {
      s += ' ' + [ps[i].name, ps[i].car_guid, ps[i].display_name].join(' ');
    }
    return s.toLowerCase();
  }
  function match(m){
    var b = blob(m);
    if (!b) return false;
    if (model && b.indexOf(model.toLowerCase()) >= 0) return true;
    if (preset && b.indexOf(preset.toLowerCase()) >= 0) return true;
    return false;
  }
  var found = null;
  try {
    var list = await VEHICLES.getAllModels();
    found = (list || []).find(match);
    if (!found) {
      for (var i = 0; i < (list || []).length; i++) {
        var ps = (list[i].__uimetadata && list[i].__uimetadata.presets) || [];
        var p = ps.find(match);
        if (p) { found = p; break; }
      }
    }
    if (!found && list && list.length)
      return 'not-found:' + list.length + ':' + blob(list[0]).slice(0, 80);
  } catch (e) {
    found = null;
  }
  if (!found && model)
    found = { name: model, display_name: model };
  if (!found) return 'not-found';
  try {
    await VEHICLES.setCurrent(found);
  } catch (e) {
    return 'set-fail:' + String(e && e.message || e);
  }
  var now = (window.CurrentCar && CurrentCar.model &&
             (CurrentCar.model.name || CurrentCar.model.display_name)) || '';
  return 'set:' + (found.name || found.car_guid || model) + ' now:' + now;
})
"""


def current_car_name():
    try:
        r = js_value(evaluate(
            "(window.CurrentCar && CurrentCar.model && "
            "(CurrentCar.model.name||'')) || ''",
            page=menu_page(), timeout=2, attempts=1))
        if r and r.get("ok"):
            return str(r.get("value") or "")
    except Exception:
        pass
    return ""


def select_car(model, preset=""):
    """Ask the garage to make this the current car. Same as picking it in UI."""
    page = menu_page()
    want = json.dumps({"model": model or "", "preset": preset or ""})
    expr = "(" + _SELECT + ")(" + want + ")"
    return js_value(evaluate(expr, page=page, timeout=30, attempts=2))


_CONDITIONS = """
(async function(want){
  if (!window.GAMEMODESELECTION) return 'no-GAMEMODESELECTION';
  function req(cmd, payload){
    return new Promise(function(resolve){
      GAMEMODESELECTION.Client.request(cmd, payload || {}, function(r){
        resolve(r || {});
      });
    });
  }
  var wtype = 'GameModeSelectionWeatherType_' + String(want.weather || 'CLEAR').toUpperCase();
  var hour = want.hour | 0;
  var minute = want.minute | 0;
  var mode = String(want.mode || '');
  var data;
  try {
    data = await GAMEMODESELECTION.getGameModeParameters();
  } catch (e) {
    return 'params-fail:' + String(e && (e.message || e.response) || e);
  }
  var typeRes = 'same';
  if (mode && data && data.type !== mode) {
    var tr = await req('SetGameType', {
      mode: 'ClientCommandsMode_SINGLEPLAYER',
      type: mode
    });
    typeRes = (tr && tr.response) || 'set';
    try { data = await GAMEMODESELECTION.getGameModeParameters(); }
    catch (e) { return 'params2-fail:' + String(e && e.message || e); }
  }
  var list = (data && data.weather_data) || [];
  var wd = null;
  for (var i = 0; i < list.length; i++) {
    if (list[i].type === wtype) { wd = list[i]; break; }
  }
  var wres = wd ? 'no-client' : 'no-preset:' + list.length;
  if (wd && GAMEMODESELECTION.Client) {
    var wr = await req('SetWeather', { weather_data: wd });
    wres = (wr && wr.response) || 'ok';
  }
  var sessions = (GAMEMODESELECTION.Current && GAMEMODESELECTION.Current.sessions)
              || (data && data.sessions) || [];
  var s0 = sessions[0];
  for (var j = 0; j < sessions.length; j++) {
    if (!sessions[j].time_of_day) sessions[j].time_of_day = {};
    sessions[j].time_of_day.hour = hour;
    sessions[j].time_of_day.minute = minute;
  }
  if (s0 && want.opponents != null) {
    s0.num_opponents = want.opponents | 0;
    s0.min_strength = want.min_strength | 0;
    s0.max_strength = want.max_strength | 0;
    s0.aggressivness = want.aggressiveness || 'Safe';
    s0.grid_type = 'GameModeSelectionGridType_AUTO';
    s0.single_make = !!want.single_make;
    if (want.starting_position)
      s0.starting_position = want.starting_position | 0;
  }
  if (sessions.length >= 4 && want.practice_min != null) {
    sessions[0].duration = (want.practice_min | 0) * 60;
    sessions[0].duration_type = 'GameModeSelectionDuration_TIME';
    sessions[1].duration = (want.quali_min | 0) * 60;
    sessions[1].duration_type = 'GameModeSelectionDuration_TIME';
    sessions[2].duration = (want.warmup_min | 0) * 60;
    sessions[2].duration_type = 'GameModeSelectionDuration_TIME';
    sessions[3].duration = want.race_laps | 0;
    sessions[3].duration_type = 'GameModeSelectionDuration_LAPS';
  } else if (s0 && want.duration_min != null && !want.opponents) {
    s0.duration = (want.duration_min | 0) * 60;
    s0.duration_type = 'GameModeSelectionDuration_TIME';
  } else if (s0 && want.race_laps != null && want.opponents != null
             && sessions.length < 4) {
    s0.duration = want.race_laps | 0;
    s0.duration_type = 'GameModeSelectionDuration_LAPS';
  }
  try {
    await GAMEMODESELECTION.save(data && data.mode, sessions);
  } catch (e) {
    return 'save-fail:' + String(e && (e.message || e.response) || e)
         + ' weather:' + wres;
  }
  var nowW = (window.CurrentGameMode && CurrentGameMode.weather_type) || '';
  var nowT = '';
  var nowAi = '';
  try {
    var cur = GAMEMODESELECTION.Current.sessions[0];
    nowT = String(cur.time_of_day.hour) + ':' + String(cur.time_of_day.minute);
    nowAi = String(cur.num_opponents) + '@' + cur.min_strength + '-' + cur.max_strength;
  } catch (e) {}
  return 'mode:' + ((data && data.type) || mode) + ' setType:' + typeRes
       + ' weather:' + wtype + ' via:' + wres + ' now:' + nowW
       + ' tod:' + nowT + ' ai:' + nowAi;
})
"""


def apply_conditions(pick=None, weather="CLEAR", hour=13, minute=0):
    """Weather, time, mode type, AI count/skill — same as the SP pages."""
    pick = pick or {}
    mode = str(pick.get("game_mode") or "").strip().upper().replace(
        "GAMEMODETYPE_", "")
    if mode and not mode.startswith("GAMEMODETYPE_"):
        mode = "GameModeType_" + mode
    ai = mode.replace("GameModeType_", "") in ("INSTANT_RACE", "RACE_WEEKEND")
    want = {
        "weather": pick.get("weather") or weather or "CLEAR",
        "hour": int(pick.get("tod_hour") if pick.get("tod_hour") is not None
                    else hour),
        "minute": int(pick.get("tod_minute") if pick.get("tod_minute") is not None
                      else minute),
        "mode": mode,
        "aggressiveness": pick.get("aggressiveness") or "Safe",
        "single_make": bool(pick.get("single_make", True)),
    }
    if ai:
        want["opponents"] = int(pick.get("num_opponents") or 10)
        want["min_strength"] = int(pick.get("skill_min") or 80)
        want["max_strength"] = int(pick.get("skill_max") or 95)
        want["starting_position"] = int(pick.get("starting_position") or 0)
        want["race_laps"] = int(pick.get("race_laps") or 10)
        if "RACE_WEEKEND" in mode:
            want["practice_min"] = int(pick.get("practice_min") or 10)
            want["quali_min"] = int(pick.get("quali_min") or 15)
            want["warmup_min"] = int(pick.get("warmup_min") or 10)
    else:
        want["duration_min"] = int(pick.get("duration_min") or 90)
    page = menu_page()
    expr = "(" + _CONDITIONS + ")(" + json.dumps(want) + ")"
    return js_value(evaluate(expr, page=page, timeout=25, attempts=2))


def where():
    """'pitlane/main|coui://.../ingame.html' or similar."""
    try:
        page = menu_page()
        r = js_value(evaluate(_WHERE, page=page, timeout=8, attempts=2))
        if r and r.get("ok"):
            return str(r.get("value") or "")
    except Exception:
        pass
    return ""


def in_pits(hint=None):
    h = (hint if hint is not None else where()).lower()
    return "pitlane" in h
