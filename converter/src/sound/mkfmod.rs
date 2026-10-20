//! `aceconv make-fmod-template <content.kspkg | game dir> <out dir>`: the FMOD Studio
//! template project the sound stage starts from, generated from the user's own Assetto
//! Corsa EVO install. The game's compiled banks are read back (`bankread`) and turned into
//! Studio XML (the inverse of `bankwrite`):
//!
//! * mixer - the buses the template events route to (and their ancestors, the reverb
//!   returns they send to, the VCAs on them, the snapshots that drive those returns),
//!   from content\sfx\master_bank.bank + its strings bank, with the game's GUIDs;
//! * parameters - every `parameter:/` path of the game's strings bank, with the
//!   definition (range, type, seek speed) from whichever game bank uses it;
//! * events - one stock event per role (engine_int/ext, turbo, backfire, gearbox,
//!   transmission, horn, indicators), decompiled with its samples into `<role>_template`.
//!
//! Objects that exist in the bank keep their bank GUID; Studio-only objects (effect
//! chains, faders, tracks, folders, presets...) get a GUID derived from a fixed seed, so
//! the output is the same on every run.

use anyhow::{anyhow, bail, Context, Result};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::bankread::{self as br, Bank, Effect, Instrument, PVal, Prop};
use super::bankwrite::compile::{C_SNAP, C_SNAPBUS};
use super::bankwrite::compile_event::{unmap, C_LOOPTR};
use super::bankwrite::effects::{self, fx, g, P};
use super::bankwrite::prj::Guid;

/// Bump when the generator's output changes (part of the cache stamp).
pub const GENERATOR_VERSION: u32 = 3;

/// The stock event behind each template event: (template base name, bank, event path, asset folder).
pub const ROLES: [(&str, &str, &str, &str); 8] = [
    ("engine_int", "ks_abarth_695_biposto", "event:/evo_cars/ks_abarth_695_biposto/engine_int", "evo_cars/engine_int"),
    ("engine_ext", "ks_abarth_695_biposto", "event:/evo_cars/ks_abarth_695_biposto/engine_ext", "evo_cars/engine_ext"),
    ("turbo", "turbo", "event:/parts/turbos/turbo_artura", "Car Events/Turbo"),
    ("backfire", "backfire", "event:/parts/backfires/backfire_abarth_695_biposto", "Car Events/Backfires"),
    ("gearbox", "gearbox", "event:/parts/gearboxes/gearbox_hpattern", "Car Events/Gearbox"),
    ("transmission", "transmission", "event:/parts/transmissions/transmission_generic", "Car Events/Transmission"),
    ("horn", "horn", "event:/parts/horns/horn_generic", "Car Events/Horns"),
    ("indicators", "indicators", "event:/parts/indicators/indicators_generic", "Car Events/Blinkers"),
];

// ------------------------------------------------------------------ game files
/// content.kspkg, or a game folder (loose content\sfx\*.bank first, else its content.kspkg).
pub enum Src {
    Pkg(crate::io::kspkg::Pkg),
    Dir(PathBuf),
}

impl Src {
    pub fn open(p: &Path) -> Result<Src> {
        if p.is_dir() {
            if p.join("content").join("sfx").join("master_bank.bank").is_file() {
                return Ok(Src::Dir(p.join("content").join("sfx")));
            }
            if p.join("master_bank.bank").is_file() {
                return Ok(Src::Dir(p.to_path_buf()));
            }
            let k = p.join("content.kspkg");
            if k.is_file() {
                return Ok(Src::Pkg(crate::io::kspkg::Pkg::open(&k)?));
            }
            bail!("{}: no content.kspkg and no content\\sfx\\master_bank.bank", p.display());
        }
        Ok(Src::Pkg(crate::io::kspkg::Pkg::open(p)?))
    }
    /// A bank by file name (`master_bank.bank`).
    pub fn bank(&self, name: &str) -> Result<Vec<u8>> {
        match self {
            Src::Pkg(k) => {
                let e = k.find(&format!("content\\sfx\\{name}")).ok_or_else(|| anyhow!("content.kspkg has no content\\sfx\\{name} (game update?)"))?;
                k.read(e)
            }
            Src::Dir(d) => std::fs::read(d.join(name)).with_context(|| format!("{}", d.join(name).display())),
        }
    }
    pub fn banks(&self) -> Vec<String> {
        let mut v: Vec<String> = match self {
            Src::Pkg(k) => k.entries.iter().filter_map(|e| {
                let l = e.path.to_lowercase();
                (l.starts_with("content\\sfx\\") && l.ends_with(".bank") && !l[12..].contains('\\')).then(|| e.path[12..].to_string())
            }).collect(),
            Src::Dir(d) => std::fs::read_dir(d).map(|r| r.flatten().filter_map(|e| e.file_name().to_str().map(String::from))
                .filter(|n| n.to_lowercase().ends_with(".bank")).collect()).unwrap_or_default(),
        };
        v.sort();
        v
    }
}

// ------------------------------------------------------------------ XML
#[derive(Clone)]
struct X {
    class: String,
    id: Guid,
    props: Vec<(String, Vec<String>)>,
    rels: Vec<(String, Vec<Guid>)>,
}

impl X {
    fn new(class: &str, id: Guid) -> X {
        X { class: class.to_string(), id, props: Vec::new(), rels: Vec::new() }
    }
    fn p(mut self, k: &str, v: impl Into<String>) -> X {
        self.props.push((k.to_string(), vec![v.into()]));
        self
    }
    fn pv(mut self, k: &str, v: Vec<String>) -> X {
        if !v.is_empty() {
            self.props.push((k.to_string(), v));
        }
        self
    }
    fn r(mut self, k: &str, ids: &[Guid]) -> X {
        if !ids.is_empty() {
            self.rels.push((k.to_string(), ids.to_vec()));
        }
        self
    }
    fn r1(self, k: &str, id: Guid) -> X {
        self.r(k, &[id])
    }
    fn push_rel(&mut self, k: &str, id: Guid) {
        match self.rels.iter_mut().find(|r| r.0 == k) {
            Some(r) => r.1.push(id),
            None => self.rels.push((k.to_string(), vec![id])),
        }
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn xml(objs: &[X]) -> String {
    let mut s = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<objects serializationModel=\"Studio.02.03.00\">\n");
    for o in objs {
        if o.props.is_empty() && o.rels.is_empty() {
            s += &format!("\t<object class=\"{}\" id=\"{}\" />\n", o.class, o.id);
            continue;
        }
        s += &format!("\t<object class=\"{}\" id=\"{}\">\n", o.class, o.id);
        for (k, vs) in &o.props {
            s += &format!("\t\t<property name=\"{k}\">\n");
            for v in vs {
                s += &format!("\t\t\t<value>{}</value>\n", esc(v));
            }
            s += "\t\t</property>\n";
        }
        for (k, ids) in &o.rels {
            s += &format!("\t\t<relationship name=\"{k}\">\n");
            for id in ids {
                s += &format!("\t\t\t<destination>{id}</destination>\n");
            }
            s += "\t\t</relationship>\n";
        }
        s += "\t</object>\n";
    }
    s += "</objects>\n";
    s
}

/// Shortest text that parses back to the same f32.
fn ff(v: f32) -> String {
    if v == v.trunc() && v.abs() < 1e9 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}
fn fd(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e12 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}
fn secs(samples: u32) -> f64 {
    samples as f64 / 48000.0
}

/// Deterministic GUID for a Studio-only object.
fn dg(seed: &str) -> Guid {
    use md5::{Digest, Md5};
    let mut h = Md5::new();
    h.update(b"aceconv-fmod-template\0");
    h.update(seed.as_bytes());
    let mut b: [u8; 16] = h.finalize().into();
    b[7] = (b[7] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    Guid(b)
}

fn rank(m: u32) -> u32 {
    match m {
        2 => 1,
        3 => 2,
        4 => 4,
        5 => 5,
        6 => 6,
        7 => 8,
        8 => 12,
        _ => 0,
    }
}
fn wider(a: u32, b: u32) -> u32 {
    if rank(b) > rank(a) { b } else { a }
}
/// FMOD speaker mode -> Studio channel-format enum.
fn studio_fmt(m: u32) -> i64 {
    match m {
        2 => 0,
        3 => 1,
        4 => 3,
        5 => 4,
        6 => 2,
        7 => 6,
        8 => 7,
        _ => 2,
    }
}

// ------------------------------------------------------------------ trigger code
#[derive(Clone, Debug)]
struct Term {
    gp: Guid,
    tag: u32,
    lo: f32,
    hi: f32,
}

/// Conditions joined with "any" (0x1102) = triggerConditionMode 1.
fn any_mode(code: &[u8]) -> bool {
    let mut r = br::R::new(code);
    while r.left() >= 4 {
        match r.u32().unwrap_or(0) {
            0x11 => {
                let _ = r.bytes(16 + 12);
            }
            0x1102 => return true,
            _ => {}
        }
    }
    false
}

fn terms(code: &[u8]) -> Result<Vec<Term>> {
    let mut r = br::R::new(code);
    let mut out = Vec::new();
    while r.left() >= 4 {
        match r.u32()? {
            0x11 => {
                let gp = r.guid()?;
                let tag = r.u32()?;
                if !matches!(tag, 0x5020 | 0x5120 | 0x5220 | 0x5320) {
                    bail!("trigger code range tag {tag:#x}");
                }
                out.push(Term { gp, tag, lo: r.f32()?, hi: r.f32()? });
            }
            0x1002 | 0x1102 | 0x30 => {}
            t => bail!("trigger code entry {t:#x}"),
        }
    }
    Ok(out)
}

// ------------------------------------------------------------------ parameters (all banks)
#[derive(Clone)]
pub struct ParamDef {
    pub p: br::Param,
    pub seek: Option<br::Modu>,
    pub bank: String,
    /// cursor automation (flags 16, type 0: the cursor follows other parameters): curves
    pub cursor: Vec<br::Curve>,
}

/// Bank paths of the template: the decompiled project plus the WAV assets.
pub struct Out {
    pub metadata: BTreeMap<String, String>,
    pub assets: BTreeMap<String, Vec<u8>>,
    pub notes: Vec<String>,
    pub events: Vec<(String, String)>,
}

struct Gen<'a> {
    params: &'a BTreeMap<Guid, ParamDef>,
    /// plugin identifier -> its parameters (type, name, data type) from the game's DLL
    plugins: HashMap<String, Vec<(i32, String, i32, String)>>,
    names: &'a br::Names,
    files: BTreeMap<String, Vec<X>>,
    assets: BTreeMap<String, Vec<u8>>,
    /// AudioFile id -> asset path
    audio: BTreeMap<Guid, String>,
    asset_dirs: BTreeSet<String>,
    /// (mixer bus fed by an event, event format)
    feeds: Vec<(Guid, u32)>,
    notes: Vec<String>,
    /// curve id -> driving GameParameter (from every bank's layouts)
    curve_param: HashMap<Guid, Guid>,
    params_used: BTreeSet<Guid>,
    ids: Ids,
    /// the Bank object's name (`master_bank_template` for the template)
    bank_name: String,
    /// event folder path -> folder id
    folders: BTreeMap<String, Guid>,
}

struct Ids {
    workspace: Guid,
    mixer: Guid,
    master_bus: Guid,
    snapshot_list: Guid,
    event_folder: Guid,
    bank_folder: Guid,
    asset_folder: Guid,
    tag_folder: Guid,
    fx_folder: Guid,
    param_folder: Guid,
    sandbox: Guid,
    profiler: Guid,
    platform: Guid,
    encoding: Guid,
    bank: Guid,
}

/// Studio property automated at (owner class, PROP index).
fn prop_name(class: &str, index: u32) -> Option<String> {
    Some(match (class, index) {
        ("SingleSound" | "MultiSound" | "SoundScatterer", 0) => "volume".into(),
        ("SingleSound" | "MultiSound" | "SoundScatterer", 1) => "pitch".into(),
        ("SingleSound" | "MultiSound" | "SoundScatterer", 11) => "startOffset".into(),
        ("EventMixerGroup" | "EventMixerMaster" | "EventMixerReturn" | "MixerGroup" | "MixerReturn" | "MixerMaster", 0) => "volume".into(),
        ("MixerSend", 0) => "level".into(),
        ("Event", 30) => "minimumDistance".into(),
        ("Event", 31) => "maximumDistance".into(),
        (c, i) if i >= 1000 => return effects::param_name(c, i),
        _ => return None,
    })
}

/// Inverse of the writer's unmap (value -> stored curve domain), exact in f32.
fn map_value(pts: &[(f32, f32)], y: f32) -> Option<f32> {
    let n = pts.len();
    let fwd = |y: f64| -> f64 {
        if y <= pts[0].0 as f64 {
            return pts[0].1 as f64;
        }
        for w in pts.windows(2) {
            let ((x0, y0), (x1, y1)) = ((w[0].0 as f64, w[0].1 as f64), (w[1].0 as f64, w[1].1 as f64));
            if y <= x1 {
                return if x1 == x0 { y1 } else { y0 + (y - x0) / (x1 - x0) * (y1 - y0) };
            }
        }
        let (x0, y0) = (pts[n - 2].0 as f64, pts[n - 2].1 as f64);
        let (x1, y1) = (pts[n - 1].0 as f64, pts[n - 1].1 as f64);
        y0 + (y - x0) / (x1 - x0) * (y1 - y0)
    };
    let v0 = fwd(y as f64) as f32;
    let step = |v: f32, up: bool| -> f32 {
        if v == 0.0 {
            return if up { f32::from_bits(1) } else { -f32::from_bits(1) };
        }
        let b = v.to_bits();
        let b = if (v > 0.0) == up { b + 1 } else { b - 1 };
        f32::from_bits(b)
    };
    let (mut a, mut b) = (v0, v0);
    for _ in 0..4096 {
        if unmap(pts, a) == y {
            return Some(a);
        }
        if unmap(pts, b) == y {
            return Some(b);
        }
        a = step(a, true);
        b = step(b, false);
    }
    None
}

impl<'a> Gen<'a> {
    fn note(&mut self, s: impl Into<String>) {
        self.notes.push(s.into());
    }

