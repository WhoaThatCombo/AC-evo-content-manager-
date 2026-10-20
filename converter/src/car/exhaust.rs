//! Exhaust emitters and backfires (requirements car-cameras-exhaust-actor.md).
//!
//! # What the game wants (stock actors)
//!
//! - `particle_emitters` (CarActorData f40): {f1 name, f2 attachment bone,
//!   f3 offset transform (car space), f7 CarParticleEmitterEnum}. Exhaust
//!   kinds are 2, 3, 4 (Exhaust_1..3 = backfire type + 1, which picks
//!   `common_assets\emitters\backfire_type_<n>`). Over the 72 stock actors:
//!   road cars use 2, race cars 3, a few loud exotics 4; another value (5)
//!   shows no exhaust effect at all.
//! - `.carexhaustbackfire` (CarExhaustBackFireData): f1 high / f2 low /
//!   f3 min rpm, f4 high rate, f5 blip time, f6 delay, f7 variation, f8
//!   time, f10..f14 audio and f20..f24 visual probabilities of the five
//!   types. Stock rpm thresholds sit at a fixed share of the limiter
//!   (mx5_na 6000 / 4500 / 3500 at ~7200, f2004 16000 / 12000 / 8000 at
//!   ~19000: about 0.83 / 0.62 / 0.45).
//!
//! # From AC1
//!
//! `flames.ini` `[FLAME_n]` POSITION / DIRECTION (kn5 frame; f2004's
//! (0.218, 0.46, -1.23) is the stock emitter (0.22, 0.46, -1.242)), the
//! `[HEADER]` INTENSITY and the flames' LSIZE x SIZE_MULT; `sounds.ini
//! [BACKFIRE]`.
//!
//! # Ours (pop law)
//!
//! - kind: 3 for race cars (ui tags / slick tyres), 4 when the AC1 flames are
//!   big (INTENSITY x size >= 40: drift cars), else 2;
//! - flame tier 0..3 (no flames / small / medium / big) scales the visual
//!   probabilities: (1,1,1,1,0) for none or small (mx5_na, whose AC1 twin has
//!   no flames.ini), (5,5,3,2,1) medium (f2004), (10,10,8,6,4) big;
//! - audio probabilities 5 (mx5_na) with a [BACKFIRE] section or flames,
//!   100 (f2004) for race cars with medium or big flames, else 2;
//! - Without flames.ini the emitters go to the car's `EXHAUST*` dummies
//!   (Kunos cars: bmw_m3_e30's EXHAUST1 / EXHAUST2), else to the tips of the
//!   exhaust meshes (the rearmost points of meshes named exhaust / muffler /
//!   tailpipe), else the donor's stay where they are with their kind,
//!   neutralised (no visual backfire).

use anyhow::Result;

use crate::ac1::car::Car;
use crate::car::frame::{self, Frame};
use crate::car::fx;
use crate::car::geometry::Inst;
use crate::files::Files;
use crate::io::pb::{Field, Msg, Val};

pub struct Outlet {
    pub pos: [f64; 3],
    pub dir: [f64; 3],
    pub left: bool,
}

/// A race car by its AC1 ui tags / class or slick tyres.
pub fn race_car(car: &Car) -> bool {
    let ui = car.ui_text.to_lowercase();
    let tags = ui.split("\"tags\"").nth(1).map(|t| t.split(']').next().unwrap_or("").to_string()).unwrap_or_default();
    let class_race = ui.contains("\"class\"") && ui.split("\"class\"").nth(1).map(|t| t.split(',').next().unwrap_or("").contains("race")).unwrap_or(false);
    let slicks = car.ini("tyres.ini").sections.values().any(|kv| kv.get("NAME").map(|n| {
        let n = n.to_lowercase();
        n.contains("slick") && !n.contains("semi")
    }).unwrap_or(false));
    tags.contains("race") || class_race || slicks
}

