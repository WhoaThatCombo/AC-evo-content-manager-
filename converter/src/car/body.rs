//! The car body: routes every kn5 instance to the exterior or interior mesh,
//! a section and a bone; builds the two skeletons; writes both meshes (5 LODs
//! each) and one material per section.
//!
//! Routing (requirements car-geometry):
//! - under COCKPIT_HR / STEER_HR -> interior (STEER_HR parts on STEER_BASE);
//!   everything else -> exterior;
//! - under SUSP_xx / HUB_xx -> the SUSP_xx bone (calipers, static uprights);
//!   loose rotors under WHEEL_xx -> DISC_xx; the rest under WHEEL_xx (rims,
//!   tyres, rim blur) is the wheel stage's;
//! - an animated node (animparts) -> its own bone;
//! - mirrors.ini meshes -> MIRROR_0 (inside, near the centre line) or MIRROR_1 /
//!   MIRROR_2 (left / right), with the stock mirror materials.
//!
//! Skeletons (names as stock actors use them): exterior CAR_BODY, DOOR_L / R,
//! MIRROR_1 / 2, FRONT_BUMPER, REAR_BUMPER, HOOD (the actor's damage parts),
//! the donor's wiper chain, and per corner HUB_xx with DISC / WHEEL / SUSP /
//! TYRE_xx children, all at the wheel centre with car axes (as on the stock
//! 992 GT3 Cup); interior COCKPIT_HR, STEER_BASE (the AC1 STEER_HR frame),
//! MIRROR_0, SEATBELT_ON / OFF.

use anyhow::Result;
use std::collections::{BTreeMap, HashMap};

use crate::ac1::kn5::Kn5;
use crate::car::frame::{self, Frame, M4, CORNERS};
use crate::car::geometry::{self, Bone, Built, Geo, Inst, Section, Under};
use crate::car::materials::{self, Hints, Kind, MatIn, Templates, Tex};
use crate::files::Files;

pub const EXT_ROOT: &str = "CAR_BODY";
pub const INT_ROOT: &str = "COCKPIT_HR";

/// One section of the plan.
pub struct Sec {
    pub name: String,
    pub kind: Kind,
    /// kn5 material index
    pub material: usize,
    pub interior: bool,
    /// (instance, bone name)
    pub parts: Vec<(usize, String)>,
}

pub struct Plan {
    pub ext_bones: Vec<Bone>,
    pub int_bones: Vec<Bone>,
    pub sections: Vec<Sec>,
    /// instances left to the wheel stage (rims, tyres, rim blur)
    pub wheel_parts: Vec<usize>,
    pub notes: Vec<String>,
}

fn bone(name: &str, parent: &str, local: M4) -> Bone {
    Bone { name: name.into(), parent: parent.into(), local }
}

fn centroid(i: &Inst) -> [f64; 3] {
    let m = i.mesh;
    let n = m.vertex_count.max(1);
    let mut s = [0f64; 3];
    for v in 0..m.vertex_count {
        let p = m.position(v);
        let q = frame::xform(&i.world, [p[0] as f64, p[1] as f64, p[2] as f64]);
        for k in 0..3 {
            s[k] += q[k];
        }
    }
    [s[0] / n as f64, s[1] / n as f64, s[2] / n as f64]
}

/// Area-weighted face normal of an instance in EVO axes (unnormalised).
fn facing(i: &Inst, f: &Frame) -> [f64; 3] {
    let m = i.mesh;
    let p = |v: usize| { let q = m.position(v); f.evo(frame::xform(&i.world, [q[0] as f64, q[1] as f64, q[2] as f64])) };
    let mut n = [0f64; 3];
    for t in 0..m.index_count() / 3 {
        let (a, b, c) = (p(m.index(t * 3) as usize), p(m.index(t * 3 + 1) as usize), p(m.index(t * 3 + 2) as usize));
        let (u, w) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
        n[0] += u[1] * w[2] - u[2] * w[1];
        n[1] += u[2] * w[0] - u[0] * w[2];
        n[2] += u[0] * w[1] - u[1] * w[0];
    }
    n
}

fn sec_name(kind: Kind, interior: bool, mat: &str, used: &mut HashMap<String, usize>) -> String {
    let side = if interior { "INT" } else { "EXT" };
    let base = match kind {
        Kind::Paint => format!("{side}_SKIN"),
        // stock: one EXT_WINDOWS per car, the cockpit's own sheets
        // INT_WINDSHIELD / INT_WINDOWS (named by `plan`), INT_GLASS over the gauges
        Kind::Glass => if interior { "INT_WINDOWS".to_string() } else { "EXT_WINDOWS".to_string() },
        Kind::GaugeGlass => "INT_GLASS".to_string(),
        Kind::LampGlass => "EXT_LIGHTS_GLASS".to_string(),
        Kind::Lamp => "EXT_LIGHTS".to_string(),
        Kind::Disc => "EXT_DISC".to_string(),
        Kind::Led => "INT_LED".to_string(),
        _ => {
            let s: String = materials::slug(mat).to_uppercase().chars().take(32).collect();
            let s = s.strip_prefix("EXT_").or_else(|| s.strip_prefix("INT_")).unwrap_or(&s).to_string();
            format!("{side}_{s}")
        }
    };
    let n = used.entry(base.clone()).or_insert(0);
    *n += 1;
    if *n == 1 { base } else { format!("{base}_{n}") }
}

/// The corners whose DISC_xx subtree has a mesh named like a disc
/// (`materials::disc_like`). Under a DISC_xx node without one, every plain
/// mesh is the disc (AC1 hangs the rotor there and nothing else).
fn disc_named(k: &Kn5, insts: &[Inst]) -> Vec<&'static str> {
    let mut v: Vec<&'static str> = insts.iter().filter(|i| i.under == Under::Disc)
        .filter(|i| k.materials.get(i.material).map(|m| materials::disc_like(m, &i.node)).unwrap_or(false))
        .filter_map(|i| i.corner).collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// The wheel corner a body-level part sits inside: its vertices within the
