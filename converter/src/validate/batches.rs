//! Batch names and shaders the game's car renderer relies on.
//!
//! The renderer's mesh commit sorts batches by EXACT name: EXT_SKIN /
//! EXT_BANNER / EXT_CARBON (exterior), the batches whose shader is
//! UberVehicleInteriorMaterial (interior; none = "Interiors are missing car
//! renderer cache"), per wheel EXT_TYRE, EXT_RIM, EXT_RIM_BLUR,
//! EXT_RIM_BLUR_STATIC, EXT_RIM_DECAL, EXT_RIM_DECAL_BLUR, and EXT_DISC. A
//! missing tyre / rim / rim-blur / rim-blur-static slot is dereferenced
//! without a null check (decals are checked): a crash.
//!
//! Every rule below is calibrated on the 71 stock cars of content.kspkg
//! (`aceconv check-car-stock`: they all pass) - the allowed shader sets are
//! exactly what stock uses for each name.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::Report;
use crate::io::pb::Msg;

/// The shaders stock uses per wheel batch name (survey of every stock base
/// rim / blur / tyre mesh: VehicleRim everywhere, one car's
/// EXT_RIM_BLUR_STATIC on UberVehicleMaterial).
const WHEEL_SHADERS: [(&str, &[&str]); 6] = [
    ("EXT_TYRE", &["VehicleTyre"]),
    ("EXT_RIM", &["VehicleRim"]),
    ("EXT_RIM_DECAL", &["VehicleRim"]),
    ("EXT_RIM_BLUR", &["VehicleRim"]),
    ("EXT_RIM_BLUR_STATIC", &["VehicleRim", "UberVehicleMaterial"]),
    ("EXT_RIM_DECAL_BLUR", &["VehicleRim"]),
];

/// Glass roles by exact name -> the shaders stock uses for them.
const GLASS_SHADERS: [(&str, &[&str]); 4] = [
    ("EXT_WINDOWS", &["VehicleGlass"]),
    ("INT_WINDOWS", &["VehicleGlassInterior", "VehicleWindow"]),
    ("INT_WINDSHIELD", &["VehicleGlassInterior", "VehicleWindshield"]),
    ("INT_GLASS", &["VehicleGlassInterior"]),
];

/// Interior shaders that are not the interior PBR shader on purpose.
const INTERIOR_OTHER: [&str; 6] = ["VehicleGlassInterior", "VehicleGlass", "VehicleWindshield", "VehicleWindow", "VehicleLed", "VehicleDisplay"];

struct Batch {
    name: String,
    material: String,
    /// LOD0 uv range (min u, min v, max u, max v)
    uv: Option<[f32; 4]>,
    /// LOD0 texcoords1 range, and whether u rises with +X and v with -Y
    uv1: Option<([f32; 4], bool, bool)>,
    /// LOD0: vertices per heaviest bone name
    bones: BTreeMap<String, usize>,
}

fn f32s(b: Option<&[u8]>) -> Vec<f32> {
    b.map(|b| b.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect()).unwrap_or_default()
}

