//! Car lamps (requirements car-lights-leds-display.md): which AC1 meshes are
//! which lamp, their EVO lamp materials, the lighting system and the actor's
//! physical lights.
//!
//! # What the game wants (read off stock cars)
//!
//! - A lamp material (VehicleLight for reflectors / housings, VehicleGlass
//!   for lenses) carries two RGBA8 function masks. `FunctionMask1`: at the
//!   front R high beam, G low beam, B daylight; at the rear R rain-fog, G
//!   brake, B tail. `FunctionMask2`: B indicator, G reverse, R special
//!   (measured on ks_mazda_mx5_na: its F_1 R channel is lit only on the
//!   right rear lamp, where its only rain-fog lamp is; F_2 G only on the left
//!   rear, its reverse lamp; F_2 B on all four indicators).
//! - Front / rear and left / right come from the lamp vertices' colour
//!   stream (mesh f13) in 1/255 steps: R left, G right, B rear, A 1.
//!   Measured on mx5_na, ae86_sprinter_trueno and 992_gt3_rs: front-left
//!   lamp vertices (1, 0, 0, 1), front-right (0, 1, 0, 1), rear-left
//!   (1, 0, 1, 1), rear-right (0, 1, 1, 1), centre rear (1, 1, 1, 1).
//! - `.carlightingsystem` (CarLightingSystemData): one stage per switch
//!   position (stock: 3), each a list of sets {category, entries {function,
//!   reflector / glass / emitter intensity, blinker}}, plus a shared stage
//!   (brake, reverse, indicators, hazards, rain light) and named blinkers.
//! - The actor's physical lights (CarActorData.lights): lamps that throw
//!   light, at their position with the functions they project.
//!
//! # From AC1
//!
//! - `lights.ini` `[LIGHT_n]` (lit with the headlights) and `[BRAKE_n]`
//!   (lit when braking; an `OFF_COLOR` means it is also a tail lamp). A
//!   LIGHT mesh at the front is a low beam, at the rear a tail lamp, unless
//!   its name says indicator / reverse / fog / high beam: whole-word
//!   keywords measured on the AC1 install's lamp names (`name_func`,
//!   data/lamp_tokens.tsv by examples/lampcorpus.rs). The corpus declares
//!   no daylight lamps (CSP has no such kind) and almost no fog lamps, so
//!   no name makes a DRL and none a fog lamp today.
//! - Vanilla AC1 has one headlight state: when no lamp is declared high
//!   beam, the low beams carry high beam too (game evidence); when only
//!   high beams are named, they carry low beam too.
//! - Fake light cones (FRONT_LIGHT_BEAM and the like) are AC1 effects and
//!   are left out of the car.
//! - Indicators, hazards, high beam, reverse and extra brake lamps from the
//!   car's Custom Shaders Patch config: `[EMISSIVE_<TYPE>_n]` (NAME / MESHES)
//!   and `[CustomEmissive]` sections whose `@` lines name TurningLights /
//!   ReverseLights / BrakeLights / ... (every repeated section counts).
//! - Dashboard backlights (cockpit meshes, gauge / needle / display names)
//!   are not lamps: they stay in the cockpit on an emissive interior
//!   material, never on the exterior lamp glass (game evidence).
//!
//! # Ours
//!
//! - Each mesh's function set gets a section (and material) of its own, so
//!   one uniform mask per material paints exactly AC1's per-mesh functions
//!   over the lamp's UV area (mask painting resolution: one 128 x 128 page).
//!   A CSP indicator on a mesh that is also another lamp (CSP lights a UV
//!   part of it) is dropped: the whole lamp would blink.
//! - Intensities and colours: the stock ks_mazda_mx5_na values (the stock
//!   twin of the AC1 Miata) per function; the AC1 colour only says whether a
//!   lamp is lit at all.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use anyhow::Result;

use crate::ac1::car::Car;
use crate::car::body::Plan;
use crate::car::frame::{self, Frame};
use crate::car::fx;
use crate::car::geometry::Inst;
use crate::car::materials::{Hints, Kind};
use crate::files::Files;
use crate::io::material::{get_slot, set_prop, set_slot};
use crate::io::pb::{Msg, Val};

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub enum Func {
    Drl,
    Low,
    High,
    Tail,
    Brake,
    Fog,
    Reverse,
    Indicator,
}

impl Func {
    fn tag(self) -> &'static str {
        match self {
            Func::Drl => "drl", Func::Low => "low", Func::High => "high", Func::Tail => "tail",
            Func::Brake => "brake", Func::Fog => "fog", Func::Reverse => "rev", Func::Indicator => "ind",
        }
    }
}

