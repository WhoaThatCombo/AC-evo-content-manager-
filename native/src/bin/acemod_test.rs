//! Dev tool: the Content page's .acemod export / install, without the app.
//! Run it in a sandbox: ACECM_DATA (its config.json names the mods folders
//! and the Convert work folder), ACECM_CLIENT_KSPKG (a COPY - installing a
//! track writes its tables) and ACECM_SHARE_BASE (the game's content.kspkg,
//! read only: the stock files a package is rebuilt from).
//!
//!   acemod_test export <car|track> <name> <dest dir> [zip]
//!   acemod_test peek <file.acemod>
//!   acemod_test install <file>        preview, then install (overwrite)
//!   acemod_test fake-archive <out.kspkg>  a small archive holding only the
//!                                     game's tracks / track_containers tables

use serde_json::json;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let t = std::time::Instant::now();
    let r = match a.get(1).map(|s| s.as_str()) {
        Some("export") => acecm::install::export_item(&a[2], &a[3], Some(&a[4]), a.get(5).map(|s| s.as_str()).unwrap_or("acemod")),
        Some("peek") => acecm::acemod::peek(std::path::Path::new(&a[2])).unwrap_or_else(|e| json!({"error": e})),
        Some("install") => {
            let pre = acecm::install::ingest(&a[2], false);
            let mut p = pre.clone();
            if let Some(m) = p.get_mut("acemod").and_then(|m| m.as_object_mut()) {
                m.remove("files");
            }
            println!("preview: {p}");
            if pre.get("need_confirm") != Some(&json!(true)) {
                pre
            } else {
                acecm::install::ingest(&a[2], true)
            }
        }
        Some("fake-archive") => fake_archive(std::path::Path::new(&a[2])),
        _ => json!({"error": "acemod_test export|peek|install|fake-archive ..."}),
    };
    println!("{:.1} s: {r}", t.elapsed().as_secs_f64());
}

/// An encrypted archive with nothing but the client's two track tables
/// (read from the real game, which is not written).
fn fake_archive(out: &std::path::Path) -> serde_json::Value {
    use acecm::kspkg::{decrypt, BUCKET, TABLE};
    let game = std::env::var("ACECM_SHARE_BASE").expect("ACECM_SHARE_BASE = the game's content.kspkg");
    let g = std::path::Path::new(&game);
    let tk = acecm::trackdeploy::read_entry(g, r"system\tracks.table").expect("tracks.table");
    let tc = acecm::trackdeploy::read_entry(g, r"system\track_containers.table").expect("track_containers.table");
    let seed = out.with_extension("seed.kspkg");
    let mut table = vec![0u8; TABLE as usize];
    let first = acecm::kspkg_write::bucket("system", 0, 0, Some(acecm::kspkg_write::KIND_DIR)).unwrap();
    for (i, c) in table.chunks_exact_mut(BUCKET).enumerate() {
        if i == 0 {
            c.copy_from_slice(&first);
        }
        decrypt(c, 0);
    }
    std::fs::write(&seed, &table).unwrap();
    let r = acecm::kspkg_write::write_archive(&seed, out, &[(r"system\tracks.table".into(), tk), (r"system\track_containers.table".into(), tc)]);
    let _ = std::fs::remove_file(&seed);
    r.unwrap_or_else(|e| json!({"error": e}))
}
