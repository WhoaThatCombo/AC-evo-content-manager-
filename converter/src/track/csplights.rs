//! CSP track lights -> EVO Light / InstancedLight entities (requirement
//! track-csp-lights).
//!
//! Inputs: the CSP config's `[LIGHT_*]` (POSITION or LINE_FROM / LINE_TO,
//! DIRECTION, SPOT, SPOT_SHARPNESS, RANGE, COLOR, CONDITION, ACTIVE) and
//! `[LIGHT_SERIES_*]` (one light per cluster of vertices of MESHES /
//! MATERIALS, OFFSET, DIRECTION = NORMAL or a vector, CLUSTER_THRESHOLD).
//! Only lights whose condition is static at night (or none) are converted
//! (look.rs conditions); flashing ones and seasonal / weather ones are
//! skipped and reported.
//!
//! EVO side (Scene.proto LightData, InstancedLightData; measured on the
//! stock scenes of 17 circuits, 1,659 Light and 34 InstancedLight entities):
//! - LightData f1 type (0 point, 1 spot: 96 % of stock), f2 colour (r, g,
//!   b, 0), f3 intensity, f4 radius, f5 / f6 inner / outer angle (degrees;
//!   stock outer 50..70), f14 specular scale 1, f21 falloff exponent (stock
//!   median 2), f7 cast shadow, f9 / f10 / f16 shadow (ESM) settings,
//!   f19 volumetric scattering;
//! - spot beams point along the light's local -Y: stock spots with a zero
//!   rotation hang from poles, the tilted ones (x = +-60..90 degrees) aim
//!   at or below the horizon;
//! - intensity by radius (stock p50): under 40 m about 5, 40..80 m 75,
//!   80 m and more 150. Ours: that median x sqrt(CSP brightness / 0.045)
//!   (0.045 = the library's median CSP COLOR level), clamped 0.3..3 x;
//! - InstancedLight: f1 max distance (stock 250..850), f2 fade distance
//!   (50..100), f3 the light, f4 instance set {f2 positions, f3 rotations,
//!   f4 scales} as packed f32 x 3 (the ISMesh layout); stock uses it for
//!   many small far lights (10..70 instances each). Ours: a CSP light group
//!   of 8 or more lights becomes one InstancedLight;
//! - at most 200 single lights and 1,000 instanced ones per track (stock
//!   p90 144 / max 221 Light entities; instanced series thinned evenly).
//! They go into the always-loaded container `decor_lights`, which declares
//! both kinds.

use super::look::csp_colour;
use super::scan::Scan;
use super::scene::{Entity, SceneBuilder};
use crate::ac1::ini::{self, Ini};
use crate::io::pb::{self, Msg, Val};

pub const CONTAINER: &str = "decor_lights";
const MAX_SINGLE: usize = 200;
const INSTANCE_MIN: usize = 8;
/// At most this many instanced lights per track (ours: stock tracks carry
/// up to about 700 in all); bigger CSP series are thinned evenly.
const MAX_INSTANCES: usize = 1000;
const CSP_MEDIAN: f32 = 0.045;

#[derive(Clone, Debug)]
pub struct Lamp {
    pub pos: [f32; 3],
    /// beam direction (unit), None = point light
    pub dir: Option<[f32; 3]>,
    pub colour: [f32; 3],
    /// CSP brightness (peak colour x mult)
    pub level: f32,
    pub range: f32,
    /// full cone angle (degrees)
    pub spot: f32,
    pub sharpness: f32,
    pub shadows: bool,
    pub volumetric: bool,
}

pub struct Group {
    pub name: String,
    pub lamps: Vec<Lamp>,
}

pub struct Report {
    pub singles: usize,
    pub instanced: usize,
    pub instances: usize,
    pub skipped: Vec<String>,
}

fn v3(s: &Ini, sec: &str, key: &str) -> Option<[f32; 3]> {
    let n = s.nums(sec, key);
    (n.len() >= 3).then(|| [n[0] as f32, n[1] as f32, n[2] as f32])
}

fn unit(v: [f32; 3]) -> Option<[f32; 3]> {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    (l > 1e-6 && l.is_finite()).then(|| [v[0] / l, v[1] / l, v[2] / l])
}

