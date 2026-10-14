//! Dev tool: list a .kspkg like kspkg.iter_entries, for comparing with Python.
//!     kspkg_dump <archive> [entry-to-find ...]
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let p = std::path::Path::new(&a[1]);
    let t = std::time::Instant::now();
    let es = acecm::kspkg::entries(p).expect("read");
    let mut sum: u64 = 0;
    for e in &es {
        sum = sum.wrapping_mul(31).wrapping_add(e.offset ^ e.size as u64 ^ acecm::kspkg::entry_id(&e.path));
    }
    println!("entries={} checksum={:016x} took={:?}", es.len(), sum, t.elapsed());
    for want in &a[2..] {
        let t = std::time::Instant::now();
        match acecm::kspkg::find_entry(p, want).expect("find") {
            Some(e) => println!("find {} -> size={} offset={} ({:?})", want, e.size, e.offset, t.elapsed()),
            None => println!("find {} -> none ({:?})", want, t.elapsed()),
        }
    }
}
