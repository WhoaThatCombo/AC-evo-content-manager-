//! AC1 car sound bank -> the events of an EVO car: which AC1 event feeds which EVO role,
//! the parameter translation (AC1 names / units -> the game's parameters), the bus routing
//! onto the game's mixer, and the restructuring of AC1's one-shot events into the parameter
//! sheets the game drives.
//!
//! Roles (the actor's audio slot type, the game event it replaces):
//! * engine_int / engine_ext / turbo / transmission: AC1 events of the same name, whole: their
//!   sheets, automation and effects are kept and re-pointed at the game's parameters;
//! * gearbox: AC1 `gear_int` (else `gear_ext`) - started once per shift in AC1, with a
//!   `state` sheet for down (0) / up (1). The game keeps one gearbox event running and pulses
//!   `gear_up` / `gear_dn`, so the sounds move onto those sheets;
//! * backfire: AC1 `backfire_ext` (else `backfire_int`) - a one-shot in AC1. The game drives a
//!   running event with `throttle_position`, so the pops sit on a throttle-lift region (OURS);
//! * horn: AC1 `horn` - started / stopped in AC1, held on `horn_sound` = 1 in the game.
//!
//! Every GUID that is not the game's is re-seeded from (car id, role, AC1 GUID), so two
//! conversions of one AC1 car under different ids never collide in FMOD.

use anyhow::{anyhow, bail, Context, Result};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use super::ac1bank::{self, Ac1Bank};
use super::bankread::{self as br, Bank, Curve, Effect, Instrument, Modu, Prop};
use super::bankwrite::prj::Guid;
use super::mkfmod::{self, ParamDef};

/// The AC1 car's bank and its event names.
pub struct Source {
    pub ac1: Ac1Bank,
    pub bank_path: PathBuf,
    /// event id -> AC1 path (event:/cars/<car>/engine_ext)
    pub names: HashMap<Guid, String>,
    /// every (path, event) GUIDs.txt gives for the bank's events (one event can carry two names)
    pub paths: Vec<(String, Guid)>,
}

/// `<car>\sfx\<car>.bank` (else the largest bank in sfx) and its GUIDs.txt (the car's own,
/// else the AC1 install's content\sfx\GUIDs.txt).
pub fn open_source(car_dir: &Path) -> Result<Source> {
    let sfx = crate::ac1::car::find_ci(car_dir, "sfx").filter(|p| p.is_dir()).ok_or_else(|| anyhow!("no sfx folder"))?;
    let folder = car_dir.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    // <car>.bank first, then the others by size: the first whose events GUIDs.txt names
    let mut cands: Vec<PathBuf> = std::fs::read_dir(&sfx).map(|r| r.flatten().map(|e| e.path()).filter(|p| {
        let n = p.file_name().map(|f| f.to_string_lossy().to_lowercase()).unwrap_or_default();
        n.ends_with(".bank") && !n.ends_with(".strings.bank")
    }).collect()).unwrap_or_default();
    cands.sort_by_key(|p| {
        let own = p.file_stem().map(|f| f.to_string_lossy().eq_ignore_ascii_case(&folder)).unwrap_or(false);
        (!own, std::cmp::Reverse(std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)), p.clone())
    });
    if cands.is_empty() {
        bail!("no .bank in {}", sfx.display());
    }
    let mut guids: Vec<(Guid, String)> = Vec::new();
    let shared = car_dir.parent().and_then(|p| p.parent()).map(|c| c.join("sfx").join("GUIDs.txt"));
    // a Kunos car copied out of the install still has its names in the
    // install's shared GUIDs.txt
    let install = crate::io::gamedir::ac1_dir().map(|d| d.join("content").join("sfx").join("GUIDs.txt")).filter(|g| Some(g) != shared.as_ref());
    for g in [crate::ac1::car::find_ci(&sfx, "GUIDs.txt"), shared, install].into_iter().flatten() {
        if let Ok(t) = std::fs::read(&g) {
            guids.extend(ac1bank::guids_txt(&String::from_utf8_lossy(&t)));
        }
    }
    let mut last = anyhow!("no bank");
    for bank in cands {
        let data = std::fs::read(&bank).with_context(|| format!("{}", bank.display()))?;
        let ac1 = match ac1bank::read(&data).with_context(|| format!("{}", bank.display())) {
            Ok(a) => a,
            Err(e) => {
                last = e;
                continue;
            }
        };
        drop(data);
        let mut names = HashMap::new();
        let mut paths = Vec::new();
        for (k, v) in &guids {
            if ac1.bank.events.contains_key(k) {
                names.entry(*k).or_insert(v.clone());
                paths.push((v.clone(), *k));
            }
        }
        if names.is_empty() {
            last = anyhow!("{}: no GUIDs.txt names its events", bank.display());
            continue;
        }
        return Ok(Source { ac1, bank_path: bank, names, paths });
    }
    Err(last)
}

/// The bank's own parameters as generator definitions.
fn param_defs(a: &Ac1Bank) -> BTreeMap<Guid, ParamDef> {
    a.bank.params.iter().map(|(g, p)| (*g, ParamDef { p: p.clone(), seek: a.seek.get(g).cloned(), bank: "ac1".into(), cursor: Vec::new() })).collect()
}

fn game_dir_of(content: &Path) -> Option<PathBuf> {
    if content.is_dir() { Some(content.to_path_buf()) } else { content.parent().map(Path::to_path_buf) }
}

/// Debug: every named event of the AC1 bank, decompiled unchanged into a Studio project.
pub fn dump(car_dir: &Path, content: &Path, out: &Path) -> Result<Vec<String>> {
    let s = open_source(car_dir)?;
    let src = mkfmod::Src::open(content)?;
    let gp = mkfmod::game_params(&src)?;
    let mut evs: Vec<(Guid, String, String)> = s.names.iter().map(|(g, p)| {
        let leaf = p.trim_start_matches("event:/").to_string();
        (*g, leaf.clone(), format!("ac1/{}", leaf.rsplit('/').next().unwrap_or("x")))
    }).collect();
    evs.sort_by(|a, b| a.1.cmp(&b.1));
    let f = mkfmod::Foreign { bank: &s.ac1.bank, params: param_defs(&s.ac1), curve_param: s.ac1.curve_param.clone(), events: evs, bank_id: s.ac1.bank.id,
        bank_name: "ac1_dump".into() };
    let o = mkfmod::generate_foreign(&src, &gp, &f, game_dir_of(content).as_deref())?;
    mkfmod::write(&o, out, "ac1_dump")?;
    let mut notes = s.ac1.bank.notes.clone();
    notes.extend(o.notes);
    Ok(notes)
}

