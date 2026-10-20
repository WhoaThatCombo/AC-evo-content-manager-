//! AI splines: `layouts\layout_<L>.ideal_line.aisplinedata`,
//! `layouts\layout_<L>.pitlane.aisplinedata` and
//! `layouts\layout_<L>.trackcontrolpoints`.
//!
//! # Format (game facts: ACECM `protos\AIData.proto.txt`, `Math.proto.txt`;
//! read off the 34 stock ideal / pit lines, road_atlanta left out)
//!
//! AISplineData:
//! - f1 InterpolatingSplineData {f1 points {f1 position, f2 cumulative
//!   length (m), f3 tag = point index}, f2 length (double, closing segment
//!   included), f3 is_closed, f5 mode (5 on stock ideal lines, 2 =
//!   Catmull-Rom on every pit lane and on cota club's ideal line), f6 grid}.
//! - f6 grid: f1 max / f2 min xz extremes = the points' xz box widened by
//!   350 m, f3 neighbours = 10, f4 sampling density 10.0 (double), f5 one
//!   row per 10 m along x, each with one cell per 10 m along z (floor of
//!   the extent / 10); a cell lists the 10 points nearest its centre in
//!   xz, nearest first (checked on brands_hatch: 117 / 117 sampled cells
//!   identical). Optional: cota club's ideal line has none.
//! - f2 one payload per point: f1 speed (13.8 everywhere: the game does not
//!   read it), f2 |radius| (m), f3 / f4 distance to the left / right white
//!   line (negative when the line is outside it), f5 / f6 = -1, f7 camber,
//!   f8 turn direction (+1 / -1), f9 surface normal, f10 forward, f11 the
//!   length of the 5-segment window the radius is taken over, f12 0.78,
//!   f14 grade, f15 grip (1.0), f16 / f17 distance (m, integers) from the
//!   last corner (capped 500) / to the next one, absent inside corners,
//!   f18 is_pitlane, f19 compression, f22 / f23 distance to the left /
//!   right wall (capped 80), f24 / f25 sides with kerbs (at least the half
//!   car width), f26 radius change rate, f27 / f28 = f3 / f4. Pit lanes
//!   carry no sides.
//! - f3 straight factor -1; f5 the layout container path without
//!   `.scene` (ideal line only); f6 recorded half car width (about 1.055;
//!   -1 on pit lanes); f7 TrackControlPoints (a copy of the
//!   `.trackcontrolpoints` file; empty on pit lanes); f8 SceneTrackInfo.
//! - Left = up x forward (local +X): sides_0 is the side that direction
//!   points to (checked against the road edges of stock monza's dynamic
//!   track grid).
//! - Stock lines lie on the road: the dynamic-track road heights under
//!   brands_hatch's and monza's ideal and pit lines are within 0.02 m of
//!   the line (p1..p99). Stock ideal lines have 1.6..1.8 m per point
//!   (median; nordschleife 5.4), pit lanes 2.2.
//!
//! TrackControlPoints {f1 repeated {f1 position, f2 forward, f3 / f5 side
//! left / right, f4 / f6 the same with kerbs, f11 / f12 walls, f7 signed
//! radius (clamped to +-10000), f8 push 1, f9 distance from the previous
//! point, f10 normalised spline position, f13 lookahead 1, f14 packed tags
//! (TrackControlPointTags: 0 long straight, 1 straight, 2 brake zone, 3
//! entry, 4 apex, 5 slowest point, 6 exit, 10 accel zone)}}. Stock: one
//! point every 60..110 m, a corner reads 2, 3, 4+5, 6+10.
//!
//! Reference laps (`.carcategory_<c>.reference`) exist on 4 of 17 stock
//! tracks only; the game simulates one from the ideal line otherwise, so
//! none are written.
//!
//! # Ours
//!
//! - The AC1 fast lane is resampled every 1.7 m (pit lane 2.21 m), smoothed
//!   lightly in xz (3 passes of a quarter-weight Laplacian) and put on the
//!   physics ground: the drivable surface nearest to the AC1 height minus
//!   the line's median height above the ground (Kunos lines ride 0.30 m
//!   above it), searched within 1.5 m.
//! - Sides / kerbs / walls: the ground is walked across the line in 0.25 m
//!   steps (0.5 m past 20 m; edges placed to 5 cm), sides capped at 15 m.
//!   Road / pit / concrete is inside the white
//!   lines, kerbs extend the kerb sides, anything else drivable is outside;
//!   a wall is a wall surface or a steep triangle 0.15..3 m above the
//!   running ground height (low walls count), a step up of more than 0.5 m,
//!   or the end of the ground.
//! - Speeds: the AC1 per-point speeds when the file has them and their
//!   lap is within 0.6..1.6 x the model's, else a
//!   point-mass lap: corner speed sqrt(1.5 g R), acceleration 7 m/s^2
//!   falling with (v / 80 m/s)^2, braking 11 m/s^2. They place the brake
//!   zones of the control points and give the report's lap time.
//! - Corners: |R| < 250 m over the smoothed radius, gaps under 30 m merged,
//!   corners under 10 m dropped.
//! - Control points: per corner a brake zone (when braking starts more
//!   than 30 m before it), the entry, the apex + slowest point, the exit +
//!   accel zone; a straight of 100 m or more gets a straight point 30 m
//!   past the exit (+ long straight from 300 m) and one every 300 m
//!   (stock: 9..16 points per km).

use anyhow::{bail, Context, Result};

use super::classes::Kind;
use super::ground::Ground;
use super::scene::v3;
use crate::io::pb::{self, Msg, Val};

pub const IDEAL_STEP: f32 = 1.7;
pub const PIT_STEP: f32 = 2.21;
/// stock recorded_half_car_width (brands_hatch, fuji, nurburgring)
pub const HALF_CAR_WIDTH: f32 = 1.055;
pub const WALL_CAP: f32 = 80.0;
/// ours: stock sides stay under 14 m; wider tarmac (pit entries, run-off)
/// is not track for the AI
pub const SIDE_CAP: f32 = 15.0;
const RADIUS_CAP: f32 = 2.5e6;
pub const CORNER_R: f32 = 250.0;
const SNAP_REACH: f32 = 1.5;

