//! Process and port helpers - ported from acecm/_proc_win.py.
//! In-process Win32 only (Toolhelp, iphlpapi, psapi); never powershell.
#![allow(dead_code)] // filled in ahead of the modules that use it

#[cfg(windows)]
mod imp {
    use std::path::Path;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_ALL,
    };
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, QueryFullProcessImageNameW,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    pub struct Proc {
        pub pid: u32,
        pub ppid: u32,
        pub name: String,
    }

    fn wstr(buf: &[u16]) -> String {
        let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..n])
    }

    /// Every process, from one Toolhelp snapshot.
    pub fn snapshot() -> Vec<Proc> {
        let mut out = vec![];
        unsafe {
            let snap: HANDLE = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE_VALUE || snap.is_null() {
                return out;
            }
            let mut pe: PROCESSENTRY32W = std::mem::zeroed();
            pe.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            if Process32FirstW(snap, &mut pe) != 0 {
                loop {
                    out.push(Proc { pid: pe.th32ProcessID, ppid: pe.th32ParentProcessID,
                                    name: wstr(&pe.szExeFile) });
                    if Process32NextW(snap, &mut pe) == 0 {
                        break;
                    }
                }
            }
            CloseHandle(snap);
        }
        out
    }

    fn open(pid: u32) -> Option<HANDLE> {
        let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if h.is_null() { None } else { Some(h) }
    }

    /// ⚠ Exit code, not "can I open it": a dead process stays openable while
    /// anyone holds a handle to it (WebView2's helpers did, for the shell).
    pub fn alive(pid: u32) -> bool {
        if pid == 0 {
            return false;
        }
        let Some(h) = open(pid) else { return false };
        let mut code = 0u32;
        let ok = unsafe { GetExitCodeProcess(h, &mut code) } != 0;
        unsafe { CloseHandle(h) };
        ok && code == 259 // STILL_ACTIVE
    }

    pub fn exe_path(pid: u32) -> String {
        let Some(h) = open(pid) else { return String::new() };
        let mut buf = vec![0u16; 32768];
        let mut n = buf.len() as u32;
        let ok = unsafe { QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut n) } != 0;
        unsafe { CloseHandle(h) };
        if ok { String::from_utf16_lossy(&buf[..n as usize]) } else { String::new() }
    }

    pub fn working_set(pid: u32) -> Option<u64> {
        let h = open(pid)?;
        let mut pmc: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
        pmc.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        let ok = unsafe { K32GetProcessMemoryInfo(h, &mut pmc, pmc.cb) } != 0;
        unsafe { CloseHandle(h) };
        ok.then_some(pmc.WorkingSetSize as u64)
    }

    /// PIDs with a TCP (IPv4) LISTEN socket on `port`.
    pub fn tcp_listen_pids(port: u16) -> Vec<u32> {
        let mut size = 0u32;
        unsafe {
            GetExtendedTcpTable(std::ptr::null_mut(), &mut size, 0, 2, TCP_TABLE_OWNER_PID_ALL, 0);
        }
        if size == 0 {
            return vec![];
        }
        let mut buf = vec![0u8; size as usize + 1024];
        size = buf.len() as u32;
        let r = unsafe {
            GetExtendedTcpTable(buf.as_mut_ptr() as _, &mut size, 0, 2, TCP_TABLE_OWNER_PID_ALL, 0)
        };
        if r != 0 {
            return vec![];
        }
        let n = u32::from_le_bytes(buf[0..4].try_into().unwrap()) as usize;
        let row = std::mem::size_of::<MIB_TCPROW_OWNER_PID>();
        let mut out = vec![];
        for i in 0..n {
            let off = 4 + i * row;
            if off + row > buf.len() {
                break;
            }
            let r: MIB_TCPROW_OWNER_PID =
                unsafe { std::ptr::read_unaligned(buf[off..].as_ptr() as *const _) };
            // MIB_TCP_STATE_LISTEN; the port is network order in the low word
            if r.dwState == 2 && u16::from_be(r.dwLocalPort as u16) == port {
                out.push(r.dwOwningPid);
            }
        }
        out
    }

    pub fn hide(cmd: &mut std::process::Command) {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

    pub fn exe_matches(p: &Path, name: &str) -> bool {
        p.file_name().map(|n| n.to_string_lossy().eq_ignore_ascii_case(name)).unwrap_or(false)
    }
}

#[cfg(windows)]
pub use imp::*;

fn norm(name: &str) -> String {
    let n = name.to_ascii_lowercase();
    if n.ends_with(".exe") { n } else { n + ".exe" }
}

