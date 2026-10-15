//! Dev: a custom track cannot take over or remove a stock track's name.
//!     names_test <client archive COPY>
use serde_json::json;

fn main() {
    let a = std::path::PathBuf::from(std::env::args().nth(1).expect("archive copy"));
    let tk = {
        let mut f = std::fs::File::open(&a).unwrap();
        let e = acecm::kspkg::find_entry(&a, "system\\tracks.table").unwrap().unwrap();
        acecm::kspkg::read_entry(&mut f, &e).unwrap()
    };
    let names = acecm::tracktables::registered_names(&tk).unwrap();
    let stock = names.iter().find(|n| !n.is_empty()).unwrap().clone();
    println!("a stock name: {stock:?} ({} names)", names.len());
    let r = acecm::trackdeploy::register_track_into(&a, "zz_names_test", &json!({"display_name": stock}), false);
    println!("register a custom folder under that name: ok={} error={}", r["ok"], r["error"]);
    let r = acecm::trackdeploy::register_track_into(&a, "zz_names_test", &json!({"display_name": "ZZ Names Test"}), false);
    println!("register under a new name: ok={} error={}", r["ok"], r["error"]);
    let r = acecm::trackdeploy::unregister_track_into(&a, "zz_names_test", &stock, false);
    println!("unregister the stock name via the custom folder: ok={} error={}", r["ok"], r["error"]);
    let r = acecm::trackdeploy::unregister_track_into(&a, "zz_names_test", "ZZ Names Test", false);
    println!("unregister our own name: ok={}", r["ok"]);
}
