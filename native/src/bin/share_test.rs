//! Loopback test of content sharing (compressed .acemod delivery included),
//! without the app's startup jobs. Run each side with its own sandbox:
//! ACECM_ROOT, USERPROFILE (the host shares tracks from its Saved Games) and
//! ACECM_CLIENT_KSPKG (a COPY of a content.kspkg - the joiner writes tables).
//!
//!   share_test host <port> <registry id>   serve the share, build packages
//!   share_test plan <base url> <registry id>
//!   share_test join <base url> <registry id>

use serde_json::json;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(|s| s.as_str()) {
        Some("host") => {
            let port: u16 = a[2].parse().expect("port");
            let sid = a[3].clone();
            std::thread::spawn(move || {
                // hash and queue the packages, then report when they are ready
                loop {
                    let m = acecm::registry::manifest(&sid, &format!("http://127.0.0.1:{port}"), true);
                    let n = m["acemods"].as_array().map(|v| v.len()).unwrap_or(0);
                    println!("manifest: {} files, {} packages ready, builder {}", m["files"].as_array().map(|v| v.len()).unwrap_or(0), n, acecm::acemod::status());
                    for p in m["acemods"].as_array().into_iter().flatten() {
                        println!("  {} {}: {} -> {} bytes", p["kind"], p["name"], p["raw_bytes"], p["size"]);
                    }
                    std::thread::sleep(std::time::Duration::from_secs(if n >= 2 { 60 } else { 5 }));
                }
            });
            let be = std::sync::Arc::new(acecm::proxy::Backend { enabled: false, base: String::new(), secret: String::new(), client: reqwest::Client::new() });
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let l = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.expect("bind");
                acecm::proxy::serve(l, be).await;
            });
        }
        Some("plan") => {
            let p = acecm::contentsync::plan(&a[2], &a[3]);
            println!("need {} files, {} bytes, download {} bytes", p["need"].as_array().map(|v| v.len()).unwrap_or(0), p["bytes"], p["download_bytes"]);
            for x in p["acemods"].as_array().into_iter().flatten() {
                println!("  package {} {}: {} -> {} bytes", x["kind"], x["name"], x["raw_bytes"], x["size"]);
            }
        }
        Some("join") => {
            let t = std::time::Instant::now();
            let r = acecm::jobs::install_content(&json!({"base": a[2], "ids": [a[3]]}));
            println!("start: {r}");
            loop {
                std::thread::sleep(std::time::Duration::from_millis(500));
                let s = acecm::jobs::status();
                let st = s["state"].as_str().unwrap_or("").to_string();
                if st != "running" && st != "starting" && !st.is_empty() {
                    println!("{:.1} s: {}", t.elapsed().as_secs_f64(), serde_json::to_string(&s).unwrap_or_default().chars().take(3000).collect::<String>());
                    break;
                }
            }
        }
        _ => eprintln!("share_test host <port> <id> | plan <base> <id> | join <base> <id>"),
    }
}
