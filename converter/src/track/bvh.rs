//! A small triangle BVH for the lighting bakes (vertex AO, irradiance
//! probes, the pigment map): binned SAH build, closest-hit ray queries.
//!
//! Compact on purpose (huge AC1 maps carry 20M+ triangles): a triangle is
//! its first vertex and two edges (36 bytes) plus a 4-byte payload, a node
//! 32 bytes, leaves of up to 4 triangles. The build is single-threaded and
//! deterministic (same input, same tree); queries are `&self` and run from
//! any number of threads.

#[derive(Clone, Copy)]
struct Node {
    lo: [f32; 3],
    /// leaf: first triangle; inner: left child (the right child follows it)
    first: u32,
    hi: [f32; 3],
    /// 0 = inner node, else the leaf's triangle count
    count: u32,
}

/// One triangle: vertex 0 and the edges to vertices 1 and 2.
#[derive(Clone, Copy)]
struct Tri {
    v0: [f32; 3],
    e1: [f32; 3],
    e2: [f32; 3],
}

pub struct Bvh {
    nodes: Vec<Node>,
    tris: Vec<Tri>,
    /// caller payload per triangle (in tree order)
    payload: Vec<u32>,
}

/// A ray hit.
#[derive(Clone, Copy, Debug)]
pub struct RayHit {
    pub t: f32,
    pub payload: u32,
    /// the ray met the triangle's back (counter-clockwise front, as the
    /// meshes are wound)
    pub back: bool,
    /// unit geometric normal of the triangle (front side)
    pub normal: [f32; 3],
}

const LEAF: usize = 4;
const BINS: usize = 12;

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn area(lo: [f32; 3], hi: [f32; 3]) -> f32 {
    let d = [(hi[0] - lo[0]).max(0.0), (hi[1] - lo[1]).max(0.0), (hi[2] - lo[2]).max(0.0)];
    d[0] * d[1] + d[1] * d[2] + d[2] * d[0]
}

