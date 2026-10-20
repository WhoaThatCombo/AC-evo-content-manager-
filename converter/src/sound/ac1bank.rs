//! Reader for Assetto Corsa's car sound banks: FMOD Studio 1.08 banks (RIFF `FEV `,
//! FMT version 0x50). The object lists are the ones FMOD Studio 2.03 still writes
//! (docs/FMOD_BANK.md); some items are laid out differently (docs/FMOD_BANK.md,
//! "FMOD Studio 1.08 banks"). The reader turns the bank into the same runtime model
//! `bankread` gives for a 2.03 bank, so `mkfmod` can decompile its events into an FMOD
//! Studio 2.03 project:
//!
//! * sheet placement lives in the parameter layout (instrument, start, length) and is
//!   moved into the instrument's trigger code as its first term, as 2.03 stores it;
//! * event parameter instances become `event ^ parameter` (2.03 derived ids);
//! * curves name their driving parameter in the CTRL item (returned in `curve_param`);
//! * parameter velocity / seek speed become a seek modulator (`seek`);
//! * the event bus list (2.03 EVTB) is rebuilt from the bus graph.

use anyhow::{anyhow, bail, Context, Result};
use std::collections::{BTreeMap, HashMap};

use super::bankread::{self as br, Bank, Bus, Curve, Ctrl, Effect, Event, Inst, Instrument, Layout, Marker, Modu, Node, PVal, Param, Prop, Timeline,
    Tran, R};
use super::bankwrite::prj::Guid;

/// A 1.08 bank in the 2.03 model.
pub struct Ac1Bank {
    pub bank: Bank,
    /// curve -> the parameter driving it
    pub curve_param: HashMap<Guid, Guid>,
    /// parameter -> seek modulator (1.08 seek speed)
    pub seek: HashMap<Guid, Modu>,
    /// FSB5 codec of the bank's samples (2 PCM16, 0x0f Vorbis, 0x10 FADPCM)
    pub codec: u32,
    /// event -> the instruments placed on its timeline / sheets (top level)
    pub event_insts: HashMap<Guid, Vec<Guid>>,
    /// instrument -> its sheet placement (parameter, start, length)
    pub sheet: HashMap<Guid, (Guid, f32, f32)>,
    /// instrument -> its own trigger conditions (without the sheet range)
    pub cond: HashMap<Guid, Vec<u8>>,
}

pub const FORMAT: u32 = 0x50;

fn items<'a>(proj: &'a Node, l: &[u8; 4]) -> Vec<&'a Node> {
    proj.list(l).map(|n| n.kids().iter().filter(|k| k.id() != *b"LCNT").collect()).unwrap_or_default()
}

fn chunk<'a>(it: &'a Node, id: &[u8; 4]) -> Result<&'a [u8]> {
    it.chunk(id).ok_or_else(|| anyhow!("{} item without {}", String::from_utf8_lossy(&it.id()), String::from_utf8_lossy(id)))
}

fn bus(list: [u8; 4], it: &Node, head: &[u8; 4]) -> Result<Bus> {
    let mut r = R::new(chunk(it, head)?);
    let id = r.guid()?;
    let (output, out_mode) = r.sub(|s| Ok((s.guid()?, s.u32()?)))?;
    // 1.08 BUS: u8 flags, u32 input mode, [pre], [post], {f32 volume, f32}, u32 max instances, u32 stealing
    let mut r = R::new(chunk(it, b"BUS ")?);
    let flags = r.u8()? as u32;
    let in_mode = r.u32()?;
    let pre = r.guids()?;
    let post = r.guids()?;
    let volume = r.sub(|s| s.f32())?;
    let max_inst = r.u32()?;
    let steal = r.u32()?;
    Ok(Bus { list, id, output, out_mode, flags, in_mode, pre, post, volume, vcas: Vec::new(), max_inst, steal, props: br::prps(it)? })
}

