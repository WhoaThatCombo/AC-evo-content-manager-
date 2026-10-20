//! AC1 wheels -> EVO rims, rim blur and tyre meshes.
//!
//! Stock layout (the donor and ks_mazda_mx5_na): one rim mesh per corner side
//! (`rim_lf` / `rim_rf`, and `_lr` / `_rr` on staggered cars) and its blur
//! twin, centred on the wheel (no bones: the actor attaches them at
//! WHEEL_xx); a `.rimmesh` per axle naming its left / right meshes, blur
//! meshes and compatible `.tyremesh`es; a `.compatiblerim` naming the front
//! and rear rimmesh and tyremesh; one symmetric tyre mesh per size, attached
//! at TYRE_xx.
//!
//! Rims are written in the wheel frame (`Frame::to_wheel`: relative to the
//! wheel centre, car axes, small WHEEL node camber / toe removed). An axle
//! whose AC1 geometry has no blur gets its static rim in the blur slot.
//!
//! Tyres are generated: a surface of revolution (80 segments, like the stock
//! tyre) whose profile is the outer envelope of the AC1 tyre measured in the
//! wheel frame, scaled to the tyres.ini RADIUS and WIDTH. UVs follow the stock
//! convention, measured on the stock 215/35 R18 tyre: u = angle / 360 deg
//! around the axle, v = arc length along the profile normalised to 0..1 from
//! the +x bead to the -x bead (tread centre at 0.5).

use anyhow::Result;
use std::collections::BTreeMap;

use crate::ac1::kn5::Kn5;
use crate::car::frame::{self, Frame, CORNERS};
use crate::car::geometry::{self, Built, Geo, Inst, Section, Vert};
use crate::car::materials::{self, Kind, RimSlot, Templates, Tex};
use crate::files::Files;
use crate::io::pb::Msg;

pub struct Wheels {
    pub notes: Vec<String>,
    /// measured AC1 tyre radius per corner (wheel frame), for the report
    pub measured_radius: BTreeMap<String, f64>,
    /// written tyre radius per axle (front, rear)
    pub tyre_radius: [f64; 2],
}

/// One corner's rim or blur geometry in the wheel frame, by kn5 material.
fn corner_geo(insts: &[Inst], parts: &[usize], f: &Frame, c: &str, kind: Kind, k: &Kn5, h: &materials::Hints) -> BTreeMap<usize, Geo> {
    let mut by: BTreeMap<usize, Geo> = BTreeMap::new();
    let unrot = f.wheel_unrotate(c);
    let centre = f.wheel_centres.get(c).copied().unwrap_or([0.0; 3]);
    for &ii in parts {
        let i = &insts[ii];
        if i.corner != Some(c) {
            continue;
        }
        let Some(m) = k.materials.get(i.material) else { continue };
        if materials::kind_of(m, &i.node, true, i.blur, false, h) != kind {
            continue;
        }
        // AC1 world -> EVO -> wheel frame
        let to_evo = f.evo_m(&i.world);
        let to_wheel = frame::mul(&frame::mul(&to_evo, &frame::translation([-centre[0], -centre[1], -centre[2]])), &unrot);
        geometry::append(by.entry(i.material).or_default(), i.mesh, &to_wheel, 0);
    }
    for g in by.values_mut() {
        geometry::compact(g);
    }
    by
}

/// (axial |x|, radius) of every vertex of a corner's tyre, in the wheel frame.
fn tyre_points(insts: &[Inst], parts: &[usize], f: &Frame, c: &str, k: &Kn5, h: &materials::Hints) -> Vec<(f64, f64)> {
    let geos = corner_geo(insts, parts, f, c, Kind::Tyre, k, h);
    let mut pts = Vec::new();
    for g in geos.values() {
        for v in &g.verts {
            let (x, y, z) = (v.pos[0] as f64, v.pos[1] as f64, v.pos[2] as f64);
            pts.push((x, (y * y + z * z).sqrt()));
        }
    }
    pts
}