/// Intensity for a range (ours, calibrated on the stock medians by radius).
pub fn intensity(range: f32, level: f32) -> f32 {
    let base = if range < 40.0 { 5.0 } else if range < 80.0 { 75.0 } else { 150.0 };
    base * (level / CSP_MEDIAN).max(0.0).sqrt().clamp(0.3, 3.0)
}

/// Read every convertible CSP light, grouped by section.
pub fn read(scan: &Scan) -> (Vec<Group>, Vec<String>) {
    let mut groups = Vec::new();
    let mut skipped = Vec::new();
    let Some((_, c)) = scan.csp.as_ref() else { return (groups, skipped) };
    let night = |sec: &str| -> bool {
        let cond = c.get(sec, "CONDITION").map(|v| v.trim().to_ascii_uppercase()).unwrap_or_default();
        if cond.is_empty() {
            return true;
        }
        // a named condition of this file: flashing ones are not static
        for (_, s) in c.with_prefix("CONDITION") {
            if s.get("NAME").map(|n| n.trim().eq_ignore_ascii_case(&cond)).unwrap_or(false) {
                let f = s.get("FLASHING_FREQUENCY").and_then(|v| ini::num_of(v)).unwrap_or(0.0);
                let input = s.get("INPUT").map(|v| v.to_ascii_uppercase()).unwrap_or_default();
                return f < 0.05 && (input.is_empty() || input.contains("SUN") || input.contains("NIGHT"));
            }
        }
        !(cond.contains("SEASON") || cond.contains("RAIN") || cond.contains("FLASH") || cond.contains("BLINK") || cond.contains("HAZARD") || cond.contains("FLAG"))
    };
    let common = |sec: &str| -> Option<(Lamp, Option<String>)> {
        if c.get(sec, "ACTIVE").map(|v| v.trim() == "0").unwrap_or(false) {
            return None;
        }
        let (colour, level) = c.get(sec, "COLOR").and_then(csp_colour)?;
        if level <= 0.0 {
            return None;
        }
        let range = c.num(sec, "RANGE").unwrap_or(20.0).clamp(1.0, 300.0) as f32;
        let spot = c.num(sec, "SPOT").unwrap_or(0.0).clamp(0.0, 179.0) as f32;
        let sharp = c.num(sec, "SPOT_SHARPNESS").unwrap_or(0.5).clamp(0.0, 1.0) as f32;
        let dir_key = c.get(sec, "DIRECTION").map(|v| v.trim().to_ascii_uppercase());
        let shadows = c.get(sec, "SHADOWS").map(|v| v.trim() == "1").unwrap_or(false) || c.section(sec).map(|s| s.contains_key("SHADOWS_SPOT")).unwrap_or(false);
        let vol = c.get(sec, "VOLUMETRIC_LIGHT").map(|v| v.trim() == "1").unwrap_or(false);
        Some((Lamp { pos: [0.0; 3], dir: None, colour, level, range, spot, sharpness: sharp, shadows, volumetric: vol }, dir_key))
    };
    for (sec, _) in c.with_prefix("LIGHT_") {
        if sec.starts_with("LIGHT_SERIES") || sec.starts_with("LIGHT_POLLUTION") {
            continue;
        }
        let Some((mut lamp, dk)) = common(sec) else { continue };
        if !night(sec) {
            skipped.push(sec.to_string());
            continue;
        }
        let dir = dk.as_deref().and_then(|_| v3(c, sec, "DIRECTION")).and_then(unit);
        let dir = if lamp.spot > 0.0 { Some(dir.unwrap_or([0.0, -1.0, 0.0])) } else { None };
        let mut lamps = Vec::new();
        if let (Some(a), Some(b)) = (v3(c, sec, "LINE_FROM"), v3(c, sec, "LINE_TO")) {
            // a line light: points every half range along it, at most 8 (ours)
            let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2) + (b[2] - a[2]).powi(2)).sqrt();
            let n = ((len / (lamp.range * 0.5).max(1.0)).ceil() as usize).clamp(1, 8);
            for i in 0..n {
                let t = (i as f32 + 0.5) / n as f32;
                lamps.push(Lamp { pos: [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t], dir, ..lamp.clone() });
            }
        } else if let Some(p) = v3(c, sec, "POSITION") {
            lamp.pos = p;
            lamp.dir = dir;
            lamps.push(lamp);
        }
        if !lamps.is_empty() {
            groups.push(Group { name: sec.to_lowercase(), lamps });
        }
    }
    for (sec, _) in c.with_prefix("LIGHT_SERIES") {
        let Some((lamp, dk)) = common(sec) else { continue };
        if !night(sec) {
            skipped.push(sec.to_string());
            continue;
        }
        let meshes = c.get(sec, "MESHES").map(|s| s.to_string()).unwrap_or_default();
        let mats = c.get(sec, "MATERIALS").map(|s| s.to_string()).unwrap_or_default();
        if meshes.is_empty() && mats.is_empty() {
            continue;
        }
        let offset = v3(c, sec, "OFFSET").unwrap_or([0.0; 3]);
        let by_normal = dk.as_deref() == Some("NORMAL");
        let fixed = if by_normal { None } else { v3(c, sec, "DIRECTION").and_then(unit) };
        let cluster = c.num(sec, "CLUSTER_THRESHOLD").unwrap_or(1.0).max(0.2) as f32;
        // vertices of the matching meshes, clustered on a grid of the threshold
        let mut cells: std::collections::HashMap<(i32, i32, i32), ([f64; 3], [f64; 3], u32)> = std::collections::HashMap::new();
        for (mi, m) in scan.meshes.iter().enumerate() {
            let names = super::look::names_of(scan, mi);
            let hit = (!meshes.is_empty() && names.iter().any(|n| super::look::name_match(&meshes, n)))
                || (!mats.is_empty() && super::look::name_match(&mats, &scan.material(m.material).name));
            if !hit {
                continue;
            }
            let mesh = scan.mesh(m);
            let pos = scan.positions(m);
            let w = &m.world;
            for (v, p) in pos.iter().enumerate() {
                let n = mesh.normal(v);
                let wn = [n[0] * w[0] + n[1] * w[4] + n[2] * w[8], n[0] * w[1] + n[1] * w[5] + n[2] * w[9], n[0] * w[2] + n[1] * w[6] + n[2] * w[10]];
                let k = ((p[0] / cluster).floor() as i32, (p[1] / cluster).floor() as i32, (p[2] / cluster).floor() as i32);
                let e = cells.entry(k).or_insert(([0.0; 3], [0.0; 3], 0));
                for i in 0..3 {
                    e.0[i] += p[i] as f64;
                    e.1[i] += wn[i] as f64;
                }
                e.2 += 1;
            }
        }
        let mut keys: Vec<_> = cells.keys().copied().collect();
        keys.sort();
        let lamps: Vec<Lamp> = keys.iter().map(|k| {
            let (s, n, cnt) = cells[k];
            let c = [s[0] / cnt as f64, s[1] / cnt as f64, s[2] / cnt as f64].map(|x| x as f32);
            let dir = if lamp.spot > 0.0 {
                Some(fixed.or_else(|| unit(n.map(|x| x as f32))).unwrap_or([0.0, -1.0, 0.0]))
            } else {
                None
            };
            Lamp { pos: [c[0] + offset[0], c[1] + offset[1], c[2] + offset[2]], dir, ..lamp.clone() }
        }).collect();
        if !lamps.is_empty() {
            groups.push(Group { name: sec.to_lowercase(), lamps });
        }
    }
    (groups, skipped)
}