/// What the lamp stage found in the AC1 data.
#[derive(Default)]
pub struct LampPlan {
    /// lower-case exterior node -> functions
    pub funcs: BTreeMap<String, BTreeSet<Func>>,
    /// indicator lamps at the front / at the rear
    pub ind_front: bool,
    pub ind_rear: bool,
    /// lower-case cockpit node -> backlight colour (AC1 COLOR)
    pub backlights: BTreeMap<String, [f32; 3]>,
    pub notes: Vec<String>,
}

impl LampPlan {
    pub fn has(&self, f: Func) -> bool {
        self.funcs.values().any(|s| s.contains(&f))
    }
}

fn group_tag(s: &BTreeSet<Func>) -> String {
    s.iter().map(|f| f.tag()).collect::<Vec<_>>().join("_")
}

/// Dashboard / instrument names (lights.ini lists their backlights).
pub fn dashy(n: &str) -> bool {
    let l = n.to_lowercase();
    ["needle", "arrow", "gauge", "dash", "cockpit", "display", "digit", "led", "rpm", "speedo", "lcd", "interior", "int_", "number", "instrument", "clock", "button", "screen"]
        .iter().any(|k| l.contains(k))
}

fn by_name(l: &str, keys: &[&str]) -> bool {
    keys.iter().any(|k| l.contains(k))
}

/// A mesh name's word tokens: split on every non-letter / non-digit, on
/// lower-to-upper case changes (camel case) and between letters and digits;
/// lower case; digits-only and one-letter tokens dropped
/// (`FrontLight_L01` -> front, light).
pub fn tokens(name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut prev: Option<char> = None;
    let flush = |cur: &mut String, out: &mut Vec<String>| {
        if cur.chars().count() > 1 && !cur.chars().all(|c| c.is_ascii_digit()) {
            out.push(cur.to_lowercase());
        }
        cur.clear();
    };
    for c in name.chars() {
        if !c.is_alphanumeric() {
            flush(&mut cur, &mut out);
            prev = None;
            continue;
        }
        if let Some(p) = prev {
            let split = (p.is_lowercase() && c.is_uppercase()) || (p.is_alphabetic() != c.is_alphabetic());
            if split {
                flush(&mut cur, &mut out);
            }
        }
        cur.push(c);
        prev = Some(c);
    }
    flush(&mut cur, &mut out);
    out
}

/// The lamp-name keywords: tokens that name one function on at least 90 %
/// of the lamps of the AC1 corpus that carry them (data/lamp_tokens.tsv,
/// written by examples/lampcorpus.rs from every lights.ini / CSP config of
/// the AC1 install; its header states the rule): token -> (function,
/// precision).
fn keywords() -> &'static HashMap<String, (Func, f64)> {
    static K: std::sync::OnceLock<HashMap<String, (Func, f64)>> = std::sync::OnceLock::new();
    K.get_or_init(|| {
        let mut k = HashMap::new();
        for l in include_str!("../../data/lamp_tokens.tsv").lines().filter(|l| !l.starts_with('#')).skip(1) {
            let c: Vec<&str> = l.split('\t').collect();
            let (Some(kw), Some(prec)) = (c.last(), c.len().checked_sub(2).and_then(|i| c[i].parse::<f64>().ok())) else { continue };
            let f = match *kw {
                "indicator" => Func::Indicator,
                "reverse" => Func::Reverse,
                "fog" => Func::Fog,
                "high" => Func::High,
                "low" => Func::Low,
                _ => continue,
            };
            k.insert(c[0].to_string(), (f, prec));
        }
        k
    })
}

/// The function a lamp's name says (whole tokens, `tokens`): the keyword of
/// the highest corpus precision, except that a low-beam word beats a
/// high-beam word (a low beam named after both, `anabbaglianti` holding
/// `abbaglianti`, is a low beam). None: the name says nothing.
pub fn name_func(name: &str) -> Option<Func> {
    let k = keywords();
    let hits: Vec<(Func, f64)> = tokens(name).iter().filter_map(|t| k.get(t).copied()).collect();
    if hits.iter().any(|h| h.0 == Func::Low) && hits.iter().any(|h| h.0 == Func::High) {
        return Some(Func::Low);
    }
    hits.into_iter().max_by(|a, b| a.1.total_cmp(&b.1)).map(|h| h.0)
}

