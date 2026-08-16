"""Silent Windows process helpers. Never shell out to powershell.exe.

ACECM used to spawn powershell.exe for Get-Process / Get-CimInstance. Each
call flashed a console and Windows toasted it. Toolhelp / psapi stay in-process.
"""
import ctypes
import struct
from ctypes import wintypes

CREATE_NO_WINDOW = 0x08000000
TH32CS_SNAPPROCESS = 0x00000002
PROCESS_QUERY_LIMITED = 0x1000
PROCESS_VM = 0x0438  # QUERY_INFO | VM_READ | VM_WRITE | VM_OP

_k32 = ctypes.WinDLL("kernel32", use_last_error=True)
_psapi = ctypes.WinDLL("psapi", use_last_error=True)


class PROCESSENTRY32W(ctypes.Structure):
    _fields_ = [
        ("dwSize", wintypes.DWORD),
        ("cntUsage", wintypes.DWORD),
        ("th32ProcessID", wintypes.DWORD),
        ("th32DefaultHeapID", ctypes.c_void_p),
        ("th32ModuleID", wintypes.DWORD),
        ("cntThreads", wintypes.DWORD),
        ("th32ParentProcessID", wintypes.DWORD),
        ("pcPriClassBase", ctypes.c_long),
        ("dwFlags", wintypes.DWORD),
        ("szExeFile", wintypes.WCHAR * 260),
    ]


class PROCESS_MEMORY_COUNTERS(ctypes.Structure):
    _fields_ = [
        ("cb", wintypes.DWORD),
        ("PageFaultCount", wintypes.DWORD),
        ("PeakWorkingSetSize", ctypes.c_size_t),
        ("WorkingSetSize", ctypes.c_size_t),
        ("QuotaPeakPagedPoolUsage", ctypes.c_size_t),
        ("QuotaPagedPoolUsage", ctypes.c_size_t),
        ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
        ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
        ("PagefileUsage", ctypes.c_size_t),
        ("PeakPagefileUsage", ctypes.c_size_t),
    ]


_k32.CreateToolhelp32Snapshot.restype = wintypes.HANDLE
_k32.Process32FirstW.argtypes = [wintypes.HANDLE, ctypes.POINTER(PROCESSENTRY32W)]
_k32.Process32NextW.argtypes = [wintypes.HANDLE, ctypes.POINTER(PROCESSENTRY32W)]
_k32.OpenProcess.restype = wintypes.HANDLE
_k32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
_k32.CloseHandle.argtypes = [wintypes.HANDLE]
_psapi.GetProcessMemoryInfo.argtypes = [
    wintypes.HANDLE, ctypes.POINTER(PROCESS_MEMORY_COUNTERS), wintypes.DWORD]


def _norm(name):
    n = (name or "").lower()
    return n if n.endswith(".exe") else n + ".exe"


def tcp_listen_pids(port):
    """PIDs with a TCP LISTEN socket on `port`. No powershell.

    GetExtendedTcpTable is in-process. The old Get-NetTCPConnection spawn
    was 200-800 ms and ran on every dashboard / backend / overview load.
    """
    iph = ctypes.WinDLL("iphlpapi", use_last_error=True)
    AF_INET = 2
    TCP_TABLE_OWNER_PID_ALL = 5
    size = ctypes.c_ulong(0)
    iph.GetExtendedTcpTable(None, ctypes.byref(size), False,
                            AF_INET, TCP_TABLE_OWNER_PID_ALL, 0)
    if size.value <= 0:
        return []
    buf = ctypes.create_string_buffer(size.value)
    if iph.GetExtendedTcpTable(buf, ctypes.byref(size), False,
                               AF_INET, TCP_TABLE_OWNER_PID_ALL, 0):
        return []

    class ROW(ctypes.Structure):
        _fields_ = [("dwState", wintypes.DWORD),
                    ("dwLocalAddr", wintypes.DWORD),
                    ("dwLocalPort", wintypes.DWORD),
                    ("dwRemoteAddr", wintypes.DWORD),
                    ("dwRemotePort", wintypes.DWORD),
                    ("dwOwningPid", wintypes.DWORD)]

    n = struct.unpack_from("<I", buf.raw, 0)[0]
    off = 4
    out = []
    want = int(port)
    import socket
    for _ in range(n):
        row = ROW.from_buffer_copy(buf.raw, off)
        off += ctypes.sizeof(ROW)
        if int(row.dwState) != 2:          # MIB_TCP_STATE_LISTEN
            continue
        if socket.ntohs(row.dwLocalPort & 0xFFFF) == want:
            out.append(int(row.dwOwningPid))
    return out


def pids_named(*names):
    """PIDs whose exe name matches any of `names` (with or without .exe)."""
    want = {_norm(n) for n in names}
    snap = _k32.CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
    if snap == wintypes.HANDLE(-1).value or snap == 0xFFFFFFFFFFFFFFFF:
        return []
    out = []
    try:
        pe = PROCESSENTRY32W()
        pe.dwSize = ctypes.sizeof(PROCESSENTRY32W)
        if not _k32.Process32FirstW(snap, ctypes.byref(pe)):
            return []
        while True:
            if pe.szExeFile.lower() in want:
                out.append(int(pe.th32ProcessID))
            if not _k32.Process32NextW(snap, ctypes.byref(pe)):
                break
    finally:
        _k32.CloseHandle(snap)
    return out


