//! dev: every property of the named kn5 materials
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let f = std::fs::File::open(&a[1]).unwrap();
    let map = unsafe { memmap2::Mmap::map(&f).unwrap() };
    let k = aceconv::ac1::kn5::parse(&map).unwrap();
    for m in &k.materials {
        if a.len() > 2 && !a[2..].iter().any(|n| m.name.eq_ignore_ascii_case(n)) {
            continue;
        }
        let props: Vec<String> = m.props.iter().map(|(n, p)| format!("{n}={}", p.value)).collect();
        println!("{} [{}] blend={} at={} depth={} tex={:?}: {}", m.name, m.shader, m.blend_mode, m.alpha_tested, m.depth_mode, m.textures, props.join(" "));
    }
}