#[derive(Clone, Copy, Debug, Default)]
pub struct Sample {
    pub pos: [f32; 3],
    /// distance from the first point (m)
    pub dist: f32,
    pub fwd: [f32; 3],
    pub normal: [f32; 3],
    /// signed radius: positive when the turn direction is +1
    pub radius: f32,
    /// left, right (left = up x forward)
    pub sides: [f32; 2],
    pub curbs: [f32; 2],
    pub walls: [f32; 2],
    pub camber: f32,
    pub grade: f32,
    pub compression: f32,
    /// m/s (AC1 data or the lap model)
    pub speed: f32,
    pub pit: bool,
    pub snapped: bool,
    /// scan surface under the point
    pub surface: Option<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct Line {
    pub samples: Vec<Sample>,
    pub closed: bool,
    /// closing segment included when closed
    pub length: f32,
    /// the AC1 line's median height above the ground
    pub offset: f32,
    /// share of points put on the ground
    pub snapped: f32,
    /// speeds came from the AC1 file
    pub ac1_speeds: bool,
}

impl Line {
    pub fn lap_time(&self) -> f32 {
        let n = self.samples.len();
        let mut t = 0.0;
        for i in 0..n {
            let j = if i + 1 < n { i + 1 } else if self.closed { 0 } else { break };
            let ds = dist3(self.samples[i].pos, self.samples[j].pos);
            t += ds / ((self.samples[i].speed + self.samples[j].speed) * 0.5).max(1.0);
        }
        t
    }
    /// The sample nearest to distance `d` along the line (wrapped when closed).
    pub fn at(&self, d: f32) -> &Sample {
        let d = if self.closed && self.length > 0.0 { d.rem_euclid(self.length) } else { d.clamp(0.0, self.length) };
        let i = self.samples.partition_point(|s| s.dist < d).min(self.samples.len() - 1);
        if i > 0 && (d - self.samples[i - 1].dist).abs() < (self.samples[i].dist - d).abs() {
            &self.samples[i - 1]
        } else {
            &self.samples[i]
        }
    }
    /// Index and xz distance of the sample nearest to `p`.
    pub fn nearest(&self, p: [f32; 3]) -> (usize, f32) {
        let mut best = (0, f32::INFINITY);
        for (i, s) in self.samples.iter().enumerate() {
            let d = (s.pos[0] - p[0]).powi(2) + (s.pos[2] - p[2]).powi(2);
            if d < best.1 {
                best = (i, d);
            }
        }
        (best.0, best.1.sqrt())
    }
}

pub fn dist3(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn norm(v: [f32; 3]) -> [f32; 3] {
    super::scan::norm3(v)
}

/// Left of a forward vector: up x forward = (fz, 0, -fx), normalised in xz.
pub fn left_of(f: [f32; 3]) -> [f32; 3] {
    norm([f[2], 0.0, -f[0]])
}

/// Points every `step` metres along a polyline (closing segment included
/// when closed; the end point repeated only for open lines).
pub fn resample(pts: &[[f32; 3]], closed: bool, step: f32) -> Vec<[f32; 3]> {
    if pts.len() < 2 {
        return pts.to_vec();
    }
    let mut path: Vec<[f32; 3]> = pts.to_vec();
    if closed {
        path.push(pts[0]);
    }
    let seg: Vec<f32> = path.windows(2).map(|w| dist3(w[0], w[1])).collect();
    let total: f32 = seg.iter().sum();
    if total <= 0.0 {
        return vec![pts[0]];
    }
    let n = ((total / step).round() as usize).max(2);
    let st = total / n as f32;
    let count = if closed { n } else { n + 1 };
    let mut out = Vec::with_capacity(count);
    let (mut k, mut acc) = (0usize, 0.0f32);
    for i in 0..count {
        let d = (i as f32 * st).min(total);
        while k + 1 < seg.len() && acc + seg[k] < d {
            acc += seg[k];
            k += 1;
        }
        let t = if seg[k] > 0.0 { ((d - acc) / seg[k]).clamp(0.0, 1.0) } else { 0.0 };
        let (a, b) = (path[k], path[k + 1]);
        out.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]);
    }
    out
}

/// Light xz smoothing (keeps the ends of open lines).
fn smooth_xz(p: &mut [[f32; 3]], closed: bool, passes: usize) {
    let n = p.len();
    if n < 5 {
        return;
    }
    for _ in 0..passes {
        let q = p.to_vec();
        for i in 0..n {
            let (a, b) = if closed { ((i + n - 1) % n, (i + 1) % n) } else if i == 0 || i + 1 == n { continue } else { (i - 1, i + 1) };
            for c in [0, 2] {
                p[i][c] = q[i][c] + 0.25 * ((q[a][c] + q[b][c]) * 0.5 - q[i][c]);
            }
        }
    }
}

fn idx(i: isize, n: usize, closed: bool) -> usize {
    if closed { i.rem_euclid(n as isize) as usize } else { i.clamp(0, n as isize - 1) as usize }
}

/// Put the points on the ground; returns (median offset of the input line
/// above the ground, share snapped, per point the hit).
fn snap(p: &mut [[f32; 3]], g: &Ground) -> (f32, f32, Vec<Option<super::Hit>>) {
    let mut dys: Vec<f32> = p.iter().filter_map(|q| g.below(q[0], q[1] + 2.0, q[2]).filter(|h| h.drivable).map(|h| q[1] - h.y)).filter(|d| d.abs() < 3.0).collect();
    dys.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let off = dys.get(dys.len() / 2).copied().unwrap_or(0.0);
    let mut hits = Vec::with_capacity(p.len());
    let mut ok = 0usize;
    for q in p.iter_mut() {
        let want = q[1] - off;
        let hs = g.hits(q[0], q[2]);
        let h = hs.iter().filter(|h| (h.y - want).abs() <= SNAP_REACH).min_by(|a, b| {
            // drivable first, then nearest in height
            (!a.drivable, (a.y - want).abs()).partial_cmp(&(!b.drivable, (b.y - want).abs())).unwrap()
        }).copied();
        match h {
            Some(h) => {
                q[1] = h.y;
                ok += 1;
            }
            None => q[1] = want,
        }
        hits.push(h);
    }
    (off, ok as f32 / p.len().max(1) as f32, hits)
}

#[derive(Clone, Copy, Debug, Default)]
struct Probe {
    /// first distance off road / pit / concrete
    road: Option<f32>,
    /// first distance off road and kerbs
    curb: Option<f32>,
    wall: Option<f32>,
}

fn class(k: Kind) -> u8 {
    match k {
        Kind::Road | Kind::Pit | Kind::Concrete => 0,
        Kind::Kerb | Kind::FlatKerb => 1,
        Kind::Wall => 3,
        _ => 2,
    }
}

