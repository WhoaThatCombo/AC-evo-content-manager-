//! dev: kn5 node tree (transforms + mesh counts), wheel datum, materials
use aceconv::ac1::kn5::{self, NodeKind, Node};
fn count(n: &Node) -> (usize, usize) {
    let mut m = (0, 0);
    if let NodeKind::Mesh(x) = &n.kind { m.0 += 1; m.1 += x.index_count() / 3; }
    for c in &n.children { let (a, b) = count(c); m.0 += a; m.1 += b; }
    m
}
fn show(n: &Node, depth: usize, max: usize, world: [f32; 16]) {
    let w = match &n.kind { NodeKind::Transform(m) => kn5::mul(m, &world), _ => world };
    if let NodeKind::Transform(_) = &n.kind {
        let (m, t) = count(n);
        println!("{}{} [{}] meshes={} tris={} pos=({:.3},{:.3},{:.3})", "  ".repeat(depth), n.name, if n.active { "on" } else { "OFF" }, m, t, w[12], w[13], w[14]);
        // KN5TREE_AXES=<substring>: also the world axes (rows x, y, z) of matching nodes
        if let Ok(f) = std::env::var("KN5TREE_AXES") {
            if n.name.to_lowercase().contains(&f.to_lowercase()) {
                println!("{}  axes x=({:.3},{:.3},{:.3}) y=({:.3},{:.3},{:.3}) z=({:.3},{:.3},{:.3})", "  ".repeat(depth), w[0], w[1], w[2], w[4], w[5], w[6], w[8], w[9], w[10]);
            }
        }
        if depth < max { for c in &n.children { show(c, depth + 1, max, w); } }
    }
}
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let f = std::fs::File::open(&a[1]).unwrap();
    let map = unsafe { memmap2::Mmap::map(&f).unwrap() };
    let k = kn5::parse(&map).unwrap();
    let depth: usize = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(3);
    show(&k.root, 0, depth, kn5::IDENTITY);
    if a.get(3).map(|s| s == "mats").unwrap_or(false) {
        for (i, m) in k.materials.iter().enumerate() {
            let p = |n: &str| m.prop(n).map(|p| p.value).unwrap_or(-1.0);
            println!("mat {i:2} {:28} shader {:24} blend {} at {} spec {:.2} exp {:.0} fres {:.2} amb {:.2} dif {:.2} emi {:.2} tex {:?}", m.name, m.shader, m.blend_mode, m.alpha_tested as u8,
                p("ksSpecular"), p("ksSpecularEXP"), p("fresnelMaxLevel"), p("ksAmbient"), p("ksDiffuse"), m.prop("ksEmissive").map(|p| p.v3[0].max(p.v3[1]).max(p.v3[2])).unwrap_or(0.0),
                m.textures.iter().map(|(s, t)| format!("{s}={t}")).collect::<Vec<_>>());
        }
    }
}
