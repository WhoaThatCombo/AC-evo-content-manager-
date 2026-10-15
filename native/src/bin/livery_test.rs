//! Dev tool: livery_test apply FILE SLOT COLOR | populate
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let r = match a[1].as_str() {
        "apply" => acecm::livery::apply_color(&a[2], &a[3], &a[4], false),
        _ => acecm::livery::populate(false),
    };
    println!("{r}");
}
