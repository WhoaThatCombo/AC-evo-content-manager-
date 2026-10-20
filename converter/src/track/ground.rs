//! Ground queries over the AC1 physics meshes: "what is the drivable
//! surface at (x, z) near height y, and which surface is it".
//!
//! Index (ours): a uniform xz grid of cells; each cell lists the triangles
//! whose xz box touches it (CSR layout: one offset array, one id array).
//! The cell size follows the mean triangle size, clamped to 2..16 m, so a
//! query visits a handful of triangles; building is O(triangles), a query
//! O(triangles in one cell). Millions of queries per second per thread on
//! the big maps.
//!
//! Built from every physics mesh of a layout, invisible ones included
//! (`Ground::build`), or from any triangle soup (`Ground::from_triangles`,
//! for tests and other stages).

use super::scan::{Role, Scan};
use super::Hit;

#[derive(Clone, Copy, Debug)]
pub struct Tri {
    pub v: [[f32; 3]; 3],
    /// index into the surface list the ground was built with
    pub surface: u16,
}

pub struct Ground {
    tris: Vec<Tri>,
    /// per surface: drivable (track, kerb, verge, pit) as classified
    drivable: Vec<bool>,
    origin: [f32; 2],
    cell: f32,
    nx: usize,
    nz: usize,
    start: Vec<u32>,
    ids: Vec<u32>,
    lo: [f32; 3],
    hi: [f32; 3],
}

impl Ground {
    /// Every physics triangle of layout `layout` (an index into
    /// `scan.layouts`). `drivable[s]` says whether surface `s` of
    /// `scan.surfaces` counts as drivable (see `classes::surface_kind`).
    pub fn build(scan: &Scan, layout: usize, drivable: Vec<bool>) -> Ground {
        let mut tris = Vec::new();
        for m in scan.meshes.iter().filter(|m| m.layouts & (1 << layout) != 0) {
            let Role::Physics { surface, .. } = m.role else { continue };
            let p = scan.positions(m);
            for t in scan.triangles(m) {
                tris.push(Tri { v: [p[t[0] as usize], p[t[1] as usize], p[t[2] as usize]], surface: surface as u16 });
            }
        }
        Ground::from_triangles(tris, drivable)
    }

