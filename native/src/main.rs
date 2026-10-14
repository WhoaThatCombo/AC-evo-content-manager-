//! ACECM native shell (Tauri + WebView2).
//!
//! Phase 1 of the Rust port: this process owns the window and the public
//! HTTP port, and runs the Python backend hidden behind it for every route
//! not ported yet (see sidecar.rs, proxy.rs, native/PORTING.md).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]


use std::sync::{Arc, Mutex};

use acecm::{paths, proxy, sidecar};
use std::time::Duration;

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
    let title: Vec<u16> = "ACECM".encode_utf16().chain(Some(0)).collect();
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

fn main() {
    // `acecm --tool <name>`: a helper process, never a second window
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|a| a == "--tool").unwrap_or(false) {
        let code = match args.get(2).map(|s| s.as_str()) {
            Some("acevo_proxy") => acecm::lobbyproxy::run(),
            Some("lobby_fetch") => acecm::lobbyfetch::run(&args[3..]),
            other => { eprintln!("unknown tool {other:?}"); 2 }
        };
        std::process::exit(code);
    }
    let listen = paths::listen();
    // ⚠ Bind the public port FIRST: if another ACECM already has it, say so
    // and stop, rather than starting a second backend on the same data.
    let std_listener = std::net::TcpListener::bind((listen.host.as_str(), listen.port))
        .unwrap_or_else(|e| fail(&format!(
            "ACECM could not open port {} ({e}).\n\nIs ACECM already running?",
            listen.port)));
    std_listener.set_nonblocking(true).ok();

    let side = sidecar::Sidecar::start(listen.port)
        .unwrap_or_else(|e| fail(&format!("could not start the ACECM backend: {e}")));
    // ⚠ Do NOT wait for it here. The window, the page and every ported
    // route are ready at once; only a request that needs the backend waits
    // (proxy::wait_backend) - that is the startup the user sees.
    let addr = side.addr;
    std::thread::spawn(move || {
        let ok = sidecar::probe(addr, Duration::from_secs(90));
        proxy::set_backend_ready(ok);
        if !ok {
            message_box_any("the ACECM backend did not start - see data\\native_backend.log");
        }
    });
    acecm::api::set_secret(&side.secret);
    acecm::api::set_backend(&format!("http://{}", side.addr));
    let backend = Arc::new(proxy::Backend {
        base: format!("http://{}", side.addr),
        secret: side.secret.clone(),
        client: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("http client"),
    });
    let side = Arc::new(Mutex::new(Some(side)));

    let url = format!("http://127.0.0.1:{}/", listen.port);
    let app = tauri::Builder::default()
        .setup(move |app| {
            let be = backend.clone();
            tauri::async_runtime::spawn(async move {
                let l = tokio::net::TcpListener::from_std(std_listener)
                    .expect("listener");
                proxy::serve(l, be).await;
            });
            // a stale saved server list is refreshed behind the first page
            std::thread::spawn(|| { let _ = acecm::backend::lobby_prefetch_default(); });
            WebviewWindowBuilder::new(app, "main",
                                      WebviewUrl::External(url.parse().unwrap()))
                .title("ACECM")
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