/// A CSP lamp kind (an `[EMISSIVE_<TYPE>_n]` type or a `[CustomEmissive]`
/// `@` kind): Some(Some(f)) a declared function, Some(None) a headlight /
/// rear light (low beam or tail by position), None not a lamp (dashboard
/// lights and the like).
pub fn csp_kind(t: &str) -> Option<Option<Func>> {
    let t = t.to_uppercase();
    if t.starts_with("DASH") {
        return None; // dashboard telltales (DashIndicator, DashTurningLights, ...)
    }
    Some(Some(if t.starts_with("TURNSIGNAL") || t.starts_with("HAZARD") || t.contains("TURNINGLIGHT") || t.contains("INDICATOR") {
        Func::Indicator
    } else if t.starts_with("HIGHBEAM") || t.contains("HIGHBEAM") {
        Func::High
    } else if t.starts_with("REVERSE") || t.contains("REVERSELIGHT") {
        Func::Reverse
    } else if t.starts_with("BRAKE") || t.contains("BRAKELIGHT") || t.contains("BRAKINGLIGHT") {
        Func::Brake
    } else if t.starts_with("FOG") || t.starts_with("RAIN") {
        Func::Fog
    } else if t.starts_with("LIGHT") || t.contains("HEADLIGHT") || t.contains("LOWBEAM") || t.contains("REARLIGHT") || t.contains("TAILLIGHT") {
        return Some(None);
    } else {
        return None;
    }))
}

/// Every lamp a CSP config declares: (mesh names, kind) per `[EMISSIVE_*]`
/// section and per `@` line of the other sections (`[CustomEmissive]`,
/// with the section's or the line's own `Meshes`).
pub fn csp_lamps(secs: &[fx::CspSection]) -> Vec<(Vec<String>, Option<Func>)> {
    let mut hits = Vec::new();
    for s in secs {
        if let Some(t) = s.name.strip_prefix("EMISSIVE_") {
            if let Some(k) = csp_kind(t) {
                let names = s.get("NAME").or(s.get("MESHES")).map(fx::name_list).unwrap_or_default();
                hits.push((names, k));
            }
        } else {
            let own = s.get("MESHES").or(s.get("NAME")).map(fx::name_list).unwrap_or_default();
            for (k, v) in &s.keys {
                if k != "@" {
                    continue;
                }
                let first = v.split(',').next().unwrap_or("").trim();
                let Some(kk) = csp_kind(first) else { continue };
                let inline = v.split(',').filter_map(|x| x.split_once('=')).find(|(a, _)| a.trim().eq_ignore_ascii_case("meshes")).map(|(_, b)| fx::name_list(b)).unwrap_or_default();
                hits.push((if inline.is_empty() { own.clone() } else { inline }, kk));
            }
        }
    }
    hits
}

/// EVO-space centroid of every instance of a node name (lower case).
fn centroids(insts: &[Inst], f: &Frame) -> HashMap<String, ([f64; 3], bool)> {
    let mut acc: HashMap<String, ([f64; 3], f64, bool)> = HashMap::new();
    for i in insts {
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
        let c = f.evo([s[0] / n as f64, s[1] / n as f64, s[2] / n as f64]);
        let e = acc.entry(i.node.to_lowercase()).or_insert(([0.0; 3], 0.0, false));
        for k in 0..3 {
            e.0[k] += c[k];
        }
        e.1 += 1.0;
        e.2 |= i.cockpit;
    }
    acc.into_iter().map(|(k, (s, n, c))| (k, ([s[0] / n, s[1] / n, s[2] / n], c))).collect()
}

