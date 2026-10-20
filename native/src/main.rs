//! ACECM native shell (Tauri + WebView2).
//!
//! Phase 1 of the Rust port: this process owns the window and the public
//! HTTP port, and runs the Python backend hidden behind it for every route
//! not ported yet (see sidecar.rs, proxy.rs, native/PORTING.md).
// ⚠ A windowed program in EVERY build, debug included. As a console
// program, each copy the update/restart scripts started (`start ...`) and
// each launch from Explorer opened a terminal window next to the app. The
// switches that print (--install, --update-check, --headless) attach to the
// console they were started from; helpers write to redirected log files.
#![windows_subsystem = "windows"]


use std::sync::{Arc, Mutex};

use acecm::{installer, paths, proxy, sidecar, updater};
use serde_json::Value;
use std::time::{Duration, Instant};

use tauri::{Manager, RunEvent, WebviewUrl, WebviewWindowBuilder};

fn fail(msg: &str) -> ! {
    eprintln!("{msg}");
    #[cfg(windows)]
    message_box(msg);
    std::process::exit(1);
}

#[cfg(windows)]
fn message_box(msg: &str) {
    // a GUI-subsystem exe has no console to print to
    let wide: Vec<u16> = msg.encode_utf16().chain(Some(0)).collect();
    let title: Vec<u16> = acecm::edition::TITLE.encode_utf16().chain(Some(0)).collect();
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
            std::ptr::null_mut(), wide.as_ptr(), title.as_ptr(), 0x10);
    }
}

fn message_box_any(msg: &str) {
    eprintln!("{msg}");
    #[cfg(windows)]
    message_box(msg);
}

/// The console switches (--install, --update-check, --headless) print: a
/// GUI-subsystem exe borrows the console it was started from.
fn attach_console() {
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::System::Console::AttachConsole(u32::MAX);
    }
}

/// `--name value`, removed from argv.
fn take_opt(args: &mut Vec<String>, name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    args.remove(i);
    if i < args.len() { Some(args.remove(i)) } else { None }
}

fn http(timeout: f64) -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder().no_proxy().timeout(Duration::from_secs_f64(timeout))
        .build().unwrap_or_else(|_| reqwest::blocking::Client::new())
}

/// Is the thing on this port one of ours?
fn acecm_answering(port: u16) -> bool {
    http(2.5).get(format!("http://127.0.0.1:{port}/api/version")).send().ok()
        .and_then(|r| r.json::<Value>().ok())
        .map(|v| v["name"] == updater::NAME).unwrap_or(false)
}

/// Ask the instance on the port to show its window. False if it has none.
fn raise_window(port: u16) -> bool {
    http(5.0).post(format!("http://127.0.0.1:{port}/api/app/show"))
        .header("Content-Type", "application/json").body("{}").send().ok()
        .and_then(|r| r.json::<Value>().ok())
        .map(|v| v["ok"] == true).unwrap_or(false)
}