    fn add(&mut self, file: &str, objs: Vec<X>) {
        self.files.entry(file.to_string()).or_default().extend(objs);
    }

    /// Automators (curves + points) and modulators of an object from its PROP list.
    fn automation(&mut self, bk: &Bank, class: &str, owner: Guid, props: &[Prop], objs: &mut Vec<X>) -> (Vec<Guid>, Vec<Guid>) {
        let mut autos = Vec::new();
        let mut mods = Vec::new();
        for p in props {
            if p.curves.is_empty() && p.mods.is_empty() {
                continue; // snapshot-target placeholder
            }
            let Some(name) = prop_name(class, p.index) else {
                let pts: Vec<String> = p.curves.iter().filter_map(|c| bk.curves.get(c)).map(|c| format!("{:?} by {:?}", c.pts.iter().map(|x| (f32::from_bits(x.0), x.1)).collect::<Vec<_>>(),
                    self.curve_param.get(&c.id).and_then(|g| self.params.get(g)).map(|d| d.p.name.clone()))).collect();
                self.note(format!("{class} {owner}: automation of property index {} has no Studio name - dropped ({})", p.index, pts.join("; ")));
                continue;
            };
            if !p.curves.is_empty() {
                let aid = dg(&format!("automator:{owner}:{}", p.index));
                let map = effects::mapping(class, p.index);
                if let Some((_, _, mg, _)) = map {
                    if g(mg) != p.map {
                        self.note(format!("{class}.{name}: mapping {} differs from the writer's table", p.map));
                    }
                }
                let pts_map: Option<Vec<(f32, f32)>> = bk.maps.get(&p.map).cloned().or_else(|| map.map(|m| m.3.to_vec()));
                let mut cids = Vec::new();
                for c in &p.curves {
                    let Some(cv) = bk.curves.get(c) else {
                        self.note(format!("curve {c} missing"));
                        continue;
                    };
                    let Some(gp) = self.curve_param.get(c).copied() else {
                        self.note(format!("curve {c} ({class}.{name}): no parameter layout drives it - dropped"));
                        continue;
                    };
                    // a curve driven by the event's timeline (AC1 banks name it in their CTRL items) is no parameter
                    if !bk.timelines.contains_key(&gp) {
                        self.params_used.insert(gp);
                    }
                    let mut pids = Vec::new();
                    for (k, (x, y, shape, fl)) in cv.pts.iter().enumerate() {
                        let pid = dg(&format!("point:{c}:{k}"));
                        let val = match &pts_map {
                            Some(m) if m.len() >= 2 => map_value(m, *y).unwrap_or_else(|| {
                                self.notes.push(format!("curve {c}: no exact value for stored {y}"));
                                *y
                            }),
                            _ => *y,
                        };
                        let mut o = X::new("AutomationPoint", pid).p("position", ff(f32::from_bits(*x))).p("value", ff(val));
                        if *shape != 0.0 {
                            o = o.p("curveShape", ff(*shape));
                        }
                        if *fl == 1 {
                            o = o.p("isSCurve", "true");
                        }
                        objs.push(o);
                        pids.push(pid);
                    }
                    objs.push(X::new("AutomationCurve", *c).r1("parameter", gp).r("automationPoints", &pids));
                    cids.push(*c);
                }
                objs.push(X::new("Automator", aid).p("nameOfPropertyBeingAutomated", name.clone()).r("automationCurves", &cids));
                autos.push(aid);
            }
            for m in &p.mods {
                let Some(mo) = bk.mods.get(m) else { continue };
                let mut r = br::R::new(&mo.data);
                let f = |r: &mut br::R| r.f32().unwrap_or(0.0);
                let o = match mo.typ {
                    0 => {
                        let v: Vec<f32> = (0..11).map(|_| f(&mut r)).collect();
                        let mut o = X::new("ADSRModulator", *m).p("nameOfPropertyBeingModulated", name.clone());
                        let keys = [("initialValue", 0.0), ("peakValue", 0.0), ("sustainValue", 0.0), ("attackTime", 1000.0), ("holdTime", 1000.0),
                            ("decayTime", 1000.0), ("releaseTime", 1000.0), ("attackShape", 0.0), ("decayShape", 0.0), ("releaseShape", 0.0), ("finalValue", 0.0)];
                        for (i, (k, d)) in keys.iter().enumerate() {
                            if v[i] != *d {
                                o = o.p(k, ff(v[i]));
                            }
                        }
                        if mo.a != 1 || mo.b != 0 {
                            self.notes.push(format!("ADSR {m}: a/b {}/{}", mo.a, mo.b));
                        }
                        o
                    }
                    1 => {
                        if mo.a != 0 {
                            self.notes.push(format!("randomizer {m} on {name}: a={} (volume randomizers are not handled by the writer)", mo.a));
                        }
                        X::new("RandomizerModulator", *m).p("nameOfPropertyBeingModulated", name.clone()).p("amount", ff(f(&mut r)))
                    }
                    t => {
                        self.notes.push(format!("modulator {m} type {t} on {class}.{name} - dropped"));
                        continue;
                    }
                };
                objs.push(o);
                mods.push(*m);
            }
        }
        (autos, mods)
    }

    /// Effect object(s) + its EFFB (bypass); returns the effect id and its automators.
    fn effect(&mut self, bk: &Bank, e: &Effect, objs: &mut Vec<X>) -> (Guid, Vec<Guid>) {
        let r = self.effect_inner(bk, e, objs);
        let id = e.id();
        if e.props().iter().any(|p| !p.mods.is_empty()) {
            self.notes.push(format!("effect {id}: modulator dropped (the writer has none on effects)"));
        }
        if let Some((fl, v)) = bk.effb.get(&id) {
            if let Some(o) = objs.iter_mut().rev().find(|o| o.id == id) {
                let meter = o.class == "LoudnessMeter";
                if fl & 1 != 0 && !meter {
                    o.props.push(("bypass".into(), vec!["true".into()]));
                }
                if fl & 1 == 0 && meter {
                    self.notes.push(format!("loudness meter {id} not bypassed"));
                }
            }
            if fl & !1 != 0 || *v != [100.0, 0.0, -80.0, 0.0] {
                self.notes.push(format!("effect {id}: EFFB {fl} {v:?} (wet/dry) not modelled"));
            }
        }
        r
    }

    fn effect_inner(&mut self, bk: &Bank, e: &Effect, objs: &mut Vec<X>) -> (Guid, Vec<Guid>) {
        let id = e.id();
        match e {
            Effect::Builtin { dsp: 27, params, props, .. } => {
                let blend = match params.get(17) {
                    Some(PVal::F(v)) => *v,
                    _ => 0.0,
                };
                let class = if blend == 1.0 { "SpatialiserEffect" } else { "MixerBusPanner" };
                let mut o = pan_props(X::new(class, id), class, params);
                let (autos, _) = self.automation(bk, class, id, props, objs);
                o = o.r("automators", &autos);
                objs.push(o);
                (id, autos)
            }
            Effect::Builtin { dsp, params, props, .. } => {
                let Some(class) = effects::class_of_dsp(*dsp) else {
                    self.note(format!("effect {id}: DSP type {dsp} unknown - kept as a Gain placeholder"));
                    objs.push(X::new("GainEffect", id));
                    return (id, vec![]);
                };
                let f = fx(class).unwrap();
                let mut o = X::new(class, id);
                for (i, pd) in f.params.iter().enumerate() {
                    match (pd, params.get(i)) {
                        (P::F(n, _), Some(PVal::F(v))) if !n.is_empty() => o = o.p(n, ff(*v)),
                        (P::I(n, _), Some(PVal::I(v))) if !n.is_empty() => o = o.p(n, v.to_string()),
                        (P::B(n, _), Some(PVal::B(v))) if !n.is_empty() => o = o.p(n, v.to_string()),
                        (P::D(d), Some(PVal::D(v))) if v.as_slice() != *d => self.notes.push(format!("{class} {id}: data parameter {i} differs")),
                        (_, None) => self.notes.push(format!("{class} {id}: parameter {i} missing")),
                        _ => {}
                    }
                }
                let (autos, _) = self.automation(bk, class, id, props, objs);
                o = o.r("automators", &autos);
                objs.push(o);
                (id, autos)
            }
            Effect::Plugin { ident, params, props, .. } => {
                let pl = dg(&format!("plugin:{id}"));
                let mut pps = Vec::new();
                let desc = self.plugins.get(ident).cloned();
                if desc.as_ref().map(|d| d.len() != params.len()).unwrap_or(true) {
                    self.notes.push(format!("plugin {ident}: parameter names unknown (DLL not found or different) - fmodstudiocl will not accept it"));
                }
                for (i, pv) in params.iter().enumerate() {
                    let pid = dg(&format!("pluginparam:{id}:{i}"));
                    let d = desc.as_ref().and_then(|d| d.get(i)).filter(|_| desc.as_ref().map(|d| d.len()) == Some(params.len()));
                    let named = |x: X| match d {
                        Some((_, n, _, _)) => x.p("name", n.clone()),
                        None => x,
                    };
                    match pv {
                        PVal::F(v) => {
                            let mut x = named(X::new("FloatPluginParameter", pid)).p("value", ff(*v));
                            if let Some((_, _, _, c)) = d.filter(|d| !d.3.is_empty()) {
                                x = x.p("curve", c.clone());
                            }
                            objs.push(x)
                        }
                        PVal::D(dd) if dd.is_empty() => {
                            let mut x = named(X::new("DataPluginParameter", pid));
                            if let Some((_, _, dt, _)) = d {
                                x = x.p("dataType", dt.to_string());
                            }
                            objs.push(x)
                        }
                        other => {
                            self.notes.push(format!("plugin {ident}: parameter {i} {other:?} not representable"));
                            continue;
                        }
                    }
                    pps.push(pid);
                }
                objs.push(X::new("Plugin", pl).p("identifier", ident.clone()).r("pluginParameters", &pps));
                let (autos, _) = self.automation(bk, "PluginEffect", id, props, objs);
                objs.push(X::new("PluginEffect", id).r1("plugin", pl).r("automators", &autos));
                (id, autos)
            }
            Effect::Send { ret, level, props, .. } => {
                let (autos, _) = self.automation(bk, "MixerSend", id, props, objs);
                let mut o = X::new("MixerSend", id).r1("mixerReturn", *ret).r("automators", &autos);
                if *level != -80.0 {
                    o = o.p("level", ff(*level));
                }
                objs.push(o);
                (id, autos)
            }
        }
    }