/// The outer profile of a tyre: (axial offset, radius) from the +x bead over
/// the tread to the -x bead, symmetric. `pts` = (axial, radius) samples.
/// Falls back to the stock 215/35 R18 profile shape.
pub fn profile(pts: &[(f64, f64)], radius: f64, width: f64, rim_radius: f64) -> Vec<(f64, f64)> {
    // stock profile (half, from the bead up to the tread centre), normalised:
    // x / half width, (r - bead r) / (R - bead r)
    const STOCK: [(f64, f64); 10] = [(0.877, 0.0), (0.98, 0.15), (1.0, 0.186), (0.997, 0.22), (0.975, 0.67), (0.929, 0.78), (0.866, 0.87), (0.772, 0.92), (0.673, 0.96), (0.0, 1.0)];
    let half = width / 2.0;
    let bead = rim_radius.min(radius - 0.02);
    let fallback = || -> Vec<(f64, f64)> { STOCK.iter().map(|(x, r)| (x * half, bead + r * (radius - bead))).collect() };
    if pts.len() < 50 {
        return mirror(&fallback());
    }
    let rmax = pts.iter().map(|p| p.1).fold(0.0, f64::max);
    let amax = pts.iter().map(|p| p.0.abs()).fold(0.0, f64::max);
    if rmax < 0.1 || amax < 0.02 {
        return mirror(&fallback());
    }
    // envelope around a centre on the profile's mid radius
    let rc = (bead.max(rmax * 0.6) + rmax) / 2.0;
    const N: usize = 48;
    let mut best = vec![None::<(f64, f64, f64)>; N + 1];
    for &(a, r) in pts {
        let (a, r) = (a.abs(), r);
        if r < bead * 0.98 {
            continue;
        }
        let (dx, dy) = (a, r - rc);
        let phi = dy.atan2(dx); // -pi/2 (down) .. pi/2 (up, tread centre)
        let t = ((phi + std::f64::consts::FRAC_PI_2) / std::f64::consts::PI * N as f64).round();
        if !(0.0..=N as f64).contains(&t) {
            continue;
        }
        // below the centre only the sidewall counts (not the inner liner)
        if phi < 0.0 && a < amax * 0.6 {
            continue;
        }
        let d = (dx * dx + dy * dy).sqrt();
        let slot = &mut best[t as usize];
        if slot.map(|s| d > s.0).unwrap_or(true) {
            *slot = Some((d, a, r));
        }
    }
    let mut ordered: Vec<(f64, f64)> = best.iter().flatten().map(|&(_, a, r)| (a, r)).collect();
    if ordered.len() < 6 {
        return mirror(&fallback());
    }
    // the tread centre point at a = 0
    let top = ordered.last().copied().unwrap_or((0.0, rmax));
    if top.0 > 1e-4 {
        ordered.push((0.0, top.1.max(rmax)));
    }
    // scale to tyres.ini
    let amax2 = ordered.iter().map(|p| p.0).fold(0.0, f64::max).max(1e-6);
    let rmax2 = ordered.iter().map(|p| p.1).fold(0.0, f64::max).max(1e-6);
    let scaled: Vec<(f64, f64)> = ordered.iter().map(|&(a, r)| (a * half / amax2, r * radius / rmax2)).collect();
    // thin to about 10 points (the stock tyre has 19 for both halves)
    let mut thin = vec![scaled[0]];
    let step = (scaled.len() as f64 / 9.0).max(1.0);
    let mut t = step;
    while (t as usize) < scaled.len() - 1 {
        thin.push(scaled[t as usize]);
        t += step;
    }
    thin.push(*scaled.last().unwrap());
    mirror(&thin)
}

/// Half profile (bead -> centre, +x side) -> full (+x bead .. -x bead).
fn mirror(h: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut out: Vec<(f64, f64)> = h.to_vec();
    for p in h.iter().rev().skip(1) {
        out.push((-p.0, p.1));
    }
    out
}

