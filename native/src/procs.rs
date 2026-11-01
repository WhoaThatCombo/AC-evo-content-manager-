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

    #[derive(Clone)]
    pub struct Proc {
        pub pid: u32,
        pub ppid: u32,
        pub name: String,
    }

    fn wstr(buf: &[u16]) -> String {
        let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..n])
    }

    static SNAP: std::sync::Mutex<Option<(std::time::Instant, Vec<Proc>)>> = std::sync::Mutex::new(None);
    const SNAP_TTL_MS: u128 = 1500;

    /// Every process, from a snapshot at most 1.5 s old.
    /// ⚠ A snapshot costs ~7 ms (hundreds of processes and their threads),
    /// one request used to take several, and the live feed took one per
    /// tick - 1.5% of a core for as long as a window was open. Anything WE
    /// spawn or kill clears it (invalidate_snapshot), so our own changes are
    /// seen at once; a process someone else starts or ends shows up within
    /// 1.5 s, well inside what every waiting loop tolerates.
    pub fn snapshot() -> Vec<Proc> {
        let mut g = SNAP.lock().unwrap();
        if let Some((at, v)) = g.as_ref() {
            if at.elapsed().as_millis() < SNAP_TTL_MS {
                return v.clone();
            }
        }
        let v = snapshot_fresh();
        *g = Some((std::time::Instant::now(), v.clone()));
        v
    }

    pub fn invalidate_snapshot() {
        *SNAP.lock().unwrap() = None;
    }

    #[link(name = "ntdll")]
    extern "system" {
        fn NtQuerySystemInformation(class: u32, buf: *mut core::ffi::c_void, len: u32, ret: *mut u32) -> i32;
    }

    /// Every process, from one NtQuerySystemInformation(SystemProcessInformation)
    /// call - what Toolhelp wraps, at about half its cost (7 vs 12 ms here).
    /// Falls back to Toolhelp if the call or the layout ever disagrees.
    pub fn snapshot_fresh() -> Vec<Proc> {
        // the offsets below are the x64 layout
        if cfg!(not(target_pointer_width = "64")) {
            return toolhelp();
        }
        const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004u32 as i32;
        let mut buf: Vec<u64> = vec![0; (2 << 20) / 8];
        for _ in 0..4 {
            let mut need = 0u32;
            let st = unsafe { NtQuerySystemInformation(5, buf.as_mut_ptr() as *mut _, (buf.len() * 8) as u32, &mut need) };
            if st == STATUS_INFO_LENGTH_MISMATCH {
                buf = vec![0; (need as usize + (1 << 16)) / 8 + 1];
                continue;
            }
            if st < 0 {
                break;
            }
            // SYSTEM_PROCESS_INFORMATION (x64): NextEntryOffset @0,
            // ImageName {u16 len, u16 max, ptr} @0x38, UniqueProcessId @0x50,
            // InheritedFromUniqueProcessId @0x58
            let base = buf.as_ptr() as *const u8;
            let total = buf.len() * 8;
            let mut out = Vec::new();
            let mut off = 0usize;
            loop {
                if off + 0x60 > total {
                    return toolhelp();
                }
                unsafe {
                    let e = base.add(off);
                    let next = (e as *const u32).read_unaligned() as usize;
                    let nlen = (e.add(0x38) as *const u16).read_unaligned() as usize / 2;
                    let nptr = (e.add(0x40) as *const *const u16).read_unaligned();
                    let pid = (e.add(0x50) as *const usize).read_unaligned() as u32;
                    let ppid = (e.add(0x58) as *const usize).read_unaligned() as u32;
                    let name = if nptr.is_null() || nlen == 0 {
                        String::new()
                    } else {
                        let start = nptr as usize;
                        if start < base as usize || start + nlen * 2 > base as usize + total {
                            return toolhelp();
                        }
                        String::from_utf16_lossy(std::slice::from_raw_parts(nptr, nlen))
                    };
                    out.push(Proc { pid, ppid, name });
                    if next == 0 {
                        break;
                    }
                    off += next;
                }
            }
            return out;
        }
        toolhelp()
    }

    /// Every process, from one Toolhelp snapshot.
    pub fn toolhelp() -> Vec<Proc> {
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
    invalidate_snapshot();
    let mut c = std::process::Command::new("taskkill");
    c.args(["/PID", &pid.to_string(), "/T", "/F"]);
    hide(&mut c);
    let ok = c.output().map(|o| o.status.success()).unwrap_or(false);
    invalidate_snapshot();
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
    fn ntquery_matches_toolhelp() {
        let a = snapshot_fresh();
        let b = toolhelp();
        let me = std::process::id();
        let mine = a.iter().find(|p| p.pid == me).expect("self in the snapshot");
        let th = b.iter().find(|p| p.pid == me).unwrap();
        assert_eq!(mine.name.to_lowercase(), th.name.to_lowercase());
        assert_eq!(mine.ppid, th.ppid);
        // the two calls are a moment apart: allow a little churn
        let pa: std::collections::HashSet<u32> = a.iter().map(|p| p.pid).collect();
        let common = b.iter().filter(|p| pa.contains(&p.pid)).count();
        assert!(common * 100 >= b.len() * 95, "{common} of {}", b.len());
        let same_name = b.iter().filter(|p| p.pid > 4)
            .filter(|p| a.iter().any(|q| q.pid == p.pid && q.name.eq_ignore_ascii_case(&p.name))).count();
        assert!(same_name * 100 >= (b.len() - 2) * 95);
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
        // the contract: whoever spawns and then looks itself up clears the cache
        invalidate_snapshot();
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

/// Block until process `pid` has exited (true), or we cannot watch it.
/// ⚠ A handle is opened and WAITED on: "is the pid alive" lies for a dead
/// process whose handle something still holds.
#[cfg(windows)]
pub fn wait_exit(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, INFINITE, PROCESS_SYNCHRONIZE};
    let h = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if h.is_null() {
        return false;
    }
    unsafe {
        WaitForSingleObject(h, INFINITE);
        CloseHandle(h);
    }
    true
}

#[cfg(not(windows))]
pub fn wait_exit(pid: u32) -> bool {
    while alive(pid) {
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    true
}

/// Helper processes the shell starts go with it: exit as soon as the
/// process named in ACECM_PARENT_PID is gone (a hard-killed shell must not
/// leave a proxy holding its ports).
pub fn exit_with_parent() {
    let Some(pid) = std::env::var("ACECM_PARENT_PID").ok().and_then(|p| p.parse::<u32>().ok()) else { return };
    std::thread::spawn(move || {
        if wait_exit(pid) {
            println!("parent {pid} has exited - stopping");
            std::process::exit(0);
        }
    });
}

/// Quote one argument the way CommandLineToArgvW reads it back.
pub fn win_quote(a: &str) -> String {
    if !a.is_empty() && !a.contains([' ', '\t', '"']) {
        return a.to_string();
    }
    let mut out = String::from("\"");
    let mut slashes = 0;
    for c in a.chars() {
        match c {
            '\\' => slashes += 1,
            '"' => {
                out.push_str(&"\\".repeat(slashes * 2 + 1));
                out.push('"');
                slashes = 0;
            }
            _ => {
                out.push_str(&"\\".repeat(slashes));
                slashes = 0;
                out.push(c);
            }
        }
    }
    out.push_str(&"\\".repeat(slashes * 2));
    out.push('"');
    out
}

/// Start a CONSOLE program with its own console, hidden, output to `log`,
/// with extra environment. (CREATE_NO_WINDOW breaks console CRT startup.)
#[cfg(windows)]
pub fn spawn_hidden_console(exe: &std::path::Path, args: &[String], cwd: &std::path::Path, log: &std::path::Path, env: &[(&str, &str)]) -> Result<u32, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE};
    use windows_sys::Win32::System::Threading::{CreateProcessW, CREATE_NEW_CONSOLE, CREATE_NEW_PROCESS_GROUP, CREATE_UNICODE_ENVIRONMENT,
                                                PROCESS_INFORMATION, STARTF_USESHOWWINDOW, STARTF_USESTDHANDLES, STARTUPINFOW};
    let w = |s: &std::ffi::OsStr| -> Vec<u16> { s.encode_wide().chain(Some(0)).collect() };
    let mut cmd = win_quote(&exe.to_string_lossy());
    for a in args {
        cmd.push(' ');
        cmd.push_str(&win_quote(a));
    }
    let mut cmdw: Vec<u16> = cmd.encode_utf16().chain(Some(0)).collect();
    // environment block: ours plus the extras, sorted, double-NUL ended
    let mut vars: Vec<(String, String)> = std::env::vars().filter(|(k, _)| !env.iter().any(|(e, _)| e.eq_ignore_ascii_case(k))).collect();
    vars.extend(env.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    vars.sort_by_key(|(k, _)| k.to_uppercase());
    let mut block: Vec<u16> = Vec::new();
    for (k, v) in &vars {
        block.extend(format!("{k}={v}").encode_utf16());
        block.push(0);
    }
    block.push(0);
    unsafe {
        let sa = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: std::ptr::null_mut(), bInheritHandle: 1 };
        let lp = w(log.as_os_str());
        let h = CreateFileW(lp.as_ptr(), GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE, &sa, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, std::ptr::null_mut());
        if h == INVALID_HANDLE_VALUE {
            return Err(format!("cannot open {}", log.display()));
        }
        let mut si: STARTUPINFOW = std::mem::zeroed();
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        si.dwFlags = STARTF_USESHOWWINDOW | STARTF_USESTDHANDLES;
        si.wShowWindow = 0;
        si.hStdOutput = h;
        si.hStdError = h;
        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
        let cw = w(cwd.as_os_str());
        let exew = w(exe.as_os_str());
        let ok = CreateProcessW(exew.as_ptr(), cmdw.as_mut_ptr(), std::ptr::null(), std::ptr::null(), 1,
                                CREATE_NEW_CONSOLE | CREATE_NEW_PROCESS_GROUP | CREATE_UNICODE_ENVIRONMENT,
                                block.as_ptr() as *const _, cw.as_ptr(), &si, &mut pi);
        CloseHandle(h);
        if ok == 0 {
            return Err(format!("CreateProcessW failed ({})", std::io::Error::last_os_error()));
        }
        CloseHandle(pi.hThread);
        CloseHandle(pi.hProcess);
        invalidate_snapshot();
        Ok(pi.dwProcessId)
    }
}

