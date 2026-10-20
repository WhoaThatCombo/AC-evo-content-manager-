//! DRS zones: AC1 `data\drs_zones.ini` -> `containers\drs_zones_<L>.scene`.
//!
//! # Format (stock, 11 DRS containers on 8 tracks)
//!
//! One Zone entity (f50.f112, debug {0.05, 3}) per container with one
//! ZoneGeometricData per DRS zone (4..11 corners each) and the instance
//! f118 DRSZone {} (an empty message). Stock places factory signs
//! (`common_assets\meshes\props\track_dist\dist_drs.mesh`) and a decal
//! line next to it; the container is listed with the layout in Race, Time
//! Attack and Hotstint, and its Container entity declares no kinds (we
//! declare `Zone`, which is what it holds).
//!
//! # Ours
//!
//! - A zone spans START..END of the AC1 lap fractions on the ideal line,
//!   across the track: the line's kerb sides + 1 m each way.
//! - Corners: both edges every 10 m, Douglas-Peucker at 0.75 m, at most 6
//!   corners an edge (12 in all; stock goes to 11).
//! - DETECTION is not written: the game's DRSZone carries no detection
//!   point. No props are placed.

use super::ai::Line;
use super::pits::{strip, zone_entity};
use super::scene::SceneBuilder;
use crate::ac1::ini::Ini;
use crate::io::pb::{Msg, Val};

#[derive(Clone, Copy, Debug)]
pub struct Zone {
    pub detection: f32,
    pub start: f32,
    pub end: f32,
}

pub fn read(ini: &Ini) -> Vec<Zone> {
    ini.with_prefix("ZONE_").filter_map(|(_, s)| {
        let f = |k: &str| s.get(k).and_then(|v| crate::ac1::ini::num_of(v)).map(|x| x as f32);
        let (st, en) = (f("START")?, f("END")?);
        ((0.0..=1.0).contains(&st) && (0.0..=1.0).contains(&en) && st != en).then(|| Zone { detection: f("DETECTION").unwrap_or(st), start: st, end: en })
    }).collect()
}

/// The polygon of one zone.
pub fn polygon(z: &Zone, line: &Line) -> Vec<[f32; 3]> {
    let n = line.samples.len();
    let a = line.samples.partition_point(|s| s.dist < z.start * line.length).min(n - 1);
    let b = line.samples.partition_point(|s| s.dist < z.end * line.length).min(n - 1);
    let width = |s: &super::ai::Sample| [s.curbs[0] + 1.0, s.curbs[1] + 1.0];
    if a <= b {
        let p = strip(line, a, b, &width, 10.0);
        squeeze(p)
    } else {
        // the zone crosses the start line: rotate a copy of the line
        let mut l2 = line.clone();
        l2.samples.rotate_left(a);
        let off = l2.samples[0].dist;
        for s in &mut l2.samples {
            s.dist = (s.dist - off).rem_euclid(line.length);
        }
        let p = strip(&l2, 0, (b + n - a).min(n - 1), &width, 10.0);
        squeeze(p)
    }
}

/// At most 6 corners an edge.
fn squeeze(p: Vec<[f32; 3]>) -> Vec<[f32; 3]> {
    if p.len() <= 12 {
        return p;
    }
    let h = p.len() / 2;
    let (l, r) = p.split_at(h);
    let pick = |v: &[[f32; 3]]| -> Vec<[f32; 3]> { (0..6).map(|k| v[k * (v.len() - 1) / 5]).collect() };
    let mut out = pick(l);
    out.extend(pick(r));
    out
}

/// Write `drs_zones_<stem>`; returns the container stem.
pub fn write(b: &mut SceneBuilder, stem: &str, zones: &[Zone], line: &Line) -> Option<String> {
    if zones.is_empty() || line.samples.len() < 4 || !line.closed {
        return None;
    }
    let polys: Vec<Vec<[f32; 3]>> = zones.iter().map(|z| polygon(z, line)).filter(|p| p.len() >= 4).collect();
    if polys.is_empty() {
        return None;
    }
    let c = format!("drs_zones_{stem}");
    let mut inst = Msg::default();
    inst.push(118, Val::Msg(Msg::default()));
    b.add(Some(&c), zone_entity("drs_zones", &polys, vec![inst]));
    Some(c)
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_zones() {
        let ini = crate::ac1::ini::parse("[ZONE_0]\nDETECTION=0.325\nSTART=0.350\nEND=0.500\n[ZONE_1]\nSTART=0.9\nEND=0.05\n");
        let z = super::read(&ini);
        assert_eq!(z.len(), 2);
        assert_eq!((z[0].detection, z[0].start, z[0].end), (0.325, 0.35, 0.5));
    }
}