/// rim radius of that wheel's axle and within the tyre width of its centre
/// plane (a loose rotor or drum modelled outside the wheel nodes).
fn wheel_at(i: &Inst, f: &Frame) -> Option<&'static str> {
    let m = f.evo_m(&i.world);
    CORNERS.into_iter().find(|c| {
        let Some(w) = f.wheel_centres.get(*c) else { return false };
        let t = f.tyre(c);
        (0..i.mesh.vertex_count).all(|v| {
            let p = i.mesh.position(v);
            let q = frame::xform(&m, [p[0] as f64, p[1] as f64, p[2] as f64]);
            let (dx, dy, dz) = (q[0] - w[0], q[1] - w[1], q[2] - w[2]);
            (dy * dy + dz * dz).sqrt() < t.rim_radius && dx.abs() < t.width
        })
    })
}

/// The material kind refined for brake discs the material rules miss, and
/// the corner whose DISC_xx bone a disc rides: under a DISC_xx node (by name,
/// or every plain mesh there when nothing is named like a disc); a
/// disc-like body-level part inside a wheel.
fn disc_route(kind: Kind, m: &crate::ac1::kn5::Material, i: &Inst, f: &Frame, interior: bool, named: &[&str]) -> (Kind, Option<&'static str>) {
    if interior {
        return (kind, i.corner);
    }
    match i.under {
        Under::Disc if kind == Kind::Generic => {
            let c = i.corner.unwrap_or("");
            if materials::disc_like(m, &i.node) || !named.contains(&c) { (Kind::Disc, i.corner) } else { (kind, i.corner) }
        }
        Under::Body if kind == Kind::Disc || (kind == Kind::Generic && materials::disc_like(m, &i.node)) => match wheel_at(i, f) {
            Some(c) => (Kind::Disc, Some(c)),
            None => (kind, None),
        },
        _ => (kind, i.corner),
    }
}

/// The exterior bone of a non-animated part.
fn ext_bone(under: Under, corner: Option<&str>, kind: Kind) -> String {
    match (under, corner, kind) {
        (_, Some(c), Kind::Disc) => format!("DISC_{c}"),
        (Under::Wheel | Under::Disc, Some(c), _) => format!("DISC_{c}"),
        (Under::Susp, Some(c), _) => format!("SUSP_{c}"),
        _ => EXT_ROOT.into(),
    }
}

/// INT_WINDSHIELD for a sheet facing forward / back in the front half of
/// the car, INT_WINDOWS for the side and rear glass.
fn glass_role(i: &Inst, f: &Frame, lo: [f64; 3], hi: [f64; 3]) -> &'static str {
    let n = facing(i, f);
    let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-12);
    let c = f.evo(centroid(i));
    if n[2].abs() / l > 0.5 && c[2] > (lo[2] + hi[2]) / 2.0 { "INT_WINDSHIELD" } else { "INT_WINDOWS" }
}