/// Read lights.ini + the CSP config; fill `hints` (lamp meshes, one section
/// tag per function set, cockpit backlights).
pub fn prepare(car: &Car, insts: &[Inst], f: &Frame, hints: &mut Hints) -> LampPlan {
    let mut p = LampPlan::default();
    let cen = centroids(insts, f);
    let front = |n: &str| cen.get(n).map(|c| c.0[2] > 0.0).unwrap_or(true);
    let cockpit = |n: &str| cen.get(n).map(|c| c.1).unwrap_or(false);
    let add = |p: &mut LampPlan, n: &str, fs: &[Func], src: &str| {
        if !cen.contains_key(n) {
            p.notes.push(format!("lights: {src} names {n}, not a mesh of the kn5"));
            return;
        }
        p.funcs.entry(n.to_string()).or_default().extend(fs.iter().copied());
    };
    let li = car.ini("lights.ini");
    let mut declared_high = false;
    for (sec, kv) in li.sections.iter() {
        let brake = sec.starts_with("BRAKE_");
        if !(brake || sec.starts_with("LIGHT_")) {
            continue;
        }
        let Some(names) = kv.get("NAME") else { continue };
        let col = fx::rgb(kv.get("COLOR"));
        let off = fx::rgb(kv.get("OFF_COLOR"));
        let lit = col.iter().any(|v| *v > 0.05);
        if !lit && !off.iter().any(|v| *v > 0.05) {
            continue;
        }
        for n in fx::name_list(names) {
            if cockpit(&n) || dashy(&n) {
                if cen.contains_key(&n) {
                    p.backlights.insert(n, col);
                }
                continue;
            }
            // fake light cones (FRONT_LIGHT_BEAM on Kunos cars) are AC1
            // effects, not lamps: the game throws real light
            if !brake && by_name(&n, &["beam", "cone", "flare"]) && !by_name(&n, &["high", "low"]) {
                p.notes.push(format!("lights: {n} is a light cone, not a lamp - left out of the car"));
                hints.drop.insert(n);
                continue;
            }
            let fr = front(&n);
            let mut fs = Vec::new();
            if brake {
                fs.push(Func::Brake);
                if off.iter().any(|v| *v > 0.05) {
                    fs.push(Func::Tail);
                }
            } else {
                match name_func(&n) {
                    Some(Func::Indicator) => fs.push(Func::Indicator),
                    Some(Func::Reverse) => fs.push(Func::Reverse),
                    Some(Func::Fog) => fs.push(if fr { Func::Drl } else { Func::Fog }),
                    Some(Func::High) if fr => {
                        fs.push(Func::High);
                        declared_high = true;
                    }
                    _ => fs.push(if fr { Func::Low } else { Func::Tail }),
                }
            }
            add(&mut p, &n, &fs, sec);
        }
    }
    // CSP config: indicators and the functions vanilla AC1 lacks
    if let Some((path, secs)) = fx::csp_config(car) {
        let before = p.funcs.len();
        for (names, k) in csp_lamps(&secs) {
            for n in names {
                if !cen.contains_key(&n) || cockpit(&n) || dashy(&n) {
                    continue;
                }
                let fr = front(&n);
                let fun = k.unwrap_or(if fr { Func::Low } else { Func::Tail });
                if fun == Func::High && fr {
                    declared_high = true;
                }
                add(&mut p, &n, &[fun], "csp");
            }
        }
        p.notes.push(format!("lights: CSP config {} ({} lamp mesh(es) more)", path.display(), p.funcs.len().saturating_sub(before)));
    }
    // a front Fog / Tail / Reverse has no channel at the front: drop it
    for (n, s) in p.funcs.iter_mut() {
        if front(n) {
            for x in [Func::Fog, Func::Tail, Func::Reverse, Func::Brake] {
                if s.remove(&x) {
                    if x == Func::Tail || x == Func::Brake {
                        s.insert(Func::Drl);
                    }
                }
            }
        } else {
            for x in [Func::Low, Func::High, Func::Drl] {
                if s.remove(&x) {
                    s.insert(Func::Tail);
                }
            }
        }
    }
    // A CSP indicator on a mesh that is also another lamp lights only part
    // of that mesh in CSP (UV channels); a whole-mesh mask would blink the
    // whole lamp amber: such meshes keep their other functions only (ours).
    let mut shared = 0;
    for s in p.funcs.values_mut() {
        let main = [Func::Low, Func::High, Func::Tail, Func::Brake, Func::Drl].iter().any(|f| s.contains(f));
        if s.contains(&Func::Indicator) && main {
            s.remove(&Func::Indicator);
            shared += 1;
        }
    }
    if shared > 0 {
        p.notes.push(format!("lights: {shared} mesh(es) shared by an indicator and another lamp keep the other lamp only"));
    }
    p.funcs.retain(|_, s| !s.is_empty());
    for (n, s) in &p.funcs {
        if s.contains(&Func::Indicator) {
            if front(n) { p.ind_front = true } else { p.ind_rear = true }
        }
    }
    // the other way round too: a car whose only headlamps are named high
    // beams (lotus_exige_v6_cup) lights them as low beams as well
    if !p.funcs.values().any(|s| s.contains(&Func::Low)) {
        for s in p.funcs.values_mut() {
            if s.contains(&Func::High) {
                s.insert(Func::Low);
            }
        }
    }
    if !declared_high {
        let mut n = 0;
        for s in p.funcs.values_mut() {
            if s.contains(&Func::Low) {
                s.insert(Func::High);
                n += 1;
            }
        }
        if n > 0 {
            p.notes.push(format!("lights: no high beam declared - {n} low-beam mesh(es) carry it too"));
        }
    }
    hints.lamps = p.funcs.keys().cloned().collect();
    for (n, s) in &p.funcs {
        hints.groups.insert(n.clone(), group_tag(s));
    }
    hints.backlights = p.backlights.keys().cloned().collect();
    let mut count: BTreeMap<String, usize> = BTreeMap::new();
    for s in p.funcs.values() {
        *count.entry(group_tag(s)).or_default() += 1;
    }
    p.notes.push(format!("lights: {} lamp mesh(es) [{}], {} dash backlight(s)", p.funcs.len(),
        count.iter().map(|(k, v)| format!("{k} x{v}")).collect::<Vec<_>>().join(", "), p.backlights.len()));
    p
}

