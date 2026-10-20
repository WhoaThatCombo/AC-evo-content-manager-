//! The "Surface Def" entity: one SurfaceDefinitionData per AC1 surface key
//! a collision mesh uses.
//!
//! SurfaceDefinitionData (ACECM `protos\LogicScene.proto.txt`): f1
//! surfaceId, f2 collisionCategory (1 surface, 2 wall, 3 object), f3
//! gripMod, f6 sectorID, f7 dirtAdditiveK, f8 isValidTrack, f10 sinHeight,
//! f11 sinLength, f12 damping, f13 granularity, f14 vibrationGain, f15
//! vibrationLength, f16 wearMult, f18 frictionRain, f19 frictionRainGamma,
//! f20 surface_type (0 asphalt, 1 extra turf, 2 kerb, 3 grass, 4 sand,
//! 5 flat kerb). The SMesh entity names its surface by string id; ours is
//! the AC1 KEY verbatim.
//!
//! Every entry starts as a copy of a stock entry of the same kind (the
//! "factory"), read from the game's own Surface Def entities (oulton_park:
//! tarmac, grass, sand, kerb, curb, concrete, wall, plasticground; brands
//! hatch: PITS, OUT), with the same values built in for when the package
//! cannot be read. Then the AC1 values with an EVO meaning are applied
//! (ours, calibrated on the stock spread of each kind, `stockstats`):
//! - gripMod = factory grip x FRICTION / (the typical Kunos AC1 friction of
//!   that kind), clamped to the stock range of the kind (a kind with one
//!   stock value, grass 0.55, may rise to 0.60);
//! - SIN_HEIGHT clamped to the stock maximum of the kind (0.03 grass /
//!   sand, 0.01 kerb; at least 0.005 when AC1 asks for one), DAMPING and
//!   VIBRATION_GAIN to the stock maximum of the kind (sand damping 0.1);
//!   SIN_LENGTH, DIRT_ADDITIVE, VIBRATION_LENGTH, IS_VALID_TRACK copied;
//! - collisionCategory wall for walls (stock walls carry nothing else);
//! - surface_type from the kind, so kerbs sound like kerbs.
//! IS_PITLANE has no surface field in EVO: pit lanes are zones (track-ai-pits).

use std::collections::HashMap;

use super::classes::{surface_kind, Kind};
use super::scan::Surface;
use crate::io::kspkg::Pkg;
use crate::io::pb::{Msg, Val};

/// A factory entry: the stock values of one kind (f-number -> value).
#[derive(Clone, Debug)]
pub struct Factory {
    pub grip: f32,
    pub msg: Msg,
}

/// (stock track, surface id, kind) the factory is cloned from.
const STOCK_SOURCES: [(&str, &str, Kind); 10] = [
    ("oulton_park", "tarmac", Kind::Road), ("oulton_park", "grass", Kind::Grass), ("oulton_park", "sand", Kind::Sand),
    ("oulton_park", "kerb", Kind::Kerb), ("oulton_park", "curb", Kind::FlatKerb), ("oulton_park", "concrete", Kind::Concrete),
    ("oulton_park", "wall", Kind::Wall), ("oulton_park", "plasticground", Kind::Turf), ("brands_hatch", "PITS", Kind::Pit),
    ("brands_hatch", "OUT", Kind::Verge),
];

/// The same entries as measured on a stock content.kspkg, for when it
/// cannot be read: (kind, [(field, value)]), surface_type in f20.
fn builtin(kind: Kind) -> Vec<(u32, f32)> {
    match kind {
        Kind::Road => vec![(2, 1.0), (3, 0.98), (8, 1.0), (16, 1.0), (18, -1.0)],
        Kind::Grass => vec![(2, 1.0), (3, 0.55), (7, 1.0), (8, 1.0), (10, 0.03), (11, 0.5), (14, 0.1), (15, 0.6), (16, 0.8), (18, 0.55), (19, 1.0), (20, 3.0)],
        Kind::Sand => vec![(2, 1.0), (3, 0.94), (7, 1.0), (8, 1.0), (10, 0.03), (11, 0.4), (12, 0.1), (14, 0.1), (15, 0.6), (16, 1.5), (18, -1.0), (19, 1.0), (20, 4.0)],
        Kind::Kerb => vec![(2, 1.0), (3, 0.95), (8, 1.0), (14, 0.3), (15, 0.8), (16, 2.0), (18, 0.6), (19, 0.1), (20, 2.0)],
        Kind::FlatKerb => vec![(2, 1.0), (3, 0.95), (8, 1.0), (14, 0.1), (15, 0.5), (16, 2.0), (18, 0.6), (19, 0.1), (20, 5.0)],
        Kind::Concrete => vec![(2, 1.0), (3, 0.97), (8, 1.0), (16, 2.0), (18, 0.7), (19, 0.5)],
        Kind::Wall => vec![(2, 2.0)],
        Kind::Turf => vec![(2, 1.0), (3, 0.85), (7, 0.5), (8, 1.0), (14, 0.1), (15, 0.25), (16, 1.0), (18, 0.6), (19, 0.1), (20, 1.0)],
        Kind::Pit => vec![(2, 1.0), (3, 0.96), (8, 1.0), (16, 1.0), (18, -1.0), (19, 1.0)],
        Kind::Verge => vec![(2, 1.0), (3, 0.9), (16, 1.0), (18, -1.0), (19, 1.0)],
    }
}