/// The outlets and the flame tier (0 none, 1 small, 2 medium, 3 big).
pub fn outlets(car: &Car, f: &Frame, k: &crate::ac1::kn5::Kn5, insts: &[Inst]) -> (Vec<Outlet>, u32, Vec<String>) {
    let mut notes = Vec::new();
    let fl = car.ini("flames.ini");
    let mut out = Vec::new();
    let mut size = 0.0;
    for (sec, kv) in fl.sections.iter() {
        if !sec.starts_with("FLAME_") {
            continue;
        }
        let p = kv.get("POSITION").map(|s| crate::ac1::ini::nums_of(s)).unwrap_or_default();
        if p.len() < 3 {
            continue;
        }
        let d = kv.get("DIRECTION").map(|s| crate::ac1::ini::nums_of(s)).filter(|d| d.len() >= 3).unwrap_or(vec![0.0, 0.0, -1.0]);
        let ls = kv.get("LSIZE").and_then(|s| crate::ac1::ini::num_of(s)).unwrap_or(0.3);
        let sm = kv.get("SIZE_MULT").and_then(|s| crate::ac1::ini::num_of(s)).unwrap_or(1.0);
        size += ls * sm;
        let left = kv.get("IS_LEFT").map(|s| s.trim() == "1").unwrap_or(p[0] > 0.0);
        out.push(Outlet { pos: f.evo([p[0], p[1], p[2]]), dir: [d[0], d[1], d[2]], left });
    }
    let tier = if out.is_empty() {
        0
    } else {
        let intensity = fl.num("HEADER", "INTENSITY").unwrap_or(30.0);
        let score = intensity * size / out.len() as f64;
        notes.push(format!("exhaust: {} flame outlet(s), intensity {intensity:.0}, flame score {score:.1}", out.len()));
        if score >= 40.0 { 3 } else if score >= 10.0 || race_car(car) { 2 } else { 1 }
    };
    if out.is_empty() {
        // Kunos cars mark their outlets with empty EXHAUST* dummies
        // (bmw_m3_e30: EXHAUST1 / EXHAUST2 at the tail pipes)
        let mut dummies: Vec<(&String, &crate::car::frame::M4)> = f.node_world.iter().filter(|(n, _)| n.to_uppercase().starts_with("EXHAUST")).collect();
        dummies.sort_by(|a, b| a.0.cmp(b.0));
        for (_, m) in dummies.into_iter().take(4) {
            let p = f.evo([m[12], m[13], m[14]]);
            if p[2] < 0.0 {
                out.push(Outlet { pos: p, dir: [0.0, 0.0, -1.0], left: p[0] > 0.0 });
            }
        }
        if !out.is_empty() {
            notes.push(format!("exhaust: no flames.ini - {} outlet(s) at the EXHAUST dummies, no visual backfire", out.len()));
        }
    }
    if out.is_empty() {
        // the tips of the exhaust meshes: per side, the rearmost vertex
        let mut tips: Vec<[f64; 3]> = Vec::new();
        let keys = ["exhaust", "muffler", "tailpipe", "tail_pipe", "scarico", "silencer"];
        for i in insts.iter().filter(|i| !i.cockpit) {
            let n = i.node.to_lowercase();
            let mat = k.materials.get(i.material).map(|m| m.name.to_lowercase()).unwrap_or_default();
            if !keys.iter().any(|k| n.contains(k) || mat.contains(k)) {
                continue;
            }
            // per side of the car: one mesh may hold both pipes
            let mut best: [Option<[f64; 3]>; 2] = [None, None];
            for v in 0..i.mesh.vertex_count {
                let p = i.mesh.position(v);
                let q = f.evo(frame::xform(&i.world, [p[0] as f64, p[1] as f64, p[2] as f64]));
                let s = (q[0] < 0.0) as usize;
                if best[s].map(|b| q[2] < b[2]).unwrap_or(true) {
                    best[s] = Some(q);
                }
            }
            // a side whose rearmost point is well ahead of the other's has no pipe
            let zmin = best.iter().flatten().map(|b| b[2]).fold(f64::MAX, f64::min);
            for b in best.into_iter().flatten().filter(|b| b[2] < zmin + 0.15) {
                if b[2] < 0.0 && !tips.iter().any(|t| (t[0] - b[0]).abs() < 0.08 && (t[2] - b[2]).abs() < 0.1) {
                    tips.push(b);
                }
            }
        }
        tips.truncate(4);
        for t in tips {
            out.push(Outlet { pos: t, dir: [0.0, 0.0, -1.0], left: t[0] > 0.0 });
        }
        if !out.is_empty() {
            notes.push(format!("exhaust: no flames.ini - {} outlet(s) at the exhaust mesh tips, no visual backfire", out.len()));
        } else {
            notes.push("exhaust: no flames.ini and no exhaust mesh - the donor's emitters stay, no visual backfire".into());
        }
    }
    (out, tier, notes)
}

