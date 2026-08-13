"""Silent Windows process helpers. Never shell out to powershell.exe.

ACECM used to spawn powershell.exe for Get-Process / Get-CimInstance. Each
call flashed a console and Windows toasted it. Toolhelp / psapi stay in-process.
"""
import ctypes
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
        ["taskkill", "/PID", str(int(pid)), "/F"],
        capture_output=True, timeout=20,
        creationflags=CREATE_NO_WINDOW)


def hidden_run(cmd, **kw):
    """subprocess.run that never flashes a console."""
    import subprocess
    kw.setdefault("creationflags", CREATE_NO_WINDOW)
    return subprocess.run(cmd, **kw)