// ------------------------------------------------------------------ roles
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Role {
    EngineInt,
    EngineExt,
    Turbo,
    Backfire,
    GearBox,
    Transmission,
    Horn,
}

impl Role {
    pub const ALL: [Role; 7] = [Role::EngineInt, Role::EngineExt, Role::Turbo, Role::Backfire, Role::GearBox, Role::Transmission, Role::Horn];
    /// The output event's name (event:/evo_cars/<id>/<name>).
    pub fn name(self) -> &'static str {
        match self {
            Role::EngineInt => "engine_int",
            Role::EngineExt => "engine_ext",
            Role::Turbo => "turbo",
            Role::Backfire => "backfire",
            Role::GearBox => "gearbox",
            Role::Transmission => "transmission",
            Role::Horn => "horn",
        }
    }
    /// The actor's AudioEventDataType.
    pub fn slot(self) -> u64 {
        match self {
            Role::EngineInt => 0,
            Role::EngineExt => 1,
            Role::Turbo => 8,
            Role::Backfire => 9,
            Role::GearBox => 12,
            Role::Transmission => 13,
            Role::Horn => 14,
        }
    }
    /// AC1 events that feed the role, first found wins.
    fn sources(self) -> &'static [&'static str] {
        match self {
            Role::EngineInt => &["engine_int"],
            Role::EngineExt => &["engine_ext"],
            Role::Turbo => &["turbo"],
            Role::Backfire => &["backfire_ext", "backfire_int"],
            Role::GearBox => &["gear_int", "gear_ext"],
            Role::Transmission => &["transmission"],
            Role::Horn => &["horn"],
        }
    }
    /// The template role (mkfmod::ROLES) whose stock event gives the mixer bus.
    fn template(self) -> &'static str {
        match self {
            Role::EngineInt => "engine_int",
            Role::EngineExt => "engine_ext",
            Role::Turbo => "turbo",
            Role::Backfire => "backfire",
            Role::GearBox => "gearbox",
            Role::Transmission => "transmission",
            Role::Horn => "horn",
        }
    }
    /// OURS: level offset (dB) on top of the AC1 event's own mix (docs/FMOD_BANK.md, section 13).
    /// The starting point of the level pass, and the gains used without the game's fmodstudio.dll:
    /// the median offsets the level pass set on the test cars (Kunos and mod banks).
    fn gain(self) -> f32 {
        match self {
            Role::EngineInt => -18.0,
            Role::EngineExt | Role::Turbo | Role::Transmission | Role::Horn => -14.0,
            Role::Backfire | Role::GearBox => -12.0,
        }
    }
}

/// AC1 parameter -> the game's parameter of the same meaning (same units on stock events).
fn evo_name(p: &br::Param) -> Option<&'static str> {
    match (p.ptype, p.name.as_str()) {
        (1, _) => Some("engine_distance"),
        (2, _) => Some("Event Cone Angle"),
        (0, "rpms" | "rpm") => Some("rpms"),
        (0, "throttle") => Some("throttle_position"),
        (0, "boost") => Some("boost"),
        (0, "bov") => Some("bov"),
        (0, "drivetrain_speed") => Some("drivetrain_speed"),
        (0, "speed") => Some("speed"),
        (0, "brake") => Some("brake_position"),
        (0, "gear") => Some("gear"),
        _ => None,
    }
}

// ------------------------------------------------------------------ game side (cached per content path)
struct Game {
    params: mkfmod::GameParams,
    /// parameter name -> GUID (the parameters the template role events use)
    by_name: HashMap<String, Guid>,
    /// template role -> the mixer bus its stock event feeds
    bus: HashMap<String, Guid>,
}

fn game(content: &Path) -> Result<Arc<Game>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<Game>>>> = OnceLock::new();
    let m = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(x) = g.get(content) {
        return Ok(x.clone());
    }
    let src = mkfmod::Src::open(content)?;
    let params = mkfmod::game_params(&src)?;
    let strings = br::strings(&src.bank("master_bank.strings.bank")?)?;
    let mut by_name = HashMap::new();
    let mut bus = HashMap::new();
    let mut banks: HashMap<&str, Bank> = HashMap::new();
    for (role, bank, path, _) in mkfmod::ROLES {
        if !banks.contains_key(bank) {
            banks.insert(bank, br::read_meta(&src.bank(&format!("{bank}.bank"))?)?);
        }
        let bk = &banks[bank];
        let id = strings.iter().find(|(p, _)| p.eq_ignore_ascii_case(path)).map(|x| x.1).ok_or_else(|| anyhow!("the game has no {path} (game update?)"))?;
        let ev = bk.events.get(&id).ok_or_else(|| anyhow!("{path} not in {bank}.bank"))?;
        let mi = bk.buses.get(&ev.mixer_input).ok_or_else(|| anyhow!("{path}: no mixer input"))?;
        bus.insert(role.to_string(), mi.output);
        for p in &ev.pinst {
            let gp = p.xor(&id);
            if let Some(d) = params.params.get(&gp) {
                by_name.entry(d.p.name.clone()).or_insert(gp);
            }
        }
    }
    let x = Arc::new(Game { params, by_name, bus });
    g.insert(content.to_path_buf(), x.clone());
    Ok(x)
}

// ------------------------------------------------------------------ building
pub struct Opts {
    pub car_id: String,
    /// the game's content.kspkg (or game folder)
    pub content: PathBuf,
    /// AC1 engine limiter rpm (gates the backfire)
    pub limiter: Option<f32>,
    /// project folder (kept for inspection); the caller removes it
    pub work: PathBuf,
    /// Vorbis quality; None = automatic (Vorbis when the AC1 bank is Vorbis, else PCM16)
    pub vorbis: Option<Option<f32>>,
}

/// ACECONV_BANK_VORBIS set: its quality (`0` = PCM16) for every car; unset: automatic.
pub fn vorbis_env() -> Option<Option<f32>> {
    std::env::var_os("ACECONV_BANK_VORBIS").map(|_| super::bankwrite::vorbisenc::quality())
}

pub struct Built {
    pub bank: Vec<u8>,
    pub strings: Vec<u8>,
    /// (role, event path)
    pub events: Vec<(Role, String)>,
    pub notes: Vec<String>,
}