/// 1.08 INST: as 2.03 but a u8 where 2.03 has its first reserved f32 and the bus block's last
/// word is the output format; it ends after the f32 that follows the start offset (no trigger
/// code on any AC1 bank seen; one is read if the item is longer).
fn inst(b: &[u8]) -> Result<Inst> {
    let mut r = R::new(b);
    let timeline = r.guid()?;
    let volume = r.f32()?;
    let pitch = r.f32()?;
    let loops = r.i32()?;
    let cutoff = r.u32()?;
    let _ = r.u8()?;
    let prob = r.f32()?;
    let dmin = r.f32()?;
    let dmax = r.f32()?;
    let _ = (r.u32()?, r.u32()?);
    let ap_param = r.guid()?;
    let ap_root = r.f32()?;
    let ap_min = r.f32()?;
    let max_voices = r.u32()?;
    let (bus, fmt) = r.sub(|s| Ok((s.guid()?, s.u32()?)))?;
    let steal = r.u32()?;
    let tl_off = r.u32()?;
    let start_off = r.f32()?;
    let _ = r.f32()?;
    let code = if r.left() >= 4 { r.code()? } else { Vec::new() };
    Ok(Inst { timeline, volume, pitch, loops, cutoff, prob, dmin, dmax, ap_param, ap_root, ap_min, max_voices, bus, fmt, steal, tl_off, start_off, code,
        props: Vec::new() })
}

/// 1.08 pan DSP (21 parameters) -> the 2.03 one (24): 2D height blend 0, attenuation range
/// (empty data), override range on (1.08 spatialisers carry their own min / max distance).
fn pan_203(mut p: Vec<PVal>) -> Vec<PVal> {
    if p.len() == 21 {
        p.push(PVal::F(0.0));
        p.push(PVal::D(Vec::new()));
        p.push(PVal::B(true));
    }
    p
}

fn term(gp: &Guid, tag: u32, lo: f32, hi: f32) -> Vec<u8> {
    let mut v = 0x11u32.to_le_bytes().to_vec();
    v.extend_from_slice(&gp.0);
    v.extend_from_slice(&tag.to_le_bytes());
    v.extend_from_slice(&lo.to_le_bytes());
    v.extend_from_slice(&hi.to_le_bytes());
    v
}

/// Trigger code with a sheet range in front of the instrument's own conditions (2.03 form).
pub fn with_sheet(gp: &Guid, lo: f32, hi: f32, pmax: f32, cond: &[u8]) -> Vec<u8> {
    let tag = if hi >= pmax { 0x5020 } else { 0x5220 };
    let mut v = term(gp, tag, lo, hi);
    if !cond.is_empty() {
        v.extend_from_slice(&0x1002u32.to_le_bytes());
        v.extend_from_slice(cond);
        v.extend_from_slice(&0x30u32.to_le_bytes());
    }
    v
}