pub fn write(files: &mut Files, id: &str, car: &Car, outlets: &[Outlet], tier: u32, limiter: f64) -> Result<Vec<String>> {
    let base = format!("content\\cars\\{id}");
    let ap = format!("{base}\\{id}.actor");
    let Some(mut actor) = files.get(&ap).and_then(|b| Msg::parse(b).ok()) else { return Ok(vec![]) };
    let mut notes = Vec::new();
    let race = race_car(car);
    let kind: u64 = if tier >= 3 { 4 } else if race { 3 } else { 2 };
    let a = actor.msg_mut(103)?;
    let ems = a.subs(40);
    let is_exh = |m: &Msg| matches!(m.varint(7), Some(2..=5));
    let tmpl = ems.iter().find(|m| is_exh(m)).cloned().unwrap_or_default();
    let mut new = Vec::new();
    if outlets.is_empty() {
        // neutralised donor emitters: kind kept (5 would show nothing at all)
        for m in ems.iter().filter(|m| is_exh(m)) {
            let mut m = m.clone();
            if m.varint(7) == Some(5) {
                m.set_varint(7, 2);
            }
            new.push(m);
        }
    } else {
        let (mut nl, mut nr) = (0, 0);
        let nleft = outlets.iter().filter(|o| o.left).count();
        let nright = outlets.len() - nleft;
        for o in outlets {
            let side = if o.left { "L" } else { "R" };
            let k = if o.left { nl += 1; nl } else { nr += 1; nr };
            let many = if o.left { nleft > 1 } else { nright > 1 };
            let name = if outlets.len() == 1 { "EXHAUST".to_string() } else if many { format!("EXHAUST {side}{k}") } else { format!("EXHAUST {side}") };
            let mut m = tmpl.clone();
            m.set_str(1, &name);
            m.remove(2);
            let r = fx::euler_minus_z(o.dir);
            // emitters keep yaw 0 for straight-back pipes like stock
            let rot = [r[0], if (r[1].abs() - 180.0).abs() < 1e-3 { 0.0 } else { r[1] }, 0.0];
            m.set(3, Val::Msg(fx::transform([o.pos[0] as f32, o.pos[1] as f32, o.pos[2] as f32], rot)));
            m.set_varint(7, kind);
            m.fields.sort_by_key(|f| f.num);
            new.push(m);
        }
    }
    // replace the exhaust emitters, keep the rest (car, tyres) in place
    let first = a.fields.iter().position(|f| f.num == 40).unwrap_or(a.fields.len());
    let rest: Vec<Field> = a.fields.drain(..).collect();
    let mut placed = false;
    for f in rest {
        if f.num == 40 {
            let m = match &f.val { Val::Bytes(b) => Msg::parse(b).unwrap_or_default(), Val::Msg(m) => m.clone(), _ => Msg::default() };
            if is_exh(&m) {
                if !placed {
                    for e in new.drain(..) {
                        a.fields.push(Field { num: 40, val: Val::Msg(e) });
                    }
                    placed = true;
                }
                continue;
            }
        }
        a.fields.push(f);
    }
    if !placed {
        let at = first.min(a.fields.len());
        for (k, e) in new.drain(..).enumerate() {
            a.fields.insert(at + k, Field { num: 40, val: Val::Msg(e) });
        }
    }
    // the backfire script
    let bp = a.str(106).unwrap_or_else(|| format!("{base}\\instruments\\{id}.carexhaustbackfire"));
    a.set_str(106, &bp);
    let snd = car.ini("sounds.ini").has("BACKFIRE");
    let lim = limiter.max(3000.0) as f32;
    let mut b = Msg::default();
    b.set_f32(1, (lim * 0.83 / 100.0).round() * 100.0);
    b.set_f32(2, (lim * 0.62 / 100.0).round() * 100.0);
    b.set_f32(3, (lim * 0.45 / 100.0).round() * 100.0);
    b.set_f32(4, 800.0);
    b.set_f32(5, 0.3);
    b.set_f32(6, if race { 0.2 } else { 0.15 });
    b.set_f32(7, 0.1);
    b.set_f32(8, if race { 0.1 } else { 0.12 });
    let audio = if race && tier > 1 { 100 } else if snd || tier > 0 { 5 } else { 2 };
    // every stock car has some visual backfire; none only where the
    // emitters are the donor's (neutralised)
    let visual: [u64; 5] = if outlets.is_empty() {
        [0; 5]
    } else {
        match tier {
            0 | 1 => [1, 1, 1, 1, 0],
            2 => [5, 5, 3, 2, 1],
            _ => [10, 10, 8, 6, 4],
        }
    };
    for k in 0..5u32 {
        if audio > 0 {
            b.set_varint(10 + k, if k == 4 && audio == 5 { 2 } else { audio });
        }
    }
    for (k, v) in visual.iter().enumerate() {
        if *v > 0 {
            b.set_varint(20 + k as u32, *v);
        }
    }
    files.put(&bp, b.encode());
    files.put(&ap, actor.encode());
    notes.push(format!("exhaust: {} emitter(s) of kind {kind} (Exhaust_{}), flame tier {tier}, backfire at {:.0} / {:.0} / {:.0} rpm, visual {:?}",
        outlets.len().max(0), kind - 1, lim * 0.83, lim * 0.62, lim * 0.45, visual));
    Ok(notes)
}