    /// A bus object (+ chain, fader, panner, effects). `fin_inferred` = the input format the
    /// writer will infer; `ev_fmt` = the event format a spatialiser outputs. Returns the bus
    /// object (caller adds class-specific properties) and the automators living on its track.
    fn bus(&mut self, bk: &Bank, b: &br::Bus, class: &str, fin_inferred: u32, ev_fmt: u32, objs: &mut Vec<X>) -> (X, Vec<Guid>) {
        let mut track_autos = Vec::new();
        let mut post = b.post.clone();
        let mut panner: Option<Guid> = None;
        if let Some(last) = post.last() {
            if let Some(Effect::Builtin { dsp: 27, params, .. }) = bk.effects.get(last) {
                if !matches!(params.get(17), Some(PVal::F(v)) if *v == 1.0) {
                    panner = post.pop();
                }
            }
        }
        let mut chain = Vec::new();
        let mut cur = b.in_mode;
        for (k, e) in b.pre.iter().chain([Guid::ZERO].iter()).chain(post.iter()).enumerate() {
            if e.is_zero() {
                let fid = dg(&format!("fader:{}", b.id));
                objs.push(X::new("MixerBusFader", fid));
                chain.push(fid);
                continue;
            }
            let Some(ef) = bk.effects.get(e).cloned() else {
                self.note(format!("bus {}: effect {e} (#{k}) missing", b.id));
                continue;
            };
            if let Effect::Builtin { dsp, params, .. } = &ef {
                if *dsp == 27 && matches!(params.get(17), Some(PVal::F(v)) if *v == 1.0) {
                    cur = ev_fmt;
                }
                if *dsp == 33 {
                    if let Some(PVal::I(gr)) = params.first() {
                        cur = match gr {
                            1 => 2,
                            2 => 3,
                            3 => 4,
                            4 => 6,
                            5 => 7,
                            6 => 6,
                            _ => cur,
                        };
                    }
                }
            }
            let (id, autos) = self.effect(bk, &ef, objs);
            track_autos.extend(autos);
            chain.push(id);
        }
        let cid = dg(&format!("chain:{}", b.id));
        objs.push(X::new("MixerBusEffectChain", cid).r("effects", &chain));
        let pid = panner.unwrap_or_else(|| dg(&format!("panner:{}", b.id)));
        let mut po = match panner.and_then(|p| bk.effects.get(&p)) {
            Some(Effect::Builtin { params, .. }) => pan_props(X::new("MixerBusPanner", pid), "MixerBusPanner", params),
            _ => X::new("MixerBusPanner", pid),
        };
        if b.out_mode != cur {
            po = po.p("overridingOutputFormat", studio_fmt(b.out_mode).to_string());
        }
        objs.push(po);
        let (autos, bmods) = self.automation(bk, class, b.id, &b.props, objs);
        track_autos.extend(autos.iter().copied());
        let mut o = X::new(class, b.id);
        if b.in_mode != fin_inferred {
            o = o.p("overridingInputFormat", studio_fmt(b.in_mode).to_string());
        }
        if b.volume != 0.0 {
            o = o.p("volume", ff(b.volume));
        }
        o = o.r1("effectChain", cid).r1("panner", pid).r("automators", &autos).r("modulators", &bmods);
        (o, track_autos)
    }

    /// An instrument (and playlist members). `place` = Some((start, length)) for top-level ones.
    fn instrument(&mut self, bk: &Bank, id: Guid, place: Option<(f64, f64)>, conds: &[Term], sheet: Option<Guid>, objs: &mut Vec<X>,
        asset_dir: &str) -> Result<()> {
        let it = bk.instruments.get(&id).cloned().ok_or_else(|| anyhow!("instrument {id} missing"))?;
        let ins = it.inst();
        let class = match &it {
            Instrument::Wave { .. } => "SingleSound",
            Instrument::Multi { .. } => "MultiSound",
            Instrument::Scatter { .. } => "SoundScatterer",
        };
        let mut o = X::new(class, id);
        if let Some((s, l)) = place {
            o = o.p("start", fd(s)).p("length", fd(l));
        }
        if ins.volume != 0.0 {
            o = o.p("volume", ff(ins.volume));
        }
        if ins.pitch != 0.0 {
            o = o.p("pitch", ff(ins.pitch));
        }
        match ins.loops {
            0 => {}
            -1 => o = o.p("looping", "true"),
            n => self.notes.push(format!("{class} {id}: loop count {n} not representable")),
        }
        if ins.cutoff != 0 {
            o = o.p("isCutoff", "true");
        }
        if ins.prob != 100.0 {
            o = o.p("triggerProbabilityEnabled", "true").p("triggerProbability", ff(ins.prob));
        }
        if ins.dmin != 0.0 || ins.dmax != 0.0 {
            o = o.p("delayType", "0").p("minimumTimeDelay", ff(ins.dmin)).p("maximumTimeDelay", ff(ins.dmax));
        }
        if ins.max_voices != 0x7fff_ffff {
            o = o.p("maxVoices", ins.max_voices.to_string());
        }
        match ins.steal {
            0 => o = o.p("voiceStealing", "3"),
            2 => {}
            s => self.notes.push(format!("{class} {id}: stealing word {s}")),
        }
        if ins.tl_off != 0 {
            o = o.p("timelockedOffset", fd(secs(ins.tl_off)));
        }
        if ins.start_off != 0.0 {
            o = o.p("startOffset", ff(ins.start_off));
        }
        // property 4 = the module's fade-in / fade-out curves
        let (fade, rest): (Vec<Prop>, Vec<Prop>) = ins.props.iter().cloned().partition(|p| p.index == 4);
        for p in fade {
            for c in &p.curves {
                let Some(cv) = bk.curves.get(c) else { continue };
                if cv.pts.len() != 2 {
                    self.notes.push(format!("{class} {id}: fade curve with {} points", cv.pts.len()));
                    continue;
                }
                let fade_in = cv.pts[0].1 < cv.pts[1].1;
                let mut pids = Vec::new();
                for (k, (x, y, shape, fl)) in cv.pts.iter().enumerate() {
                    let pid = dg(&format!("fadepoint:{c}:{k}"));
                    let mut o = X::new("AutomationPoint", pid).p("position", ff(f32::from_bits(*x))).p("value", ff(*y));
                    if *shape != 0.0 {
                        o = o.p("curveShape", ff(*shape));
                    }
                    if *fl == 1 {
                        o = o.p("isSCurve", "true");
                    }
                    objs.push(o);
                    pids.push(pid);
                }
                objs.push(X::new("FadeCurve", *c).r1("startPoint", pids[0]).r1("endPoint", pids[1]));
                o = o.r1(if fade_in { "fadeInCurve" } else { "fadeOutCurve" }, *c);
            }
        }
        let (autos, mut mods) = self.automation(bk, class, id, &rest, objs);
        if !ins.ap_param.is_zero() {
            if Some(ins.ap_param) != sheet {
                self.notes.push(format!("{class} {id}: autopitch parameter is not its sheet parameter"));
            }
            let aid = dg(&format!("autopitch:{id}"));
            let mut a = X::new("AutopitchModulator", aid).p("nameOfPropertyBeingModulated", "pitch").p("root", ff(ins.ap_root));
            if ins.ap_min != 0.0 {
                a = a.p("pitchAtMinimum", ff(ins.ap_min));
            }
            objs.push(a);
            mods.insert(0, aid);
        }
        o = o.r("automators", &autos).r("modulators", &mods);
        let mut cids = Vec::new();
        for (k, t) in conds.iter().enumerate() {
            let cid = dg(&format!("cond:{id}:{k}"));
            cids.push(cid);
            objs.push(self.condition(cid, t));
        }
        if any_mode(&ins.code) {
            o = o.p("triggerConditionMode", "1");
        }
        o = o.r("triggerConditions", &cids);
        match &it {
            Instrument::Wave { audio, .. } => {
                self.audio_file(bk, *audio, asset_dir)?;
                o = o.r1("audioFile", *audio);
            }
            Instrument::Multi { mode, sounds, .. } => {
                if *mode != 2 {
                    o = o.p("playlistMode", mode.to_string());
                }
                let n = sounds.len().max(1) as f32;
                if sounds.iter().any(|s| s.1 != 100.0 / n) {
                    let mut pps = Vec::new();
                    for (s, w) in sounds.iter() {
                        let pid = dg(&format!("playpercentage:{id}:{s}"));
                        objs.push(X::new("PlayPercentage", pid).p("percentage", ff(*w)).r1("sound", *s));
                        pps.push(pid);
                    }
                    o = o.r("playPercentages", &pps);
                }
                let ids: Vec<Guid> = sounds.iter().map(|s| s.0).collect();
                for s in &ids {
                    self.instrument(bk, *s, None, &[], None, objs, asset_dir)?;
                }
                o = o.r("sounds", &ids);
            }
            Instrument::Scatter { poly, total, tmin, tmax, steal, rate, sound, .. } => {
                if *poly != 16 {
                    o = o.p("polyphony", poly.to_string());
                }
                if *total != 33 {
                    o = o.p("totalSounds", total.to_string());
                }
                let ms = |v: f32| -> String {
                    // writer: (ms / 1000) as f32
                    let x = v as f64 * 1000.0;
                    let x = [x, (x * 1e6).round() / 1e6, x.round()].into_iter().find(|c| (*c / 1000.0) as f32 == v).unwrap_or(x);
                    fd(x)
                };
                if *tmin != 0.5 {
                    o = o.p("minimumTimeBetweenSounds", ms(*tmin));
                }
                if *tmax != 1.0 {
                    o = o.p("maximumTimeBetweenSounds", ms(*tmax));
                }
                if *steal != 2 {
                    o = o.p("soundStealing", (*steal as i64 - 2).to_string());
                }
                if *rate != 100.0 {
                    o = o.p("spawnRate", ff(*rate));
                }
                o = o.p("minimumScatterDistance", "0").p("maximumScatterDistance", "0");
                self.instrument(bk, *sound, None, &[], None, objs, asset_dir)?;
                o = o.r1("sound", *sound);
            }
        }
        objs.push(o);
        Ok(())
    }

    /// Sheet region length. The writer marks the region's end exclusive (0x5220) iff
    /// (start + length) as f32 < the parameter maximum; Kunos' banks also have exclusive ends at the
    /// maximum. Then the end moves down one f32 step, which the writer encodes as exclusive.
    fn sheet_length(&mut self, ev: &str, inst: Guid, t: &Term) -> f64 {
        let gmax = self.params.get(&t.gp).map(|d| if d.p.flags & 8 != 0 { d.p.max + 1.0 } else { d.p.max }).unwrap_or(1.0);
        let excl = t.tag & 0x200 != 0;
        if excl == (t.hi < gmax) {
            return t.hi as f64 - t.lo as f64;
        }
        if excl {
            let hi = f32::from_bits(if t.hi > 0.0 { t.hi.to_bits() - 1 } else { t.hi.to_bits() + 1 });
            self.notes.push(format!("event {ev}: instrument {inst} region ends exclusively at the parameter maximum {} - end moved down one f32 step", t.hi));
            return hi as f64 - t.lo as f64;
        }
        self.notes.push(format!("event {ev}: instrument {inst} region [{}, {}] is inclusive below the parameter maximum - the writer makes it exclusive", t.lo, t.hi));
        t.hi as f64 - t.lo as f64
    }

    fn condition(&mut self, id: Guid, t: &Term) -> X {
        self.params_used.insert(t.gp);
        let discrete = self.params.get(&t.gp).map(|p| p.p.flags & 8 != 0).unwrap_or(false);
        let hi = if discrete && t.tag & 0x200 != 0 { t.hi - 1.0 } else { t.hi };
        let mut o = X::new("ParameterCondition", id).p("minimum", ff(t.lo)).p("maximum", ff(hi));
        if t.tag & 0x100 != 0 {
            o = o.p("isInverted", "true");
        }
        o.r1("parameter", t.gp)
    }

