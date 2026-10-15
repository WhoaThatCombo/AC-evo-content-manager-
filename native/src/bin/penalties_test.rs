//! Dev tool: penalties_test ARCHIVE state|off|on
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let p = std::path::Path::new(&a[1]);
    match a[2].as_str() {
        "off" => println!("{:?}", acecm::penalties::patch(p)),
        "on" => println!("{:?}", acecm::penalties::restore(p)),
        _ => {}
    }
    println!("{}", acecm::penalties::state(p));
}
