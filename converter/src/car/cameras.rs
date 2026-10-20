//! Cameras, mirrors, probes and the showroom framing
//! (requirements car-cameras-exhaust-actor.md).
//!
//! # What the game wants (CarActorData, stock actors)
//!
//! - `drivable_cameras` (f10): Cockpit, Dash, Bonnet, Bumper, NearChase,
//!   FarChase, each {f1 transform, f2 CameraData {f2 name, f14 {f2 fov,
//!   f13 mode}}}. Positions are in car space (origin on the ground between the
//!   axles, +X left, +Z forward), not relative to a bone: CameraData `offset`
//!   is empty on every stock car. A camera looks along its local -Z; Euler
//!   degrees (x pitch, y yaw, 0) with R = Ry * Rx, so (pitch, 180, 0) looks
//!   along the car's +Z and a negative pitch looks down (stock bonnet and
//!   chase cameras). FOV 55 on every stock drivable camera.
//! - `cameras` (f1): the onboard (F6) cameras, 6..9 per stock car, FOV 55..60.
//! - `mirrors` (f23): one per MIRROR_0 / 1 / 2 bone (the body stage puts
//!   the bones on the AC1 mirror meshes), offsets zero on stock.
//! - `cockpit_probe_transform` (f17), `rear_camera` (f24), `showroom_camera`
//!   (f32): see `write`.
//!
//! # Measured on the 71 stock cars (body = the exterior mesh bounds)
//!
//! | camera | stock rule (median, p10..p90) |
//! |---|---|
//! | NearChase | y 1.80 (1.80..1.85), z tail - 2.78 (-3.00..-2.69), pitch -5 |
//! | FarChase | y 2.50, z tail - 4.22 (-4.37..-4.09), pitch -5 |
//! | Bumper | y = cockpit y (0 off on 64 of 71), z nose - 0.06, pitch = cockpit pitch |
//! | Bonnet | x 0, 0.15 behind the windscreen base (cowl), median; 0.14 over the glass (0.07..0.21), pitch -5.7 |
//! | Dash | eye + (0, 0.007, 0.196) |
//! | cockpit probe | x 0, y eye + 0.02, z eye + 0.62 (0.49..0.79), scale (2, 1, ~2.2) |
//! | rear camera | offset y 0.8, z -2.2 / -2.5 (tail - 0.10) |
//! | showroom | min_distance 0.61 x length (2.0..2.8); internal view at the eye |
//!
//! # From AC1 (checked on 13 Kunos AC1 / EVO twins)
//!
//! - car.ini `DRIVEREYES`, `dash_cam.ini` POS and `cameras.ini` POSITION /
//!   FORWARD are in the kn5 frame: EVO = AC1 + datum. The twins' stock eyes
//!   land within 1..7 cm (y) of the AC1 eye, even on cars with a large datum
//!   (sport_quattro z -0.44: 1 cm). The AC1 kn5 axes are EVO's (+X left, +Z
//!   forward): cameras.ini CAMERA_3 / CAMERA_4 of ks_mazda_miata (right seat,
//!   looking forward / looking left-back) match mx5_na's onboard cameras 7 / 4.
//! - car.ini `BONNET_CAMERA_POS` and `BUMPER_CAMERA_POS` are in the AC1
//!   physics (CoG) frame: EVO = AC1 - GRAPHICS_OFFSET + datum = AC1 + cog.
//!   In the kn5 frame ks_mazda_miata's bonnet camera would sit at y 0.45 (in
//!   the engine bay) and its bumper camera at y 0.40 inside the nose (6 of 6
//!   axis rays blocked): that was the in-game "hood camera off" bug. Even in
//!   the CoG frame AC1's bonnet camera is an AC1 convention (on the bonnet
//!   panel, 0.08 m over it); EVO's sits over the lower windscreen, so the
//!   bonnet camera follows the stock rule and BONNET_CAMERA_POS is only a
//!   fallback. The bumper follows the stock rule (cockpit height).
//! - The pitches are degrees, negative = down (f2004 bonnet -13.7 vs stock
//!   -14.6, countach -7.0 vs -6.3).
//! - `ON_BOARD_PITCH_ANGLE` does not carry over: twins with -5.4 (ae86,
//!   supra, sport_quattro, 190) have -1.8..+0.2 on stock. OURS: kept inside
//!   the stock p10..p90 range (-1.7..+0.6), else the stock median -0.2.
//! - AC1 has no per-car chase positions: the stock rule above is used.
//! - FOV: cameras.ini FOV (vertical degrees in AC1) is kept; stock onboard
//!   cameras use 55..60, AC1's are 60.