/// Route the instances. `anim`: animated-part bones (`None` = none).
pub fn plan(k: &Kn5, insts: &[Inst], f: &Frame, h: &Hints, donor_ext_bones: &[Bone], mut anim: Option<&mut crate::car::animparts::Plan>) -> Plan {
    let mut notes = Vec::new();
    let mut ext: Vec<Bone> = vec![bone(EXT_ROOT, "", frame::I4)];
    let mut int: Vec<Bone> = vec![bone(INT_ROOT, "", frame::I4)];
    // the body's extent for the damage bones
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for i in insts.iter().filter(|i| !i.cockpit && i.under == Under::Body) {
        let c = f.evo(centroid(i));
        for kk in 0..3 {
            lo[kk] = lo[kk].min(c[kk]);
            hi[kk] = hi[kk].max(c[kk]);
        }
    }
    if !lo[0].is_finite() {
        lo = [-0.9, 0.2, -2.0];
        hi = [0.9, 1.2, 2.0];
    }
    let midy = (lo[1] + hi[1]) / 2.0;
    // mirrors: placed where their meshes are
    let mut mirror_pos: BTreeMap<&str, [f64; 3]> = BTreeMap::new();
    for i in insts.iter() {
        if h.mirrors.contains(&i.node.to_lowercase()) {
            let c = f.evo(centroid(i));
            let which = if i.cockpit && c[0].abs() < 0.25 { "MIRROR_0" } else if c[0] >= 0.0 { "MIRROR_1" } else { "MIRROR_2" };
            mirror_pos.entry(which).or_insert(c);
        }
    }
    let door_y = midy;
    ext.push(bone("DOOR_L", EXT_ROOT, frame::translation([hi[0] * 0.95, door_y, (lo[2] + hi[2]) * 0.5 + 0.4])));
    ext.push(bone("DOOR_R", EXT_ROOT, frame::translation([lo[0] * 0.95, door_y, (lo[2] + hi[2]) * 0.5 + 0.4])));
    for (name, side) in [("MIRROR_1", 1.0), ("MIRROR_2", -1.0)] {
        let p = mirror_pos.get(name).copied().unwrap_or([side * hi[0].abs().max(0.8), hi[1] * 0.75, (lo[2] + hi[2]) * 0.5 + 0.6]);
        ext.push(bone(name, EXT_ROOT, frame::translation(p)));
    }
    ext.push(bone("FRONT_BUMPER", EXT_ROOT, frame::translation([0.0, lo[1] + 0.3, hi[2]])));
    ext.push(bone("REAR_BUMPER", EXT_ROOT, frame::translation([0.0, lo[1] + 0.3, lo[2]])));
    ext.push(bone("HOOD", EXT_ROOT, frame::translation([0.0, midy + 0.25, hi[2] * 0.6])));
    // the donor's wiper chain (the wiper system names its bones)
    for b in donor_ext_bones.iter().filter(|b| b.name.starts_with("WIPER")) {
        ext.push(b.clone());
    }
    for c in CORNERS {
        let w = f.wheel_centres.get(c).copied().unwrap_or([0.0; 3]);
        ext.push(bone(&format!("HUB_{c}"), EXT_ROOT, frame::translation(w)));
        for part in ["DISC", "WHEEL", "SUSP", "TYRE"] {
            ext.push(bone(&format!("{part}_{c}"), &format!("HUB_{c}"), frame::I4));
        }
    }
    // interior
    let steer = f.node_world.iter().find(|(n, _)| n.eq_ignore_ascii_case("STEER_HR")).map(|(_, m)| f.evo_m(m)).unwrap_or(frame::translation([0.35, 0.75, 0.1]));
    int.push(bone("STEER_BASE", INT_ROOT, steer));
    let m0 = mirror_pos.get("MIRROR_0").copied().unwrap_or([0.0, hi[1] * 0.9, 0.2]);
    int.push(bone("MIRROR_0", INT_ROOT, frame::translation(m0)));
    int.push(bone("SEATBELT_ON", INT_ROOT, frame::I4));
    int.push(bone("SEATBELT_OFF", INT_ROOT, frame::I4));
    // sections
    let mut sections: Vec<Sec> = Vec::new();
    let mut by_key: HashMap<(Kind, usize, bool, String), usize> = HashMap::new();
    let mut used: HashMap<String, usize> = HashMap::new();
    let mut wheel_parts = Vec::new();
    let mut bound: Vec<(String, bool)> = Vec::new();
    let named = disc_named(k, insts);
    for (ii, i) in insts.iter().enumerate() {
        // the seat belts are meshes of their own (car::belts)
        if crate::car::belts::belt_of(&i.path).is_some() || h.drop.contains(&i.node.to_lowercase()) {
            continue;
        }
        let Some(m) = k.materials.get(i.material) else {
            notes.push(format!("geometry: {} has no material {} - dropped", i.node, i.material));
            continue;
        };
        let forced_int = anim.as_ref().map(|a| a.force_interior(&i.path)).unwrap_or(false);
        let interior = i.cockpit || forced_int;
        let kind = materials::kind_of(m, &i.node, i.under == Under::Wheel, i.blur, interior, h);
        // AC1's ksWindscreen is the cockpit side of the windscreen / windows
        // (shown from inside only): the interior's INT_WINDSHIELD / INT_WINDOWS
        let interior = interior || (kind == Kind::Glass && m.shader.to_lowercase().contains("windscreen"));
        let (kind, corner) = disc_route(kind, m, i, f, interior, &named);
        if matches!(kind, Kind::Tyre | Kind::Rim | Kind::RimBlur) {
            wheel_parts.push(ii);
            continue;
        }
        let mut mirror_bone = String::new();
        let (kind, name_hint) = if kind == Kind::Mirror {
            let c = f.evo(centroid(i));
            let which = if interior && c[0].abs() < 0.25 { "MIRROR_0" } else if c[0] >= 0.0 { "MIRROR_1" } else { "MIRROR_2" };
            mirror_bone = which.to_string();
            (kind, which.to_string())
        } else if matches!(kind, Kind::Lamp | Kind::Led) {
            // car functions: one section per function set / LED
            (kind, h.groups.get(&i.node.to_lowercase()).cloned().unwrap_or_default())
        } else if kind == Kind::Glass && interior {
            (kind, glass_role(i, f, lo, hi).to_string())
        } else if h.backlights.contains(&i.node.to_lowercase()) {
            // dash backlights: a section of their own (lit by the lamp stage)
            (kind, "backlight".to_string())
        } else {
            (kind, String::new())
        };
        let interior = if kind == Kind::Mirror { mirror_bone == "MIRROR_0" } else { interior };
        let anim_bone = anim.as_ref().and_then(|a| a.bone_for(&i.path, interior));
        if anim_bone.is_some() {
            bound.push((i.path.clone(), interior));
        }
        let bone_name = anim_bone.unwrap_or_else(|| {
            if kind == Kind::Mirror && !interior {
                return mirror_bone.clone();
            }
            if interior {
                if i.steer { "STEER_BASE".into() } else { INT_ROOT.into() }
            } else {
                ext_bone(i.under, corner, kind)
            }
        });
        // one batch per mirror, per window role (stock: one EXT_WINDOWS,
        // one INT_WINDSHIELD, one INT_WINDOWS) and one INT_GLASS
        let one = matches!(kind, Kind::Mirror | Kind::Glass | Kind::GaugeGlass);
        let key = (kind, if one { 0 } else { i.material }, interior, name_hint.clone());
        let si = *by_key.entry(key).or_insert_with(|| {
            let name = if kind == Kind::Mirror || (kind == Kind::Glass && interior) { name_hint.clone() } else { sec_name(kind, interior, &m.name, &mut used) };
            sections.push(Sec { name, kind, material: i.material, interior, parts: Vec::new() });
            sections.len() - 1
        });
        sections[si].parts.push((ii, bone_name));
    }
    // stock cars carry the windows twice: EXT_WINDOWS outside and the cockpit's
    // INT_WINDSHIELD / INT_WINDOWS. An AC1 car without cockpit-side glass gets
    // its windows copied into the interior (on the cockpit root).
    if !sections.iter().any(|s| s.kind == Kind::Glass && s.interior) {
        let ext: Vec<(usize, usize)> = sections.iter().filter(|s| s.kind == Kind::Glass && !s.interior).flat_map(|s| s.parts.iter().map(move |p| (s.material, p.0))).collect();
        for (mat, ii) in ext {
            let role = glass_role(&insts[ii], f, lo, hi);
            let si = match sections.iter().position(|s| s.kind == Kind::Glass && s.interior && s.name == role) {
                Some(x) => x,
                None => {
                    sections.push(Sec { name: role.to_string(), kind: Kind::Glass, material: mat, interior: true, parts: Vec::new() });
                    sections.len() - 1
                }
            };
            sections[si].parts.push((ii, INT_ROOT.to_string()));
        }
        if sections.iter().any(|s| s.kind == Kind::Glass && s.interior) {
            notes.push("geometry: no cockpit-side glass in the kn5 - the windows are copied into the interior (stock INT_WINDSHIELD / INT_WINDOWS)".into());
        }
    }
    // animated-part bones: only where they carry geometry
    if let Some(a) = anim.as_mut() {
        a.settle(bound.iter().map(|(p, i)| (p.as_str(), *i)));
        ext.extend(a.bone_list(false));
        int.extend(a.bone_list(true));
        a.fix_steer_bones(&mut int);
    }
    let lamps = sections.iter().filter(|s| s.kind == Kind::Lamp).count();
    notes.push(format!("geometry: {} sections ({} exterior, {} interior, {lamps} lamp), {} wheel parts",
        sections.len(), sections.iter().filter(|s| !s.interior).count(), sections.iter().filter(|s| s.interior).count(), wheel_parts.len()));
    Plan { ext_bones: ext, int_bones: int, sections, wheel_parts, notes }
}