/// Start a CONSOLE program in a VISIBLE console window of its own, for a
/// tool that talks to the user there (evoview --wheel-setup waits for Enter).
/// Run through cmd so the window stays up with its last words until a key.
/// ⚠ No STARTF_USESTDHANDLES: our own handles are those of a GUI process
/// (none), and passing them would leave the tool with no stdin to read.
#[cfg(windows)]
pub fn spawn_console_window(exe: &std::path::Path, args: &[String], cwd: &std::path::Path, title: &str) -> Result<u32, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{CreateProcessW, CREATE_NEW_CONSOLE, PROCESS_INFORMATION, STARTUPINFOW};
    let w = |s: &std::ffi::OsStr| -> Vec<u16> { s.encode_wide().chain(Some(0)).collect() };
    let mut inner = win_quote(&exe.to_string_lossy());
    for a in args {
        inner.push(' ');
        inner.push_str(&win_quote(a));
    }
    let comspec = std::env::var("ComSpec").unwrap_or_else(|_| r"C:\Windows\System32\cmd.exe".into());
    // cmd /c "<cmd line> & pause": the outer quotes are stripped by cmd
    let cmd = format!("{} /c \"{inner} & echo. & pause\"", win_quote(&comspec));
    let mut cmdw: Vec<u16> = cmd.encode_utf16().chain(Some(0)).collect();
    let tw = w(std::ffi::OsStr::new(title));
    unsafe {
        let mut si: STARTUPINFOW = std::mem::zeroed();
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        si.lpTitle = tw.as_ptr() as *mut u16;
        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
        let cw = w(cwd.as_os_str());
        let ok = CreateProcessW(std::ptr::null(), cmdw.as_mut_ptr(), std::ptr::null(), std::ptr::null(), 0,
                                CREATE_NEW_CONSOLE, std::ptr::null(), cw.as_ptr(), &si, &mut pi);
        if ok == 0 {
            return Err(format!("CreateProcessW failed ({})", std::io::Error::last_os_error()));
        }
        CloseHandle(pi.hThread);
        CloseHandle(pi.hProcess);
        invalidate_snapshot();
        Ok(pi.dwProcessId)
    }
}

#[cfg(not(windows))]
pub fn spawn_console_window(exe: &std::path::Path, args: &[String], cwd: &std::path::Path, _title: &str) -> Result<u32, String> {
    std::process::Command::new(exe).args(args).current_dir(cwd).spawn().map(|c| c.id()).map_err(|e| e.to_string())
}

#[cfg(not(windows))]
pub fn spawn_hidden_console(exe: &std::path::Path, args: &[String], cwd: &std::path::Path, log: &std::path::Path, env: &[(&str, &str)]) -> Result<u32, String> {
    let f = std::fs::File::create(log).map_err(|e| e.to_string())?;
    let mut c = std::process::Command::new(exe);
    c.args(args).current_dir(cwd).stdout(f.try_clone().map_err(|e| e.to_string())?).stderr(f);
    for (k, v) in env {
        c.env(k, v);
    }
    c.spawn().map(|ch| ch.id()).map_err(|e| e.to_string())
}
