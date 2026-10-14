//! Dev tool: deploy / register / unregister / redeclare(dry) against copies
//! of an archive, for a byte comparison with tracks.py.
//!     trackdeploy_test deploy <archive-copy> <folder>
//!     trackdeploy_test register <archive-copy> <folder>
//!     trackdeploy_test unregister <archive-copy> <folder>
//!     trackdeploy_test redeclare_dry <archive>
//!     trackdeploy_test names
use acecm::trackdeploy as td;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let root = std::path::PathBuf::from(std::env::var("USERPROFILE").unwrap())
        .join("Saved Games").join("ACE").join("mods").join("content").join("tracks");
    let r = match a[1].as_str() {
        "deploy" => td::deploy_native_into(a[2].as_ref(), &root.join(&a[3]), false, None, None, false),
        "register" => td::register_track_into(a[2].as_ref(), &a[3], &serde_json::json!({}), false),
        "unregister" => td::unregister_track_into(a[2].as_ref(), &a[3], "", false),
        "redeclare_dry" => td::redeclare_tracks_into(a[2].as_ref(), true, false),
        _ => serde_json::json!(td::client_track_folders().iter()
            .map(|f| (f.clone(), td::display_name_for(f))).collect::<Vec<_>>()),
    };
    println!("{r}");
}