/// Shift a section's UVs by whole tiles into 0..1 (kn5 v runs 0..-1; stock
/// mirrors and discs run 0..1 with the same orientation, and a wrapped
/// texture reads the same either way).
fn unit_uvs(g: &mut Geo) {
    for k in 0..2 {
        let lo = g.verts.iter().map(|v| v.uv[k]).fold(f32::INFINITY, f32::min);
        if !lo.is_finite() {
            return;
        }
        let d = (lo + 1e-4).floor();
        for v in g.verts.iter_mut() {
            v.uv[k] -= d;
        }
    }
}

/// A mirror's render-target UVs (texcoords1). Each mirror renders into its
/// own target (the renderer makes one `mirror_texture` per mirror) and
/// stock mirrors map it with texcoords1: on all 202 stock MIRROR_0 / 1 / 2
/// batches texcoords1 is an affine stretch of texcoords0 (residual under
/// 0.001 on the MX-5 NA) into a per-mirror box, u rising with +X (car left)
/// and v rising downwards (-Y). An axis whose AC1 UVs already span most of
/// 0..1 keeps them (stock does: the MX-5 NA's MIRROR_0 and the side mirrors'
/// v); AC1's side mirrors share one target, each on half of it, so their u is
/// stretched into the stock box (medians: side mirrors u 0.11..0.89, the
/// centre mirror 0.03..0.97, v 0.01..0.99). Ours were all zero: the whole
/// glass showed one texel of the target.
pub const MIRROR_UV1_SIDE: [f32; 4] = [0.11, 0.01, 0.89, 0.99];
pub const MIRROR_UV1_CENTRE: [f32; 4] = [0.03, 0.01, 0.97, 0.99];

pub fn mirror_uv1(g: &mut Geo, centre: bool) {
    let n = g.verts.len();
    if n == 0 {
        return;
    }
    let bx = if centre { MIRROR_UV1_CENTRE } else { MIRROR_UV1_SIDE };
    // the stock orientation: d(u)/d(x) > 0, d(v)/d(y) < 0
    let cov = |k: usize, a: usize| -> f64 {
        let (mu, mp) = g.verts.iter().fold((0.0, 0.0), |s, v| (s.0 + v.uv[k] as f64, s.1 + v.pos[a] as f64));
        let (mu, mp) = (mu / n as f64, mp / n as f64);
        g.verts.iter().map(|v| (v.uv[k] as f64 - mu) * (v.pos[a] as f64 - mp)).sum()
    };
    let flip = [cov(0, 0) < 0.0, cov(1, 1) > 0.0];
    for k in 0..2 {
        let lo = g.verts.iter().map(|v| v.uv[k]).fold(f32::INFINITY, f32::min);
        let hi = g.verts.iter().map(|v| v.uv[k]).fold(f32::NEG_INFINITY, f32::max);
        let span = (hi - lo).max(1e-6);
        if !flip[k] && span >= 0.6 && lo >= 0.0 && hi <= 1.0 {
            for v in g.verts.iter_mut() {
                v.uv1[k] = v.uv[k];
            }
            continue;
        }
        for v in g.verts.iter_mut() {
            let mut t = (v.uv[k] - lo) / span;
            if flip[k] {
                t = 1.0 - t;
            }
            v.uv1[k] = bx[k] + t * (bx[k + 2] - bx[k]);
        }
    }
}

/// The UV fix-ups of the sections that need them: mirrors (0..1 + their
/// render-target UVs) and the brake discs (0..1, as on all 71 stock EXT_DISC).
pub fn fix_uvs(name: &str, g: &mut Geo) {
    if name.starts_with("MIRROR_") {
        unit_uvs(g);
        mirror_uv1(g, name == "MIRROR_0");
    } else if name.starts_with("EXT_DISC") {
        unit_uvs(g);
    }
}

// ------------------------------------------------------- fallback disc --

/// The disc's outer radius over the rim radius (RIM_RADIUS) for a generated
/// disc. OURS, calibrated on the 70 stock cars with a disc
/// (`examples/discsurvey.rs`): the stock disc's outer radius over the inner
/// radius of its tyre is 0.62..0.78 (p10..p90), median 0.71.
pub const DISC_RIM_RATIO: f64 = 0.70;

fn f32v(b: Option<&[u8]>) -> Vec<f32> {
    b.map(|b| b.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect()).unwrap_or_default()
}

