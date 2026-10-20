//! The driver (requirements car-driver.md).
//!
//! # Mode (ours): an EVO stock driver posed to the AC1 seat
//!
//! The game dresses its own driver (head, suit, gloves, shoes, helmet from
//! `content\characters`, the player's choice) and poses it with the car's
//! driver animation layers; each layer's tracks are bone LOCALS of the
//! shared rig (COG > A_Hips > ... > A_Head; the parent table below is read
//! off the stock character meshes) and are already in car space - A_Hips
//! carries the seat. So the driver is placed by rewriting the layers: the
//! donor's (Kunos SDK sample) seated pose is retargeted onto the AC1 driver's
//! joints, taken from AC1's `driver_base_pos.knh` (the seated pose in the
//! kn5 frame) and `animations\steer.ksanim`, plus the body datum.
//!
//! # Retarget (ours)
//!
//! Joint pairs: RIG_Hips / A_Hips, RIG_Cest / A_CEST, RIG_Nek / A_Neck,
//! RIG_Head / A_Head, RIG_Clave / *_Shoulder, RIG_Arm / *Arm, RIG_ForeArm /
//! *ForeArm (elbow), RIG_HAND / *Hand (wrist), RIG_Leg / *UpLeg (hip),
//! RIG_Shin / *Leg (knee), RIG_Hill / *Foot (ankle), RIG_Foot / *ToeBase.
//! 1. the pelvis turned and moved so the hip sockets and the chest line up;
//! 2. the torso and the neck aimed at the AC1 chest / neck / head;
//! 3. the clavicles aimed at the AC1 shoulders;
//! 4. arms and legs by two-bone IK: wrists and ankles onto the AC1 ones,
//!    elbows and knees bent towards the AC1 elbows and knees; the bone
//!    lengths stay the EVO driver's (an unreachable target is approached
//!    in a straight line);
//! 5. hands and feet aimed along the AC1 hand / foot, the palm turned by the
//!    index-to-pinkie line; the fingers keep the donor's grip.
//!
//! # Layers
//!
//! - position (109): the retargeted seat, 201 frames as stock.
//! - steer (101): 201 frames from -360 to +360 degrees of wheel, like every
//!   stock clip (frame = middle + middle x angle / 360). The AC1 clip spans
//!   -LOCK..+LOCK over its frames (orbit direction matched to the donor's);
//!   past 180 degrees each frame blends towards the pose one turn back
//!   (smoothstep), so the clip ends on its centre pose: a continuous full
//!   turn whatever LOCK is, and a periodic AC1 clip is left unchanged.
//! - accelerator / brake (102, 103): the donor's pedal motion (per bone,
//!   relative to its seat: the ankle and knee press) on our legs, so the
//!   feet visibly press. The clutch layer stays empty as on stock (the
//!   brake layer drives the same leg).
//! - H-shifter (105..107): where AC1 has `shift.ksanim`, the approach is the
//!   AC1 hand path to the lever (resampled to the donor's 32 frames), the
//!   shift itself the hand held at the lever; else those slots are emptied.
//! - idle, handbrake and the paddle layers are emptied (their arm motion is
//!   the donor's car).
//!
//! A failed stage leaves the donor's driver (the caller rolls the journal
//! back).

use std::collections::HashMap;

use anyhow::{anyhow, bail, Result};

use crate::ac1::car::{find_ci, Car};
use crate::car::anim::encode_clip;
use crate::car::frame::{self, Frame, M4};
use crate::files::Files;
use crate::io::pb::{Msg, Val};

