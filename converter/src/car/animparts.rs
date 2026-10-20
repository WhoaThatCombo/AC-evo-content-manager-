//! AC1 moving parts -> EVO bones, car animation clips and dash needles.
//!
//! The donor's own clips (doors, wipers, pedals, shifter) animate donor
//! geometry, so the actor clean-up strips them; without this stage every
//! converted car bound no car animation at all and kept the donor's turbo
//! needle on a bone with no geometry. Here:
//!
//! - car ksanims (`animations\car_door_l.ksanim`, `car_wiper`, `car_shift*`,
//!   `car_hood`, `lights.ksanim` pop-ups...) -> one bone per animated kn5
//!   node, in the exterior AND interior mesh (`AC1_<node>` / `AC1_<node>_I`),
//!   the node's meshes skinned to it, one `.animation` per mesh side, the
//!   actor slot (f103.27) pointed at them;
//! - `analog_instruments.ini` needles -> an interior bone at the needle node
//!   (its frame: the needle turns about its local Z), the needle meshes
//!   forced interior and skinned to it, a `.caranalogicsystem` written from
//!   the ini. A car without a convertible needle gets NO analogic system
//!   (f103.103 removed), like the stock cars with digital dashes - the donor's
//!   turbo gauge on a bone without geometry is never left behind.
//!
//! Bones: rest local = the node's kn5 rest relative to the nearest animated
//! ancestor (or, under the mesh root, its kn5 world + the datum offset), so
//! the bind pose is the kn5 pose and vertices stay in car space. Clip frames
//! = the ksanim locals in the same frame (row-vector, translation 12..14).

use anyhow::Result;
use std::collections::{BTreeMap, HashMap, HashSet};

use crate::ac1::ini::{self, Ini};
use crate::ac1::kn5::{Kn5, Node, NodeKind};
use crate::ac1::ksanim::{read as ksanim, Track};
use crate::car::anim::encode_clip;
use crate::car::frame::M4;
use crate::files::Files;
use crate::io::pb::{Msg, Val};

/// Insert field `num` before the first field with a higher number (keeps
/// the stock field order of the actor's slot blocks).
fn put(m: &mut Msg, num: u32, val: Val) {
    let at = m.fields.iter().position(|f| f.num > num).unwrap_or(m.fields.len());
    m.fields.insert(at, crate::io::pb::Field { num, val });
}

pub const EXT_ROOT: &str = "CAR_BODY";
pub const INT_ROOT: &str = "COCKPIT_HR";

const I4: M4 = [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.];

fn mul(a: &M4, b: &M4) -> M4 {
    let mut o = [0f64; 16];
    for r in 0..4 {
        for c in 0..4 {
            o[r * 4 + c] = (0..4).map(|k| a[r * 4 + k] * b[k * 4 + c]).sum();
        }
    }
    o
}

fn f64m(m: &[f32; 16]) -> M4 {
    m.map(|x| x as f64)
}

/// Affine inverse (row-vector, translation in 12..14).
fn inv4(m: &M4) -> Option<M4> {
    let a = [[m[0], m[1], m[2]], [m[4], m[5], m[6]], [m[8], m[9], m[10]]];
    let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1]) - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0]) + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
    if det.abs() < 1e-12 {
        return None;
    }
    let c = |r: usize, k: usize| -> f64 {
        let (r1, r2) = ((r + 1) % 3, (r + 2) % 3);
        let (k1, k2) = ((k + 1) % 3, (k + 2) % 3);
        a[r1][k1] * a[r2][k2] - a[r1][k2] * a[r2][k1]
    };
    // inverse 3x3 = adjugate / det (adj[i][j] = cofactor[j][i])
    let mut o = [0f64; 16];
    for i in 0..3 {
        for j in 0..3 {
            o[i * 4 + j] = c(j, i) / det;
        }
    }
    let t = [m[12], m[13], m[14]];
    for j in 0..3 {
        o[12 + j] = -(t[0] * o[j] + t[1] * o[4 + j] + t[2] * o[8 + j]);
    }
    o[15] = 1.0;
    Some(o)
}