/// Walk the ground from `p` along `dir` (xz unit).
fn probe(g: &Ground, kinds: &[Kind], p: [f32; 3], dir: [f32; 3], max: f32) -> Probe {
    let mut r = Probe::default();
    let mut y = p[1];
    let mut s = 0.0f32;
    loop {
        s += if s < 20.0 { 0.25 } else { 0.5 };
        if s > max {
            break;
        }
        let (x, z) = (p[0] + dir[0] * s, p[2] + dir[2] * s);
        let hs = g.hits(x, z);
        let kind = |h: &super::Hit| kinds.get(h.surface).copied().unwrap_or(Kind::Road);
        // a wall: wall surface or steep face standing above the running ground
        if hs.iter().any(|h| (kind(h) == Kind::Wall || h.normal[1] < 0.5) && h.y > y + 0.15 && h.y < y + 3.0) {
            r.wall = Some(s);
            break;
        }
        let ground = hs.iter().filter(|h| kind(h) != Kind::Wall && h.normal[1] >= 0.5 && h.y > y - 1.5 && h.y < y + 0.6)
            .min_by(|a, b| (a.y - y).abs().partial_cmp(&(b.y - y).abs()).unwrap()).copied();
        let Some(h) = ground else {
            // the ground ends, or rises more than 0.6 m at once
            r.wall = Some(s);
            break;
        };
        if h.y - y > 0.5 {
            r.wall = Some(s);
            break;
        }
        y = h.y;
        let c = class(kind(&h));
        // an edge: walk back over the last step in 5 cm to place it
        let edge = |want: u8| -> f32 {
            let back = if s <= 20.0 { 0.25 } else { 0.5 };
            let mut e = s;
            let mut t = s - back + 0.05;
            while t < s {
                let k = g.hits(p[0] + dir[0] * t, p[2] + dir[2] * t).iter().filter(|h| kind(h) != Kind::Wall && h.normal[1] >= 0.5 && (h.y - y).abs() < 0.6)
                    .min_by(|a, b| (a.y - y).abs().partial_cmp(&(b.y - y).abs()).unwrap()).map(|h| class(kind(h)));
                if k.map(|k| k >= want).unwrap_or(false) {
                    e = t;
                    break;
                }
                t += 0.05;
            }
            e
        };
        if c >= 1 && r.road.is_none() {
            r.road = Some(edge(1));
        }
        if c >= 2 && r.curb.is_none() {
            r.curb = Some(edge(2));
        }
    }
    r
}

/// Signed curvature radius at every point over a 5-segment window, and
/// the window length.
fn radii(p: &[[f32; 3]], closed: bool) -> Vec<(f32, f32)> {
    let n = p.len();
    (0..n).map(|i| {
        let (a, b, c) = (idx(i as isize - 2, n, closed), i, idx(i as isize + 3, n, closed));
        if a == b || b == c {
            return (RADIUS_CAP, 0.0);
        }
        let u = [p[b][0] - p[a][0], p[b][2] - p[a][2]];
        let v = [p[c][0] - p[b][0], p[c][2] - p[b][2]];
        let lu = (u[0] * u[0] + u[1] * u[1]).sqrt();
        let lv = (v[0] * v[0] + v[1] * v[1]).sqrt();
        // stock direction sign: (u.z * v.x - u.x * v.z)
        let cr = u[1] * v[0] - u[0] * v[1];
        let ang = cr.atan2(u[0] * v[0] + u[1] * v[1]);
        let len = lu + lv;
        let r = if ang.abs() > 1e-7 { (len * 0.5) / ang } else { RADIUS_CAP };
        (r.clamp(-RADIUS_CAP, RADIUS_CAP), len)
    }).collect()
}

/// AC1 speeds along our line (scaled by length), when the file has them.
fn ac1_speeds(ac1: &crate::ac1::ai::AiLine, dists: &[f32], total: f32) -> Option<Vec<f32>> {
    if ac1.extra.len() != ac1.points.len() || ac1.points.len() < 4 {
        return None;
    }
    let mut sp: Vec<f32> = ac1.extra.iter().map(|e| e.speed).collect();
    let mut s2 = sp.clone();
    s2.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = s2[s2.len() / 2];
    if !(5.0..120.0).contains(&med) || sp.iter().any(|v| !v.is_finite()) {
        return None;
    }
    for v in &mut sp {
        *v = v.clamp(3.0, 120.0);
    }
    let alen = ac1.length().max(1.0);
    let scale = alen / total.max(1.0);
    let ad: Vec<f32> = ac1.points.iter().map(|p| p.dist).collect();
    Some(dists.iter().map(|d| {
        let x = d * scale;
        let i = ad.partition_point(|v| *v < x).min(ad.len() - 1);
        if i == 0 {
            return sp[0];
        }
        let (d0, d1) = (ad[i - 1], ad[i]);
        let t = if d1 > d0 { ((x - d0) / (d1 - d0)).clamp(0.0, 1.0) } else { 0.0 };
        sp[i - 1] + (sp[i] - sp[i - 1]) * t
    }).collect())
}

/// Point-mass lap (ours): corner limit, then acceleration / braking passes.
pub fn lap_model(radius: &[f32], ds: &[f32], closed: bool) -> Vec<f32> {
    const G: f32 = 9.81;
    const MU: f32 = 1.5;
    const VMAX: f32 = 80.0;
    const ACC: f32 = 7.0;
    const BRAKE: f32 = 11.0;
    let n = radius.len();
    let lim: Vec<f32> = radius.iter().map(|r| (MU * G * r.abs()).sqrt().min(VMAX)).collect();
    let mut v = lim.clone();
    let rounds = if closed { 2 } else { 1 };
    // forward (acceleration)
    if !closed {
        v[0] = v[0].min(5.0);
    }
    for _ in 0..rounds {
        for i in 0..n {
            let j = if i + 1 < n { i + 1 } else if closed { 0 } else { break };
            let a = ACC * (1.0 - (v[i] / VMAX).powi(2)).max(0.0);
            v[j] = v[j].min((v[i] * v[i] + 2.0 * a * ds[i]).sqrt());
        }
    }
    // backward (braking)
    for _ in 0..rounds {
        for i in (0..n).rev() {
            let j = if i + 1 < n { i + 1 } else if closed { 0 } else { continue };
            v[i] = v[i].min((v[j] * v[j] + 2.0 * BRAKE * ds[i]).sqrt());
        }
    }
    v
}