const INT_FIELDS: [u32; 4] = [2, 6, 8, 20];

fn msg_of(fields: &[(u32, f32)]) -> Msg {
    let mut m = Msg::default();
    for &(f, v) in fields {
        if INT_FIELDS.contains(&f) {
            if v != 0.0 {
                m.set_varint(f, v as u64);
            }
        } else if v != 0.0 {
            m.set_f32(f, v);
        }
    }
    m
}

/// Stock grip range of each kind (min, max) over the 17 stock circuits
/// (road_atlanta excluded: locally replaced), `stockstats`.
pub fn stock_grip_range(k: Kind) -> (f32, f32) {
    match k {
        Kind::Road => (0.8, 1.1),
        Kind::Kerb => (0.7, 0.98),
        Kind::FlatKerb => (0.95, 0.97),
        Kind::Grass => (0.55, 0.55),
        Kind::Sand => (0.6, 0.94),
        Kind::Concrete => (0.9, 0.98),
        Kind::Pit => (0.96, 1.0),
        Kind::Verge => (0.9, 1.0),
        Kind::Turf => (0.65, 0.96),
        Kind::Wall => (0.0, 1.0),
    }
}

/// Stock maxima of (sin height, damping, vibration gain) per kind.
fn stock_max(k: Kind) -> (f32, f32, f32) {
    match k {
        Kind::Grass => (0.03, 0.0, 0.1),
        Kind::Sand => (0.03, 0.1, 0.1),
        Kind::Kerb => (0.01, 0.0, 0.6),
        Kind::FlatKerb => (0.0, 0.0, 0.15),
        Kind::Road => (0.025, 0.0, 0.5),
        _ => (0.0, 0.0, 0.1),
    }
}

/// Typical AC1 friction of a kind on the Kunos tracks of the AC1 install
/// (the value a "normal" surface of that kind has).
fn ac1_reference(k: Kind) -> f32 {
    match k {
        Kind::Road => 0.98,
        Kind::Kerb | Kind::FlatKerb => 0.93,
        Kind::Grass => 0.6,
        Kind::Sand => 0.8,
        Kind::Concrete => 0.9,
        Kind::Pit => 0.95,
        Kind::Verge => 0.92,
        Kind::Turf => 0.7,
        Kind::Wall => 1.0,
    }
}

/// The factory entries: cloned from the game's stock Surface Defs when
/// `pkg` can be read, else the built-in copy of the same values.
pub fn factory(pkg: Option<&Pkg>) -> HashMap<Kind, Factory> {
    let mut out: HashMap<Kind, Factory> = HashMap::new();
    if let Some(p) = pkg {
        let mut by_track: HashMap<&str, Vec<Msg>> = HashMap::new();
        for (t, _, _) in STOCK_SOURCES {
            if by_track.contains_key(t) {
                continue;
            }
            let want = format!("content\\tracks\\{t}\\{t}.scene");
            let entries = p.find(&want).and_then(|e| p.read(e).ok()).and_then(|b| Msg::parse(&b).ok()).map(|s| {
                s.subs(2).into_iter().filter_map(|e| e.sub(50).and_then(|a| a.sub(109))).flat_map(|d| d.subs(1)).collect::<Vec<Msg>>()
            }).unwrap_or_default();
            by_track.insert(t, entries);
        }
        for (t, id, k) in STOCK_SOURCES {
            if let Some(m) = by_track.get(t).and_then(|v| v.iter().find(|m| m.str(1).as_deref() == Some(id))) {
                let mut m = m.clone();
                m.remove(1);
                out.insert(k, Factory { grip: m.f32(3).unwrap_or(0.0), msg: m });
            }
        }
    }
    for (_, _, k) in STOCK_SOURCES {
        out.entry(k).or_insert_with(|| {
            let m = msg_of(&builtin(k));
            Factory { grip: m.f32(3).unwrap_or(0.0), msg: m }
        });
    }
    out
}

/// One converted entry and why (for the report).
#[derive(Clone, Debug)]
pub struct Converted {
    pub key: String,
    pub kind: Kind,
    pub grip: f32,
    pub msg: Msg,
}