/// Euler (degrees) of a rotation taking local -Y onto `dir`.
fn rotation_for(dir: [f32; 3]) -> [f32; 3] {
    // forward (+Z) perpendicular to the beam, up (+Y) = -beam
    let up = [-dir[0], -dir[1], -dir[2]];
    let fwd = if up[1].abs() > 0.99 { [0.0, 0.0, 1.0] } else {
        let f = [up[0] * up[1], up[1] * up[1] - 1.0, up[2] * up[1]];
        unit(f).unwrap_or([0.0, 0.0, 1.0])
    };
    super::scene::euler_of(fwd, up)
}

fn light_data(l: &Lamp) -> Msg {
    let mut d = Msg::default();
    if l.dir.is_some() {
        d.set_varint(1, 1);
    }
    let mut col = Msg::default();
    for (i, x) in l.colour.iter().enumerate() {
        if *x != 0.0 {
            col.set_f32(i as u32 + 1, *x);
        }
    }
    d.push(2, Val::Msg(col));
    d.set_f32(3, intensity(l.range, l.level));
    d.set_f32(4, l.range);
    if l.dir.is_some() {
        let outer = (l.spot * 0.5).clamp(5.0, 80.0);
        d.set_f32(5, (outer * l.sharpness.clamp(0.1, 0.95)).max(1.0));
        d.set_f32(6, outer);
    }
    if l.shadows {
        d.set_varint(7, 1);
        d.set_f32(9, 0.985);
        d.set_f32(10, 2.5);
        d.set_f32(16, 2.0);
    }
    d.set_f32(14, 1.0);
    if l.volumetric {
        d.set_f32(19, 50.0);
    }
    d.set_f32(21, 2.0);
    d
}

