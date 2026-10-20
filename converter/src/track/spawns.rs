//! Spawn points (grid, pit / box, hotlap) and timing gates of one layout.
//!
//! Game facts (stock containers, ACECM `protos\Scene.proto.txt`):
//! - "Start Pos" entities, f50.f104 StartingPositionData: f6 packed types
//!   (3 grid, 4 pit, 5 box; a slot may carry 4 and 5 together), f2 slot
//!   number (absent for 0), f4 tags (empty). Stock hotlap starts are type 3
//!   without a slot. Stock spawns sit on the road (measured: 0.00..0.10 m
//!   above it on oulton_park and kyalami); the car's forward is local +Z.
//! - Stock pit containers hold pit slots in the lane (type 4) and boxes in
//!   the garages (type 5).
//! - Timing: one "Zone" entity, f50.f112 ZoneActorData {f30 debug, f1000
//!   ZoneData {f1 one ZoneGeometricData per gate: 4 corner transforms, f21
//!   colour, f22 render height 0.8; f2 instance {f100 timelines}}}. The
//!   gates are in lap order with the start / finish line LAST (stock: the
//!   last gate lies at distance 0 / lap length of the ideal line, the
//!   others at the sector splits). Stock gates are thin quads 26..50 m
//!   wide and 0.7..2.4 m deep.
//!
//! AC1 inputs: `AC_START_n`, `AC_PIT_n`, `AC_HOTLAP_START_n`,
//! `AC_TIME_n_L` / `_R` (n = 0 is the start / finish line). Dummies point
//! along their local +Z.
//!
//! Ours: a spawn is snapped to the highest track surface under its dummy
//! (from 3 m above to 20 m below: AC1 drops cars from dummies that may
//! float metres up; Barber's hotlap start is 6.7 m above the road), else
//! to any surface there, and lifted 0.05 m; duplicates (within 0.5 m) appear once, in AC1 order. A
//! track without `AC_PIT_n` gets its pit and box spawns on the grid slots;
//! without `AC_START_n` its grid on the pit slots; with neither, both are
//! laid along the AI line behind its start (two columns 5 m apart, 8 m per
//! row). Without a hotlap dummy the hotlap start is grid slot 0. A gate is
//! the AC1 line widened by 3 m each side, 0.9 m deep; without AC1 time
//! lines one start / finish gate is laid across the AI line's first point,
//! or, without an AI line, 10 m ahead of grid slot 0.

use super::ground::Ground;
use super::scan::{norm3, Dummy, Layout};
use super::scene::{euler_of, transform, Entity, SceneBuilder};
use crate::io::pb::{Msg, Val};

/// Height above the snapped ground (ours).
pub const CLEARANCE: f32 = 0.05;
/// Snap reach above the dummy, and how far below it ground is searched (ours).
const REACH: f32 = 3.0;
const DROP: f32 = 20.0;

#[derive(Clone, Copy, Debug)]
pub struct Spot {
    pub pos: [f32; 3],
    pub fwd: [f32; 3],
    pub up: [f32; 3],
    /// snapped to the ground (false = kept at the AC1 height)
    pub snapped: bool,
}

#[derive(Clone, Debug)]
pub struct Gate {
    /// AC1 line number (0 = start / finish)
    pub n: u32,
    /// corners, world space
    pub corners: [[f32; 3]; 4],
    pub fwd: [f32; 3],
}

#[derive(Clone, Debug, Default)]
pub struct Spawns {
    pub grid: Vec<Spot>,
    /// pit-lane slots, written with types 4 + 5 (pit and box)
    pub pits: Vec<Spot>,
    pub hotlap: Vec<Spot>,
    /// in the game's order (start / finish last)
    pub gates: Vec<Gate>,
    pub notes: Vec<String>,
}

