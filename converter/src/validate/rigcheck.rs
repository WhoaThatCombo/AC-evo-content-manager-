//! Data-only car rig checks for the validator: the cheap versions of what
//! evoview `--check` finds by posing the car (no renderer, no posing loop).
//!
//! - every needle of the .caranalogicsystem names a bone that carries
//!   vertices (a gauge on an empty bone - the donor's turbo - shows nothing);
//! - the actor binds car animation clips (doors, wipers...); every clip track
//!   names an existing bone with geometry under it;
//! - the driver's accel / brake layers actually move; the steer layer is one
//!   full turn each way (its end frames equal its middle frame, like every
//!   stock clip);
//! - at rest no bodywork sits inside a tyre;
//! - brake / hub parts stay at their wheel (a bone under HUB/SUSP whose
//!   origin is far from the wheel swings round it when steering).

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::io::pb::{Msg, Val};

pub struct Found {
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

type M = [f32; 16];

fn mul(a: &M, b: &M) -> M {
    let mut o = [0f32; 16];
    for r in 0..4 {
        for c in 0..4 {
            o[r * 4 + c] = (0..4).map(|k| a[r * 4 + k] * b[k * 4 + c]).sum();
        }
    }
    o
}

fn mat(m: &Msg) -> M {
    let mut o = [0f32; 16];
    for (i, v) in o.iter_mut().enumerate() {
        *v = m.f32(i as u32 + 1).unwrap_or(0.0);
    }
    o
}

fn f32s(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect()
}

struct Mesh {
    names: Vec<String>,
    world: Vec<M>,
    parent: Vec<Option<usize>>,
    pos: Vec<[f32; 3]>,
    /// dominant bone per vertex
    bone: Vec<usize>,
}

fn load_mesh(b: &[u8]) -> Option<Mesh> {
    let top = Msg::parse(b).ok()?;
    let lod = top.subs(5).into_iter().next()?;
    let bones = lod.subs(12);
    let names: Vec<String> = bones.iter().map(|x| x.str(1).unwrap_or_default()).collect();
    let parents: Vec<String> = bones.iter().map(|x| x.str(2).unwrap_or_default()).collect();
    let locals: Vec<M> = bones.iter().map(|x| x.sub(3).map(|m| mat(&m)).unwrap_or([1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.])).collect();
    let parent: Vec<Option<usize>> = parents.iter().enumerate().map(|(i, p)| names.iter().position(|n| n == p).filter(|&j| j != i)).collect();
    let mut world: Vec<Option<M>> = vec![None; names.len()];
    fn get(i: usize, l: &[M], p: &[Option<usize>], w: &mut Vec<Option<M>>, d: usize) -> M {
        if let Some(m) = w[i] {
            return m;
        }
        let m = match p[i] {
            Some(j) if d < 64 => mul(&l[i], &get(j, l, p, w, d + 1)),
            _ => l[i],
        };
        w[i] = Some(m);
        m
    }
    for i in 0..names.len() {
        get(i, &locals, &parent, &mut world, 0);
    }
    let pos: Vec<[f32; 3]> = lod.bytes(5).map(f32s).unwrap_or_default().chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
    let bid = lod.bytes(10).map(f32s).unwrap_or_default();
    let wgt = lod.bytes(9).map(f32s).unwrap_or_default();
    let bone: Vec<usize> = (0..pos.len()).map(|v| {
        if bid.len() < (v + 1) * 4 {
            return 0;
        }
        let mut best = (f32::MIN, 0usize);
        for k in 0..4 {
            let w = wgt.get(v * 4 + k).copied().unwrap_or(if k == 0 { 1.0 } else { 0.0 });
            if w > best.0 {
                best = (w, bid[v * 4 + k].max(0.0) as usize);
            }
        }
        best.1
    }).collect();
    Some(Mesh { names, world: world.into_iter().map(|m| m.unwrap()).collect(), parent, pos, bone })
}

/// (bone name -> vertices on it or on a descendant)
fn geometry_under(m: &Mesh) -> HashMap<String, usize> {
    let mut own = vec![0usize; m.names.len()];
    for &b in &m.bone {
        if b < own.len() {
            own[b] += 1;
        }
    }
    let mut out: HashMap<String, usize> = HashMap::new();
    for i in 0..m.names.len() {
        let mut j = Some(i);
        let mut g = 0;
        while let Some(x) = j {
            *out.entry(m.names[x].clone()).or_default() += own[i];
            j = m.parent[x];
            g += 1;
            if g > 64 {
                break;
            }
        }
    }
    out
}

/// Tracks of an .animation: (bone, frames).
fn tracks(b: &[u8]) -> Vec<(String, Vec<M>)> {
    let Ok(top) = Msg::parse(b) else { return vec![] };
    let Some(take) = top.sub(2) else { return vec![] };
    take.subs(3).iter().map(|t| (t.str(1).unwrap_or_default(), t.subs(2).iter().map(mat).collect())).collect()
}

fn frames_differ(a: &M, b: &M, tol: f32) -> bool {
    a.iter().zip(b).any(|(x, y)| (x - y).abs() > tol)
}

fn slot_paths(m: &Msg) -> Vec<String> {
    m.subs(1).iter().filter_map(|r| r.str(1)).collect()
}

pub fn check(files: &BTreeMap<String, Vec<u8>>, id: &str) -> Found {
    let mut f = Found { warnings: Vec::new(), errors: Vec::new() };
    let get = |p: &str| -> Option<&Vec<u8>> {
        files.get(p).or_else(|| files.iter().find(|(k, _)| k.eq_ignore_ascii_case(p)).map(|(_, v)| v))
    };
    let ap = format!("content\\cars\\{id}\\{id}.actor");
    let Some(actor) = get(&ap).and_then(|b| Msg::parse(b).ok()) else { return f };
    let Some(a) = actor.sub(103) else { return f };
    // ---- meshes: body (no bone), tyres (TYRE_xx)
    let mut body: Vec<(String, Mesh)> = Vec::new();
    let mut tyres: Vec<(String, String)> = Vec::new();
    if let Some(parts) = a.sub(12) {
        for fl in &parts.fields {
            let Val::Bytes(b) = &fl.val else { continue };
            // f12.f1 is the exterior mesh's path itself; the parts follow
            if fl.num == 1 && b.ends_with(b".mesh") {
                let mesh = String::from_utf8_lossy(b).into_owned();
                match get(&mesh).and_then(|x| load_mesh(x)) {
                    Some(m) => body.push((mesh.to_lowercase(), m)),
                    None => f.warnings.push(format!("rig check: body mesh {mesh} unreadable")),
                }
                continue;
            }
            let Ok(e) = Msg::parse(b) else { continue };
            let (Some(mesh), bone) = (e.str(1), e.str(2)) else { continue };
            match bone {
                None => match get(&mesh).and_then(|b| load_mesh(b)) {
                    Some(m) => body.push((mesh.to_lowercase(), m)),
                    None => f.warnings.push(format!("rig check: body mesh {mesh} unreadable")),
                },
                Some(bn) if bn.starts_with("TYRE_") => tyres.push((bn[5..].to_string(), mesh)),
                _ => {}
            }
        }
    }
    let ext = body.iter().find(|(p, _)| p.ends_with("exterior.mesh")).map(|x| &x.1);
    let under: HashMap<String, usize> = {
        let mut u = HashMap::new();
        for (_, m) in &body {
            for (k, v) in geometry_under(m) {
                *u.entry(k).or_insert(0) += v;
            }
        }
        u
    };
    let bones: HashSet<String> = body.iter().flat_map(|(_, m)| m.names.iter().cloned()).collect();
    // ---- needles
    if let Some(ap) = a.str(103) {
        match get(&ap).and_then(|b| Msg::parse(b).ok()) {
            None => f.errors.push(format!("analogic system {ap} is missing or unreadable")),
            Some(an) => {
                for fl in &an.fields {
                    let Val::Bytes(b) = &fl.val else { continue };
                    let Some(bone) = Msg::parse(b).ok().and_then(|e| e.str(1)) else { continue };
                    if !bones.contains(&bone) {
                        f.warnings.push(format!("needle (analogic field {}) bone {bone} is not in the car's meshes", fl.num));
                    } else if under.get(&bone).copied().unwrap_or(0) == 0 {
                        f.warnings.push(format!("needle (analogic field {}) bone {bone} has no geometry - the gauge shows nothing", fl.num));
                    }
                }
            }
        }
    }
    // ---- exhaust emitters: kind = backfire type + 1 (2..4) on every stock car
    for fl in a.fields.iter().filter(|x| x.num == 40) {
        let Val::Bytes(b) = &fl.val else { continue };
        let Ok(e) = Msg::parse(b) else { continue };
        let name = e.str(1).unwrap_or_default();
        if name.to_uppercase().starts_with("EXHAUST") && !matches!(e.varint(7), Some(2..=4)) {
            f.warnings.push(format!("exhaust emitter {name}: kind {:?} is no backfire type (2..4) - the game shows no exhaust effects", e.varint(7)));
        }
    }
    // ---- LED rules need an LED material to light
    if let Some(lp) = a.str(102).filter(|p| p.to_lowercase().ends_with(".carledsystem")) {
        if let Some(ls) = get(&lp).and_then(|b| Msg::parse(b).ok()) {
            let rules = ls.fields.iter().filter(|x| x.num != 33 && x.num != 35).filter_map(|x| match &x.val { Val::Bytes(b) if !b.is_empty() => Some(()), _ => None }).count();
            // an INT_LED section with triangles in a body mesh (the donor ships the material either way)
            let has_led = body.iter().any(|(p, _)| {
                get(p).and_then(|b| Msg::parse(b).ok()).and_then(|t| t.subs(5).into_iter().next())
                    .map(|l| l.subs(4).iter().any(|s| s.str(1).as_deref() == Some("INT_LED") && s.varint(3).unwrap_or(0) > 0))
                    .unwrap_or(false)
            });
            if rules > 0 && !has_led {
                f.warnings.push(format!("LED system has {rules} rule group(s) but the car has no INT_LED material (a donor template?)"));
            }
            if let (Some(lim), Some(r)) = (ls.varint(33), ls.sub(1)) {
                let top = r.subs(1).iter().filter_map(|e| e.f32(2)).fold(0f32, f32::max);
                if top > lim as f32 * 1.05 {
                    f.warnings.push(format!("shift lights run to {top:.0} rpm, past the LED limiter {lim}"));
                }
            }
        }
    }
    // ---- car animation clips
    let mut n_car = 0;
    if let Some(ca) = a.sub(27) {
        for fl in &ca.fields {
            if !(102..=122).contains(&fl.num) {
                continue;
            }
            let Val::Bytes(b) = &fl.val else { continue };
            let Ok(slot) = Msg::parse(b) else { continue };
            for p in slot_paths(&slot) {
                n_car += 1;
                let Some(ab) = get(&p) else { continue };
                let tr = tracks(ab);
                let dead: Vec<&str> = tr.iter().filter(|(b, _)| under.get(b).copied().unwrap_or(0) == 0).map(|(b, _)| b.as_str()).collect();
                if !tr.is_empty() && dead.len() == tr.len() {
                    f.warnings.push(format!("car clip {} (slot {}): no track names a bone with geometry ({})", p.rsplit('\\').next().unwrap_or(""), fl.num, dead.iter().take(3).cloned().collect::<Vec<_>>().join(", ")));
                }
            }
        }
    }
    if n_car == 0 {
        f.warnings.push("the actor binds no car animation clips (doors, wipers, shifter, pop-ups...)".into());
    }
    // ---- driver layers
    if let Some(da) = a.sub(31) {
        for (num, what) in [(102u32, "accel"), (103, "brake"), (101, "steer")] {
            let Some(slot) = da.sub(num) else { continue };
            for p in slot_paths(&slot) {
                let Some(ab) = get(&p) else { continue };
                let tr = tracks(ab);
                if tr.is_empty() {
                    continue;
                }
                if what != "steer" {
                    if !tr.iter().any(|(_, fr)| fr.iter().any(|x| frames_differ(x, &fr[0], 1e-4))) {
                        f.warnings.push(format!("driver {what} clip moves nothing (the feet never press)"));
                    }
                } else {
                    // a full turn: at both ends the hands are back where the
                    // middle frame has them (chained shoulder -> hand from the
                    // clip's own tracks; elbows and fingers may differ a little)
                    let find = |n: &str| tr.iter().find(|(b, _)| b == n).map(|(_, f)| f);
                    let mut off = false;
                    let mut chained = false;
                    for chain in [["RIGHT_Shoulder", "RightArm", "RighttForeArm", "RightHand"], ["LEFT_Shoulder", "LeftArm", "LeftForeArm", "LeftHand"]] {
                        let fs: Vec<&Vec<M>> = chain.iter().filter_map(|n| find(n)).collect();
                        if fs.len() != 4 || fs.iter().any(|f| f.len() < 3 || f.len() != fs[0].len()) {
                            continue;
                        }
                        chained = true;
                        let hand = |i: usize| -> [f32; 3] {
                            // row-vector: hand world (in the shoulder's parent frame) = Lh * Lf * La * Ls
                            let m = mul(&mul(&mul(&fs[3][i], &fs[2][i]), &fs[1][i]), &fs[0][i]);
                            [m[12], m[13], m[14]]
                        };
                        let n = fs[0].len();
                        let mid = hand((n - 1) / 2);
                        for e in [hand(0), hand(n - 1)] {
                            let d = ((e[0] - mid[0]).powi(2) + (e[1] - mid[1]).powi(2) + (e[2] - mid[2]).powi(2)).sqrt();
                            if d > 0.03 {
                                off = true;
                            }
                        }
                    }
                    if !chained {
                        off = tr.iter().filter(|(_, fr)| fr.len() > 2).any(|(_, fr)| {
                            let mid = &fr[(fr.len() - 1) / 2];
                            frames_differ(&fr[0], mid, 0.05) || frames_differ(&fr[fr.len() - 1], mid, 0.05)
                        });
                    }
                    if off {
                        f.warnings.push("driver steer clip is not a full turn each way (its ends are not its centre pose)".into());
                    }
                }
            }
        }
    }
    // ---- tyres at rest, wheel-carried bones
    if let Some(ext) = ext {
        let idx: HashMap<&str, usize> = ext.names.iter().enumerate().map(|(i, n)| (n.as_str(), i)).collect();
        let wheel_part = |b: usize| -> bool {
            let mut j = Some(b);
            let mut g = 0;
            while let Some(x) = j {
                let n = &ext.names[x];
                if ["SUSP", "DIR_", "WHEEL", "TYRE", "DISC", "HUB"].iter().any(|k| n.contains(k)) {
                    return true;
                }
                j = ext.parent[x];
                g += 1;
                if g > 64 {
                    break;
                }
            }
            false
        };
        for (c, mesh) in &tyres {
            let (Some(&ti), Some(tb)) = (idx.get(format!("TYRE_{c}").as_str()), get(mesh)) else { continue };
            let Some(t) = load_mesh(tb) else { continue };
            let (mut ro, mut ri, mut hw) = (0f32, f32::MAX, 0f32);
            for p in &t.pos {
                let r = p[1].hypot(p[2]);
                ro = ro.max(r);
                ri = ri.min(r);
                hw = hw.max(p[0].abs());
            }
            if ro < 0.1 {
                continue;
            }
            let w = ext.world[ti];
            let (cx, cy, cz) = (w[12], w[13], w[14]);
            let mut n = 0;
            let mut deep = 0f32;
            for (v, p) in ext.pos.iter().enumerate() {
                if wheel_part(ext.bone[v]) {
                    continue;
                }
                let (x, y, z) = (p[0] - cx, p[1] - cy, p[2] - cz);
                if x.abs() > hw - 0.004 {
                    continue;
                }
                let r = y.hypot(z);
                if r > ri + 0.015 && r < ro - 0.006 {
                    n += 1;
                    deep = deep.max(ro - r);
                }
            }
            if n >= 25 {
                f.warnings.push(format!("at rest {n} body vertices sit inside the {c} tyre (up to {:.0} mm deep)", deep * 1000.0));
            }
        }
        // bones under a wheel carrier must stay near that wheel
        for c in ["LF", "RF", "LR", "RR"] {
            let Some(&wi) = idx.get(format!("WHEEL_{c}").as_str()) else { continue };
            let wc = ext.world[wi];
            for (i, n) in ext.names.iter().enumerate() {
                let mut j = ext.parent[i];
                let mut on = false;
                while let Some(x) = j {
                    if ext.names[x] == format!("HUB_{c}") || ext.names[x] == format!("SUSP_{c}") {
                        on = true;
                        break;
                    }
                    j = ext.parent[x];
                }
                if !on || n.starts_with("DIR_") || under.get(n).copied().unwrap_or(0) == 0 {
                    continue;
                }
                let b = ext.world[i];
                let d = ((b[12] - wc[12]).powi(2) + (b[13] - wc[13]).powi(2) + (b[14] - wc[14]).powi(2)).sqrt();
                if d > 0.6 {
                    f.errors.push(format!("bone {n} hangs off the {c} wheel carrier {d:.2} m from the wheel - it swings round when steering"));
                }
            }
        }
    }
    f
}
