//! AC1 -> the source representation: car.ini, suspensions.ini, tyres.ini,
//! engine.ini, drivetrain.ini, brakes.ini, aero.ini and the car frame.
//!
//! Defaults for absent keys are the converter's long-standing ones (the
//! physics stages used them before this representation existed).

use super::*;
use crate::ac1::car::Car;
use crate::ac1::ini::{self, Ini};
use crate::ac1::tyre::Ac1Tyre;
use crate::car::frame::Frame;
use crate::car::physics::{ac1_v3, G, KG_PER_LITRE};

/// AC1 rims are one inch over nominal (tyres.ini: "use 1 inch more than
/// nominal"); EVO stores the nominal rim (14" = 0.1778).
pub fn nominal_rim(rim: f64) -> f64 {
    let d = 2.0 * rim / 0.0254;
    let nom = (d - 1.0).round();
    if (d - 1.0 - nom).abs() < 0.3 { nom * 0.0254 / 2.0 } else { rim }
}

/// An AC1 lut by file name, or an inline `(|x=y|)` value.
pub fn lut(car: &Car, v: &str) -> Vec<(f64, f64)> {
    let v = v.trim();
    if v.starts_with('(') {
        return ini::lut(v);
    }
    car.data.get(&v.to_lowercase()).map(|s| ini::lut(s)).unwrap_or_default()
}

const DWB: [&str; 8] = ["WBCAR_TOP_FRONT", "WBCAR_TOP_REAR", "WBCAR_BOTTOM_FRONT", "WBCAR_BOTTOM_REAR", "WBTYRE_TOP", "WBTYRE_BOTTOM", "WBCAR_STEER", "WBTYRE_STEER"];
const STRUT: [&str; 7] = ["STRUT_CAR", "STRUT_TYRE", "WBCAR_BOTTOM_FRONT", "WBCAR_BOTTOM_REAR", "WBTYRE_BOTTOM", "WBCAR_STEER", "WBTYRE_STEER"];

fn pts<const N: usize>(s: &Ini, sec: &str, keys: [&str; N]) -> Option<[[f64; 3]; N]> {
    let mut o = [[0.0; 3]; N];
    for (k, p) in keys.iter().zip(o.iter_mut()) {
        *p = ac1_v3(s, sec, k)?;
    }
    Some(o)
}

fn geometry(s: &Ini, sec: &str) -> (Geometry, String) {
    let kind = s.get(sec, "TYPE").unwrap_or("DWB").trim().to_uppercase();
    let g = match kind.as_str() {
        "STRUT" => pts(s, sec, STRUT).map(Geometry::Strut),
        "AXLE" => {
            let n = s.num("AXLE", "LINK_COUNT").unwrap_or(4.0) as usize;
            let links = (0..n).filter_map(|k| Some((ac1_v3(s, "AXLE", &format!("J{k}_CAR"))?, ac1_v3(s, "AXLE", &format!("J{k}_AXLE"))?))).collect();
            Some(Geometry::Axle {
                links,
                attach: s.num("AXLE", "ATTACH_REL_POS").unwrap_or(0.5),
                leaf_lat_k: s.num("AXLE", "LEAF_SPRING_LAT_K").unwrap_or(0.0),
                torque_reaction: s.num("AXLE", "TORQUE_REACTION").unwrap_or(0.0),
            })
        }
        "ML" => {
            let j: Vec<_> = (0..5).filter_map(|k| Some((ac1_v3(s, sec, &format!("JOINT{k}_CAR"))?, ac1_v3(s, sec, &format!("JOINT{k}_TYRE"))?))).collect();
            if j.len() == 5 { Some(Geometry::MultiLink(j)) } else { pts(s, sec, DWB).map(Geometry::DoubleWishbone) }
        }
        _ => pts(s, sec, DWB).map(Geometry::DoubleWishbone),
    };
    (g.unwrap_or_default(), kind)
}

