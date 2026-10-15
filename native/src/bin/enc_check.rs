//! Dev: is_encrypted() on each archive given (read-only).
fn main() {
    for a in std::env::args().skip(1) {
        println!("{:?}  {a}", acecm::kspkg_write::is_encrypted(std::path::Path::new(&a)).map_err(|e| e.to_string()));
    }
}