    fn audio_file(&mut self, bk: &Bank, af: Guid, dir: &str) -> Result<()> {
        if self.audio.contains_key(&af) {
            return Ok(());
        }
        let idx = *bk.waves.get(&af).ok_or_else(|| anyhow!("audio file {af}: no WAV entry"))? as usize;
        let s = bk.samples.get(idx).ok_or_else(|| anyhow!("audio file {af}: sample {idx} missing"))?;
        let mut path = format!("{dir}/{}.wav", s.name);
        let mut k = 2;
        while self.assets.contains_key(&path) {
            self.notes.push(format!("asset name {} used twice in {dir}", s.name));
            path = format!("{dir}/{}_{k}.wav", s.name);
            k += 1;
        }
        self.assets.insert(path.clone(), wav(s));
        let mut d = String::new();
        for seg in dir.split('/') {
            d += seg;
            d.push('/');
            self.asset_dirs.insert(d.clone());
        }
        let frames = s.pcm.len() as f64 / (2.0 * s.channels as f64);
        let o = X::new("AudioFile", af).p("assetPath", path.clone()).p("frequencyInKHz", fd(s.rate as f64 / 1000.0))
            .p("channelCount", s.channels.to_string()).p("length", fd(frames / s.rate as f64)).r1("masterAssetFolder", self.ids.asset_folder);
        self.add(&format!("AudioFile/{af}.xml"), vec![o]);
        self.audio.insert(af, path);
        Ok(())
    }

    // -------------------------------------------------------------- events
    fn event(&mut self, bk: &Bank, ev_id: Guid, name: &str, asset_dir: &str) -> Result<()> {
        let ev = bk.events.get(&ev_id).cloned().ok_or_else(|| anyhow!("event {ev_id} not in its bank"))?;
        let tl = bk.timelines.get(&ev.timeline).cloned().ok_or_else(|| anyhow!("event {name}: timeline missing"))?;
        let mut objs: Vec<X> = Vec::new();
        let ibus = bk.buses.get(&ev.mixer_input).cloned().ok_or_else(|| anyhow!("event {name}: mixer input bus missing"))?;
        let ev_fmt = ibus.in_mode;
        self.feeds.push((ibus.output, ev_fmt));
        let gps: Vec<Guid> = ev.pinst.iter().map(|p| p.xor(&ev.id)).collect();
        for gp in &gps {
            self.params_used.insert(*gp);
        }
        let mut busset: Vec<Guid> = ev.buses.clone();
        busset.push(ev.master_bus);
        // top-level instruments by bus
        let mut top: Vec<(Guid, Guid)> = bk.instruments.values().filter(|i| busset.contains(&i.inst().bus)).map(|i| (i.id(), i.inst().bus)).collect();
        top.sort();
        // formats: writer's inference per event bus
        let mut ifmt: HashMap<Guid, u32> = HashMap::new();
        for (i, b) in &top {
            let f = bk.instruments[i].inst().fmt;
            let e = ifmt.entry(*b).or_insert(2);
            *e = wider(*e, f);
        }
        let inferred = |b: &br::Bus| -> u32 {
            let mut m = ifmt.get(&b.id).copied().unwrap_or(2);
            for c in busset.iter().filter_map(|x| bk.buses.get(x)) {
                if c.id != ev.master_bus && (c.output == b.id || (b.id == ev.master_bus && c.output.is_zero())) && c.id != b.id {
                    m = wider(m, c.out_mode);
                }
            }
            m
        };
        // ---- mixer input
        let (mi, _) = self.bus(bk, &ibus, "MixerInput", ev_fmt, ev_fmt, &mut objs);
        let mi = mi.r1("output", ibus.output);
        objs.push(mi);
        // ---- buses + tracks
        let mixer_id = dg(&format!("eventmixer:{ev_id}"));
        let mbus = bk.buses.get(&ev.master_bus).cloned().ok_or_else(|| anyhow!("event {name}: master bus missing"))?;
        let (mo, mautos) = self.bus(bk, &mbus, "EventMixerMaster", inferred(&mbus), ev_fmt, &mut objs);
        objs.push(mo.r1("mixer", mixer_id));
        objs.push(X::new("EventMixer", mixer_id).r1("masterBus", ev.master_bus));
        let mut tracks: Vec<(Guid, Guid, X)> = Vec::new(); // (bus, track id, track)
        let mt = dg(&format!("mastertrack:{ev_id}"));
        tracks.push((ev.master_bus, mt, X::new("MasterTrack", mt).r1("mixerGroup", ev.master_bus)));
        let mut track_autos: Vec<(Guid, Vec<Guid>)> = vec![(ev.master_bus, mautos)];
        let mut group_tracks = Vec::new();
        let mut return_tracks = Vec::new();
        for b in &ev.buses {
            let bus = bk.buses.get(b).cloned().ok_or_else(|| anyhow!("event {name}: bus {b} missing"))?;
            let ret = &bus.list == b"RBSS";
            let class = if ret { "EventMixerReturn" } else { "EventMixerGroup" };
            let (o, autos) = self.bus(bk, &bus, class, inferred(&bus), ev_fmt, &mut objs);
            let out = if bus.output.is_zero() { ev.master_bus } else { bus.output };
            objs.push(o.r1("output", out));
            let tid = dg(&format!("track:{b}"));
            let t = if ret { X::new("ReturnTrack", tid).r1("mixerReturn", *b) } else { X::new("GroupTrack", tid).r1("mixerGroup", *b) };
            tracks.push((*b, tid, t));
            if ret { return_tracks.push(tid) } else { group_tracks.push(tid) }
            track_autos.push((*b, autos));
        }
        // ---- instruments
        let mut sheet_mods: BTreeMap<Guid, Vec<Guid>> = BTreeMap::new();
        let mut tl_mods: Vec<Guid> = Vec::new();
        let tl_pos: HashMap<Guid, (u32, u32)> = tl.insts.iter().map(|(g, s, l)| (*g, (*s, *l))).collect();
        for (i, b) in &top {
            let ins = bk.instruments[i].inst().clone();
            let ts = terms(&ins.code).with_context(|| format!("instrument {i}"))?;
            let (place, conds, sheet) = if ins.timeline == ev.timeline && !ins.timeline.is_zero() {
                let (s, l) = tl_pos.get(i).copied().unwrap_or((0, 0));
                tl_mods.push(*i);
                (Some((secs(s), secs(l))), ts, None)
            } else if let Some(first) = ts.first().cloned() {
                if !gps.contains(&first.gp) {
                    self.note(format!("event {name}: instrument {i} sits on a parameter the event has no sheet for"));
                }
                sheet_mods.entry(first.gp).or_default().push(*i);
                let len = self.sheet_length(name, *i, &first);
                (Some((first.lo as f64, len)), ts[1..].to_vec(), Some(first.gp))
            } else {
                self.note(format!("event {name}: instrument {i} has neither a timeline nor a sheet - put on the timeline at 0"));
                tl_mods.push(*i);
                (Some((0.0, 0.0)), ts, None)
            };
            self.instrument(bk, *i, place, &conds, sheet, &mut objs, asset_dir)?;
            if let Some(t) = tracks.iter_mut().find(|t| t.0 == *b) {
                t.2.push_rel("modules", *i);
            }
        }
        // ---- markers, regions, transitions
        let markers_by: HashMap<Guid, &br::Marker> = tl.markers.iter().map(|m| (m.id, m)).collect();
        let loop_tr = g(C_LOOPTR);
        // marker-track order: merge the marker list and the transition list (loops are in both)
        let mseq: Vec<Guid> = tl.markers.iter().map(|m| m.id).collect();
        let tseq: Vec<Guid> = tl.trans.iter().map(|t| if markers_by.contains_key(&t.dest) && t.id == t.dest.xor(&loop_tr) { t.dest } else { t.id }).collect();
        let order = merge(&mseq, &tseq).unwrap_or_else(|| {
            self.notes.push(format!("event {name}: marker and transition orders disagree - transition priority may change"));
            let mut v = mseq.clone();
            v.extend(tseq.iter().filter(|x| !mseq.contains(x)));
            v
        });
        let mut marker_tracks = Vec::new();
        let mut mt_of: HashMap<Guid, Guid> = HashMap::new();
        for (k, m) in order.iter().enumerate() {
            let t = dg(&format!("markertrack:{ev_id}:{k}"));
            marker_tracks.push(t);
            mt_of.insert(*m, t);
            objs.push(X::new("MarkerTrack", t));
        }
        if marker_tracks.is_empty() {
            let t = dg(&format!("markertrack:{ev_id}:0"));
            marker_tracks.push(t);
            objs.push(X::new("MarkerTrack", t));
        }
        let mut loop_x: HashMap<Guid, X> = HashMap::new();
        for m in &tl.markers {
            if m.len > 0 {
                loop_x.insert(m.id, X::new("LoopRegion", m.id).p("position", fd(secs(m.pos))).p("length", fd(secs(m.len))));
            } else {
                objs.push(X::new("NamedMarker", m.id).p("position", fd(secs(m.pos))).p("name", m.name.clone()).r1("timeline", ev.timeline)
                    .r1("markerTrack", mt_of[&m.id]));
            }
        }
        for t in &tl.trans {
            let conds = terms(&t.code)?;
            let mut cids = Vec::new();
            for (k, c) in conds.iter().enumerate() {
                let cid = dg(&format!("cond:{}:{k}", t.id));
                cids.push(cid);
                let x = self.condition(cid, c);
                objs.push(x);
            }
            let tt = match &t.trtl {
                Some(tt) => Some(self.transition_timeline(bk, t.id, tt, &mut objs, &mut tracks)),
                None => None,
            };
            let is_loop = markers_by.contains_key(&t.dest) && t.id == t.dest.xor(&loop_tr);
            if is_loop {
                let Some(mut x) = loop_x.remove(&t.dest) else { continue };
                x = x.r("triggerConditions", &cids);
                if any_mode(&t.code) {
                    x = x.p("triggerConditionMode", "1");
                }
                if let Some(tt) = tt {
                    x = x.r1("transitionTimeline", tt);
                }
                if (t.flags == 2) == tt.is_some() {
                    self.notes.push(format!("event {name}: loop-back flags {} vs transition timeline", t.flags));
                }
                loop_x.insert(t.dest, x);
                continue;
            }
            let mut x = match t.flags {
                0 => X::new("TransitionRegion", t.id).p("position", fd(secs(t.start))).p("length", fd(secs(t.end - t.start))),
                1 => X::new("TransitionMarker", t.id).p("position", fd(secs(t.start))),
                f => {
                    self.notes.push(format!("event {name}: transition flags {f}"));
                    continue;
                }
            };
            x = x.r1("timeline", ev.timeline).r1("markerTrack", mt_of.get(&t.id).copied().unwrap_or(marker_tracks[0])).r1("destination", t.dest)
                .r("triggerConditions", &cids);
            if any_mode(&t.code) {
                x = x.p("triggerConditionMode", "1");
            }
            if let Some(tt) = tt {
                x = x.r1("transitionTimeline", tt);
            }
            objs.push(x);
        }
        // in marker order (the map would make the output order vary run to run)
        for m in &tl.markers {
            if let Some(x) = loop_x.remove(&m.id) {
                objs.push(x.r1("timeline", ev.timeline).r1("markerTrack", mt_of.get(&m.id).copied().unwrap_or(marker_tracks[0])));
            }
        }
        // ---- event properties
        let ap = dg(&format!("props:{ev_id}"));
        let mut apo = X::new("EventAutomatableProperties", ap);
        if ev.flags & 2 != 0 {
            apo = apo.p("isPersistent", "true");
        }
        if ev.flags & 0x100 != 0 {
            apo = apo.p("dopplerEnabled", "true");
            if ev.doppler != 100.0 {
                apo = apo.p("dopplerScale", ff(ev.doppler));
            }
        }
        let known = 0x1441 | 0x20 | 2 | 0x100;
        if ev.flags & !known != 0 {
            self.notes.push(format!("event {name}: flags {:#x} beyond the writer's (0x{:x})", ev.flags, ev.flags & !known));
        }
        if ev.min_dist != 1.0 {
            apo = apo.p("minimumDistance", ff(ev.min_dist));
        }
        if ev.max_dist != 20.0 {
            apo = apo.p("maximumDistance", ff(ev.max_dist));
        }
        if ev.max_voices != 0x7fff_ffff {
            apo = apo.p("maxVoices", ev.max_voices.to_string());
        }
        let prio = 4 - (ev.priority / 64) as i64;
        if prio != 2 {
            apo = apo.p("priority", prio.to_string());
        }
        let vs = match (ev.steal, ev.steal_flag) {
            (2, 0) => 0,
            (3, 0) => 1,
            (1, 0) => 2,
            (0, 1) => 3,
            (4, 0) => 4,
            (s, f) => {
                self.notes.push(format!("event {name}: stealing {s}/{f}"));
                0
            }
        };
        if vs != 0 {
            apo = apo.p("voiceStealing", vs.to_string());
        }
        if ev.sched == 3 {
            apo = apo.p("noLookahead", "true");
        }
        if !ev.user_props.is_empty() {
            self.notes.push(format!("event {name}: user properties {:?} not carried", ev.user_props));
        }
        let (eautos, _) = self.automation(bk, "Event", ev_id, &ev.props, &mut objs);
        apo = apo.r("automators", &eautos);
        objs.push(apo);
        track_autos[0].1.extend(eautos);
        // automation tracks on the owning tracks
        for (b, autos) in &track_autos {
            if let Some(t) = tracks.iter_mut().find(|t| t.0 == *b) {
                for a in autos {
                    let at = dg(&format!("automationtrack:{a}"));
                    objs.push(X::new("AutomationTrack", at).r1("automator", *a));
                    t.2.push_rel("automationTracks", at);
                }
            }
        }
        // ---- sheets
        let mut proxies = Vec::new();
        for gp in &gps {
            let pid = dg(&format!("proxy:{ev_id}:{gp}"));
            proxies.push(pid);
            objs.push(X::new("ParameterProxy", pid).r("modules", sheet_mods.get(gp).map(|v| v.as_slice()).unwrap_or(&[])).r1("preset", *gp));
        }
        // the timeline lists its markers / regions (Studio's ownership: without it they are unreachable from the Event)
        objs.push(X::new("Timeline", ev.timeline).r("modules", &tl_mods).r("markers", &order));
        for t in tracks {
            objs.push(t.2);
        }
        let (folder, leaf) = self.event_folder(name);
        let mut e = X::new("Event", ev_id).p("name", leaf);
        if ev_fmt != 2 {
            e = e.p("outputFormat", studio_fmt(ev_fmt).to_string());
        }
        e = e.r1("folder", folder).r1("mixer", mixer_id).r1("masterTrack", mt).r1("mixerInput", ev.mixer_input)
            .r1("automatableProperties", ap).r("markerTracks", &marker_tracks).r("groupTracks", &group_tracks).r("returnTracks", &return_tracks)
            .r1("timeline", ev.timeline).r("parameters", &proxies).r1("banks", self.ids.bank);
        objs.insert(0, e);
        self.add(&format!("Event/{ev_id}.xml"), objs);
        Ok(())
    }

