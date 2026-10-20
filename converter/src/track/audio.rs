//! Audio zones: `containers\fmod_<id>_track.scene` ("Zone Sound" entities).
//!
//! # Format (stock, 18 fmod containers)
//!
//! f50.f125 ZoneSoundData: f1 zone type (0 ambient, 1 reverb), f6 bank,
//! f7 event or snapshot, f10 area type (0 sphere, 1 polygon), f11 radius,
//! f12 scale {x, y}, f13 offset, f14 max volume, f15 polygon (local corner
//! transforms, f21 colour {1, 1, 0, 1}, f22 height), f22 only focused car,
//! f24 use fading, f30 debug {0.05, 6}, f21 = 1 (not in the descriptor;
//! kept as stock writes it).
//! - Ambience: one per track, `event:/environment/track_ambience` in
//!   `content\sfx\environment.bank`, a sphere of radius 1250..10000 m
//!   (scale 0.7, 0.7; max volume 1; a dummy 20 m polygon).
//! - Reverb / underpass: `snapshot:/snap_reverb` / `snapshot:/snap_underpass`
//!   in `content\sfx\master_bank.bank`, type 1, polygon, radius 10, max
//!   volume 0.7..0.8 (reverb) / 1.0 (underpass), polygon heights 25..75 m
//!   (reverb) / 3.9..10 m (underpass), entity rotated to the track.
//! The main scene's Container entity declares `Zone Sound` and is not
//! loaded on demand; no declaration row lists the container.
//!
//! # Ours
//!
//! - Ambience: centred on the physics ground's box, radius half its
//!   diagonal + 200 m (1250..10000).
//! - One zone per enabled AC1 `[REVERB_n]` whose NODE exists (an `AC_*`
//!   dummy, else a kn5 mesh of that name: its box centre): an underpass
//!   when the AC1 preset or node name says tunnel / underpass / bridge or
//!   MAXDISTANCE < 60 m (a 40 x 40 m polygon, 10 m high), else a reverb
//!   (a square of side MINDISTANCE clamped to 40..300 m, 25 m high),
//!   turned to the ideal line's heading nearest to it.

use super::ai::Line;
use super::scan::Scan;
use super::scene::{transform, v3, Entity, SceneBuilder};
use crate::io::pb::{Msg, Val};

pub const ENV_BANK: &str = "content\\sfx\\environment.bank";
pub const MASTER_BANK: &str = "content\\sfx\\master_bank.bank";

#[derive(Clone, Debug)]
pub struct Reverb {
    pub name: String,
    pub at: [f32; 3],
    pub underpass: bool,
    pub half: f32,
    pub height: f32,
}

/// The enabled AC1 reverbs of a layout whose anchor exists.
pub fn reverbs(scan: &Scan, layout: usize) -> (Vec<Reverb>, Vec<String>) {
    let l = &scan.layouts[layout];
    let mut out = Vec::new();
    let mut missing = Vec::new();
    let Some(ini) = l.data.get("audio_sources.ini") else { return (out, missing) };
    for (sec, s) in ini.with_prefix("REVERB_") {
        if s.get("ENABLED").map(|v| v.trim() == "0").unwrap_or(false) {
            continue;
        }
        let Some(node) = s.get("NODE").map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) else { continue };
        let up = node.to_uppercase();
        let at = l.dummies.iter().find(|d| d.name.to_uppercase() == up).map(|d| d.position()).or_else(|| {
            scan.meshes.iter().filter(|m| m.layouts & (1 << layout) != 0 && m.name.to_uppercase() == up).find_map(|m| {
                let p = scan.positions(m);
                if p.is_empty() {
                    return None;
                }
                let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
                for v in p.iter() {
                    for k in 0..3 {
                        lo[k] = lo[k].min(v[k]);
                        hi[k] = hi[k].max(v[k]);
                    }
                }
                Some([(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0])
            })
        });
        let Some(at) = at else {
            missing.push(format!("{sec} ({node})"));
            continue;
        };
        let f = |k: &str| s.get(k).and_then(|v| crate::ac1::ini::num_of(v)).map(|x| x as f32);
        let preset = s.get("PRESET").cloned().unwrap_or_default().to_uppercase();
        let words = ["TUNNEL", "UNDERPASS", "BRIDGE", "PIPE", "SEWER"];
        let underpass = words.iter().any(|w| preset.contains(w) || up.contains(w)) || f("MAXDISTANCE").map(|d| d < 60.0).unwrap_or(false);
        let (half, height) = if underpass { (20.0, 10.0) } else { (f("MINDISTANCE").unwrap_or(150.0).clamp(40.0, 300.0) / 2.0, 25.0) };
        out.push(Reverb { name: format!("fmod_snap_{}_{}", if underpass { "underpass" } else { "reverb" }, sec.trim_start_matches("REVERB_")), at, underpass, half, height });
    }
    (out, missing)
}

