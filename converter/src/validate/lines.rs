//! Per session (a layout's base row + one mode row): the layout lines the
//! game looks up, and the spawn points' orientation.
//!
//! Game facts (ace.exe strings and the 17 stock tracks):
//! - the game looks the track limits up by NAME among the session's scene
//!   actors: no spline named `*track_limits_left*` logs "scene_actors
//!   collection doesn't contain track_limits_left splines, seems like we
//!   lack the right containers in this Gamemode", more than one "contains
//!   multiple track_limits_left splines". The container holding it, minus
//!   ".scene", is the layout path: `layouts\<stem>.ideal_line.aisplinedata`
//!   is loaded from it ("Loaded AISpline ... (cache loaded)"; its f5
//!   layout_path is that path). Without it the session has "no ideal line":
//!   "can't create simulated reference", "No simulated reference found for
//!   car", and the car-relative camera sequence of the pit menu sits at the
//!   world origin (under the ground on the converted Brands Hatch).
//! - every stock race layout's container also holds a spline whose name
//!   contains `ideal_line` (tagged `ideal_line`, or `center_spline` on
//!   monza / oulton / donington) and a `*track_limits_right*`.
//! - Start Pos rotations are Euler degrees, R = Rz Ry Rx, Y up, car forward
//!   local +Z (`track::scene::euler_of`); a container entity's transform
//!   places its whole scene. Stock grid, pit and hotlap starts are upright
//!   (up.y >= 0.98) and face along the nearest line (the ideal line, or the
//!   pit lane for pit slots); box spawns face into the garages.
//!
//! Ours (checks): see the constants below.

use std::collections::BTreeMap;

use super::decl::{Row, MODES};
use super::{entities, entity_kind, Report};
use crate::io::pb::Msg;
use crate::track::scene::matrix_of;

type M3 = [[f32; 3]; 3];

/// Ours: the largest tilt of an upright spawn, the largest heading error
/// (stock's worst: Donington's grid_start 29, 47 deg off the ideal line),
/// and how near a line a spawn must be for its heading to be checked (a
/// grid can be several rows wide off the line; stock pit slots sit 2..5 m
/// off their pit-lane spline).
const UPRIGHT_DEG: f32 = 25.0;
const FACING_DEG: f32 = 50.0;
const GRID_NEAR: f32 = 40.0;
const PIT_NEAR: f32 = 8.0;

#[derive(Clone, Copy)]
struct Xf {
    r: M3,
    t: [f32; 3],
}

impl Xf {
    const ID: Xf = Xf { r: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], t: [0.0; 3] };
    fn of(m: Option<Msg>) -> Xf {
        let Some(m) = m else { return Xf::ID };
        let v = |n: u32| -> [f32; 3] {
            let s = m.sub(n).unwrap_or_default();
            [s.f32(1).unwrap_or(0.0), s.f32(2).unwrap_or(0.0), s.f32(3).unwrap_or(0.0)]
        };
        Xf { r: matrix_of(v(2)), t: v(1) }
    }
    fn apply(&self, p: [f32; 3]) -> [f32; 3] {
        let mut o = self.t;
        for (i, oi) in o.iter_mut().enumerate() {
            *oi += self.r[i][0] * p[0] + self.r[i][1] * p[1] + self.r[i][2] * p[2];
        }
        o
    }
    fn then(&self, child: &Xf) -> Xf {
        let mut r = [[0f32; 3]; 3];
        for (i, row) in r.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                *v = (0..3).map(|k| self.r[i][k] * child.r[k][j]).sum();
            }
        }
        Xf { r, t: self.apply(child.t) }
    }
    fn col(&self, j: usize) -> [f32; 3] {
        [self.r[0][j], self.r[1][j], self.r[2][j]]
    }
}

struct Spline {
    name: String,
    container: String,
    pts: Vec<[f32; 3]>,
}

struct Spawn {
    name: String,
    container: String,
    xf: Xf,
    types: Vec<u64>,
}

/// The container entities of a main scene: lower-case scene path -> transform.
fn container_xfs(main: &[u8]) -> BTreeMap<String, Xf> {
    let mut out = BTreeMap::new();
    for e in entities(main) {
        if entity_kind(&e).as_deref() != Some("Container") {
            continue;
        }
        if let Some(p) = e.sub(50).and_then(|a| a.sub(121)).and_then(|c| c.str(1)) {
            out.insert(p.to_lowercase(), Xf::of(e.sub(4)));
        }
    }
    out
}

fn short(p: &str) -> &str {
    p.rsplit('\\').next().unwrap_or(p)
}