/// A tyre mesh section from a profile (axial, radius), `seg` segments.
pub fn lathe(prof: &[(f64, f64)], seg: usize) -> Geo {
    let mut arc = vec![0f64];
    for w in prof.windows(2) {
        let d = ((w[1].0 - w[0].0).powi(2) + (w[1].1 - w[0].1).powi(2)).sqrt();
        arc.push(arc.last().unwrap() + d);
    }
    let total = arc.last().copied().unwrap_or(1.0).max(1e-9);
    let np = prof.len();
    let mut g = Geo::default();
    for s in 0..=seg {
        let th = s as f64 / seg as f64 * std::f64::consts::TAU - std::f64::consts::PI;
        let (sn, cs) = th.sin_cos();
        for (j, &(a, r)) in prof.iter().enumerate() {
            // profile normal: perpendicular to the profile tangent, outward
            let (da, dr) = if j + 1 < np { (prof[j + 1].0 - a, prof[j + 1].1 - r) } else { (a - prof[j - 1].0, r - prof[j - 1].1) };
            let (na, nr) = { let l = (da * da + dr * dr).sqrt().max(1e-12); (dr / l, -da / l) };
            // going from +x to -x over the top, (dr, -da) points outward
            let pos = [a as f32, (r * sn) as f32, (r * cs) as f32];
            let nrm = [na as f32, (nr * sn) as f32, (nr * cs) as f32];
            // along u (the circumference); w = -1 like the stock tyre
            let tan = [0.0, cs as f32, -sn as f32, -1.0];
            g.verts.push(Vert { pos, nrm, uv: [s as f32 / seg as f32, (arc[j] / total) as f32], uv1: [0.0; 2], tan, bone: 0 });
        }
    }
    for s in 0..seg {
        for j in 0..np - 1 {
            let a = (s * np + j) as u32;
            let b = ((s + 1) * np + j) as u32;
            // front faces: cross(b - a, c - a) along the normal, as on every
            // stock mesh (checked by the test below)
            g.idx.extend_from_slice(&[a, b, a + 1, b, b + 1, a + 1]);
        }
    }
    g
}

/// Body vertices (not wheel / suspension / cockpit parts) inside the rubber
/// of a tyre of `radius` and half width `hw` at a corner, at rest: between
/// the rim radius and the tread (the validator's measure, src/validate/rigcheck.rs).
fn intrusion(insts: &[Inst], f: &Frame, c: &str, radius: f64, hw: f64) -> usize {
    let rim = f.tyre(c).rim_radius.min(radius - 0.03);
    let mut n = 0;
    for i in insts.iter().filter(|i| i.under == geometry::Under::Body && !i.cockpit) {
        let m = f.evo_m(&i.world);
        for v in 0..i.mesh.vertex_count {
            let p = i.mesh.position(v);
            let q = f.to_wheel(c, frame::xform(&m, [p[0] as f64, p[1] as f64, p[2] as f64]));
            let r = (q[1] * q[1] + q[2] * q[2]).sqrt();
            if q[0].abs() <= hw - 0.004 && r > rim + 0.015 && r < radius - 0.006 {
                n += 1;
            }
        }
    }
    n
}

fn nominal(rim_radius: f64) -> f64 {
    // AC1 RIM_RADIUS is "1 inch more than nominal" (tyres.ini comment)
    ((2.0 * rim_radius / 0.0254) - 1.0).round().clamp(10.0, 24.0)
}

fn round_to(v: f64, s: f64) -> f64 {
    (v / s).round() * s
}

/// Split a corner's geometry groups (kn5 material -> geometry) into the
/// stock batch slots, largest group first: on a rim the main opaque group
/// is EXT_RIM and everything else EXT_RIM_DECAL; on a blur mesh the main
/// see-through group (texture alpha, or AC1 blend / alpha test) is
/// EXT_RIM_BLUR, the main opaque one EXT_RIM_BLUR_STATIC and the next one
/// EXT_RIM_DECAL_BLUR. Further groups merge into the slot of their kind
/// (their geometry keeps its UVs; the slot wears its main group's material).
fn assign(groups: &BTreeMap<usize, Geo>, k: &Kn5, tex: &Tex, skin: Option<&(&str, std::path::PathBuf)>, blur: bool, merged: &mut usize) -> Vec<(RimSlot, usize, Geo)> {
    let mut order: Vec<(&usize, &Geo)> = groups.iter().collect();
    order.sort_by_key(|(mi, g)| (std::cmp::Reverse(g.tris()), **mi));
    let see_through = |mi: usize| -> bool {
        let m = &k.materials[mi];
        let a = m.texture("txDiffuse").and_then(|d| tex.image(d, skin.map(|s| s.1.as_path()))).map(|(img, _, _)| materials::mean(&img)[3]).unwrap_or(1.0);
        m.blend_mode == 1 || m.alpha_tested || a < 0.9
    };
    let mut out: Vec<(RimSlot, usize, Geo)> = Vec::new();
    let mut put = |slot: RimSlot, mi: usize, g: &Geo, out: &mut Vec<(RimSlot, usize, Geo)>| {
        match out.iter_mut().find(|x| x.0 == slot) {
            Some(x) => {
                geometry::merge(&mut x.2, g);
                *merged += 1;
            }
            None => out.push((slot, mi, g.clone())),
        }
    };
    if !blur {
        // the main rim: the largest group that is not a see-through decal
        let main = order.iter().find(|(mi, _)| !see_through(**mi)).or(order.first()).map(|(mi, _)| **mi);
        for (mi, g) in &order {
            let slot = if Some(**mi) == main { RimSlot::Rim } else { RimSlot::Decal };
            put(slot, **mi, g, &mut out);
        }
    } else {
        for (mi, g) in &order {
            let slot = if see_through(**mi) {
                if out.iter().any(|x| x.0 == RimSlot::Blur) && !out.iter().any(|x| x.0 == RimSlot::DecalBlur) { RimSlot::DecalBlur } else { RimSlot::Blur }
            } else if !out.iter().any(|x| x.0 == RimSlot::BlurStatic) {
                RimSlot::BlurStatic
            } else {
                RimSlot::DecalBlur
            };
            put(slot, **mi, g, &mut out);
        }
    }
    out
}

