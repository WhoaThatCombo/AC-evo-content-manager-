//! Dev tool: write a Drive session into a copy of the settings folder.
//!     drive_write_test ROOT PICK.json
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let pick: serde_json::Value = serde_json::from_slice(&std::fs::read(&a[2]).unwrap()).unwrap();
    let r = acecm::drive::write_gamemode_into(std::path::Path::new(&a[1]), pick.as_object().unwrap(), false);
    println!("{r}");
}