/// A deterministic GUID from (car, role, original).
fn seeded(car: &str, role: &str, g: &Guid) -> Guid {
    use md5::{Digest, Md5};
    let mut h = Md5::new();
    h.update(b"aceconv-sound\0");
    h.update(car.as_bytes());
    h.update([0]);
    h.update(role.as_bytes());
    h.update(g.0);
    let mut b: [u8; 16] = h.finalize().into();
    b[7] = (b[7] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    Guid(b)
}

fn g(s: &str) -> Guid {
    Guid::parse(s).unwrap()
}
/// EventMixerGroup / Master volume mapping (fader domain)
const MAP_BUS_VOLUME: &str = "{d65ba128-a6f5-41d9-b6a1-a7c402f329ac}";

/// The objects of one AC1 event (ids as in the AC1 bank).
#[derive(Default)]
struct Sub {
    buses: BTreeSet<Guid>,
    effects: BTreeSet<Guid>,
    /// top-level instruments
    top: Vec<Guid>,
    insts: BTreeSet<Guid>,
    curves: BTreeSet<Guid>,
    mods: BTreeSet<Guid>,
}

fn props_into(props: &[Prop], s: &mut Sub) {
    for p in props {
        s.curves.extend(p.curves.iter().copied());
        s.mods.extend(p.mods.iter().copied());
    }
}

fn collect(a: &Ac1Bank, ev: Guid) -> Result<Sub> {
    let bk = &a.bank;
    let e = bk.events.get(&ev).ok_or_else(|| anyhow!("event {ev} missing"))?;
    let mut s = Sub::default();
    props_into(&e.props, &mut s);
    s.buses.insert(e.mixer_input);
    s.buses.insert(e.master_bus);
    s.buses.extend(e.buses.iter().copied());
    for b in s.buses.clone() {
        let bus = bk.buses.get(&b).ok_or_else(|| anyhow!("bus {b} missing"))?;
        props_into(&bus.props, &mut s);
        for fx in bus.pre.iter().chain(bus.post.iter()) {
            s.effects.insert(*fx);
            if let Some(x) = bk.effects.get(fx) {
                props_into(x.props(), &mut s);
            }
        }
    }
    let busset: BTreeSet<Guid> = s.buses.iter().copied().collect();
    let mut top: Vec<Guid> = a.event_insts.get(&ev).cloned().unwrap_or_default();
    // instruments on the event's buses that no list places (kept on the timeline at 0)
    for i in bk.instruments.values() {
        if busset.contains(&i.inst().bus) && !top.contains(&i.id()) && !is_nested(bk, i.id()) {
            top.push(i.id());
        }
    }
    top.sort();
    top.dedup();
    let mut todo = top.clone();
    while let Some(i) = todo.pop() {
        if !s.insts.insert(i) {
            continue;
        }
        // an AC1 bank can name a sound it does not hold (a nested event / programmer sound): skipped
        let Some(it) = bk.instruments.get(&i) else {
            s.insts.remove(&i);
            continue;
        };
        props_into(&it.inst().props, &mut s);
        match it {
            Instrument::Multi { sounds, .. } => todo.extend(sounds.iter().map(|x| x.0)),
            Instrument::Scatter { sound, .. } => todo.push(*sound),
            Instrument::Wave { .. } => {}
        }
    }
    s.top = top;
    Ok(s)
}

fn is_nested(bk: &Bank, id: Guid) -> bool {
    bk.instruments.values().any(|i| match i {
        Instrument::Multi { sounds, .. } => sounds.iter().any(|x| x.0 == id),
        Instrument::Scatter { sound, .. } => *sound == id,
        _ => false,
    })
}

/// An AC1 effect as the in-house writer can build it, or None (dropped, noted): FMOD 1.08 shipped
/// several FMOD effects as plugins (built in since 2.0); automation without a known Studio mapping
/// keeps the effect at its set value.
fn writable(e: Effect, notes: &mut Vec<String>, role: &str) -> Option<Effect> {
    use super::bankwrite::effects;
    let e = match e {
        Effect::Plugin { id, ident, params, props } => {
            let f = |i: usize| match params.get(i) {
                Some(br::PVal::F(v)) => Some(*v),
                _ => None,
            };
            match ident.as_str() {
                "FMOD Distance Filter" => Effect::Plugin { id, ident, params, props },
                "FMOD Lowpass Simple" => Effect::Builtin { id, dsp: 20, params: vec![br::PVal::F(f(0).unwrap_or(22000.0))], props },
                "FMOD Gain" => Effect::Builtin { id, dsp: 7, params: vec![br::PVal::F(f(0).unwrap_or(0.0)), br::PVal::D(Vec::new())], props },
                _ => {
                    notes.push(format!("sound: {role}: AC1 effect {ident} has no counterpart in the writer - left out"));
                    return None;
                }
            }
        }
        x => x,
    };
    let class = match &e {
        Effect::Builtin { dsp: 27, params, .. } => {
            if matches!(params.get(17), Some(br::PVal::F(v)) if *v == 1.0) { "SpatialiserEffect" } else { "MixerBusPanner" }
        }
        Effect::Builtin { dsp, .. } => match effects::class_of_dsp(*dsp) {
            Some(c) => c,
            None => {
                notes.push(format!("sound: {role}: AC1 effect (DSP type {dsp}) has no counterpart in the writer - left out"));
                return None;
            }
        },
        Effect::Plugin { .. } => "PluginEffect",
        Effect::Send { .. } => "MixerSend",
    };
    let keep = |p: &Prop| (p.curves.is_empty() && p.mods.is_empty()) || effects::mapping(class, p.index).is_some();
    let fix = |props: Vec<Prop>, notes: &mut Vec<String>| -> Vec<Prop> {
        let (k, d): (Vec<Prop>, Vec<Prop>) = props.into_iter().partition(keep);
        for p in d {
            notes.push(format!("sound: {role}: automation of {class} parameter {} has no known mapping - held at its value", p.index));
        }
        k
    };
    Some(match e {
        Effect::Builtin { id, dsp, params, props } => Effect::Builtin { id, dsp, params, props: fix(props, notes) },
        Effect::Plugin { id, ident, params, props } => Effect::Plugin { id, ident, params, props: fix(props, notes) },
        Effect::Send { id, ret, level, props } => Effect::Send { id, ret, level, props: fix(props, notes) },
    })
}

/// The modulators the writer builds: ADSR on volume (instruments and buses), randomizers on an
/// instrument's pitch or start offset. Others are left out (noted).
fn keep_mods(props: &[Prop], mods: &BTreeMap<Guid, Modu>, inst: bool, notes: &mut Vec<String>, role: &str) -> Vec<Prop> {
    props.iter().map(|p| {
        let mut q = p.clone();
        q.mods.retain(|m| {
            let ok = mods.get(m).map(|mo| match (mo.typ, p.index) {
                (0, 0) => true,
                (1, 1 | 11) => inst,
                _ => false,
            }).unwrap_or(false);
            if !ok {
                notes.push(format!("sound: {role}: a modulator (type {}) on property {} is left out", mods.get(m).map(|x| x.typ).unwrap_or(99), p.index));
            }
            ok
        });
        q
    }).collect()
}

/// Rewrite the parameter GUIDs of a trigger code.
fn remap_code(code: &[u8], f: &dyn Fn(&Guid) -> Guid) -> Vec<u8> {
    let mut out = Vec::with_capacity(code.len());
    let mut i = 0;
    while i + 4 <= code.len() {
        let t = u32::from_le_bytes(code[i..i + 4].try_into().unwrap());
        out.extend_from_slice(&code[i..i + 4]);
        i += 4;
        if t == 0x11 && i + 16 + 12 <= code.len() {
            let gp = Guid(code[i..i + 16].try_into().unwrap());
            out.extend_from_slice(&f(&gp).0);
            out.extend_from_slice(&code[i + 16..i + 28]);
            i += 28;
        }
    }
    out.extend_from_slice(&code[i..]);
    out
}

/// The parameters a code tests.
fn code_params(code: &[u8]) -> Vec<Guid> {
    let mut v = Vec::new();
    let mut i = 0;
    while i + 4 <= code.len() {
        let t = u32::from_le_bytes(code[i..i + 4].try_into().unwrap());
        i += 4;
        if t == 0x11 && i + 28 <= code.len() {
            v.push(Guid(code[i..i + 16].try_into().unwrap()));
            i += 28;
        }
    }
    v
}

fn term(gp: &Guid, tag: u32, lo: f32, hi: f32) -> Vec<u8> {
    let mut v = 0x11u32.to_le_bytes().to_vec();
    v.extend_from_slice(&gp.0);
    v.extend_from_slice(&tag.to_le_bytes());
    v.extend_from_slice(&lo.to_le_bytes());
    v.extend_from_slice(&hi.to_le_bytes());
    v
}

fn inst_mut(i: &mut Instrument) -> &mut br::Inst {
    match i {
        Instrument::Wave { inst, .. } | Instrument::Multi { inst, .. } | Instrument::Scatter { inst, .. } => inst,
    }
}

/// One output event under construction (AC1 ids; parameters already the game's where mapped).
struct Work<'a> {
    a: &'a Ac1Bank,
    s: Sub,
    /// instrument edits
    insts: BTreeMap<Guid, Instrument>,
    buses: BTreeMap<Guid, br::Bus>,
    curves: BTreeMap<Guid, Curve>,
    curve_param: HashMap<Guid, Guid>,
    timeline: br::Timeline,
    extra_insts: Vec<Guid>,
    /// buses merged in from another AC1 event (now group buses of this one)
    extra_buses: Vec<Guid>,
}