/// Bind the public port. ⚠ Wait for a HANDOVER first: after an update or
/// Restart the outgoing copy is briefly still bound. A just-updated copy
/// never defers (the one answering is the outgoing build shutting down).
fn bind(host: &str, port: u16, handover: bool) -> Result<std::net::TcpListener, String> {
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        match std::net::TcpListener::bind((host, port)) {
            Ok(l) => return Ok(l),
            Err(e) => {
                if (!handover && acecm_answering(port)) || Instant::now() >= deadline {
                    return Err(e.to_string());
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
}

#[derive(PartialEq, Clone, Copy)]
enum Mode { Window, Browser, Headless }

fn main() {
    // `acecm --tool <name>`: a helper process, never a second window
    let mut args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|a| a == "--tool").unwrap_or(false) {
        let code = match args.get(2).map(|s| s.as_str()) {
            Some("acevo_proxy") => acecm::lobbyproxy::run(),
            Some("acevo_backend") => acecm::lobbybackend::run(),
            Some("lobby_fetch") => acecm::lobbyfetch::run(&args[3..]),
            other => { eprintln!("unknown tool {other:?}"); 2 }
        };
        std::process::exit(code);
    }
    // written by the update swap script: the new exe must create it once it
    // is serving, or the previous build is put back
    let okflag = take_opt(&mut args, "--okflag");
    let relaunch = args.iter().any(|a| a == "--relaunch");
    let _ = take_opt(&mut args, "--updated");
    let has = |f: &str| args.iter().any(|a| a == f);
    acecm::edition::seed_config();

    // setup steps: run and exit, they are not the app
    if has("--install") || has("--uninstall") {
        attach_console();
        if has("--uninstall") {
            let r = installer::uninstall(has("--purge"));
            for f in r["removed"].as_array().cloned().unwrap_or_default() {
                println!("removed {}", f.as_str().unwrap_or(""));
            }
            println!("your settings and profiles are untouched: {}", r["data_kept"].as_str().unwrap_or(""));
            std::process::exit(if r["ok"] == true { 0 } else { 1 });
        }
        let r = installer::install(!has("--no-desktop"), false);
        if r["ok"] != true {
            println!("install failed: {}", r["error"].as_str().unwrap_or(""));
            std::process::exit(1);
        }
        println!("installed to {}", r["exe"].as_str().unwrap_or(""));
        for s in r["shortcuts"].as_array().cloned().unwrap_or_default() {
            println!("  shortcut: {}", s.as_str().unwrap_or(""));
        }
        println!("launch it from the Start Menu from now on");
        std::process::exit(0);
    }
    // support switch: what would the launch updater decide?
    if has("--update-check") {
        attach_console();
        println!("{}", serde_json::to_string_pretty(&installer::pending_update()).unwrap_or_default());
        std::process::exit(0);
    }

    // A build run from OUTSIDE the install folder (a fresh download) offers to
    // replace an older installed copy, relaunches it and exits. The installed
    // copy never offers anything. Any failure falls through to a normal boot.
    if updater::frozen() && !has("--no-update-prompt") && !relaunch {
        let pu = installer::pending_update();
        acecm::logs::info(format!("update check: this={} installed={} running_installed={} installed_flag={} -> offer={} offer_install={}",
            pu["this_version"], pu["installed_version"], pu["running_installed"], pu["installed"], pu["offer"], pu["offer_install"]));
        let update = pu["offer"] == true;
        if update || pu["offer_install"] == true {
            let want = if update { installer::prompt_update(&pu) } else { installer::prompt_install(&pu) };
            acecm::logs::info(format!("update prompt answered: {want}"));
            if want {
                let r = installer::apply_update(update);
                if r["ok"] == true {
                    std::process::exit(0);
                }
                let e = r["error"].as_str().unwrap_or("update failed").to_string();
                acecm::logs::warn(format!("update failed: {e}"));
                installer::messagebox(&e, acecm::edition::TITLE, false);
            }
        }
    }

    let mode = if has("--browser") { Mode::Browser }
        else if has("--headless") || has("--no-ui") { Mode::Headless } else { Mode::Window };
    if mode == Mode::Headless {
        attach_console();
        // ⚠ a headless copy exists to be reached from elsewhere: remote admin
        // on (the TOKEN still grants access, never the mode)
        let cfg = acecm::config::load();
        if cfg.get("remote_admin").and_then(|v| v.as_bool()) != Some(true) {
            let _ = acecm::config::save(&serde_json::json!({"remote_admin": true}));
        }
    }

    let listen = paths::listen();
    let std_listener = match bind(&listen.host, listen.port, relaunch || okflag.is_some()) {
        Ok(l) => l,
        Err(e) => {
            if !acecm_answering(listen.port) {
                fail(&format!("Port {0} is in use by something that is not ACECM, so the app cannot start.\n\n{e}\n\nClose whatever is using port {0}, or change ui_port in ACECM's settings file:\n{1}",
                    listen.port, paths::data().join("config.json").display()));
            }
            // ⚠ a second launch: confirm the update anyway (this binary is
            // fine, another instance owns the port - not confirming made
            // Restart-after-update roll itself back)
            updater::confirm_update(okflag.as_deref());
            if raise_window(listen.port) {
                acecm::logs::info("ACECM already running; raised its window");
                return;
            }
            if mode != Mode::Window {
                if mode == Mode::Browser {
                    acecm::launch::open_url(&format!("http://127.0.0.1:{}", listen.port));
                }
                return;
            }
            // the occupant has no window (headless, a leftover): take over
            acecm::logs::info("ACECM already running without a window; taking over");
            installer::ask_installed_to_quit(true);
            bind(&listen.host, listen.port, true).unwrap_or_else(|_| fail(
                "ACECM is already running without a window and did not exit when asked.\n\nClose the leftover process, then start ACECM again."))
        }
    };
    std_listener.set_nonblocking(true).ok();
    // ⚠ The port is ours: this binary is good enough to keep. Confirm the
    // update handshake NOW, before anything spawns helpers from this exe -
    // a helper still alive at the swap script's check would otherwise make a
    // build that died right after spawning it look like it started.
    updater::confirm_update(okflag.as_deref());
    // a swap that was scheduled has run (or given up) by the time anything
    // binds this port again: a flag left behind made Restart quit for good
    let _ = std::fs::remove_file(updater::pending_path());
    updater::consume_rollback();
    // the UI, catalogs and viewer this exe carries (release builds)
    acecm::bundle::ensure();
    // what app.serve() used to start (the backend skips it under the shell)
    acecm::startup::run(listen.port);
    // leaked Python onefile folders from before the native build
    std::thread::spawn(|| { let _ = installer::sweep_bundles(); });
    // fill the Convert page's car / track list caches once the window is up,
    // so the first visit is as quick as the second (cold tracks list: ~0.5 s)
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(3));
        let _ = acecm::convert::cars();
        let _ = acecm::converttrack::tracks();
    });

    // the Python backend: only for debug comparisons now (sidecar::wanted)
    let side = if sidecar::wanted() {
        Some(sidecar::Sidecar::start(listen.port)
            .unwrap_or_else(|e| fail(&format!("could not start the ACECM backend: {e}"))))
    } else {
        None
    };
    // ⚠ Do NOT wait for it here. The window, the page and every ported
    // route are ready at once; only a request that needs the backend waits
    // (proxy::wait_backend) - that is the startup the user sees.
    let (base, secret) = match &side {
        Some(s) => {
            let addr = s.addr;
            std::thread::spawn(move || {
                let ok = sidecar::probe(addr, Duration::from_secs(90));
                proxy::set_backend_ready(ok);
                if !ok {
                    message_box_any("the ACECM backend did not start - see data\\native_backend.log");
                }
            });
            (format!("http://{}", s.addr), s.secret.clone())
        }
        None => (String::new(), sidecar::secret()),
    };
    acecm::api::set_secret(&secret);
    acecm::api::set_backend(&base);
    let backend = Arc::new(proxy::Backend {
        enabled: side.is_some(),
        base,
        secret,
        client: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("http client"),
    });
    let side = Arc::new(Mutex::new(side));
    let start_background = || {
        // the pit button / key combination
        acecm::hotkey::watch();
        // share sizes are computed before the first joiner's probe (it
        // gives up at 4 s)
        std::thread::spawn(|| { let _ = acecm::registry::public_list(""); });
        // a stale saved server list is refreshed behind the first page
        std::thread::spawn(|| { let _ = acecm::backend::lobby_prefetch_default(); });
    };

    let url = format!("http://127.0.0.1:{}/", listen.port);
    if mode != Mode::Window {
        // no window: serve until asked to quit
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let side_q = side.clone();
        acecm::api::set_quit(move || {
            let _ = acecm::backend::stop();
            if let Some(mut s) = side_q.lock().unwrap().take() {
                s.stop();
            }
            std::process::exit(0);
        });
        acecm::api::set_show(|| false);
        start_background();
        rt.block_on(async move {
            let l = tokio::net::TcpListener::from_std(std_listener).expect("listener");
            if mode == Mode::Browser {
                acecm::launch::open_url(&url);
            } else {
                println!("Assetto Corsa EVO Content Manager (headless)\n  {url}");
            }
            proxy::serve(l, backend).await;
        });
        return;
    }
    let app = tauri::Builder::default()
        .setup(move |app| {
            let be = backend.clone();
            tauri::async_runtime::spawn(async move {
                let l = tokio::net::TcpListener::from_std(std_listener)
                    .expect("listener");
                proxy::serve(l, be).await;
            });
            // HTTP is up: this binary is good enough to keep. Before the
            // window, so a later window close is not mistaken for a failed
            // update.
            let h = app.handle().clone();
            acecm::api::set_quit(move || h.exit(0));
            let h = app.handle().clone();
            acecm::api::set_show(move || match h.get_webview_window("main") {
                Some(w) => {
                    let _ = w.unminimize();
                    let _ = w.show();
                    let _ = w.set_focus();
                    true
                }
                None => false,
            });
            start_background();
            WebviewWindowBuilder::new(app, "main",
                                      WebviewUrl::External(url.parse().unwrap()))
                .title(acecm::edition::TITLE)
                .inner_size(1320.0, 860.0)
                .min_inner_size(900.0, 600.0)
                .build()?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("tauri");

    let side_exit = side.clone();
    app.run(move |handle, ev| {
        if let RunEvent::Exit = ev {
            // the shell owns the lobby proxy: it goes with us
            let _ = acecm::backend::stop();
            if let Some(mut s) = side_exit.lock().unwrap().take() {
                s.stop();
            }
            let _ = handle.get_webview_window("main");
        }
    });
}