/// The stock driver rig: (bone, parent), from the stock character meshes.
const RIG: &[(&str, &str)] = &[
    ("COG", ""), ("A_Hips", "COG"), ("A_Abdom_LW", "A_Hips"), ("A_Abdom_UP", "A_Abdom_LW"), ("A_CEST", "A_Abdom_UP"),
    ("RIGHT_Shoulder", "A_CEST"), ("RightArm", "RIGHT_Shoulder"), ("RighttForeArm", "RightArm"), ("RightHand", "RighttForeArm"),
    ("RightHandThumb1", "RightHand"), ("RightHandThumb2", "RightHandThumb1"), ("RightHandThumb3", "RightHandThumb2"),
    ("RightHandIndex1", "RightHand"), ("RightHandIndex2", "RightHandIndex1"), ("RightHandIndex3", "RightHandIndex2"),
    ("RightHandMiddle1", "RightHand"), ("RightHandMiddle2", "RightHandMiddle1"), ("RightHandMiddle3", "RightHandMiddle2"),
    ("RightHandRing1", "RightHand"), ("RightHandRing2", "RightHandRing1"), ("RightHandRing3", "RightHandRing2"),
    ("RightHandPinky1", "RightHand"), ("RightHandPinky2", "RightHandPinky1"), ("RightHandPinky3", "RightHandPinky2"),
    ("RightForeArmMiddle", "RighttForeArm"), ("RightForeArmScale", "RighttForeArm"), ("RightArm_joint", "RIGHT_Shoulder"),
    ("LEFT_Shoulder", "A_CEST"), ("LeftArm", "LEFT_Shoulder"), ("LeftForeArm", "LeftArm"), ("LeftHand", "LeftForeArm"),
    ("LeftHandPinky1", "LeftHand"), ("LeftHandPinky2", "LeftHandPinky1"), ("LeftHandPinky3", "LeftHandPinky2"),
    ("LeftHandThumb1", "LeftHand"), ("LeftHandThumb2", "LeftHandThumb1"), ("LeftHandThumb3", "LeftHandThumb2"),
    ("LeftHandMiddle1", "LeftHand"), ("LeftHandMiddle2", "LeftHandMiddle1"), ("LeftHandMiddle3", "LeftHandMiddle2"),
    ("LeftHandIndex1", "LeftHand"), ("LeftHandIndex2", "LeftHandIndex1"), ("LeftHandIndex3", "LeftHandIndex2"),
    ("LeftHandRing1", "LeftHand"), ("LeftHandRing2", "LeftHandRing1"), ("LeftHandRing3", "LeftHandRing2"),
    ("LeftForeArmMiddle", "LeftForeArm"), ("LeftForeArmScale", "LeftForeArm"), ("LeftArm_joint", "LEFT_Shoulder"),
    ("A_Neck", "A_CEST"), ("A_Head", "A_Neck"),
    ("LeftUpLeg", "A_Hips"), ("LeftLeg", "LeftUpLeg"), ("LeftFoot", "LeftLeg"), ("LeftToeBase", "LeftFoot"),
    ("RightUpLeg", "A_Hips"), ("RightLeg", "RightUpLeg"), ("RightFoot", "RightLeg"), ("RightToeBase", "RightFoot"),
];

// ------------------------------------------------------------------ maths --

type V3 = [f64; 3];

fn sub(a: V3, b: V3) -> V3 { [a[0] - b[0], a[1] - b[1], a[2] - b[2]] }
fn add(a: V3, b: V3) -> V3 { [a[0] + b[0], a[1] + b[1], a[2] + b[2]] }
fn scale(a: V3, k: f64) -> V3 { [a[0] * k, a[1] * k, a[2] * k] }
fn dot(a: V3, b: V3) -> f64 { a[0] * b[0] + a[1] * b[1] + a[2] * b[2] }
fn cross(a: V3, b: V3) -> V3 { [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]] }
fn len(a: V3) -> f64 { dot(a, a).sqrt() }
fn norm(a: V3) -> V3 { let l = len(a); if l > 1e-12 { scale(a, 1.0 / l) } else { [0.0, 0.0, 0.0] } }
fn lerp(a: V3, b: V3, t: f64) -> V3 { add(a, scale(sub(b, a), t)) }
fn pos(m: &M4) -> V3 { [m[12], m[13], m[14]] }

/// Row-vector rotation (v' = v R) about unit `axis` by `ang` radians.
fn axis_angle(axis: V3, ang: f64) -> [[f64; 3]; 3] {
    let (s, c) = ang.sin_cos();
    let [x, y, z] = axis;
    let t = 1.0 - c;
    // column form, then transposed
    let col = [
        [t * x * x + c, t * x * y - s * z, t * x * z + s * y],
        [t * x * y + s * z, t * y * y + c, t * y * z - s * x],
        [t * x * z - s * y, t * y * z + s * x, t * z * z + c],
    ];
    [[col[0][0], col[1][0], col[2][0]], [col[0][1], col[1][1], col[2][1]], [col[0][2], col[1][2], col[2][2]]]
}

fn rmul(v: V3, r: &[[f64; 3]; 3]) -> V3 {
    [v[0] * r[0][0] + v[1] * r[1][0] + v[2] * r[2][0], v[0] * r[0][1] + v[1] * r[1][1] + v[2] * r[2][1], v[0] * r[0][2] + v[1] * r[1][2] + v[2] * r[2][2]]
}

fn rcompose(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut o = [[0.0; 3]; 3];
    for i in 0..3 {
        o[i] = rmul(a[i], b);
    }
    o
}