/// Build a line from AC1 points: resample, smooth, put on the ground and
/// measure everything the payloads carry.
pub fn build(points: &[[f32; 3]], closed: bool, step: f32, g: &Ground, kinds: &[Kind], pit: bool, ac1: Option<&crate::ac1::ai::AiLine>) -> Line {
    let mut p = resample(points, closed, step);
    smooth_xz(&mut p, closed, 3);
    let (offset, share, hits) = snap(&mut p, g);
    let n = p.len();
    let mut dists = Vec::with_capacity(n);
    let mut acc = 0.0f32;
    for i in 0..n {
        dists.push(acc);
        if i + 1 < n {
            acc += dist3(p[i], p[i + 1]);
        }
    }
    let length = acc + if closed && n > 1 { dist3(p[n - 1], p[0]) } else { 0.0 };
    let ds: Vec<f32> = (0..n).map(|i| {
        let j = if i + 1 < n { i + 1 } else if closed { 0 } else { i };
        dist3(p[i], p[j])
    }).collect();
    let rad = radii(&p, closed);
    let rr: Vec<f32> = rad.iter().map(|r| r.0).collect();
    // AC1 speeds only when their lap is within 0.6..1.6 x the model's
    // (some AC1 files carry recording placeholders, laps of 300 s and more)
    let model = lap_model(&rr, &ds, closed);
    let lap = |v: &[f32]| -> f32 { ds.iter().zip(v).map(|(d, v)| d / v.max(1.0)).sum() };
    let (speeds, from_ac1) = match ac1.and_then(|a| ac1_speeds(a, &dists, length)).filter(|s| (0.6..1.6).contains(&(lap(s) / lap(&model).max(1.0)))) {
        Some(s) => (s, true),
        None => (model, false),
    };
    use rayon::prelude::*;
    let probes: Vec<(Probe, Probe)> = (0..n).into_par_iter().map(|i| {
        let a = p[idx(i as isize - 1, n, closed)];
        let b = p[idx(i as isize + 1, n, closed)];
        let f = norm([b[0] - a[0], b[1] - a[1], b[2] - a[2]]);
        let l = left_of(f);
        let r = [-l[0], 0.0, -l[2]];
        (probe(g, kinds, p[i], l, WALL_CAP), probe(g, kinds, p[i], r, WALL_CAP))
    }).collect();
    let mut samples = Vec::with_capacity(n);
    for i in 0..n {
        let a = p[idx(i as isize - 1, n, closed)];
        let b = p[idx(i as isize + 1, n, closed)];
        let f = norm([b[0] - a[0], b[1] - a[1], b[2] - a[2]]);
        let l = left_of(f);
        let normal = hits[i].map(|h| h.normal).unwrap_or([0.0, 1.0, 0.0]);
        let surface = hits[i].map(|h| h.surface);
        let (pl, pr) = probes[i];
        // no edge within the probe: 6 m (ours)
        let mut sides = [pl.road.or(pl.wall).unwrap_or(6.0).min(SIDE_CAP), pr.road.or(pr.wall).unwrap_or(6.0).min(SIDE_CAP)];
        if pl.road.is_none() && pr.road.is_none() {
            // no edge found at all: the AC1 side distances, else 6 m
            if let Some(e) = ac1.and_then(|a| a.extra.get(i * a.extra.len() / n.max(1))) {
                if e.side_left > 0.0 && e.side_right > 0.0 {
                    sides = [e.side_left.min(30.0), e.side_right.min(30.0)];
                }
            }
        }
        let curbs = [pl.curb.or(pl.wall).unwrap_or(sides[0]).min(SIDE_CAP).max(sides[0]).max(HALF_CAR_WIDTH), pr.curb.or(pr.wall).unwrap_or(sides[1]).min(SIDE_CAP).max(sides[1]).max(HALF_CAR_WIDTH)];
        let walls = [pl.wall.unwrap_or(WALL_CAP).min(WALL_CAP), pr.wall.unwrap_or(WALL_CAP).min(WALL_CAP)];
        // camber: the normal's lean to the right (stock sign)
        let camber = -(normal[0] * l[0] + normal[2] * l[2]);
        samples.push(Sample {
            pos: p[i], dist: dists[i], fwd: f, normal, radius: rr[i], sides, curbs, walls, camber, grade: f[1], compression: 0.0,
            speed: speeds[i], pit, snapped: hits[i].is_some(), surface,
        });
    }
    for i in 0..n {
        let a = idx(i as isize - 1, n, closed);
        let b = idx(i as isize + 1, n, closed);
        let d = (samples[b].dist - samples[a].dist).abs().max(0.1);
        if a != b {
            samples[i].compression = (samples[b].grade - samples[a].grade) / d;
        }
    }
    Line { samples, closed, length, offset, snapped: share, ac1_speeds: from_ac1 }
}

/// A road centre line traced on the ground from a start point and heading
/// (ours, for AC1 tracks without `fast_lane.ai`): every 2 m the road +
/// kerb width is measured across the heading (up to 15 m a side), the
/// point moves to its middle (at most 0.5 m sideways a step, so a pit or
/// junction mouth does not pull it off), and the heading turns towards the
/// middle 8 m ahead. It stops when it comes back within 6 m of the start
/// after 300 m (closed), when the road is lost (narrower than 3 m for 10
/// steps), or after 40 km. Rejected when a quarter of the way finds no
/// edge within 15 m on either side (a paved lot or test pad, not a road).
pub fn trace_centre(g: &Ground, kinds: &[Kind], start: [f32; 3], fwd: [f32; 3]) -> Option<(Vec<[f32; 3]>, bool)> {
    const STEP: f32 = 2.0;
    const SIDE: f32 = 15.0;
    let track = |x: f32, y: f32, z: f32| -> Option<f32> {
        g.hits(x, z).iter().filter(|h| (h.y - y).abs() < 1.5 && h.normal[1] > 0.5).min_by(|a, b| (a.y - y).abs().partial_cmp(&(b.y - y).abs()).unwrap())
            .filter(|h| class(kinds.get(h.surface).copied().unwrap_or(Kind::Road)) <= 1).map(|h| h.y)
    };
    // edge distance along dir, from p (0.25 m steps)
    let edge = |p: [f32; 3], dir: [f32; 3]| -> f32 {
        let mut y = p[1];
        let mut s = 0.0;
        while s < SIDE {
            let t = s + 0.25;
            match track(p[0] + dir[0] * t, y, p[2] + dir[2] * t) {
                Some(h) => {
                    y = h;
                    s = t;
                }
                None => break,
            }
        }
        s
    };
    let centre = |p: [f32; 3], h: [f32; 3], max_shift: f32| -> Option<([f32; 3], f32)> {
        let y = track(p[0], p[1], p[2])?;
        let p = [p[0], y, p[2]];
        let x = left_of(h);
        let (l, r) = (edge(p, x), edge(p, [-x[0], 0.0, -x[2]]));
        let shift = ((l - r) * 0.5).clamp(-max_shift, max_shift);
        let c = [p[0] + x[0] * shift, y, p[2] + x[2] * shift];
        let cy = track(c[0], y, c[2]).unwrap_or(y);
        Some(([c[0], cy, c[2]], l + r))
    };
    let mut h = norm([fwd[0], 0.0, fwd[2]]);
    let (mut p, _) = centre(start, h, 15.0)?;
    let first = p;
    let mut out = vec![p];
    let mut travelled = 0.0f32;
    let mut lost = 0;
    let mut open = 0usize;
    while travelled < 40_000.0 {
        let ahead = [p[0] + h[0] * 8.0, p[1], p[2] + h[2] * 8.0];
        if let Some((c2, _)) = centre(ahead, h, 2.0) {
            let d = norm([c2[0] - p[0], 0.0, c2[2] - p[2]]);
            // turn at most 25 degrees a step
            let dot = (d[0] * h[0] + d[2] * h[2]).clamp(-1.0, 1.0);
            h = if dot > 25f32.to_radians().cos() { d } else { norm([h[0] * 0.6 + d[0] * 0.4, 0.0, h[2] * 0.6 + d[2] * 0.4]) };
        }
        let next = [p[0] + h[0] * STEP, p[1], p[2] + h[2] * STEP];
        match centre(next, h, 0.5) {
            Some((c, w)) if w >= 3.0 => {
                lost = 0;
                if w >= 2.0 * SIDE - 0.5 {
                    open += 1;
                }
                p = c;
            }
            Some((c, _)) => {
                lost += 1;
                p = c;
            }
            None => {
                lost += 1;
                p = next;
            }
        }
        if lost >= 10 {
            out.truncate(out.len().saturating_sub(lost));
            break;
        }
        travelled += STEP;
        out.push(p);
        if travelled > 300.0 && ((p[0] - first[0]).powi(2) + (p[2] - first[2]).powi(2)).sqrt() < 6.0 {
            return (out.len() > 50 && open * 4 < out.len()).then_some((out, true));
        }
    }
    // an open paved area (no edges on a quarter of the way) is no road
    (out.len() > 50 && open * 4 < out.len()).then_some((out, false))
}