    /// `a/b/name` -> (the folder `a/b`, created on first use, "name"); a plain name sits in the master folder.
    fn event_folder(&mut self, path: &str) -> (Guid, String) {
        let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let Some((leaf, dirs)) = segs.split_last() else { return (self.ids.event_folder, path.to_string()) };
        let mut parent = self.ids.event_folder;
        let mut acc = String::new();
        for d in dirs {
            acc += d;
            acc.push('/');
            parent = match self.folders.get(&acc) {
                Some(f) => *f,
                None => {
                    let f = dg(&format!("eventfolder:{acc}"));
                    self.folders.insert(acc.clone(), f);
                    self.add(&format!("EventFolder/{f}.xml"), vec![X::new("EventFolder", f).p("name", *d).r1("folder", parent)]);
                    f
                }
            };
        }
        (parent, leaf.to_string())
    }

    fn transition_timeline(&mut self, bk: &Bank, tran: Guid, tt: &br::Trtl, objs: &mut Vec<X>, tracks: &mut [(Guid, Guid, X)]) -> Guid {
        let id = dg(&format!("transitiontimeline:{tran}"));
        let mut mods = Vec::new();
        for (src, list, len) in [(true, &tt.src, tt.src_len), (false, &tt.dst, tt.dst_len)] {
            for (bus, snd) in list.iter() {
                let (class, crel, ccls) = if src {
                    ("TransitionSourceSound", "fadeOutCurve", "TransitionSourceFadeOutCurve")
                } else {
                    ("TransitionDestinationSound", "fadeInCurve", "TransitionDestinationFadeInCurve")
                };
                let mut o = X::new(class, *snd);
                let pts = bk.curves.get(snd).map(|c| c.pts.clone()).unwrap_or_default();
                // always an explicit fade curve (absolute sample positions): Studio's implicit ramp is not
                // the writer's for very short sounds; each sound gets the longest length (the writer uses the max)
                {
                    o = o.p("length", fd(secs(len)));
                    let cid = dg(&format!("fade:{snd}"));
                    let mut pp = Vec::new();
                    for (k, (x, y, sh, _)) in pts.iter().enumerate().take(2) {
                        let pid = dg(&format!("fadepoint:{snd}:{k}"));
                        let mut p = X::new("AutomationPoint", pid).p("position", fd(secs(*x))).p("value", ff(*y));
                        if *sh != 0.0 {
                            p = p.p("curveShape", ff(*sh));
                        }
                        objs.push(p);
                        pp.push(pid);
                    }
                    let mut c = X::new(ccls, cid);
                    if let Some(a) = pp.first() {
                        c = c.r1("startPoint", *a);
                    }
                    if let Some(b) = pp.get(1) {
                        c = c.r1("endPoint", *b);
                    }
                    objs.push(c);
                    o = o.r1(crel, cid);
                }
                objs.push(o);
                mods.push(*snd);
                if let Some(t) = tracks.iter_mut().find(|t| t.0 == *bus) {
                    t.2.push_rel("modules", *snd);
                }
            }
        }
        objs.push(X::new("TransitionTimeline", id).p("length", fd(secs(tt.length))).r("modules", &mods));
        id
    }

    // -------------------------------------------------------------- mixer
    fn mixer(&mut self, mb: &Bank) -> Result<()> {
        let master = self.names.by_guid.iter().find(|(_, p)| p.as_str() == "bus:/").map(|(g, _)| *g)
            .ok_or_else(|| anyhow!("master strings bank has no bus:/"))?;
        // buses the events feed, their ancestors, the returns their sends reach
        let mut inc: BTreeSet<Guid> = BTreeSet::new();
        let mut todo: Vec<Guid> = self.feeds.iter().map(|f| f.0).collect();
        while let Some(b) = todo.pop() {
            if b.is_zero() || !inc.insert(b) {
                continue;
            }
            let Some(bus) = mb.buses.get(&b) else {
                self.notes.push(format!("mixer bus {b} ({}) not in master_bank", self.names.path(&b).unwrap_or("?")));
                continue;
            };
            todo.push(bus.output);
            for e in bus.pre.iter().chain(bus.post.iter()) {
                if let Some(Effect::Send { ret, .. }) = mb.effects.get(e) {
                    todo.push(*ret);
                }
            }
        }
        inc.insert(master);
        // VCAs on them
        let vcas: BTreeSet<Guid> = inc.iter().filter_map(|b| mb.buses.get(b)).flat_map(|b| b.vcas.iter().copied()).collect();
        // snapshots whose targets are all in the included mixer
        let mut owned: HashSet<Guid> = HashSet::new();
        for b in &inc {
            if let Some(bus) = mb.buses.get(b) {
                owned.insert(*b);
                owned.extend(bus.pre.iter().chain(bus.post.iter()).copied());
            }
        }
        let snaps: Vec<&br::Snap> = mb.snaps.values().filter(|s| s.props.iter().all(|p| owned.contains(&p.1))).collect();
        // formats: inputs = event feeds + child buses
        let mut feeds: HashMap<Guid, u32> = HashMap::new();
        for (b, f) in &self.feeds {
            let e = feeds.entry(*b).or_insert(2);
            *e = wider(*e, *f);
        }
        let infer = |id: Guid| -> u32 {
            let mut m = feeds.get(&id).copied().unwrap_or(2);
            for c in inc.iter().filter_map(|x| mb.buses.get(x)) {
                if c.output == id {
                    m = wider(m, c.out_mode);
                }
            }
            m
        };
        for b in inc.iter().copied().collect::<Vec<_>>() {
            let Some(bus) = mb.buses.get(&b).cloned() else { continue };
            let mut objs = Vec::new();
            let path = self.names.path(&b).map(String::from);
            if b == master {
                let (mut o, _) = self.bus(mb, &bus, "MixerMaster", infer(b), bus.in_mode, &mut objs);
                o = o.p("name", "Master Bus");
                if bus.steal != 2 {
                    o = o.p("instanceStealing", bus.steal.to_string());
                }
                if bus.max_inst != 0x7fff_ffff {
                    o = o.p("maxInstances", bus.max_inst.to_string());
                }
                objs.insert(0, o.r1("mixer", self.ids.mixer));
                self.ids.master_bus = b;
                self.add("Master.xml", objs);
                continue;
            }
            let ret = &bus.list == b"RBSS";
            let class = if ret { "MixerReturn" } else { "MixerGroup" };
            let (mut o, _) = self.bus(mb, &bus, class, infer(b), bus.in_mode, &mut objs);
            let name = path.as_deref().and_then(|p| p.rsplit('/').next()).unwrap_or("bus").to_string();
            if path.is_none() {
                self.notes.push(format!("mixer bus {b} has no path in the strings bank"));
            }
            o = o.p("name", name);
            if bus.max_inst != 0x7fff_ffff {
                o = o.p("maxInstances", bus.max_inst.to_string());
            }
            o = o.r("masters", &bus.vcas).r1("output", bus.output);
            objs.insert(0, o);
            self.add(&format!("{}/{b}.xml", if ret { "Return" } else { "Group" }), objs);
        }
        for v in &vcas {
            let Some(vc) = mb.vcas.get(v) else { continue };
            let name = self.names.path(v).and_then(|p| p.strip_prefix("vca:/")).unwrap_or("vca").to_string();
            let mut o = X::new("MixerVCA", *v).p("name", name);
            if vc.volume != 0.0 {
                o = o.p("volume", ff(vc.volume));
            }
            self.add(&format!("VCA/{v}.xml"), vec![o.r1("mixer", self.ids.mixer)]);
        }
        // snapshots: the snapshot event carries the timeline / sheets; SNAB the property values
        let mut order: Vec<(u32, Guid)> = Vec::new();
        for s in snaps {
            let sid = s.id.xor(&g(C_SNAP));
            let Some(ev) = mb.events.get(&sid) else {
                self.notes.push(format!("snapshot {sid}: no snapshot event"));
                continue;
            };
            if ev.master_bus != sid.xor(&g(C_SNAPBUS)) {
                self.notes.push(format!("snapshot {sid}: master bus id is not derived"));
            }
            order.push((s.priority, sid));
            let name = self.names.path(&sid).and_then(|p| p.strip_prefix("snapshot:/")).unwrap_or("snapshot").to_string();
            let mut objs = Vec::new();
            let ap = dg(&format!("props:{sid}"));
            objs.push(X::new("EventAutomatableProperties", ap));
            let mk = dg(&format!("markertrack:{sid}:0"));
            objs.push(X::new("MarkerTrack", mk));
            objs.push(X::new("Timeline", ev.timeline));
            let mut proxies = Vec::new();
            for p in &ev.pinst {
                let gp = p.xor(&sid);
                self.params_used.insert(gp);
                let pid = dg(&format!("proxy:{sid}:{gp}"));
                objs.push(X::new("ParameterProxy", pid).r1("preset", gp));
                proxies.push(pid);
            }
            let smt = dg(&format!("snapshotmastertrack:{sid}"));
            objs.push(X::new("SnapshotMasterTrack", smt));
            let mut sps = Vec::new();
            for (k, (_key, target, idx, value)) in s.props.iter().enumerate() {
                let tclass = match mb.effects.get(target) {
                    Some(Effect::Builtin { dsp: 27, params, .. }) => {
                        if matches!(params.get(17), Some(PVal::F(v)) if *v == 1.0) { "SpatialiserEffect" } else { "MixerBusPanner" }
                    }
                    Some(Effect::Builtin { dsp, .. }) => effects::class_of_dsp(*dsp).unwrap_or("?"),
                    Some(Effect::Send { .. }) => "MixerSend",
                    _ => "?",
                };
                let Some(pn) = effects::param_name(tclass, *idx) else {
                    self.notes.push(format!("snapshot {name}: property {idx} of {tclass} {target} has no name - dropped"));
                    continue;
                };
                let spid = dg(&format!("snapprop:{sid}:{k}"));
                objs.push(X::new("SnapshotProperty", spid).p("propertyName", pn).p("value", ff(*value)).r1("automatableObject", *target));
                sps.push(spid);
            }
            let mut so = X::new("Snapshot", sid).p("name", name.clone());
            match (s.blend, s.behavior) {
                (1, 3) => so = so.p("behavior", "1"),
                (b, h) => self.notes.push(format!("snapshot {name}: blend/behaviour {b}/{h} not representable")),
            }
            if s.intensity != 100.0 {
                so = so.p("intensity", ff(s.intensity));
            }
            so = so.r1("mixer", self.ids.mixer).r1("automatableProperties", ap).r1("markerTracks", mk).r1("timeline", ev.timeline)
                .r("parameters", &proxies).r1("snapshotMasterTrack", smt).r("snapshotProperties", &sps);
            objs.insert(0, so);
            self.add(&format!("Snapshot/{sid}.xml"), objs);
        }
        order.sort_by(|a, b| b.0.cmp(&a.0));
        let items: Vec<Guid> = order.iter().map(|x| x.1).collect();
        self.add(&format!("SnapshotGroup/{}.xml", self.ids.snapshot_list), vec![X::new("SnapshotList", self.ids.snapshot_list).r("items", &items)
            .r1("mixer", self.ids.mixer)]);
        let mx = X::new("Mixer", self.ids.mixer).r1("masterBus", master).r1("snapshotList", self.ids.snapshot_list);
        self.add("Mixer.xml", vec![mx]);
        Ok(())
    }

