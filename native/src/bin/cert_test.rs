//! Dev tool: ensure_cert(force) under ACECM_DATA.
fn main() {
    let t = std::time::Instant::now();
    println!("{} ({:?})", acecm::backend::ensure_cert(true), t.elapsed());
}
