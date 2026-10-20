//! Reader for compiled FMOD Studio 2.03 banks (FEV, FMT 0x92): the decoder side of
//! `bankwrite` (docs/FORMATS.md, "FMOD Studio banks"). It parses the runtime bank model -
//! buses, effects, VCAs, events, timelines, parameter layouts, parameters, automation
//! curves, instruments, modulators, snapshots, the FSB5 samples - and the strings bank's
//! path table. `mkfmod` turns that back into an FMOD Studio project.
//!
//! Fields the writer always emits as zero/constant are checked: a bank that sets them uses
//! a feature the writer does not model, and the reader says so in `Bank::notes`.

use anyhow::{anyhow, bail, Context, Result};
use std::collections::{BTreeMap, HashMap};

use super::bankwrite::prj::Guid;

// ------------------------------------------------------------------ cursor
pub struct R<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> R<'a> {
    pub fn new(b: &'a [u8]) -> R<'a> {
        R { b, p: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.p + n > self.b.len() {
            bail!("bank chunk truncated (need {n} at {} of {})", self.p, self.b.len());
        }
        let s = &self.b[self.p..self.p + n];
        self.p += n;
        Ok(s)
    }
    pub fn left(&self) -> usize {
        self.b.len() - self.p
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn guid(&mut self) -> Result<Guid> {
        Ok(Guid(self.take(16)?.try_into().unwrap()))
    }
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        self.take(n)
    }
    pub fn vcount(&mut self) -> Result<u32> {
        let a = self.u16()? as u32;
        if a & 0x8000 == 0 {
            return Ok(a);
        }
        let b = self.u16()? as u32;
        Ok((a & 0x7fff) | b << 15)
    }
    pub fn str(&mut self) -> Result<String> {
        let n = self.vcount()? as usize;
        Ok(String::from_utf8_lossy(self.take(n)?).into_owned())
    }
    /// u16-size-prefixed sub-block.
    pub fn sub<T>(&mut self, f: impl FnOnce(&mut R<'a>) -> Result<T>) -> Result<T> {
        let n = self.u16()? as usize;
        let mut s = R::new(self.take(n)?);
        f(&mut s)
    }
    /// Array (same=1: one element size; same=0: a size before each element).
    pub fn arr<T>(&mut self, mut f: impl FnMut(&mut R<'a>) -> Result<T>) -> Result<Vec<T>> {
        let h = self.vcount()?;
        let (n, same) = ((h >> 1) as usize, h & 1 == 1);
        let mut out = Vec::with_capacity(n);
        if same && n > 0 {
            let sz = self.u16()? as usize;
            for _ in 0..n {
                let mut s = R::new(self.take(sz)?);
                out.push(f(&mut s)?);
            }
        } else if !same {
            for _ in 0..n {
                let sz = self.u16()? as usize;
                let mut s = R::new(self.take(sz)?);
                out.push(f(&mut s)?);
            }
        }
        Ok(out)
    }
    pub fn guids(&mut self) -> Result<Vec<Guid>> {
        self.arr(|r| r.guid())
    }
    /// code: u32 byte length + entries.
    pub fn code(&mut self) -> Result<Vec<u8>> {
        let n = self.u32()? as usize;
        Ok(self.take(n)?.to_vec())
    }
}

// ------------------------------------------------------------------ RIFF tree
#[derive(Debug, Clone)]
pub enum Node {
    Chunk([u8; 4], Vec<u8>),
    List([u8; 4], Vec<Node>),
}

impl Node {
    pub fn id(&self) -> [u8; 4] {
        match self {
            Node::Chunk(i, _) | Node::List(i, _) => *i,
        }
    }
    pub fn kids(&self) -> &[Node] {
        match self {
            Node::List(_, k) => k,
            _ => &[],
        }
    }
    pub fn chunk(&self, id: &[u8; 4]) -> Option<&[u8]> {
        self.kids().iter().find_map(|n| match n {
            Node::Chunk(i, b) if i == id => Some(b.as_slice()),
            _ => None,
        })
    }
    pub fn list(&self, id: &[u8; 4]) -> Option<&Node> {
        self.kids().iter().find(|n| matches!(n, Node::List(i, _) if i == id))
    }
}

pub(crate) fn tree(b: &[u8]) -> Result<Vec<Node>> {
    let mut out = Vec::new();
    let mut p = 0;
    while p + 8 <= b.len() {
        let id: [u8; 4] = b[p..p + 4].try_into().unwrap();
        let n = u32::from_le_bytes(b[p + 4..p + 8].try_into().unwrap()) as usize;
        let body = b.get(p + 8..p + 8 + n).ok_or_else(|| anyhow!("RIFF chunk {} overruns", String::from_utf8_lossy(&id)))?;
        if &id == b"LIST" {
            let t: [u8; 4] = body[..4].try_into().unwrap();
            out.push(Node::List(t, tree(&body[4..])?));
        } else if &id == b"SND " {
            out.push(Node::Chunk(id, Vec::new()));
        } else {
            out.push(Node::Chunk(id, body.to_vec()));
        }
        p += 8 + n + (n & 1);
    }
    Ok(out)
}

// ------------------------------------------------------------------ model
#[derive(Debug, Clone, Default)]
pub struct Prop {
    pub index: u32,
    pub a: u16,
    pub b: u16,
    pub map: Guid,
    pub curves: Vec<Guid>,
    pub mods: Vec<Guid>,
}

#[derive(Debug, Clone)]
pub struct Bus {
    pub list: [u8; 4],
    pub id: Guid,
    pub output: Guid,
    pub out_mode: u32,
    pub flags: u32,
    pub in_mode: u32,
    pub pre: Vec<Guid>,
    pub post: Vec<Guid>,
    pub volume: f32,
    pub vcas: Vec<Guid>,
    pub max_inst: u32,
    pub steal: u32,
    pub props: Vec<Prop>,
}

#[derive(Debug, Clone)]
pub enum PVal {
    F(f32),
    I(i32),
    B(bool),
    D(Vec<u8>),
}

#[derive(Debug, Clone)]
pub enum Effect {
    Builtin { id: Guid, dsp: u32, params: Vec<PVal>, props: Vec<Prop> },
    Plugin { id: Guid, ident: String, params: Vec<PVal>, props: Vec<Prop> },
    Send { id: Guid, ret: Guid, level: f32, props: Vec<Prop> },
}

impl Effect {
    pub fn id(&self) -> Guid {
        match self {
            Effect::Builtin { id, .. } | Effect::Plugin { id, .. } | Effect::Send { id, .. } => *id,
        }
    }
    pub fn props(&self) -> &[Prop] {
        match self {
            Effect::Builtin { props, .. } | Effect::Plugin { props, .. } | Effect::Send { props, .. } => props,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Vca {
    pub id: Guid,
    pub buses: Vec<Guid>,
    pub volume: f32,
}

#[derive(Debug, Clone)]
pub struct Event {
    pub id: Guid,
    pub snab: Guid,
    pub timeline: Guid,
    pub mixer_input: Guid,
    pub master_bus: Guid,
    pub max_voices: u32,
    pub priority: u32,
    pub steal_flag: u8,
    pub sched: u32,
    pub pinst: Vec<Guid>,
    pub user_props: Vec<(String, String)>,
    pub doppler: f32,
    pub steal: u32,
    pub flags: u32,
    pub buses: Vec<Guid>,
    pub min_dist: f32,
    pub max_dist: f32,
    pub props: Vec<Prop>,
}

#[derive(Debug, Clone)]
pub struct Trtl {
    pub length: u32,
    pub src_len: u32,
    pub dst_len: u32,
    pub src: Vec<(Guid, Guid)>,
    pub dst: Vec<(Guid, Guid)>,
}

#[derive(Debug, Clone)]
pub struct Tran {
    pub id: Guid,
    pub dest: Guid,
    pub start: u32,
    pub end: u32,
    pub code: Vec<u8>,
    pub flags: u32,
    pub trtl: Option<Trtl>,
}

#[derive(Debug, Clone)]
pub struct Marker {
    pub id: Guid,
    pub pos: u32,
    pub name: String,
    pub len: u32,
}

#[derive(Debug, Clone)]
pub struct Timeline {
    pub id: Guid,
    pub insts: Vec<(Guid, u32, u32)>,
    pub markers: Vec<Marker>,
    pub trans: Vec<Tran>,
}

#[derive(Debug, Clone)]
pub struct Layout {
    pub id: Guid,
    pub gp: Guid,
    pub insts: Vec<Guid>,
    pub global: bool,
    pub ctrls: Vec<Guid>,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub id: Guid,
    pub flags: u32,
    pub ptype: u32,
    pub name: String,
    pub min: f32,
    pub max: f32,
    pub init: f32,
    pub labels: Vec<String>,
    pub props: Vec<Prop>,
}

#[derive(Debug, Clone)]
pub struct Ctrl {
    pub id: Guid,
    pub target: Guid,
    pub curve: Guid,
    pub prop: u32,
}

#[derive(Debug, Clone)]
pub struct Curve {
    pub id: Guid,
    pub g2: Guid,
    /// x (f32 bits; u32 sample position for transition fades), y, shape, flags
    pub pts: Vec<(u32, f32, f32, u32)>,
}

#[derive(Debug, Clone)]
pub struct Inst {
    pub timeline: Guid,
    pub volume: f32,
    pub pitch: f32,
    pub loops: i32,
    pub cutoff: u32,
    pub prob: f32,
    pub dmin: f32,
    pub dmax: f32,
    pub ap_param: Guid,
    pub ap_root: f32,
    pub ap_min: f32,
    pub max_voices: u32,
    pub bus: Guid,
    pub fmt: u32,
    pub steal: u32,
    pub tl_off: u32,
    pub start_off: f32,
    pub code: Vec<u8>,
    pub props: Vec<Prop>,
}

#[derive(Debug, Clone)]
pub enum Instrument {
    Wave { id: Guid, audio: Guid, inst: Inst },
    Multi { id: Guid, mode: u32, sounds: Vec<(Guid, f32)>, inst: Inst },
    Scatter { id: Guid, poly: u32, total: u32, tmin: f32, tmax: f32, steal: u32, rate: f32, sound: Guid, inst: Inst },
}

impl Instrument {
    pub fn id(&self) -> Guid {
        match self {
            Instrument::Wave { id, .. } | Instrument::Multi { id, .. } | Instrument::Scatter { id, .. } => *id,
        }
    }
    pub fn inst(&self) -> &Inst {
        match self {
            Instrument::Wave { inst, .. } | Instrument::Multi { inst, .. } | Instrument::Scatter { inst, .. } => inst,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Modu {
    pub id: Guid,
    pub target: Guid,
    pub prop: u32,
    pub typ: u32,
    pub a: u32,
    pub b: u32,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Snap {
    pub id: Guid,
    pub priority: u32,
    pub props: Vec<(u32, Guid, u32, f32)>,
    pub blend: u8,
    pub behavior: u32,
    pub intensity: f32,
}

#[derive(Debug, Clone)]
pub struct Sample {
    pub name: String,
    pub rate: u32,
    pub channels: u32,
    pub pcm: Vec<u8>,
    pub sync: Vec<(u32, String)>,
}

#[derive(Default)]
pub struct Bank {
    pub id: Guid,
    pub n_events: u32,
    pub buses: BTreeMap<Guid, Bus>,
    pub effects: BTreeMap<Guid, Effect>,
    pub vcas: BTreeMap<Guid, Vca>,
    pub events: BTreeMap<Guid, Event>,
    pub timelines: BTreeMap<Guid, Timeline>,
    pub layouts: BTreeMap<Guid, Layout>,
    pub params: BTreeMap<Guid, Param>,
    pub ctrls: BTreeMap<Guid, Ctrl>,
    pub curves: BTreeMap<Guid, Curve>,
    pub maps: BTreeMap<Guid, Vec<(f32, f32)>>,
    pub instruments: BTreeMap<Guid, Instrument>,
    pub mods: BTreeMap<Guid, Modu>,
    pub waves: BTreeMap<Guid, u32>,
    pub snaps: BTreeMap<Guid, Snap>,
    /// effect id -> EFFB (flags: bit 0 = bypass; wet mix, base wet, base dry, 0)
    pub effb: BTreeMap<Guid, (u32, [f32; 4])>,
    pub samples: Vec<Sample>,
    pub platform: Option<Guid>,
    /// features the in-house writer does not model (non-default reserved fields, unknown lists)
    pub notes: Vec<String>,
}

pub(crate) fn prps(n: &Node) -> Result<Vec<Prop>> {
    let Some(l) = n.list(b"PRPS") else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    for k in l.kids() {
        if let Node::Chunk(id, b) = k {
            if id == b"PROP" {
                let mut r = R::new(b);
                out.push(Prop { index: r.u32()?, a: r.u16()?, b: r.u16()?, map: r.guid()?, curves: r.guids()?, mods: r.guids()? });
            }
        }
    }
    Ok(out)
}

fn effb(m: &mut BTreeMap<Guid, (u32, [f32; 4])>, id: Guid, it: &Node) -> Result<()> {
    if let Some(b) = it.chunk(b"EFFB") {
        let mut r = R::new(b);
        m.insert(id, (r.u32()?, [r.f32()?, r.f32()?, r.f32()?, r.f32()?]));
    }
    Ok(())
}

pub(crate) fn pmef(b: &[u8]) -> Result<Vec<PVal>> {
    let mut r = R::new(b);
    let n = r.u32()?;
    let mut out = Vec::new();
    for _ in 0..n {
        out.push(match r.u32()? {
            0 => PVal::F(r.f32()?),
            1 => PVal::I(r.i32()?),
            2 => PVal::B(r.u8()? != 0),
            3 => {
                let n = r.vcount()? as usize;
                PVal::D(r.bytes(n)?.to_vec())
            }
            t => bail!("PMEF parameter type {t}"),
        });
    }
    Ok(out)
}

fn inst(b: &[u8], notes: &mut Vec<String>, owner: Guid) -> Result<Inst> {
    let mut r = R::new(b);
    let timeline = r.guid()?;
    let volume = r.f32()?;
    let pitch = r.f32()?;
    let loops = r.i32()?;
    let cutoff = r.u32()?;
    let z1 = r.f32()?;
    let prob = r.f32()?;
    let dmin = r.f32()?;
    let dmax = r.f32()?;
    let (z2, z3) = (r.u32()?, r.u32()?);
    let ap_param = r.guid()?;
    let ap_root = r.f32()?;
    let ap_min = r.f32()?;
    let max_voices = r.u32()?;
    let (bus, fmt) = r.sub(|s| Ok((s.guid()?, s.u32()?)))?;
    let steal = r.u32()?;
    let tl_off = r.u32()?;
    let start_off = r.f32()?;
    let z4 = r.f32()?;
    let code = r.code()?;
    if z1 != 0.0 || z2 != 0 || z3 != 0 || z4 != 0.0 {
        notes.push(format!("instrument {owner}: reserved INST fields set ({z1} {z2} {z3} {z4})"));
    }
    Ok(Inst { timeline, volume, pitch, loops, cutoff, prob, dmin, dmax, ap_param, ap_root, ap_min, max_voices, bus, fmt, steal, tl_off, start_off,
        code, props: Vec::new() })
}

fn bus(list: [u8; 4], it: &Node, head: &[u8; 4]) -> Result<Bus> {
    let h = it.chunk(head).ok_or_else(|| anyhow!("bus item without {}", String::from_utf8_lossy(head)))?;
    let mut r = R::new(h);
    let id = r.guid()?;
    let (output, out_mode) = r.sub(|s| Ok((s.guid()?, s.u32()?)))?;
    let b = it.chunk(b"BUS ").ok_or_else(|| anyhow!("bus item without BUS"))?;
    let mut r = R::new(b);
    let flags = r.u32()?;
    let in_mode = r.u32()?;
    let pre = r.guids()?;
    let post = r.guids()?;
    let volume = r.sub(|s| s.f32())?;
    let vcas = r.guids()?;
    let max_inst = r.u32()?;
    let steal = r.u32()?;
    Ok(Bus { list, id, output, out_mode, flags, in_mode, pre, post, volume, vcas, max_inst, steal, props: prps(it)? })
}

/// Parse a compiled bank (with its samples).
pub fn read(data: &[u8]) -> Result<Bank> {
    read_opts(data, true)
}

/// Parse a compiled bank without decoding the FSB5 samples.
pub fn read_meta(data: &[u8]) -> Result<Bank> {
    read_opts(data, false)
}

fn read_opts(data: &[u8], with_samples: bool) -> Result<Bank> {
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"FEV " {
        bail!("not an FMOD bank (no RIFF FEV header)");
    }
    let top = tree(&data[12..])?;
    let fmt = top.iter().find_map(|n| match n {
        Node::Chunk(i, b) if i == b"FMT " => Some(b.clone()),
        _ => None,
    }).ok_or_else(|| anyhow!("bank without FMT"))?;
    let ver = u32::from_le_bytes(fmt[0..4].try_into().unwrap());
    if ver != 0x92 {
        bail!("bank format version {ver:#x} (the reader knows 0x92 = FMOD Studio 2.03)");
    }
    let proj = top.iter().find(|n| matches!(n, Node::List(t, _) if t == b"PROJ")).ok_or_else(|| anyhow!("bank without PROJ"))?;
    let mut bk = Bank::default();
    if let Some(b) = proj.chunk(b"BNKI") {
        let mut r = R::new(b);
        bk.id = r.guid()?;
        r.u32()?;
        r.u32()?;
        bk.n_events = r.u32()?;
    }
    let items = |l: &[u8; 4]| -> Vec<&Node> {
        proj.list(l).map(|n| n.kids().iter().filter(|k| k.id() != *b"LCNT").collect()).unwrap_or_default()
    };
    let notes = &mut bk.notes;
    for (l, head) in [(b"IBSS", b"IBSB"), (b"GBSS", b"GBSB"), (b"RBSS", b"RBSB"), (b"MBSS", b"MBSB")] {
        for it in items(l) {
            let b = bus(*l, it, head)?;
            bk.buses.insert(b.id, b);
        }
    }
    for it in items(b"PRTS") {
        notes.push(format!("port bus {:?} not modelled", it.id()));
    }
    for it in items(b"BEFX") {
        let mut r = R::new(it.chunk(b"BEFB").ok_or_else(|| anyhow!("BEFF without BEFB"))?);
        let (id, dsp) = (r.guid()?, r.u32()?);
        let params = pmef(it.chunk(b"PMEF").unwrap_or(&[0, 0, 0, 0, 0]))?;
        bk.effects.insert(id, Effect::Builtin { id, dsp, params, props: prps(it)? });
        effb(&mut bk.effb, id, it)?;
    }
    for it in items(b"PEFX") {
        let mut r = R::new(it.chunk(b"PEFB").ok_or_else(|| anyhow!("PEFF without PEFB"))?);
        let id = r.guid()?;
        let ident = r.str()?;
        let params = pmef(it.chunk(b"PMEF").unwrap_or(&[0, 0, 0, 0, 0]))?;
        bk.effects.insert(id, Effect::Plugin { id, ident, params, props: prps(it)? });
        effb(&mut bk.effb, id, it)?;
    }
    for it in items(b"SEFX") {
        let mut r = R::new(it.chunk(b"SEFB").ok_or_else(|| anyhow!("SEFF without SEFB"))?);
        let (id, ret, level) = (r.guid()?, r.guid()?, r.f32()?);
        bk.effects.insert(id, Effect::Send { id, ret, level, props: prps(it)? });
        effb(&mut bk.effb, id, it)?;
    }
    for l in [b"SCFX", b"SSFX"] {
        if !items(l).is_empty() {
            notes.push(format!("{} side-chain effects not modelled", String::from_utf8_lossy(l)));
        }
    }
    for it in items(b"VCAS") {
        let mut r = R::new(it.chunk(b"VCAB").ok_or_else(|| anyhow!("VCA without VCAB"))?);
        let id = r.guid()?;
        let buses = r.guids()?;
        let volume = r.sub(|s| s.f32())?;
        bk.vcas.insert(id, Vca { id, buses, volume });
    }
    for it in items(b"EVTS") {
        let mut r = R::new(it.chunk(b"EVTB").ok_or_else(|| anyhow!("EVNT without EVTB"))?);
        let id = r.guid()?;
        let snab = r.guid()?;
        let timeline = r.guid()?;
        let mixer_input = r.guid()?;
        let master_bus = r.guid()?;
        let max_voices = r.u32()?;
        let priority = r.u32()?;
        let steal_flag = r.u8()?;
        let sched = r.u32()?;
        let pinst = r.guids()?;
        let nums = r.arr(|s| Ok((s.str()?, s.u32()?)))?;
        let user_props = r.arr(|s| Ok((s.str()?, s.str()?)))?;
        if !nums.is_empty() {
            notes.push(format!("event {id}: numeric user properties not modelled"));
        }
        let doppler = r.f32()?;
        let steal = r.u32()?;
        let z = r.u32()?;
        let flags = r.u32()?;
        let buses = r.guids()?;
        let _pids = r.arr(|s| Ok((s.u32()?, s.u32()?)))?;
        let extra = r.guids()?;
        let min_dist = r.f32()?;
        let max_dist = r.f32()?;
        if z != 0 || !extra.is_empty() {
            notes.push(format!("event {id}: reserved EVTB fields set"));
        }
        bk.events.insert(id, Event { id, snab, timeline, mixer_input, master_bus, max_voices, priority, steal_flag, sched, pinst, user_props, doppler, steal,
            flags, buses, min_dist, max_dist, props: prps(it)? });
    }
    for it in items(b"TLNS") {
        let mut r = R::new(it.chunk(b"TLNB").ok_or_else(|| anyhow!("TMLN without TLNB"))?);
        let id = r.guid()?;
        let unused = r.arr(|s| Ok((s.guid()?, s.u32()?, s.u32()?)))?;
        let insts = r.arr(|s| Ok((s.guid()?, s.u32()?, s.u32()?)))?;
        let codes = r.arr(|s| Ok((s.u32()?, s.code()?)))?;
        let markers = r.arr(|s| Ok(Marker { id: s.guid()?, pos: s.u32()?, name: s.str()?, len: s.u32()? }))?;
        let regions = r.arr(|s| Ok(s.bytes(s.left())?.len()))?;
        if !unused.is_empty() || !codes.is_empty() || !regions.is_empty() {
            notes.push(format!("timeline {id}: tempo/region data not modelled"));
        }
        let mut trans = Vec::new();
        if let Some(tl) = it.list(b"TRNS") {
            for t in tl.kids().iter().filter(|k| k.id() == *b"TRAN") {
                let mut r = R::new(t.chunk(b"TRNB").ok_or_else(|| anyhow!("TRAN without TRNB"))?);
                let id = r.guid()?;
                let dest = r.guid()?;
                let start = r.u32()?;
                let end = r.u32()?;
                let code = r.code()?;
                let (z1, z2, f) = (r.u32()?, r.u32()?, r.f32()?);
                let flags = r.u32()?;
                if z1 != 0 || z2 != 0 || f != 100.0 {
                    notes.push(format!("transition {id}: probability/quantisation fields set"));
                }
                let trtl = match t.chunk(b"TRTL") {
                    Some(b) if !b.is_empty() => {
                        let mut r = R::new(b);
                        let length = r.u32()?;
                        for _ in 0..3 {
                            if !r.arr(|s| Ok(s.left()))?.is_empty() {
                                notes.push(format!("transition {id}: transition timeline extras not modelled"));
                            }
                        }
                        let src_len = r.u32()?;
                        let dst_len = r.u32()?;
                        let src = r.arr(|s| Ok((s.guid()?, s.guid()?)))?;
                        let dst = r.arr(|s| Ok((s.guid()?, s.guid()?)))?;
                        Some(Trtl { length, src_len, dst_len, src, dst })
                    }
                    _ => None,
                };
                trans.push(Tran { id, dest, start, end, code, flags, trtl });
            }
        }
        bk.timelines.insert(id, Timeline { id, insts, markers, trans });
    }
    for it in items(b"PMLS") {
        let mut r = R::new(it.chunk(b"PMLB").ok_or_else(|| anyhow!("PMLO without PMLB"))?);
        let id = r.guid()?;
        let gp = r.guid()?;
        let insts = r.guids()?;
        let global = r.u32()? == 1;
        let ctrls = it.chunk(b"CTRO").map(|b| R::new(b).guids()).transpose()?.unwrap_or_default();
        bk.layouts.insert(id, Layout { id, gp, insts, global, ctrls });
    }
    for it in items(b"PRMS") {
        let mut r = R::new(it.chunk(b"PRMB").ok_or_else(|| anyhow!("PARM without PRMB"))?);
        let id = r.guid()?;
        let flags = r.u32()?;
        let ptype = r.u32()?;
        let name = r.str()?;
        let (min, max, init, _z) = (r.f32()?, r.f32()?, r.f32()?, r.f32()?);
        let labels = r.arr(|s| s.str())?;
        bk.params.insert(id, Param { id, flags, ptype, name, min, max, init, labels, props: prps(it)? });
    }
    for it in items(b"CTRS") {
        if let Node::Chunk(_, b) = it {
            let mut r = R::new(b);
            let c = Ctrl { id: r.guid()?, target: r.guid()?, curve: r.guid()?, prop: r.u32()? };
            bk.ctrls.insert(c.id, c);
        }
    }
    for it in items(b"CRVS") {
        if let Node::Chunk(_, b) = it {
            let mut r = R::new(b);
            let id = r.guid()?;
            let g2 = r.guid()?;
            let pts = r.arr(|s| Ok((s.u32()?, s.f32()?, s.f32()?, s.u32()?)))?;
            bk.curves.insert(id, Curve { id, g2, pts });
        }
    }
    for it in items(b"MPGS") {
        if let Node::Chunk(_, b) = it {
            let mut r = R::new(b);
            let id = r.guid()?;
            bk.maps.insert(id, r.arr(|s| Ok((s.f32()?, s.f32()?)))?);
        }
    }
    let bnotes = &mut bk.notes;
    for it in items(b"WAIS") {
        let mut r = R::new(it.chunk(b"WAIB").ok_or_else(|| anyhow!("WAIT without WAIB"))?);
        let (id, audio) = (r.guid()?, r.guid()?);
        let mut i = inst(it.chunk(b"INST").ok_or_else(|| anyhow!("WAIT without INST"))?, bnotes, id)?;
        i.props = prps(it)?;
        bk.instruments.insert(id, Instrument::Wave { id, audio, inst: i });
    }
    for it in items(b"MUIS") {
        let id = R::new(it.chunk(b"MUIB").ok_or_else(|| anyhow!("MUIT without MUIB"))?).guid()?;
        let mut r = R::new(it.chunk(b"PLST").ok_or_else(|| anyhow!("MUIT without PLST"))?);
        let mode = r.u32()?;
        r.u32()?;
        let sounds = r.arr(|s| Ok((s.guid()?, s.f32()?)))?;
        let mut i = inst(it.chunk(b"INST").ok_or_else(|| anyhow!("MUIT without INST"))?, bnotes, id)?;
        i.props = prps(it)?;
        bk.instruments.insert(id, Instrument::Multi { id, mode, sounds, inst: i });
    }
    for it in items(b"SPIS") {
        let mut r = R::new(it.chunk(b"SPIB").ok_or_else(|| anyhow!("SPIT without SPIB"))?);
        let id = r.guid()?;
        let (poly, total, tmin, tmax, steal, rate, z1, z2) = (r.u32()?, r.u32()?, r.f32()?, r.f32()?, r.u32()?, r.f32()?, r.u32()?, r.u32()?);
        if z1 != 0 || z2 != 0 {
            bnotes.push(format!("scatterer {id}: tempo spawning not modelled"));
        }
        let mut r = R::new(it.chunk(b"PLST").ok_or_else(|| anyhow!("SPIT without PLST"))?);
        r.u32()?;
        r.u32()?;
        let sound = r.arr(|s| s.guid())?.first().copied().unwrap_or(Guid::ZERO);
        let mut i = inst(it.chunk(b"INST").ok_or_else(|| anyhow!("SPIT without INST"))?, bnotes, id)?;
        i.props = prps(it)?;
        bk.instruments.insert(id, Instrument::Scatter { id, poly, total, tmin, tmax, steal, rate, sound, inst: i });
    }
    for l in [b"PRIS", b"EVIS", b"EFIS", b"CMDS", b"SLNS", b"LWVS"] {
        if !items(l).is_empty() {
            bnotes.push(format!("{} instruments/items not modelled ({})", String::from_utf8_lossy(l), items(l).len()));
        }
    }
    for it in items(b"WAVS") {
        if let Node::Chunk(_, b) = it {
            let mut r = R::new(b);
            let id = r.guid()?;
            let idx = r.sub(|s| {
                s.u32()?;
                s.u32()
            })?;
            bk.waves.insert(id, idx);
        }
    }
    for it in items(b"SNAS") {
        let mut r = R::new(it.chunk(b"SNAB").ok_or_else(|| anyhow!("SNAP without SNAB"))?);
        let id = r.guid()?;
        let priority = r.u32()?;
        let props = r.arr(|s| Ok((s.u32()?, s.guid()?, s.u32()?, s.f32()?)))?;
        let blend = r.u8()?;
        let behavior = r.u32()?;
        let intensity = r.f32()?;
        bk.snaps.insert(id, Snap { id, priority, props, blend, behavior, intensity });
    }
    for it in items(b"MODS") {
        let b = it.chunk(b"MODB").ok_or_else(|| anyhow!("MODU without MODB"))?;
        let mut r = R::new(b);
        let (id, target, prop, typ, a, bb) = r.sub(|s| Ok((s.guid()?, s.guid()?, s.u32()?, s.u32()?, s.u32()?, s.u32()?)))?;
        let n = r.left();
        bk.mods.insert(id, Modu { id, target, prop, typ, a, b: bb, data: r.bytes(n)?.to_vec() });
    }
    if let Some(b) = proj.chunk(b"PLAT") {
        if b.len() >= 16 {
            bk.platform = Some(R::new(b).guid()?);
        }
    }
    // FSB5 (SNDH: offset, size)
    if let Some(b) = proj.chunk(b"SNDH").filter(|_| with_samples) {
        if !b.is_empty() {
            let v = R::new(b).arr(|s| Ok((s.u32()?, s.u32()?)))?;
            for (off, size) in v {
                let blob = data.get(off as usize..(off + size) as usize).ok_or_else(|| anyhow!("SNDH points outside the bank"))?;
                if blob.len() >= 28 && blob[24..28] != [2, 0, 0, 0] {
                    bk.notes.push(format!("samples are FSB5 codec {:#x} - decoded to PCM16", u32::from_le_bytes(blob[24..28].try_into().unwrap())));
                }
                bk.samples.extend(fsb(blob).context("bank FSB5")?);
            }
        }
    }
    Ok(bk)
}

/// FSB5 (PCM16) -> samples with their sync points.
pub fn fsb(b: &[u8]) -> Result<Vec<Sample>> {
    if b.len() < 60 || &b[0..4] != b"FSB5" {
        bail!("not an FSB5 blob");
    }
    let rd = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    let (n, shs, nts, ds, mode) = (rd(8) as usize, rd(12) as usize, rd(16) as usize, rd(20) as usize, rd(24));
    // compressed banks (the game's are Vorbis): decoded in-house to PCM16, as the AC1 path does
    let decoded = if mode != 2 {
        let info = super::fsb5::parse_blob(b, 0)?;
        match super::fsb5::decode_blob(b, &info) {
            Ok(d) => Some(d),
            // the in-house Vorbis decoder does not take every stream: the game's own fmod.dll does
            Err(e) => {
                let dec = super::fsb5::Decoder::new().with_context(|| format!("FSB5 codec {mode:#x}: in-house decode failed ({e:#}) and fmod.dll is unavailable"))?;
                Some(dec.decode_fsb(b)?.into_iter().map(|x| x.0).collect())
            }
        }
    } else {
        None
    };
    let nt = 60 + shs;
    let data = nt + nts;
    let mut hdrs = Vec::with_capacity(n);
    let mut i = 60;
    for _ in 0..n {
        let v = u64::from_le_bytes(b[i..i + 8].try_into().unwrap());
        i += 8;
        let rc = ((v >> 1) & 0xf) as u32;
        let mut ch = [1u32, 2, 6, 8][((v >> 5) & 3) as usize];
        let off = ((v >> 7) & 0x7ff_ffff) as usize * 32;
        let frames = (v >> 34) as usize;
        let mut sync = Vec::new();
        let mut more = v & 1;
        while more != 0 {
            let z = rd(i);
            i += 4;
            more = (z & 1) as u64;
            let size = ((z >> 1) & 0xff_ffff) as usize;
            let ty = z >> 25;
            let p = &b[i..i + size];
            match ty {
                14 if size >= 4 => ch = u32::from_le_bytes(p[0..4].try_into().unwrap()),
                5 => {
                    for c in p.chunks_exact(4) {
                        sync.push((u32::from_le_bytes(c.try_into().unwrap()), String::new()));
                    }
                }
                4 => {
                    for c in p.chunks_exact(260) {
                        let nm = &c[4..];
                        let e = nm.iter().position(|&x| x == 0).unwrap_or(256);
                        sync.push((u32::from_le_bytes(c[0..4].try_into().unwrap()), String::from_utf8_lossy(&nm[..e]).into_owned()));
                    }
                }
                _ => {}
            }
            i += size;
        }
        let rate = match rc {
            0 => 4000, 1 => 8000, 2 => 11000, 3 => 12000, 4 => 16000, 5 => 22050, 6 => 24000, 7 => 32000, 8 => 44100, 9 => 48000, 10 => 96000,
            _ => 44100,
        };
        hdrs.push((rate, ch, off, frames, sync));
    }
    let mut out = Vec::with_capacity(n);
    for (k, (rate, ch, off, frames, sync)) in hdrs.into_iter().enumerate() {
        let name = if nts == 0 {
            format!("sample_{k}")
        } else {
            let o = nt + rd(nt + 4 * k) as usize;
            let e = b[o..].iter().position(|&x| x == 0).map(|e| o + e).unwrap_or(b.len());
            String::from_utf8_lossy(&b[o..e]).into_owned()
        };
        let len = frames * ch as usize * 2;
        let s = data + off;
        let pcm = match &decoded {
            Some(d) => {
                let mut p = d.get(k).cloned().unwrap_or_default();
                p.resize(len, 0);
                p
            }
            None => b.get(s..s + len).ok_or_else(|| anyhow!("FSB5 sample {k} overruns (data {ds})"))?.to_vec(),
        };
        out.push(Sample { name, rate, channels: ch, pcm, sync });
    }
    Ok(out)
}

/// Strings bank: path -> GUID (STDT radix tree).
pub fn strings(data: &[u8]) -> Result<Vec<(String, Guid)>> {
    let top = tree(&data[12..])?;
    let proj = top.iter().find(|n| matches!(n, Node::List(t, _) if t == b"PROJ")).ok_or_else(|| anyhow!("strings bank without PROJ"))?;
    let b = proj.chunk(b"STDT").ok_or_else(|| anyhow!("strings bank without STDT"))?;
    let mut r = R::new(b);
    r.u32()?;
    let nodes = r.arr(|s| Ok((s.u32()?, s.u32()?)))?;
    let guids = r.guids()?;
    let bl = r.vcount()? as usize;
    let blob = r.bytes(bl)?.to_vec();
    let tri = |r: &mut R| -> Result<Vec<u32>> {
        let n = r.vcount()? as usize;
        let bb = r.bytes(n * 3)?;
        Ok(bb.chunks_exact(3).map(|c| c[0] as u32 | (c[1] as u32) << 8 | (c[2] as u32) << 16).collect())
    };
    let a = tri(&mut r)?;
    let par = tri(&mut r)?;
    let mut out = Vec::new();
    for (gi, &ni) in a.iter().enumerate() {
        let mut parts: Vec<String> = Vec::new();
        let mut k = ni;
        let mut guard = 0;
        while k != 0xffffff && guard < 4096 {
            let (w0, _) = *nodes.get(k as usize).ok_or_else(|| anyhow!("STDT node {k} out of range"))?;
            let off = (w0 & 0xffffff) as usize;
            if off != 0xffffff && off < blob.len() {
                let e = blob[off..].iter().position(|&x| x == 0).map(|e| off + e).unwrap_or(blob.len());
                parts.push(String::from_utf8_lossy(&blob[off..e]).into_owned());
            }
            k = *par.get(k as usize).unwrap_or(&0xffffff);
            guard += 1;
        }
        parts.reverse();
        if let Some(g) = guids.get(gi) {
            out.push((parts.concat(), *g));
        }
    }
    Ok(out)
}

/// path -> guid and guid -> path maps of a strings bank.
pub struct Names {
    pub by_guid: HashMap<Guid, String>,
}

impl Names {
    pub fn new(entries: &[(String, Guid)]) -> Names {
        Names { by_guid: entries.iter().map(|(p, g)| (*g, p.clone())).collect() }
    }
    pub fn path(&self, g: &Guid) -> Option<&str> {
        self.by_guid.get(g).map(|s| s.as_str())
    }
}