/// Is the point inside a corner (smoothed radius)?
pub fn corner_mask(line: &Line) -> Vec<bool> {
    let n = line.samples.len();
    if n == 0 {
        return Vec::new();
    }
    let step = (line.length / n as f32).max(0.1);
    let w = ((10.0 / step).round() as isize).max(1);
    let mut m: Vec<bool> = (0..n).map(|i| {
        // smoothed curvature: mean 1/R over +-10 m
        let mut k = 0.0;
        for d in -w..=w {
            k += 1.0 / line.samples[idx(i as isize + d, n, line.closed)].radius;
        }
        (k / (2 * w + 1) as f32).abs() > 1.0 / CORNER_R
    }).collect();
    // merge gaps under 30 m, drop corners under 10 m
    let runs = |m: &[bool], val: bool| -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < m.len() {
            if m[i] == val {
                let s = i;
                while i < m.len() && m[i] == val {
                    i += 1;
                }
                out.push((s, i));
            } else {
                i += 1;
            }
        }
        out
    };
    for (s, e) in runs(&m, false) {
        if s > 0 && e < n && (e - s) as f32 * step < 30.0 {
            for v in &mut m[s..e] {
                *v = true;
            }
        }
    }
    for (s, e) in runs(&m, true) {
        if ((e - s) as f32 * step) < 10.0 {
            for v in &mut m[s..e] {
                *v = false;
            }
        }
    }
    m
}

#[derive(Clone, Debug)]
pub struct ControlPoint {
    pub i: usize,
    pub tags: Vec<u32>,
}

/// Control points (ours): per corner brake zone (2), entry (3), apex (4)
/// + slowest point (5), exit + accel zone (6, 10); straights get a point
/// at their start (1, + 0 when 300 m or longer) and every 300 m.
pub fn control_points(line: &Line) -> Vec<ControlPoint> {
    let n = line.samples.len();
    if n < 8 {
        return Vec::new();
    }
    let step = line.length / n as f32;
    let mask = corner_mask(line);
    // corner runs (start, end exclusive); a closed line may wrap
    let mut corners: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < n {
        if mask[i] {
            let s = i;
            while i < n && mask[i] {
                i += 1;
            }
            corners.push((s, i));
        } else {
            i += 1;
        }
    }
    if line.closed && corners.len() >= 2 && corners[0].0 == 0 && corners.last().unwrap().1 == n {
        let last = corners.pop().unwrap();
        corners[0].0 = last.0; // wraps: start > end
    }
    let mut cps: Vec<ControlPoint> = Vec::new();
    let sp = |k: usize| line.samples[k % n].speed;
    let span = |s: usize, e: usize| -> Vec<usize> { if s < e { (s..e).collect() } else { (s..n).chain(0..e).collect() } };
    for &(s, e) in &corners {
        let idxs = span(s, e);
        if idxs.is_empty() {
            continue;
        }
        let apex = *idxs.iter().min_by(|a, b| line.samples[**a].radius.abs().partial_cmp(&line.samples[**b].radius.abs()).unwrap()).unwrap();
        let slow = *idxs.iter().min_by(|a, b| sp(**a).partial_cmp(&sp(**b)).unwrap()).unwrap();
        // brake zone: walk back from the entry while the speed falls
        let mut b = s;
        let mut walked = 0usize;
        while walked < (400.0 / step) as usize {
            let prev = if b == 0 { if line.closed { n - 1 } else { break } } else { b - 1 };
            if sp(prev) <= sp(b) + 0.05 {
                break;
            }
            b = prev;
            walked += 1;
        }
        if walked as f32 * step > 30.0 {
            cps.push(ControlPoint { i: b, tags: vec![2] });
        }
        cps.push(ControlPoint { i: s, tags: vec![3] });
        let gap = |a: usize, c: usize| { let d = (a as isize - c as isize).unsigned_abs(); d.min(n - d) as f32 * step };
        if gap(apex, slow) < 15.0 {
            cps.push(ControlPoint { i: apex, tags: vec![4, 5] });
        } else {
            let (x, y) = if span(s, e).iter().position(|k| *k == apex) < span(s, e).iter().position(|k| *k == slow) { (apex, slow) } else { (slow, apex) };
            cps.push(ControlPoint { i: x, tags: if x == apex { vec![4] } else { vec![5] } });
            cps.push(ControlPoint { i: y, tags: if y == apex { vec![4] } else { vec![5] } });
        }
        let ex = if e == 0 { n - 1 } else { e - 1 };
        cps.push(ControlPoint { i: ex, tags: vec![6, 10] });
    }
    // straights: from each corner exit to the next brake / entry point
    cps.sort_by_key(|c| c.i);
    cps.dedup_by_key(|c| c.i);
    let mut extra = Vec::new();
    let m = cps.len();
    let bounds: Vec<(usize, usize)> = if m == 0 {
        vec![(0, if line.closed { n } else { n - 1 })]
    } else {
        let mut v: Vec<(usize, usize)> = (0..m.saturating_sub(1)).map(|k| (cps[k].i, cps[k + 1].i)).collect();
        if line.closed {
            v.push((cps[m - 1].i, cps[0].i + n));
        } else {
            v.insert(0, (0, cps[0].i));
            v.push((cps[m - 1].i, n - 1));
        }
        v
    };
    for (a, b) in bounds {
        let len = (b - a) as f32 * step;
        if len < 100.0 {
            continue;
        }
        let first = a + (30.0 / step) as usize;
        let mut tags = vec![1];
        if len >= 300.0 {
            tags.insert(0, 0);
        }
        extra.push(ControlPoint { i: first % n, tags });
        let mut k = first + (300.0 / step) as usize;
        while (k as f32) < b as f32 - 60.0 / step {
            extra.push(ControlPoint { i: k % n, tags: vec![1] });
            k += (300.0 / step) as usize;
        }
    }
    cps.extend(extra);
    cps.sort_by_key(|c| c.i);
    cps.dedup_by_key(|c| c.i);
    cps
}