    pub fn from_triangles(mut tris: Vec<Tri>, drivable: Vec<bool>) -> Ground {
        tris.retain(|t| t.v.iter().all(|p| p.iter().all(|c| c.is_finite())));
        let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        let mut area = 0f64;
        for t in &tris {
            for p in &t.v {
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
            let (ex, ez) = (t.v.iter().map(|p| p[0]).fold(f32::MIN, f32::max) - t.v.iter().map(|p| p[0]).fold(f32::MAX, f32::min),
                t.v.iter().map(|p| p[2]).fold(f32::MIN, f32::max) - t.v.iter().map(|p| p[2]).fold(f32::MAX, f32::min));
            area += (ex.max(0.01) * ez.max(0.01)) as f64;
        }
        if tris.is_empty() {
            return Ground { tris, drivable, origin: [0.0; 2], cell: 1.0, nx: 0, nz: 0, start: vec![0], ids: Vec::new(), lo: [0.0; 3], hi: [0.0; 3] };
        }
        let mean = (area / tris.len() as f64).sqrt() as f32;
        let mut cell = (mean * 2.0).clamp(2.0, 16.0);
        // keep the grid itself bounded (a 60 km map at 2 m would be 900M cells)
        while ((hi[0] - lo[0]) / cell) as f64 * ((hi[2] - lo[2]) / cell) as f64 > 16.0e6 {
            cell *= 1.5;
        }
        let origin = [lo[0], lo[2]];
        let nx = (((hi[0] - lo[0]) / cell) as usize + 1).max(1);
        let nz = (((hi[2] - lo[2]) / cell) as usize + 1).max(1);
        let span = |t: &Tri| {
            let (mut a, mut b) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
            for p in &t.v {
                a[0] = a[0].min(p[0]);
                a[1] = a[1].min(p[2]);
                b[0] = b[0].max(p[0]);
                b[1] = b[1].max(p[2]);
            }
            let c = |v: f32, o: f32, n: usize| (((v - o) / cell) as isize).clamp(0, n as isize - 1) as usize;
            (c(a[0], origin[0], nx), c(b[0], origin[0], nx), c(a[1], origin[1], nz), c(b[1], origin[1], nz))
        };
        let mut count = vec![0u32; nx * nz + 1];
        for t in &tris {
            let (x0, x1, z0, z1) = span(t);
            for z in z0..=z1 {
                for x in x0..=x1 {
                    count[z * nx + x + 1] += 1;
                }
            }
        }
        for i in 1..count.len() {
            count[i] += count[i - 1];
        }
        let start = count.clone();
        let mut fill = count;
        let mut ids = vec![0u32; *start.last().unwrap() as usize];
        for (ti, t) in tris.iter().enumerate() {
            let (x0, x1, z0, z1) = span(t);
            for z in z0..=z1 {
                for x in x0..=x1 {
                    let c = z * nx + x;
                    ids[fill[c] as usize] = ti as u32;
                    fill[c] += 1;
                }
            }
        }
        Ground { tris, drivable, origin, cell, nx, nz, start, ids, lo, hi }
    }

    pub fn is_empty(&self) -> bool {
        self.tris.is_empty()
    }

    pub fn triangle_count(&self) -> usize {
        self.tris.len()
    }

    fn cell_of(&self, x: f32, z: f32) -> Option<usize> {
        if self.nx == 0 {
            return None;
        }
        let cx = ((x - self.origin[0]) / self.cell).floor();
        let cz = ((z - self.origin[1]) / self.cell).floor();
        if cx < 0.0 || cz < 0.0 || cx as usize >= self.nx || cz as usize >= self.nz {
            return None;
        }
        Some(cz as usize * self.nx + cx as usize)
    }

    /// Height of triangle `t` at (x, z), with its unit normal (pointing up).
    fn height(t: &Tri, x: f32, z: f32) -> Option<(f32, [f32; 3])> {
        let [a, b, c] = t.v;
        let d = (b[2] - c[2]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[2] - c[2]);
        if d.abs() < 1e-12 {
            return None; // vertical in xz
        }
        let l1 = ((b[2] - c[2]) * (x - c[0]) + (c[0] - b[0]) * (z - c[2])) / d;
        let l2 = ((c[2] - a[2]) * (x - c[0]) + (a[0] - c[0]) * (z - c[2])) / d;
        let l3 = 1.0 - l1 - l2;
        const E: f32 = -1e-5;
        if l1 < E || l2 < E || l3 < E {
            return None;
        }
        let y = l1 * a[1] + l2 * b[1] + l3 * c[1];
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let mut n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        if n[1] < 0.0 {
            n = [-n[0], -n[1], -n[2]];
        }
        Some((y, super::scan::norm3(n)))
    }

    fn hit_of(&self, t: &Tri, y: f32, n: [f32; 3]) -> Hit {
        Hit { y, surface: t.surface as usize, drivable: self.drivable.get(t.surface as usize).copied().unwrap_or(false), normal: n }
    }

    /// Every surface at (x, z), highest first.
    pub fn hits(&self, x: f32, z: f32) -> Vec<Hit> {
        let Some(c) = self.cell_of(x, z) else { return Vec::new() };
        let mut out: Vec<Hit> = self.ids[self.start[c] as usize..self.start[c + 1] as usize].iter()
            .filter_map(|&i| { let t = &self.tris[i as usize]; Ground::height(t, x, z).map(|(y, n)| self.hit_of(t, y, n)) })
            .collect();
        out.sort_by(|a, b| b.y.partial_cmp(&a.y).unwrap());
        out
    }

    /// The highest surface at or below `y_from` (within 1 mm).
    pub fn below(&self, x: f32, y_from: f32, z: f32) -> Option<Hit> {
        let Some(c) = self.cell_of(x, z) else { return None };
        let mut best: Option<Hit> = None;
        for &i in &self.ids[self.start[c] as usize..self.start[c + 1] as usize] {
            let t = &self.tris[i as usize];
            if let Some((y, n)) = Ground::height(t, x, z) {
                if y <= y_from + 0.001 && best.map(|b| y > b.y).unwrap_or(true) {
                    best = Some(self.hit_of(t, y, n));
                }
            }
        }
        best
    }

    /// The surface at (x, z) closest in height to `y` (above or below).
    pub fn nearest(&self, x: f32, y: f32, z: f32) -> Option<Hit> {
        let Some(c) = self.cell_of(x, z) else { return None };
        let mut best: Option<Hit> = None;
        for &i in &self.ids[self.start[c] as usize..self.start[c + 1] as usize] {
            let t = &self.tris[i as usize];
            if let Some((h, n)) = Ground::height(t, x, z) {
                if best.map(|b| (h - y).abs() < (b.y - y).abs()).unwrap_or(true) {
                    best = Some(self.hit_of(t, h, n));
                }
            }
        }
        best
    }

    /// Like `nearest`, preferring drivable surfaces within `reach` metres
    /// of `y` over anything else (spawn snapping: a car above a road under
    /// a bridge).
    pub fn nearest_drivable(&self, x: f32, y: f32, z: f32, reach: f32) -> Option<Hit> {
        let hs = self.hits(x, z);
        hs.iter().filter(|h| h.drivable && (h.y - y).abs() <= reach).min_by(|a, b| (a.y - y).abs().partial_cmp(&(b.y - y).abs()).unwrap()).copied()
            .or_else(|| self.nearest(x, y, z))
    }

    /// The surface index at a point (the nearest surface in height).
    pub fn surface_at(&self, x: f32, y: f32, z: f32) -> Option<usize> {
        self.nearest(x, y, z).map(|h| h.surface)
    }

    /// Every triangle whose surface is in `surfaces`.
    pub fn triangles_of<'a>(&'a self, surfaces: &'a [usize]) -> impl Iterator<Item = &'a Tri> + 'a {
        self.tris.iter().filter(move |t| surfaces.contains(&(t.surface as usize)))
    }