    // -------------------------------------------------------------- parameters
    fn parameters(&mut self) {
        let mut folders: BTreeMap<String, Guid> = BTreeMap::new();
        let mut paths: Vec<(String, Guid)> =
            self.names.by_guid.iter().filter_map(|(g, p)| p.strip_prefix("parameter:/").map(|s| (s.to_string(), *g))).collect();
        paths.sort();
        let used: Vec<Guid> = self.params_used.iter().copied().collect();
        for gp in used {
            if !paths.iter().any(|p| p.1 == gp) {
                let name = self.params.get(&gp).map(|d| d.p.name.clone()).unwrap_or_else(|| format!("param_{gp}"));
                self.notes.push(format!("parameter {name} {gp} is used but has no strings path - put at the top level"));
                paths.push((name, gp));
            }
        }
        for (path, gp) in paths {
            let mut parent = self.ids.param_folder;
            let segs: Vec<&str> = path.split('/').collect();
            let mut acc = String::new();
            for s in &segs[..segs.len() - 1] {
                acc += s;
                acc.push('/');
                let fid = match folders.get(&acc) {
                    Some(f) => *f,
                    None => {
                        let f = dg(&format!("paramfolder:{acc}"));
                        folders.insert(acc.clone(), f);
                        self.add(&format!("ParameterPresetFolder/{f}.xml"), vec![X::new("ParameterPresetFolder", f).p("name", *s).r1("folder", parent)]);
                        f
                    }
                };
                parent = fid;
            }
            // the strings trie merges paths case-insensitively (first label wins): prefer the PRMB name
            let name = match self.params.get(&gp) {
                Some(d) if d.p.name.eq_ignore_ascii_case(segs[segs.len() - 1]) => d.p.name.as_str(),
                _ => segs[segs.len() - 1],
            };
            let pr = dg(&format!("preset:{gp}"));
            let mut objs = vec![X::new("ParameterPreset", pr).p("name", name).r1("folder", parent).r1("parameter", gp)];
            let mut o = X::new("GameParameter", gp);
            match self.params.get(&gp) {
                Some(d) => {
                    let p = &d.p;
                    let ptype = if p.flags & 16 != 0 {
                        match p.ptype {
                            0 if !d.cursor.is_empty() => 0, // automatic: cursor driven by other parameters
                            1 => 3,
                            2 => 6,
                            t => {
                                self.notes.push(format!("parameter {name}: built-in type {t} not handled by the writer"));
                                3
                            }
                        }
                    } else if p.flags & 32 != 0 {
                        2
                    } else if p.flags & 8 != 0 {
                        1
                    } else {
                        0
                    };
                    if ptype != 0 {
                        o = o.p("parameterType", ptype.to_string());
                    }
                    let max = if p.flags & 8 != 0 { p.max + 1.0 } else { p.max };
                    // explicit: Studio's initial value defaults to the minimum, the writer's to 0
                    o = o.p("minimum", ff(p.min)).p("maximum", ff(max)).p("initialValue", ff(p.init));
                    if p.flags & 1 != 0 {
                        o = o.p("isGlobal", "true");
                    }
                    o = o.pv("enumerationLabels", p.labels.clone());
                    if let Some(sk) = &d.seek {
                        let mut r = br::R::new(&sk.data);
                        let asym = r.u32().unwrap_or(0) != 0;
                        let up = r.f32().unwrap_or(0.0);
                        let dn = r.f32().unwrap_or(0.0);
                        let mut s = X::new("SeekModulator", sk.id).p("nameOfPropertyBeingModulated", "cursorPosition").p("clockSource", sk.b.to_string());
                        s = s.p("speedAscending", ff(up)).p("speedDescending", ff(dn)).p("asymmetric", asym.to_string());
                        objs.push(s);
                        o = o.r1("modulators", sk.id);
                    }
                    if !d.cursor.is_empty() {
                        let aid = dg(&format!("automator:{gp}:cursor"));
                        let mut cids = Vec::new();
                        for cv in &d.cursor {
                            let Some(drv) = self.curve_param.get(&cv.id).copied() else {
                                self.notes.push(format!("parameter {name}: cursor curve {} has no driving parameter", cv.id));
                                continue;
                            };
                            let mut pids = Vec::new();
                            for (k, (x, y, shape, fl)) in cv.pts.iter().enumerate() {
                                let pid = dg(&format!("point:{}:{k}", cv.id));
                                let v = map_value(&[(0.0, p.min), (1.0, p.max)], *y).unwrap_or(*y);
                                let mut pt = X::new("AutomationPoint", pid).p("position", ff(f32::from_bits(*x))).p("value", ff(v));
                                if *shape != 0.0 {
                                    pt = pt.p("curveShape", ff(*shape));
                                }
                                if *fl == 1 {
                                    pt = pt.p("isSCurve", "true");
                                }
                                objs.push(pt);
                                pids.push(pid);
                            }
                            objs.push(X::new("AutomationCurve", cv.id).r1("parameter", drv).r("automationPoints", &pids));
                            cids.push(cv.id);
                        }
                        objs.push(X::new("Automator", aid).p("nameOfPropertyBeingAutomated", "cursorPosition").r("automationCurves", &cids));
                        o = o.r1("automators", aid);
                    }
                    if p.name != name {
                        self.notes.push(format!("parameter {path}: bank name {}", p.name));
                    }
                }
                None => {
                    if self.params_used.contains(&gp) {
                        self.notes.push(format!("parameter {path}: used but defined in no game bank - default 0..1"));
                    }
                }
            }
            objs.insert(1, o);
            self.add(&format!("ParameterPreset/{pr}.xml"), objs);
        }
        self.add(&format!("ParameterPresetFolder/{}.xml", self.ids.param_folder), vec![X::new("MasterParameterPresetFolder", self.ids.param_folder)]);
    }

