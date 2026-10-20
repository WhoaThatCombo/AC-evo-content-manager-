//! Pit lane: the AC1 pit lane trimmed to where it leaves / rejoins the
//! ideal line, and `containers\pitlane_zones_<L>.scene`.
//!
//! # Format (stock, the 22 pitlane_zones containers of 17 tracks)
//!
//! Zone entities (f50.f112 ZoneActorData {f30 debug {f1 0.05, f3 3},
//! f1000 ZoneData {f1 one ZoneGeometricData {f1 corner transforms, f21
//! colour (empty), f22 render height 0.8}, f2 instances}}), four per
//! layout:
//! - `pitlane_entry`: instance f101 Pitlane {} (type Entry = 0);
//! - `pitlane_main`: f101 {f1 3} (Main) and f111 SpeedLimit {f1 command
//!   "pitlane"} - the limiter;
//! - `pitlane_exit`: f101 {f1 1} (Exit);
//! - `safety_car_line_2`: f114 SafetyCarLine {f1 2}, a 4-corner quad.
//! Polygons have 4..31 corners. The Container entity declares `Zone`,
//! `pitlane`, `safety_car_line`, `speed_limit`; the rows list the
//! container with the layout in Race, Time Attack and Hotstint. No stock
//! track uses Pitbox zones: pit stops use the Start Pos slots (type 4 in
//! the lane, type 5 boxes; stock monza: slots 8 m off the pit spline).
//! Stock pit splines start and end within 0..6 m of the ideal line, with
//! 0..290 m running along it.
//!
//! # Ours
//!
//! - Trim: the AC1 pit lane is cut 30 m before it first leaves the ideal
//!   line (more than 2.5 m away) and 30 m after it last rejoins it.
//! - Main zone: the pit-lane points on AC1 IS_PITLANE surfaces (first to
//!   last); without such surfaces, from 20 m after the lane is 6 m off the
//!   ideal line to 20 m before it comes back. Entry = spline start to the
//!   main zone, exit = main zone to the spline end.
//! - Width: each side reaches to the wall the pit spline measured, 0.2 m
//!   short, clamped to 2.5..7 m.
//! - Polygons: both edges sampled every 5 m, Douglas-Peucker at 0.5 m,
//!   at most 24 corners.
//! - Safety-car line 2: across the ideal line where the pit lane rejoins
//!   it, the ideal line's kerb width + 2 m each side, 1 m deep.

use super::ai::{self, Line};
use super::classes::Kind;
use super::scene::{transform, Entity, SceneBuilder};
use crate::io::pb::{Msg, Val};

const LEAVE: f32 = 2.5;
const KEEP: f32 = 30.0;
const MAX_CORNERS: usize = 24;

/// Cut the AC1 pit lane to its part off the ideal line (+ 30 m each end).
pub fn trim(pit: &[[f32; 3]], ideal: &Line) -> Vec<[f32; 3]> {
    if pit.len() < 4 || ideal.samples.is_empty() {
        return pit.to_vec();
    }
    let d: Vec<f32> = pit.iter().map(|p| ideal.nearest(*p).1).collect();
    let Some(first) = d.iter().position(|x| *x > LEAVE) else { return pit.to_vec() };
    let last = d.iter().rposition(|x| *x > LEAVE).unwrap();
    let mut cum = vec![0.0f32; pit.len()];
    for i in 1..pit.len() {
        cum[i] = cum[i - 1] + ai::dist3(pit[i - 1], pit[i]);
    }
    let s = (0..=first).rev().find(|&i| cum[first] - cum[i] >= KEEP).unwrap_or(0);
    let e = (last..pit.len()).find(|&i| cum[i] - cum[last] >= KEEP).unwrap_or(pit.len() - 1);
    pit[s..=e].to_vec()
}

#[derive(Clone, Debug, Default)]
pub struct Zones {
    /// pit sample index ranges (inclusive)
    pub entry: (usize, usize),
    pub main: (usize, usize),
    pub exit: (usize, usize),
    pub polygons: Vec<(String, Vec<[f32; 3]>)>,
    pub sc_line: Option<Vec<[f32; 3]>>,
    pub main_from_surfaces: bool,
    pub main_length: f32,
}

fn dp(pts: &[[f32; 3]], tol: f32) -> Vec<[f32; 3]> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let mut keep = vec![false; pts.len()];
    keep[0] = true;
    keep[pts.len() - 1] = true;
    let mut stack = vec![(0usize, pts.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        let (pa, pb) = (pts[a], pts[b]);
        let (dx, dz) = (pb[0] - pa[0], pb[2] - pa[2]);
        let l = (dx * dx + dz * dz).sqrt().max(1e-6);
        let mut best = (0usize, 0.0f32);
        for i in a + 1..b {
            let d = ((pts[i][0] - pa[0]) * dz - (pts[i][2] - pa[2]) * dx).abs() / l;
            if d > best.1 {
                best = (i, d);
            }
        }
        if best.1 > tol {
            keep[best.0] = true;
            stack.push((a, best.0));
            stack.push((best.0, b));
        }
    }
    pts.iter().zip(&keep).filter(|(_, k)| **k).map(|(p, _)| *p).collect()
}

