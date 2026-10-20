//! dev: UV statistics of the meshes whose material matches a pattern
//! kn5uv <file.kn5> <material regex>
use aceconv::ac1::kn5::{self, Node, NodeKind};

fn walk(n: &Node, k: &kn5::Kn5, re: &regex::Regex, shown: &mut usize) {
    if let NodeKind::Mesh(m) = &n.kind {
        let mat = k.materials.get(m.material_id as usize).map(|x| x.name.as_str()).unwrap_or("?");
        if re.is_match(mat) && *shown < 12 {
            *shown += 1;
            let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
            for v in 0..m.vertex_count {
                let uv = m.uv(v);
                for c in 0..2 {
                    lo[c] = lo[c].min(uv[c]);
                    hi[c] = hi[c].max(uv[c]);
                }
            }
            // UV area / world area per triangle: a texel density; scrambled UVs
            // give wildly varying ratios (and near-zero UV areas)
            let mut ratios: Vec<f64> = Vec::new();
            let ic = m.index_count();
            let mut t = 0;
            while t + 2 < ic {
                let (a, b, c) = (m.index(t) as usize, m.index(t + 1) as usize, m.index(t + 2) as usize);
                t += 3;
                let (pa, pb, pc) = (m.position(a), m.position(b), m.position(c));
                let (ua, ub, uc) = (m.uv(a), m.uv(b), m.uv(c));
                let cr = |p: [f32; 3], q: [f32; 3], r: [f32; 3]| {
                    let (x, y) = ([q[0] - p[0], q[1] - p[1], q[2] - p[2]], [r[0] - p[0], r[1] - p[1], r[2] - p[2]]);
                    let c = [x[1] * y[2] - x[2] * y[1], x[2] * y[0] - x[0] * y[2], x[0] * y[1] - x[1] * y[0]];
                    ((c[0] * c[0] + c[1] * c[1] + c[2] * c[2]) as f64).sqrt() / 2.0
                };
                let wa = cr(pa, pb, pc);
                let uva = (((ub[0] - ua[0]) * (uc[1] - ua[1]) - (uc[0] - ua[0]) * (ub[1] - ua[1])) as f64).abs() / 2.0;
                if wa > 1e-6 {
                    ratios.push(uva / wa);
                }
            }
            ratios.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let q = |f: f64| ratios.get(((ratios.len() as f64 - 1.0) * f) as usize).copied().unwrap_or(0.0);
            let zero = ratios.iter().filter(|&&r| r < 1e-9).count();
            println!("{:28} {:6} tris  uv [{:.2},{:.2}]..[{:.2},{:.2}]  uv/m2 p10 {:.2e} p50 {:.2e} p90 {:.2e}  zero-area {}", n.name, ic / 3, lo[0], lo[1], hi[0], hi[1], q(0.1), q(0.5), q(0.9), zero);
        }
    }
    for c in &n.children {
        walk(c, k, re, shown);
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let f = std::fs::File::open(&a[1]).unwrap();
    let map = unsafe { memmap2::Mmap::map(&f).unwrap() };
    let k = kn5::parse(&map).unwrap();
    let re = regex::Regex::new(&format!("(?i){}", a.get(2).map(|s| s.as_str()).unwrap_or("."))).unwrap();
    let mut shown = 0;
    walk(&k.root, &k, &re, &mut shown);
}