/// How one-shot instruments move onto a game parameter's sheet.
struct Trigger {
    /// (parameter, lo, hi, parameter maximum for the end-tag rule)
    sheet: (Guid, f32, f32, f32),
    extra: Vec<u8>,
    looping: bool,
    prob: Option<f32>,
}

/// OURS: the level a converted engine is set to - RMS (dBFS) of a 1000 rpm -> limiter sweep at
/// full throttle (exterior 6 m behind, interior 1 m) - the median of the 75 stock cars measured the same way
/// (docs/FMOD_BANK.md, section 13).
const TARGET_EXT: f32 = -15.6;
const TARGET_INT: f32 = -17.2;

/// Above this a PCM16 car bank is written as Vorbis (OURS: stock car banks are 2-20 MB of Vorbis).
const PCM_LIMIT: usize = 96 << 20;

/// The level shots: exterior 6 m behind the car, interior 1 m, full-throttle rpm sweep.
fn level_shots<'a>(ext: &'a str, int: &'a str, lim: f32) -> Vec<super::render::Shot<'a>> {
    let mk = |e: &'a str, d: f32| super::render::Shot { event: e, distance: d, secs: 8.0,
        ramps: vec![("rpms", 1000.0, lim), ("throttle_position", 1.0, 1.0), ("ignition", 2.0, 2.0), ("starter", 2.0, 2.0)] };
    vec![mk(ext, 6.0), mk(int, 1.0)]
}

/// Convert the AC1 car's sounds: build, measure the engine level through the game's runtime,
/// rebuild at the target level.
pub fn build(car_dir: &Path, o: &Opts) -> Result<Built> {
    let src_bank = open_source(car_dir)?;
    let gm = game(&o.content)?;
    let q = match o.vorbis {
        Some(q) => q,
        None => (src_bank.ac1.codec == super::fsb5::MODE_VORBIS).then_some(0.8),
    };
    let mut gains: HashMap<Role, f32> = Role::ALL.iter().map(|r| (*r, r.gain())).collect();
    let game_dir = game_dir_of(&o.content).unwrap_or_default();
    let lim = o.limiter.unwrap_or(7000.0);
    let master = mkfmod::Src::open(&o.content)?;
    let (ms, mb) = (master.bank("master_bank.strings.bank")?, master.bank("master_bank.bank")?);
    let mut level_note = String::from("level: not measured");
    if game_dir.join("fmodstudio.dll").is_file() {
        for round in 0..3 {
            let b = pass(&src_bank, &gm, o, &gains, None)?;
            let ext = b.events.iter().find(|e| e.0 == Role::EngineExt).map(|e| e.1.clone());
            let int = b.events.iter().find(|e| e.0 == Role::EngineInt).map(|e| e.1.clone());
            let (e, i) = (ext.clone().or(int.clone()), int.clone().or(ext.clone()));
            let (Some(e), Some(i)) = (e, i) else { break };
            let lv = match super::render::rms(&game_dir, &[&ms, &mb, &b.strings, &b.bank], &level_shots(&e, &i, lim), &o.work.join("level")) {
                Ok(v) => v,
                Err(err) => {
                    level_note = format!("level: render failed ({err:#}) - fixed role gains");
                    break;
                }
            };
            let (de, di) = ((TARGET_EXT - lv[0]).clamp(-30.0, 30.0), (TARGET_INT - lv[1]).clamp(-30.0, 30.0));
            level_note = format!("level: engine ext {:.1} dB, int {:.1} dB (target {TARGET_EXT} / {TARGET_INT}) after round {}", lv[0], lv[1], round);
            if std::env::var_os("ACECONV_SOUND_DEBUG").is_some() {
                eprintln!("level round {round}: gains ext {:+.1} int {:+.1} -> {:.1} / {:.1}", gains[&Role::EngineExt], gains[&Role::EngineInt], lv[0], lv[1]);
            }
            if de.abs() < 0.5 && di.abs() < 0.5 {
                break;
            }
            // a silent sweep (an engine that needs parameters the game does not set) is left alone
            if lv[0] < -60.0 || lv[1] < -60.0 {
                level_note += " - too quiet to set, AC1's own level kept";
                gains = Role::ALL.iter().map(|r| (*r, r.gain())).collect();
                break;
            }
            for (r, g) in gains.iter_mut() {
                *g += match r {
                    Role::EngineInt if int.is_some() => di,
                    _ => de,
                };
            }
            let _ = std::fs::remove_dir_all(o.work.join("level"));
        }
    }
    let mut b = pass(&src_bank, &gm, o, &gains, q)?;
    // OURS: a PCM16 bank this large costs the game that much memory per car: Vorbis (the game's
    // own codec) unless a codec was asked for
    if q.is_none() && o.vorbis.is_none() && b.bank.len() > PCM_LIMIT {
        let mb = b.bank.len() >> 20;
        b = pass(&src_bank, &gm, o, &gains, Some(0.8))?;
        b.notes.push(format!("sound: PCM16 would be {mb} MB - Vorbis"));
    }
    b.notes.push(format!("sound: {level_note}; gains {}", Role::ALL.iter().map(|r| format!("{} {:+.1}", r.name(), gains[r])).collect::<Vec<_>>().join(", ")));
    Ok(b)
}