/// (FunctionMask1, FunctionMask2) of a function set (see the module header).
fn masks(s: &BTreeSet<Func>) -> ([u8; 4], [u8; 4]) {
    let (mut a, mut b) = ([0u8, 0, 0, 255], [0u8, 0, 0, 255]);
    for f in s {
        match f {
            Func::High | Func::Fog => a[0] = 255,
            Func::Low | Func::Brake => a[1] = 255,
            Func::Drl | Func::Tail => a[2] = 255,
            Func::Reverse => b[1] = 255,
            Func::Indicator => b[2] = 255,
        }
    }
    (a, b)
}

/// Stock ks_mazda_mx5_na colours per lamp function (linear RGB).
const WARM: [f32; 3] = [0.888, 0.707, 0.434];
const RED: [f32; 3] = [1.0, 0.0, 0.0];
const AMBER: [f32; 3] = [1.0, 0.246, 0.0];

fn colour_props(m: &mut Msg) {
    for (n, c) in [("DaylightColor", WARM), ("LowBeamColor", WARM), ("HighBeamColor", WARM), ("ReverseLightColor", WARM),
                   ("RearLightColor", RED), ("BrakeLightColor", RED), ("RainFogLightColor", RED),
                   ("FrontIndicatorColor", AMBER), ("RearIndicatorColor", AMBER)] {
        set_prop(m, n, &[c[0], c[1], c[2], 0.0]);
    }
}