def alive(pid):
    if not pid:
        return False
    h = _k32.OpenProcess(PROCESS_QUERY_LIMITED, False, int(pid))
    if not h:
        return False
    _k32.CloseHandle(h)
    return True


def working_set(pid):
    """Working set in bytes, or None if the process is gone."""
    h = _k32.OpenProcess(PROCESS_QUERY_LIMITED, False, int(pid))
    if not h:
        return None
    try:
        pmc = PROCESS_MEMORY_COUNTERS()
        pmc.cb = ctypes.sizeof(pmc)
        if not _psapi.GetProcessMemoryInfo(h, ctypes.byref(pmc), pmc.cb):
            return None
        return int(pmc.WorkingSetSize)
    finally:
        _k32.CloseHandle(h)


def kill(pid):
    import subprocess
    subprocess.run(
        ["taskkill", "/PID", str(int(pid)), "/T", "/F"],
        capture_output=True, timeout=20,
        creationflags=CREATE_NO_WINDOW)


def kill_named(*names):
    """Force-kill every process whose exe matches, plus its children.

    /T is required: Steam starts a wrapper, and killing only the parent
    leaf leaves AssettoCorsaEVO.exe on screen.
    """
    import subprocess
    out = []
    for name in names:
        n = _norm(name)
        r = subprocess.run(
            ["taskkill", "/IM", n, "/T", "/F"],
            capture_output=True, timeout=20,
            creationflags=CREATE_NO_WINDOW)
        out.append((n, r.returncode))
    return out


def pids_named_prefix(*prefixes):
    """PIDs whose exe name starts with any of `prefixes` (case-insensitive)."""
    want = [p.lower() if p.lower().endswith(".exe") else p.lower()
            for p in prefixes]
    snap = _k32.CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
    if snap == wintypes.HANDLE(-1).value or snap == 0xFFFFFFFFFFFFFFFF:
        return []
    out = []
    try:
        pe = PROCESSENTRY32W()
        pe.dwSize = ctypes.sizeof(PROCESSENTRY32W)
        if not _k32.Process32FirstW(snap, ctypes.byref(pe)):
            return []
        while True:
            n = pe.szExeFile.lower()
            if any(n.startswith(p) for p in want):
                out.append(int(pe.th32ProcessID))
            if not _k32.Process32NextW(snap, ctypes.byref(pe)):
                break
    finally:
        _k32.CloseHandle(snap)
    return out


def hidden_run(cmd, **kw):
    """subprocess.run that never flashes a console."""
    import subprocess
    kw.setdefault("creationflags", CREATE_NO_WINDOW)
    return subprocess.run(cmd, **kw)


def hidden_popen(cmd, **kw):
    """Start a helper (Python / ACECM --tool) without a visible console.

    Do not use this for AssettoCorsaEVOServer.exe — that is a console
    subsystem binary and CREATE_NO_WINDOW stops it binding ports. Use
    hidden_console_popen for those.
    """
    import subprocess
    import sys
    if sys.platform == "win32":
        kw.setdefault("creationflags", CREATE_NO_WINDOW)
    return subprocess.Popen(cmd, **kw)


def hidden_console_popen(cmd, **kw):
    """Start a console exe hidden, but still give it a console.

    CREATE_NO_WINDOW (0x08000000) creates no console at all. Console-subsystem
    programs (the dedicated server) then never finish CRT startup and never
    listen on TCP. CREATE_NEW_CONSOLE + SW_HIDE allocates a console and hides
    the window — same on every Windows machine.
    """
    import subprocess
    import sys
    if sys.platform == "win32":
        si = kw.get("startupinfo") or subprocess.STARTUPINFO()
        si.dwFlags |= subprocess.STARTF_USESHOWWINDOW
        si.wShowWindow = 0
        kw["startupinfo"] = si
        kw["creationflags"] = kw.get("creationflags", 0) | subprocess.CREATE_NEW_CONSOLE
    return subprocess.Popen(cmd, **kw)


def hide_console():
    """Hide the console window so only the app window remains.

    ⚠ FreeConsole alone is not enough in a shipped build, and this is why the
    terminal shows for the .exe but not when running from source:

      * from source we start under pythonw, which never had a console;
      * the frozen build is --console AND onefile, which is TWO processes - a
        bootloader parent and the Python child. FreeConsole detaches only the
        caller, so the parent stays attached and its console window remains on
        screen for the whole session.

    Hiding the window works whoever owns it, so do that first and keep the
    detach as a fallback. The console is only ever a viewer here: everything
    printed to it is in the log file as well, so nothing is lost by hiding it.
    """
    import sys
    if sys.platform != "win32":
        return
    hidden = False
    try:
        # ⚠ Declare the types: ctypes defaults a return value to c_int, which
        # TRUNCATES a 64-bit window handle - the call then "succeeds" against a
        # garbage hwnd and hides nothing.
        _k32.GetConsoleWindow.restype = wintypes.HWND
        ctypes.windll.user32.ShowWindow.argtypes = [wintypes.HWND, ctypes.c_int]
        ctypes.windll.user32.ShowWindow.restype = wintypes.BOOL
        hwnd = _k32.GetConsoleWindow()
        if hwnd:
            SW_HIDE = 0
            ctypes.windll.user32.ShowWindow(hwnd, SW_HIDE)
            hidden = True
    except Exception:
        pass
    try:
        # Still detach: it releases the console handle, and on a non-onefile
        # build it closes the window outright.
        _k32.FreeConsole()
    except Exception:
        pass
    return hidden