impl Bvh {
    /// Build over triangles given as three vertices each, with a payload
    /// per triangle. Degenerate and non-finite triangles are dropped.
    pub fn build(tris: Vec<[[f32; 3]; 3]>, payload: Vec<u32>) -> Bvh {
        let mut keep: Vec<(Tri, u32, [f32; 3], [f32; 3])> = Vec::with_capacity(tris.len());
        for (t, p) in tris.into_iter().zip(payload) {
            if !t.iter().flatten().all(|v| v.is_finite()) {
                continue;
            }
            let e1 = sub(t[1], t[0]);
            let e2 = sub(t[2], t[0]);
            let n = cross(e1, e2);
            if dot(n, n) < 1e-14 {
                continue;
            }
            let lo = [0, 1, 2].map(|k| t[0][k].min(t[1][k]).min(t[2][k]));
            let hi = [0, 1, 2].map(|k| t[0][k].max(t[1][k]).max(t[2][k]));
            keep.push((Tri { v0: t[0], e1, e2 }, p, lo, hi));
        }
        let n = keep.len();
        let mut order: Vec<u32> = (0..n as u32).collect();
        let cent: Vec<[f32; 3]> = keep.iter().map(|k| [0, 1, 2].map(|i| (k.2[i] + k.3[i]) * 0.5)).collect();
        let mut nodes: Vec<Node> = Vec::with_capacity(n / 2 + 1);
        if n == 0 {
            return Bvh { nodes, tris: Vec::new(), payload: Vec::new() };
        }
        let bounds = |ix: &[u32]| {
            let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
            for &i in ix {
                let k = &keep[i as usize];
                for a in 0..3 {
                    lo[a] = lo[a].min(k.2[a]);
                    hi[a] = hi[a].max(k.3[a]);
                }
            }
            (lo, hi)
        };
        // (node index, start, end)
        let (lo, hi) = bounds(&order);
        nodes.push(Node { lo, hi, first: 0, count: 0 });
        let mut stack = vec![(0usize, 0usize, n)];
        while let Some((ni, s, e)) = stack.pop() {
            let cnt = e - s;
            if cnt <= LEAF {
                nodes[ni].first = s as u32;
                nodes[ni].count = cnt as u32;
                continue;
            }
            // centroid bounds
            let (mut clo, mut chi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
            for &i in &order[s..e] {
                let c = cent[i as usize];
                for a in 0..3 {
                    clo[a] = clo[a].min(c[a]);
                    chi[a] = chi[a].max(c[a]);
                }
            }
            let mut best = (f32::INFINITY, 0usize, 0usize);
            for a in 0..3 {
                let ext = chi[a] - clo[a];
                if ext <= 1e-6 {
                    continue;
                }
                let mut bin_n = [0usize; BINS];
                let mut bin_lo = [[f32::INFINITY; 3]; BINS];
                let mut bin_hi = [[f32::NEG_INFINITY; 3]; BINS];
                for &i in &order[s..e] {
                    let b = (((cent[i as usize][a] - clo[a]) / ext * BINS as f32) as usize).min(BINS - 1);
                    bin_n[b] += 1;
                    let k = &keep[i as usize];
                    for q in 0..3 {
                        bin_lo[b][q] = bin_lo[b][q].min(k.2[q]);
                        bin_hi[b][q] = bin_hi[b][q].max(k.3[q]);
                    }
                }
                // sweep: cost of splitting after bin j
                let mut right_area = [0f32; BINS];
                let mut right_n = [0usize; BINS];
                let (mut rlo, mut rhi, mut rn) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3], 0usize);
                for j in (1..BINS).rev() {
                    rn += bin_n[j];
                    for q in 0..3 {
                        rlo[q] = rlo[q].min(bin_lo[j][q]);
                        rhi[q] = rhi[q].max(bin_hi[j][q]);
                    }
                    right_area[j] = area(rlo, rhi);
                    right_n[j] = rn;
                }
                let (mut llo, mut lhi, mut ln) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3], 0usize);
                for j in 0..BINS - 1 {
                    ln += bin_n[j];
                    for q in 0..3 {
                        llo[q] = llo[q].min(bin_lo[j][q]);
                        lhi[q] = lhi[q].max(bin_hi[j][q]);
                    }
                    if ln == 0 || right_n[j + 1] == 0 {
                        continue;
                    }
                    let c = area(llo, lhi) * ln as f32 + right_area[j + 1] * right_n[j + 1] as f32;
                    if c < best.0 {
                        best = (c, a, j);
                    }
                }
            }
            let mid = if best.0.is_finite() {
                let (a, j) = (best.1, best.2);
                let ext = chi[a] - clo[a];
                let slice = &mut order[s..e];
                let mut l = 0;
                for r in 0..slice.len() {
                    let b = (((cent[slice[r] as usize][a] - clo[a]) / ext * BINS as f32) as usize).min(BINS - 1);
                    if b <= j {
                        slice.swap(l, r);
                        l += 1;
                    }
                }
                s + l
            } else {
                // every centroid in one point: split in the middle
                s + cnt / 2
            };
            let mid = if mid == s || mid == e { s + cnt / 2 } else { mid };
            let (llo, lhi) = bounds(&order[s..mid]);
            let (rlo, rhi) = bounds(&order[mid..e]);
            let left = nodes.len();
            nodes.push(Node { lo: llo, hi: lhi, first: 0, count: 0 });
            let right = nodes.len();
            nodes.push(Node { lo: rlo, hi: rhi, first: 0, count: 0 });
            nodes[ni].first = left as u32;
            stack.push((right, mid, e));
            stack.push((left, s, mid));
        }
        let tris: Vec<Tri> = order.iter().map(|&i| keep[i as usize].0).collect();
        let payload: Vec<u32> = order.iter().map(|&i| keep[i as usize].1).collect();
        Bvh { nodes, tris, payload }
    }

    pub fn len(&self) -> usize {
        self.tris.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tris.is_empty()
    }

    /// Bytes held (for the memory budget report).
    pub fn bytes(&self) -> usize {
        self.nodes.len() * std::mem::size_of::<Node>() + self.tris.len() * (std::mem::size_of::<Tri>() + 4)
    }

    /// The closest hit along `o + t d` for t in (tmin, tmax); `d` need not
    /// be unit length (t is in units of |d|).
    pub fn hit(&self, o: [f32; 3], d: [f32; 3], tmin: f32, tmax: f32) -> Option<RayHit> {
        if self.nodes.is_empty() {
            return None;
        }
        let inv = [1.0 / d[0], 1.0 / d[1], 1.0 / d[2]];
        let mut best_t = tmax;
        let mut best: Option<(usize, bool)> = None;
        let mut stack = [0u32; 64];
        let mut sp = 0usize;
        let mut ni = 0usize;
        loop {
            let n = &self.nodes[ni];
            if n.count > 0 {
                let f = n.first as usize;
                for ti in f..f + n.count as usize {
                    let t = &self.tris[ti];
                    // Moller-Trumbore, both faces
                    let p = cross(d, t.e2);
                    let det = dot(t.e1, p);
                    if det.abs() < 1e-12 {
                        continue;
                    }
                    let id = 1.0 / det;
                    let s = sub(o, t.v0);
                    let u = dot(s, p) * id;
                    if !(0.0..=1.0).contains(&u) {
                        continue;
                    }
                    let q = cross(s, t.e1);
                    let v = dot(d, q) * id;
                    if v < 0.0 || u + v > 1.0 {
                        continue;
                    }
                    let tt = dot(t.e2, q) * id;
                    if tt > tmin && tt < best_t {
                        best_t = tt;
                        best = Some((ti, det < 0.0));
                    }
                }
            } else {
                let l = n.first as usize;
                let r = l + 1;
                let tl = self.slab(&self.nodes[l], o, inv, tmin, best_t);
                let tr = self.slab(&self.nodes[r], o, inv, tmin, best_t);
                match (tl, tr) {
                    (Some(a), Some(b)) => {
                        let (near, far) = if a <= b { (l, r) } else { (r, l) };
                        if sp < stack.len() {
                            stack[sp] = far as u32;
                            sp += 1;
                        }
                        ni = near;
                        continue;
                    }
                    (Some(_), None) => {
                        ni = l;
                        continue;
                    }
                    (None, Some(_)) => {
                        ni = r;
                        continue;
                    }
                    (None, None) => {}
                }
            }
            if sp == 0 {
                break;
            }
            sp -= 1;
            ni = stack[sp] as usize;
        }
        best.map(|(ti, back)| {
            let t = &self.tris[ti];
            let n = cross(t.e1, t.e2);
            let l = dot(n, n).sqrt().max(1e-20);
            RayHit { t: best_t, payload: self.payload[ti], back, normal: [n[0] / l, n[1] / l, n[2] / l] }
        })
    }

    /// Entry distance of the ray into a node's box, if it enters before `tmax`.
    #[inline]
    fn slab(&self, n: &Node, o: [f32; 3], inv: [f32; 3], tmin: f32, tmax: f32) -> Option<f32> {
        let mut t0 = tmin;
        let mut t1 = tmax;
        for a in 0..3 {
            let mut ta = (n.lo[a] - o[a]) * inv[a];
            let mut tb = (n.hi[a] - o[a]) * inv[a];
            if ta > tb {
                std::mem::swap(&mut ta, &mut tb);
            }
            // NaN (0 * inf) keeps the old bound
            if ta > t0 {
                t0 = ta;
            }
            if tb < t1 {
                t1 = tb;
            }
            if t0 > t1 {
                return None;
            }
        }
        Some(t0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(y: f32, s: f32) -> Vec<[[f32; 3]; 3]> {
        // counter-clockwise seen from above (+Y): front faces up
        vec![[[-s, y, -s], [-s, y, s], [s, y, s]], [[-s, y, -s], [s, y, s], [s, y, -s]]]
    }

    #[test]
    fn hits_closest_and_back() {
        let mut t = quad(0.0, 10.0);
        t.extend(quad(5.0, 10.0));
        let n = t.len();
        let b = Bvh::build(t, (0..n as u32).collect());
        let h = b.hit([0.0, 10.0, 0.0], [0.0, -1.0, 0.0], 0.0, 100.0).unwrap();
        assert!((h.t - 5.0).abs() < 1e-4 && h.payload >= 2 && !h.back, "{h:?}");
        assert!(h.normal[1] > 0.99);
        let h = b.hit([0.0, 2.0, 0.0], [0.0, 1.0, 0.0], 0.0, 100.0).unwrap();
        assert!((h.t - 3.0).abs() < 1e-4 && h.back);
        assert!(b.hit([0.0, 2.0, 0.0], [0.0, 1.0, 0.0], 0.0, 2.0).is_none());
        assert!(b.hit([50.0, 2.0, 0.0], [0.0, 1.0, 0.0], 0.0, 100.0).is_none());
    }

    #[test]
    fn many_triangles() {
        // a grid of small quads; every downward ray hits the one under it
        let mut t = Vec::new();
        for i in 0..60 {
            for j in 0..60 {
                let (x, z) = (i as f32, j as f32);
                t.push([[x, 0.0, z], [x, 0.0, z + 1.0], [x + 1.0, 0.0, z + 1.0]]);
                t.push([[x, 0.0, z], [x + 1.0, 0.0, z + 1.0], [x + 1.0, 0.0, z]]);
            }
        }
        let n = t.len();
        let b = Bvh::build(t, (0..n as u32).collect());
        for k in 0..200 {
            let x = (k * 37 % 600) as f32 / 10.0 + 0.03;
            let z = (k * 91 % 600) as f32 / 10.0 + 0.07;
            let h = b.hit([x, 3.0, z], [0.0, -1.0, 0.0], 0.0, 10.0).unwrap();
            assert!((h.t - 3.0).abs() < 1e-4);
            let cell = (x.floor() as u32 * 60 + z.floor() as u32) * 2;
            assert!(h.payload == cell || h.payload == cell + 1, "{x} {z} {}", h.payload);
        }
    }
}