fn put(m: &mut Msg, n: u32, v: f32) {
    if v != 0.0 && v.is_finite() {
        m.set_f32(n, v);
    }
}

pub fn control_points_msg(line: &Line, cps: &[ControlPoint]) -> Msg {
    let mut out = Msg::default();
    let len = line.length.max(1.0);
    for (k, c) in cps.iter().enumerate() {
        let s = &line.samples[c.i];
        let prev = if k > 0 { line.samples[cps[k - 1].i].dist } else if line.closed { line.samples[cps[cps.len() - 1].i].dist - len } else { 0.0 };
        let mut m = Msg::default();
        m.push(1, Val::Msg(v3(s.pos)));
        m.push(2, Val::Msg(v3(s.fwd)));
        put(&mut m, 3, s.sides[0]);
        put(&mut m, 4, s.curbs[0]);
        put(&mut m, 5, s.sides[1]);
        put(&mut m, 6, s.curbs[1]);
        put(&mut m, 7, s.radius.clamp(-10000.0, 10000.0));
        put(&mut m, 8, 1.0);
        put(&mut m, 9, s.dist - prev);
        put(&mut m, 10, s.dist / len);
        put(&mut m, 11, s.walls[0]);
        put(&mut m, 12, s.walls[1]);
        put(&mut m, 13, 1.0);
        let mut t = Vec::new();
        for x in &c.tags {
            pb::write_varint(&mut t, *x as u64);
        }
        m.push(14, Val::Bytes(t));
        out.push(1, Val::Msg(m));
    }
    out
}

/// 2-d kd-tree for the k-nearest grid lists.
struct Kd {
    pts: Vec<[f32; 2]>,
    order: Vec<u32>,
}

impl Kd {
    fn new(pts: Vec<[f32; 2]>) -> Kd {
        let mut order: Vec<u32> = (0..pts.len() as u32).collect();
        fn build(p: &[[f32; 2]], o: &mut [u32], depth: usize) {
            if o.len() <= 1 {
                return;
            }
            let ax = depth % 2;
            let mid = o.len() / 2;
            o.select_nth_unstable_by(mid, |a, b| p[*a as usize][ax].partial_cmp(&p[*b as usize][ax]).unwrap());
            let (l, r) = o.split_at_mut(mid);
            build(p, l, depth + 1);
            build(p, &mut r[1..], depth + 1);
        }
        build(&pts, &mut order, 0);
        Kd { pts, order }
    }
    /// k nearest (index, squared distance), nearest first (ties by index).
    fn knn(&self, q: [f32; 2], k: usize) -> Vec<(u32, f32)> {
        let mut best: Vec<(u32, f32)> = Vec::with_capacity(k + 1);
        fn go(t: &Kd, o: &[u32], depth: usize, q: [f32; 2], k: usize, best: &mut Vec<(u32, f32)>) {
            if o.is_empty() {
                return;
            }
            let ax = depth % 2;
            let mid = o.len() / 2;
            let i = o[mid];
            let p = t.pts[i as usize];
            let d = (p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2);
            if best.len() < k || d < best[best.len() - 1].1 || (d == best[best.len() - 1].1 && i < best[best.len() - 1].0) {
                let pos = best.partition_point(|x| x.1 < d || (x.1 == d && x.0 < i));
                best.insert(pos, (i, d));
                best.truncate(k);
            }
            let diff = q[ax] - p[ax];
            let (near, far) = if diff < 0.0 { (&o[..mid], &o[mid + 1..]) } else { (&o[mid + 1..], &o[..mid]) };
            go(t, near, depth + 1, q, k, best);
            if best.len() < k || diff * diff <= best[best.len() - 1].1 {
                go(t, far, depth + 1, q, k, best);
            }
        }
        go(self, &self.order, 0, q, k, &mut best);
        best
    }
}

/// The nearest-point grid (stock layout, see the module header).
fn grid(points: &[[f32; 3]]) -> Msg {
    const MARGIN: f32 = 350.0;
    const CELL: f32 = 10.0;
    const K: usize = 10;
    let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
    for p in points {
        lo = [lo[0].min(p[0]), lo[1].min(p[2])];
        hi = [hi[0].max(p[0]), hi[1].max(p[2])];
    }
    lo = [lo[0] - MARGIN, lo[1] - MARGIN];
    hi = [hi[0] + MARGIN, hi[1] + MARGIN];
    let nx = ((hi[0] - lo[0]) / CELL).floor() as usize;
    let nz = ((hi[1] - lo[1]) / CELL).floor() as usize;
    let kd = Kd::new(points.iter().map(|p| [p[0], p[2]]).collect());
    let k = K.min(points.len());
    use rayon::prelude::*;
    let rows: Vec<Vec<u8>> = (0..nx).into_par_iter().map(|r| {
        let mut row = Msg::default();
        for c in 0..nz {
            let q = [lo[0] + (r as f32 + 0.5) * CELL, lo[1] + (c as f32 + 0.5) * CELL];
            let ids: Vec<u32> = kd.knn(q, k).into_iter().map(|x| x.0).collect();
            let mut cell = Msg::default();
            cell.push(1, Val::Bytes(pb::varint_bytes(&ids)));
            row.push(1, Val::Msg(cell));
        }
        row.encode()
    }).collect();
    let mut g = Msg::default();
    g.push(1, Val::Msg(v3([hi[0], 0.0, hi[1]])));
    g.push(2, Val::Msg(v3([lo[0], 0.0, lo[1]])));
    g.set_varint(3, K as u64);
    g.push(4, Val::F64(10.0f64.to_le_bytes()));
    for r in rows {
        g.push(5, Val::Bytes(r));
    }
    g
}

pub struct Extra<'a> {
    /// layout container path without `.scene` (ideal line)
    pub layout_path: Option<String>,
    pub control_points: Option<&'a Msg>,
    pub track_info: Option<&'a Msg>,
    pub is_pitlane: bool,
}

