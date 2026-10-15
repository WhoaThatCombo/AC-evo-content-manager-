//! The Python backend, run hidden behind the shell while the port is in
//! progress. Every API route the shell has not ported yet is passed to it.
//!
//! ⚠ It listens on a PRIVATE loopback port chosen per launch; the shell owns
//! the public one. ⚠ It is handed a per-launch secret so it believes the
//! shell's X-ACECM-Peer header (who really connected) and nobody else's -
//! without that every LAN client would look like this machine.

use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::paths;

pub struct Sidecar {
    pub addr: SocketAddr,
    pub secret: String,
    child: Child,
}

pub fn python() -> PathBuf {
    if let Ok(p) = std::env::var("ACECM_PYTHON") {
        return PathBuf::from(p);
    }
    // ACECM must run on 3.14 (acecm-build-env-and-release-traps)
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        let p = PathBuf::from(local)
            .join("Python")
            .join("pythoncore-3.14-64")
            .join("python.exe");
        if p.is_file() {
            return p;
        }
    }
    PathBuf::from("python")
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(18092)
}

/// Whether to run the Python backend at all. Every route is the shell's
/// now; a debug build still starts it so tools/diff_api.py can compare
/// answers (ACECM_SIDECAR=1 / =0 overrides either way).
pub fn wanted() -> bool {
    match std::env::var("ACECM_SIDECAR").as_deref() {
        Ok("1") => true,
        Ok("0") => false,
        _ => cfg!(debug_assertions),
    }
}

pub fn secret() -> String {
    let mut b = [0u8; 24];
    getrandom::fill(&mut b).expect("no OS randomness");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Is a backend process (still) part of this run?
pub fn running() -> bool {
    RUNNING.load(std::sync::atomic::Ordering::Relaxed)
}

impl Sidecar {
    pub fn start(public_port: u16) -> std::io::Result<Sidecar> {
        let port = free_port();
        let secret = secret();
        let data = paths::data();
        let _ = std::fs::create_dir_all(&data);
        let log = std::fs::File::create(data.join("native_backend.log"))?;
        let mut cmd = Command::new(python());
        cmd.args(["-u", "-m", "acecm", "--headless"])
            .current_dir(paths::repo())
            .env("ACECM_ROOT", paths::root())
            .env("ACECM_UI_PORT", port.to_string())
            .env("ACECM_LISTEN", "127.0.0.1")
            .env("ACECM_PROXY_SECRET", &secret)
            .env("ACECM_PARENT_PID", std::process::id().to_string())
            // the reverse bridge: subsystems the shell owns (servers) are
            // asked here, with the same secret, instead of run twice
            .env("ACECM_NATIVE_URL", format!("http://127.0.0.1:{public_port}"))
            .env("PYTHONUNBUFFERED", "1")
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let child = cmd.spawn()?;
        RUNNING.store(true, std::sync::atomic::Ordering::Relaxed);
        Ok(Sidecar {
            addr: SocketAddr::from(([127, 0, 0, 1], port)),
            secret,
            child,
        })
    }

}

/// Block until something accepts connections on `addr` (or give up).
pub fn probe(addr: SocketAddr, limit: Duration) -> bool {
    let until = Instant::now() + limit;
    while Instant::now() < until {
        if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

impl Sidecar {
    /// Block until the backend accepts connections (or gives up).
    #[allow(dead_code)]
    pub fn wait_ready(&mut self, limit: Duration) -> bool {
        let until = Instant::now() + limit;
        while Instant::now() < until {
            if let Ok(Some(_)) = self.child.try_wait() {
                return false; // it exited - see native_backend.log
            }
            if TcpStream::connect_timeout(&self.addr, Duration::from_millis(200)).is_ok() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        false
    }

    /// Ask it to quit first - that path stops the lobby proxy and telemetry
    /// helpers, which a bare kill would leave running - then kill it if it
    /// has not gone in a few seconds.
    pub fn stop(&mut self) {
        RUNNING.store(false, std::sync::atomic::Ordering::Relaxed);
        if let Ok(None) = self.child.try_wait() {
            self.ask_quit();
            let until = Instant::now() + Duration::from_secs(5);
            while Instant::now() < until {
                if let Ok(Some(_)) = self.child.try_wait() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn ask_quit(&self) {
        use std::io::{Read, Write};
        let Ok(mut s) = TcpStream::connect_timeout(&self.addr, Duration::from_millis(500)) else {
            return;
        };
        let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
        let req = [
            "POST /api/app/quit HTTP/1.1",
            "Host: 127.0.0.1",
            "X-ACECM-Peer: 127.0.0.1",
            &format!("X-ACECM-Proxy: {}", self.secret),
            "Content-Type: application/json",
            "Content-Length: 2",
            "Connection: close",
            "",
            "{}",
        ]
        .join("\r\n");
        let _ = s.write_all(req.as_bytes());
        let mut buf = [0u8; 256];
        let _ = s.read(&mut buf);
    }
}

impl Drop for Sidecar {
    fn drop(&mut self) {
        self.stop();
    }
}