fn tangent_err(pts: &[[f32; 3]], p: [f32; 3], f: [f32; 3]) -> Option<(f32, f32)> {
    if pts.len() < 2 {
        return None;
    }
    let (mut bi, mut bd) = (0, f32::INFINITY);
    for (i, q) in pts.iter().enumerate() {
        let d = (q[0] - p[0]).powi(2) + (q[2] - p[2]).powi(2);
        if d < bd {
            bd = d;
            bi = i;
        }
    }
    let (a, b) = if bi + 1 < pts.len() { (pts[bi], pts[bi + 1]) } else { (pts[bi - 1], pts[bi]) };
    let t = [b[0] - a[0], b[2] - a[2]];
    let (tl, fl) = ((t[0] * t[0] + t[1] * t[1]).sqrt(), (f[0] * f[0] + f[2] * f[2]).sqrt());
    if tl < 1e-6 || fl < 1e-6 {
        return None;
    }
    let c = ((t[0] * f[0] + t[1] * f[2]) / (tl * fl)).clamp(-1.0, 1.0);
    Some((bd.sqrt(), c.acos().to_degrees()))
}

/// Check every (layout, mode) session of `track`. `main` = the track's
/// main scene, `read` returns a container / layouts file (None = missing).
/// `strict` (our own output) makes a missing line an error; stock layouts
/// without one (cruise / drift layouts) only warn.
pub fn check(track: &str, rows: &[Row], main: &[u8], read: &dyn Fn(&str) -> Option<Vec<u8>>, strict: bool, r: &mut Report) -> Vec<String> {
    let mut summary = Vec::new();
    let xfs = container_xfs(main);
    let mine: Vec<&Row> = rows.iter().filter(|x| x.track == track && !x.academy).collect();
    let mut layouts: Vec<&str> = mine.iter().map(|x| x.layout.as_str()).collect();
    layouts.sort();
    layouts.dedup();
    let mut cache: BTreeMap<String, Option<Vec<Msg>>> = BTreeMap::new();
    let mut checked_spawns: std::collections::BTreeSet<String> = Default::default();
    for l in layouts {
        let base: Vec<&&Row> = mine.iter().filter(|x| x.layout == l && x.base).collect();
        for (mode, _) in MODES {
            let Some(row) = mine.iter().find(|x| x.layout == l && match mode { "Race" => x.race, "Time Attack" => x.time_attack, _ => x.hotstint }) else { continue };
            let mut files: Vec<&String> = base.iter().flat_map(|b| b.containers.iter()).collect();
            files.extend(row.containers.iter());
            let mut splines: Vec<Spline> = Vec::new();
            let mut spawns: Vec<Spawn> = Vec::new();
            for f in files {
                let key = f.to_lowercase();
                if !key.ends_with(".scene") {
                    continue;
                }
                let ents = cache.entry(key.clone()).or_insert_with(|| read(f).map(|b| entities(&b))).clone();
                let Some(ents) = ents else { continue };
                let cx = xfs.get(&key).copied().unwrap_or(Xf::ID);
                for e in ents {
                    let name = e.str(1).unwrap_or_default();
                    let ex = cx.then(&Xf::of(e.sub(4)));
                    let actor = e.sub(50).unwrap_or_default();
                    if let Some(sp) = actor.sub(108) {
                        let pts = sp.sub(1).map(|d| d.subs(3)).unwrap_or_default().iter()
                            .map(|n| Xf::of(n.sub(1)).t).map(|p| ex.apply(p)).collect();
                        splines.push(Spline { name: name.to_lowercase(), container: f.clone(), pts });
                    } else if let Some(sp) = actor.sub(104) {
                        let types = sp.bytes(6).map(crate::io::pb::varints).unwrap_or_default();
                        spawns.push(Spawn { name, container: f.clone(), xf: ex, types });
                    }
                }
            }
            let what = format!("{track} / {l} / {mode}");
            let lefts: Vec<&Spline> = splines.iter().filter(|s| s.name.contains("track_limits_left")).collect();
            let rights = splines.iter().filter(|s| s.name.contains("track_limits_right")).count();
            let ideal: Option<&Spline> = lefts.first().and_then(|lc| splines.iter().find(|s| s.name.contains("ideal_line") && s.container == lc.container))
                .or_else(|| splines.iter().find(|s| s.name.contains("ideal_line")));
            let miss = |m: String, r: &mut Report| if strict { r.err(m) } else { r.warn(m) };
            match lefts.len() {
                0 => miss(format!("{what}: no spline named *track_limits_left* in the session's containers - the game finds no layout path: no AI spline, \"no ideal line\", no simulated reference, the pit-menu camera at the world origin"), r),
                1 => {}
                // stock Mullenbachschleife (a cruise layout) loads two
                n => miss(format!("{what}: {n} splines named *track_limits_left* (the game: \"contains multiple track_limits_left splines\")"), r),
            }
            if rights == 0 {
                miss(format!("{what}: no spline named *track_limits_right*"), r);
            }
            if ideal.is_none() {
                miss(format!("{what}: no spline named *ideal_line*"), r);
            }
            if let Some(lc) = lefts.first() {
                let stem = short(&lc.container).trim_end_matches(".scene").to_string();
                let dir = lc.container.rsplit_once("\\containers\\").map(|x| x.0.to_string()).unwrap_or_default();
                let ai = format!("{dir}\\layouts\\{stem}.ideal_line.aisplinedata");
                match read(&ai) {
                    None => miss(format!("{what}: no {ai} (the layout path of {})", short(&lc.container)), r),
                    Some(b) => {
                        let want = lc.container.trim_end_matches(".scene").to_lowercase();
                        let lp = Msg::parse(&b).ok().and_then(|m| m.str(5)).unwrap_or_default().to_lowercase();
                        if lp != want {
                            r.warn(format!("{what}: {} layout_path {lp:?}, the container is {want:?}", short(&ai)));
                        }
                    }
                }
            }
            // spawns: upright; a grid / hotlap start (type 3) faces along
            // the ideal line, a pit slot (type 4, no 3) within PIT_NEAR of a
            // pit-lane spline along it. Box-only spawns face their garage;
            // pit slots away from the pit line are parking places (stock
            // Touristenfahrten: 50 spots across a car park).
            let ideal_lines: Vec<&Spline> = splines.iter().filter(|s| s.name.contains("ideal_line") && s.pts.len() > 1).collect();
            let pit_lines: Vec<&Spline> = splines.iter().filter(|s| s.name.contains("pit") && s.pts.len() > 1).collect();
            let (mut n, mut bad) = (0, 0);
            for s in &spawns {
                let key = format!("{}|{}", s.container.to_lowercase(), s.name);
                if !checked_spawns.insert(key) {
                    continue;
                }
                n += 1;
                let up = s.xf.col(1);
                let fwd = s.xf.col(2);
                if up[1] < UPRIGHT_DEG.to_radians().cos() {
                    bad += 1;
                    r.err(format!("{what}: spawn {} in {} is not upright (up {:?}): the car starts tilted or upside down", s.name, short(&s.container), up.map(|v| (v * 100.0).round() / 100.0)));
                    continue;
                }
                let (lines, reach) = if s.types.contains(&3) || s.types.is_empty() {
                    (&ideal_lines, GRID_NEAR)
                } else if s.types.contains(&4) {
                    (&pit_lines, PIT_NEAR)
                } else {
                    continue;
                };
                let best = lines.iter().filter_map(|li| tangent_err(&li.pts, s.xf.t, fwd).map(|e| (e, li))).min_by(|a, b| a.0 .0.partial_cmp(&b.0 .0).unwrap());
                if let Some(((d, ang), li)) = best {
                    if ang > FACING_DEG && d < reach {
                        bad += 1;
                        r.err(format!("{what}: spawn {} in {} faces {ang:.0} deg off {} ({d:.0} m away) - the car starts backwards or sideways", s.name, short(&s.container), li.name));
                    }
                }
            }
            summary.push(format!("{what}: limits {}/{} ideal {} | {n} spawns checked, {bad} bad", lefts.len(), rights, ideal.map(|s| s.name.as_str()).unwrap_or("-")));
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stock_pit_spawn_world_heading() {
        // stock brands_hatch: spawnpoints container (-155.6, 143.6, -380.1)
        // rot (174.87, 67.33, 174.14), pit_spawn_0 local rot (-1.32, 2.27,
        // -1.78): heading atan2(x, z) 114.8 deg, upright
        let c = Xf { r: matrix_of([174.87396, 67.330864, 174.13829]), t: [-155.566, 143.569, -380.07] };
        let e = Xf { r: matrix_of([-1.3228488, 2.2741272, -1.7834023]), t: [-17.98, -0.904, 12.27] };
        let w = c.then(&e);
        let f = w.col(2);
        assert!(w.col(1)[1] > 0.99);
        assert!((f[0].atan2(f[2]).to_degrees() - 114.8).abs() < 0.5, "{f:?}");
    }
}