/// The smallest rotation taking direction `a` onto `b`.
fn from_to(a: V3, b: V3) -> [[f64; 3]; 3] {
    let (a, b) = (norm(a), norm(b));
    let c = dot(a, b).clamp(-1.0, 1.0);
    let ax = cross(a, b);
    if len(ax) < 1e-9 {
        if c > 0.0 {
            return [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        }
        let perp = if a[0].abs() < 0.9 { norm(cross(a, [1.0, 0.0, 0.0])) } else { norm(cross(a, [0.0, 1.0, 0.0])) };
        return axis_angle(perp, std::f64::consts::PI);
    }
    axis_angle(norm(ax), c.acos())
}

/// Signed angle about `axis` from `a` to `b` (both projected on its plane).
fn twist_angle(axis: V3, a: V3, b: V3) -> f64 {
    let n = norm(axis);
    let pa = norm(sub(a, scale(n, dot(a, n))));
    let pb = norm(sub(b, scale(n, dot(b, n))));
    if len(pa) < 1e-9 || len(pb) < 1e-9 {
        return 0.0;
    }
    dot(cross(pa, pb), n).atan2(dot(pa, pb))
}

/// `w` rotated by `r` about the world point `c`.
fn rotate_about(w: &M4, c: V3, r: &[[f64; 3]; 3]) -> M4 {
    let mut o = *w;
    for i in 0..3 {
        let row = rmul([w[i * 4], w[i * 4 + 1], w[i * 4 + 2]], r);
        o[i * 4] = row[0];
        o[i * 4 + 1] = row[1];
        o[i * 4 + 2] = row[2];
    }
    let t = add(rmul(sub(pos(w), c), r), c);
    o[12] = t[0];
    o[13] = t[1];
    o[14] = t[2];
    o
}

fn slerp_m(a: &M4, b: &M4, t: f64) -> M4 {
    // locals of the same bone: blend rows and re-orthonormalise (small angles)
    let mut o = [0f64; 16];
    for i in 0..16 {
        o[i] = a[i] + (b[i] - a[i]) * t;
    }
    let x = norm([o[0], o[1], o[2]]);
    let y0 = [o[4], o[5], o[6]];
    let z = norm(cross(x, y0));
    let y = cross(z, x);
    o[0..3].copy_from_slice(&x);
    o[4..7].copy_from_slice(&y);
    o[8..11].copy_from_slice(&z);
    o
}

// ------------------------------------------------------------------- pose --

#[derive(Clone)]
struct Pose {
    local: HashMap<String, M4>,
    parent: HashMap<String, String>,
}

impl Pose {
    fn world(&self, n: &str) -> M4 {
        let mut m = self.local.get(n).copied().unwrap_or(frame::I4);
        let mut p = self.parent.get(n).cloned().unwrap_or_default();
        let mut guard = 0;
        while !p.is_empty() && guard < 64 {
            if let Some(l) = self.local.get(&p) {
                m = frame::mul(&m, l);
            }
            p = self.parent.get(&p).cloned().unwrap_or_default();
            guard += 1;
        }
        m
    }
    fn p(&self, n: &str) -> V3 {
        pos(&self.world(n))
    }
    fn set_world(&mut self, n: &str, w: &M4) {
        let pw = match self.parent.get(n).filter(|p| !p.is_empty()) {
            Some(p) => self.world(p),
            None => frame::I4,
        };
        let inv = frame::inv(&pw).unwrap_or(frame::I4);
        self.local.insert(n.to_string(), frame::mul(w, &inv));
    }
    /// Turn `bone` about its own origin so its `child` lies along `dir`;
    /// then about `dir` so the vector `from` (world, after the first turn)
    /// lines up with `to`.
    fn aim(&mut self, bone: &str, child: &str, dir: V3, twist: Option<(V3, V3)>) {
        let w = self.world(bone);
        let c = pos(&w);
        let cur = sub(self.p(child), c);
        if len(cur) < 1e-6 || len(dir) < 1e-6 {
            return;
        }
        let mut r = from_to(cur, dir);
        if let Some((from, to)) = twist {
            let f = rmul(from, &r);
            let a = twist_angle(dir, f, to);
            r = rcompose(&r, &axis_angle(norm(dir), a));
        }
        let nw = rotate_about(&w, c, &r);
        self.set_world(bone, &nw);
    }
    /// Two-bone IK: `upper` > `lower` > `end`; the end onto `target`, the
    /// middle joint bent towards `pole`.
    fn ik(&mut self, upper: &str, lower: &str, end: &str, target: V3, pole: V3) {
        let s = self.p(upper);
        let l1 = len(sub(self.p(lower), s));
        let l2 = len(sub(self.p(end), self.p(lower)));
        if l1 < 1e-4 || l2 < 1e-4 {
            return;
        }
        let to = sub(target, s);
        let d = len(to).clamp((l1 - l2).abs() + 1e-3, (l1 + l2) * 0.999);
        let dir = norm(to);
        let a = (l1 * l1 - l2 * l2 + d * d) / (2.0 * d);
        let h = (l1 * l1 - a * a).max(0.0).sqrt();
        let pv = sub(pole, s);
        let mut pd = sub(pv, scale(dir, dot(pv, dir)));
        if len(pd) < 1e-5 {
            let cur = sub(self.p(lower), s);
            pd = sub(cur, scale(dir, dot(cur, dir)));
        }
        let e = add(add(s, scale(dir, a)), scale(norm(pd), h));
        self.aim(upper, lower, sub(e, s), None);
        let t = add(s, scale(dir, d));
        self.aim(lower, end, sub(t, self.p(lower)), None);
    }
}

// ---------------------------------------------------------------- AC1 rig --

/// AC1 driver joints in EVO space (knh pose with some locals replaced).
struct Ac1 {
    nodes: Vec<(String, M4, Option<String>)>,
    datum: [f64; 3],
}

impl Ac1 {
    fn joints(&self, over: &HashMap<String, M4>) -> HashMap<String, V3> {
        let mut w: HashMap<String, M4> = HashMap::new();
        let mut out = HashMap::new();
        for (n, l, p) in &self.nodes {
            let l = over.get(n).copied().unwrap_or(*l);
            let m = match p.as_ref().and_then(|p| w.get(p)) {
                Some(pw) => frame::mul(&l, pw),
                None => l,
            };
            w.insert(n.clone(), m);
            out.insert(n.clone(), add(pos(&m), self.datum));
        }
        out
    }
}

fn j(jt: &HashMap<String, V3>, n: &str) -> Result<V3> {
    jt.get(&format!("DRIVER:{n}")).copied().ok_or_else(|| anyhow!("the AC1 driver has no {n}"))
}

struct Side {
    clave: &'static str,
    arm: &'static str,
    fore: &'static str,
    hand: &'static str,
    index: &'static str,
    middle: &'static str,
    pinky: &'static str,
    a_clave: &'static str,
    a_arm: &'static str,
    a_fore: &'static str,
    a_hand: &'static str,
    a_index: &'static str,
    a_middle: &'static str,
    a_pinky: &'static str,
}

const LEFT: Side = Side {
    clave: "LEFT_Shoulder", arm: "LeftArm", fore: "LeftForeArm", hand: "LeftHand", index: "LeftHandIndex1", middle: "LeftHandMiddle1", pinky: "LeftHandPinky1",
    a_clave: "RIG_Clave_L", a_arm: "RIG_Arm_L", a_fore: "RIG_ForeArm_L", a_hand: "RIG_HAND_L", a_index: "HAND_Index1", a_middle: "HAND_Middle1", a_pinky: "HAND_Pinkie1",
};
const RIGHT: Side = Side {
    clave: "RIGHT_Shoulder", arm: "RightArm", fore: "RighttForeArm", hand: "RightHand", index: "RightHandIndex1", middle: "RightHandMiddle1", pinky: "RightHandPinky1",
    a_clave: "RIG_Clave_R", a_arm: "RIG_Arm_R", a_fore: "RIG_ForeArm_R", a_hand: "RIG_HAND_R", a_index: "HAND_Index4", a_middle: "HAND_Middle4", a_pinky: "HAND_Pinkie4",
};

fn solve_arm(p: &mut Pose, jt: &HashMap<String, V3>, s: &Side) -> Result<()> {
    let (cl, ar, fo, ha) = (j(jt, s.a_clave)?, j(jt, s.a_arm)?, j(jt, s.a_fore)?, j(jt, s.a_hand)?);
    let _ = cl;
    let c = p.p(s.clave);
    p.aim(s.clave, s.arm, sub(ar, c), None);
    p.ik(s.arm, s.fore, s.hand, ha, fo);
    let (mi, ix, pk) = (j(jt, s.a_middle)?, j(jt, s.a_index)?, j(jt, s.a_pinky)?);
    let from = sub(p.p(s.index), p.p(s.pinky));
    p.aim(s.hand, s.middle, sub(mi, ha), Some((from, sub(ix, pk))));
    Ok(())
}

fn solve_leg(p: &mut Pose, jt: &HashMap<String, V3>, left: bool) -> Result<()> {
    let (up, lo, ft, toe) = if left { ("LeftUpLeg", "LeftLeg", "LeftFoot", "LeftToeBase") } else { ("RightUpLeg", "RightLeg", "RightFoot", "RightToeBase") };
    let sfx = if left { "L" } else { "R" };
    let (knee, ankle, foot) = (j(jt, &format!("RIG_Shin_{sfx}"))?, j(jt, &format!("RIG_Hill_{sfx}"))?, j(jt, &format!("RIG_Foot_{sfx}"))?);
    p.ik(up, lo, ft, ankle, knee);
    // the foot along the AC1 foot, its sole kept facing down
    let a = p.p(ft);
    let side = sub(p.p("LeftUpLeg"), p.p("RightUpLeg"));
    let cur = cross(sub(p.p(toe), a), side);
    let want = cross(sub(foot, ankle), side);
    p.aim(ft, toe, sub(foot, ankle), Some((cur, want)));
    Ok(())
}

/// The whole body onto the AC1 joints.
fn solve_body(p: &mut Pose, jt: &HashMap<String, V3>) -> Result<()> {
    // 1. pelvis: hip sockets and chest
    let (hl, hr, cest) = (j(jt, "RIG_Leg_L")?, j(jt, "RIG_Leg_R")?, j(jt, "RIG_Cest")?);
    // some AC1 driver rigs (ks_toyota_supra_mkiv) have no neck / head joints
    let (nek, head) = (j(jt, "RIG_Nek").ok(), j(jt, "RIG_Head").ok());
    let ht = lerp(hl, hr, 0.5);
    let hc = lerp(p.p("LeftUpLeg"), p.p("RightUpLeg"), 0.5);
    let r1 = from_to(sub(p.p("LeftUpLeg"), p.p("RightUpLeg")), sub(hl, hr));
    let up_cur = rmul(sub(p.p("A_CEST"), hc), &r1);
    let side_t = sub(hl, hr);
    let a = twist_angle(side_t, up_cur, sub(cest, ht));
    let r = rcompose(&r1, &axis_angle(norm(side_t), a));
    let w = p.world("A_Hips");
    let mut nw = rotate_about(&w, hc, &r);
    let d = sub(ht, hc);
    nw[12] += d[0];
    nw[13] += d[1];
    nw[14] += d[2];
    p.set_world("A_Hips", &nw);
    // 2. torso and neck
    let shoulders_cur = sub(p.p("LeftArm"), p.p("RightArm"));
    let shoulders_t = sub(j(jt, "RIG_Arm_L")?, j(jt, "RIG_Arm_R")?);
    // the lower spine leans the whole torso: hips-to-neck along AC1's
    match nek {
        Some(n) => p.aim("A_Abdom_LW", "A_Neck", sub(n, ht), Some((shoulders_cur, shoulders_t))),
        None => p.aim("A_Abdom_LW", "A_CEST", sub(cest, ht), Some((shoulders_cur, shoulders_t))),
    }
    if let (Some(n), Some(h)) = (nek, head) {
        p.aim("A_Neck", "A_Head", sub(h, n), None);
    }
    // 3..5. arms and legs
    solve_arm(p, jt, &LEFT)?;
    solve_arm(p, jt, &RIGHT)?;
    solve_leg(p, jt, true)?;
    solve_leg(p, jt, false)?;
    Ok(())
}

// ----------------------------------------------------------------- driver --

fn read_anim(files: &Files, path: &str) -> Option<Vec<(String, Vec<M4>)>> {
    let m = Msg::parse(files.get(path)?).ok()?;
    let mut out = Vec::new();
    for take in m.subs(2) {
        for t in take.subs(3) {
            let frames: Vec<M4> = t.subs(2).iter().map(|f| {
                let mut o = [0f64; 16];
                for (i, v) in o.iter_mut().enumerate() {
                    *v = f.f32(i as u32 + 1).unwrap_or(0.0) as f64;
                }
                o
            }).collect();
            out.push((t.str(1).unwrap_or_default(), frames));
        }
    }
    (!out.is_empty()).then_some(out)
}

/// Driver slot -> its first animation path (actor f103.31).
fn slot_paths(actor: &Msg) -> HashMap<u32, String> {
    let mut out = HashMap::new();
    let Some(car) = actor.sub(103) else { return out };
    let Some(da) = car.sub(31) else { return out };
    for f in &da.fields {
        if let Some(s) = match &f.val { Val::Bytes(b) => Msg::parse(b).ok(), Val::Msg(m) => Some(m.clone()), _ => None } {
            if let Some(p) = s.subs(1).first().and_then(|a| a.str(1)) {
                if p.to_lowercase().ends_with(".animation") {
                    out.insert(f.num, p);
                }
            }
        }
    }
    out
}

fn ksanim(car: &Car, name: &str) -> Option<Vec<crate::ac1::ksanim::Track>> {
    let p = find_ci(&car.dir.join("animations"), name)?;
    crate::ac1::ksanim::read(&std::fs::read(p).ok()?).ok()
}

/// Frame `f` (fractional) of an AC1 clip as locals by node name.
fn ac1_frame(tr: &[crate::ac1::ksanim::Track], f: f64) -> HashMap<String, M4> {
    let mut out = HashMap::new();
    for t in tr {
        if t.frames.is_empty() {
            continue;
        }
        let n = t.frames.len();
        let x = f.clamp(0.0, (n - 1) as f64);
        let (a, b) = (x.floor() as usize, (x.ceil() as usize).min(n - 1));
        let m = if a == b { t.frames[a] } else { slerp_m(&t.frames[a], &t.frames[b], x - a as f64) };
        out.insert(t.node.clone(), m);
    }
    out
}

/// Orbit direction of a hand path about the car's forward axis.
fn orbit(a: V3, b: V3, c: V3) -> f64 {
    dot(cross(sub(a, c), sub(b, c)), [0.0, 0.0, 1.0]).signum()
}

pub fn write(files: &mut Files, id: &str, car: &Car, f: &Frame) -> Result<Vec<String>> {
    let base = format!("content\\cars\\{id}");
    let ap = format!("{base}\\{id}.actor");
    let mut actor = Msg::parse(files.get(&ap).ok_or_else(|| anyhow!("no actor"))?)?;
    let slots = slot_paths(&actor);
    let pos_path = slots.get(&109).cloned().ok_or_else(|| anyhow!("the donor has no driver position layer"))?;
    let steer_path = slots.get(&101).cloned().ok_or_else(|| anyhow!("the donor has no driver steer layer"))?;
    let donor_pos = read_anim(files, &pos_path).ok_or_else(|| anyhow!("{pos_path} unreadable"))?;
    let donor_steer = read_anim(files, &steer_path).ok_or_else(|| anyhow!("{steer_path} unreadable"))?;
    let knh = find_ci(&car.dir, "driver_base_pos.knh").ok_or_else(|| anyhow!("no driver_base_pos.knh"))?;
    let k = crate::ac1::knh::parse(&std::fs::read(&knh)?)?;
    let ac1 = Ac1 { nodes: k.flat(), datum: f.datum };
    if !ac1.nodes.iter().any(|n| n.0 == "DRIVER:RIG_Hips") {
        bail!("driver_base_pos.knh has no DRIVER:RIG_* joints");
    }
    // the donor's seat: position frame 0, the steer clip's middle frame (grip)
    let parent: HashMap<String, String> = RIG.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
    let mut base_pose = Pose { local: HashMap::new(), parent: parent.clone() };
    for (b, fr) in &donor_pos {
        if let Some(m) = fr.first() {
            base_pose.local.insert(b.clone(), *m);
        }
    }
    let donor_rest = base_pose.clone();
    let mid = |fr: &Vec<M4>| fr[(fr.len() - 1) / 2];
    for (b, fr) in &donor_steer {
        if !fr.is_empty() {
            base_pose.local.insert(b.clone(), mid(fr));
        }
    }
    let donor_grip = base_pose.clone();
    for (b, _) in RIG {
        if !base_pose.local.contains_key(*b) {
            bail!("the donor's position layer lacks {b}");
        }
    }
    // AC1 steer clip
    let d3 = car.ini("driver3d.ini");
    let steer_name = d3.get("STEER_ANIMATION", "NAME").unwrap_or("steer.ksanim").to_string();
    let lock = d3.num("STEER_ANIMATION", "LOCK").unwrap_or(360.0).abs().max(30.0);
    let steer = ksanim(car, &steer_name);
    let mut notes = Vec::new();
    // seat
    let jt0 = match &steer {
        Some(tr) => ac1.joints(&ac1_frame(tr, (tr.iter().map(|t| t.frames.len()).max().unwrap_or(1) - 1) as f64 / 2.0)),
        None => ac1.joints(&HashMap::new()),
    };
    let mut seat = base_pose.clone();
    solve_body(&mut seat, &jt0)?;
    let names_pos: Vec<String> = donor_pos.iter().map(|(b, _)| b.clone()).collect();
    let pos_frames = donor_pos.iter().map(|(_, fr)| fr.len()).max().unwrap_or(201).max(1);
    let tracks: Vec<(&str, Vec<M4>)> = names_pos.iter().map(|b| (b.as_str(), vec![seat.local[b]; pos_frames])).collect();
    files.put(&pos_path, encode_clip(&stem_of(&pos_path), &tracks));
    let hips = seat.p("A_Hips");
    let head = seat.p("A_Head");
    notes.push(format!("driver: stock driver seated per driver_base_pos.knh: hips ({:.2}, {:.2}, {:.2}), head ({:.2}, {:.2}, {:.2}); hands {:.3} / {:.3} m from the AC1 wrists",
        hips[0], hips[1], hips[2], head[0], head[1], head[2],
        len(sub(seat.p("LeftHand"), j(&jt0, "RIG_HAND_L")?)), len(sub(seat.p("RightHand"), j(&jt0, "RIG_HAND_R")?))));
    // steer: 201 frames, -360..+360
    let steer_names: Vec<String> = donor_steer.iter().map(|(b, _)| b.clone()).collect();
    let n_out = 201usize;
    let mut steer_tracks: Vec<Vec<M4>> = vec![Vec::with_capacity(n_out); steer_names.len()];
    match &steer {
        Some(tr) => {
            let n = tr.iter().map(|t| t.frames.len()).max().unwrap_or(1);
            let m = (n - 1) as f64 / 2.0;
            // orbit direction: AC1 per frame vs the donor per frame
            let jm = ac1.joints(&ac1_frame(tr, m));
            let jk = ac1.joints(&ac1_frame(tr, (m + (n as f64 / 20.0).max(1.0)).min((n - 1) as f64)));
            let c = lerp(j(&jm, "RIG_HAND_L")?, j(&jm, "RIG_HAND_R")?, 0.5);
            let s_ac1 = orbit(j(&jm, "RIG_HAND_L")?, j(&jk, "RIG_HAND_L")?, c);
            let dl = donor_steer.iter().find(|(b, _)| b == "LeftHand").map(|x| x.1.clone());
            let s_evo = match dl {
                Some(fr) if fr.len() > 10 => {
                    let mut a = donor_rest.clone();
                    let mut b = donor_rest.clone();
                    for (bn, frs) in &donor_steer {
                        a.local.insert(bn.clone(), frs[(frs.len() - 1) / 2]);
                        b.local.insert(bn.clone(), frs[((frs.len() - 1) / 2 + 10).min(frs.len() - 1)]);
                    }
                    let cc = lerp(a.p("LeftHand"), a.p("RightHand"), 0.5);
                    orbit(a.p("LeftHand"), b.p("LeftHand"), cc)
                }
                _ => 1.0,
            };
            let sign = if s_ac1 == s_evo || s_ac1 == 0.0 { 1.0 } else { -1.0 };
            let at = |deg: f64| -> f64 { (sign * deg.clamp(-lock, lock) / lock + 1.0) * m };
            for kf in 0..n_out {
                let ang = -360.0 + 720.0 * kf as f64 / (n_out - 1) as f64;
                let ja = ac1.joints(&ac1_frame(tr, at(ang)));
                let w = ((ang.abs() - 180.0) / 180.0).clamp(0.0, 1.0);
                let w = w * w * (3.0 - 2.0 * w);
                let jt: HashMap<String, V3> = if w > 0.0 {
                    let jb = ac1.joints(&ac1_frame(tr, at(ang - 360.0 * ang.signum())));
                    ja.iter().map(|(k, v)| (k.clone(), lerp(*v, jb.get(k).copied().unwrap_or(*v), w))).collect()
                } else {
                    ja
                };
                let mut p = seat.clone();
                for (b, _) in &donor_steer {
                    p.local.insert(b.clone(), donor_grip.local[b]);
                }
                // fingers stay the donor's grip; arms onto this frame's hands
                for b in ["LEFT_Shoulder", "LeftArm", "LeftForeArm", "LeftHand", "RIGHT_Shoulder", "RightArm", "RighttForeArm", "RightHand"] {
                    p.local.insert(b.to_string(), seat.local[b]);
                }
                solve_arm(&mut p, &jt, &LEFT)?;
                solve_arm(&mut p, &jt, &RIGHT)?;
                for (ti, b) in steer_names.iter().enumerate() {
                    steer_tracks[ti].push(p.local.get(b).copied().unwrap_or(frame::I4));
                }
            }
            notes.push(format!("driver: steer layer from {steer_name} (LOCK {lock:.0}, {n} frames) as a full turn, 201 frames{}", if lock < 359.0 || lock > 361.0 { ", faded to the pose one turn back past 180 deg" } else { "" }));
        }
        None => {
            // no AC1 clip: the seat, arms held (the wheel turns under the grip)
            for (ti, b) in steer_names.iter().enumerate() {
                steer_tracks[ti] = vec![seat.local.get(b).copied().unwrap_or(frame::I4); n_out];
            }
            notes.push("driver: no AC1 steer clip - hands held at the seat pose".into());
        }
    }
    let st: Vec<(&str, Vec<M4>)> = steer_names.iter().zip(steer_tracks).map(|(b, v)| (b.as_str(), v)).collect();
    files.put(&steer_path, encode_clip(&stem_of(&steer_path), &st));
    // pedals: the donor's motion relative to its seat, on ours
    let mut pedal_notes = Vec::new();
    // (the clutch slot stays empty, as on stock: the brake layer already
    // drives the left leg and a second one would override it)
    for (slot, what) in [(102u32, "accelerator"), (103, "brake")] {
        let Some(path) = slots.get(&slot).cloned() else { continue };
        let Some(clip) = read_anim(files, &path) else { continue };
        let mut out: Vec<(String, Vec<M4>)> = Vec::new();
        let mut moved = 0.0f64;
        for (b, fr) in &clip {
            let rest = donor_rest.local.get(b).copied().unwrap_or(frame::I4);
            let inv = frame::inv(&rest).unwrap_or(frame::I4);
            let ours = seat.local.get(b).copied().unwrap_or(rest);
            let v: Vec<M4> = fr.iter().map(|m| {
                let d = frame::mul(m, &inv);
                frame::mul(&d, &ours)
            }).collect();
            for m in &v {
                moved = moved.max((0..12).map(|i| (m[i] - ours[i]).abs()).fold(0.0, f64::max));
            }
            out.push((b.clone(), v));
        }
        let refs: Vec<(&str, Vec<M4>)> = out.iter().map(|(a, b)| (a.as_str(), b.clone())).collect();
        files.put(&path, encode_clip(&stem_of(&path), &refs));
        pedal_notes.push(format!("{what} {} tracks (rotation up to {:.2})", out.len(), moved));
    }
    notes.push(format!("driver: pedal layers {}", pedal_notes.join(", ")));
    // H-shifter from AC1 shift.ksanim
    let shift = ksanim(car, "shift.ksanim");
    let approach = slots.get(&107).cloned();
    let change = slots.get(&105).cloned();
    let mut shifted = false;
    if let (Some(tr), Some(ap_path), Some(ch_path)) = (&shift, &approach, &change) {
        let n = tr.iter().map(|t| t.frames.len()).max().unwrap_or(1);
        let moves = |nm: &str| tr.iter().any(|t| t.node == nm && t.frames.iter().any(|m| (0..16).any(|i| (m[i] - t.frames[0][i]).abs() > 1e-3)));
        let side = if moves("DRIVER:RIG_Arm_R") || moves("DRIVER:RIG_HAND_R") { Some(&RIGHT) } else if moves("DRIVER:RIG_Arm_L") || moves("DRIVER:RIG_HAND_L") { Some(&LEFT) } else { None };
        if let Some(sd) = side {
            let dn = read_anim(files, ap_path).map(|c| c.len()).unwrap_or(22);
            let arm_bones: Vec<String> = RIG.iter().map(|x| x.0.to_string()).filter(|b| {
                let mut q = b.clone();
                loop {
                    if q == sd.clave {
                        return true;
                    }
                    match parent.get(&q) { Some(p) if !p.is_empty() => q = p.clone(), _ => return false }
                }
            }).collect();
            let _ = dn;
            let frames_ap = 32usize;
            let mut ap_tracks: Vec<Vec<M4>> = vec![Vec::new(); arm_bones.len()];
            let mut last = seat.clone();
            for kf in 0..frames_ap {
                let fa = (n - 1) as f64 * kf as f64 / (frames_ap - 1) as f64;
                let jt = ac1.joints(&ac1_frame(tr, fa));
                let mut p = seat.clone();
                solve_arm(&mut p, &jt, sd)?;
                for (ti, b) in arm_bones.iter().enumerate() {
                    ap_tracks[ti].push(p.local[b]);
                }
                last = p;
            }
            let at: Vec<(&str, Vec<M4>)> = arm_bones.iter().zip(ap_tracks).map(|(b, v)| (b.as_str(), v)).collect();
            files.put(ap_path, encode_clip(&stem_of(ap_path), &at));
            let ch: Vec<(&str, Vec<M4>)> = arm_bones.iter().map(|b| (b.as_str(), vec![last.local[b]; 22])).collect();
            files.put(ch_path, encode_clip(&stem_of(ch_path), &ch));
            shifted = true;
            notes.push(format!("driver: H-shifter layers from shift.ksanim ({} hand)", if std::ptr::eq(sd, &LEFT) { "left" } else { "right" }));
        }
    }
    let mut emptied = vec![110u32, 115, 112, 113, 114];
    if !shifted {
        emptied.extend([105, 106, 107]);
    }
    for s in &emptied {
        set_slot(&mut actor, *s, None)?;
        if let Some(p) = slots.get(s) {
            // shared files (up / down use one clip) only go when nothing else names them
            if !slots.iter().any(|(k, v)| v == p && !emptied.contains(k)) {
                files.remove(p);
            }
        }
    }
    // race cars get the racing outfit (stock race cars set the mechanical
    // presets' driver_model_type f18 and the actor's f200 to Racing)
    let racing = crate::car::exhaust::race_car(car);
    if racing {
        actor.msg_mut(103)?.set_varint(200, 1);
        for p in files.under(&format!("{base}\\presets")) {
            if !p.to_lowercase().ends_with(".mechanicalcarpreset") {
                continue;
            }
            let Some(mut m) = files.get(&p).and_then(|b| Msg::parse(b).ok()) else { continue };
            m.remove(18);
            let at = m.fields.iter().position(|f| f.num > 18).unwrap_or(m.fields.len());
            m.fields.insert(at, crate::io::pb::Field { num: 18, val: Val::Varint(1) });
            files.put(&p, m.encode());
        }
    }
    files.put(&ap, actor.encode());
    notes.push(format!("driver: {} outfit; idle / handbrake / paddle layers emptied{}", if racing { "racing" } else { "street" },
        if shifted { "" } else { ", no AC1 shift clip: H-shifter layers emptied" }));
    Ok(notes)
}

fn stem_of(p: &str) -> String {
    let n = p.rsplit('\\').next().unwrap_or(p);
    n.trim_end_matches(".animation").to_string()
}

/// Point driver slot `slot` at `path` (None = no clip; the slot and its
/// play type stay).
fn set_slot(actor: &mut Msg, slot: u32, path: Option<&str>) -> Result<()> {
    let car = actor.msg_mut(103)?;
    let da = car.msg_mut(31)?;
    let i = match da.fields.iter().position(|f| f.num == slot) {
        Some(i) => i,
        None => {
            da.push(slot, Val::Msg(Msg::default()));
            da.fields.len() - 1
        }
    };
    let s = da.open(i)?;
    s.remove(1);
    let mut a = Msg::default();
    if let Some(p) = path {
        a.set_str(1, p);
    }
    s.fields.insert(0, crate::io::pb::Field { num: 1, val: Val::Msg(a) });
    // the legacy mirror (slot - 100: {f1 path}) the donor's actor also carries
    if let Some(j) = da.fields.iter().position(|f| f.num == slot - 100) {
        let mut l = Msg::default();
        l.set_str(1, path.unwrap_or(""));
        da.fields[j].val = Val::Msg(l);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotations() {
        let r = from_to([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let v = rmul([1.0, 0.0, 0.0], &r);
        assert!((v[1] - 1.0).abs() < 1e-9 && v[0].abs() < 1e-9);
        let a = twist_angle([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        assert!((a - std::f64::consts::FRAC_PI_2).abs() < 1e-9);
    }

    #[test]
    fn two_bone_ik_reaches() {
        let parent: HashMap<String, String> = [("a", ""), ("b", "a"), ("c", "b")].iter().map(|(x, y)| (x.to_string(), y.to_string())).collect();
        let mut p = Pose { local: HashMap::new(), parent };
        p.local.insert("a".into(), frame::I4);
        p.local.insert("b".into(), frame::translation([0.3, 0.0, 0.0]));
        p.local.insert("c".into(), frame::translation([0.25, 0.0, 0.0]));
        p.ik("a", "b", "c", [0.2, 0.3, 0.1], [0.0, 1.0, 0.0]);
        let e = p.p("c");
        assert!(len(sub(e, [0.2, 0.3, 0.1])) < 1e-6, "{e:?}");
        assert!((len(p.p("b")) - 0.3).abs() < 1e-9);
    }
}