/// The EXT_DISC of a stock-format mesh (the donor's exterior) per corner:
/// LOD0 triangles whose vertices all ride DISC_xx, in that bone's frame.
pub fn stock_discs(mesh: &[u8]) -> BTreeMap<&'static str, Geo> {
    let mut out = BTreeMap::new();
    let Ok(top) = crate::io::pb::Msg::parse(mesh) else { return out };
    let Some(lod) = top.subs(5).into_iter().next() else { return out };
    let (pos, nrm, uv, tan, bi) = (f32v(lod.bytes(5)), f32v(lod.bytes(6)), f32v(lod.bytes(7)), f32v(lod.bytes(8)), f32v(lod.bytes(10)));
    let idx = lod.bytes(11).map(crate::io::pb::varints).unwrap_or_default();
    let bones = lod.subs(12);
    let names: Vec<String> = bones.iter().map(|b| b.str(1).unwrap_or_default()).collect();
    let worlds = geometry::bone_worlds(&bones);
    let nv = pos.len() / 3;
    if nrm.len() < nv * 3 || uv.len() < nv * 2 || tan.len() < nv * 4 || bi.len() < nv * 4 {
        return out;
    }
    for c in CORNERS {
        let Some(b) = names.iter().position(|n| *n == format!("DISC_{c}")) else { continue };
        let Some(to_bone) = frame::inv(&frame::m64(&worlds[b])) else { continue };
        let mut g = Geo::default();
        let mut remap: HashMap<usize, u32> = HashMap::new();
        for s in lod.subs(4).iter().filter(|s| s.str(1).as_deref() == Some("EXT_DISC")) {
            let (st, n) = (s.varint(2).unwrap_or(0) as usize, s.varint(3).unwrap_or(0) as usize);
            for t in idx.get(st..st + n).unwrap_or(&[]).chunks_exact(3) {
                let t: Vec<usize> = t.iter().map(|&v| v as usize).collect();
                if !t.iter().all(|&v| v < nv && bi[v * 4] as usize == b) {
                    continue;
                }
                for &v in &t {
                    let k = *remap.entry(v).or_insert_with(|| {
                        let p = frame::xform(&to_bone, [pos[v * 3] as f64, pos[v * 3 + 1] as f64, pos[v * 3 + 2] as f64]);
                        let d = frame::xdir(&to_bone, [nrm[v * 3] as f64, nrm[v * 3 + 1] as f64, nrm[v * 3 + 2] as f64]);
                        let e = frame::xdir(&to_bone, [tan[v * 4] as f64, tan[v * 4 + 1] as f64, tan[v * 4 + 2] as f64]);
                        g.verts.push(geometry::Vert {
                            pos: [p[0] as f32, p[1] as f32, p[2] as f32],
                            nrm: [d[0] as f32, d[1] as f32, d[2] as f32],
                            uv: [uv[v * 2], uv[v * 2 + 1]],
                            uv1: [0.0; 2],
                            tan: [e[0] as f32, e[1] as f32, e[2] as f32, tan[v * 4 + 3]],
                            bone: 0,
                        });
                        g.verts.len() as u32 - 1
                    });
                    g.idx.push(k);
                }
            }
        }
        if g.tris() > 0 {
            out.insert(c, g);
        }
    }
    out
}

/// A plain generated disc (ring 0.45..1 of `r`, 22 mm thick, 48 segments)
/// for a corner the template mesh has no disc for.
fn ring_disc(r: f64) -> Geo {
    let (ri, h) = (0.45 * r, 0.011);
    let prof = [(h, ri), (h, r), (-h, r), (-h, ri), (h, ri)];
    let mut g = crate::car::wheels::lathe(&prof, 48);
    for v in g.verts.iter_mut() {
        v.uv[1] = v.uv[1].clamp(0.0, 1.0);
    }
    g
}

/// EXT_DISC for a car whose model has no brake disc: per corner the
/// template's (the donor's stock) disc scaled to DISC_RIM_RATIO of that
/// wheel's rim radius, on its DISC_xx bone at the wheel centre; the
/// generated ring where the template has none for a corner. The game needs
/// the batch: the car renderer's material commit dereferences the EXT_DISC
/// slot without a null check.
pub fn fallback_disc(template: Option<&[u8]>, f: &Frame, bones: &[Bone]) -> (Geo, String) {
    let stock = template.map(stock_discs).unwrap_or_default();
    let worlds = geometry::worlds_of(bones);
    let mut g = Geo::default();
    let mut from = Vec::new();
    for c in CORNERS {
        let name = format!("DISC_{c}");
        let (Some(bi), Some(w)) = (bones.iter().position(|b| b.name == name), worlds.get(&name)) else { continue };
        let target = DISC_RIM_RATIO * f.tyre(c).rim_radius;
        let (src, scale) = match stock.get(c) {
            Some(s) => {
                let r = s.verts.iter().map(|v| ((v.pos[1] * v.pos[1] + v.pos[2] * v.pos[2]) as f64).sqrt()).fold(0.0, f64::max);
                (s.clone(), if r > 1e-3 { target / r } else { 1.0 })
            }
            None => (ring_disc(target), 1.0),
        };
        from.push(if stock.contains_key(c) { format!("{c} x{scale:.2}") } else { format!("{c} ring") });
        let base = g.verts.len() as u32;
        for v in &src.verts {
            let p = frame::xform(w, [v.pos[0] as f64 * scale, v.pos[1] as f64 * scale, v.pos[2] as f64 * scale]);
            let d = frame::xdir(w, [v.nrm[0] as f64, v.nrm[1] as f64, v.nrm[2] as f64]);
            let e = frame::xdir(w, [v.tan[0] as f64, v.tan[1] as f64, v.tan[2] as f64]);
            g.verts.push(geometry::Vert {
                pos: [p[0] as f32, p[1] as f32, p[2] as f32],
                nrm: [d[0] as f32, d[1] as f32, d[2] as f32],
                tan: [e[0] as f32, e[1] as f32, e[2] as f32, v.tan[3]],
                bone: bi as u16,
                ..*v
            });
        }
        g.idx.extend(src.idx.iter().map(|i| i + base));
    }
    (g, format!("geometry: no brake disc in the model - stock disc on DISC_xx ({})", from.join(", ")))
}

/// The section's geometry in EVO space.
fn section_geo(s: &Sec, insts: &[Inst], f: &Frame, bones: &[Bone]) -> (Geo, usize) {
    let idx: HashMap<&str, u16> = bones.iter().enumerate().map(|(i, b)| (b.name.as_str(), i as u16)).collect();
    let mut g = Geo::default();
    for (ii, b) in &s.parts {
        let i = &insts[*ii];
        let m = f.evo_m(&i.world);
        let bi = idx.get(b.as_str()).copied().unwrap_or(0);
        geometry::append(&mut g, i.mesh, &m, bi);
    }
    let pairs = geometry::split_double_sided(&mut g, 0.0004);
    geometry::compact(&mut g);
    (g, pairs)
}