pub fn read(data: &[u8]) -> Result<Ac1Bank> {
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"FEV " {
        bail!("not an FMOD bank (no RIFF FEV header)");
    }
    let top = br::tree(&data[12..])?;
    let fmt = top.iter().find_map(|n| match n {
        Node::Chunk(i, b) if i == b"FMT " => Some(b.clone()),
        _ => None,
    }).ok_or_else(|| anyhow!("bank without FMT"))?;
    let ver = u32::from_le_bytes(fmt[0..4].try_into().unwrap());
    if ver != FORMAT {
        bail!("bank format version {ver:#x} (the AC1 reader knows {FORMAT:#x} = FMOD Studio 1.08)");
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
    for (l, head) in [(b"IBSS", b"IBSB"), (b"GBSS", b"GBSB"), (b"RBSS", b"RBSB"), (b"MBSS", b"MBSB")] {
        for it in items(proj, l) {
            let b = bus(*l, it, head).with_context(|| format!("{}", String::from_utf8_lossy(l)))?;
            bk.buses.insert(b.id, b);
        }
    }
    // ---- effects: BEFB {guid, u32 speaker mode, u32 DSP type, u8 bypass}
    for it in items(proj, b"BEFX") {
        let mut r = R::new(chunk(it, b"BEFB")?);
        let (id, _mode, dsp) = (r.guid()?, r.u32()?, r.u32()?);
        let bypass = r.u8().unwrap_or(0);
        let mut params = br::pmef(it.chunk(b"PMEF").unwrap_or(&[0, 0, 0, 0]))?;
        if dsp == 27 {
            params = pan_203(params);
        }
        bk.effects.insert(id, Effect::Builtin { id, dsp, params, props: br::prps(it)? });
        bk.effb.insert(id, (bypass as u32 & 1, [100.0, 0.0, -80.0, 0.0]));
    }
    // PEFB {guid, u32 speaker mode, str identifier, str, u8}
    for it in items(proj, b"PEFX") {
        let mut r = R::new(chunk(it, b"PEFB")?);
        let id = r.guid()?;
        let _mode = r.u32()?;
        let ident = r.str()?;
        let params = br::pmef(it.chunk(b"PMEF").unwrap_or(&[0, 0, 0, 0]))?;
        let props = br::prps(it)?;
        // 1.08 shipped some FMOD effects as plugins; 2.03 has them built in
        let e = match ident.as_str() {
            "FMOD Distortion" => Effect::Builtin { id, dsp: 9, params, props },
            _ => Effect::Plugin { id, ident, params, props },
        };
        bk.effects.insert(id, e);
        bk.effb.insert(id, (0, [100.0, 0.0, -80.0, 0.0]));
    }
    for it in items(proj, b"SEFX") {
        let mut r = R::new(chunk(it, b"SEFB")?);
        let (id, ret, level) = (r.guid()?, r.guid()?, r.f32()?);
        bk.effects.insert(id, Effect::Send { id, ret, level, props: br::prps(it)? });
        bk.effb.insert(id, (0, [100.0, 0.0, -80.0, 0.0]));
    }
    for l in [b"SCFX", b"VCAS", b"PRIS", b"EVIS", b"EFIS", b"SNAS"] {
        let n = items(proj, l).len();
        if n > 0 {
            bk.notes.push(format!("{} items not modelled ({n})", String::from_utf8_lossy(l)));
        }
    }
    // ---- events: EVTB {id, snapshot, timeline, mixer input, master bus, u32 max voices, u32 priority,
    //      u8, u32 scheduling, [parameter], [str u32], [str str], f32 doppler, u32 stealing, u32}
    for it in items(proj, b"EVTS") {
        let mut r = R::new(chunk(it, b"EVTB")?);
        let id = r.guid()?;
        let snab = r.guid()?;
        let timeline = r.guid()?;
        let mixer_input = r.guid()?;
        let master_bus = r.guid()?;
        let max_voices = r.u32()?;
        let priority = r.u32()?;
        let steal_flag = r.u8()?;
        let sched = r.u32()?;
        let params = r.guids()?;
        let _nums = r.arr(|s| Ok((s.str()?, s.u32()?)))?;
        let user_props = r.arr(|s| Ok((s.str()?, s.str()?)))?;
        let doppler = r.f32()?;
        let steal = r.u32().unwrap_or(2);
        let mut flags = 0x1441 | 0x20;
        if doppler > 0.0 {
            flags |= 0x100;
        }
        let pinst = params.iter().map(|p| p.xor(&id)).collect();
        bk.events.insert(id, Event { id, snab, timeline, mixer_input, master_bus, max_voices, priority, steal_flag, sched, pinst, user_props,
            doppler: if doppler > 0.0 { doppler } else { 100.0 }, steal: if steal <= 4 { steal } else { 2 }, flags, buses: Vec::new(), min_dist: 1.0,
            max_dist: 20.0, props: br::prps(it)? });
    }
    // ---- timelines: TLNB {guid, guid event, [guid u32 u32], [guid u32 start u32 length], [u32 code],
    //      [guid u32 position str name], [..]} (the first list: non-timelocked instruments); TRNB {guid, guid destination, u32 start,
    //      u32 end, [guid parameter, f32 min, f32 max], u32, u32, f32 probability, u32, u8}
    let mut tl_event: HashMap<Guid, Guid> = HashMap::new();
    for it in items(proj, b"TLNS") {
        let mut r = R::new(chunk(it, b"TLNB")?);
        let id = r.guid()?;
        let ev = r.guid()?;
        tl_event.insert(id, ev);
        // two placement lists: non-timelocked (async) instruments, then timelocked ones
        let mut insts = r.arr(|s| Ok((s.guid()?, s.u32()?, s.u32()?)))?;
        insts.extend(r.arr(|s| Ok((s.guid()?, s.u32()?, s.u32()?)))?);
        let _codes = r.arr(|s| Ok(s.left()))?;
        let markers = r.arr(|s| Ok(Marker { id: s.guid()?, pos: s.u32()?, name: s.str()?, len: 0 }))?;
        let mut trans = Vec::new();
        if let Some(tl) = it.list(b"TRNS") {
            for t in tl.kids().iter().filter(|k| k.id() == *b"TRAN") {
                let mut r = R::new(chunk(t, b"TRNB")?);
                let tid = r.guid()?;
                let dest = r.guid()?;
                let start = r.u32()?;
                let end = r.u32()?;
                // conditions {guid parameter, f32 min, f32 max} -> 2.03 trigger code (continuous, inclusive)
                let conds = r.arr(|s| Ok((s.guid()?, s.f32()?, s.f32()?)))?;
                let mut code = Vec::new();
                for (k, (gp, lo, hi)) in conds.iter().enumerate() {
                    if k > 0 {
                        code.extend_from_slice(&0x1002u32.to_le_bytes());
                    }
                    code.extend(term(gp, 0x5020, *lo, *hi));
                }
                if conds.len() > 1 {
                    code.extend_from_slice(&0x30u32.to_le_bytes());
                }
                // a point is a transition marker, a span a transition region
                trans.push(Tran { id: tid, dest, start, end, code, flags: (start == end) as u32, trtl: None });
            }
        }
        // 1.08 gives a transition back to a marker that marker's own GUID: a separate object in 2.03
        for t in trans.iter_mut() {
            if markers.iter().any(|m| m.id == t.id) {
                t.id = t.id.xor(&Guid([0x7a; 16]));
            }
        }
        bk.timelines.insert(id, Timeline { id, insts, markers, trans });
    }
    // ---- parameters: PRMB {guid, u8 flags, u32 type (0 game, 1 distance, 2 cone angle, 4 direction ...),
    //      str name, f32 min, f32 max, f32 initial, f32 velocity, f32 seek speed, [guid], u8}
    let mut seek = HashMap::new();
    for it in items(proj, b"PRMS") {
        let mut r = R::new(chunk(it, b"PRMB")?);
        let id = r.guid()?;
        let _fl = r.u8()?;
        let ptype = r.u32()?;
        let name = r.str()?;
        let (min, max, init, velocity, speed) = (r.f32()?, r.f32()?, r.f32()?, r.f32()?, r.f32()?);
        if velocity != 0.0 {
            bk.notes.push(format!("parameter {name}: velocity {velocity} not modelled"));
        }
        if speed > 0.0 {
            let mut d = 0u32.to_le_bytes().to_vec();
            d.extend_from_slice(&speed.to_le_bytes());
            d.extend_from_slice(&speed.to_le_bytes());
            seek.insert(id, Modu { id: id.xor(&Guid([0x5e; 16])), target: id, prop: 2, typ: 4, a: 0, b: 1, data: d });
        }
        let flags = if ptype != 0 { 16 } else { 0 };
        bk.params.insert(id, Param { id, flags, ptype, name, min, max, init, labels: Vec::new(), props: br::prps(it)? });
    }
    // ---- parameter layouts: PMLB {guid layout (= parameter), guid parameter, guid event, [guid instrument, f32 start, f32 length]}
    let mut placed: HashMap<Guid, (Guid, f32, f32)> = HashMap::new();
    let mut event_insts: HashMap<Guid, Vec<Guid>> = HashMap::new();
    for it in items(proj, b"PMLS") {
        let mut r = R::new(chunk(it, b"PMLB")?);
        let id = r.guid()?;
        let gp = r.guid()?;
        let ev = r.guid()?;
        let pl = r.arr(|s| Ok((s.guid()?, s.f32()?, s.f32()?)))?;
        let ctrls = it.chunk(b"CTRO").map(|b| R::new(b).guids()).transpose()?.unwrap_or_default();
        for (i, s, l) in &pl {
            placed.insert(*i, (gp, *s, *l));
            event_insts.entry(ev).or_default().push(*i);
        }
        bk.layouts.insert(id.xor(&ev), Layout { id: id.xor(&ev), gp, insts: pl.iter().map(|x| x.0).collect(), global: false, ctrls });
    }
    for tl in bk.timelines.values() {
        if let Some(ev) = tl_event.get(&tl.id) {
            event_insts.entry(*ev).or_default().extend(tl.insts.iter().map(|x| x.0));
        }
    }
    // ---- automation: CTRL {guid, guid target, guid parameter, guid curve, u32 property}
    let mut curve_param = HashMap::new();
    for it in items(proj, b"CTRS") {
        if let Node::Chunk(_, b) = it {
            let mut r = R::new(b);
            let id = r.guid()?;
            let target = r.guid()?;
            let gp = r.guid()?;
            let curve = r.guid()?;
            let prop = r.u32()?;
            curve_param.insert(curve, gp);
            bk.ctrls.insert(id, Ctrl { id, target, curve, prop });
        }
    }
    for it in items(proj, b"CRVS") {
        if let Node::Chunk(_, b) = it {
            let mut r = R::new(b);
            let id = r.guid()?;
            let g2 = r.guid()?;
            let pts = r.arr(|s| Ok((s.u32()?, s.f32()?, s.f32()?, s.u32()?)))?;
            bk.curves.insert(id, Curve { id, g2, pts });
        }
    }
    for it in items(proj, b"MPGS") {
        if let Node::Chunk(_, b) = it {
            let mut r = R::new(b);
            let id = r.guid()?;
            bk.maps.insert(id, r.arr(|s| Ok((s.f32()?, s.f32()?)))?);
        }
    }
    // ---- instruments
    for it in items(proj, b"WAIS") {
        let mut r = R::new(chunk(it, b"WAIB")?);
        let (id, audio) = (r.guid()?, r.guid()?);
        let mut i = inst(chunk(it, b"INST")?).with_context(|| format!("sound {id}"))?;
        i.props = br::prps(it)?;
        bk.instruments.insert(id, Instrument::Wave { id, audio, inst: i });
    }
    for it in items(proj, b"MUIS") {
        let id = R::new(chunk(it, b"MUIB")?).guid()?;
        let mut r = R::new(chunk(it, b"PLST")?);
        let mode = r.u32()?;
        r.u32()?;
        let raw = r.arr(|s| Ok((s.guid()?, s.f32()?)))?;
        let total: f32 = raw.iter().map(|x| x.1.max(0.0)).sum();
        let sounds = raw.iter().map(|(g, w)| (*g, if total > 0.0 { 100.0 * w.max(0.0) / total } else { 100.0 / raw.len().max(1) as f32 })).collect();
        let mut i = inst(chunk(it, b"INST")?).with_context(|| format!("multi sound {id}"))?;
        i.props = br::prps(it)?;
        bk.instruments.insert(id, Instrument::Multi { id, mode, sounds, inst: i });
    }
    for it in items(proj, b"SPIS") {
        let mut r = R::new(chunk(it, b"SPIB")?);
        let id = r.guid()?;
        let (poly, total, tmin, tmax, steal, rate) = (r.u32()?, r.u32()?, r.f32()?, r.f32()?, r.u32()?, r.f32()?);
        let mut r = R::new(chunk(it, b"PLST")?);
        r.u32()?;
        r.u32()?;
        let sound = r.arr(|s| s.guid())?.first().copied().unwrap_or(Guid::ZERO);
        let mut i = inst(chunk(it, b"INST")?).with_context(|| format!("scatterer {id}"))?;
        i.props = br::prps(it)?;
        bk.instruments.insert(id, Instrument::Scatter { id, poly, total, tmin, tmax, steal: steal.clamp(2, 3), rate, sound, inst: i });
    }
    // sheet placement -> the first trigger term
    let mut cond = HashMap::new();
    for ins in bk.instruments.values_mut() {
        let id = ins.id();
        let i = match ins {
            Instrument::Wave { inst, .. } | Instrument::Multi { inst, .. } | Instrument::Scatter { inst, .. } => inst,
        };
        if !i.code.is_empty() {
            cond.insert(id, i.code.clone());
        }
        if let Some((gp, s, l)) = placed.get(&id) {
            let pmax = bk.params.get(gp).map(|p| p.max).unwrap_or(f32::MAX);
            i.code = with_sheet(gp, *s, s + l, pmax, &i.code);
        }
    }
    // ---- modulators: MODU {guid, guid target, u32 property, u32 type, u16 size, data}
    for it in items(proj, b"MODS") {
        let b = match it {
            Node::Chunk(_, b) => b.as_slice(),
            Node::List(..) => chunk(it, b"MODB")?,
        };
        let mut r = R::new(b);
        let (id, target, prop, typ) = (r.guid()?, r.guid()?, r.u32()?, r.u32()?);
        let n = r.u16()? as usize;
        let d = r.bytes(n.min(r.left()))?;
        let f: Vec<f32> = d.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect();
        let (a, bb, data) = match typ {
            // ADSR: initial, peak, sustain, attack, hold, decay, release (s), 3 shapes -> 2.03 (ms) + final value;
            // 1.08 keeps volume levels in the fader-mapped domain, 2.03 in dB
            0 if f.len() >= 7 => {
                let lv = |x: f32| if prop == 0 { fader_db(x) } else { x };
                let mut v = vec![lv(f[0]), lv(f[1]), lv(f[2]), f[3] * 1000.0, f[4] * 1000.0, f[5] * 1000.0, f[6] * 1000.0];
                v.extend(f.iter().skip(7).take(3).copied());
                while v.len() < 10 {
                    v.push(0.0);
                }
                v.push(lv(f[0]));
                (1, 0, v)
            }
            // randomizer: (-amount, +amount) -> amount
            1 if !f.is_empty() => (if prop == 0 { 1 } else { 0 }, 0, vec![f.iter().fold(0f32, |m, x| m.max(x.abs()))]),
            _ => {
                bk.notes.push(format!("modulator {id} type {typ} not modelled"));
                continue;
            }
        };
        bk.mods.insert(id, Modu { id, target, prop, typ, a, b: bb, data: data.iter().flat_map(|x| x.to_le_bytes()).collect() });
    }
    // ---- samples: WAV {guid, {u32 FSB, u32 index, u32}}; SNDH = [(offset, size)] per FSB5
    let mut fsbs: Vec<(u32, u32)> = Vec::new();
    if let Some(b) = proj.chunk(b"SNDH") {
        if !b.is_empty() {
            fsbs = R::new(b).arr(|s| Ok((s.u32()?, s.u32()?)))?;
        }
    }
    let mut base = Vec::new();
    let mut codec = 2;
    for (off, size) in &fsbs {
        let blob = data.get(*off as usize..(*off + *size) as usize).ok_or_else(|| anyhow!("SNDH points outside the bank"))?;
        if blob.len() >= 28 {
            let c = u32::from_le_bytes(blob[24..28].try_into().unwrap());
            // one codec for the bank only when every FSB has it (else PCM16)
            codec = if base.is_empty() || c == codec { c } else { 2 };
        }
        base.push(bk.samples.len() as u32);
        bk.samples.extend(br::fsb(blob).context("bank FSB5")?);
    }
    for it in items(proj, b"WAVS") {
        if let Node::Chunk(_, b) = it {
            let mut r = R::new(b);
            let id = r.guid()?;
            let (f, idx) = r.sub(|s| Ok((s.u32()?, s.u32()?)))?;
            let b0 = base.get(f as usize).copied().ok_or_else(|| anyhow!("audio file {id}: FSB {f} missing"))?;
            bk.waves.insert(id, b0 + idx);
        }
    }
    // ---- event buses (2.03 EVTB lists them): every group / return bus whose output chain reaches the event's master bus
    let outs: BTreeMap<Guid, Guid> = bk.buses.values().map(|b| (b.id, b.output)).collect();
    let reaches = |mut b: Guid, m: Guid| -> bool {
        for _ in 0..64 {
            if b == m {
                return true;
            }
            match outs.get(&b) {
                Some(o) if !o.is_zero() => b = *o,
                _ => return false,
            }
        }
        false
    };
    let cands: Vec<(Guid, [u8; 4])> = bk.buses.values().filter(|b| &b.list == b"GBSS" || &b.list == b"RBSS").map(|b| (b.id, b.list)).collect();
    for ev in bk.events.values_mut() {
        let mut v: Vec<Guid> = cands.iter().filter(|(b, _)| *b != ev.master_bus && reaches(*b, ev.master_bus)).map(|x| x.0).collect();
        v.sort();
        ev.buses = v;
        // spatialiser range -> event min / max distance
        if let Some(mb) = bk.buses.get(&ev.master_bus) {
            for e in mb.pre.iter().chain(mb.post.iter()) {
                if let Some(Effect::Builtin { dsp: 27, params, .. }) = bk.effects.get(e) {
                    if let (Some(PVal::F(lo)), Some(PVal::F(hi)), Some(PVal::F(blend))) = (params.get(12), params.get(13), params.get(17)) {
                        if *blend == 1.0 {
                            ev.min_dist = *lo;
                            ev.max_dist = *hi;
                        }
                    }
                }
            }
        }
    }
    // the in-house writer builds for a 5.1 platform: 7.1 formats become 5.1 (as for the game's banks)
    for b in bk.buses.values_mut() {
        for m in [&mut b.in_mode, &mut b.out_mode] {
            if *m == 7 {
                *m = 6;
            }
        }
    }
    for ins in bk.instruments.values_mut() {
        let i = match ins {
            Instrument::Wave { inst, .. } | Instrument::Multi { inst, .. } | Instrument::Scatter { inst, .. } => inst,
        };
        if i.fmt == 7 {
            i.fmt = 6;
        }
    }
    let _ = Prop::default();
    Ok(Ac1Bank { bank: bk, curve_param, seek, codec, event_insts, sheet: placed, cond })
}

/// Fader-mapped value -> dB (Studio's fader mapping: -42 = -80 dB, identity from -10 up).
pub fn fader_db(x: f32) -> f32 {
    const M: [(f32, f32); 9] = [(-42.0, -80.0), (-39.0, -60.0), (-36.0, -50.0), (-32.0, -40.0), (-27.0, -30.0), (-20.0, -20.0), (-10.0, -10.0), (0.0, 0.0),
        (10.0, 10.0)];
    if x <= M[0].0 {
        return -80.0;
    }
    for w in M.windows(2) {
        if x <= w[1].0 {
            return w[0].1 + (x - w[0].0) / (w[1].0 - w[0].0) * (w[1].1 - w[0].1);
        }
    }
    x
}

/// dB -> fader-mapped value (the inverse of `fader_db`).
pub fn db_fader(y: f32) -> f32 {
    const M: [(f32, f32); 9] = [(-42.0, -80.0), (-39.0, -60.0), (-36.0, -50.0), (-32.0, -40.0), (-27.0, -30.0), (-20.0, -20.0), (-10.0, -10.0), (0.0, 0.0),
        (10.0, 10.0)];
    if y <= M[0].1 {
        return -42.0;
    }
    for w in M.windows(2) {
        if y <= w[1].1 {
            return w[0].0 + (y - w[0].1) / (w[1].1 - w[0].1) * (w[1].0 - w[0].0);
        }
    }
    y
}

/// `GUIDs.txt`: `{guid} path` lines -> path by GUID.
/// (In file order: one event can be listed under two paths.)
pub fn guids_txt(text: &str) -> Vec<(Guid, String)> {
    let mut m = Vec::new();
    for l in text.lines() {
        let mut it = l.split_whitespace();
        if let (Some(g), Some(p)) = (it.next(), it.next()) {
            if let Some(g) = Guid::parse(g) {
                m.push((g, p.to_string()));
            }
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fader_round_trip() {
        for db in [-80.0f32, -60.0, -45.0, -20.0, -3.5, 0.0, 6.0] {
            assert!((fader_db(db_fader(db)) - db).abs() < 1e-3, "{db}");
        }
        assert_eq!(fader_db(-42.0), -80.0);
    }

    #[test]
    fn sheet_term_tags() {
        let g = Guid([3; 16]);
        // ends below the maximum: exclusive; at the maximum: inclusive
        let c = with_sheet(&g, 0.0, 0.5, 1.0, &[]);
        assert_eq!(u32::from_le_bytes(c[20..24].try_into().unwrap()), 0x5220);
        let c = with_sheet(&g, 0.5, 1.0, 1.0, &[]);
        assert_eq!(u32::from_le_bytes(c[20..24].try_into().unwrap()), 0x5020);
        // conditions are joined and the list closed
        let c = with_sheet(&g, 0.5, 1.0, 1.0, &[1, 2, 3, 4]);
        assert_eq!(&c[c.len() - 4..], &0x30u32.to_le_bytes());
    }
}