/// Add the lights to the builder's `decor_lights` container.
pub fn add(b: &mut SceneBuilder, groups: &[Group]) -> Report {
    let mut rep = Report { singles: 0, instanced: 0, instances: 0, skipped: Vec::new() };
    if groups.is_empty() {
        return rep;
    }
    b.container(CONTAINER, false);
    b.declare(CONTAINER, "Light");
    b.declare(CONTAINER, "InstancedLight");
    let total: usize = groups.iter().filter(|g| g.lamps.len() >= INSTANCE_MIN).map(|g| g.lamps.len()).sum();
    let stride = total.div_ceil(MAX_INSTANCES).max(1);
    if stride > 1 {
        rep.skipped.push(format!("{total} series lights thinned to every {stride}th"));
    }
    for g in groups {
        if g.lamps.len() >= INSTANCE_MIN {
            let lamps: Vec<&Lamp> = g.lamps.iter().step_by(stride).collect();
            let l = lamps[0];
            let mut pos = Vec::new();
            let mut rot = Vec::new();
            let mut scale = Vec::new();
            let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
            for x in &lamps {
                pos.extend_from_slice(&x.pos);
                rot.extend_from_slice(&x.dir.map(rotation_for).unwrap_or([0.0; 3]));
                scale.extend_from_slice(&[1.0, 1.0, 1.0]);
                lo = [lo[0].min(x.pos[0] - x.range), lo[1].min(x.pos[2] - x.range)];
                hi = [hi[0].max(x.pos[0] + x.range), hi[1].max(x.pos[2] + x.range)];
            }
            let mut set = Msg::default();
            set.push(2, Val::Bytes(pb::f32_bytes(&pos)));
            set.push(3, Val::Bytes(pb::f32_bytes(&rot)));
            set.push(4, Val::Bytes(pb::f32_bytes(&scale)));
            let mut d = Msg::default();
            d.set_f32(1, 450.0);
            d.set_f32(2, 50.0);
            d.push(3, Val::Msg(light_data(l)));
            d.push(4, Val::Msg(set));
            let mut e = Entity::new(&g.name, "InstancedLight", 139, d);
            e.bounds = Some((lo, hi));
            b.add(Some(CONTAINER), e);
            rep.instanced += 1;
            rep.instances += lamps.len();
        } else {
            for (i, l) in g.lamps.iter().enumerate() {
                if rep.singles >= MAX_SINGLE {
                    rep.skipped.push(format!("{} (over {MAX_SINGLE} lights)", g.name));
                    break;
                }
                let mut e = Entity::new(&format!("{}_{i}", g.name), "Light", 101, light_data(l));
                e.position = l.pos;
                e.rotation = l.dir.map(rotation_for).unwrap_or([0.0; 3]);
                e.bounds = Some(([l.pos[0] - l.range, l.pos[2] - l.range], [l.pos[0] + l.range, l.pos[2] + l.range]));
                b.add(Some(CONTAINER), e);
                rep.singles += 1;
            }
        }
    }
    rep
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(m: [[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
        [0, 1, 2].map(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
    }

    #[test]
    fn beam_along_local_minus_y() {
        for d in [[0.0, -1.0, 0.0], [0.6, -0.8, 0.0], [0.0, -0.2, 0.98], [-0.7, -0.1, -0.7]] {
            let d = unit(d).unwrap();
            let m = super::super::scene::matrix_of(rotation_for(d));
            let b = apply(m, [0.0, -1.0, 0.0]);
            assert!((0..3).all(|k| (b[k] - d[k]).abs() < 1e-3), "{d:?} -> {b:?}");
        }
        assert!((intensity(20.0, 0.045) - 5.0).abs() < 1e-4 && intensity(100.0, 10.0) <= 450.0);
    }
}