fn pass(src_bank: &Source, gm: &Game, o: &Opts, gains: &HashMap<Role, f32>, q: Option<f32>) -> Result<Built> {
    let a = &src_bank.ac1;
    let mut notes: Vec<String> = Vec::new();
    // AC1 events by leaf name
    let mut by_leaf: HashMap<String, Guid> = HashMap::new();
    for (p, g) in &src_bank.paths {
        by_leaf.entry(p.rsplit('/').next().unwrap_or("").to_lowercase()).or_insert(*g);
    }
    // AC1 parameter -> game parameter
    let mut pmap: HashMap<Guid, Guid> = HashMap::new();
    for p in a.bank.params.values() {
        if let Some(n) = evo_name(p) {
            match gm.by_name.get(n) {
                Some(gp) => {
                    pmap.insert(p.id, *gp);
                }
                None => notes.push(format!("sound: the game has no parameter {n} (for AC1 {}) - kept local", p.name)),
            }
        }
    }
    let gpar = |n: &str| -> Result<Guid> { gm.by_name.get(n).copied().ok_or_else(|| anyhow!("the game has no parameter {n} (game update?)")) };
    let pmax = |gp: &Guid| -> f32 {
        gm.params.params.get(gp).map(|d| if d.p.flags & 8 != 0 { d.p.max + 1.0 } else { d.p.max }).unwrap_or(1.0)
    };
    let mut out = Bank { id: seeded(&o.car_id, "bank", &a.bank.id), maps: a.bank.maps.clone(), ..Default::default() };
    let mut out_params: BTreeMap<Guid, ParamDef> = BTreeMap::new();
    let mut out_curve_param: HashMap<Guid, Guid> = HashMap::new();
    let mut events: Vec<(Guid, String, String)> = Vec::new();
    let mut built: Vec<(Role, String)> = Vec::new();
    let keep: BTreeSet<Guid> = gm.params.params.keys().copied().chain(gm.bus.values().copied()).collect();
    for role in Role::ALL {
        let Some(ev) = role.sources().iter().find_map(|n| by_leaf.get(*n).copied()) else { continue };
        let s = collect(a, ev)?;
        let e = &a.bank.events[&ev];
        let master_vol = a.bank.buses.get(&e.master_bus).map(|b| b.volume).unwrap_or(0.0);
        if s.top.is_empty() || s.insts.is_empty() || master_vol <= -60.0 {
            notes.push(format!("sound: AC1 {} is silent (no instruments or muted) - the game's {} stays", src_bank.names[&ev], role.name()));
            continue;
        }
        let bus_out = *gm.bus.get(role.template()).ok_or_else(|| anyhow!("no game bus for {}", role.name()))?;
        let mut w = Work { a, insts: s.insts.iter().map(|i| (*i, a.bank.instruments[i].clone())).collect(),
            buses: s.buses.iter().map(|b| (*b, a.bank.buses[b].clone())).collect(),
            curves: s.curves.iter().filter_map(|c| a.bank.curves.get(c).map(|x| (*c, x.clone()))).collect(),
            curve_param: s.curves.iter().filter_map(|c| a.curve_param.get(c).map(|p| (*c, *p))).collect(),
            timeline: a.bank.timelines.get(&e.timeline).cloned().unwrap_or(br::Timeline { id: e.timeline, insts: vec![], markers: vec![], trans: vec![] }),
            extra_insts: Vec::new(), extra_buses: Vec::new(), s };
        // the event's mixer input feeds the game's bus for the role
        if let Some(mi) = w.buses.get_mut(&e.mixer_input) {
            mi.output = bus_out;
        }
        // role restructuring
        match role {
            Role::GearBox => {
                let (up, dn) = (gpar("gear_up")?, gpar("gear_dn")?);
                let top = w.s.top.clone();
                for i in top {
                    let (is_up, is_dn) = match a.sheet.get(&i) {
                        Some((gp, lo, len)) if a.bank.params.get(gp).map(|p| p.name == "state").unwrap_or(false) => (lo + len > 0.5, *lo < 0.5),
                        _ => (true, true),
                    };
                    let mk = |p: Guid| Trigger { sheet: (p, 0.5, 1.0, pmax(&p)), extra: vec![], looping: false, prob: None };
                    match (is_up, is_dn) {
                        (true, true) => {
                            w.retrigger(i, &mk(up));
                            let c = w.clone_inst(i, &o.car_id)?;
                            w.retrigger(c, &mk(dn));
                        }
                        (true, false) => w.retrigger(i, &mk(up)),
                        (false, true) => w.retrigger(i, &mk(dn)),
                        _ => {}
                    }
                }
            }
            Role::Backfire => {
                let tp = gpar("throttle_position")?;
                let rp = gpar("rpms")?;
                // OURS: pops when the throttle closes from above 25 % at more than half the limiter rpm, 60 % of the time
                let lim = o.limiter.unwrap_or(7000.0);
                let extra = term(&rp, 0x5020, (lim * 0.5).round(), pmax(&rp));
                let t = Trigger { sheet: (tp, 0.0, 0.25, pmax(&tp)), extra, looping: false, prob: Some(60.0) };
                for i in w.s.top.clone() {
                    w.retrigger(i, &t);
                }
            }
            Role::Horn => {
                let hs = gpar("horn_sound")?;
                let t = Trigger { sheet: (hs, 1.0, 2.0, pmax(&hs)), extra: vec![], looping: true, prob: None };
                for i in w.s.top.clone() {
                    w.retrigger(i, &t);
                }
            }
            Role::EngineInt | Role::EngineExt => {
                // AC1 starts its limiter event on the limiter; the game raises `limiter` on the engine events
                if let Some(lv) = by_leaf.get("limiter").copied() {
                    let ls = collect(a, lv)?;
                    let lm = a.bank.buses.get(&a.bank.events[&lv].master_bus).map(|b| b.volume).unwrap_or(0.0);
                    if !ls.top.is_empty() && !ls.insts.is_empty() && lm > -60.0 {
                        let lp = gpar("limiter")?;
                        let t = Trigger { sheet: (lp, 0.5, 1.0, pmax(&lp)), extra: vec![], looping: true, prob: None };
                        let n = w.merge(lv, ls, e.master_bus);
                        for i in n {
                            w.retrigger(i, &t);
                        }
                        notes.push(format!("sound: {}: + AC1 limiter on the game's limiter parameter", role.name()));
                    }
                }
            }
            _ => {}
        }
        if w.timeline.insts.is_empty() {
            w.timeline.markers.clear();
            w.timeline.trans.clear();
        }
        // OURS: engines are silent when the engine is not turning (AC1 sheets often start at 0 rpm)
        if matches!(role, Role::EngineInt | Role::EngineExt) {
            let rp = gpar("rpms")?;
            w.gate(e.master_bus, rp, &[(0.0, -80.0), (100.0, -80.0), (300.0, 0.0)], &o.car_id);
        }
        // the level offset sits on the event's mixer input (never automated; the runtime caps a bus
        // at +10 dB), any boost beyond that on the master
        let g = gains.get(&role).copied().unwrap_or(role.gain());
        w.gain(e.mixer_input, g.min(10.0));
        if g > 10.0 {
            w.gain(e.master_bus, g - 10.0);
        }
        // ---- copy out with game parameters and re-seeded ids
        let rn = role.name();
        let car = o.car_id.clone();
        let f = |x: &Guid| -> Guid {
            if x.is_zero() || keep.contains(x) {
                return *x;
            }
            if let Some(p) = pmap.get(x) {
                return *p;
            }
            seeded(&car, rn, x)
        };
        let nev = f(&ev);
        let mut used: BTreeSet<Guid> = BTreeSet::new();
        for (c, p) in &w.curve_param {
            out_curve_param.insert(f(c), f(p));
            // timeline automation: the curve follows the event's timeline, not a parameter
            if !a.bank.timelines.contains_key(p) {
                used.insert(f(p));
            }
        }
        for c in w.curves.values() {
            out.curves.insert(f(&c.id), Curve { id: f(&c.id), g2: f(&c.g2), pts: c.pts.clone() });
        }
        // automation along the event's timeline (AC1 start-up sequences) has no counterpart in the
        // writer: those curves are left out
        let on_timeline = |c: &Guid| w.curve_param.get(c).map(|p| a.bank.timelines.contains_key(p)).unwrap_or(false);
        let n_tl = w.curves.keys().filter(|c| on_timeline(c)).count();
        if n_tl > 0 {
            notes.push(format!("sound: {rn}: {n_tl} timeline automation curve(s) left out"));
        }
        let fp = |props: &[Prop]| -> Vec<Prop> {
            props.iter().map(|p| Prop { index: p.index, a: p.a, b: p.b, map: p.map, curves: p.curves.iter().filter(|c| !on_timeline(c)).map(&f).collect(),
                mods: p.mods.iter().map(&f).collect() })
                .filter(|p| !(p.curves.is_empty() && p.mods.is_empty()) || props.iter().any(|q| q.index == p.index && q.curves.is_empty() && q.mods.is_empty()))
                .collect()
        };
        let mut kept: BTreeSet<Guid> = BTreeSet::new();
        for x in &w.s.effects {
            let Some(ef) = a.bank.effects.get(x) else { continue };
            let Some(ef) = writable(ef.clone(), &mut notes, rn) else { continue };
            let ne = match ef {
                Effect::Builtin { id, dsp, params, props } => Effect::Builtin { id: f(&id), dsp, params, props: fp(&props) },
                Effect::Plugin { id, ident, params, props } => Effect::Plugin { id: f(&id), ident, params, props: fp(&props) },
                Effect::Send { id, ret, level, props } => Effect::Send { id: f(&id), ret: f(&ret), level, props: fp(&props) },
            };
            if let Some(fb) = a.bank.effb.get(x) {
                out.effb.insert(ne.id(), *fb);
            }
            kept.insert(*x);
            out.effects.insert(ne.id(), ne);
        }
        for b in w.buses.values() {
            let mut nb = b.clone();
            nb.id = f(&b.id);
            nb.output = f(&b.output);
            nb.pre = b.pre.iter().filter(|x| kept.contains(x)).map(&f).collect();
            nb.post = b.post.iter().filter(|x| kept.contains(x)).map(&f).collect();
            nb.props = fp(&keep_mods(&b.props, &a.bank.mods, false, &mut notes, rn));
            out.buses.insert(nb.id, nb);
        }
        for m in &w.s.mods {
            if let Some(mo) = a.bank.mods.get(m) {
                out.mods.insert(f(m), Modu { id: f(m), target: f(&mo.target), ..mo.clone() });
            }
        }
        for it in w.insts.values() {
            let mut ni = it.clone();
            {
                let i = inst_mut(&mut ni);
                i.timeline = f(&i.timeline);
                i.bus = f(&i.bus);
                i.ap_param = f(&i.ap_param);
                i.code = remap_code(&i.code, &f);
                used.extend(code_params(&i.code));
                if !i.ap_param.is_zero() {
                    used.insert(i.ap_param);
                }
                i.props = fp(&keep_mods(&i.props, &a.bank.mods, true, &mut notes, rn));
            }
            let ni = match ni {
                Instrument::Wave { id, audio, inst } => {
                    let na = f(&audio);
                    if let Some(ix) = a.bank.waves.get(&audio) {
                        out.waves.insert(na, *ix);
                    }
                    Instrument::Wave { id: f(&id), audio: na, inst }
                }
                Instrument::Multi { id, mode, sounds, inst } => {
                    let sounds: Vec<(Guid, f32)> = sounds.iter().filter(|x| w.insts.contains_key(&x.0)).map(|(s, wt)| (f(s), *wt)).collect();
                    if sounds.is_empty() {
                        continue;
                    }
                    Instrument::Multi { id: f(&id), mode, sounds, inst }
                }
                Instrument::Scatter { id, poly, total, tmin, tmax, steal, rate, sound, inst } => {
                    if !w.insts.contains_key(&sound) {
                        continue;
                    }
                    Instrument::Scatter { id: f(&id), poly, total, tmin, tmax, steal, rate, sound: f(&sound), inst }
                }
            };
            out.instruments.insert(ni.id(), ni);
        }
        let tl = &w.timeline;
        out.timelines.insert(f(&tl.id), br::Timeline {
            id: f(&tl.id),
            insts: tl.insts.iter().map(|(i, s, l)| (f(i), *s, *l)).collect(),
            markers: tl.markers.iter().map(|m| br::Marker { id: f(&m.id), ..m.clone() }).collect(),
            trans: tl.trans.iter().map(|t| {
                used.extend(code_params(&remap_code(&t.code, &f)));
                br::Tran { id: f(&t.id), dest: f(&t.dest), code: remap_code(&t.code, &f), ..t.clone() }
            }).collect(),
        });
        // parameters: the game's, or the AC1 event's own (re-seeded)
        for gp in &used {
            if gm.params.params.contains_key(gp) {
                continue;
            }
            let Some((ap, p)) = a.bank.params.iter().find(|(k, _)| f(k) == *gp) else {
                notes.push(format!("sound: {rn}: parameter {gp} has no definition"));
                continue;
            };
            let mut np = p.clone();
            np.id = *gp;
            // built-ins other than distance / cone angle (direction, elevation ...): a plain
            // parameter at its initial value (the game never sets it)
            if np.flags & 16 != 0 && !matches!(np.ptype, 1 | 2) {
                notes.push(format!("sound: {rn}: AC1 built-in parameter {} (type {}) held at {}", np.name, np.ptype, np.init));
                np.flags = 0;
                np.ptype = 0;
            }
            out_params.insert(*gp, ParamDef { p: np, seek: a.seek.get(ap).map(|m| Modu { id: f(&m.id), target: *gp, ..m.clone() }), bank: "ac1".into(),
                cursor: Vec::new() });
        }
        let mut ne = e.clone();
        ne.id = nev;
        ne.timeline = f(&e.timeline);
        ne.mixer_input = f(&e.mixer_input);
        ne.master_bus = f(&e.master_bus);
        ne.buses = e.buses.iter().chain(w.extra_buses.iter()).map(&f).collect();
        ne.buses.sort();
        ne.props = fp(&e.props);
        ne.pinst = used.iter().map(|p| p.xor(&nev)).collect();
        ne.pinst.sort();
        ne.user_props.clear();
        // the game keeps its backfire event running (stock: persistent) and drives it by parameters;
        // the horn waits for horn_sound (stock: a timeline loop until pressed, the same lifetime)
        if matches!(role, Role::Backfire | Role::Horn) {
            ne.flags |= 2;
        }
        out.events.insert(nev, ne);
        let path = format!("evo_cars/{}/{rn}", o.car_id);
        events.push((nev, path.clone(), format!("{}/{rn}", o.car_id)));
        built.push((role, format!("event:/{path}")));
        notes.push(format!("sound: {rn} <- AC1 {} ({} instruments)", src_bank.names[&ev], w.insts.len()));
    }
    if built.is_empty() {
        bail!("no AC1 event converts (all missing or silent)");
    }
    // only the samples the built events play (AC1 banks also hold wheels, skids, doors ...)
    let mut used_ix: Vec<u32> = out.waves.values().copied().collect();
    used_ix.sort();
    used_ix.dedup();
    let remap: HashMap<u32, u32> = used_ix.iter().enumerate().map(|(k, i)| (*i, k as u32)).collect();
    out.samples = used_ix.iter().map(|i| a.bank.samples.get(*i as usize).cloned()).collect::<Option<Vec<_>>>()
        .ok_or_else(|| anyhow!("the AC1 bank names a sample it does not hold"))?;
    for v in out.waves.values_mut() {
        *v = remap[v];
    }
    out.n_events = out.events.len() as u32;
    // ---- Studio project -> banks
    let src = mkfmod::Src::open(&o.content)?;
    let fr = mkfmod::Foreign { bank: &out, params: out_params, curve_param: out_curve_param, events, bank_id: out.id, bank_name: o.car_id.clone() };
    let prj = mkfmod::generate_foreign(&src, &gm.params, &fr, game_dir_of(&o.content).as_deref())?;
    // generator notes that only describe how a region / mapping was written are left out
    let quiet = ["has no strings path", "inclusive below the parameter maximum", "differs from the writer's table", "ends exclusively at the parameter maximum"];
    notes.extend(prj.notes.iter().filter(|n| !quiet.iter().any(|q| n.contains(q))).map(|n| format!("sound: {n}")));
    mkfmod::write(&prj, &o.work, &o.car_id)?;
    drop(prj);
    drop(out);
    let b = super::bankwrite::build_dir_with(&o.work, q).context("bank writer")?;
    let codec = super::fsb5::blobs_of(&b.bank).ok().and_then(|v| v.first().copied()).and_then(|o| super::fsb5::u32_(&b.bank, o + 24).ok());
    notes.push(format!("sound: {} events, bank {:.1} MB ({})", built.len(), b.bank.len() as f64 / 1048576.0, match codec {
        Some(super::fsb5::MODE_VORBIS) => "Vorbis",
        Some(super::fsb5::MODE_PCM16) => "PCM16",
        _ => "no samples",
    }));
    Ok(Built { bank: b.bank, strings: b.strings, events: built, notes })
}