struct NodeInfo {
    name: String,
    /// '/'-joined names from the root (root excluded) - geometry::flatten's Inst.path
    path: String,
    parent: Option<usize>,
    /// kn5 local (transforms) or identity (mesh / other nodes)
    local: M4,
    world: M4,
    is_transform: bool,
    /// renderable meshes in the subtree (this node included)
    meshes: usize,
}

fn index(k: &Kn5) -> Vec<NodeInfo> {
    let mut out: Vec<NodeInfo> = Vec::new();
    fn walk(n: &Node, parent: Option<usize>, pw: &M4, path: &str, root: bool, out: &mut Vec<NodeInfo>) -> usize {
        let (local, is_t) = match &n.kind {
            NodeKind::Transform(m) => (f64m(m), true),
            _ => (I4, false),
        };
        let world = mul(&local, pw);
        let p = if root { String::new() } else if path.is_empty() { n.name.clone() } else { format!("{path}/{}", n.name) };
        let me = out.len();
        let own = matches!(&n.kind, NodeKind::Mesh(m) if m.renderable) as usize;
        out.push(NodeInfo { name: n.name.clone(), path: p.clone(), parent, local, world, is_transform: is_t, meshes: own });
        let mut sum = own;
        for c in &n.children {
            sum += walk(c, if root { None } else { Some(me) }, &world, &p, false, out);
        }
        out[me].meshes = sum;
        sum
    }
    walk(&k.root, None, &I4, "", true, &mut out);
    out
}

/// One output clip: (stem, slot numbers, play type, ext bytes, int bytes).
pub struct Clip {
    pub stem: String,
    pub slots: Vec<u32>,
    pub play: u64,
    pub frames: usize,
    /// (bone base name, local per frame)
    tracks: Vec<(String, Vec<M4>)>,
}

pub struct Needle {
    pub field: u32,
    pub bone: String,
    pub lut: Vec<(f64, f64)>,
    pub clock: bool,
    pub what: String,
}

#[derive(Default)]
pub struct Plan {
    /// path prefix -> bone base name (longest prefix wins)
    bind: Vec<(String, String)>,
    /// (base name, parent base name or None, rest local; for a bone on the
    /// steering wheel (`steer`) the rest WORLD, made local to STEER_BASE once
    /// that bone is placed - see fix_steer)
    bones: Vec<(String, Option<String>, M4)>,
    /// bones hanging off STEER_BASE (paddles): base names
    steer: HashSet<String>,
    /// STEER_BASE's world once the interior rig is placed
    steer_world: std::cell::Cell<Option<M4>>,
    /// paths whose meshes must go to the interior mesh (needles)
    interior: Vec<String>,
    pub clips: Vec<Clip>,
    pub needles: Vec<Needle>,
    pub notes: Vec<String>,
    /// bones that carry geometry (or a child that does) per mesh side; None = not settled yet
    used: Option<(HashSet<String>, HashSet<String>)>,
}

fn ext_name(base: &str) -> String {
    format!("AC1_{base}")
}
fn int_name(base: &str) -> String {
    format!("AC1_{base}_I")
}

fn seg_prefix(path: &str, prefix: &str) -> bool {
    path.len() >= prefix.len() && path[..prefix.len()].eq_ignore_ascii_case(prefix) && (path.len() == prefix.len() || path.as_bytes()[prefix.len()] == b'/')
}