/// Per LOD, the batches of a mesh.
fn batches(mesh: &[u8]) -> Option<Vec<Vec<Batch>>> {
    let top = Msg::parse(mesh).ok()?;
    let mut out = Vec::new();
    for (li, body) in top.subs(5).iter().enumerate() {
        let l0 = li == 0;
        let uvs = if l0 { f32s(body.bytes(7)) } else { Vec::new() };
        let idx = if l0 { body.bytes(11).map(crate::io::pb::varints).unwrap_or_default() } else { Vec::new() };
        let (pos, uv1s, bw, bi) = if l0 { (f32s(body.bytes(5)), f32s(body.bytes(14)), f32s(body.bytes(9)), f32s(body.bytes(10))) } else { Default::default() };
        let bone_names: Vec<String> = if l0 { body.subs(12).iter().map(|b| b.str(1).unwrap_or_default()).collect() } else { Vec::new() };
        let range = |t: &[f32], vs: &[usize]| -> Option<[f32; 4]> {
            let mut r = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
            for &i in vs {
                if let (Some(u), Some(v)) = (t.get(i * 2), t.get(i * 2 + 1)) {
                    r = [r[0].min(*u), r[1].min(*v), r[2].max(*u), r[3].max(*v)];
                }
            }
            r[0].is_finite().then_some(r)
        };
        // covariance of channel k of a 2-wide stream with axis a of the positions
        let cov = |t: &[f32], k: usize, a: usize, vs: &[usize]| -> f64 {
            let n = vs.len().max(1) as f64;
            let (mut st, mut sp, mut stp) = (0.0, 0.0, 0.0);
            for &i in vs {
                let (x, y) = (*t.get(i * 2 + k).unwrap_or(&0.0) as f64, *pos.get(i * 3 + a).unwrap_or(&0.0) as f64);
                st += x;
                sp += y;
                stp += x * y;
            }
            stp / n - st / n * sp / n
        };
        let mut lod = Vec::new();
        for s in body.subs(4) {
            let (mut uv, mut uv1, mut bones) = (None, None, BTreeMap::new());
            if l0 {
                let (st, n) = (s.varint(2).unwrap_or(0) as usize, s.varint(3).unwrap_or(0) as usize);
                let mut vs: Vec<usize> = idx.get(st..st + n).unwrap_or(&[]).iter().map(|&i| i as usize).collect();
                vs.sort_unstable();
                vs.dedup();
                uv = range(&uvs, &vs);
                uv1 = range(&uv1s, &vs).map(|r| (r, cov(&uv1s, 0, 0, &vs) > 0.0, cov(&uv1s, 1, 1, &vs) < 0.0));
                for &i in &vs {
                    let w = |k: usize| bw.get(i * 4 + k).copied().unwrap_or(0.0);
                    let k = (0..4).max_by(|&a, &b| w(a).total_cmp(&w(b))).unwrap_or(0);
                    if let Some(name) = bi.get(i * 4 + k).and_then(|&b| bone_names.get(b as usize)) {
                        *bones.entry(name.clone()).or_insert(0) += 1;
                    }
                }
            }
            lod.push(Batch { name: s.str(1).unwrap_or_default(), material: s.str(4).unwrap_or_default(), uv, uv1, bones });
        }
        out.push(lod);
    }
    Some(out)
}

