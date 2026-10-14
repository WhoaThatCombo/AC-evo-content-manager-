//! Dev tool: publish every profile then prune, against ACECM_DATA.
fn main() {
    let profs = acecm::profiles::load();
    for p in &profs {
        acecm::autoshare::publish(p);
    }
    let dropped = acecm::autoshare::prune(&profs);
    println!("{}", serde_json::json!({"profiles": profs.len(), "dropped": dropped}));
}