fn polygon(half: f32, height: f32) -> Msg {
    let mut g = Msg::default();
    for (x, z) in [(-half, half), (half, half), (half, -half), (-half, -half)] {
        g.push(1, Val::Msg(transform([x, 0.0, z], [0.0; 3], [1.0; 3])));
    }
    let mut col = Msg::default();
    col.set_f32(1, 1.0);
    col.set_f32(2, 1.0);
    col.set_f32(4, 1.0);
    g.push(21, Val::Msg(col));
    g.set_f32(22, height);
    g
}

fn debug() -> Msg {
    let mut d = Msg::default();
    d.set_f32(1, 0.05);
    d.set_varint(3, 6);
    d
}

fn scale(x: f32, y: f32) -> Msg {
    let mut s = Msg::default();
    s.set_f32(1, x);
    s.set_f32(2, y);
    s
}

/// Write `fmod_<id>_track` (loaded with the main scene); returns its stem.
pub fn write(b: &mut SceneBuilder, id: &str, bounds: ([f32; 3], [f32; 3]), reverbs: &[Reverb], line: Option<&Line>) -> String {
    let c = format!("fmod_{id}_track");
    b.container(&c, false);
    let (lo, hi) = bounds;
    let centre = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
    let diag = ((hi[0] - lo[0]).powi(2) + (hi[2] - lo[2]).powi(2)).sqrt();
    let radius = (diag / 2.0 + 200.0).clamp(1250.0, 10000.0);
    let mut z = Msg::default();
    z.set_str(6, ENV_BANK);
    z.set_str(7, "event:/environment/track_ambience");
    z.set_f32(11, radius);
    z.push(12, Val::Msg(scale(0.7, 0.7)));
    z.set_f32(14, 1.0);
    z.push(15, Val::Msg(polygon(10.0, 3.0)));
    z.push(30, Val::Msg(debug()));
    z.set_varint(21, 1);
    let mut e = Entity::new("fmod_ambient", "Zone Sound", 125, z);
    e.position = centre;
    e.distance_override = Some(100000.0);
    e.bounds = Some(([-radius, -radius], [radius, radius]));
    b.add(Some(&c), e);
    for r in reverbs {
        let mut z = Msg::default();
        z.set_varint(1, 1);
        z.set_str(6, MASTER_BANK);
        z.set_str(7, if r.underpass { "snapshot:/snap_underpass" } else { "snapshot:/snap_reverb" });
        z.set_varint(10, 1);
        z.set_f32(11, 10.0);
        z.push(12, Val::Msg(scale(0.7, 0.7)));
        z.push(13, Val::Msg(Msg::default()));
        z.set_f32(14, if r.underpass { 1.0 } else { 0.8 });
        z.push(15, Val::Msg(polygon(r.half, r.height)));
        z.set_varint(22, 1);
        z.set_varint(24, 1);
        z.push(30, Val::Msg(debug()));
        z.set_varint(21, 1);
        let mut e = Entity::new(&r.name, "Zone Sound", 125, z);
        e.position = r.at;
        if let Some(l) = line.filter(|l| !l.samples.is_empty()) {
            let (i, _) = l.nearest(r.at);
            let f = l.samples[i].fwd;
            e.rotation = [0.0, f[0].atan2(f[2]).to_degrees(), 0.0];
        }
        e.bounds = Some(([-r.half * 1.5, -r.half * 1.5], [r.half * 1.5, r.half * 1.5]));
        b.add(Some(&c), e);
    }
    let _ = v3;
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ambience_and_reverb() {
        let mut b = SceneBuilder::new("t");
        let r = Reverb { name: "fmod_snap_reverb_0".into(), at: [10.0, 2.0, 3.0], underpass: false, half: 75.0, height: 25.0 };
        let c = write(&mut b, "t", ([-500.0, 0.0, -300.0], [500.0, 50.0, 300.0]), &[r], None);
        assert_eq!(c, "fmod_t_track");
        let s = &b.containers[c.as_str()];
        assert!(!s.on_demand);
        assert_eq!(s.scene.entities.len(), 2);
        let amb = &s.scene.entities[0].1;
        assert_eq!(amb.actor.1.f32(11), Some(1250.0));
        assert_eq!(s.scene.entities[1].1.actor.1.str(7).as_deref(), Some("snapshot:/snap_reverb"));
    }
}