impl Plan {
    /// The bone (mesh-side name) an instance at `path` follows, if any.
    pub fn bone_for(&self, path: &str, interior: bool) -> Option<String> {
        self.bind.iter().filter(|(p, _)| seg_prefix(path, p)).max_by_key(|(p, _)| p.len())
            .map(|(_, b)| if interior { int_name(b) } else { ext_name(b) })
    }
    pub fn force_interior(&self, path: &str) -> bool {
        self.interior.iter().any(|p| seg_prefix(path, p))
    }
    /// After routing: which side (exterior / interior mesh) each bone has
    /// geometry on - bones and clip tracks go only where they move something.
    /// `bound` = (instance path, interior?) of every part that may follow a bone.
    pub fn settle<'a>(&mut self, bound: impl Iterator<Item = (&'a str, bool)>) {
        let (mut ext, mut int) = (HashSet::new(), HashSet::new());
        for (path, interior) in bound {
            let Some((_, b)) = self.bind.iter().filter(|(p, _)| seg_prefix(path, p)).max_by_key(|(p, _)| p.len()) else { continue };
            if interior { int.insert(b.clone()); } else { ext.insert(b.clone()); }
        }
        // a used bone's ancestors are used
        let parent: HashMap<String, Option<String>> = self.bones.iter().map(|(n, p, _)| (n.clone(), p.clone())).collect();
        // steering-wheel parts live in the interior mesh only
        ext.retain(|b| !self.steer.contains(b));
        for set in [&mut ext, &mut int] {
            let mut add = Vec::new();
            for b in set.iter() {
                let mut q = parent.get(b).cloned().flatten();
                while let Some(x) = q {
                    add.push(x.clone());
                    q = parent.get(&x).cloned().flatten();
                }
            }
            set.extend(add);
        }
        // needles that lost their geometry (routed away) are not written
        self.needles.retain(|n| int.iter().any(|b| int_name(b) == n.bone));
        self.used = Some((ext, int));
    }
    fn on_side(&self, base: &str, interior: bool) -> bool {
        match &self.used {
            None => true,
            Some((e, i)) => if interior { i.contains(base) } else { e.contains(base) },
        }
    }
    /// The new bone messages for one mesh (appended after the donor's).
    pub fn bone_msgs(&self, interior: bool) -> Vec<Msg> {
        let root = if interior { INT_ROOT } else { EXT_ROOT };
        self.bones.iter().filter(|(n, _, _)| self.on_side(n, interior)).map(|(n, p, m)| {
            let name = if interior { int_name(n) } else { ext_name(n) };
            let parent = match p {
                Some(x) => if interior { int_name(x) } else { ext_name(x) },
                None if self.steer.contains(n) => "STEER_BASE".to_string(),
                None => root.to_string(),
            };
            bone_msg(&name, &parent, m)
        }).collect()
    }
    /// After STEER_BASE is recentred on the wheel: the paddle bones (rest
    /// worlds until now) become local to it. `bones` = the interior mesh's.
    pub fn fix_steer(&self, bones: &mut [Msg]) {
        if self.steer.is_empty() {
            return;
        }
        let names: Vec<String> = bones.iter().map(|b| b.str(1).unwrap_or_default()).collect();
        let Some(si) = names.iter().position(|n| n == "STEER_BASE") else { return };
        let sw = f64m(&crate::car::geometry::bone_worlds(bones)[si]);
        let Some(inv) = inv4(&sw) else { return };
        self.steer_world.set(Some(sw));
        for (n, p, m) in &self.bones {
            if p.is_some() || !self.steer.contains(n) {
                continue;
            }
            if let Some(i) = names.iter().position(|x| *x == int_name(n)) {
                bones[i] = bone_msg(&int_name(n), "STEER_BASE", &mul(m, &inv));
            }
        }
    }
    pub fn is_empty(&self) -> bool {
        self.bones.is_empty()
    }
    /// `bone_msgs` as skeleton entries (for the body stage).
    pub fn bone_list(&self, interior: bool) -> Vec<crate::car::geometry::Bone> {
        self.bone_msgs(interior).iter().map(msg_bone).collect()
    }
    /// `fix_steer` on a skeleton of `Bone`s.
    pub fn fix_steer_bones(&self, bones: &mut [crate::car::geometry::Bone]) {
        let mut msgs: Vec<Msg> = bones.iter().map(crate::car::geometry::bone_msg).collect();
        self.fix_steer(&mut msgs);
        for (b, m) in bones.iter_mut().zip(&msgs) {
            *b = msg_bone(m);
        }
    }
}

fn msg_bone(m: &Msg) -> crate::car::geometry::Bone {
    let t = m.sub(3).unwrap_or_default();
    let mut l = [0f64; 16];
    for (i, v) in l.iter_mut().enumerate() {
        *v = t.f32(i as u32 + 1).unwrap_or(0.0) as f64;
    }
    crate::car::geometry::Bone { name: m.str(1).unwrap_or_default(), parent: m.str(2).unwrap_or_default(), local: l }
}