/// Write rims, blur, tyres and their customisation files; point the actor at them.
#[allow(clippy::too_many_arguments)]
pub fn write(files: &mut Files, id: &str, stem: &str, k: &Kn5, insts: &[Inst], parts: &[usize], f: &Frame, h: &materials::Hints, tex: &mut Tex, t: &Templates, default_skin: Option<(&str, std::path::PathBuf)>, game: Option<&crate::validate::Game>) -> Result<Wheels> {
    let base = format!("content\\cars\\{id}");
    let mut notes = Vec::new();
    // (kn5 material, slot) -> material path; the first material of a slot is
    // <BATCH>.material (the rim design's slots name those), later ones
    // <BATCH>_<n>.material - the batch name itself is always exact
    let mut mats: BTreeMap<(usize, RimSlot), String> = BTreeMap::new();
    let mut used_names: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut rim_paths: BTreeMap<String, (String, String)> = BTreeMap::new();
    let mut merged = 0usize;
    for c in CORNERS {
        let lc = c.to_lowercase();
        let rims = corner_geo(insts, parts, f, c, Kind::Rim, k, h);
        if rims.is_empty() {
            continue;
        }
        let blurs = corner_geo(insts, parts, f, c, Kind::RimBlur, k, h);
        // rim mesh: the largest opaque material is EXT_RIM, the rest one
        // EXT_RIM_DECAL (stock rims carry exactly these two)
        let rim_slots = assign(&rims, k, tex, default_skin.as_ref(), false, &mut merged);
        // blur mesh: the see-through disc EXT_RIM_BLUR, the opaque rest
        // EXT_RIM_BLUR_STATIC (+ one EXT_RIM_DECAL_BLUR); without AC1 blur
        // the static rim is the whole rim (AC1 then shows the rim at every
        // speed) and the disc an empty batch
        let blur_slots = if blurs.is_empty() {
            rim_slots.iter().map(|(s, mi, g)| (if *s == RimSlot::Rim { RimSlot::BlurStatic } else { RimSlot::DecalBlur }, *mi, g.clone())).collect()
        } else {
            assign(&blurs, k, tex, default_skin.as_ref(), true, &mut merged)
        };
        let main_mi = rim_slots.first().map(|x| x.1).unwrap_or(0);
        for (file, slots, need) in [(format!("{stem}_rim_{lc}"), rim_slots, &[RimSlot::Rim][..]), (format!("{stem}_rim_blur_{lc}"), blur_slots, &[RimSlot::Blur, RimSlot::BlurStatic][..])] {
            let mut slots = slots;
            for n in need {
                if !slots.iter().any(|x| x.0 == *n) {
                    let ph = geometry::placeholder("", "");
                    slots.push((*n, main_mi, ph.geo));
                }
            }
            slots.sort_by_key(|x| x.0);
            let mut secs = Vec::new();
            for (slot, mi, g) in slots {
                let key = (mi, slot);
                if !mats.contains_key(&key) {
                    let batch = slot.batch();
                    let n = used_names.entry(batch).or_insert(0);
                    *n += 1;
                    let name = if *n == 1 { batch.to_string() } else { format!("{batch}_{n}") };
                    let mm = materials::build_rim(files, tex, t, &k.materials[mi], default_skin.clone(), slot);
                    let path = format!("{base}\\materials\\{name}.material");
                    files.put(&path, mm.encode());
                    mats.insert(key, path);
                }
                secs.push(Section { name: slot.batch().to_string(), material: mats[&key].clone(), geo: g });
            }
            let mut b = Built { bones: vec![], lods: vec![secs], lod_in: vec![0.0], skinned: false };
            geometry::make_lods(&mut b, &geometry::WHEEL_LODS);
            // every LOD keeps every batch (the renderer looks them up by name)
            for lod in b.lods.iter_mut() {
                for s in lod.iter_mut().filter(|s| s.geo.tris() == 0) {
                    s.geo = geometry::placeholder("", "").geo;
                }
            }
            let mp = format!("{base}\\meshes\\{file}.mesh");
            let tmpl = files.get(&format!("{base}\\meshes\\{stem}_rim_lf.mesh")).cloned();
            files.put(&mp, b.encode(tmpl.as_deref()));
            let e = rim_paths.entry(c.to_string()).or_default();
            if file.contains("_rim_blur_") { e.1 = mp } else { e.0 = mp }
        }
    }
    if merged > 0 {
        notes.push(format!("wheels: {merged} extra rim material group(s) merged into a stock batch (stock rims carry one material per batch)"));
    }
    // a corner without its own rim borrows the other side / the other axle's
    let fallback = |c: &str, rp: &BTreeMap<String, (String, String)>| -> (String, String) {
        let order: &[&str] = match c { "LF" => &["LF", "RF", "LR", "RR"], "RF" => &["RF", "LF", "RR", "LR"], "LR" => &["LR", "RR", "LF", "RF"], _ => &["RR", "LR", "RF", "LF"] };
        let rim = order.iter().filter_map(|o| rp.get(*o)).map(|x| x.0.clone()).find(|p| !p.is_empty()).unwrap_or_default();
        let blur = order.iter().filter_map(|o| rp.get(*o)).map(|x| x.1.clone()).find(|p| !p.is_empty()).unwrap_or_default();
        (rim, blur)
    };
    let paths: BTreeMap<&str, (String, String)> = CORNERS.iter().map(|c| (*c, fallback(c, &rim_paths))).collect();
    if paths.values().all(|p| p.0.is_empty()) {
        notes.push("wheels: no rim geometry under WHEEL_xx - the donor rims stay".into());
        return Ok(Wheels { notes, measured_radius: BTreeMap::new(), tyre_radius: [f.tyres[0].radius, f.tyres[1].radius] });
    }
    // tyres
    let mut measured = BTreeMap::new();
    let mut tyre_meshes: [String; 2] = Default::default();
    let tyre_mat = "content\\cars\\common_assets\\parts\\tyres\\materials\\EXT_TYRE.material".to_string();
    let tyre_mat = if game.map(|g| g.has(&tyre_mat)).unwrap_or(true) { tyre_mat } else { format!("{base}\\materials\\EXT_TYRE.material") };
    let mut sizes = [(0.0, 0.0, 0.0); 2];
    for (ax, (c1, c2)) in [("LF", "RF"), ("LR", "RR")].iter().enumerate() {
        let ty = f.tyres[ax];
        let mut pts = tyre_points(insts, parts, f, c1, k, h);
        if pts.len() < 50 {
            pts = tyre_points(insts, parts, f, c2, k, h);
        }
        if !pts.is_empty() {
            let r = pts.iter().map(|p| p.1).fold(0.0, f64::max);
            measured.insert(c1.to_string(), r);
            if (r - ty.radius).abs() / ty.radius > 0.03 {
                notes.push(format!("wheels: the AC1 {c1} tyre is {r:.3} m in radius, tyres.ini says {:.3} m (tyres.ini wins)", ty.radius));
            }
        } else {
            notes.push(format!("wheels: no tyre mesh under WHEEL_{c1} - stock profile"));
        }
        // fitment: tyres.ini sizes, unless they put more bodywork inside
        // the tyre than the AC1 model's own tyre does (then the model's
        // width; the radius stays, it is the ground contact)
        let mut ty = ty;
        if pts.len() >= 50 {
            let kr = pts.iter().map(|p| p.1).fold(0.0, f64::max);
            let kw = pts.iter().map(|p| p.0.abs()).fold(0.0, f64::max);
            let ours = intrusion(insts, f, c1, ty.radius, ty.width / 2.0) + intrusion(insts, f, c2, ty.radius, ty.width / 2.0);
            let theirs = intrusion(insts, f, c1, kr, kw) + intrusion(insts, f, c2, kr, kw);
            if ours > theirs + 10 && kw * 2.0 < ty.width * 0.97 {
                let narrow = intrusion(insts, f, c1, ty.radius, kw) + intrusion(insts, f, c2, ty.radius, kw);
                notes.push(format!("wheels: {} tyres.ini width {:.3} m puts {ours} body vertices inside the tyre (the AC1 tyre, {:.3} m wide: {theirs}); using the model's width ({narrow})",
                    if ax == 0 { "front" } else { "rear" }, ty.width, kw * 2.0));
                ty.width = kw * 2.0;
            } else if ours >= 25 {
                notes.push(format!("wheels: {} tyres put {ours} body vertices inside the tyre at rest; the AC1 model's own tyre has {theirs}",
                    if ax == 0 { "front" } else { "rear" }));
            }
        }
        let prof = profile(&pts, ty.radius, ty.width, ty.rim_radius);
        let g = lathe(&prof, 80);
        let rim_in = nominal(ty.rim_radius);
        let w_mm = (ty.width * 1000.0).round();
        let ratio = round_to(((ty.radius - rim_in * 0.0254 / 2.0) / ty.width * 100.0).clamp(20.0, 90.0), 5.0);
        sizes[ax] = (w_mm, ratio, rim_in);
        let tag = format!("{}_{}_{}", w_mm as u32, ratio as u32, rim_in as u32);
        let dir = format!("{base}\\parts\\tyres\\meshes\\{tag}");
        let mp = format!("{dir}\\tyre_{tag}{}.mesh", if ax == 1 { "_r" } else { "" });
        let mut b = Built { bones: vec![], lods: vec![vec![Section { name: "EXT_TYRE".into(), material: tyre_mat.clone(), geo: g }]], lod_in: vec![0.0], skinned: false };
        geometry::make_lods(&mut b, &geometry::WHEEL_LODS);
        files.put(&mp, b.encode(None));
        tyre_meshes[ax] = mp;
    }
    // the donor's tyremesh as the template (textures list, shadow texture)
    let donor_tm = files.under(&base).into_iter().find(|p| p.to_lowercase().ends_with(".tyremesh"));
    let tm_tmpl = donor_tm.as_ref().and_then(|p| files.get(p)).and_then(|b| Msg::parse(b).ok()).unwrap_or_default();
    let mut tyremesh = [String::new(), String::new()];
    for ax in 0..2 {
        let ty = f.tyres[ax];
        let (w, r, d) = sizes[ax];
        let mut m = tm_tmpl.clone();
        m.set_f32(2, w as f32);
        m.set_f32(3, r as f32);
        m.set_f32(4, d as f32);
        m.set_f32(5, (ty.width * 1.3) as f32);
        m.set_f32(10, (ty.radius * 2.0) as f32);
        m.set_f32(6, -(ty.radius as f32));
        m.set_str(7, &tyre_meshes[ax]);
        let p = tyre_meshes[ax].replace(".mesh", ".tyremesh");
        files.put(&p, m.encode());
        tyremesh[ax] = p;
    }
    if let Some(d) = donor_tm {
        if !tyremesh.iter().any(|p| p.eq_ignore_ascii_case(&d)) {
            files.remove(&d);
        }
    }
    // rimmesh per axle
    let donor_rm = files.under(&format!("{base}\\skins")).into_iter().find(|p| p.to_lowercase().ends_with(".rimmesh"));
    let rm_tmpl = donor_rm.as_ref().and_then(|p| files.get(p)).and_then(|b| Msg::parse(b).ok()).unwrap_or_default();
    let mut rimmesh = [String::new(), String::new()];
    for (ax, (l, r)) in [("LF", "RF"), ("LR", "RR")].iter().enumerate() {
        let (w, _, d) = sizes[ax];
        let rim_w = round_to(w / 25.4 * 0.85, 0.5);
        let mut m = rm_tmpl.clone();
        m.set_f32(2, d as f32);
        m.set_f32(3, rim_w as f32);
        m.set_str(5, &paths[l].0);
        m.set_str(6, &paths[r].0);
        m.set_str(7, &paths[l].1);
        m.set_str(8, &paths[r].1);
        m.remove(9);
        m.push(9, crate::io::pb::Val::Bytes(tyremesh[ax].as_bytes().to_vec()));
        let p = format!("{base}\\skins\\rim_{stem}_{}{}jx{}.rimmesh", if ax == 0 { "f_" } else { "r_" }, rim_w, d as u32);
        files.put(&p, m.encode());
        rimmesh[ax] = p;
    }
    if let Some(d) = donor_rm {
        files.remove(&d);
    }
    // compatible rim
    if let Some(cr) = files.under(&format!("{base}\\skins")).into_iter().find(|p| p.to_lowercase().ends_with(".compatiblerim")) {
        let mut m = Msg::parse(files.get(&cr).unwrap())?;
        m.set_str(1, &rimmesh[0]);
        m.remove(3);
        m.set_str(4, &rimmesh[1]);
        m.remove(6);
        // keep field order: 1, 3, 4, 6, 7, 10
        let rest: Vec<_> = m.fields.drain(..).collect();
        for fl in rest {
            let n = fl.num;
            m.fields.push(fl);
            if n == 1 {
                m.push(3, crate::io::pb::Val::Bytes(tyremesh[0].as_bytes().to_vec()));
            }
            if n == 4 {
                m.push(6, crate::io::pb::Val::Bytes(tyremesh[1].as_bytes().to_vec()));
            }
        }
        files.put(&cr, m.encode());
    }
    // the actor's base meshes
    let ap = format!("{base}\\{id}.actor");
    if let Some(ab) = files.get(&ap).cloned() {
        let mut a = Msg::parse(&ab)?;
        let car = a.msg_mut(103)?;
        let bm = car.msg_mut(12)?;
        let set = |bm: &mut Msg, field: u32, mesh: &str| -> Result<()> {
            let s = bm.msg_mut(field)?;
            s.set_str(1, mesh);
            Ok(())
        };
        // tyres f2..f5, rims f6..f9, blur f18..f21 (LF, RF, LR, RR)
        for (i, c) in CORNERS.iter().enumerate() {
            set(bm, 2 + i as u32, &tyre_meshes[if c.ends_with('F') { 0 } else { 1 }])?;
            set(bm, 6 + i as u32, &paths[c].0)?;
            set(bm, 18 + i as u32, &paths[c].1)?;
        }
        files.put(&ap, a.encode());
    }
    notes.push(format!("wheels: tyres {}/{}R{} front, {}/{}R{} rear; rims {} meshes", sizes[0].0, sizes[0].1, sizes[0].2, sizes[1].0, sizes[1].1, sizes[1].2,
        rim_paths.values().map(|p| (!p.0.is_empty()) as usize + (!p.1.is_empty()) as usize).sum::<usize>()));
    Ok(Wheels { notes, measured_radius: measured, tyre_radius: [f.tyres[0].radius, f.tyres[1].radius] })
}

