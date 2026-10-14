//! Dev tool: a saved BackendMessage server-list frame -> the captured rows.
fn main() {
    let raw = std::fs::read(std::env::args().nth(1).unwrap()).unwrap();
    let (_, msg) = acecm::lobbyproxy::unwrap(&raw).unwrap();
    println!("{}", serde_json::Value::Array(acecm::lobbyproxy::capture_list(&msg.unwrap())));
}