use anyhow::Result;

use crate::ac1::car::Car;
use crate::car::frame::{self, Frame};
use crate::car::fx;
use crate::files::Files;
use crate::io::pb::{Field, Msg, Val};

pub struct Cams {
    /// the cockpit camera (EVO space)
    pub eye: [f64; 3],
    pub pitch: f32,
    pub notes: Vec<String>,
}

fn v3(v: &[f64]) -> Option<[f64; 3]> {
    (v.len() >= 3).then(|| [v[0], v[1], v[2]])
}

/// OURS: the stock cockpit pitch range (p10..p90 of 71 cars).
pub const COCKPIT_PITCH: (f32, f32) = (-1.7, 0.6);
/// The stock cockpit pitch median; the AC1 twins with -5.4 have -1.8..+0.2.
pub const COCKPIT_PITCH_MEDIAN: f32 = -0.2;

pub fn eye(car: &Car, f: &Frame, body: ([f32; 3], [f32; 3])) -> Cams {
    let ci = car.ini("car.ini");
    let mut notes = Vec::new();
    let eye = match v3(&ci.nums("GRAPHICS", "DRIVEREYES")) {
        Some(e) => f.evo(e),
        None => {
            notes.push("cameras: car.ini has no DRIVEREYES - the eye is guessed from the body".into());
            [0.35, body.1[1] as f64 - 0.32, (body.0[2] as f64 + body.1[2] as f64) * 0.5 - 0.3]
        }
    };
    let raw = ci.num("GRAPHICS", "ON_BOARD_PITCH_ANGLE").unwrap_or(0.0) as f32;
    let pitch = if (COCKPIT_PITCH.0..=COCKPIT_PITCH.1).contains(&raw) { raw } else { COCKPIT_PITCH_MEDIAN };
    if (pitch - raw).abs() > 1e-3 {
        notes.push(format!("cameras: ON_BOARD_PITCH_ANGLE {raw:.2} is outside the stock cockpit range - {pitch:.2} (stock median)"));
    }
    if let Some(h) = ac1_head(car, f) {
        let d = [eye[0] - h[0], eye[1] - h[1], eye[2] - h[2]];
        let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        // twins: the eye is 0..13 cm above and 0..16 cm ahead of RIG_Head
        if dist > 0.3 {
            notes.push(format!("cameras: the eye is {dist:.2} m from the AC1 driver's head (RIG_Head ({:.2}, {:.2}, {:.2}))", h[0], h[1], h[2]));
        }
    }
    Cams { eye, pitch, notes }
}

/// The AC1 driver's RIG_Head joint (driver_base_pos.knh) in EVO space.
fn ac1_head(car: &Car, f: &Frame) -> Option<[f64; 3]> {
    let p = crate::ac1::car::find_ci(&car.dir, "driver_base_pos.knh")?;
    let k = crate::ac1::knh::parse(&std::fs::read(p).ok()?).ok()?;
    let mut w: std::collections::HashMap<String, frame::M4> = std::collections::HashMap::new();
    for (n, l, par) in k.flat() {
        let m = match par.as_ref().and_then(|p| w.get(p)) {
            Some(pw) => frame::mul(&l, pw),
            None => l,
        };
        if n.ends_with("RIG_Head") {
            return Some(f.evo([m[12], m[13], m[14]]));
        }
        w.insert(n, m);
    }
    None
}

// ------------------------------------------------------------ the shell --

/// LOD0 triangles of an encoded mesh (positions f5, indices f11).
pub fn mesh_tris(b: &[u8]) -> Vec<[[f32; 3]; 3]> {
    let Some(lod) = Msg::parse(b).ok().and_then(|m| m.subs(5).into_iter().next()) else { return vec![] };
    let pos = fx::stream(&lod, 5);
    let idx = lod.bytes(11).map(crate::io::pb::varints).unwrap_or_default();
    let p = |i: u64| -> Option<[f32; 3]> {
        let i = i as usize * 3;
        (i + 2 < pos.len()).then(|| [pos[i], pos[i + 1], pos[i + 2]])
    };
    idx.chunks_exact(3).filter_map(|t| Some([p(t[0])?, p(t[1])?, p(t[2])?])).collect()
}