/// The radius of a written tyre mesh (LOD0, wheel frame): max over vertices.
pub fn mesh_radius(mesh: &[u8]) -> Option<f64> {
    let top = Msg::parse(mesh).ok()?;
    let lod = top.subs(5).into_iter().next()?;
    let p = lod.bytes(5)?;
    let v: Vec<f32> = p.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect();
    Some(v.chunks_exact(3).map(|c| ((c[1] * c[1] + c[2] * c[2]) as f64).sqrt()).fold(0.0, f64::max))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tyre_is_sized_and_outward() {
        let prof = profile(&[], 0.3, 0.2, 0.2);
        let g = lathe(&prof, 80);
        let r = g.verts.iter().map(|v| ((v.pos[1] * v.pos[1] + v.pos[2] * v.pos[2]) as f64).sqrt()).fold(0.0, f64::max);
        assert!((r - 0.3).abs() < 1e-4, "{r}");
        let w = g.verts.iter().map(|v| v.pos[0].abs()).fold(0.0, f32::max);
        assert!((w - 0.1).abs() < 1e-4, "{w}");
        // a tread triangle faces away from the axle
        let t = &g.idx[..3];
        let p = |i: u32| g.verts[i as usize].pos;
        let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
        let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let n = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
        let v = g.verts[t[0] as usize].nrm;
        assert!(n[0] * v[0] + n[1] * v[1] + n[2] * v[2] > 0.0);
        // uv law
        assert!(g.verts.iter().all(|v| (0.0..=1.0).contains(&v.uv[1])));
    }
}