fn numbered<'a>(l: &'a Layout, prefix: &str) -> Vec<(u32, &'a Dummy)> {
    let p = prefix.to_uppercase();
    let mut v: Vec<(u32, &Dummy)> = l.dummies.iter().filter_map(|d| {
        let u = d.name.to_uppercase();
        let rest = u.strip_prefix(&p)?;
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        (!digits.is_empty() && digits.len() == rest.len()).then(|| (digits.parse().ok(), d)).and_then(|(n, d)| n.map(|n| (n, d)))
    }).collect();
    v.sort_by_key(|x| x.0);
    v
}

pub fn dist3(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn snap(g: &Ground, p: [f32; 3], fwd: [f32; 3]) -> Spot {
    // AC1 drops cars from their dummy: the drivable surface just under it
    // (up to DROP below, REACH above), else any surface under it
    let hs = g.hits(p[0], p[2]);
    let under = |drivable: bool| hs.iter().filter(|h| (!drivable || h.drivable) && h.y <= p[1] + REACH && h.y >= p[1] - DROP)
        .max_by(|a, b| a.y.partial_cmp(&b.y).unwrap()).copied();
    match under(true).or_else(|| under(false)) {
        Some(h) => Spot { pos: [p[0], h.y + CLEARANCE, p[2]], fwd, up: h.normal, snapped: true },
        None => Spot { pos: p, fwd, up: [0.0, 1.0, 0.0], snapped: false },
    }
}

fn dedup(v: Vec<Spot>) -> Vec<Spot> {
    let mut out: Vec<Spot> = Vec::new();
    for s in v {
        if !out.iter().any(|o| dist3(o.pos, s.pos) < 0.5) {
            out.push(s);
        }
    }
    out
}

/// Spots along the AI line behind its first point (synthetic grid).
fn along_line(l: &Layout, g: &Ground, n: usize) -> Vec<Spot> {
    let Some(ai) = &l.fast_lane else { return Vec::new() };
    let pts: Vec<[f32; 3]> = ai.points.iter().map(|p| p.pos).collect();
    if pts.len() < 4 {
        return Vec::new();
    }
    // walk backwards from the end of the loop
    let mut out = Vec::new();
    let total = ai.length();
    for k in 0..n {
        let d = total - 10.0 - 8.0 * (k / 2) as f32;
        let i = ai.points.iter().rposition(|p| p.dist <= d).unwrap_or(0);
        let j = (i + 1).min(pts.len() - 1);
        let f = norm3([pts[j][0] - pts[i][0], 0.0, pts[j][2] - pts[i][2]]);
        let side = if k % 2 == 0 { 2.5 } else { -2.5 };
        // local +X = up x forward = (fz, 0, -fx)
        let p = [pts[i][0] + side * f[2], pts[i][1] + 1.0, pts[i][2] - side * f[0]];
        out.push(snap(g, p, f));
    }
    out
}

/// The spawns and gates of layout `l` on its ground.
pub fn build(l: &Layout, g: &Ground) -> Spawns {
    let mut s = Spawns::default();
    let spots = |prefix: &str| -> Vec<Spot> { dedup(numbered(l, prefix).into_iter().map(|(_, d)| snap(g, d.position(), d.forward())).collect()) };
    let mut grid = spots("AC_START_");
    let mut pits = spots("AC_PIT_");
    let hot = spots("AC_HOTLAP_START_");
    if grid.is_empty() && !pits.is_empty() {
        s.notes.push("no AC_START_n: the grid is laid on the pit slots".into());
        grid = pits.clone();
    }
    if pits.is_empty() && !grid.is_empty() {
        s.notes.push("no AC_PIT_n: pit and box spawns are laid on the grid slots".into());
        pits = grid.clone();
    }
    if grid.is_empty() {
        grid = along_line(l, g, 20);
        pits = grid.clone();
        if !grid.is_empty() {
            s.notes.push("no AC_START_n / AC_PIT_n: grid and pits laid along the AI line".into());
        }
    }
    s.hotlap = if hot.is_empty() { grid.first().copied().into_iter().collect() } else { hot };
    let unsnapped = grid.iter().chain(&pits).chain(&s.hotlap).filter(|x| !x.snapped).count();
    if unsnapped > 0 {
        s.notes.push(format!("{unsnapped} spawn(s) found no ground from {REACH} m above to {DROP} m below: kept at the AC1 height"));
    }
    s.grid = grid;
    s.pits = pits;
    s.gates = gates(l, g);
    if s.gates.is_empty() {
        // no AC1 time lines and no AI line: one start / finish line 10 m
        // ahead of grid slot 0, 30 m wide, so every mode has a lap line
        if let Some(p) = s.grid.first() {
            let f = norm3([p.fwd[0], 0.0, p.fwd[2]]);
            let c = [p.pos[0] + f[0] * 10.0, p.pos[1], p.pos[2] + f[2] * 10.0];
            let a = [c[0] + f[2] * 12.0, c[1], c[2] - f[0] * 12.0];
            let b = [c[0] - f[2] * 12.0, c[1], c[2] + f[0] * 12.0];
            s.gates.push(gate(0, a, b, Some(f), g));
            s.notes.push("no AC1 time lines or AI line: a start / finish line is laid 10 m ahead of grid slot 0".into());
        }
    }
    s
}

fn ground_y(g: &Ground, p: [f32; 3]) -> f32 {
    g.nearest(p[0], p[1], p[2]).filter(|h| (h.y - p[1]).abs() < 10.0).map(|h| h.y).unwrap_or(p[1])
}

fn ai_forward(l: &Layout, at: [f32; 3]) -> Option<[f32; 3]> {
    let ai = l.fast_lane.as_ref()?;
    let n = ai.points.len();
    if n < 2 {
        return None;
    }
    let i = (0..n).min_by(|&a, &b| dist3(ai.points[a].pos, at).partial_cmp(&dist3(ai.points[b].pos, at)).unwrap())?;
    let (a, b) = if i + 1 < n { (ai.points[i].pos, ai.points[i + 1].pos) } else { (ai.points[i - 1].pos, ai.points[i].pos) };
    Some(norm3([b[0] - a[0], 0.0, b[2] - a[2]]))
}

/// A gate quad between two track-edge points.
fn gate(n: u32, a: [f32; 3], b: [f32; 3], fwd_hint: Option<[f32; 3]>, g: &Ground) -> Gate {
    let across = norm3([b[0] - a[0], 0.0, b[2] - a[2]]);
    // forward = perpendicular to the line in xz, along the driving direction
    let mut fwd = [-across[2], 0.0, across[0]];
    if let Some(h) = fwd_hint {
        if fwd[0] * h[0] + fwd[2] * h[2] < 0.0 {
            fwd = [-fwd[0], 0.0, -fwd[2]];
        }
    }
    const MARGIN: f32 = 3.0;
    const HALF_DEPTH: f32 = 0.45;
    let l = [a[0] - across[0] * MARGIN, a[1], a[2] - across[2] * MARGIN];
    let r = [b[0] + across[0] * MARGIN, b[1], b[2] + across[2] * MARGIN];
    let mut c = [
        [l[0] - fwd[0] * HALF_DEPTH, l[1], l[2] - fwd[2] * HALF_DEPTH],
        [r[0] - fwd[0] * HALF_DEPTH, r[1], r[2] - fwd[2] * HALF_DEPTH],
        [r[0] + fwd[0] * HALF_DEPTH, r[1], r[2] + fwd[2] * HALF_DEPTH],
        [l[0] + fwd[0] * HALF_DEPTH, l[1], l[2] + fwd[2] * HALF_DEPTH],
    ];
    for p in &mut c {
        p[1] = ground_y(g, *p);
    }
    Gate { n, corners: c, fwd }
}

fn gates(l: &Layout, g: &Ground) -> Vec<Gate> {
    let left = numbered_lr(l, "_L");
    let right = numbered_lr(l, "_R");
    let mut out: Vec<Gate> = Vec::new();
    for (n, a) in &left {
        let Some((_, b)) = right.iter().find(|(m, _)| m == n) else { continue };
        let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0, (a[2] + b[2]) / 2.0];
        out.push(gate(*n, *a, *b, ai_forward(l, mid), g));
    }
    if out.is_empty() {
        if let Some(ai) = &l.fast_lane {
            if ai.points.len() >= 2 {
                let p = ai.points[0].pos;
                let f = norm3([ai.points[1].pos[0] - p[0], 0.0, ai.points[1].pos[2] - p[2]]);
                let (wl, wr) = ai.extra.first().map(|e| (e.side_left.clamp(3.0, 30.0), e.side_right.clamp(3.0, 30.0))).unwrap_or((8.0, 8.0));
                // local +X (left) = (fz, 0, -fx)
                let a = [p[0] + f[2] * wl, p[1], p[2] - f[0] * wl];
                let b = [p[0] - f[2] * wr, p[1], p[2] + f[0] * wr];
                out.push(gate(0, a, b, Some(f), g));
            }
        }
    }
    // lap order, start / finish last
    out.sort_by_key(|x| if x.n == 0 { u32::MAX } else { x.n });
    out
}

