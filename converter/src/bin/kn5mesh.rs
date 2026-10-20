//! dev: every mesh node with its parent path, material, tris, active flags
use aceconv::ac1::kn5::{self, Node, NodeKind};

fn walk(n: &Node, path: &str, k: &kn5::Kn5, out: &mut Vec<String>) {
    let p = if path.is_empty() { n.name.clone() } else { format!("{path}/{}", n.name) };
    if let NodeKind::Mesh(m) = &n.kind {
        let mat = k.materials.get(m.material_id as usize).map(|x| x.name.as_str()).unwrap_or("?");
        out.push(format!("{:6} tris  act={} rend={} lod={}/{}  mat={:24} {}", m.index_count() / 3, n.active as u8,
            m.renderable as u8, m.lod_in, m.lod_out, mat, p));
    }
    for c in &n.children {
        walk(c, &p, k, out);
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let f = std::fs::File::open(&a[1]).unwrap();
    let map = unsafe { memmap2::Mmap::map(&f).unwrap() };
    let k = kn5::parse(&map).unwrap();
    let mut out = vec![];
    walk(&k.root, "", &k, &mut out);
    for l in out {
        println!("{l}");
    }
}