/// Encode a line as AISplineData.
pub fn encode(line: &Line, x: &Extra) -> Vec<u8> {
    let n = line.samples.len();
    let mut sd = Msg::default();
    for (i, s) in line.samples.iter().enumerate() {
        let mut p = Msg::default();
        p.push(1, Val::Msg(v3(s.pos)));
        put(&mut p, 2, s.dist);
        if i > 0 {
            p.set_varint(3, i as u64);
        }
        sd.push(1, Val::Msg(p));
    }
    sd.push(2, Val::F64((line.length as f64).to_le_bytes()));
    if line.closed {
        sd.set_varint(3, 1);
    }
    sd.set_varint(5, 2);
    if n >= 2 {
        let pts: Vec<[f32; 3]> = line.samples.iter().map(|s| s.pos).collect();
        sd.push(6, Val::Msg(grid(&pts)));
    }
    let mut top = Msg::default();
    top.push(1, Val::Msg(sd));
    let mask = if x.is_pitlane { vec![false; n] } else { corner_mask(line) };
    // distance from the last corner / to the next corner (absent inside one)
    let mut from = vec![f32::NAN; n];
    let mut to = vec![f32::NAN; n];
    if mask.iter().any(|m| *m) {
        let rounds = if line.closed { 2 } else { 1 };
        let mut last: Option<f32> = None;
        for r in 0..rounds {
            for i in 0..n {
                let d = line.samples[i].dist + r as f32 * line.length;
                if mask[i] {
                    last = Some(d);
                } else if let Some(l) = last {
                    from[i] = d - l;
                }
            }
        }
        let mut next: Option<f32> = None;
        for r in (0..rounds).rev() {
            for i in (0..n).rev() {
                let d = line.samples[i].dist + r as f32 * line.length;
                if mask[i] {
                    next = Some(d);
                } else if let Some(l) = next {
                    to[i] = l - d;
                }
            }
        }
    }
    for i in 0..n {
        let s = &line.samples[i];
        let j = idx(i as isize + 1, n, line.closed);
        let ds = (line.samples[j].dist - s.dist).abs().max(0.1);
        let rate = if j != i { (line.samples[j].radius.abs().min(1e5) - s.radius.abs().min(1e5)) / ds } else { 0.0 };
        let mut m = Msg::default();
        put(&mut m, 1, 13.8);
        put(&mut m, 2, s.radius.abs());
        if !x.is_pitlane {
            put(&mut m, 3, s.sides[0]);
            put(&mut m, 4, s.sides[1]);
        }
        put(&mut m, 5, -1.0);
        put(&mut m, 6, -1.0);
        put(&mut m, 7, s.camber);
        put(&mut m, 8, if s.radius >= 0.0 { 1.0 } else { -1.0 });
        m.push(9, Val::Msg(v3(s.normal)));
        m.push(10, Val::Msg(v3(s.fwd)));
        let win = {
            let a = idx(i as isize - 2, n, line.closed);
            let b = idx(i as isize + 3, n, line.closed);
            let d = line.samples[b].dist - line.samples[a].dist;
            if d < 0.0 { d + line.length } else { d }
        };
        put(&mut m, 11, win);
        put(&mut m, 12, 0.78);
        put(&mut m, 14, s.grade);
        put(&mut m, 15, 1.0);
        if !mask[i] && from[i].is_finite() {
            put(&mut m, 16, from[i].round().clamp(1.0, 500.0));
        }
        if !mask[i] && to[i].is_finite() {
            put(&mut m, 17, to[i].round().max(1.0));
        }
        if s.pit {
            m.set_varint(18, 1);
        }
        put(&mut m, 19, s.compression);
        put(&mut m, 22, s.walls[0]);
        put(&mut m, 23, s.walls[1]);
        if !x.is_pitlane {
            put(&mut m, 24, s.curbs[0]);
            put(&mut m, 25, s.curbs[1]);
        }
        put(&mut m, 26, rate);
        if !x.is_pitlane {
            put(&mut m, 27, s.sides[0]);
            put(&mut m, 28, s.sides[1]);
        }
        top.push(2, Val::Msg(m));
    }
    put(&mut top, 3, -1.0);
    if let Some(p) = &x.layout_path {
        top.set_str(5, p);
    }
    put(&mut top, 6, if x.is_pitlane { -1.0 } else { HALF_CAR_WIDTH });
    top.push(7, Val::Msg(x.control_points.cloned().unwrap_or_default()));
    if let Some(t) = x.track_info {
        top.push(8, Val::Msg(t.clone()));
    }
    top.encode()
}

/// What our reader sees in an `.aisplinedata`.
#[derive(Clone, Debug, Default)]
pub struct Parsed {
    pub points: Vec<[f32; 3]>,
    pub point_length: Vec<f32>,
    pub length: f64,
    pub closed: bool,
    pub mode: u64,
    pub payloads: usize,
    /// (rows, cells per row) of the nearest-point grid
    pub grid: Option<(usize, usize)>,
    pub control_points: usize,
    pub speeds: Vec<f32>,
    pub sides: Vec<[f32; 2]>,
    pub pit_flags: usize,
}

pub fn parse(b: &[u8]) -> Result<Parsed> {
    let m = Msg::parse(b).context("not a protobuf")?;
    let sd = m.sub(1).context("no spline data (f1)")?;
    let mut p = Parsed { closed: sd.varint(3) == Some(1), mode: sd.varint(5).unwrap_or(0), ..Default::default() };
    for (i, pt) in sd.subs(1).iter().enumerate() {
        let v = pt.sub(1).unwrap_or_default();
        p.points.push([v.f32(1).unwrap_or(0.0), v.f32(2).unwrap_or(0.0), v.f32(3).unwrap_or(0.0)]);
        p.point_length.push(pt.f32(2).unwrap_or(0.0));
        if pt.varint(3).unwrap_or(0) != i as u64 {
            bail!("point {i}: tag {} is not its index", pt.varint(3).unwrap_or(0));
        }
    }
    p.length = match sd.get(2) {
        Some(Val::F64(x)) => f64::from_le_bytes(*x),
        _ => 0.0,
    };
    if let Some(g) = sd.sub(6) {
        let rows = g.subs(5);
        let cells = rows.first().map(|r| r.subs(1).len()).unwrap_or(0);
        let n = p.points.len() as u64;
        for (ri, r) in rows.iter().enumerate() {
            let cs = r.subs(1);
            if cs.len() != cells {
                bail!("grid row {ri}: {} cells, row 0 has {cells}", cs.len());
            }
            for c in cs {
                if pb::varints(c.bytes(1).unwrap_or(&[])).iter().any(|x| *x >= n) {
                    bail!("grid row {ri}: a point index past the {n} points");
                }
            }
        }
        p.grid = Some((rows.len(), cells));
    }
    let pays = m.subs(2);
    p.payloads = pays.len();
    for q in &pays {
        p.speeds.push(q.f32(1).unwrap_or(0.0));
        p.sides.push([q.f32(3).unwrap_or(0.0), q.f32(4).unwrap_or(0.0)]);
        if q.varint(18) == Some(1) {
            p.pit_flags += 1;
        }
    }
    p.control_points = m.sub(7).map(|c| c.subs(1).len()).unwrap_or(0);
    if p.payloads != p.points.len() {
        bail!("{} payloads for {} points", p.payloads, p.points.len());
    }
    if p.points.len() < 2 {
        bail!("{} points", p.points.len());
    }
    if p.point_length.windows(2).any(|w| w[1] < w[0]) {
        bail!("point lengths are not ascending");
    }
    Ok(p)
}

