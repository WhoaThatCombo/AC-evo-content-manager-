//! The car side: AC1 car folder -> EVO car mod package.
//!
//! `convert` runs the stages in order: identity + donor clone, frame, body
//! (geometry, LODs, materials), wheels and tyres, actor clean-up, animated
//! parts, liveries, identity finish (names, final-state ids, brand),
//! physics (every physics, setup and AI file, `physics`), collider, pruning
//! of donor files nothing references, then the package and its `<id>.json`.
//! The car functions (lights, LEDs, display, cameras, exhaust, ground
//! shadow, driver) run after the actor clean-up; the sound stage edits the
//! same `Files`.

pub mod anim;
pub mod animparts;
pub mod belts;
pub mod brand;
pub mod body;
pub mod cameras;
pub mod display;
pub mod driver;
pub mod exhaust;
pub mod frame;
pub mod fx;
pub mod geometry;
pub mod identity;
pub mod leds;
pub mod lights;
pub mod materials;
pub mod needles;
pub mod physics;
pub mod skins;
pub mod usersettings;
pub mod wheels;

use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::ac1::car::Car;
use crate::files::Files;
use crate::io::pb::{Msg, Val};

pub struct Opts {
    pub ac1: PathBuf,
    pub out: PathBuf,
    /// the mod id (default: `ids::car_id` of the folder)
    pub id: Option<String>,
    pub donor_zip: PathBuf,
    pub refmats: PathBuf,
    /// the game's content.kspkg (stock ids, brands, common assets); None = offline
    pub content: Option<PathBuf>,
}

pub struct Report {
    pub id: String,
    pub package: PathBuf,
    pub notes: Vec<String>,
}

/// Textures any AC1 skin of the car replaces (lower-case file names).
fn skin_textures(car: &Car) -> HashSet<String> {
    let mut out = HashSet::new();
    for s in &car.skins {
        for e in std::fs::read_dir(car.skin_dir(s)).into_iter().flatten().flatten() {
            let n = e.file_name().to_string_lossy().to_lowercase();
            if n.ends_with(".dds") || n.ends_with(".png") || n.ends_with(".jpg") || n.ends_with(".tga") {
                out.insert(n);
            }
        }
    }
    out
}

/// What the donor's actor names that the converted car replaces or drops:
/// car animation clips (the animated-parts stage writes ours), the
/// Abarth's belts and number plate meshes, lamp / LED batch names that no
/// section carries.
fn actor_cleanup(files: &mut Files, id: &str, stem: &str, lamp_sections: &[String]) -> Result<Vec<String>> {
    let base = format!("content\\cars\\{id}");
    let ap = format!("{base}\\{id}.actor");
    let Some(b) = files.get(&ap).cloned() else { return Ok(vec![]) };
    let mut a = Msg::parse(&b)?;
    let car = a.msg_mut(103)?;
    // car animations: every slot loses its clips; legacy lists go
    if let Some(i) = car.idx(27).first().copied() {
        let an = car.open(i)?;
        an.fields.retain(|f| !(2..=19).contains(&f.num));
        for j in 0..an.fields.len() {
            if an.fields[j].num >= 100 {
                let s = an.open(j)?;
                s.remove(1);
            }
        }
    }
    // base meshes: belts (f23 / f24) and the plate (f27) are the Abarth's
    let bm = car.msg_mut(12)?;
    for f in [23u32, 24, 27] {
        if let Some(i) = bm.idx(f).first().copied() {
            let s = bm.open(i)?;
            s.remove(1);
            s.remove(2);
        }
    }
    // lamp batches: our lamp sections; LED batches: none until the LED stage
    if let Some(i) = car.idx(25).first().copied() {
        let lb = car.open(i)?;
        lb.remove(1);
        for n in lamp_sections {
            lb.push(1, Val::Bytes(n.as_bytes().to_vec()));
        }
        lb.fields.sort_by_key(|f| f.num);
    }
    if let Some(i) = car.idx(26).first().copied() {
        car.open(i)?.remove(1);
    }
    files.put(&ap, a.encode());
    let mut removed = 0;
    for p in files.under(&format!("{base}\\animations")) {
        let l = p.to_lowercase();
        if !l.contains("driver_animations") && l.ends_with(".animation") {
            files.remove(&p);
            removed += 1;
        }
    }
    for p in [format!("{base}\\meshes\\{stem}_belt_auto.mesh"), format!("{base}\\meshes\\{stem}_harness_off.mesh")] {
        files.remove(&p);
    }
    Ok(vec![format!("actor: donor car clips ({removed}), belts and plate removed")])
}