fn bone_msg(name: &str, parent: &str, m: &M4) -> Msg {
    let mut b = Msg::default();
    b.set_str(1, name);
    b.set_str(2, parent);
    let mut mm = Msg::default();
    for (k, v) in m.iter().enumerate() {
        let v = *v as f32;
        if v != 0.0 {
            mm.push(k as u32 + 1, Val::F32(v.to_le_bytes()));
        }
    }
    b.push(3, Val::Msg(mm));
    b
}

/// Which actor slots a car ksanim drives (file stem, lower case).
fn slots_for(stem: &str) -> Option<(Vec<u32>, u64)> {
    let s = stem.trim_start_matches("car_").replace(['-', ' '], "_");
    let r = match s.as_str() {
        "door_l" | "door_fl" | "doors_l" => (vec![105], 3),
        "door_r" | "door_fr" | "doors_r" => (vec![106], 3),
        "door_rl" => (vec![107], 3),
        "door_rr" => (vec![108], 3),
        "wiper" | "wipers" => (vec![102], 2),
        "hood" | "bonnet" => (vec![103], 3),
        "trunk" | "boot" | "tailgate" => (vec![104], 3),
        "lights" | "light" | "headlights" | "popup" | "popups" => (vec![119], 3),
        "shift" | "shifter" | "gear" | "gearlever" => (vec![116, 117], 0),
        "shift_up" | "shiftup" => (vec![116], 0),
        "shift_dw" | "shift_down" | "shiftdw" | "shiftdown" => (vec![117], 0),
        "paddle_up" | "paddleup" | "shiftpad_up" => (vec![121], 0),
        "paddle_dw" | "paddle_down" | "paddledw" | "shiftpad_dw" | "shiftpad_down" => (vec![122], 0),
        "handbrake" => (vec![120], 1),
        "pedal_gas" | "pedal_accel" | "pedal_throttle" | "gas" | "throttle" => (vec![113], 1),
        "pedal_brake" | "brake" => (vec![114], 1),
        "pedal_clutch" | "clutch" => (vec![115], 1),
        _ => return None,
    };
    // the bare names are only car animations with the car_ prefix (shift.ksanim,
    // steer.ksanim... without it are DRIVER animations); lights.ksanim is the pop-ups
    if !stem.starts_with("car_") && s != "lights" {
        return None;
    }
    Some(r)
}

pub struct Input<'a> {
    pub k: &'a Kn5<'a>,
    pub ac1_dir: &'a std::path::Path,
    pub data: &'a BTreeMap<String, String>,
    pub ah: [f32; 3],
}