/// After the body is written: lamp masks and colours, lamp vertex colours,
/// backlight materials, the lighting system and the physical lights.
#[allow(clippy::too_many_arguments)]
pub fn write(files: &mut Files, id: &str, stem: &str, plan: &Plan, insts: &[Inst], lp: &LampPlan,
             written: &[(String, Kind, usize, bool, String)], popups: bool) -> Result<Vec<String>> {
    let base = format!("content\\cars\\{id}");
    let mut notes = Vec::new();
    // section name -> its function set (from any of its parts' nodes)
    let mut sec_funcs: BTreeMap<String, BTreeSet<Func>> = BTreeMap::new();
    for s in plan.sections.iter().filter(|s| s.kind == Kind::Lamp) {
        let mut set = BTreeSet::new();
        for (ii, _) in &s.parts {
            if let Some(fs) = lp.funcs.get(&insts[*ii].node.to_lowercase()) {
                set.extend(fs.iter().copied());
            }
        }
        sec_funcs.insert(s.name.clone(), set);
    }
    // ---- materials: masks + colours
    let mut tex_done: HashMap<[u8; 4], String> = HashMap::new();
    let mut tex_for = |files: &mut Files, rgba: [u8; 4], srgb: bool| -> String {
        if let Some(p) = tex_done.get(&rgba) {
            return p.clone();
        }
        let p = format!("{base}\\texture\\lights\\ext_lights_f_{:02x}{:02x}{:02x}.texture", rgba[0], rgba[1], rgba[2]);
        fx::uniform_texture(files, &p, rgba, srgb);
        tex_done.insert(rgba, p.clone());
        p
    };
    let mut lamp_secs = 0;
    for (name, kind, _, _, mat) in written {
        if *kind != Kind::Lamp {
            continue;
        }
        let Some(fs) = sec_funcs.get(name) else { continue };
        let Some(mut m) = files.get(mat).and_then(|b| Msg::parse(b).ok()) else { continue };
        let (a, b) = masks(fs);
        let pa = tex_for(files, a, true);
        let pb = tex_for(files, b, true);
        set_slot(&mut m, "FunctionMask1", &pa);
        set_slot(&mut m, "FunctionMask2", &pb);
        colour_props(&mut m);
        files.put(mat, m.encode());
        lamp_secs += 1;
    }
    // ---- backlights: their own interior sections (alpha cut-outs kept),
    // lit through an emissive map = their colour map
    let mut back = 0;
    for s in plan.sections.iter().filter(|s| s.interior) {
        let col = s.parts.iter().find_map(|(ii, _)| lp.backlights.get(&insts[*ii].node.to_lowercase()).copied());
        let Some(col) = col else { continue };
        let Some((_, _, _, _, mat)) = written.iter().find(|w| w.0 == s.name) else { continue };
        let Some(mut m) = files.get(mat).and_then(|b| Msg::parse(b).ok()) else { continue };
        let h = fx::hue(col);
        // OURS: AC1 backlight colours are small numbers (0..10); 5 -> 1.0
        let k = (col[0].max(col[1]).max(col[2]) / 5.0).clamp(0.2, 2.0);
        set_prop(&mut m, "Base_EmissiveColor", &[h[0], h[1], h[2], 0.0]);
        set_prop(&mut m, "Base_EmissiveIntensity", &[k]);
        if let Some(t) = get_slot(&m, "Base_BaseColorMap") {
            set_slot(&mut m, "Base_EmissiveMap", &t);
            set_prop(&mut m, "Base_HasEmissiveMap", &[1.0]);
        }
        files.put(mat, m.encode());
        back += 1;
    }
    // ---- vertex colours of the lamp sections, every LOD
    let ep = format!("{base}\\meshes\\{stem}_exterior.mesh");
    let lamp_names: HashSet<&str> = sec_funcs.keys().map(|s| s.as_str()).collect();
    let mut lamp_pts: Vec<([f32; 3], BTreeSet<Func>)> = Vec::new();
    if let Some(mut top) = files.get(&ep).and_then(|b| Msg::parse(b).ok()) {
        for (li, i) in top.idx(5).into_iter().enumerate() {
            let lod = top.open(i)?;
            let pos = fx::stream(lod, 5);
            let nv = pos.len() / 3;
            let mut col = fx::stream(lod, 13);
            col.resize(nv * 4, 0.0);
            for (name, vs) in fx::section_vertices(lod) {
                if !lamp_names.contains(name.as_str()) {
                    continue;
                }
                for v in vs {
                    let v = v as usize;
                    if v >= nv {
                        continue;
                    }
                    let (x, z) = (pos[v * 3], pos[v * 3 + 2]);
                    let (l, r) = if x.abs() < 0.02 { (1.0, 1.0) } else if x > 0.0 { (1.0, 0.0) } else { (0.0, 1.0) };
                    let rear = if z < 0.0 { 1.0 } else { 0.0 };
                    col[v * 4..v * 4 + 4].copy_from_slice(&[l / 255.0, r / 255.0, rear / 255.0, 1.0 / 255.0]);
                    if li == 0 {
                        lamp_pts.push(([pos[v * 3], pos[v * 3 + 1], pos[v * 3 + 2]], sec_funcs[&name].clone()));
                    }
                }
            }
            fx::set_stream(lod, 13, &col);
        }
        files.put(&ep, top.encode());
    }
    // ---- lighting system + physical lights
    let ap = format!("{base}\\{id}.actor");
    let Some(mut actor) = files.get(&ap).and_then(|b| Msg::parse(b).ok()) else { return Ok(notes) };
    let car = actor.msg_mut(103)?;
    let ls_path = car.str(101).unwrap_or_else(|| format!("{base}\\instruments\\{id}.carlightingsystem"));
    let tmpl = files.get(&ls_path).and_then(|b| Msg::parse(b).ok()).unwrap_or_default();
    files.put(&ls_path, lighting_system(&tmpl, lp, popups).encode());
    car.set_str(101, &ls_path);
    // physical lights: front pair (low / high), rear pair (tail / brake), rain
    let mean = |pred: &dyn Fn(&[f32; 3], &BTreeSet<Func>) -> bool| -> Option<[f32; 3]> {
        let mut s = [0f64; 3];
        let mut n = 0.0;
        for (p, fs) in &lamp_pts {
            if pred(p, fs) {
                for k in 0..3 {
                    s[k] += p[k] as f64;
                }
                n += 1.0;
            }
        }
        (n > 0.0).then(|| [(s[0] / n) as f32, (s[1] / n) as f32, (s[2] / n) as f32])
    };
    let old: Vec<Msg> = car.subs(2);
    let rot_front = old.iter().find(|l| l.varint(6).unwrap_or(0) <= 1).and_then(|l| l.sub(3)).map(|t| fx::transform_of(&t).1).unwrap_or([-80.35, 0.0, 0.0]);
    let rot_rear = old.iter().find(|l| matches!(l.varint(6), Some(2) | Some(3))).and_then(|l| l.sub(3)).map(|t| fx::transform_of(&t).1).unwrap_or([116.5, 0.0, -180.0]);
    car.remove(2);
    let mut lights: Vec<Msg> = Vec::new();
    let mk = |name: &str, loc: u64, pos: [f32; 3], rot: [f32; 3], funcs: &[u64], colour: Option<[f32; 3]>| -> Msg {
        let mut l = Msg::default();
        l.set_str(1, name);
        for f in funcs {
            l.push(2, Val::Varint(*f));
        }
        l.push(3, Val::Msg(fx::transform(pos, rot)));
        if loc != 0 {
            l.set_varint(6, loc);
        }
        match colour {
            Some(c) => {
                l.push(7, Val::Msg(fx::vec_msg(&c)));
                l.set_varint(8, 1);
            }
            None => l.push(7, Val::Msg(Msg::default())),
        }
        l
    };
    let head = |p: &[f32; 3], fs: &BTreeSet<Func>| fs.contains(&Func::Low) && p[2] > 0.0;
    let tail = |p: &[f32; 3], fs: &BTreeSet<Func>| (fs.contains(&Func::Tail) || fs.contains(&Func::Brake)) && p[2] < 0.0;
    for (name, loc, left) in [("front left", 0u64, true), ("front right", 1, false)] {
        if let Some(c) = mean(&|p, fs| head(p, fs) && (p[0] > 0.0) == left) {
            let fun: Vec<u64> = [(Func::Low, 1u64), (Func::High, 2), (Func::Drl, 0)].iter().filter(|(f, _)| lp.has(*f)).map(|x| x.1).collect();
            lights.push(mk(name, loc, c, rot_front, &fun, Some(WARM)));
        }
    }
    for (name, loc, left) in [("rear left", 2u64, true), ("rear right", 3, false)] {
        if let Some(c) = mean(&|p, fs| tail(p, fs) && (p[0] > 0.0) == left) {
            let fun: Vec<u64> = [(Func::Tail, 3u64), (Func::Brake, 4)].iter().filter(|(f, _)| lp.has(*f)).map(|x| x.1).collect();
            lights.push(mk(name, loc, c, rot_rear, &fun, None));
        }
    }
    if let Some(c) = mean(&|p, fs| fs.contains(&Func::Fog) && p[2] < 0.0) {
        lights.push(mk("rear fog", 4, c, rot_rear, &[5], None));
    }
    let n_phys = lights.len();
    // stock field order: f2 lights after f1 cameras
    let at = car.fields.iter().position(|x| x.num > 2).unwrap_or(car.fields.len());
    for (k, l) in lights.into_iter().enumerate() {
        car.fields.insert(at + k, crate::io::pb::Field { num: 2, val: Val::Msg(l) });
    }
    files.put(&ap, actor.encode());
    notes.push(format!("lights: {lamp_secs} lamp section(s) with function masks, {back} backlight material(s), {n_phys} physical light(s), lighting system {}",
        ls_path.rsplit('\\').next().unwrap_or("")));
    Ok(notes)
}