/// How many of the six axis rays (+-X, +-Y, +-Z) from `p` hit the shell
/// within 3 m. Stock: every external camera (bonnet, bumper, chase) 0..2,
/// every cockpit / dash camera 2..6. A camera buried in the body hits 6.
pub fn enclosed(tris: &[[[f32; 3]; 3]], p: [f64; 3]) -> usize {
    const DIRS: [[f64; 3]; 6] = [[1., 0., 0.], [-1., 0., 0.], [0., 1., 0.], [0., -1., 0.], [0., 0., 1.], [0., 0., -1.]];
    let sub = |a: [f64; 3], b: [f64; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let cross = |a: [f64; 3], b: [f64; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let mut hit = [false; 6];
    for t in tris {
        let v = t.map(|q| q.map(|x| x as f64));
        let (e1, e2) = (sub(v[1], v[0]), sub(v[2], v[0]));
        let s = sub(p, v[0]);
        for (k, d) in DIRS.iter().enumerate() {
            if hit[k] {
                continue;
            }
            let h = cross(*d, e2);
            let a = dot(e1, h);
            if a.abs() < 1e-12 {
                continue;
            }
            let f = 1.0 / a;
            let u = f * dot(s, h);
            if !(0.0..=1.0).contains(&u) {
                continue;
            }
            let q = cross(s, e1);
            let w = f * dot(*d, q);
            if w < 0.0 || u + w > 1.0 {
                continue;
            }
            let dist = f * dot(e2, q);
            if dist > 1e-4 && dist < 3.0 {
                hit[k] = true;
            }
        }
        if hit.iter().all(|h| *h) {
            break;
        }
    }
    hit.iter().filter(|h| **h).count()
}

// ------------------------------------------------------------ the actor --

fn camera(tmpl: Option<&Msg>, name: &str, pos: [f64; 3], rot: [f32; 3], fov: Option<f32>, mode: Option<u64>) -> Msg {
    let mut c = tmpl.cloned().unwrap_or_default();
    c.set(1, Val::Msg(fx::transform([pos[0] as f32, pos[1] as f32, pos[2] as f32], rot)));
    let mut d = c.sub(2).unwrap_or_default();
    if !name.is_empty() {
        d.set_str(2, name);
    }
    let mut bp = d.sub(14).unwrap_or_default();
    if let Some(v) = fov {
        bp.set_f32(2, v);
    }
    if let Some(m) = mode {
        bp.set_varint(13, m);
    }
    d.set(14, Val::Msg(bp));
    c.set(2, Val::Msg(d));
    // field order as stock: transform, data
    c.fields.sort_by_key(|f| f.num);
    c
}

fn insert_all(car: &mut Msg, num: u32, items: Vec<Msg>) {
    let at = car.fields.iter().position(|f| f.num == num).unwrap_or_else(|| car.fields.iter().position(|f| f.num > num).unwrap_or(car.fields.len()));
    car.remove(num);
    let at = at.min(car.fields.len());
    for (k, m) in items.into_iter().enumerate() {
        car.fields.insert(at + k, Field { num, val: Val::Msg(m) });
    }
}

/// The drivable camera positions (EVO space) and pitches of a car.
pub struct Drivable {
    pub dash: [f64; 3],
    pub bonnet: ([f64; 3], f32),
    pub bumper: ([f64; 3], f32),
    pub near: [f64; 3],
    pub far: [f64; 3],
}

/// The highest body surface straight below (x, z): the max y of the
/// triangles whose XZ projection holds the point.
pub fn surface_y(tris: &[[[f32; 3]; 3]], x: f64, z: f64) -> Option<f64> {
    let mut best: Option<f64> = None;
    for t in tris {
        let (ax, az, bx, bz, cx, cz) = (t[0][0] as f64, t[0][2] as f64, t[1][0] as f64, t[1][2] as f64, t[2][0] as f64, t[2][2] as f64);
        if x < ax.min(bx).min(cx) || x > ax.max(bx).max(cx) || z < az.min(bz).min(cz) || z > az.max(bz).max(cz) {
            continue;
        }
        let d = (bz - cz) * (ax - cx) + (cx - bx) * (az - cz);
        if d.abs() < 1e-12 {
            continue;
        }
        let l0 = ((bz - cz) * (x - cx) + (cx - bx) * (z - cz)) / d;
        let l1 = ((cz - az) * (x - cx) + (ax - cx) * (z - cz)) / d;
        let l2 = 1.0 - l0 - l1;
        if l0 < 0.0 || l1 < 0.0 || l2 < 0.0 {
            continue;
        }
        let y = l0 * t[0][1] as f64 + l1 * t[1][1] as f64 + l2 * t[2][1] as f64;
        best = Some(best.map_or(y, |b: f64| b.max(y)));
    }
    best
}

/// The windscreen on the centreline: (top z, base z). Top = the highest
/// centreline point from 0.4 m behind to 0.6 m ahead of the eye (roof front
/// or screen header); base = the cowl, the first point ahead of the top
/// at least 10 cm lower where the profile stops falling (under 2 cm over
/// the next 10 cm). On the AC1 ks_mazda_miata: top 0.04, base 0.62 (stock
/// mx5_na: 0.06, 0.66).
pub fn windscreen(tris: &[[[f32; 3]; 3]], eye_z: f64, nose: f64) -> Option<(f64, f64)> {
    let zs: Vec<f64> = (0..).map(|i| eye_z - 0.4 + i as f64 * 0.025).take_while(|z| *z < nose - 0.2).collect();
    let ys: Vec<Option<f64>> = zs.iter().map(|z| surface_y(tris, 0.0, *z)).collect();
    let it = (0..zs.len()).filter(|i| zs[*i] <= eye_z + 0.6).filter_map(|i| ys[i].map(|y| (i, y))).max_by(|a, b| a.1.total_cmp(&b.1))?.0;
    let yt = ys[it]?;
    for i in it + 1..zs.len().saturating_sub(4) {
        if let (Some(y), Some(y4)) = (ys[i], ys[i + 4]) {
            if y4 - y > -0.02 && yt - y > 0.1 {
                return Some((zs[it], zs[i]));
            }
        }
    }
    None
}

/// OURS: the bonnet camera this far behind the cowl. Stock (68 cars with a
/// windscreen found): median -0.15, p10..p90 -0.44..+0.36, in two groups,
/// the modern cars near -0.15 and mx5_na / ae86 / supra / 964 / 718 RS near
/// -0.42; -0.30 lands within 5 cm of mx5_na on the AC1 ks_mazda_miata.
pub const BONNET_BEHIND_COWL: f64 = 0.30;
/// Stock bonnet camera height over the body straight below: median 0.14,
/// p10..p90 0.07..0.21 (mx5_na 0.152).
pub const BONNET_CLEARANCE: f64 = 0.15;
/// Stock bonnet pitch: median -5.7, p10..p90 -10..-2.4.
pub const BONNET_PITCH: f32 = -5.7;

fn stock_bonnet_fallback(eye: [f64; 3], hi: [f64; 3]) -> [f64; 3] {
    [0.0, eye[1] + 0.14, (eye[2] + 0.94).min(hi[2] - 0.6)]
}

pub fn drivable(car: &Car, f: &Frame, c: &Cams, body: ([f32; 3], [f32; 3]), tris: &[[[f32; 3]; 3]], notes: &mut Vec<String>) -> Drivable {
    let ci = car.ini("car.ini");
    let (lo, hi) = (body.0.map(|x| x as f64), body.1.map(|x| x as f64));
    let cog = |p: [f64; 3]| [p[0] + f.cog[0], p[1] + f.cog[1], p[2] + f.cog[2]];
    // dash ("no wheel" view): AC1 dash_cam.ini, kn5 frame. ks_mazda_miata
    // (0.348, 0.988, -0.177) vs mx5_na (0.35, 0.964, -0.13); without the file
    // the stock offset from the eye (median (0, +0.007, +0.196))
    let dash = car.ini("dash_cam.ini");
    let dash_pos = v3(&dash.nums("DASH_CAM", "POS")).map(|p| f.evo(p)).unwrap_or([c.eye[0], c.eye[1] + 0.007, c.eye[2] + 0.196]);
    // bonnet: EVO puts it on the centreline over the lower windscreen,
    // BONNET_CLEARANCE over the glass; AC1's BONNET_CAMERA_POS (CoG frame)
    // sits lower and further forward, on the bonnet itself (ks_mazda_miata
    // (0, 0.92, 0.83), 0.08 m over the panel), so it is only the fallback
    let ac1_pitch = ci.num("GRAPHICS", "BONNET_CAMERA_PITCH").map(|p| p as f32);
    let bon_pitch = ac1_pitch.filter(|p| *p < -2.0).unwrap_or(BONNET_PITCH);
    let ws = if tris.is_empty() { None } else { windscreen(tris, c.eye[2], hi[2]) };
    let bonnet = match ws.filter(|w| w.1 > c.eye[2] + 0.2) {
        Some((_, base)) => {
            let z = (base - BONNET_BEHIND_COWL).max(c.eye[2] + 0.3);
            let y = surface_y(tris, 0.0, z).map(|s| s + BONNET_CLEARANCE).unwrap_or(c.eye[1] + 0.14);
            [0.0, y, z]
        }
        None => {
            let mut b = v3(&ci.nums("GRAPHICS", "BONNET_CAMERA_POS")).map(cog).unwrap_or_else(|| stock_bonnet_fallback(c.eye, hi));
            b[0] = 0.0;
            // a mod with bonnet and bumper swapped (honda_s2000_a_spec), or one
            // buried in the body
            if b[2] > hi[2] - 0.3 || (!tris.is_empty() && enclosed(tris, b) >= 4) {
                let s = stock_bonnet_fallback(c.eye, hi);
                notes.push(format!("cameras: BONNET_CAMERA_POS ({:.2}, {:.2}, {:.2}) is in or ahead of the body - stock rule", b[0], b[1], b[2]));
                b = s;
            }
            notes.push("cameras: no windscreen found on the centreline - bonnet camera from BONNET_CAMERA_POS".into());
            b
        }
    };
    // bumper: stock puts it at the cockpit height (64 of 71 cars exactly)
    // and pitch, 0.06 m behind the nose; AC1's BUMPER_CAMERA_POS (CoG frame)
    // agrees in z (ks_mazda_miata nose - 0.064) and sits 8 cm lower than mx5_na
    let bumper = [0.0, c.eye[1], hi[2] - 0.06];
    let bum_pitch = c.pitch;
    // chase: stock heights 1.80 / 2.50 (1.96 / 2.67 on the tallest cars:
    // OURS eye + 0.72, clamped), distances from the tail
    let ny = (c.eye[1] + 0.72).clamp(1.8, 2.0);
    let near = [0.0, ny, lo[2] - 2.78];
    let far = [0.0, ny + 0.7, lo[2] - 4.22];
    Drivable { dash: dash_pos, bonnet: (bonnet, bon_pitch), bumper: (bumper, bum_pitch), near, far }
}

/// Rewrite every camera of the actor for this car.
pub fn write(files: &mut Files, id: &str, car: &Car, f: &Frame, c: &Cams, body: ([f32; 3], [f32; 3])) -> Result<Vec<String>> {
    let base = format!("content\\cars\\{id}");
    let ap = format!("{base}\\{id}.actor");
    let Some(mut actor) = files.get(&ap).and_then(|b| Msg::parse(b).ok()) else { return Ok(vec![]) };
    let mut notes = c.notes.clone();
    let (lo, hi) = (body.0.map(|x| x as f64), body.1.map(|x| x as f64));
    let ext = files.under(&format!("{base}\\meshes")).into_iter().find(|k| k.to_lowercase().ends_with("_exterior.mesh"));
    let tris = ext.and_then(|p| files.get(&p).map(|b| mesh_tris(b))).unwrap_or_default();
    let dv = drivable(car, f, c, body, &tris, &mut notes);
    let a = actor.msg_mut(103)?;
    let old: Vec<Msg> = a.subs(10);
    let by = |n: &str| old.iter().find(|m| m.sub(2).and_then(|d| d.str(2)).map(|s| s.eq_ignore_ascii_case(n)).unwrap_or(false));
    let fwd = |p: f32| [p, 180.0, 0.0];
    let drivable = vec![
        camera(by("Cockpit"), "Cockpit", c.eye, fwd(c.pitch), Some(55.0), Some(1)),
        camera(by("Dash"), "Dash", dv.dash, fwd(c.pitch), Some(55.0), Some(4)),
        camera(by("Bonnet"), "Bonnet", dv.bonnet.0, fwd(dv.bonnet.1), Some(55.0), Some(5)),
        camera(by("Bumper"), "Bumper", dv.bumper.0, fwd(dv.bumper.1), Some(55.0), Some(3)),
        camera(by("NearChase"), "NearChase", dv.near, [-5.0, 180.0, 0.0], Some(55.0), Some(2)),
        camera(by("FarChase"), "FarChase", dv.far, [-5.0, 180.0, 0.0], Some(55.0), Some(2)),
    ];
    insert_all(a, 10, drivable);
    // onboard cameras from cameras.ini (kn5 frame, + datum)
    let cams_ini = car.ini("cameras.ini");
    let tmpl_on = a.subs(1).into_iter().next();
    let mut onboard = Vec::new();
    for (sec, kv) in cams_ini.sections.iter() {
        if !sec.starts_with("CAMERA_") {
            continue;
        }
        let p = kv.get("POSITION").map(|s| crate::ac1::ini::nums_of(s)).and_then(|v| v3(&v));
        let d = kv.get("FORWARD").map(|s| crate::ac1::ini::nums_of(s)).and_then(|v| v3(&v));
        let (Some(p), Some(d)) = (p, d) else { continue };
        let fov = kv.get("FOV").and_then(|s| crate::ac1::ini::num_of(s)).filter(|v| *v > 10.0 && *v < 120.0).unwrap_or(60.0) as f32;
        let mut cm = camera(tmpl_on.as_ref(), "", f.evo(p), fx::euler_minus_z(d), Some(fov), None);
        // the onboard set carries no name and no mode on stock
        if let Some(mut dd) = cm.sub(2) {
            dd.remove(2);
            cm.set(2, Val::Msg(dd));
        }
        onboard.push(cm);
    }
    let old_eye = old.first().and_then(|m| m.sub(1)).map(|t| fx::transform_of(&t).0).unwrap_or([0.0; 3]);
    if !onboard.is_empty() {
        notes.push(format!("cameras: {} onboard camera(s) from cameras.ini", onboard.len()));
        insert_all(a, 1, onboard);
    } else {
        // keep the donor's set, moved by the cockpit offset
        let d = [c.eye[0] as f32 - old_eye[0], c.eye[1] as f32 - old_eye[1], c.eye[2] as f32 - old_eye[2]];
        let moved: Vec<Msg> = a.subs(1).into_iter().map(|mut m| {
            if let Some(t) = m.sub(1) {
                let (p, r) = fx::transform_of(&t);
                m.set(1, Val::Msg(fx::transform([p[0] + d[0], p[1] + d[1], p[2] + d[2]], r)));
            }
            m
        }).collect();
        insert_all(a, 1, moved);
    }
    // cockpit probe: centred, stock offset from the eye, the donor's box scale
    let probe = [0.0f32, (c.eye[1] + 0.02) as f32, (c.eye[2] + 0.62).min(hi[2] - 0.8) as f32];
    let mut pt = fx::transform(probe, [0.0; 3]);
    if let Some(s) = a.sub(17).and_then(|t| t.sub(3)) {
        pt.set(3, Val::Msg(s));
    }
    a.set(17, Val::Msg(pt));
    // showroom: the interior view at the eye, limits around it
    let mut sr = a.sub(32).unwrap_or_default();
    let mut inner = sr.sub(2).unwrap_or_default();
    let (ex, ey, ez) = (c.eye[0] as f32, c.eye[1] as f32, c.eye[2] as f32);
    inner.set(7, Val::Msg(fx::transform([ex, ey, ez], [-5.0, 180.0, 0.0])));
    // stock: symmetric x limits 0.42..0.50 (eye x + 0.07..0.13)
    let xl = (ex.abs() + 0.08).clamp(0.42, 0.5);
    inner.set_f32(1, -xl);
    inner.set_f32(2, xl);
    inner.set_f32(3, ey - 0.25);
    inner.set_f32(4, ey + 0.1);
    inner.set_f32(5, ez);
    inner.set_f32(6, ez + 0.3);
    inner.fields.sort_by_key(|f| if f.num == 7 { 0 } else { f.num });
    sr.set(2, Val::Msg(inner));
    let len = (hi[2] - lo[2]).max(2.5) as f32;
    // stock: 0.61 x length (0.56..0.79), 2.0..2.8 m
    sr.set_f32(3, (0.6 * len).clamp(2.0, 2.8));
    a.set(32, Val::Msg(sr));
    // mirrors render from their bones (MIRROR_0 / 1 / 2, on the AC1 mirror
    // meshes): offsets zero, as on 198 of the 202 stock mirrors (the rest
    // 1 cm); AC1's MIRROR_POSITION (20..50 cm off the glass) is not used.
    // The VR look angles (f10 vr_yaw) follow the driver's side: stock LHD
    // centre -12..-18.5, left 15..21, right -27..-32; RHD (ae86, rx7_fd)
    // centre +20..22, left 27..32, right -17..-19 = the LHD set mirrored.
    let flip = c.eye[0].abs() > 0.1 && old_eye[0].abs() > 0.1 && (c.eye[0] > 0.0) != (old_eye[0] > 0.0);
    let olds = a.subs(23);
    let yaw_of = |t: u64| olds.iter().find(|m| m.varint(1).unwrap_or(0) == t).and_then(|m| m.f32(10));
    let mirrors: Vec<Msg> = olds.iter().cloned().map(|mut m| {
        m.set(5, Val::Msg(Msg::default()));
        if flip {
            let t = m.varint(1).unwrap_or(0);
            let src = match t { 1 => 2, 2 => 1, x => x };
            if let Some(y) = yaw_of(src) {
                m.set_f32(10, -y);
            }
        }
        m
    }).collect();
    if flip {
        notes.push("cameras: mirror VR yaw mirrored for the other driving side".into());
    }
    insert_all(a, 23, mirrors);
    // rear-view camera (the mirror render on cars without mirror meshes):
    // stock y 0.8, z tail - 0.1
    let mut rc = a.sub(24).unwrap_or_default();
    rc.set(1, Val::Msg(fx::vec_msg(&[0.0, 0.8, (lo[2] - 0.1).clamp(-2.6, -1.1) as f32])));
    a.set(24, Val::Msg(rc));
    files.put(&ap, actor.encode());
    let p3 = |p: [f64; 3]| format!("({:.3}, {:.3}, {:.3})", p[0], p[1], p[2]);
    notes.push(format!("cameras: cockpit {} pitch {:.2}, dash {}, bonnet {} pitch {:.1}, bumper {} pitch {:.1}, chase {} / {}; cockpit probe ({:.2}, {:.2}, {:.2})",
        p3(c.eye), c.pitch, p3(dv.dash), p3(dv.bonnet.0), dv.bonnet.1, p3(dv.bumper.0), dv.bumper.1, p3(dv.near), p3(dv.far), probe[0], probe[1], probe[2]));
    Ok(notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enclosure() {
        // a unit cube around the origin: inside = 6, outside = 0..1
        let v = |x: f32, y: f32, z: f32| [x, y, z];
        let q = |a, b, c, d| [[a, b, c], [a, c, d]];
        let mut t: Vec<[[f32; 3]; 3]> = Vec::new();
        let (n, p) = (-1.0, 1.0);
        for f in [
            q(v(n, n, n), v(p, n, n), v(p, p, n), v(n, p, n)),
            q(v(n, n, p), v(p, n, p), v(p, p, p), v(n, p, p)),
            q(v(n, n, n), v(n, p, n), v(n, p, p), v(n, n, p)),
            q(v(p, n, n), v(p, p, n), v(p, p, p), v(p, n, p)),
            q(v(n, n, n), v(p, n, n), v(p, n, p), v(n, n, p)),
            q(v(n, p, n), v(p, p, n), v(p, p, p), v(n, p, p)),
        ] {
            t.extend(f);
        }
        assert_eq!(enclosed(&t, [0.1, 0.2, -0.3]), 6);
        assert!(enclosed(&t, [0.0, 2.0, 0.0]) <= 1);
        assert_eq!(enclosed(&t, [0.0, 0.0, -5.0]), 0);
    }
}