fn cap(v: Vec<[f32; 3]>, n: usize) -> Vec<[f32; 3]> {
    if v.len() <= n || n < 2 {
        return v;
    }
    (0..n).map(|k| v[k * (v.len() - 1) / (n - 1)]).collect()
}

/// A strip polygon over samples a..=b of `line` (left edge forward, right
/// edge back).
pub fn strip(line: &Line, a: usize, b: usize, width: &dyn Fn(&ai::Sample) -> [f32; 2], every: f32) -> Vec<[f32; 3]> {
    let (mut l, mut r) = (Vec::new(), Vec::new());
    let mut last = f32::NEG_INFINITY;
    for i in a..=b {
        let s = &line.samples[i];
        if s.dist - last < every && i != b {
            continue;
        }
        last = s.dist;
        let x = ai::left_of(s.fwd);
        let w = width(s);
        l.push([s.pos[0] + x[0] * w[0], s.pos[1], s.pos[2] + x[2] * w[0]]);
        r.push([s.pos[0] - x[0] * w[1], s.pos[1], s.pos[2] - x[2] * w[1]]);
    }
    let l = dp(&l, 0.5);
    let mut r = dp(&r, 0.5);
    let half = MAX_CORNERS / 2;
    let mut out = cap(l, half);
    r.reverse();
    out.extend(cap(r, half));
    out
}

/// Mark the pit samples of the main zone and lay out the polygons.
pub fn zones(pit: &mut Line, ideal: &Line, kinds: &[Kind]) -> Zones {
    let n = pit.samples.len();
    let mut z = Zones::default();
    if n < 4 {
        return z;
    }
    let on_pit: Vec<bool> = pit.samples.iter().map(|s| s.surface.map(|k| kinds.get(k) == Some(&Kind::Pit)).unwrap_or(false)).collect();
    let pits = on_pit.iter().filter(|x| **x).count();
    let (ms, me) = if pits >= 5 {
        z.main_from_surfaces = true;
        (on_pit.iter().position(|x| *x).unwrap(), on_pit.iter().rposition(|x| *x).unwrap())
    } else {
        let off: Vec<bool> = pit.samples.iter().map(|s| ideal.nearest(s.pos).1 > 6.0).collect();
        match (off.iter().position(|x| *x), off.iter().rposition(|x| *x)) {
            (Some(a), Some(b)) => {
                let da = pit.samples[a].dist + 20.0;
                let db = pit.samples[b].dist - 20.0;
                let s = pit.samples.iter().position(|x| x.dist >= da).unwrap_or(a);
                let e = pit.samples.iter().rposition(|x| x.dist <= db).unwrap_or(b);
                if s < e { (s, e) } else { (a, b.max(a + 1).min(n - 1)) }
            }
            _ => (n / 4, 3 * n / 4),
        }
    };
    let ms = ms.clamp(1, n - 3);
    let me = me.clamp(ms + 1, n - 2);
    z.entry = (0, ms);
    z.main = (ms, me);
    z.exit = (me, n - 1);
    z.main_length = pit.samples[me].dist - pit.samples[ms].dist;
    for s in &mut pit.samples[ms..=me] {
        s.pit = true;
    }
    let width = |s: &ai::Sample| [(s.walls[0] - 0.2).clamp(2.5, 7.0), (s.walls[1] - 0.2).clamp(2.5, 7.0)];
    for (name, (a, b)) in [("pitlane_entry", z.entry), ("pitlane_main", z.main), ("pitlane_exit", z.exit)] {
        z.polygons.push((name.to_string(), strip(pit, a, b, &width, 5.0)));
    }
    // safety-car line 2 where the pit lane rejoins the ideal line
    let (k, _) = ideal.nearest(pit.samples[n - 1].pos);
    let s = &ideal.samples[k];
    let x = ai::left_of(s.fwd);
    let f = [s.fwd[0], 0.0, s.fwd[2]];
    let fl = (f[0] * f[0] + f[2] * f[2]).sqrt().max(1e-6);
    let f = [f[0] / fl, 0.0, f[2] / fl];
    let (wl, wr) = (s.curbs[0] + 2.0, s.curbs[1] + 2.0);
    let c = |w: f32, d: f32| [s.pos[0] + x[0] * w + f[0] * d, s.pos[1], s.pos[2] + x[2] * w + f[2] * d];
    z.sc_line = Some(vec![c(wl, -0.5), c(wl, 0.5), c(-wr, 0.5), c(-wr, -0.5)]);
    z
}