/// The AC1 collider -> the collider mesh (one section on the editor
/// default material, like stock colliders). Ours: vertices welded on a 1 cm
/// grid; a collider with no triangles, or more than 1.5 x the body's size,
/// keeps the donor's.
fn collider(files: &mut Files, car: &Car, f: &frame::Frame, id: &str, stem: &str, body: ([f32; 3], [f32; 3])) -> Vec<String> {
    let mut notes = Vec::new();
    let path = format!("content\\cars\\{id}\\collider\\{stem}_collider.mesh");
    let Some(cp) = &car.collider else {
        notes.push("collider: no collider.kn5 - the donor's stays".into());
        return notes;
    };
    let Ok(map) = crate::ac1::kn5_decrypt::open(cp) else {
        notes.push("collider: collider.kn5 is protected - the donor's stays".into());
        return notes;
    };
    if crate::ac1::kn5::protected(&map) {
        notes.push("collider: collider.kn5 is protected - the donor's stays".into());
        return notes;
    }
    let Ok(k) = crate::ac1::kn5::parse(&map) else {
        notes.push("collider: collider.kn5 unreadable - the donor's stays".into());
        return notes;
    };
    let mut dropped = Vec::new();
    let insts = geometry::flatten(&k, &mut dropped);
    let mut g = geometry::Geo::default();
    for i in &insts {
        geometry::append(&mut g, i.mesh, &f.evo_m(&i.world), 0);
    }
    for v in &mut g.verts {
        v.pos = v.pos.map(|x| (x * 100.0).round() / 100.0);
        v.uv = [0.0, 0.0];
        v.nrm = [0.0, 1.0, 0.0];
        v.tan = [1.0, 0.0, 0.0, 1.0];
    }
    geometry::compact(&mut g);
    let b = geometry::Built { bones: vec![], lods: vec![vec![geometry::Section { name: "COLLIDER".into(), material: "editor/default.material".into(), geo: g }]], lod_in: vec![0.0], skinned: false };
    let (lo, hi) = b.bounds();
    let size = |l: [f32; 3], h: [f32; 3]| ((h[0] - l[0]).max(0.0), (h[1] - l[1]).max(0.0), (h[2] - l[2]).max(0.0));
    let (cs, bs) = (size(lo, hi), size(body.0, body.1));
    if b.tris(0) == 0 || cs.0 > bs.0 * 1.5 + 0.2 || cs.2 > bs.2 * 1.5 + 0.2 || cs.1 > bs.1 * 1.5 + 0.2 {
        notes.push(format!("collider: implausible AC1 collider ({} tris, {:.2} x {:.2} x {:.2} m) - the donor's stays", b.tris(0), cs.0, cs.1, cs.2));
        return notes;
    }
    let tmpl = files.get(&path).cloned();
    files.put(&path, b.encode(tmpl.as_deref()));
    notes.push(format!("collider: {} tris", b.tris(0)));
    notes
}