impl Work<'_> {
    /// Move a top-level instrument onto a game parameter's sheet (its own conditions kept).
    fn retrigger(&mut self, i: Guid, t: &Trigger) {
        let cond = self.a.cond.get(&i).cloned().unwrap_or_default();
        let (gp, lo, hi, pmax) = t.sheet;
        let mut c = cond;
        if !t.extra.is_empty() {
            if !c.is_empty() {
                c.extend_from_slice(&0x1002u32.to_le_bytes());
            }
            c.extend_from_slice(&t.extra);
        }
        if let Some(it) = self.insts.get_mut(&i) {
            let ins = inst_mut(it);
            ins.timeline = Guid::ZERO;
            ins.code = ac1bank::with_sheet(&gp, lo, hi, pmax, &c);
            if t.looping {
                ins.loops = -1;
                ins.cutoff = 1;
            }
            if let Some(p) = t.prob {
                ins.prob = p;
            }
        }
        self.timeline.insts.retain(|x| x.0 != i);
    }

    /// Another AC1 event's buses, effects and instruments, as a group inside this event: its master
    /// bus feeds `into` (its spatialiser dropped: this event spatialises), its mixer input goes.
    /// Returns its top-level instruments.
    fn merge(&mut self, ev: Guid, o: Sub, into: Guid) -> Vec<Guid> {
        let e = &self.a.bank.events[&ev];
        for b in &o.buses {
            if *b == e.mixer_input {
                continue;
            }
            let mut bus = self.a.bank.buses[b].clone();
            if *b == e.master_bus {
                bus.output = into;
                bus.post.retain(|x| !matches!(self.a.bank.effects.get(x), Some(Effect::Builtin { dsp: 27, .. })));
                bus.pre.retain(|x| !matches!(self.a.bank.effects.get(x), Some(Effect::Builtin { dsp: 27, .. })));
                bus.flags = 0;
            }
            for x in bus.pre.iter().chain(bus.post.iter()) {
                self.s.effects.insert(*x);
            }
            self.buses.insert(*b, bus);
            self.extra_buses.push(*b);
        }
        for i in &o.insts {
            self.insts.entry(*i).or_insert_with(|| self.a.bank.instruments[i].clone());
        }
        for c in &o.curves {
            if let Some(x) = self.a.bank.curves.get(c) {
                self.curves.insert(*c, x.clone());
            }
            if let Some(p) = self.a.curve_param.get(c) {
                self.curve_param.insert(*c, *p);
            }
        }
        self.s.mods.extend(o.mods.iter().copied());
        self.s.insts.extend(o.insts.iter().copied());
        self.s.buses.extend(self.extra_buses.iter().copied());
        o.top
    }

    /// A copy of a top-level instrument (and its nested sounds, curves, modulators) under new ids.
    fn clone_inst(&mut self, i: Guid, car: &str) -> Result<Guid> {
        let tag = format!("copy:{i}");
        let nid = |g: &Guid| seeded(car, &tag, g);
        let mut todo = vec![i];
        let mut map: HashMap<Guid, Guid> = HashMap::new();
        while let Some(x) = todo.pop() {
            if map.contains_key(&x) {
                continue;
            }
            map.insert(x, nid(&x));
            match self.insts.get(&x) {
                Some(Instrument::Multi { sounds, .. }) => todo.extend(sounds.iter().map(|s| s.0)),
                Some(Instrument::Scatter { sound, .. }) => todo.push(*sound),
                _ => {}
            }
        }
        for (old, new) in map.clone() {
            let mut it = self.insts.get(&old).cloned().ok_or_else(|| anyhow!("instrument {old} missing"))?;
            let ins = inst_mut(&mut it);
            for p in ins.props.iter_mut() {
                for c in p.curves.iter_mut() {
                    let nc = nid(c);
                    if let Some(cv) = self.curves.get(c).cloned() {
                        self.curves.insert(nc, Curve { id: nc, g2: nc, pts: cv.pts });
                    }
                    if let Some(gp) = self.curve_param.get(c).copied() {
                        self.curve_param.insert(nc, gp);
                    }
                    *c = nc;
                }
                // modulators are shared objects in the AC1 bank: copies get none
                p.mods.clear();
            }
            let it = match it {
                Instrument::Wave { audio, inst, .. } => Instrument::Wave { id: new, audio, inst },
                Instrument::Multi { mode, sounds, inst, .. } => Instrument::Multi { id: new, mode, sounds: sounds.iter().map(|(s, w)| (map[s], *w)).collect(), inst },
                Instrument::Scatter { poly, total, tmin, tmax, steal, rate, sound, inst, .. } => {
                    Instrument::Scatter { id: new, poly, total, tmin, tmax, steal, rate, sound: map[&sound], inst }
                }
            };
            self.insts.insert(new, it);
        }
        let n = map[&i];
        self.extra_insts.push(n);
        Ok(n)
    }

    /// Level offset on a bus. An automated volume ignores the bus's own value (Studio: the curves
    /// give the value, several curves add up), so the offset goes onto one of its curves.
    fn gain(&mut self, bus: Guid, db: f32) {
        let Some(b) = self.buses.get_mut(&bus) else { return };
        b.volume += db;
        let Some(c) = b.props.iter().find(|p| p.index == 0).and_then(|p| p.curves.first().copied()) else { return };
        if let Some(cv) = self.curves.get_mut(&c) {
            for p in cv.pts.iter_mut() {
                p.1 = ac1bank::db_fader(ac1bank::fader_db(p.1) + db);
            }
        }
    }

    /// A volume curve on a bus driven by `gp` (points in dB).
    fn gate(&mut self, bus: Guid, gp: Guid, pts: &[(f32, f32)], car: &str) {
        let cid = seeded(car, "gate", &bus);
        let curve = Curve { id: cid, g2: cid, pts: pts.iter().map(|(x, db)| (x.to_bits(), ac1bank::db_fader(*db), 0.0, 0)).collect() };
        self.curves.insert(cid, curve);
        self.curve_param.insert(cid, gp);
        if let Some(b) = self.buses.get_mut(&bus) {
            match b.props.iter_mut().find(|p| p.index == 0) {
                Some(p) => p.curves.push(cid),
                None => b.props.push(Prop { index: 0, a: 2, b: 1, map: g(MAP_BUS_VOLUME), curves: vec![cid], mods: vec![] }),
            }
        }
    }
}