/// ZoneActorData with one polygon and the given instances.
pub fn zone_entity(name: &str, polys: &[Vec<[f32; 3]>], instances: Vec<Msg>) -> Entity {
    let mut zd = Msg::default();
    let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
    for poly in polys {
        let mut geo = Msg::default();
        for p in poly {
            geo.push(1, Val::Msg(transform(*p, [0.0; 3], [1.0; 3])));
            lo = [lo[0].min(p[0]), lo[1].min(p[2])];
            hi = [hi[0].max(p[0]), hi[1].max(p[2])];
        }
        geo.push(21, Val::Msg(Msg::default()));
        geo.set_f32(22, 0.8);
        zd.push(1, Val::Msg(geo));
    }
    for i in instances {
        zd.push(2, Val::Msg(i));
    }
    let mut za = Msg::default();
    let mut dbg = Msg::default();
    dbg.set_f32(1, 0.05);
    dbg.set_varint(3, 3);
    za.push(30, Val::Msg(dbg));
    za.push(1000, Val::Msg(zd));
    let mut e = Entity::new(name, "Zone", 112, za);
    if lo[0].is_finite() {
        e.bounds = Some((lo, hi));
    }
    e
}

fn instance(field: u32, body: Msg) -> Msg {
    let mut m = Msg::default();
    m.push(field, Val::Msg(body));
    m
}

/// Write `pitlane_zones_<stem>`; returns the container stem.
pub fn write(b: &mut SceneBuilder, stem: &str, z: &Zones) -> Option<String> {
    if z.polygons.is_empty() {
        return None;
    }
    let c = format!("pitlane_zones_{stem}");
    for (name, poly) in &z.polygons {
        let mut pl = Msg::default();
        let inst = match name.as_str() {
            "pitlane_main" => {
                pl.set_varint(1, 3);
                let mut sl = Msg::default();
                sl.set_str(1, "pitlane");
                vec![instance(101, pl), instance(111, sl)]
            }
            "pitlane_exit" => {
                pl.set_varint(1, 1);
                vec![instance(101, pl)]
            }
            _ => vec![instance(101, pl)],
        };
        b.add(Some(&c), zone_entity(name, std::slice::from_ref(poly), inst));
    }
    if let Some(sc) = &z.sc_line {
        let mut l = Msg::default();
        l.set_varint(1, 2);
        b.add(Some(&c), zone_entity("safety_car_line_2", std::slice::from_ref(sc), vec![instance(114, l)]));
    }
    for t in ["pitlane", "safety_car_line", "speed_limit"] {
        b.declare(&c, t);
    }
    Some(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simplify_and_cap() {
        let pts: Vec<[f32; 3]> = (0..100).map(|i| [i as f32, 0.0, 0.0]).collect();
        assert_eq!(dp(&pts, 0.5).len(), 2);
        let wig: Vec<[f32; 3]> = (0..100).map(|i| [i as f32, 0.0, if i % 2 == 0 { 0.0 } else { 3.0 }]).collect();
        assert_eq!(cap(dp(&wig, 0.5), 12).len(), 12);
    }

    #[test]
    fn trims_to_the_lane() {
        // ideal line along x at z = 0; pit lane on the line, then 12 m off, then back
        let ideal_pts: Vec<[f32; 3]> = (0..400).map(|i| [i as f32 * 2.0, 0.0, 0.0]).collect();
        let g = crate::track::ground::Ground::from_triangles(vec![
            crate::track::ground::Tri { v: [[-10.0, 0.0, -50.0], [900.0, 0.0, -50.0], [900.0, 0.0, 50.0]], surface: 0 },
            crate::track::ground::Tri { v: [[-10.0, 0.0, -50.0], [900.0, 0.0, 50.0], [-10.0, 0.0, 50.0]], surface: 0 },
        ], vec![true]);
        let ideal = ai::build(&ideal_pts, false, 2.0, &g, &[Kind::Road], false, None);
        let pit: Vec<[f32; 3]> = (0..400).map(|i| {
            let x = i as f32 * 2.0;
            let z = if (200.0..600.0).contains(&x) { 12.0 } else { 0.0 };
            [x, 0.0, z]
        }).collect();
        let t = trim(&pit, &ideal);
        assert!((160.0..200.0).contains(&t[0][0]) && (600.0..640.0).contains(&t[t.len() - 1][0]), "{:?} {:?}", t[0], t[t.len() - 1]);
        let mut pl = ai::build(&t, false, ai::PIT_STEP, &g, &[Kind::Road], true, None);
        let z = zones(&mut pl, &ideal, &[Kind::Road]);
        assert_eq!(z.polygons.len(), 3);
        assert!(z.polygons.iter().all(|(_, p)| p.len() >= 4 && p.len() <= MAX_CORNERS));
        assert!(pl.samples.iter().filter(|s| s.pit).count() > 50);
        let mut b = SceneBuilder::new("t");
        assert_eq!(write(&mut b, "x", &z).as_deref(), Some("pitlane_zones_x"));
        assert_eq!(b.containers["pitlane_zones_x"].scene.entities.len(), 4);
    }
}