/// The `.carlightingsystem`: the stock ks_mazda_mx5_na layout (3 switch
/// stages, flashing high beam, shared brake / reverse / indicator / hazard /
/// rain sets) with only the functions this car has lamps for. Unknown
/// fields of the template (the donor's) are kept.
fn lighting_system(tmpl: &Msg, lp: &LampPlan, popups: bool) -> Msg {
    let entry = |f: u64, on: f32, glass: f32, emitter: f32, blinker: Option<(&str, f32)>, popup: bool| -> Msg {
        let mut e = Msg::default();
        if f != 0 {
            e.set_varint(1, f);
        }
        e.set_f32(3, on);
        if let Some((_, t)) = blinker {
            e.set_varint(4, 1);
            e.set_f32(7, t);
            e.set_f32(9, t);
        }
        if emitter > 0.0 {
            e.set_f32(14, emitter);
        }
        e.set_f32(20, glass);
        if let Some((name, _)) = blinker {
            e.set_str(27, name);
        }
        if popup {
            e.set_varint(28, 1);
        }
        e
    };
    let set = |cat: u64, entries: Vec<Msg>, auto_stop: f32| -> Msg {
        let mut s = Msg::default();
        if cat != 0 {
            s.set_varint(1, cat);
        }
        for e in entries {
            s.push(3, Val::Msg(e));
        }
        if auto_stop > 0.0 {
            s.set_f32(4, auto_stop);
        }
        s
    };
    let stage = |sets: Vec<Msg>| -> Msg {
        let mut s = Msg::default();
        for x in sets {
            s.push(1, Val::Msg(x));
        }
        s
    };
    let pop = popups;
    let (low, high, tail, drl) = (lp.has(Func::Low), lp.has(Func::High), lp.has(Func::Tail), lp.has(Func::Drl));
    let flash = |on: bool| set(6, if high { vec![entry(2, 20.0, 0.6, 1.0, Some((if on { "ON_200ms_200ms" } else { "OFF_200ms_200ms" }, 0.2)), pop)] } else { vec![] }, 1.0);
    let mut stages = Vec::new();
    for st in 1..=3 {
        let mut e = Vec::new();
        if drl {
            e.push(entry(0, 1.0, 0.05, 0.1, None, false));
        }
        if st >= 2 && low {
            e.push(entry(1, 2.0, 0.1, 0.3, None, pop));
        }
        if st == 3 && high {
            e.push(entry(2, 20.0, 0.6, 1.0, None, pop));
        }
        if tail {
            e.push(entry(3, 3.0, 0.1, 0.1, None, false));
        }
        stages.push(stage(vec![set(2, e, 0.0), flash(st < 3)]));
    }
    let blink = Some(("ON_500ms_500ms", 0.5));
    let mut shared = Vec::new();
    if lp.has(Func::Brake) {
        shared.push(set(4, vec![entry(4, 20.0, 0.6, 1.0, None, false)], 0.0));
    }
    if lp.has(Func::Reverse) {
        shared.push(set(9, vec![entry(6, 6.0, 0.6, 0.0, None, false)], 0.0));
    }
    if lp.has(Func::Indicator) {
        // only the ends that have indicator lamps
        let (f, r) = (lp.ind_front || !lp.ind_rear, lp.ind_rear || !lp.ind_front);
        let side = |fr: u64, rr: u64| -> Vec<Msg> {
            let mut v = Vec::new();
            if f { v.push(entry(fr, 3.0, 0.3, 0.0, blink, false)); }
            if r { v.push(entry(rr, 20.0, 1.0, 0.0, blink, false)); }
            v
        };
        shared.push(set(7, side(7, 9), 0.0));
        shared.push(set(8, side(8, 10), 0.0));
        let mut all = side(7, 9);
        all.extend(side(8, 10));
        shared.push(set(10, all, 0.0));
    }
    if lp.has(Func::Fog) {
        shared.push(set(5, vec![entry(5, 20.0, 0.6, 1.0, None, false)], 0.0));
    }
    let mut out = Msg::default();
    for s in stages {
        out.push(1, Val::Msg(s));
    }
    out.push(2, Val::Msg(Msg::default()));
    // intensity filters: the template's, else the stock values
    match tmpl.get(7) {
        Some(v) => out.push(7, v.clone()),
        None => {
            let f: Vec<f32> = [30.0, 30.0, 30.0, 30.0, 50.0, 30.0, 30.0, 40.0, 40.0, 40.0, 40.0].iter().copied().chain(std::iter::repeat(0.0).take(8)).collect();
            out.push(7, Val::Bytes(crate::io::pb::f32_bytes(&f)));
        }
    }
    out.push(20, Val::Msg(stage(shared)));
    out.push(21, Val::Msg(Msg::default()));
    out.push(22, Val::Msg(Msg::default()));
    for (name, off, t) in [("ON_500ms_500ms", false, 0.5f32), ("ON_200ms_200ms", false, 0.2), ("OFF_200ms_200ms", true, 0.2)] {
        let mut b = Msg::default();
        b.set_str(1, name);
        if off {
            b.set_varint(2, 1);
        }
        b.set_f32(3, t);
        b.set_f32(4, t);
        out.push(23, Val::Msg(b));
    }
    for f in tmpl.fields.iter().filter(|f| ![1, 2, 5, 6, 7, 20, 21, 22, 23].contains(&f.num)) {
        out.fields.push(f.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_channels() {
        let s: BTreeSet<Func> = [Func::Low, Func::High].into_iter().collect();
        assert_eq!(masks(&s), ([255, 255, 0, 255], [0, 0, 0, 255]));
        let s: BTreeSet<Func> = [Func::Tail, Func::Brake].into_iter().collect();
        assert_eq!(masks(&s).0, [0, 255, 255, 255]);
        let s: BTreeSet<Func> = [Func::Indicator].into_iter().collect();
        assert_eq!(masks(&s).1, [0, 0, 255, 255]);
    }

    #[test]
    fn system_only_has_functions_with_lamps() {
        let mut lp = LampPlan::default();
        lp.funcs.insert("head".into(), [Func::Low, Func::High].into_iter().collect());
        let m = lighting_system(&Msg::default(), &lp, false);
        assert_eq!(m.subs(1).len(), 3);
        // no brake / indicator lamps: an empty shared stage
        assert!(m.sub(20).unwrap().subs(1).is_empty());
        assert_eq!(m.subs(23).len(), 3);
    }

    #[test]
    fn tokens_split_words() {
        assert_eq!(tokens("FrontLight_L01"), vec!["front", "light"]);
        assert_eq!(tokens("TURN_signal-RR2"), vec!["turn", "signal", "rr"]);
    }

    #[test]
    fn name_keywords_from_the_corpus() {
        assert_eq!(name_func("TURN_LF"), Some(Func::Indicator));
        assert_eq!(name_func("Reverse_L"), Some(Func::Reverse));
        assert_eq!(name_func("highbeam_R"), Some(Func::High));
        assert_eq!(name_func("lowbeam_R"), Some(Func::Low));
        assert_eq!(name_func("body_shell"), None);
    }

    #[test]
    fn low_beam_word_holding_a_high_beam_word_is_low() {
        // Italian: anabbaglianti = low beams, abbaglianti = high beams
        assert_ne!(name_func("FARI_ANABBAGLIANTI_L"), Some(Func::High));
        assert_ne!(name_func("anabbaglianti"), Some(Func::High));
        // both words, or a low-beam and a high-beam keyword: low
        assert_eq!(name_func("lowbeam_highbeam"), Some(Func::Low));
        assert_eq!(name_func("hb_lowbeam_L"), Some(Func::Low));
    }
}