    /// Every triangle.
    pub fn triangles(&self) -> &[Tri] {
        &self.tris
    }

    /// World bounds of all physics triangles (min, max).
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        (self.lo, self.hi)
    }

    /// World xz bounds of the drivable triangles: (min x, min z), (max x, max z).
    pub fn drivable_bounds(&self) -> ([f32; 2], [f32; 2]) {
        let (mut a, mut b) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
        for t in self.tris.iter().filter(|t| self.drivable.get(t.surface as usize).copied().unwrap_or(false)) {
            for p in &t.v {
                a[0] = a[0].min(p[0]);
                a[1] = a[1].min(p[2]);
                b[0] = b[0].max(p[0]);
                b[1] = b[1].max(p[2]);
            }
        }
        if !a[0].is_finite() {
            return ([self.lo[0], self.lo[2]], [self.hi[0], self.hi[2]]);
        }
        (a, b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(y: f32, s: u16) -> Vec<Tri> {
        vec![
            Tri { v: [[0.0, y, 0.0], [10.0, y, 0.0], [10.0, y, 10.0]], surface: s },
            Tri { v: [[0.0, y, 0.0], [10.0, y, 10.0], [0.0, y, 10.0]], surface: s },
        ]
    }

    #[test]
    fn synthetic_quads() {
        // a road at y = 1 under a bridge at y = 6, and a sloped wall-free ramp
        let mut t = quad(1.0, 0);
        t.extend(quad(6.0, 1));
        let g = Ground::from_triangles(t, vec![true, false]);
        let h = g.below(5.0, 10.0, 5.0).unwrap();
        assert_eq!((h.y, h.surface), (6.0, 1));
        let h = g.below(5.0, 5.0, 5.0).unwrap();
        assert_eq!((h.y, h.surface, h.drivable), (1.0, 0, true));
        assert!(g.below(5.0, 0.5, 5.0).is_none());
        assert_eq!(g.nearest(5.0, 4.5, 5.0).unwrap().y, 6.0);
        assert_eq!(g.nearest_drivable(5.0, 4.5, 5.0, 4.0).unwrap().y, 1.0);
        assert_eq!(g.hits(2.0, 7.0).len(), 2);
        assert!(g.below(-1.0, 10.0, 5.0).is_none());
        assert_eq!(g.surface_at(9.9, 1.2, 0.1), Some(0));
        assert_eq!(g.drivable_bounds(), ([0.0, 0.0], [10.0, 10.0]));
        assert_eq!(g.triangles_of(&[1]).count(), 2);
        // a slope: y = x / 10
        let s = vec![Tri { v: [[0.0, 0.0, 0.0], [10.0, 1.0, 0.0], [0.0, 0.0, 10.0]], surface: 0 }];
        let g = Ground::from_triangles(s, vec![true]);
        let h = g.below(5.0, 100.0, 2.0).unwrap();
        assert!((h.y - 0.5).abs() < 1e-5 && h.normal[1] > 0.99 && h.normal[0] < 0.0);
    }
}
