//! Dev tool: apply assists values (JSON arg) under %USERPROFILE%.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let v: serde_json::Value = serde_json::from_str(&a[1]).unwrap();
    println!("{}", acecm::assists::write(&v));
}
