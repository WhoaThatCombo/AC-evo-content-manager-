//! Dev tool: per-profile lobby blob, allowed cars and car policy as JSON.
fn main() {
    let out: Vec<serde_json::Value> = acecm::profiles::load().iter().map(|p| serde_json::json!({
        "lobby": acecm::lobby::from_profile(p),
        "allowed": acecm::servers::allowed_car_ids(p),
        "policy": acecm::servers::car_policy(p),
    })).collect();
    println!("{}", serde_json::Value::Array(out));
}
