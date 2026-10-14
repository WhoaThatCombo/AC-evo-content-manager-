//! ACECM native shell (Tauri + WebView2).
//!
//! Phase 1 of the Rust port: this process owns the window and the public
//! HTTP port, and runs the Python backend hidden behind it for every route
//! not ported yet (see sidecar.rs, proxy.rs, native/PORTING.md).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod paths;
mod proxy;
mod sidecar;
mod web;

use std::sync::{Arc, Mutex};
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

fn main() {
    let listen = paths::listen();
    // ⚠ Bind the public port FIRST: if another ACECM already has it, say so
    // and stop, rather than starting a second backend on the same data.
    let std_listener = std::net::TcpListener::bind((listen.host.as_str(), listen.port))
        .unwrap_or_else(|e| fail(&format!(
            "ACECM could not open port {} ({e}).\n\nIs ACECM already running?",
            listen.port)));
    std_listener.set_nonblocking(true).ok();

    let mut side = sidecar::Sidecar::start()
        .unwrap_or_else(|e| fail(&format!("could not start the ACECM backend: {e}")));
    if !side.wait_ready(Duration::from_secs(60)) {
        side.stop();
        fail("the ACECM backend did not start - see data\\native_backend.log");
    }
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
            if let Some(mut s) = side_exit.lock().unwrap().take() {
                s.stop();
            }
            let _ = handle.get_webview_window("main");
        }
    });
}
