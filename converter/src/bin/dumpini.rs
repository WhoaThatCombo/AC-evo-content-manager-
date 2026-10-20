//! dev: print chosen files from a car's data (data.acd or data/)
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let d = aceconv::ac1::acd::read_car_data(std::path::Path::new(&a[1])).unwrap();
    for n in &a[2..] {
        println!("===== {n}");
        println!("{}", d.get(&n.to_lowercase()).map(|s| s.as_str()).unwrap_or("<missing>"));
    }
}