fn axle(s: &Ini, sec: &str, frame: &Frame) -> Axle {
    let n = |k: &str| s.num(sec, k);
    let pos = |k: &str, d: f64| n(k).filter(|v| *v > 0.0).unwrap_or(d);
    let (geometry, kind) = geometry(s, sec);
    let slow_b = pos("DAMP_BUMP", 3000.0);
    let slow_r = pos("DAMP_REBOUND", 6000.0);
    Axle {
        geometry,
        kind,
        track: n("TRACK").or(frame.track_width(sec == "FRONT")).unwrap_or(1.5),
        spring: n("SPRING_RATE").filter(|v| *v > 1000.0).unwrap_or(40000.0),
        progressive: n("PROGRESSIVE_SPRING_RATE").filter(|v| *v > 0.0),
        dampers: Dampers {
            bump: slow_b,
            rebound: slow_r,
            fast_bump: pos("DAMP_FAST_BUMP", slow_b * 0.5),
            fast_rebound: pos("DAMP_FAST_REBOUND", slow_r * 0.5),
            fast_bump_threshold: pos("DAMP_FAST_BUMPTHRESHOLD", 0.1),
            fast_rebound_threshold: pos("DAMP_FAST_REBOUNDTHRESHOLD", 0.1),
        },
        arb: s.num("ARB", sec).unwrap_or(0.0).max(0.0),
        bump_stop_rate: pos("BUMP_STOP_RATE", 50000.0),
        bump_stop_up: n("BUMPSTOP_UP").unwrap_or(0.08),
        bump_stop_down: n("BUMPSTOP_DN").unwrap_or(0.08),
        rod_length: n("ROD_LENGTH").unwrap_or(0.0),
        base_y: n("BASEY").unwrap_or(-0.2),
        hub_mass: n("HUB_MASS").unwrap_or(40.0),
        toe_out: n("TOE_OUT").unwrap_or(0.0),
        static_camber_deg: n("STATIC_CAMBER").unwrap_or(0.0),
        rim_offset: n("RIM_OFFSET").unwrap_or(0.0),
    }
}

/// The compounds of tyres.ini ([FRONT] / [REAR], [FRONT_1] / [REAR_1] ...),
/// at most 16. `fz` = static wheel loads (FZ0 of the older files).
pub fn compounds(t: &Ini, fz: [f64; 2]) -> Vec<Compound> {
    let mut out = Vec::new();
    for k in 0..16 {
        let (sf, sr) = if k == 0 { ("FRONT".to_string(), "REAR".to_string()) } else { (format!("FRONT_{k}"), format!("REAR_{k}")) };
        let (Some(f), Some(r)) = (Ac1Tyre::read(t, &sf, fz[0]), Ac1Tyre::read(t, &sr, fz[1])) else { continue };
        let tyre = |a: Ac1Tyre, sec: &str| {
            let law = match a.dy { crate::ac1::tyre::Law::Power { .. } => "DY_REF / FZ0 / LS_EXPY", _ => "DY0 + DY1 x Fz (no FZ0)" };
            Tyre {
                name: a.name.clone(),
                short: a.short.clone(),
                width: a.width,
                radius: a.radius,
                rim_radius: nominal_rim(a.rim_radius),
                rate: a.rate,
                notes: vec![format!("[{sec}] {law}")],
                curve: Box::new(a),
            }
        };
        out.push(Compound { tyres: [tyre(f, &sf), tyre(r, &sr)] });
    }
    out
}

