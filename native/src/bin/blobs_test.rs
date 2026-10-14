//! Dev tool: profiles (JSON array on stdin) -> the decoded launch blobs.
use std::io::Read;

fn main() {
    let mut s = String::new();
    std::io::stdin().read_to_string(&mut s).unwrap();
    let profs: Vec<serde_json::Value> = serde_json::from_str(&s).unwrap();
    let out: Vec<serde_json::Value> = profs.iter().map(|p| match acecm::serverproc::build_blobs(p) {
        Ok((c, se)) => serde_json::json!({"cfg": c, "season": se}),
        Err(e) => serde_json::json!({"error": e}),
    }).collect();
    println!("{}", serde_json::Value::Array(out));
}
