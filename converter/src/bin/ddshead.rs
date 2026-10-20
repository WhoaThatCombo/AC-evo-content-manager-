//! dev: DDS header fields of a kn5-embedded texture
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let f = std::fs::File::open(&a[1]).unwrap();
    let map = unsafe { memmap2::Mmap::map(&f).unwrap() };
    let k = aceconv::ac1::kn5::parse(&map).unwrap();
    for t in k.textures.iter().filter(|t| a.len() < 3 || t.name.to_lowercase().contains(&a[2].to_lowercase())) {
        let b = t.data;
        let u = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        if b.len() > 128 && &b[0..4] == b"DDS " {
            println!("{:40} {}x{} mips {} pfflags {:#x} fourcc {:?} bits {} masks r {:#x} g {:#x} b {:#x} a {:#x}  size {}", t.name, u(16), u(12), u(28), u(80),
                String::from_utf8_lossy(&b[84..88]), u(88), u(92), u(96), u(100), u(104), b.len());
        } else {
            println!("{:40} not DDS: {:?} size {}", t.name, &b[..8.min(b.len())], b.len());
        }
    }
}