pub struct Written {
    /// (section name, kind, kn5 material, interior, material path)
    pub sections: Vec<(String, Kind, usize, bool, String)>,
    pub ext_tris: usize,
    pub int_tris: usize,
    pub notes: Vec<String>,
}

/// Build both meshes and their materials into `files`.
#[allow(clippy::too_many_arguments)]
pub fn write(files: &mut Files, id: &str, stem: &str, k: &Kn5, insts: &[Inst], f: &Frame, p: &Plan, tex: &mut Tex, t: &Templates, game: Option<&crate::validate::Game>, default_skin: Option<(&str, std::path::PathBuf)>, author: &Author) -> Result<Written> {
    let base = format!("content\\cars\\{id}");
    let mut notes = Vec::new();
    let mut out = Vec::new();
    let mut ext_secs = Vec::new();
    let mut int_secs = Vec::new();
    let mut pairs_total = 0;
    for s in &p.sections {
        let m = &k.materials[s.material];
        let bones = if s.interior { &p.int_bones } else { &p.ext_bones };
        let (mut geo, pairs) = section_geo(s, insts, f, bones);
        pairs_total += pairs;
        if geo.tris() == 0 {
            continue;
        }
        fix_uvs(&s.name, &mut geo);
        let mat_path = if matches!(s.kind, Kind::Mirror | Kind::GaugeGlass) {
            let stock = format!("content\\cars\\common_assets\\materials\\{}.material", s.name);
            if game.map(|g| g.has(&stock)).unwrap_or(false) {
                stock
            } else {
                format!("{base}\\materials\\{}.material", s.name)
            }
        } else {
            let skin = default_skin.clone();
            let inp = MatIn { kind: s.kind, m, interior: s.interior, metallic: default_skin.as_ref().map(|(n, _)| crate::car::skins::is_metallic(n)).unwrap_or(false), skin };
            let mm = if s.kind == Kind::Glass && s.name == "INT_WINDSHIELD" {
                materials::glass_on(tex, &t.windshield_int, m, inp.skin.as_ref().map(|(n, p)| (*n, p.as_path())))
            } else {
                materials::build(files, tex, t, &inp)
            };
            let path = format!("{base}\\materials\\{}.material", s.name);
            files.put(&path, mm.encode());
            path
        };
        let sec = Section { name: s.name.clone(), material: mat_path.clone(), geo };
        if s.interior { int_secs.push(sec) } else { ext_secs.push(sec) }
        out.push((s.name.clone(), s.kind, s.material, s.interior, mat_path));
    }
    if pairs_total > 0 {
        notes.push(format!("geometry: {pairs_total} double-sided triangle pairs separated"));
    }
    let ep = format!("{base}\\meshes\\{stem}_exterior.mesh");
    let ip = format!("{base}\\meshes\\{stem}_interior.mesh");
    let et = files.get(&ep).cloned();
    let it = files.get(&ip).cloned();
    if !ext_secs.iter().any(|s| s.name == "EXT_DISC") {
        let (geo, note) = fallback_disc(et.as_deref(), f, &p.ext_bones);
        if geo.tris() > 0 {
            let path = format!("{base}\\materials\\EXT_DISC.material");
            files.put(&path, t.disc.encode());
            ext_secs.push(Section { name: "EXT_DISC".into(), material: path, geo });
            notes.push(note);
        }
    }
    if int_secs.is_empty() {
        notes.push("geometry: no interior parts - placeholder interior".into());
        int_secs.push(geometry::placeholder(&format!("{base}\\materials\\INT_PLACEHOLDER.material"), "INT_PLACEHOLDER"));
        files.put(&format!("{base}\\materials\\INT_PLACEHOLDER.material"), t.simple.encode());
    }
    if ext_secs.is_empty() {
        anyhow::bail!("the kn5 has no exterior geometry");
    }
    let mut ext = Built { bones: p.ext_bones.clone(), lods: vec![ext_secs], lod_in: vec![0.0], skinned: true };
    let mut int = Built { bones: p.int_bones.clone(), lods: vec![int_secs], lod_in: vec![0.0], skinned: true };
    geometry::make_lods(&mut ext, &geometry::EXT_LODS);
    geometry::make_lods(&mut int, &geometry::INT_LODS);
    notes.extend(author_lods(&mut ext, k, p, f, author));
    let (ext_tris, int_tris) = (ext.tris(0), int.tris(0));
    files.put(&ep, ext.encode(et.as_deref()));
    files.put(&ip, int.encode(it.as_deref()));
    notes.push(format!("geometry: exterior {} tris (LODs {}), interior {} tris (LODs {})",
        ext_tris, (1..5).map(|l| ext.tris(l).to_string()).collect::<Vec<_>>().join("/"),
        int_tris, (1..5).map(|l| int.tris(l).to_string()).collect::<Vec<_>>().join("/")));
    Ok(Written { sections: out, ext_tris, int_tris, notes })
}

/// The donor mesh's bones (as `Bone`s) from its LOD0.
pub fn bones_of(mesh: &[u8]) -> Vec<Bone> {
    let Ok(top) = crate::io::pb::Msg::parse(mesh) else { return vec![] };
    let Some(lod) = top.subs(5).into_iter().next() else { return vec![] };
    lod.subs(12).iter().map(|b| {
        let t = b.sub(3).unwrap_or_default();
        let mut l = [0f64; 16];
        for (i, v) in l.iter_mut().enumerate() {
            *v = t.f32(i as u32 + 1).unwrap_or(0.0) as f64;
        }
        Bone { name: b.str(1).unwrap_or_default(), parent: b.str(2).unwrap_or_default(), local: l }
    }).collect()
}

// ------------------------------------------------------------ author LODs --

