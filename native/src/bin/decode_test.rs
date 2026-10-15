//! Dev tool: decode_test MESSAGE ARCHIVE ENTRY - decode one archive entry
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let e = acecm::kspkg::find_entry(std::path::Path::new(&a[2]), &a[3]).unwrap().expect("no entry");
    let mut f = std::fs::File::open(&a[2]).unwrap();
    let b = acecm::kspkg::read_entry(&mut f, &e).unwrap();
    println!("{} bytes", b.len());
    match acecm::protos::decode_lenient(&a[1], &b) {
        Ok(m) => println!("ok: {}", m.fields().count()),
        Err(e) => println!("ERR {e}"),
    }
}