pub fn plan(inp: &Input) -> Plan {
    let nodes = index(inp.k);
    let mut by_name: HashMap<String, usize> = HashMap::new();
    for (i, n) in nodes.iter().enumerate() {
        // first transform of that name wins (ksanim tracks name transforms)
        if n.is_transform {
            by_name.entry(n.name.to_lowercase()).or_insert(i);
        }
    }
    let mut p = Plan::default();
    // ---- car ksanims: which nodes they move
    let mut clips_src: Vec<(String, Vec<u32>, u64, Vec<Track>)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(inp.ac1_dir.join("animations")) {
        let mut files: Vec<std::path::PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().map(|e| e.eq_ignore_ascii_case("ksanim")).unwrap_or(false)).collect();
        files.sort();
        for f in files {
            let stem = f.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
            let Some((slots, play)) = slots_for(&stem) else { continue };
            let tracks = match std::fs::read(&f).map_err(anyhow::Error::from).and_then(|b| ksanim(&b)) {
                Ok(t) => t,
                Err(e) => {
                    p.notes.push(format!("animation {stem}: unreadable ({e})"));
                    continue;
                }
            };
            // tracks that MOVE a node with geometry under it (AC1 exporters
            // write every node of the scene into each ksanim, static ones too)
            let mut odd = 0;
            let live: Vec<Track> = tracks.into_iter().filter(|t| {
                let Some(&i) = by_name.get(&t.node.to_lowercase()) else { return false };
                if t.frames.is_empty() || nodes[i].meshes == 0 {
                    return false;
                }
                let moves = t.frames.iter().any(|f| f.iter().zip(&t.frames[0]).any(|(a, b)| (a - b).abs() > 1e-4));
                // frame 0 must be the node's kn5 pose (else the part would jump)
                let l = &nodes[i].local;
                let f0 = &t.frames[0];
                let off = ((f0[12] - l[12]).powi(2) + (f0[13] - l[13]).powi(2) + (f0[14] - l[14]).powi(2)).sqrt();
                if moves && off > 0.05 {
                    odd += 1;
                    return false;
                }
                moves
            }).collect();
            if odd > 0 {
                p.notes.push(format!("animation {stem}: {odd} track(s) whose first frame is not the kn5 pose - left out"));
            }
            if live.is_empty() {
                p.notes.push(format!("animation {stem}: moves no geometry of the kn5 - skipped"));
                continue;
            }
            clips_src.push((stem, slots, play, live));
        }
    }
    // a later clip for the same slot (car_shift vs car_shift_up) replaces nothing: keep the first per slot
    let mut taken: HashSet<u32> = HashSet::new();
    clips_src.retain_mut(|c| {
        c.1.retain(|s| taken.insert(*s));
        !c.1.is_empty()
    });
    let mut animated: Vec<usize> = Vec::new();
    for (_, _, _, tr) in &clips_src {
        for t in tr {
            let i = by_name[&t.node.to_lowercase()];
            if !animated.contains(&i) {
                animated.push(i);
            }
        }
    }
    // ---- needles
    let ai = ini::parse(inp.data.get("analog_instruments.ini").map(|s| s.as_str()).unwrap_or(""));
    let needles = needle_specs(&ai, inp.data, &mut p.notes);
    let mut needle_nodes: Vec<(usize, Option<usize>, NeedleSpec)> = Vec::new();
    for spec in needles {
        // the node: a transform (rotates about its own frame) or a mesh node
        // (rotates in its parent's frame - its vertices are in that frame)
        let want = spec.object.to_lowercase();
        let hit = by_name.get(&want).copied().filter(|&i| nodes[i].meshes > 0)
            .map(|i| (i, None))
            .or_else(|| nodes.iter().position(|n| !n.is_transform && n.meshes > 0 && n.name.to_lowercase() == want)
                .map(|m| (nodes[m].parent.unwrap_or(m), Some(m))));
        match hit {
            Some((i, mesh)) => needle_nodes.push((i, mesh, spec)),
            None => p.notes.push(format!("needle [{}] {}: no such node with geometry in the kn5 - left out", spec.section, spec.object)),
        }
    }
    // ---- bones (animated nodes; needles last, never under a moving part's bone chain)
    let anc_animated = |mut i: usize, set: &[usize]| -> Option<usize> {
        while let Some(pp) = nodes[i].parent {
            if set.contains(&pp) {
                return Some(pp);
            }
            i = pp;
        }
        None
    };
    let base_of = |i: usize| -> String { nodes[i].name.replace(['/', '\\', ' '], "_") };
    let mut used: HashSet<String> = HashSet::new();
    let mut base_names: HashMap<usize, String> = HashMap::new();
    let unique = |i: usize, used: &mut HashSet<String>| -> String {
        let mut b = base_of(i);
        let mut n = 2;
        while used.contains(&b.to_lowercase()) {
            b = format!("{}_{n}", base_of(i));
            n += 1;
        }
        used.insert(b.to_lowercase());
        b
    };
    // parent-first order so each bone's parent exists before it
    animated.sort_by_key(|&i| {
        let mut d = 0;
        let mut j = i;
        while let Some(pp) = nodes[j].parent {
            d += 1;
            j = pp;
        }
        d
    });
    // nodes on the steering wheel: their bones hang off STEER_BASE
    let on_wheel = |i: usize| nodes[i].path.split('/').any(|s| s.eq_ignore_ascii_case("STEER_HR") || s.eq_ignore_ascii_case("STEER_LR"));
    // relative local of node i under ancestor a (exclusive), with i's own local = li
    let rel = |i: usize, li: &M4, a: Option<usize>| -> M4 {
        let mut m = *li;
        let mut j = nodes[i].parent;
        while let Some(x) = j {
            if Some(x) == a {
                break;
            }
            m = mul(&m, &nodes[x].local);
            j = nodes[x].parent;
        }
        if a.is_none() {
            m[12] += inp.ah[0] as f64;
            m[13] += inp.ah[1] as f64;
            m[14] += inp.ah[2] as f64;
        }
        m
    };
    for &i in &animated {
        let b = unique(i, &mut used);
        let a = anc_animated(i, &animated);
        if on_wheel(i) {
            p.steer.insert(b.clone());
        }
        p.bones.push((b.clone(), a.map(|x| base_names[&x].clone()), rel(i, &nodes[i].local, a)));
        p.bind.push((nodes[i].path.clone(), b.clone()));
        base_names.insert(i, b);
    }
    for (i, mesh, spec) in needle_nodes {
        let b = unique(i, &mut used);
        let b = if b.to_lowercase().starts_with("arrow") || b.to_lowercase().contains("needle") { b } else { format!("NEEDLE_{b}") };
        // rotating with a moving parent part is not supported: a needle hangs off the cockpit
        let w = {
            let mut w = nodes[i].world;
            w[12] += inp.ah[0] as f64;
            w[13] += inp.ah[1] as f64;
            w[14] += inp.ah[2] as f64;
            w
        };
        p.bones.push((b.clone(), None, w));
        let path = match mesh { Some(m) => nodes[m].path.clone(), None => nodes[i].path.clone() };
        p.bind.push((path.clone(), b.clone()));
        p.interior.push(path);
        p.needles.push(Needle { field: spec.field, bone: int_name(&b), lut: spec.lut, clock: spec.clock, what: format!("{} -> {}", spec.section, spec.channel) });
    }
    // ---- clips
    for (stem, slots, play, tracks) in clips_src {
        let n = tracks.iter().map(|t| t.frames.len()).max().unwrap_or(1);
        let mut keyed: Vec<(String, Vec<M4>)> = Vec::new();
        for t in &tracks {
            let i = by_name[&t.node.to_lowercase()];
            let a = anc_animated(i, &animated);
            let keys: Vec<M4> = (0..n).map(|f| rel(i, &t.frames[f.min(t.frames.len() - 1)], a)).collect();
            keyed.push((base_names[&i].clone(), keys));
        }
        // a shift clip moving the steering wheel's paddles drives the paddle slots
        let slots = if !keyed.is_empty() && keyed.iter().all(|(b, _)| p.steer.contains(b)) {
            slots.iter().map(|s| match s { 116 => 121, 117 => 122, x => *x }).collect()
        } else {
            slots
        };
        p.clips.push(Clip { stem: stem.clone(), slots, play, frames: n, tracks: keyed });
    }
    p
}