fn surface_type(k: Kind) -> u64 {
    match k {
        Kind::Turf => 1,
        Kind::Kerb => 2,
        Kind::Grass => 3,
        Kind::Sand => 4,
        Kind::FlatKerb => 5,
        _ => 0,
    }
}

/// The entry of one AC1 surface.
pub fn convert(s: &Surface, fac: &HashMap<Kind, Factory>) -> Converted {
    let kind = surface_kind(s);
    let f = &fac[&kind];
    let mut m = Msg::default();
    m.set_str(1, &s.key);
    if kind == Kind::Wall {
        m.set_varint(2, 2);
        return Converted { key: s.key.clone(), kind, grip: 0.0, msg: m };
    }
    for fl in &f.msg.fields {
        m.fields.push(fl.clone());
    }
    let (glo, ghi) = stock_grip_range(kind);
    // inside the stock range of the kind; a kind every stock track gives one
    // value (grass 0.55) may rise 0.05 above it for the AC1 author's intent
    let (glo, ghi) = if ghi - glo < 0.01 { (glo, ghi + 0.05) } else { (glo, ghi) };
    let grip = (f.grip * s.friction / ac1_reference(kind)).clamp(glo, ghi);
    m.set_f32(3, grip);
    let (sin_max, damp_max, vib_max) = stock_max(kind);
    let put = |m: &mut Msg, n: u32, v: f32| {
        if v > 0.0 { m.set_f32(n, v) } else { m.remove(n) }
    };
    put(&mut m, 7, s.dirt_additive.clamp(0.0, 1.0));
    if s.valid_track { m.set_varint(8, 1) } else { m.remove(8) }
    let sin_cap = sin_max.max(if s.sin_height > 0.0 { 0.005 } else { 0.0 });
    put(&mut m, 10, s.sin_height.clamp(0.0, sin_cap));
    if s.sin_height > 0.0 {
        put(&mut m, 11, s.sin_length.clamp(0.05, 1.0));
    } else {
        m.remove(11);
    }
    put(&mut m, 12, s.damping.clamp(0.0, damp_max));
    put(&mut m, 14, s.vibration_gain.clamp(0.0, vib_max));
    if s.vibration_gain > 0.0 {
        put(&mut m, 15, s.vibration_length.clamp(0.0, 1.0));
    }
    let st = surface_type(kind);
    if st > 0 { m.set_varint(20, st) } else { m.remove(20) }
    m.set_varint(2, 1);
    // keep field order stable: surfaceId first, the rest by number (as stock)
    let id = m.fields.remove(0);
    m.fields.sort_by_key(|x| x.num);
    m.fields.insert(0, id);
    Converted { key: s.key.clone(), kind, grip, msg: m }
}

/// The SurfaceDefinitionActorData (f50.f109) for the given surfaces.
pub fn actor_data(entries: &[Converted]) -> Msg {
    let mut d = Msg::default();
    for e in entries {
        d.push(1, Val::Msg(e.msg.clone()));
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(key: &str, wav: &str, friction: f32) -> Surface {
        Surface { key: key.into(), wav: wav.into(), friction, ..Default::default() }
    }

    #[test]
    fn convert_kinds() {
        let fac = factory(None);
        let road = convert(&s("ROAD", "", 0.98), &fac);
        assert_eq!(road.kind, Kind::Road);
        assert!((road.grip - 0.98).abs() < 1e-4);
        let mut g = s("GRASS", "grass.wav", 0.6);
        g.sin_height = 0.03;
        g.sin_length = 0.5;
        g.dirt_additive = 1.0;
        let grass = convert(&g, &fac);
        assert_eq!(grass.msg.varint(20), Some(3));
        assert!((grass.grip - 0.55).abs() < 1e-4);
        assert_eq!(grass.msg.f32(10), Some(0.03));
        // a grippier AC1 grass stays inside the widened stock grass range
        assert!(convert(&s("ERBA", "grass.wav", 0.85), &fac).grip <= 0.60 + 1e-6);
        assert!(convert(&s("GRASS", "grass.wav", 0.5), &fac).grip >= 0.55 - 1e-6);
        // concrete never above the stock maximum
        assert!(convert(&s("CONCR", "", 0.95), &fac).grip <= 0.98 + 1e-6);
        let w = convert(&s("WALL", "", 1.0), &fac);
        assert_eq!(w.msg.varint(2), Some(2));
        assert_eq!(w.msg.fields.len(), 2);
        let k = convert(&s("KERB", "kerb.wav", 0.95), &fac);
        assert_eq!(k.msg.varint(20), Some(5)); // no sin, low vibration -> flat kerb
        assert_eq!(k.msg.str(1).as_deref(), Some("KERB"));
    }
}