/// The batch checks of one car. `get` resolves a content path (the car's
/// own files, then the game's); `own` lists the car's own file paths.
pub fn check(id: &str, own: &[String], get: &dyn Fn(&str) -> Option<Vec<u8>>, r: &mut Report) {
    let base = format!("content\\cars\\{id}\\");
    let Some(actor) = get(&format!("{base}{id}.actor")).and_then(|b| Msg::parse(&b).ok()) else { return };
    let Some(car) = actor.sub(103) else { return };
    let bm = car.sub(12).unwrap_or_default();
    let mut shader_cache: HashMap<String, Option<String>> = HashMap::new();
    let mut shader = |p: &str| -> Option<String> {
        shader_cache.entry(p.to_lowercase()).or_insert_with(|| get(p).and_then(|b| Msg::parse(&b).ok()).and_then(|m| m.str(1))).clone()
    };
    let mut mesh_cache: HashMap<String, Option<Vec<Vec<Batch>>>> = HashMap::new();
    fn load_in(c: &mut HashMap<String, Option<Vec<Vec<Batch>>>>, get: &dyn Fn(&str) -> Option<Vec<u8>>, p: &str) -> bool {
        let k = p.to_lowercase();
        if !c.contains_key(&k) {
            let v = get(p).and_then(|b| batches(&b));
            c.insert(k.clone(), v);
        }
        c[&k].is_some()
    }
    macro_rules! load {
        ($p:expr) => {
            load_in(&mut mesh_cache, get, $p)
        };
    }
    // every batch name in the car (for the design-slot check)
    let mut all_names: BTreeSet<String> = BTreeSet::new();
    // -- wheels
    let corners = ["LF", "RF", "LR", "RR"];
    let slots: [(&str, u32, &[&str], &[&str]); 3] = [
        ("tyre", 2, &["EXT_TYRE"], &["EXT_TYRE"]),
        ("rim", 6, &["EXT_RIM"], &["EXT_RIM", "EXT_RIM_DECAL", "EXT_RIM_BLUR", "EXT_RIM_BLUR_STATIC", "EXT_RIM_DECAL_BLUR"]),
        ("rim blur", 18, &["EXT_RIM_BLUR", "EXT_RIM_BLUR_STATIC"], &["EXT_RIM_BLUR", "EXT_RIM_BLUR_STATIC", "EXT_RIM_DECAL_BLUR", "EXT_RIM", "EXT_RIM_DECAL"]),
    ];
    for (what, first, need, known) in slots {
        for (ci, c) in corners.iter().enumerate() {
            let Some(path) = bm.sub(first + ci as u32).and_then(|s| s.str(1)) else {
                r.err(format!("actor: no {what} mesh for WHEEL_{c} - the renderer dereferences it"));
                continue;
            };
            if !load!(&path) {
                r.err(format!("actor: {what} mesh {path} (WHEEL_{c}) is missing or unreadable"));
                continue;
            }
            let lods = mesh_cache[&path.to_lowercase()].as_ref().unwrap();
            let short = path.rsplit('\\').next().unwrap_or(&path).to_string();
            for (li, lod) in lods.iter().enumerate() {
                // LOD0 is what the renderer commits (stock: one car's rim
                // LOD1 holds only blur batches); the other LODs should match
                for n in need {
                    if !lod.iter().any(|b| b.name == *n) {
                        let m = format!("{short} (WHEEL_{c} {what}) LOD{li} has no {n} batch (has {:?})", lod.iter().map(|b| b.name.as_str()).collect::<Vec<_>>());
                        if li == 0 { r.err(format!("{m} - the game crashes committing the wheel materials")) } else { r.warn(m) }
                    }
                }
                for b in lod {
                    all_names.insert(b.name.clone());
                    if !known.contains(&b.name.as_str()) {
                        r.warn(format!("{short} LOD{li}: batch {} is not a stock {what} batch name ({known:?})", b.name));
                    }
                    if let Some((_, allowed)) = WHEEL_SHADERS.iter().find(|(n, _)| *n == b.name) {
                        match shader(&b.material) {
                            None => r.err(format!("{short}: {} material {} is missing", b.name, b.material)),
                            Some(s) if !allowed.contains(&s.as_str()) => r.err(format!("{short}: {} uses {s}; stock uses {allowed:?} for it", b.name)),
                            _ => {}
                        }
                    }
                }
            }
        }
    }
    // -- body
    let body_path = bm.str(1);
    let mut ext_names: Vec<String> = Vec::new();
    match body_path.as_deref() {
        Some(p) if load!(p) => {
            let lods = mesh_cache[&p.to_lowercase()].as_ref().unwrap();
            let lod0 = &lods[0];
            ext_names = lod0.iter().map(|b| b.name.clone()).collect();
            for lod in lods {
                all_names.extend(lod.iter().map(|b| b.name.clone()));
            }
            if !ext_names.iter().any(|n| n == "EXT_DISC") {
                r.err("exterior: no EXT_DISC batch (the brake discs; stock: every car)".into());
            }
            for b in lod0.iter().filter(|b| b.name == "EXT_DISC") {
                // stock: the discs ride the DISC_xx bones (70 of 71 cars all of
                // them; ks_alfa_romeo_75_turbo_evo 79 %, its rear drums on the body)
                let total: usize = b.bones.values().sum();
                let on_disc: usize = b.bones.iter().filter(|(n, _)| n.to_ascii_uppercase().starts_with("DISC_")).map(|(_, c)| c).sum();
                if total > 0 && on_disc * 2 < total {
                    r.err(format!("exterior: EXT_DISC has {on_disc} of {total} vertices on DISC_xx bones ({:?}) - the discs do not turn or steer with the wheels (stock: on DISC_xx)", b.bones));
                }
                if let Some(uv) = b.uv {
                    if uv[0] < -0.02 || uv[1] < -0.02 || uv[2] > 1.02 || uv[3] > 1.02 {
                        r.warn(format!("exterior: EXT_DISC UVs {uv:.2?} leave 0..1 (all 71 stock discs are inside 0..1)"));
                    }
                }
            }
            if !ext_names.iter().any(|n| matches!(n.as_str(), "EXT_SKIN" | "EXT_BANNER" | "EXT_CARBON")) {
                r.err("exterior: none of EXT_SKIN / EXT_BANNER / EXT_CARBON (the body the liveries and paint go on)".into());
            }
            for b in lod0 {
                if let Some((_, allowed)) = GLASS_SHADERS.iter().find(|(n, _)| *n == b.name) {
                    if let Some(s) = shader(&b.material) {
                        if !allowed.contains(&s.as_str()) {
                            r.err(format!("exterior: {} uses {s}; stock uses {allowed:?}", b.name));
                        }
                    }
                }
            }
        }
        Some(p) => r.err(format!("actor: body mesh {p} is missing or unreadable")),
        None => r.err("actor: no body mesh (f12.f1)".into()),
    }
    // -- interior
    let mut int_names: Vec<String> = Vec::new();
    match bm.sub(10).and_then(|s| s.str(1)) {
        Some(p) if load!(&p) => {
            let lods = mesh_cache[&p.to_lowercase()].as_ref().unwrap();
            for lod in lods {
                all_names.extend(lod.iter().map(|b| b.name.clone()));
            }
            let lod0 = &lods[0];
            int_names = lod0.iter().map(|b| b.name.clone()).collect();
            let (mut ui, mut other) = (0, Vec::new());
            for b in lod0 {
                let s = shader(&b.material).unwrap_or_default();
                if let Some((_, allowed)) = GLASS_SHADERS.iter().find(|(n, _)| *n == b.name) {
                    if !allowed.contains(&s.as_str()) {
                        r.err(format!("interior: {} uses {s}; stock uses {allowed:?}", b.name));
                    }
                }
                if s == "UberVehicleInteriorMaterial" {
                    ui += 1;
                } else if !(INTERIOR_OTHER.contains(&s.as_str()) || b.name.starts_with("MIRROR")) {
                    other.push(format!("{}={s}", b.name));
                }
            }
            if ui == 0 {
                r.err("interior: no batch uses UberVehicleInteriorMaterial - the game throws \"Interiors are missing car renderer cache\"".into());
            }
            // stock: at most 4 such batches in one interior (netting, speakers, window frames)
            if other.len() > 4 {
                r.err(format!("interior: {} batches use neither UberVehicleInteriorMaterial nor a glass / LED / mirror shader (stock: at most 4): {}", other.len(), other.iter().take(6).cloned().collect::<Vec<_>>().join(", ")));
            } else if !other.is_empty() {
                r.warn(format!("interior: {} batch(es) not on UberVehicleInteriorMaterial: {}", other.len(), other.join(", ")));
            }
        }
        Some(p) => r.err(format!("actor: interior mesh {p} is missing or unreadable")),
        None => r.err("actor: no interior mesh (f12.f10)".into()),
    }
    // -- mirrors: the actor's mirror cameras render into the batch they name
    for m in car.subs(23) {
        let (ty, batch) = (m.varint(1).unwrap_or(0), m.str(2).unwrap_or_default());
        let (names, side) = if ty == 0 { (&int_names, "interior") } else { (&ext_names, "exterior") };
        // stock: 8 cars name a mirror batch their mesh does not have (open cars)
        if !names.iter().any(|n| *n == batch) {
            r.warn(format!("actor mirror {batch} (type {ty}): no {side} batch of that name - that mirror shows nothing"));
        }
    }
    for p in [body_path.clone(), bm.sub(10).and_then(|s| s.str(1))].into_iter().flatten() {
        let Some(Some(lods)) = mesh_cache.get(&p.to_lowercase()) else { continue };
        for b in lods[0].iter().filter(|b| b.name.starts_with("MIRROR_")) {
            match shader(&b.material) {
                None => r.err(format!("{}: material {} is missing", b.name, b.material)),
                Some(s) if s != "UberVehicleMaterial" => r.err(format!("{}: {s}; every stock mirror uses the common_assets MIRROR_n material (UberVehicleMaterial)", b.name)),
                _ => {}
            }
            // stock mirrors keep texcoords0 inside 0..1
            if let Some(uv) = b.uv {
                if uv[0] < -0.01 || uv[1] < -0.01 || uv[2] > 1.01 || uv[3] > 1.01 {
                    r.err(format!("{}: UVs {:.2?} leave 0..1 - the mirror image is clamped / smeared (stock mirrors: 0..1)", b.name, uv));
                }
            }
            // each mirror renders into its own target, mapped by texcoords1:
            // on all 202 stock MIRROR_0 / 1 / 2 batches it spans a per-mirror
            // box (u 0.11..0.89 / 0.03..0.97, v 0.01..0.99 medians) with u
            // rising to +X and v to -Y
            if matches!(b.name.as_str(), "MIRROR_0" | "MIRROR_1" | "MIRROR_2") {
                match b.uv1 {
                    None => r.err(format!("{}: no texcoords1 - the mirror shows one texel of its render target (stock: a 0..1 box)", b.name)),
                    Some((u1, ux, vy)) => {
                        if u1[2] - u1[0] < 0.4 || u1[3] - u1[1] < 0.4 || u1[0] < -0.01 || u1[1] < -0.01 || u1[2] > 1.01 || u1[3] > 1.01 {
                            r.err(format!("{}: texcoords1 {:.2?} - the mirror maps its render target with them (stock: a box inside 0..1, at least 0.4 wide each way)", b.name, u1));
                        } else if !(ux && vy) {
                            r.err(format!("{}: texcoords1 orientation (u rises with +X: {ux}, v with -Y: {vy}) - a flipped mirror image (stock: both, 202 of 202)", b.name));
                        }
                    }
                }
            }
        }
    }
    // -- numbered copies of the wheel names in the body (EXT_RIM_2: the
    //    renderer reads exact names; stock never numbers them - it does
    //    number glass, EXT_WINDOWS_2 on 12 cars)
    for n in ext_names.iter().chain(int_names.iter()) {
        for (g, _) in WHEEL_SHADERS.iter() {
            if let Some(rest) = n.strip_prefix(g).and_then(|x| x.strip_prefix('_')) {
                if rest.chars().all(|c| c.is_ascii_digit()) && !rest.is_empty() {
                    r.warn(format!("batch {n}: a numbered copy of {g} (stock has one {g})"));
                }
            }
        }
    }
    // part meshes (optional parts, belts) also carry batches design slots may name
    for p in own.iter().filter(|p| p.to_lowercase().ends_with(".mesh")) {
        if load!(p) {
            for lod in mesh_cache[&p.to_lowercase()].as_ref().unwrap() {
                all_names.extend(lod.iter().map(|b| b.name.clone()));
            }
        }
    }
    // -- designs: a slot replaces the material of the batches of its name;
    //    a slot no batch carries does nothing (EXT_RIM_BLUR_STATIC written
    //    and referenced, but no batch of that name: the crash above)
    let mut unused: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for p in own.iter().filter(|p| p.to_lowercase().ends_with(".design")) {
        let Some(d) = get(p).and_then(|b| Msg::parse(&b).ok()) else { continue };
        for s in d.subs(3) {
            let slot = s.str(1).unwrap_or_default();
            if !slot.is_empty() && !all_names.contains(&slot) {
                unused.entry(slot).or_default().push(p.rsplit('\\').next().unwrap_or(p).to_string());
            }
        }
    }
    for (slot, ds) in unused {
        r.warn(format!("design slot {slot} ({} design(s), e.g. {}): no batch of that name - its material is used by nothing", ds.len(), ds[0]));
    }
}