/// Every `content\cars\...` path string the package's files mention
/// (lower case, raw byte scan: a protobuf string runs to the next
/// non-printable byte, so a path may carry trailing bytes - callers match
/// prefixes).
fn mentioned(files: &Files, skip: &HashSet<String>) -> Vec<String> {
    let mut out = Vec::new();
    let pat = b"content\\cars\\";
    for (p, d) in &files.map {
        if skip.contains(&p.to_lowercase()) || p.to_lowercase().ends_with(".texturemips") {
            continue;
        }
        let mut i = 0;
        while let Some(q) = memchr::memmem::find(&d[i..], pat) {
            let s = i + q;
            let mut e = s;
            while e < d.len() && (0x20..0x7f).contains(&d[e]) {
                e += 1;
            }
            out.push(String::from_utf8_lossy(&d[s..e]).to_lowercase());
            // the next path may follow within the printable run (a field
            // tag and length can both be printable bytes)
            i = s + pat.len();
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Drop donor files nothing references any more (materials, textures,
/// meshes, colours, designs), until nothing changes.
fn prune(files: &mut Files, id: &str) -> usize {
    let base = format!("content\\cars\\{id}\\").to_lowercase();
    let prunable = |l: &str| -> bool {
        let rel = &l[base.len()..];
        let dir_ok = ["materials\\", "texture\\", "meshes\\", "customization\\", "parts\\", "skins\\"].iter().any(|d| rel.starts_with(d));
        let ext_ok = [".material", ".texture", ".mesh", ".oemmultilayercolor", ".design", ".cardesign", ".tyremesh", ".rimmesh"].iter().any(|e| rel.ends_with(e));
        dir_ok && ext_ok
    };
    let mut total = 0;
    loop {
        let skip = HashSet::new();
        let refs = mentioned(files, &skip);
        let has = |p: &str| -> bool {
            let i = refs.partition_point(|r| r.as_str() < p);
            refs.get(i).map(|r| r.starts_with(p)).unwrap_or(false)
        };
        let gone: Vec<String> = files.map.keys().filter(|k| {
            let l = k.to_lowercase();
            l.starts_with(&base) && prunable(&l) && !has(&l)
        }).cloned().collect();
        if gone.is_empty() {
            break;
        }
        for g in &gone {
            files.remove(g);
            files.remove(&format!("{g}mips"));
            total += 1;
        }
    }
    total
}

/// LOD0 bounds of an encoded mesh.
pub fn mesh_bounds(files: &Files, path: &str) -> ([f32; 3], [f32; 3]) {
    files.get(path).and_then(|b| Msg::parse(b).ok()).map(|m| {
        let v = |x: Option<Msg>| x.map(|m| [m.f32(1).unwrap_or(0.0), m.f32(2).unwrap_or(0.0), m.f32(3).unwrap_or(0.0)]).unwrap_or([0.0; 3]);
        (v(m.sub(6)), v(m.sub(7)))
    }).unwrap_or(([-1.0, 0.0, -2.0], [1.0, 1.5, 2.0]))
}

/// The car functions after the body: lamps, LEDs, cameras, the dash
/// display, exhaust and the ground shadow.
#[allow(clippy::too_many_arguments)]
fn functions(files: &mut Files, id: &str, stem: &str, car: &Car, k: &crate::ac1::kn5::Kn5, f: &frame::Frame, insts: &[geometry::Inst], plan: &body::Plan,
             written: &[(String, materials::Kind, usize, bool, String)], anim: &animparts::Plan,
             lamp_plan: &lights::LampPlan, led_plan: &leds::LedPlan, disp_plan: &display::DisplayPlan) -> Result<Vec<String>> {
    let mut notes = Vec::new();
    let popups = anim.clips.iter().any(|c| c.slots.contains(&119));
    notes.extend(lights::write(files, id, stem, plan, insts, lamp_plan, written, popups)?);
    notes.extend(leds::write(files, id, plan, insts, led_plan, written)?);
    let body = mesh_bounds(files, &format!("content\\cars\\{id}\\meshes\\{stem}_exterior.mesh"));
    let cams = cameras::eye(car, f, body);
    notes.extend(cameras::write(files, id, car, f, &cams, body)?);
    notes.extend(display::write(files, id, stem, disp_plan, cams.eye)?);
    let (outlets, tier, n) = exhaust::outlets(car, f, k, insts);
    notes.extend(n);
    notes.extend(exhaust::write(files, id, car, &outlets, tier, led_plan.limiter as f64)?);
    notes.extend(shadow(files, id, body)?);
    Ok(notes)
}

/// The body ground shadow, sized and centred on the converted body (ours,
/// calibrated on 10 stock cars: decal length 1.02..1.07 x body length,
/// median 1.04, centred on the body's mid z; width 2.12 on every one).
fn shadow(files: &mut Files, id: &str, body: ([f32; 3], [f32; 3])) -> Result<Vec<String>> {
    let ap = format!("content\\cars\\{id}\\{id}.actor");
    let Some(mut actor) = files.get(&ap).and_then(|b| Msg::parse(b).ok()) else { return Ok(vec![]) };
    let (lo, hi) = body;
    let (w, l) = (hi[0] - lo[0], hi[2] - lo[2]);
    let (sx, sz, pz) = ((1.05 * w).max(2.12), 1.04 * l, (hi[2] + lo[2]) * 0.5);
    let a = actor.msg_mut(103)?;
    let sd = a.msg_mut(28)?;
    sd.set_f32(1, sx);
    sd.set_f32(2, sz);
    sd.set_f32(5, pz);
    sd.remove(8);
    files.put(&ap, actor.encode());
    Ok(vec![format!("shadow: {sx:.2} x {sz:.2} m at z {pz:.3} (body {w:.2} x {l:.2})")])
}

/// Convert one AC1 car.
pub fn convert(o: &Opts) -> Result<Report> {
    let t0 = std::time::Instant::now();
    let (donor_zip, refmats) = if o.donor_zip.as_os_str().is_empty() || o.refmats.as_os_str().is_empty() {
        let (d, r) = crate::batch::donor_paths()?;
        (if o.donor_zip.as_os_str().is_empty() { d } else { o.donor_zip.clone() }, if o.refmats.as_os_str().is_empty() { r } else { o.refmats.clone() })
    } else {
        (o.donor_zip.clone(), o.refmats.clone())
    };
    let car = Car::open(&o.ac1)?;
    let game = match &o.content {
        Some(c) if c.is_file() => Some(crate::validate::Game::open(c)?),
        _ => None,
    };
    let taken = game.as_ref().map(|g| g.ids("cars")).unwrap_or_default();
    let brands = game.as_ref().map(identity::stock_brands).unwrap_or_default();
    let ident = identity::identity(&car, o.id.as_deref(), &taken, &brands);
    let (id, stem) = (ident.id.clone(), ident.stem.clone());
    let mut notes = vec![format!("identity: {} ({} {}, brand {}{})", id, ident.display, ident.year, ident.brand, if ident.stock_brand { ", stock" } else { "" })];
    let donor = crate::files::load_donor(&donor_zip, identity::DONOR_ID).with_context(|| format!("donor {}", donor_zip.display()))?;
    let mut files = identity::clone_donor(donor, &ident);
    let base = ident.base();
    let k = car.kn5()?;
    let frame = frame::Frame::build(&car, &k);
    notes.extend(frame.notes.iter().cloned());
    let insts = geometry::flatten(&k, &mut notes);
    let mut hints = materials::Hints::from_data(car.ini("lights.ini"), car.ini("mirrors.ini"), skin_textures(&car));
    hints.paint = materials::paint_fallback(&k.materials, &hints);
    if !hints.paint.is_empty() {
        let mut v: Vec<&String> = hints.paint.iter().collect();
        v.sort();
        notes.push(format!("paint: no car-paint material; painting {} (named like the body)", v.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
    }
    let templates = materials::Templates::load(&refmats, &files, &id)?;
    // car functions that shape the body's sections (read before the body
    // overwrites the donor's materials)
    let lamp_plan = lights::prepare(&car, &insts, &frame, &mut hints);
    let led_plan = leds::prepare(&car, &insts, &files, &id, &mut hints);
    let disp_plan = display::prepare(&car, &frame, &files, &id);
    notes.extend(lamp_plan.notes.iter().cloned());
    notes.extend(led_plan.notes.iter().cloned());
    let paint_tmpl = files.get(&format!("{base}\\materials\\EXT_SKIN.material")).and_then(|b| Msg::parse(b).ok());
    let donor_ext = files.get(&format!("{base}\\meshes\\{stem}_exterior.mesh")).map(|b| body::bones_of(b)).unwrap_or_default();
    let d = frame.datum;
    let mut anim = animparts::plan(&animparts::Input { k: &k, ac1_dir: &car.dir, data: &car.data, ah: [d[0] as f32, d[1] as f32, d[2] as f32] });
    let plan = body::plan(&k, &insts, &frame, &hints, &donor_ext, Some(&mut anim));
    notes.extend(plan.notes.iter().cloned());
    let mut tex = materials::Tex::new(&k, &id);
    tex.default_skin = car.skins.first().cloned();
    let default_skin = car.skins.first().map(|s| (s.as_str(), car.skin_dir(s)));
    let author = body::Author { files: car.lods.iter().map(|l| l.file.clone()).collect(), hints: &hints, anim: Some(&anim) };
    let written = body::write(&mut files, &id, &stem, &k, &insts, &frame, &plan, &mut tex, &templates, game.as_ref(), default_skin.clone(), &author)?;
    notes.extend(written.notes.iter().cloned());
    let wheels = wheels::write(&mut files, &id, &stem, &k, &insts, &plan.wheel_parts, &frame, &hints, &mut tex, &templates, default_skin.clone(), game.as_ref())?;
    notes.extend(wheels.notes.iter().cloned());
    notes.extend(belts::write(&mut files, &id, &stem, &k, &insts, &frame, &mut tex, &templates, default_skin.clone())?);
    let lamps: Vec<String> = written.sections.iter().filter(|s| s.1 == materials::Kind::Lamp).map(|s| s.0.clone()).collect();
    notes.extend(actor_cleanup(&mut files, &id, &stem, &lamps)?);
    notes.extend(functions(&mut files, &id, &stem, &car, &k, &frame, &insts, &plan, &written.sections, &anim, &lamp_plan, &led_plan, &disp_plan)?);
    notes.extend(animparts::write(&mut files, &id, &anim)?);
    let secs: Vec<skins::Section> = written.sections.iter().map(|(n, kind, m, int, p)| skins::Section { name: n, kind: *kind, material: *m, interior: *int, path: p }).collect();
    let sk = skins::write(&mut files, &ident, &car, &k, &secs, &mut tex, &templates, paint_tmpl)?;
    notes.extend(sk.notes);
    notes.extend(identity::apply(&mut files, &ident, &car)?);
    let phys = physics::write(&mut files, &id, &stem, &car, &frame, o.content.as_deref())?;
    notes.extend(phys.notes);
    // sound: a failed sound stage is rolled back, the donor's (the game's shared) sounds stay
    if let Some(c) = o.content.as_deref().filter(|c| c.is_file()) {
        files.begin();
        match crate::sound::car_stage(&mut files, &id, &car.dir, c, &o.out.join(format!(".sound_{id}"))) {
            Ok(n) => {
                files.commit();
                notes.extend(n);
            }
            Err(e) => {
                files.rollback();
                notes.push(format!("sound: failed ({e:#}) - the donor's sound kept"));
            }
        }
    }
    let body_bounds = {
        let ep = format!("{base}\\meshes\\{stem}_exterior.mesh");
        files.get(&ep).and_then(|b| Msg::parse(b).ok()).map(|m| {
            let v = |x: Option<Msg>| x.map(|m| [m.f32(1).unwrap_or(0.0), m.f32(2).unwrap_or(0.0), m.f32(3).unwrap_or(0.0)]).unwrap_or([0.0; 3]);
            (v(m.sub(6)), v(m.sub(7)))
        }).unwrap_or(([-1.0, 0.0, -2.0], [1.0, 1.5, 2.0]))
    };
    notes.extend(collider(&mut files, &car, &frame, &id, &stem, body_bounds));
    // the driver last: a failed driver stage is rolled back, the donor's stays
    files.begin();
    match driver::write(&mut files, &id, &car, &frame) {
        Ok(n) => {
            files.commit();
            notes.extend(n);
        }
        Err(e) => {
            files.rollback();
            notes.push(format!("driver: failed ({e:#}) - the donor's driver kept"));
        }
    }
    let pruned = prune(&mut files, &id);
    notes.extend(tex.notes.iter().cloned());
    notes.push(format!("textures: {} written, {:.0} MB of mips; {} unreferenced donor files pruned", tex.count, tex.bytes as f64 / 1048576.0, pruned));
    let perf = phys.perf;
    let pkg = crate::files::write_mod(&mut files, &o.out, &crate::files::ModMeta {
        id: &id, mech_preset: &ident.mech_preset(), display_name: &ident.display, perf, source_folder: Some(&car.folder), ac1_traction: Some(&phys.derived.ac1_traction),
    })?;
    notes.push(format!("done in {:.1} s: {} files -> {}", t0.elapsed().as_secs_f64(), files.map.len(), pkg.display()));
    Ok(Report { id, package: pkg, notes })
}

/// Options with the defaults: the game's content.kspkg; the donor and
/// refmats are left empty and found (or built) by `convert` only when the
/// caller does not name them.
pub fn default_opts(ac1: &Path, out: &Path) -> Opts {
    let c = crate::io::gamedir::evo_content();
    Opts { ac1: ac1.to_path_buf(), out: out.to_path_buf(), id: None, donor_zip: PathBuf::new(), refmats: PathBuf::new(), content: c.is_file().then_some(c) }
}

/// The donor's files the physics stage reads (no meshes, textures, sounds),
/// for `physics_dry_run`.
pub fn physics_donor(donor_zip: &Path) -> Result<std::collections::BTreeMap<String, Vec<u8>>> {
    let d = crate::files::load_donor(donor_zip, identity::DONOR_ID)?;
    Ok(d.map.into_iter().filter(|(k, _)| {
        let l = k.to_lowercase();
        ![".mesh", ".texture", ".texturemips", ".bank", ".dds", ".png"].iter().any(|e| l.ends_with(e))
    }).collect())
}

/// Only the physics stage of one AC1 car, on a copy of the donor's physics
/// files: what the converter would write for its drivetrain, engine, tyres,
/// setup and AI, without building the package (a library sweep).
pub fn physics_dry_run(ac1: &Path, donor: &std::collections::BTreeMap<String, Vec<u8>>, content: Option<&Path>) -> Result<serde_json::Value> {
    let car = Car::open(ac1)?;
    let ident = identity::identity(&car, None, &HashSet::new(), &Default::default());
    let mut d = Files::default();
    for (k, v) in donor {
        d.put(k, v.clone());
    }
    let mut files = identity::clone_donor(d, &ident);
    let k = car.kn5()?;
    let frame = frame::Frame::build(&car, &k);
    let r = physics::write(&mut files, &ident.id, &ident.stem, &car, &frame, content)?;
    let cdp = files.find_ci(&format!("{}\\data\\cardata.car", ident.base())).ok_or_else(|| anyhow::anyhow!("no CarData"))?;
    let cd = Msg::parse(files.get(&cdp).unwrap())?;
    let dt = car.ini("drivetrain.ini");
    let sec = |s: &str| dt.section(s).map(|m| m.iter().map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone()))).collect::<serde_json::Map<_, _>>());
    let ctrl: Vec<&String> = car.data.keys().filter(|k| k.starts_with("ctrl_") && (k.contains("awd") || k.contains("lock") || k.contains("diff"))).collect();
    let setup_diff = cd.str(19).and_then(|p| files.get(&p).cloned()).and_then(|b| Msg::parse(&b).ok())
        .and_then(|s| s.sub(1)).and_then(|g| g.sub(4))
        .map(|m| [m.f32(1).unwrap_or(0.0), m.f32(2).unwrap_or(0.0), m.f32(3).unwrap_or(0.0)]);
    let (de, dw) = physics::dtcheck::check(&cd, Some(&r.derived.ac1_traction));
    Ok(serde_json::json!({
        "car": car.folder,
        "ac1": { "traction": r.derived.ac1_traction, "differential": sec("DIFFERENTIAL"), "awd": sec("AWD"), "awd2": sec("AWD2"), "ctrl": ctrl },
        "evo": physics::dtcheck::summary(&cd),
        "setup_diff": setup_diff,
        "check": { "errors": de, "warnings": dw },
        "notes": r.notes.iter().filter(|n| n.contains("drivetrain") || n.contains("AWD") || n.contains("lock")).collect::<Vec<_>>(),
    }))
}
