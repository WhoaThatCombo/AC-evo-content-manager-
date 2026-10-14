//! Dev tool: run every tracktables operation on real tables and write the
//! results, for a byte comparison with tracktables.py.
//!     tracktables_test <tracks.table> <track_containers.table> <outdir>
use std::collections::HashMap;

use acecm::tracktables as tt;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let tracks = std::fs::read(&a[1]).unwrap();
    let cont = std::fs::read(&a[2]).unwrap();
    let out = std::path::Path::new(&a[3]);
    let w = |n: &str, b: &[u8]| std::fs::write(out.join(n), b).unwrap();
    let names: Vec<String> = tt::registered_names(&tracks).unwrap().into_iter().collect();
    w("names.json", serde_json::to_string(&names).unwrap().as_bytes());
    let rows = tt::rows_for_folder(&tracks, "road_atlanta").unwrap();
    w("rows.json", serde_json::to_string(&rows).unwrap().as_bytes());
    w("upsert_new.bin", &tt::upsert_track_row(&tracks, "ACECM Test Track", "acecm_test", Some("Road Atlanta")).unwrap());
    w("upsert_existing.bin", &tt::upsert_track_row(&tracks, "Road Atlanta", "acecm_test2", None).unwrap());
    w("remove_track.bin", &tt::remove_track_row(&tracks, "Road Atlanta").unwrap());
    w("remove_cont.bin", &tt::remove_container_rows(&cont, "Road Atlanta").unwrap());
    let mut c = HashMap::new();
    c.insert("layout".to_string(), "layout_layout_test".to_string());
    c.insert("race_scenery".to_string(), "ground_grass".to_string());
    c.insert("timelines".to_string(), "timelines".to_string());
    let (blob, made) = tt::upsert_container_rows(&cont, "ACECM Test Track", "acecm_test", "layout_test", &c, "Road Atlanta").unwrap();
    w("upsert_cont.bin", &blob);
    println!("made {made}");
}