// ------------------------------------------------------------ needles --

// Needles: `analog_instruments.ini` -> `.caranalogicsystem`, in
// `car::needles` (requirements car-animations-needles.md).

pub use crate::car::needles::NeedleSpec;

fn needle_specs(ai: &Ini, data: &BTreeMap<String, String>, notes: &mut Vec<String>) -> Vec<NeedleSpec> {
    crate::car::needles::specs(ai, data, notes)
}

fn analogic_bytes(needles: &[Needle]) -> Vec<u8> {
    crate::car::needles::analogic(needles)
}

/// Write clips + analogic system and point the actor at them. Runs after
/// finish::convert (which empties the donor's car animation slots).
pub fn write(files: &mut Files, id: &str, p: &Plan) -> Result<Vec<String>> {
    let base = format!("content\\cars\\{id}");
    let ap = format!("{base}\\{id}.actor");
    let Some(ab) = files.get(&ap).cloned() else { return Ok(vec![]) };
    let mut actor = Msg::parse(&ab)?;
    let mut notes = p.notes.clone();
    let mut slot_paths: BTreeMap<u32, (u64, Vec<String>)> = BTreeMap::new();
    for c in &p.clips {
        let mut paths = Vec::new();
        for (interior, side) in [(false, "ext"), (true, "int")] {
            let steer_inv = p.steer_world.get().and_then(|w| inv4(&w));
            let top = |b: &str| p.bones.iter().any(|(n, par, _)| n == b && par.is_none());
            let tr: Vec<(String, Vec<M4>)> = c.tracks.iter().filter(|(b, _)| p.on_side(b, interior))
                .filter(|(b, _)| !(p.steer.contains(b) && top(b) && steer_inv.is_none()))
                .map(|(b, k)| {
                    let k = match steer_inv {
                        Some(inv) if p.steer.contains(b) && top(b) => k.iter().map(|m| mul(m, &inv)).collect(),
                        _ => k.clone(),
                    };
                    (if interior { int_name(b) } else { ext_name(b) }, k)
                }).collect();
            if tr.is_empty() {
                continue;
            }
            let refs: Vec<(&str, Vec<M4>)> = tr.iter().map(|(a, b)| (a.as_str(), b.clone())).collect();
            let path = format!("{base}\\animations\\{id}_ac1_{}_{side}.animation", c.stem);
            files.put(&path, encode_clip(&format!("{}_{side}", c.stem), &refs));
            paths.push(path);
        }
        if paths.is_empty() {
            notes.push(format!("animation {}: its parts were not converted - skipped", c.stem));
            continue;
        }
        for s in &c.slots {
            let e = slot_paths.entry(*s).or_insert((c.play, Vec::new()));
            e.1.extend(paths.iter().cloned());
        }
        notes.push(format!("animation {} -> slot {:?} ({} frames, {} bones)", c.stem, c.slots, c.frames, c.tracks.len()));
    }
    {
        let a = actor.msg_mut(103)?;
        if !slot_paths.is_empty() {
            if a.get(27).is_none() {
                put(a, 27, Val::Msg(Msg::default()));
            }
            let i27 = a.fields.iter().position(|f| f.num == 27).unwrap();
            let an = a.open(i27)?;
            for (slot, (play, paths)) in &slot_paths {
                let pos = an.fields.iter().position(|f| f.num == *slot);
                let i = match pos {
                    Some(i) => {
                        if let Val::Bytes(b) = &an.fields[i].val {
                            if Msg::parse(b).is_err() || b.is_empty() {
                                an.fields[i].val = Val::Msg(Msg::default());
                            }
                        }
                        i
                    }
                    None => {
                        put(an, *slot, Val::Msg(Msg::default()));
                        an.fields.iter().position(|f| f.num == *slot).unwrap()
                    }
                };
                let s = an.open(i)?;
                s.remove(1);
                let mut refs = Vec::new();
                for pth in paths {
                    let mut r = Msg::default();
                    r.set_str(1, pth);
                    r.set_f32(2, 1.0);
                    refs.push(crate::io::pb::Field { num: 1, val: Val::Msg(r) });
                }
                // f1 refs first, then the slot's own params
                let rest = std::mem::take(&mut s.fields);
                s.fields = refs;
                s.fields.extend(rest);
                if s.f32(2).is_none() {
                    s.set_f32(2, 1.0);
                }
                s.remove(13);
                if *play != 0 {
                    s.set_varint(13, *play);
                }
                // legacy mirror (2..19 = 102..119)
                let legacy = slot - 100;
                if (2..=19).contains(&legacy) {
                    let mut l = Msg::default();
                    for pth in paths {
                        l.push(1, Val::Bytes(pth.as_bytes().to_vec()));
                    }
                    if let Some(j) = an.fields.iter().position(|f| f.num == legacy) {
                        an.fields[j].val = Val::Msg(l);
                    } else {
                        put(an, legacy, Val::Msg(l));
                    }
                }
            }
        }
        // dash needles: our analogic system, or none at all
        let old = a.str(103);
        if p.needles.is_empty() {
            a.remove(103);
            if let Some(o) = &old {
                files.remove(o);
            }
            notes.push("needles: none convertible - analogic system removed (no donor turbo gauge)".into());
        } else {
            let path = old.clone().unwrap_or_else(|| format!("{base}\\instruments\\{id}.caranalogicsystem"));
            files.put(&path, analogic_bytes(&p.needles));
            if old.is_none() {
                a.set_str(103, &path);
            }
            notes.push(format!("needles: {}", p.needles.iter().map(|n| format!("{} ({})", n.what, n.bone)).collect::<Vec<_>>().join(", ")));
        }
    }
    files.put(&ap, actor.encode());
    Ok(notes)
}