fn numbered_lr(l: &Layout, side: &str) -> Vec<(u32, [f32; 3])> {
    l.dummies.iter().filter_map(|d| {
        let u = d.name.to_uppercase();
        let rest = u.strip_prefix("AC_TIME_")?.strip_suffix(side)?;
        Some((rest.parse().ok()?, d.position()))
    }).collect()
}

fn start_pos(name: &str, s: &Spot, types: &[u64], slot: Option<u32>) -> Entity {
    let mut d = Msg::default();
    if let Some(n) = slot.filter(|n| *n > 0) {
        d.set_varint(2, n as u64);
    }
    d.push(4, Val::Msg(Msg::default()));
    let mut packed = Vec::new();
    for t in types {
        crate::io::pb::write_varint(&mut packed, *t);
    }
    d.push(6, Val::Bytes(packed));
    let mut e = Entity::new(name, "Start Pos", 104, d);
    e.position = s.pos;
    e.rotation = euler_of(s.fwd, s.up);
    e
}

/// Write the spawn and timeline containers of layout `stem` (`<stem>` is
/// the layout's container suffix): `spawnpoints_grid_<stem>`,
/// `spawnpoints_pitlane_<stem>`, `spawnpoints_hotlap_<stem>`,
/// `timelines_<stem>`. Returns the container stems written.
pub fn write(b: &mut SceneBuilder, stem: &str, s: &Spawns) -> Vec<String> {
    let mut made = Vec::new();
    let grid = format!("spawnpoints_grid_{stem}");
    for (i, p) in s.grid.iter().enumerate() {
        b.add(Some(&grid), start_pos(&format!("grid{i}"), p, &[3], Some(i as u32)));
    }
    let pit = format!("spawnpoints_pitlane_{stem}");
    for (i, p) in s.pits.iter().enumerate() {
        b.add(Some(&pit), start_pos(&format!("pit{i}"), p, &[4, 5], Some(i as u32)));
    }
    let hot = format!("spawnpoints_hotlap_{stem}");
    for (i, p) in s.hotlap.iter().enumerate() {
        b.add(Some(&hot), start_pos(&format!("hotlap_start_{i}"), p, &[3], None));
    }
    for c in [&grid, &pit, &hot] {
        if b.containers.contains_key(c.as_str()) {
            made.push(c.clone());
        }
    }
    if !s.gates.is_empty() {
        let tl = format!("timelines_{stem}");
        let mut zd = Msg::default();
        let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
        for gt in &s.gates {
            let mut geo = Msg::default();
            let rot = euler_of(gt.fwd, [0.0, 1.0, 0.0]);
            for c in &gt.corners {
                geo.push(1, Val::Msg(transform(*c, rot, [1.0; 3])));
                lo = [lo[0].min(c[0]), lo[1].min(c[2])];
                hi = [hi[0].max(c[0]), hi[1].max(c[2])];
            }
            geo.push(21, Val::Msg(super::scene::v3([0.0, 1.0, 0.0])));
            geo.set_f32(22, 0.8);
            zd.push(1, Val::Msg(geo));
        }
        let mut inst = Msg::default();
        inst.push(100, Val::Msg(Msg::default()));
        zd.push(2, Val::Msg(inst));
        let mut za = Msg::default();
        let mut dbg = Msg::default();
        dbg.set_f32(1, 0.05);
        dbg.set_varint(3, 3);
        za.push(30, Val::Msg(dbg));
        za.push(1000, Val::Msg(zd));
        let mut e = Entity::new(&tl, "Zone", 112, za);
        e.bounds = Some((lo, hi));
        b.add(Some(&tl), e);
        b.declare(&tl, "timelines");
        made.push(tl);
    }
    made
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::ground::Tri;

    fn flat() -> Ground {
        let t = vec![
            Tri { v: [[-100.0, 2.0, -100.0], [100.0, 2.0, -100.0], [100.0, 2.0, 100.0]], surface: 0 },
            Tri { v: [[-100.0, 2.0, -100.0], [100.0, 2.0, 100.0], [-100.0, 2.0, 100.0]], surface: 0 },
        ];
        Ground::from_triangles(t, vec![true])
    }

    fn dummy(name: &str, p: [f32; 3]) -> Dummy {
        let mut w = crate::ac1::kn5::IDENTITY;
        w[12] = p[0];
        w[13] = p[1];
        w[14] = p[2];
        Dummy { name: name.into(), world: w }
    }

    fn layout(d: Vec<Dummy>) -> Layout {
        Layout { name: String::new(), ui: Default::default(), surfaces_ini: Default::default(), fast_lane: None, pit_lane: None, dummies: d, data: Default::default() }
    }

    #[test]
    fn grid_pits_gates() {
        let g = flat();
        let l = layout(vec![
            dummy("AC_START_1", [0.0, 2.5, -8.0]), dummy("AC_START_0", [0.0, 2.5, 0.0]), dummy("AC_START_2", [0.0, 2.5, -8.0]),
            dummy("AC_TIME_0_L", [5.0, 2.0, 10.0]), dummy("AC_TIME_0_R", [-5.0, 2.0, 10.0]),
            dummy("AC_TIME_1_L", [5.0, 2.0, 50.0]), dummy("AC_TIME_1_R", [-5.0, 2.0, 50.0]),
        ]);
        let s = build(&l, &g);
        // duplicate slot 2 dropped, AC1 order kept, snapped to the ground
        assert_eq!(s.grid.len(), 2);
        assert_eq!(s.grid[0].pos, [0.0, 2.0 + CLEARANCE, 0.0]);
        // no AC_PIT: pits on the grid; no hotlap: grid 0
        assert_eq!(s.pits.len(), 2);
        assert_eq!(s.hotlap.len(), 1);
        // start / finish last, 16 m wide (10 + 2 x 3 m margin)
        assert_eq!(s.gates.iter().map(|x| x.n).collect::<Vec<_>>(), vec![1, 0]);
        let c = s.gates[1].corners;
        assert!((dist3(c[0], c[1]) - 16.0).abs() < 1e-3 && (dist3(c[1], c[2]) - 0.9).abs() < 1e-3);
        let mut b = SceneBuilder::new("t");
        let made = write(&mut b, "x", &s);
        assert_eq!(made, ["spawnpoints_grid_x", "spawnpoints_pitlane_x", "spawnpoints_hotlap_x", "timelines_x"]);
        assert_eq!(b.containers["timelines_x"].extra, ["timelines"]);
    }
}