pub fn read(car: &Car, frame: &Frame) -> SourceCar {
    let ci = car.ini("car.ini");
    let s = car.ini("suspensions.ini");
    let mass = ci.num("BASIC", "TOTALMASS").filter(|m| *m > 100.0).unwrap_or(1200.0);
    let fuel = ci.num("FUEL", "FUEL").filter(|v| *v > 0.0).unwrap_or(30.0);
    let max_fuel = ci.num("FUEL", "MAX_FUEL").filter(|v| *v > 0.0).unwrap_or(fuel.max(50.0));
    let cg_front = s.num("BASIC", "CG_LOCATION").filter(|v| *v > 0.05 && *v < 0.95).unwrap_or(0.5);
    let wheelbase = s.num("BASIC", "WHEELBASE").or(frame.wheelbase()).unwrap_or(2.5);
    let fuel_kg = fuel.min(max_fuel) * KG_PER_LITRE;
    let ms = mass + fuel_kg;
    let fz = [ms * G * cg_front / 2.0, ms * G * (1.0 - cg_front) / 2.0];
    let t = car.ini("tyres.ini");
    let compounds = compounds(t, fz);
    let default_compound = (t.num("COMPOUND_DEFAULT", "INDEX").unwrap_or(0.0).max(0.0) as usize).min(compounds.len().saturating_sub(1));
    // engine
    let ei = car.ini("engine.ini");
    let mut torque = lut(car, ei.get("HEADER", "POWER_CURVE").unwrap_or("power.lut"));
    torque.retain(|p| p.0.is_finite() && p.1.is_finite());
    torque.sort_by(|a, b| a.0.total_cmp(&b.0));
    let dt = car.ini("drivetrain.ini");
    let n = dt.num("GEARS", "COUNT").unwrap_or(0.0) as usize;
    let trac = crate::car::physics::drivetrain::ac1_traction(dt);
    let engine = Engine {
        idle: ei.num("ENGINE_DATA", "MINIMUM").filter(|v| *v > 100.0).unwrap_or(900.0),
        limiter: ei.num("ENGINE_DATA", "LIMITER").unwrap_or_else(|| torque.last().map(|p| p.0).unwrap_or(7000.0)),
        torque,
        gears: (1..=n).filter_map(|k| dt.num("GEARS", &format!("GEAR_{k}"))).filter(|r| *r > 0.0).collect(),
        final_drive: dt.num("GEARS", "FINAL").filter(|v| *v > 0.1).unwrap_or(4.0),
        driven: match crate::car::physics::drivetrain::evo_traction(&trac) { 0 => 0, 1 => 1, _ => 2 },
    };
    let bi = car.ini("brakes.ini");
    // aero: horizontal wings at their angle, split onto the axles by position
    let a = car.ini("aero.ini");
    let mut aero = Aero::default();
    let b_cg = cg_front * wheelbase; // CoG to the rear axle
    for (sec, _) in a.with_prefix("WING_") {
        let (chord, span) = (a.num(sec, "CHORD").unwrap_or(1.0), a.num(sec, "SPAN").unwrap_or(1.0));
        let angle = a.num(sec, "ANGLE").unwrap_or(0.0);
        let at = |k: &str| a.get(sec, k).map(|v| lut(car, v)).filter(|l| !l.is_empty()).map(|l| ini::lut_at(&l, angle).unwrap_or(0.0)).unwrap_or(0.0);
        let cl = at("LUT_AOA_CL") * a.num(sec, "CL_GAIN").unwrap_or(0.0) * chord * span;
        let cd = at("LUT_AOA_CD") * a.num(sec, "CD_GAIN").unwrap_or(0.0) * chord * span;
        let z = ac1_v3(a, sec, "POSITION").map(|p| p[2]).unwrap_or(0.0);
        let front = ((b_cg + z) / wheelbase).clamp(0.0, 1.0);
        aero.cl_area[0] += cl * front;
        aero.cl_area[1] += cl * (1.0 - front);
        aero.cd_area += cd;
    }
    SourceCar {
        game: "ac1",
        name: car.folder.clone(),
        mass,
        fuel_kg,
        cg_front,
        cg_height: frame.cog[1],
        wheelbase,
        axles: [axle(s, "FRONT", frame), axle(s, "REAR", frame)],
        compounds,
        default_compound,
        engine,
        brake_torque: bi.num("DATA", "MAX_TORQUE").filter(|v| *v > 0.0).unwrap_or(2000.0),
        brake_front: bi.num("DATA", "FRONT_SHARE").filter(|v| *v > 0.0 && *v < 1.0).unwrap_or(0.65),
        aero,
        materials: Vec::new(),
    }
}

/// The kn5's materials (name, shader, texture slots, scalar params).
pub fn materials(k: &crate::ac1::kn5::Kn5) -> Vec<Material> {
    k.materials.iter().map(|m| Material {
        name: m.name.clone(),
        shader: m.shader.clone(),
        textures: m.textures.clone(),
        params: m.props.iter().map(|(n, p)| (n.clone(), p.value)).collect(),
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dwb_points_in_evo_order_and_compounds() {
        let s = ini::parse("[FRONT]\nTYPE=DWB\nWBCAR_TOP_FRONT=1,2,3\nWBCAR_TOP_REAR=4,5,6\nWBCAR_BOTTOM_FRONT=7,8,9\nWBCAR_BOTTOM_REAR=10,11,12\nWBTYRE_TOP=13,14,15\nWBTYRE_BOTTOM=16,17,18\nWBCAR_STEER=19,20,21\nWBTYRE_STEER=22,23,24\nSPRING_RATE=50000\n[ARB]\nFRONT=12000\n");
        let a = axle(&s, "FRONT", &Frame::default());
        match a.geometry {
            Geometry::DoubleWishbone(p) => {
                assert_eq!(p[0], [1.0, 2.0, 3.0]);
                assert_eq!(p[7], [22.0, 23.0, 24.0]);
            }
            g => panic!("{g:?}"),
        }
        assert_eq!((a.spring, a.arb), (50000.0, 12000.0));
        let t = ini::parse("[FRONT]\nRADIUS=0.3\n[REAR]\nRADIUS=0.31\n[FRONT_1]\nRADIUS=0.3\n[REAR_1]\nRADIUS=0.31\n");
        let c = compounds(&t, [3000.0, 3000.0]);
        assert_eq!(c.len(), 2);
        assert!((c[1].tyres[1].radius - 0.31).abs() < 1e-9);
    }
}
