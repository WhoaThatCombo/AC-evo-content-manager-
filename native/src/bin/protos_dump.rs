//! Dev tool: build the schema pool like the app does and report it.
fn main() {
    let t = std::time::Instant::now();
    println!("{}", acecm::protos::state());
    for n in std::env::args().skip(1) {
        println!("{n}: {}", acecm::protos::has(&n));
    }
    println!("took {:?}", t.elapsed());
}