/// The AC1 author LODs (`lods.ini` LOD_1.. kn5s) and what routing needs.
pub struct Author<'a> {
    pub files: Vec<std::path::PathBuf>,
    pub hints: &'a Hints,
    pub anim: Option<&'a crate::car::animparts::Plan>,
}

/// Exterior LOD2..4 from the AC1 author LODs B, C, D where they exist (the
/// requirement: author LODs first, simplification otherwise). AC1 switches
/// to LOD_B at 13-16 m and Kunos's LOD_B carries about a quarter of the
/// triangles, which is what stock EVO cars carry at their 20 m level, so
/// B -> level 2, C -> 3, D -> 4; level 1 (10 m) is LOD0 simplified. Their parts go to the LOD0 section of the
/// same AC1 material name and kind (a material LOD0 does not have is
/// dropped from that level); cockpit and wheel parts stay out (the interior
/// mesh and the wheel meshes have their own levels). A level that comes out
/// empty, or not lighter than the level before, keeps the simplified one.
fn author_lods(ext: &mut Built, k0: &Kn5, p: &Plan, f: &Frame, a: &Author) -> Vec<String> {
    let mut notes = Vec::new();
    if a.files.is_empty() {
        return notes;
    }
    let idx: HashMap<&str, u16> = ext.bones.iter().enumerate().map(|(i, b)| (b.name.as_str(), i as u16)).collect();
    // LOD0 sections by (material name, kind)
    let mut by_mat: HashMap<(String, Kind), (String, String)> = HashMap::new();
    for s in p.sections.iter().filter(|s| !s.interior) {
        let mname = k0.materials[s.material].name.to_lowercase();
        if let Some(sec) = ext.lods[0].iter().find(|x| x.name == s.name) {
            by_mat.entry((mname, s.kind)).or_insert((sec.name.clone(), sec.material.clone()));
        }
    }
    let mut used = 0;
    for (li, file) in a.files.iter().take(3).enumerate() {
        let level = li + 2;
        let Ok(map) = crate::ac1::kn5_decrypt::open(file) else {
            notes.push(format!("lods: {} is protected - simplified level kept", file.display()));
            continue;
        };
        if crate::ac1::kn5::protected(&map) {
            notes.push(format!("lods: {} is protected - simplified level kept", file.display()));
            continue;
        }
        let Ok(k) = crate::ac1::kn5::parse(&map) else {
            notes.push(format!("lods: {} unreadable - simplified level kept", file.display()));
            continue;
        };
        let mut dropped = Vec::new();
        let insts = geometry::flatten(&k, &mut dropped);
        let named = disc_named(&k, &insts);
        let mut secs: Vec<Section> = Vec::new();
        let mut lost = 0usize;
        for i in &insts {
            if i.cockpit {
                continue;
            }
            let Some(m) = k.materials.get(i.material) else { continue };
            let kind = materials::kind_of(m, &i.node, i.under == Under::Wheel, i.blur, false, a.hints);
            let (kind, corner) = disc_route(kind, m, i, f, false, &named);
            if matches!(kind, Kind::Tyre | Kind::Rim | Kind::RimBlur) {
                continue;
            }
            let key = (m.name.to_lowercase(), kind);
            let Some((sname, smat)) = by_mat.get(&key).or_else(|| by_mat.iter().find(|((_, kk), _)| *kk == kind && kind == Kind::Paint).map(|(_, v)| v)) else {
                lost += i.mesh.index_count() / 3;
                continue;
            };
            let bone = a.anim.and_then(|x| x.bone_for(&i.path, false)).filter(|b| idx.contains_key(b.as_str())).unwrap_or_else(|| ext_bone(i.under, corner, kind));
            let bi = idx.get(bone.as_str()).copied().unwrap_or(0);
            let si = match secs.iter().position(|x| x.name == *sname) {
                Some(si) => si,
                None => {
                    secs.push(Section { name: sname.clone(), material: smat.clone(), geo: Geo::default() });
                    secs.len() - 1
                }
            };
            geometry::append(&mut secs[si].geo, i.mesh, &f.evo_m(&i.world), bi);
        }
        for s in &mut secs {
            geometry::split_double_sided(&mut s.geo, 0.0004);
            geometry::compact(&mut s.geo);
            fix_uvs(&s.name, &mut s.geo);
        }
        secs.retain(|s| s.geo.tris() > 0);
        let tris: usize = secs.iter().map(|s| s.geo.tris()).sum();
        let prev = ext.tris(level - 1);
        if tris == 0 || tris as f64 > prev as f64 * 0.95 {
            notes.push(format!("lods: {} gives {tris} tris for level {level} (level {} has {prev}) - simplified level kept", file.file_name().unwrap_or_default().to_string_lossy(), level - 1));
            continue;
        }
        if lost > 0 {
            notes.push(format!("lods: {}: {lost} tris on materials LOD0 does not have - left out", file.file_name().unwrap_or_default().to_string_lossy()));
        }
        ext.lods[level] = secs;
        used = level;
    }
    if used > 0 {
        // levels after the last author level: that level, simplified
        let last = ext.lods[used].clone();
        for l in used + 1..5 {
            let r = geometry::EXT_LODS.ratio[l] / geometry::EXT_LODS.ratio[used].max(1e-3);
            ext.lods[l] = last.iter().map(|s| Section { name: s.name.clone(), material: s.material.clone(), geo: geometry::simplify(&s.geo, r) }).collect();
        }
        notes.push(format!("lods: author levels up to {used} from lods.ini"));
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::car::geometry::Vert;

    fn quad(uv: [[f32; 2]; 4]) -> Geo {
        // a side mirror: +X outward, +Y up; AC1 UVs on half of the target
        let pos = [[0.75, 0.80, 0.0], [0.90, 0.80, 0.0], [0.90, 0.92, 0.0], [0.75, 0.92, 0.0]];
        Geo { verts: (0..4).map(|i| Vert { pos: pos[i], uv: uv[i], ..Default::default() }).collect(), idx: vec![0, 1, 2, 0, 2, 3] }
    }

    fn disc_bones() -> Vec<Bone> {
        let mut b = vec![bone(EXT_ROOT, "", frame::I4)];
        for (c, x, z) in [("LF", 0.75, 1.3), ("RF", -0.75, 1.3), ("LR", 0.75, -1.2), ("RR", -0.75, -1.2)] {
            b.push(bone(&format!("HUB_{c}"), EXT_ROOT, frame::translation([x, 0.3, z])));
            b.push(bone(&format!("DISC_{c}"), &format!("HUB_{c}"), frame::I4));
        }
        b
    }

    fn disc_frame() -> Frame {
        let mut f = Frame::default();
        for (c, x, z) in [("LF", 0.70, 1.4), ("RF", -0.70, 1.4), ("LR", 0.72, -1.1), ("RR", -0.72, -1.1)] {
            f.wheel_centres.insert(c.into(), [x, 0.31, z]);
        }
        f.tyres = [frame::Tyre { radius: 0.30, width: 0.2, rim_radius: 0.19 }, frame::Tyre { radius: 0.32, width: 0.22, rim_radius: 0.215 }];
        f
    }

    #[test]
    fn fallback_disc_scales_the_template_disc_onto_disc_bones() {
        // a template mesh: one EXT_DISC ring of radius 0.15 per corner, on DISC_xx
        let tb = disc_bones();
        let w = geometry::worlds_of(&tb);
        let mut g = Geo::default();
        for (bi, b) in tb.iter().enumerate().filter(|(_, b)| b.name.starts_with("DISC_")) {
            let mut r = ring_disc(0.15);
            for v in r.verts.iter_mut() {
                let p = frame::xform(&w[&b.name], [v.pos[0] as f64, v.pos[1] as f64, v.pos[2] as f64]);
                v.pos = [p[0] as f32, p[1] as f32, p[2] as f32];
                v.bone = bi as u16;
                v.uv = [0.25, 0.75];
            }
            geometry::merge(&mut g, &r);
        }
        let other = placeholder_like("EXT_SKIN");
        let mesh = Built { bones: tb, lods: vec![vec![other, Section { name: "EXT_DISC".into(), material: "m".into(), geo: g }]], lod_in: vec![0.0], skinned: true }.encode(None);
        let stock = stock_discs(&mesh);
        assert_eq!(stock.len(), 4);
        let f = disc_frame();
        let ours = disc_bones_at(&f);
        let (d, note) = fallback_disc(Some(&mesh), &f, &ours);
        assert!(note.contains("LF x"), "{note}");
        let wo = geometry::worlds_of(&ours);
        for c in CORNERS {
            let bi = ours.iter().position(|b| b.name == format!("DISC_{c}")).unwrap() as u16;
            let o = wo[&format!("DISC_{c}")];
            let vs: Vec<_> = d.verts.iter().filter(|v| v.bone == bi).collect();
            assert!(!vs.is_empty(), "{c}");
            let r = vs.iter().map(|v| (((v.pos[1] as f64 - o[13]).powi(2) + (v.pos[2] as f64 - o[14]).powi(2)).sqrt())).fold(0.0, f64::max);
            let want = DISC_RIM_RATIO * f.tyre(c).rim_radius;
            assert!((r - want).abs() < 1e-3, "{c}: {r} vs {want}");
            // the stock UVs ride along
            assert!(vs.iter().all(|v| v.uv == [0.25, 0.75]));
        }
        // every vertex rides a DISC_xx bone
        assert!(d.verts.iter().all(|v| ours[v.bone as usize].name.starts_with("DISC_")));
    }

    #[test]
    fn fallback_disc_without_template_is_a_ring_inside_the_rim() {
        let f = disc_frame();
        let ours = disc_bones_at(&f);
        let (d, note) = fallback_disc(None, &f, &ours);
        assert!(note.contains("ring"), "{note}");
        assert!(d.tris() > 0);
        assert!(d.verts.iter().all(|v| ours[v.bone as usize].name.starts_with("DISC_") && (0.0..=1.0).contains(&v.uv[0]) && (0.0..=1.0).contains(&v.uv[1])));
    }

    fn placeholder_like(name: &str) -> Section {
        geometry::placeholder("m", name)
    }

    /// Our skeleton's corner bones at the frame's wheel centres (as `plan`).
    fn disc_bones_at(f: &Frame) -> Vec<Bone> {
        let mut b = vec![bone(EXT_ROOT, "", frame::I4)];
        for c in CORNERS {
            b.push(bone(&format!("HUB_{c}"), EXT_ROOT, frame::translation(f.wheel_centres[c])));
            b.push(bone(&format!("DISC_{c}"), &format!("HUB_{c}"), frame::I4));
        }
        b
    }

    #[test]
    fn mirror_uv1_stock_box_and_orientation() {
        // kn5 v runs 0..-1; u on the right half of a shared target
        let mut g = quad([[0.52, -0.01], [0.88, -0.01], [0.88, -0.99], [0.52, -0.99]]);
        fix_uvs("MIRROR_1", &mut g);
        let u: Vec<f32> = g.verts.iter().map(|v| v.uv1[0]).collect();
        let v: Vec<f32> = g.verts.iter().map(|v| v.uv1[1]).collect();
        assert!((u[0] - 0.11).abs() < 1e-4 && (u[1] - 0.89).abs() < 1e-4, "{u:?}");
        // v (0.01..0.99 after the tile shift) already spans the target: kept, rising downwards
        assert!(v[2] < v[0], "{v:?}");
        assert!(g.verts.iter().all(|x| (0.0..=1.0).contains(&x.uv[1])));
        // a mirror authored the other way round is turned to the stock orientation
        let mut f = quad([[0.88, 0.99], [0.52, 0.99], [0.52, 0.01], [0.88, 0.01]]);
        mirror_uv1(&mut f, false);
        assert!(f.verts[1].uv1[0] > f.verts[0].uv1[0] && f.verts[2].uv1[1] < f.verts[1].uv1[1]);
    }
}