    fn workspace(&mut self) {
        let i = &self.ids;
        let ws = X::new("Workspace", i.workspace).p("builtBanksOutputDirectory", "build").r1("masterEventFolder", i.event_folder)
            .r1("masterTagFolder", i.tag_folder).r1("masterEffectPresetFolder", i.fx_folder).r1("masterParameterPresetFolder", i.param_folder)
            .r1("masterBankFolder", i.bank_folder).r1("masterSandboxFolder", i.sandbox).r1("masterAssetFolder", i.asset_folder).r1("mixer", i.mixer)
            .r1("profilerSessionFolder", i.profiler).r1("platforms", i.platform);
        let (ef, bf, af, tf, xf, sb, pf, pl, en, bank) = (i.event_folder, i.bank_folder, i.asset_folder, i.tag_folder, i.fx_folder, i.sandbox, i.profiler,
            i.platform, i.encoding, i.bank);
        self.add("Workspace.xml", vec![ws]);
        self.add(&format!("EventFolder/{ef}.xml"), vec![X::new("MasterEventFolder", ef).p("name", "Master")]);
        self.add(&format!("BankFolder/{bf}.xml"), vec![X::new("MasterBankFolder", bf)]);
        self.add("Tags.xml", vec![X::new("MasterTagFolder", tf).p("name", "Master")]);
        self.add(&format!("EffectPresetFolder/{xf}.xml"), vec![X::new("MasterEffectPresetFolder", xf)]);
        self.add(&format!("SandboxFolder/{sb}.xml"), vec![X::new("MasterSandboxFolder", sb)]);
        self.add(&format!("ProfilerFolder/{pf}.xml"), vec![X::new("ProfilerSessionFolder", pf)]);
        self.add(&format!("Platform/{pl}.xml"), vec![X::new("Platform", pl).p("hardwareType", "0").p("name", "Desktop").p("subDirectory", "sfx")
            .p("speakerFormat", "5")]);
        self.add(&format!("EncodingSetting/{en}.xml"), vec![X::new("EncodingSetting", en).p("quality", "37").p("sampleRateMode", "2")
            .p("sampleRate", "44100").r1("platform", pl).r1("encodable", pl)]);
        self.add(&format!("Bank/{bank}.xml"), vec![X::new("Bank", bank).p("name", self.bank_name.clone()).p("isMasterBank", "true").r1("folder", bf)]);
        let mut assets = vec![(format!("Asset/{af}.xml"), X::new("MasterAssetFolder", af).p("streamingAssetThresholdLength", "-1"))];
        for d in &self.asset_dirs {
            let id = dg(&format!("assetdir:{d}"));
            assets.push((format!("Asset/{id}.xml"), X::new("EncodableAsset", id).p("assetPath", d.clone()).r1("masterAssetFolder", af)));
        }
        for (f, x) in assets {
            self.add(&f, vec![x]);
        }
    }
}

/// Pan DSP parameters -> MixerBusPanner / SpatialiserEffect properties (non-default only).
fn pan_props(mut o: X, class: &str, params: &[PVal]) -> X {
    let defs: [(usize, f32); 13] = [(1, 0.0), (2, 0.0), (3, 360.0), (4, 0.0), (5, 0.0), (7, 60.0), (8, 0.0), (12, 1.0), (13, 20.0), (15, 0.0), (16, 0.0),
        (6, 1.0), (11, 0.0)];
    for (i, d) in defs {
        let v = match params.get(i) {
            Some(PVal::F(v)) => *v,
            Some(PVal::I(v)) => *v as f32,
            _ => continue,
        };
        // written even at the writer's default: Studio's own defaults are not always the same
        let _ = d;
        if let Some(n) = effects::param_name(class, 1000 + i as u32) {
            o = o.p(&n, if matches!(params.get(i), Some(PVal::I(_))) { (v as i64).to_string() } else { ff(v) });
        }
    }
    if let Some(PVal::I(v)) = params.get(14) {
        o = o.p("extentMode", v.to_string());
    }
    if let Some(PVal::I(v)) = params.get(18) {
        o = o.p("LFEUpmixEnabled", (*v != 0).to_string());
    }
    if class == "SpatialiserEffect" {
        if let Some(PVal::B(b)) = params.get(23) {
            o = o.p("overrideRange", b.to_string());
        }
    }
    o
}

/// Parameter descriptions of a DSP plugin DLL (FMODGetDSPDescription): (type, name, data type).
/// Studio matches a placed plugin's parameters by name, so the template carries them.
fn plugin_params(dll: &Path) -> Option<Vec<(i32, String, i32, String)>> {
    unsafe {
        let lib = libloading::Library::new(dll).ok()?;
        let f: libloading::Symbol<unsafe extern "system" fn() -> *const u8> = lib.get(b"FMODGetDSPDescription").ok()?;
        let d = f();
        if d.is_null() {
            return None;
        }
        // FMOD_DSP_DESCRIPTION (x64): numparameters @96, paramdesc (FMOD_DSP_PARAMETER_DESC**) @104
        let n = std::ptr::read_unaligned(d.add(96) as *const i32);
        let pd = std::ptr::read_unaligned(d.add(104) as *const *const *const u8);
        if !(0..=64).contains(&n) || pd.is_null() {
            return None;
        }
        let mut out = Vec::new();
        for i in 0..n as usize {
            let p = *pd.add(i);
            if p.is_null() {
                return None;
            }
            // FMOD_DSP_PARAMETER_DESC: type @0, name[16] @4, data type (DATA union) @48
            let ty = std::ptr::read_unaligned(p as *const i32);
            let nm = std::slice::from_raw_parts(p.add(4), 16);
            let nm = String::from_utf8_lossy(&nm[..nm.iter().position(|&c| c == 0).unwrap_or(16)]).into_owned();
            let dt = if ty == 3 { std::ptr::read_unaligned(p.add(48) as *const i32) } else { 0 };
            // float: min @48, max @52, default @56, mapping (8-aligned) type @64 (2 = piecewise linear:
            // numpoints @72, values* @80, positions* @88) -> Studio's "curve" string
            let mut curve = String::new();
            let (mn, mx) = (std::ptr::read_unaligned(p.add(48) as *const f32), std::ptr::read_unaligned(p.add(52) as *const f32));
            let mt = if ty == 0 { std::ptr::read_unaligned(p.add(64) as *const i32) } else { -1 };
            if mt == 1 {
                // AUTO mapping: Studio derives the curve itself; this is the one it makes for 10..22000 Hz
                let pts: &[(f32, f32)] = match (mn, mx) {
                    (10.0, 22000.0) => &[(0.0, 10.0), (1.0, 100.0), (2.0, 400.0), (3.0, 750.0), (4.0, 1250.0), (5.0, 2000.0), (6.0, 3200.0), (7.0, 5500.0),
                        (8.0, 10000.0), (9.0, 18000.0), (10.0, 22000.0)],
                    _ => &[],
                };
                if !pts.is_empty() {
                    curve = format!("LinearPiecewiseCurveStrategy{{{}}}", pts.iter().map(|(a, b)| format!("({a:.6},{b:.6})")).collect::<String>());
                }
            }
            if mt == 2 {
                let np = std::ptr::read_unaligned(p.add(72) as *const i32);
                let vals = std::ptr::read_unaligned(p.add(80) as *const *const f32);
                let pos = std::ptr::read_unaligned(p.add(88) as *const *const f32);
                if (1..=64).contains(&np) && !vals.is_null() && !pos.is_null() {
                    curve = "LinearPiecewiseCurveStrategy{".to_string();
                    for k in 0..np as usize {
                        curve += &format!("({:.6},{:.6})", *pos.add(k), *vals.add(k));
                    }
                    curve += "}";
                }
            }
            out.push((ty, nm, dt, curve));
        }
        Some(out)
    }
}

/// Merge two orderings that share some elements into one order consistent with both.
fn merge(a: &[Guid], b: &[Guid]) -> Option<Vec<Guid>> {
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    let in_b: HashSet<Guid> = b.iter().copied().collect();
    let in_a: HashSet<Guid> = a.iter().copied().collect();
    while i < a.len() || j < b.len() {
        if i < a.len() && !in_b.contains(&a[i]) {
            out.push(a[i]);
            i += 1;
        } else if j < b.len() && !in_a.contains(&b[j]) {
            out.push(b[j]);
            j += 1;
        } else if i < a.len() && j < b.len() && a[i] == b[j] {
            out.push(a[i]);
            i += 1;
            j += 1;
        } else {
            return None;
        }
    }
    Some(out)
}

/// PCM16 WAV with cue points / labels for the sample's sync points.
fn wav(s: &br::Sample) -> Vec<u8> {
    let mut b = super::fsb5::wav(&s.pcm, s.channels, s.rate);
    if s.sync.is_empty() {
        return b;
    }
    let mut cue = Vec::new();
    cue.extend_from_slice(&(s.sync.len() as u32).to_le_bytes());
    for (k, (off, _)) in s.sync.iter().enumerate() {
        for v in [k as u32 + 1, *off] {
            cue.extend_from_slice(&v.to_le_bytes());
        }
        cue.extend_from_slice(b"data");
        for v in [0u32, 0, *off] {
            cue.extend_from_slice(&v.to_le_bytes());
        }
    }
    b.extend_from_slice(b"cue ");
    b.extend_from_slice(&(cue.len() as u32).to_le_bytes());
    b.extend_from_slice(&cue);
    if s.sync.iter().any(|x| !x.1.is_empty()) {
        let mut l = b"adtl".to_vec();
        for (k, (_, n)) in s.sync.iter().enumerate() {
            let mut body = (k as u32 + 1).to_le_bytes().to_vec();
            body.extend_from_slice(n.as_bytes());
            body.push(0);
            l.extend_from_slice(b"labl");
            l.extend_from_slice(&(body.len() as u32).to_le_bytes());
            l.extend_from_slice(&body);
            if body.len() & 1 == 1 {
                l.push(0);
            }
        }
        b.extend_from_slice(b"LIST");
        b.extend_from_slice(&(l.len() as u32).to_le_bytes());
        b.extend_from_slice(&l);
    }
    let n = b.len() as u32 - 8;
    b[4..8].copy_from_slice(&n.to_le_bytes());
    b
}

/// The game is built for a 7.1 platform, the template (and the in-house writer) for 5.1: platform-surround formats (7.1) become 5.1, as Studio would build them there.
fn platform_51(bk: &mut Bank) -> bool {
    if std::env::var("ACECONV_FMOD_KEEP_PLATFORM").as_deref() == Ok("1") {
        return false; // verification against the game banks
    }
    let mut hit = false;
    for b in bk.buses.values_mut() {
        for m in [&mut b.in_mode, &mut b.out_mode] {
            if *m == 7 {
                *m = 6;
                hit = true;
            }
        }
    }
    hit
}

/// Every parameter definition (PRMB + seek modulator) in the given banks; first one wins.
/// Also: curve id -> driving parameter from every layout.
fn scan_params(src: &Src, banks: &[String], notes: &mut Vec<String>) -> Result<(BTreeMap<Guid, ParamDef>, HashMap<Guid, Guid>)> {
    let mut out: BTreeMap<Guid, ParamDef> = BTreeMap::new();
    let mut cp = HashMap::new();
    for name in banks {
        let data = src.bank(name)?;
        if name.to_lowercase().ends_with(".strings.bank") {
            continue;
        }
        let bk = match br::read_meta(&data) {
            Ok(b) => b,
            Err(e) => {
                notes.push(format!("{name}: {e:#}"));
                continue;
            }
        };
        for l in bk.layouts.values() {
            for c in &l.ctrls {
                cp.entry(*c).or_insert(l.gp);
            }
        }
        for (id, p) in bk.params {
            let seek = p.props.iter().find(|x| x.index == 2).and_then(|x| x.mods.first()).and_then(|m| bk.mods.get(m)).filter(|m| m.typ == 4).cloned();
            match out.get(&id) {
                Some(d) => {
                    if d.p.min != p.min || d.p.max != p.max || d.p.flags != p.flags {
                        notes.push(format!("parameter {} differs between {} and {name}", p.name, d.bank));
                    }
                }
                None => {
                    let cursor = p.props.iter().filter(|x| x.index == 2).flat_map(|x| x.curves.iter()).filter_map(|c| bk.curves.get(c).cloned()).collect();
                    out.insert(id, ParamDef { p, seek, bank: name.clone(), cursor });
                }
            }
        }
    }
    Ok((out, cp))
}

/// Find an event by path in a strings table.
fn event_guid(names: &[(String, Guid)], path: &str) -> Option<Guid> {
    names.iter().find(|(p, _)| p.eq_ignore_ascii_case(path)).map(|x| x.1)
}

/// Decompile: the mixer of master_bank + `events` [(bank file stem, event path, template name, asset dir)].
pub fn generate(src: &Src, events: &[(String, String, String, String)], game: Option<&Path>) -> Result<Out> {
    let mut notes = Vec::new();
    let mut master = br::read_meta(&src.bank("master_bank.bank")?).context("master_bank.bank")?;
    if platform_51(&mut master) {
        notes.push("game banks are built for a 7.1 platform; the template uses 5.1 (as the in-house writer does)".into());
    }
    let strings = br::strings(&src.bank("master_bank.strings.bank")?).context("master_bank.strings.bank")?;
    let names = br::Names::new(&strings);
    let all = src.banks();
    let (params, curve_param) = scan_params(src, &all, &mut notes)?;
    let ids = Ids {
        workspace: dg("workspace"),
        mixer: dg("mixer"),
        master_bus: Guid::ZERO,
        snapshot_list: dg("snapshotlist"),
        event_folder: dg("eventfolder"),
        bank_folder: dg("bankfolder"),
        asset_folder: dg("assetfolder"),
        tag_folder: dg("tagfolder"),
        fx_folder: dg("effectpresetfolder"),
        param_folder: dg("parameterpresetfolder"),
        sandbox: dg("sandboxfolder"),
        profiler: dg("profilerfolder"),
        platform: master.platform.unwrap_or_else(|| dg("platform")),
        encoding: dg("encodingsetting"),
        bank: master.id,
    };
    // the DSP plugins the events may use (parameter names for Studio)
    let mut plugins: HashMap<String, Vec<(i32, String, i32, String)>> = HashMap::new();
    if let Some(p) = game.and_then(|g| plugin_params(&g.join("fmod_distance_filter.dll"))) {
        plugins.insert("FMOD Distance Filter".into(), p);
    }
    let mut gn = Gen { params: &params, names: &names, plugins, files: BTreeMap::new(), assets: BTreeMap::new(), audio: BTreeMap::new(),
        asset_dirs: BTreeSet::new(), feeds: Vec::new(), notes, curve_param, params_used: BTreeSet::new(), ids, bank_name: "master_bank_template".into(),
        folders: BTreeMap::new() };
    let mut banks: HashMap<String, Bank> = HashMap::new();
    let mut evs_out = Vec::new();
    for (bank, path, name, dir) in events {
        if !banks.contains_key(bank) {
            let mut b = br::read(&src.bank(&format!("{bank}.bank"))?).with_context(|| format!("{bank}.bank"))?;
            platform_51(&mut b);
            gn.notes.extend(b.notes.iter().map(|n| format!("{bank}: {n}")));
            banks.insert(bank.clone(), b);
        }
        let bk = &banks[bank];
        let id = event_guid(&strings, path).ok_or_else(|| anyhow!("the game's strings bank has no {path} (game update?)"))?;
        if !bk.events.contains_key(&id) {
            bail!("{path} is not in {bank}.bank");
        }
        gn.event(bk, id, name, dir).with_context(|| format!("decompile {path}"))?;
        evs_out.push((name.clone(), path.clone()));
    }
    gn.mixer(&master)?;
    gn.parameters();
    gn.workspace();
    let metadata = gn.files.iter().map(|(k, v)| (k.clone(), xml(v))).collect();
    Ok(Out { metadata, assets: gn.assets, notes: gn.notes, events: evs_out })
}

/// Events of a bank that is not one of the game's (an AC1 bank read into the 2.03 model by
/// `ac1bank`, already re-pointed at the game's parameters and buses), decompiled onto the game's
/// mixer and parameters.
pub struct Foreign<'a> {
    pub bank: &'a Bank,
    /// parameter definitions the game does not have (the foreign bank's own)
    pub params: BTreeMap<Guid, ParamDef>,
    /// curve -> driving parameter (foreign curves)
    pub curve_param: HashMap<Guid, Guid>,
    /// (event id, `folder/.../name` under event:/, asset folder)
    pub events: Vec<(Guid, String, String)>,
    pub bank_id: Guid,
    pub bank_name: String,
}