/// PIDs whose exe name matches any of `names` (with or without .exe).
pub fn pids_named(names: &[&str]) -> Vec<u32> {
    let want: Vec<String> = names.iter().map(|n| norm(n)).collect();
    snapshot().into_iter().filter(|p| want.contains(&p.name.to_ascii_lowercase()))
        .map(|p| p.pid).collect()
}

/// PIDs whose exe name STARTS with any prefix (AssettoCorsaEVOServer*).
pub fn pids_named_prefix(prefixes: &[&str]) -> Vec<u32> {
    let want: Vec<String> = prefixes.iter().map(|p| p.to_ascii_lowercase()).collect();
    snapshot().into_iter()
        .filter(|p| { let n = p.name.to_ascii_lowercase(); want.iter().any(|w| n.starts_with(w)) })
        .map(|p| p.pid).collect()
}

pub fn ppid(pid: u32) -> u32 {
    snapshot().into_iter().find(|p| p.pid == pid).map(|p| p.ppid).unwrap_or(0)
}

/// Force-kill a process tree. True if it is gone afterwards.
/// ⚠ Report the OUTCOME: a server started elevated cannot be killed by a
/// non-elevated ACECM, and taskkill then fails while the server lives on.
pub fn kill(pid: u32) -> bool {
    let mut c = std::process::Command::new("taskkill");
    c.args(["/PID", &pid.to_string(), "/T", "/F"]);
    hide(&mut c);
    let ok = c.output().map(|o| o.status.success()).unwrap_or(false);
    if !alive(pid) {
        return true;
    }
    if ok {
        for _ in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            if !alive(pid) {
                return true;
            }
        }
    }
    !alive(pid)
}

/// Is the game itself running? (backend._game_running)
pub fn game_running() -> bool {
    !pids_named(&["AssettoCorsaEVO"]).is_empty()
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn sees_itself() {
        let me = std::process::id();
        assert!(alive(me));
        assert!(snapshot().iter().any(|p| p.pid == me));
        assert!(exe_path(me).to_ascii_lowercase().ends_with(".exe"));
        assert!(working_set(me).unwrap_or(0) > 0);
        assert_ne!(ppid(me), 0);
    }

    #[test]
    fn finds_a_listening_port() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        assert_eq!(tcp_listen_pids(port), vec![std::process::id()]);
        drop(l);
        assert!(tcp_listen_pids(port).is_empty());
    }

    #[test]
    fn kills_and_reports_dead() {
        let mut c = std::process::Command::new("ping");
        c.args(["-n", "30", "127.0.0.1"]);
        hide(&mut c);
        let mut child = c.spawn().unwrap();
        let pid = child.id();
        assert!(alive(pid));
        assert!(pids_named(&["PING"]).contains(&pid));
        assert!(kill(pid));
        let _ = child.wait();
        assert!(!alive(pid));
        assert!(!alive(0));
    }
}

/// Full command line of `pid`, or "" - read from the process itself
/// (ProcessCommandLineInformation), no powershell spawn. Works for any
/// process this user can query.
#[cfg(windows)]
pub fn cmdline(pid: u32) -> String {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    #[link(name = "ntdll")]
    extern "system" {
        fn NtQueryInformationProcess(h: *mut core::ffi::c_void, class: u32, info: *mut core::ffi::c_void,
                                     len: u32, ret: *mut u32) -> i32;
    }
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return String::new();
        }
        let mut need = 0u32;
        NtQueryInformationProcess(h, 60, std::ptr::null_mut(), 0, &mut need);
        let mut out = String::new();
        if need > 0 {
            let mut buf = vec![0u8; need as usize + 16];
            if NtQueryInformationProcess(h, 60, buf.as_mut_ptr() as _, buf.len() as u32, &mut need) >= 0 {
                // UNICODE_STRING { u16 Length, u16 MaximumLength, PWSTR Buffer }
                let len = u16::from_le_bytes([buf[0], buf[1]]) as usize / 2;
                let ptr = usize::from_le_bytes(buf[8..16].try_into().unwrap()) as *const u16;
                if !ptr.is_null() && len > 0 {
                    out = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
                }
            }
        }
        CloseHandle(h);
        out
    }
}

#[cfg(all(test, windows))]
mod cmdline_tests {
    #[test]
    fn reads_a_child_command_line() {
        let mut c = std::process::Command::new("ping");
        c.args(["-n", "30", "127.0.0.1"]);
        super::hide(&mut c);
        let mut child = c.spawn().unwrap();
        let line = super::cmdline(child.id());
        let _ = child.kill();
        let _ = child.wait();
        assert!(line.contains("-n 30 127.0.0.1"), "{line}");
    }
}