/// Read a `.trackcontrolpoints`: the number of points, checked ascending.
pub fn parse_control_points(b: &[u8]) -> Result<usize> {
    let m = Msg::parse(b).context("not a protobuf")?;
    let cps = m.subs(1);
    let pos: Vec<f32> = cps.iter().map(|c| c.f32(10).unwrap_or(0.0)).collect();
    if pos.windows(2).any(|w| w[1] < w[0]) {
        bail!("spline positions are not ascending");
    }
    if pos.iter().any(|p| !(0.0..=1.0).contains(p)) {
        bail!("a spline position outside 0..1");
    }
    Ok(cps.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::ground::Tri;

    fn flat(size: f32) -> (Ground, Vec<Kind>) {
        // a 20 m wide road ring-ish square plate with grass around it
        let t = vec![
            Tri { v: [[-size, 0.0, -size], [size, 0.0, -size], [size, 0.0, size]], surface: 0 },
            Tri { v: [[-size, 0.0, -size], [size, 0.0, size], [-size, 0.0, size]], surface: 0 },
        ];
        (Ground::from_triangles(t, vec![true]), vec![Kind::Road])
    }

    fn circle(r: f32, n: usize, y: f32) -> Vec<[f32; 3]> {
        (0..n).map(|i| {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            [r * a.cos(), y, r * a.sin()]
        }).collect()
    }

    #[test]
    fn circle_line() {
        let (g, k) = flat(200.0);
        let pts = circle(100.0, 400, 0.3);
        let l = build(&pts, true, IDEAL_STEP, &g, &k, false, None);
        let want = std::f32::consts::TAU * 100.0;
        assert!((l.length - want).abs() / want < 0.01, "{}", l.length);
        assert!((l.offset - 0.3).abs() < 1e-3);
        assert!(l.samples.iter().all(|s| s.pos[1].abs() < 1e-4));
        // radius 100, one direction everywhere; the lap model's corner speed
        let r: Vec<f32> = l.samples.iter().map(|s| s.radius).collect();
        assert!(r.iter().all(|x| (x.abs() - 100.0).abs() < 3.0 && x.signum() == r[0].signum()), "{:?}", &r[..4]);
        let v = (1.5 * 9.81 * 100.0f32).sqrt();
        assert!(l.samples.iter().all(|s| (s.speed - v).abs() < 1.0));
        let cps = control_points(&l);
        let msg = control_points_msg(&l, &cps);
        let b = encode(&l, &Extra { layout_path: Some("content\\tracks\\t\\containers\\layout_x".into()), control_points: Some(&msg), track_info: None, is_pitlane: false });
        let p = parse(&b).unwrap();
        assert_eq!(p.points.len(), l.samples.len());
        assert!(p.closed && (p.length - l.length as f64).abs() < 0.01);
        let (rows, cells) = p.grid.unwrap();
        assert!((89..=90).contains(&rows) && (89..=90).contains(&cells), "{rows} {cells}");
        assert_eq!(parse_control_points(&msg.encode()).unwrap(), cps.len());
    }

    #[test]
    fn kd_matches_brute_force() {
        let pts: Vec<[f32; 2]> = (0..500).map(|i| { let a = i as f32 * 0.37; [a.cos() * (50.0 + i as f32), a.sin() * 80.0] }).collect();
        let kd = Kd::new(pts.clone());
        for q in [[0.0, 0.0], [300.0, -40.0], [-1000.0, 1000.0]] {
            let mut b: Vec<(u32, f32)> = pts.iter().enumerate().map(|(i, p)| (i as u32, (p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2))).collect();
            b.sort_by(|x, y| x.1.partial_cmp(&y.1).unwrap().then(x.0.cmp(&y.0)));
            let got: Vec<u32> = kd.knn(q, 10).iter().map(|x| x.0).collect();
            assert_eq!(got, b[..10].iter().map(|x| x.0).collect::<Vec<_>>());
        }
    }

    #[test]
    fn sides_and_walls() {
        // road 10 m wide (x in -5..5), grass to 15, then a wall
        let mut t = Vec::new();
        let quad = |x0: f32, x1: f32, s: u16, t: &mut Vec<Tri>| {
            t.push(Tri { v: [[x0, 0.0, -100.0], [x1, 0.0, -100.0], [x1, 0.0, 100.0]], surface: s });
            t.push(Tri { v: [[x0, 0.0, -100.0], [x1, 0.0, 100.0], [x0, 0.0, 100.0]], surface: s });
        };
        quad(-5.0, 5.0, 0, &mut t);
        quad(-15.0, -5.0, 1, &mut t);
        quad(5.0, 15.0, 1, &mut t);
        // walls: steep triangles at |x| = 15
        for x in [-15.0f32, 15.0] {
            t.push(Tri { v: [[x, 0.0, -100.0], [x + 0.05 * x.signum(), 2.0, -100.0], [x + 0.05 * x.signum(), 2.0, 100.0]], surface: 2 });
            t.push(Tri { v: [[x, 0.0, -100.0], [x + 0.05 * x.signum(), 2.0, 100.0], [x, 0.0, 100.0]], surface: 2 });
        }
        let g = Ground::from_triangles(t, vec![true, true, false]);
        let kinds = vec![Kind::Road, Kind::Grass, Kind::Wall];
        // the line runs along +z at x = 2: left (up x forward) = +x
        let pts: Vec<[f32; 3]> = (0..60).map(|i| [2.0, 0.3, -50.0 + i as f32 * 2.0]).collect();
        let l = build(&pts, false, IDEAL_STEP, &g, &kinds, false, None);
        let s = &l.samples[l.samples.len() / 2];
        assert!((s.sides[0] - 3.0).abs() < 0.3 && (s.sides[1] - 7.0).abs() < 0.3, "{:?}", s.sides);
        assert!((s.walls[0] - 13.0).abs() < 0.6 && (s.walls[1] - 17.0).abs() < 0.6, "{:?}", s.walls);
        assert_eq!(s.curbs, s.sides);
    }
}