/// AC1 `engine.ini [ENGINE_DATA] LIMITER` (data.acd or data\).
pub fn limiter_rpm(car_dir: &Path) -> Option<f32> {
    let d = crate::ac1::acd::read_car_data(car_dir).ok()?;
    let e = crate::ac1::ini::parse(d.get("engine.ini")?);
    e.num("ENGINE_DATA", "LIMITER").map(|v| v as f32).filter(|v| *v > 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_parameters_remapped() {
        let (a, b) = (Guid([1; 16]), Guid([2; 16]));
        let mut c = term(&a, 0x5020, 0.0, 1.0);
        c.extend_from_slice(&0x1002u32.to_le_bytes());
        c.extend(term(&a, 0x5220, 2.0, 3.0));
        c.extend_from_slice(&0x30u32.to_le_bytes());
        let r = remap_code(&c, &|g: &Guid| if *g == a { b } else { *g });
        assert_eq!(code_params(&r), vec![b, b]);
        assert_eq!(r.len(), c.len());
    }

    #[test]
    fn seeded_ids_depend_on_car_and_role() {
        let g = Guid([7; 16]);
        assert_ne!(seeded("a", "engine_ext", &g), seeded("b", "engine_ext", &g));
        assert_ne!(seeded("a", "engine_ext", &g), seeded("a", "engine_int", &g));
        assert_eq!(seeded("a", "horn", &g), seeded("a", "horn", &g));
    }
}