/// Game parameter definitions and their curve map, read once (all game banks).
pub struct GameParams {
    pub params: BTreeMap<Guid, ParamDef>,
    pub curve_param: HashMap<Guid, Guid>,
    pub notes: Vec<String>,
}

pub fn game_params(src: &Src) -> Result<GameParams> {
    let mut notes = Vec::new();
    let (params, curve_param) = scan_params(src, &src.banks(), &mut notes)?;
    Ok(GameParams { params, curve_param, notes })
}

pub fn generate_foreign(src: &Src, gp: &GameParams, f: &Foreign, game: Option<&Path>) -> Result<Out> {
    let mut notes = Vec::new();
    let mut master = br::read_meta(&src.bank("master_bank.bank")?).context("master_bank.bank")?;
    platform_51(&mut master);
    let strings = br::strings(&src.bank("master_bank.strings.bank")?).context("master_bank.strings.bank")?;
    let names = br::Names::new(&strings);
    let mut params = gp.params.clone();
    for (k, v) in &f.params {
        params.entry(*k).or_insert_with(|| ParamDef { p: v.p.clone(), seek: v.seek.clone(), bank: v.bank.clone(), cursor: v.cursor.clone() });
    }
    let mut curve_param = gp.curve_param.clone();
    curve_param.extend(f.curve_param.iter().map(|(a, b)| (*a, *b)));
    let ids = Ids {
        workspace: dg(&format!("workspace:{}", f.bank_id)),
        mixer: dg("mixer"),
        master_bus: Guid::ZERO,
        snapshot_list: dg("snapshotlist"),
        event_folder: dg("eventfolder"),
        bank_folder: dg("bankfolder"),
        asset_folder: dg("assetfolder"),
        tag_folder: dg("tagfolder"),
        fx_folder: dg("effectpresetfolder"),
        param_folder: dg("parameterpresetfolder"),
        sandbox: dg("sandboxfolder"),
        profiler: dg("profilerfolder"),
        platform: master.platform.unwrap_or_else(|| dg("platform")),
        encoding: dg("encodingsetting"),
        bank: f.bank_id,
    };
    let mut plugins: HashMap<String, Vec<(i32, String, i32, String)>> = HashMap::new();
    if let Some(p) = game.and_then(|g| plugin_params(&g.join("fmod_distance_filter.dll"))) {
        plugins.insert("FMOD Distance Filter".into(), p);
    }
    let mut gn = Gen { params: &params, names: &names, plugins, files: BTreeMap::new(), assets: BTreeMap::new(), audio: BTreeMap::new(),
        asset_dirs: BTreeSet::new(), feeds: Vec::new(), notes: Vec::new(), curve_param, params_used: BTreeSet::new(), ids, bank_name: f.bank_name.clone(),
        folders: BTreeMap::new() };
    let mut evs_out = Vec::new();
    for (id, path, dir) in &f.events {
        gn.event(f.bank, *id, path, dir).with_context(|| format!("decompile {path}"))?;
        evs_out.push((path.clone(), format!("event:/{path}")));
    }
    gn.mixer(&master)?;
    gn.parameters();
    gn.workspace();
    notes.extend(gn.notes);
    let metadata = gn.files.iter().map(|(k, v)| (k.clone(), xml(v))).collect();
    Ok(Out { metadata, assets: gn.assets, notes, events: evs_out })
}

/// Write a generated project to `dir` (replacing it).
pub fn write(out: &Out, dir: &Path, name: &str) -> Result<()> {
    if dir.exists() {
        std::fs::remove_dir_all(dir).with_context(|| format!("clear {}", dir.display()))?;
    }
    let meta = dir.join("metadata");
    std::fs::create_dir_all(&meta)?;
    std::fs::write(dir.join(format!("{name}.fspro")), "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<objects serializationModel=\"Studio.02.03.00\" />\n")?;
    for (k, v) in &out.metadata {
        let p = meta.join(k.replace('/', "\\"));
        std::fs::create_dir_all(p.parent().unwrap())?;
        std::fs::write(&p, v)?;
    }
    for (k, v) in &out.assets {
        let mut p = dir.join("assets");
        for seg in k.split('/') {
            p.push(seg);
        }
        std::fs::create_dir_all(p.parent().unwrap())?;
        std::fs::write(&p, v)?;
    }
    let mut notes = out.notes.clone();
    notes.sort();
    notes.dedup();
    std::fs::write(dir.join("generator_notes.txt"), notes.join("\n") + "\n")?;
    Ok(())
}

/// The template the converter uses: the eight role events.
pub fn make(content: &Path, dir: &Path) -> Result<Out> {
    let src = Src::open(content)?;
    let evs: Vec<(String, String, String, String)> =
        ROLES.iter().map(|(n, b, p, d)| (b.to_string(), p.to_string(), format!("{n}_template"), d.to_string())).collect();
    // fmodstudiocl (the fallback builder) needs the DSP plugin the events use: the game's own copy
    let game = if content.is_dir() { content.to_path_buf() } else { content.parent().map(Path::to_path_buf).unwrap_or_default() };
    let mut out = generate(&src, &evs, Some(&game))?;
    let plugin = game.join("fmod_distance_filter.dll");
    let plugin_dll = std::fs::read(&plugin).ok();
    if plugin_dll.is_none() {
        out.notes.push(format!("{} not found: fmodstudiocl cannot build the distance filter (the in-house writer does not need it)", plugin.display()));
    }
    write(&out, dir, "fmod_evo_template")?;
    if let Some(b) = plugin_dll {
        std::fs::create_dir_all(dir.join("Plugins"))?;
        std::fs::write(dir.join("Plugins").join("fmod_distance_filter.dll"), b)?;
    }
    std::fs::write(dir.join("stamp.txt"), stamp(content)?)?;
    Ok(out)
}

/// Every event of one game bank (debug / round-trip checks).
pub fn decompile_bank(content: &Path, bank: &str, dir: &Path, filter: &str) -> Result<Out> {
    let src = Src::open(content)?;
    let strings = br::strings(&src.bank("master_bank.strings.bank")?)?;
    let bk = br::read_meta(&src.bank(&format!("{bank}.bank"))?)?;
    let mut evs = Vec::new();
    for (p, gid) in &strings {
        if p.starts_with("event:/") && bk.events.contains_key(gid) && p.contains(filter) {
            let name = p.rsplit('/').next().unwrap_or("event").to_string();
            evs.push((bank.to_string(), p.clone(), name, format!("{bank}/{}", p[7..].replace('/', "_"))));
        }
    }
    let game = if content.is_dir() { Some(content.to_path_buf()) } else { content.parent().map(Path::to_path_buf) };
    let out = generate(&src, &evs, game.as_deref())?;
    write(&out, dir, bank)?;
    Ok(out)
}

// ------------------------------------------------------------------ cache
/// Generator version + content.kspkg size and mtime.
pub fn stamp(content: &Path) -> Result<String> {
    let k = if content.is_dir() {
        [content.join("content.kspkg"), content.join("content").join("sfx").join("master_bank.bank"), content.join("master_bank.bank")]
            .into_iter().find(|p| p.is_file()).unwrap_or_else(|| content.join("content.kspkg"))
    } else {
        content.to_path_buf()
    };
    let m = std::fs::metadata(&k).with_context(|| format!("{}", k.display()))?;
    let t = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
    Ok(format!("fmod v{GENERATOR_VERSION} {} {t}\n", m.len()))
}

/// The generated template's folder: ACECONV_FMOD_DIR, else `fmod_template` beside the donor cache.
pub fn cache_dir() -> Result<PathBuf> {
    if let Some(d) = std::env::var_os("ACECONV_FMOD_DIR") {
        return Ok(PathBuf::from(d));
    }
    let donor = crate::donor::cache_dir()?;
    Ok(donor.parent().map(|p| p.join("fmod_template")).unwrap_or_else(|| donor.join("fmod_template")))
}

/// The template project dir, generated from `content` on first use and whenever content.kspkg changes.
pub fn ensure(content: &Path) -> Result<PathBuf> {
    // the batch converts cars on many threads: one builds, the others wait for it
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = cache_dir()?;
    let want = stamp(content)?;
    let fresh = std::fs::read_to_string(dir.join("stamp.txt")).map(|s| s == want).unwrap_or(false)
        && dir.join("metadata").join("Workspace.xml").is_file();
    if fresh {
        return Ok(dir);
    }
    if let Some(free) = crate::donor::free_bytes(&dir) {
        if free < 2 << 30 {
            bail!("only {} MB free for the generated FMOD template at {}; set ACECONV_FMOD_DIR to a folder on a roomier drive", free >> 20, dir.display());
        }
    }
    eprintln!("building the FMOD template from {} -> {}", content.display(), dir.display());
    let parent = dir.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let tmp = parent.join(format!("fmod_template.tmp-{}", std::process::id()));
    make(content, &tmp)?;
    let _ = std::fs::remove_dir_all(&dir);
    if std::fs::rename(&tmp, &dir).is_err() {
        let _ = std::fs::remove_dir_all(&tmp);
        if !std::fs::read_to_string(dir.join("stamp.txt")).map(|s| s == want).unwrap_or(false) {
            bail!("could not place the FMOD template in {}", dir.display());
        }
    }
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(n: u8) -> Guid {
        Guid([n; 16])
    }

    #[test]
    fn merge_keeps_both_orders() {
        // markers m1 L m2, transitions t1 L t2 (L = loop region, in both lists)
        let (m1, l, m2, t1, t2) = (g(1), g(2), g(3), g(4), g(5));
        let o = merge(&[m1, l, m2], &[t1, l, t2]).unwrap();
        let pos = |x: Guid| o.iter().position(|y| *y == x).unwrap();
        assert!(pos(m1) < pos(l) && pos(l) < pos(m2) && pos(t1) < pos(l) && pos(l) < pos(t2));
        assert!(merge(&[m1, l], &[l, m1]).is_none());
    }

    #[test]
    fn trigger_code_terms() {
        // sheet [0, 0.5) on P1, then P2 [0, 500] inverted, joined and closed as Studio does
        let mut c = Vec::new();
        for (gp, tag, lo, hi) in [(g(7), 0x5220u32, 0.0f32, 0.5f32), (g(8), 0x5120, 0.0, 500.0)] {
            if gp == g(8) {
                c.extend_from_slice(&0x1002u32.to_le_bytes());
            }
            c.extend_from_slice(&0x11u32.to_le_bytes());
            c.extend_from_slice(&gp.0);
            c.extend_from_slice(&tag.to_le_bytes());
            c.extend_from_slice(&lo.to_le_bytes());
            c.extend_from_slice(&hi.to_le_bytes());
        }
        c.extend_from_slice(&0x30u32.to_le_bytes());
        let t = terms(&c).unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!((t[1].tag, t[1].hi), (0x5120, 500.0));
        assert!(!any_mode(&c));
    }

    #[test]
    fn mapped_values_round_trip() {
        let fader: &[(f32, f32)] = &[(-42.0, -80.0), (-39.0, -60.0), (-36.0, -50.0), (-32.0, -40.0), (-27.0, -30.0), (-20.0, -20.0), (-10.0, -10.0),
            (0.0, 0.0), (10.0, 10.0)];
        for y in [-41.5f32, -33.25, -12.0, 0.0, 3.7] {
            let v = map_value(fader, y).unwrap();
            assert_eq!(unmap(fader, v), y);
        }
    }
}
