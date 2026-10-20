//! Mixer: buses, effects, sends, VCAs, snapshots, global parameters.

use anyhow::Result;
use std::collections::{HashMap, HashSet};

use super::compile::{prps, unsupported, Ctx, Prop, C_GLOBAL, C_PMAP, C_SNAP, C_SNAPBUS};
use super::effects::{self, g, speaker_mode};
use super::hash;
use super::prj::{Guid, Obj};
use super::w::W;

/// Bus classes whose BUS flags = 1 (mixer level).
fn mixer_level(cls: &str) -> bool {
    matches!(cls, "MixerGroup" | "MixerReturn" | "MixerMaster" | "MixerInput" | "MixerPort")
}

/// Channel count rank of a speaker mode (for max()).
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

/// The output speaker mode a panner forces, if it overrides.
fn panner_override(cx: &Ctx, bus: &Obj) -> Option<u32> {
    let pan = cx.p.of(bus.r1("panner"))?;
    pan.p("overridingOutputFormat").and_then(|v| v.parse::<i64>().ok()).map(speaker_mode)
}

/// Compute (input, output) speaker modes of every bus reachable from the events in the bank.
pub fn formats(cx: &mut Ctx, events: &[&Obj]) -> Result<()> {
    // inputs: bus -> list of input buses' output formats
    let p = cx.p;
    let mut inputs: HashMap<Guid, Vec<u32>> = HashMap::new();
    // event mixer inputs feed their output bus with the event format
    for e in events {
        let fmt = speaker_mode(e.i("outputFormat", 0));
        if let Some(mi) = p.of(e.r1("mixerInput")) {
            cx.fmt.insert(mi.id, (fmt, fmt));
            if let Some(o) = mi.r1("output") {
                inputs.entry(o).or_default().push(fmt);
            }
        }
    }
    // event-internal buses are computed in compile_event; here: mixer groups / returns / master
    // resolve recursively
    let buses: Vec<&Obj> = p.objs.values().filter(|o| matches!(o.class.as_str(), "MixerGroup" | "MixerReturn" | "MixerMaster")).collect();
    for b in &buses {
        if let Some(o) = b.r1("output") {
            inputs.entry(o).or_default();
            let _ = o;
        }
    }
    fn resolve(cx: &mut Ctx, id: Guid, inputs: &HashMap<Guid, Vec<u32>>, by_out: &HashMap<Guid, Vec<Guid>>, depth: u32) -> (u32, u32) {
        if let Some(f) = cx.fmt.get(&id) {
            return *f;
        }
        let o = cx.p.get(&id).unwrap();
        let mut fin = if let Some(v) = o.p("overridingInputFormat").and_then(|v| v.parse::<i64>().ok()) {
            speaker_mode(v)
        } else {
            let mut m = 2u32;
            for f in inputs.get(&id).into_iter().flatten() {
                if rank(*f) > rank(m) {
                    m = *f;
                }
            }
            if depth < 64 {
                for c in by_out.get(&id).cloned().unwrap_or_default() {
                    let (_, co) = resolve(cx, c, inputs, by_out, depth + 1);
                    if rank(co) > rank(m) {
                        m = co;
                    }
                }
            }
            m
        };
        if fin == 0 {
            fin = 2;
        }
        let fout = bus_fout(cx, o, fin, 6);
        cx.fmt.insert(id, (fin, fout));
        (fin, fout)
    }
    let mut by_out: HashMap<Guid, Vec<Guid>> = HashMap::new();
    for b in &buses {
        if let Some(o) = b.r1("output") {
            by_out.entry(o).or_default().push(b.id);
        }
    }
    let ids: Vec<Guid> = buses.iter().map(|b| b.id).collect();
    for id in ids {
        resolve(cx, id, &inputs, &by_out, 0);
    }
    Ok(())
}

pub struct BusSpec {
    pub list: &'static [u8; 4],
    pub head: &'static [u8; 4],
    pub item: &'static [u8; 4],
}

/// Effect chain split into (pre-fader, post-fader) effect objects (fader removed).
pub fn chain<'a>(cx: &Ctx<'a>, bus: &Obj) -> Result<(Vec<&'a Obj>, Vec<&'a Obj>)> {
    let mut pre = Vec::new();
    let mut post = Vec::new();
    let mut after = false;
    if let Some(ch) = cx.p.of(bus.r1("effectChain")) {
        for e in ch.r("effects") {
            let o = cx.obj(e)?;
            if o.class == "MixerBusFader" {
                after = true;
                continue;
            }
            if after { post.push(o) } else { pre.push(o) }
        }
    }
    Ok((pre, post))
}

/// Format after the bus effect chain (a spatialiser outputs `evfmt`).
pub fn chain_out(cx: &Ctx, bus: &Obj, fin: u32, evfmt: u32) -> u32 {
    let mut cur = fin;
    if let Some(ch) = cx.p.of(bus.r1("effectChain")) {
        for e in ch.r("effects") {
            if let Some(o) = cx.p.get(e) {
                if o.class == "SpatialiserEffect" {
                    cur = evfmt;
                }
                // a channel mix with an output grouping changes the format too (as in bus_item)
                if o.class == "ChannelMixEffect" {
                    cur = match o.i("outputGrouping", 0) {
                        1 => 2,
                        2 => 3,
                        3 => 4,
                        4 | 6 => 6,
                        5 => 7,
                        _ => cur,
                    };
                }
            }
        }
    }
    cur
}

/// Bus output format: panner override, else the chain output.
pub fn bus_fout(cx: &Ctx, bus: &Obj, fin: u32, evfmt: u32) -> u32 {
    panner_override(cx, bus).unwrap_or_else(|| chain_out(cx, bus, fin, evfmt))
}

/// Whether a bus panner is emitted as a DSP (post-fader, last): it changes the format,
/// or it pans into a surround output with non-default surround settings.
pub fn panner_dsp<'a>(cx: &Ctx<'a>, bus: &Obj, cur: u32, out: u32) -> Option<&'a Obj> {
    let pan = cx.p.of(bus.r1("panner"))?;
    if cur != out {
        return Some(pan);
    }
    let keys = ["surroundLFELevel", "surroundPanDirection", "stereoPosition", "surroundPanExtent", "stereoSeparation", "LFEUpmixEnabled",
        "stereoToSurroundPanMode", "surroundPanRotation", "stereoAxis"];
    if rank(out) >= 4 && keys.iter().any(|k| pan.p(k).is_some()) {
        return Some(pan);
    }
    // a stereo pan position also needs the DSP (game banks: stereo bus panned to 30)
    if rank(out) >= 2 && pan.f("stereoPosition", 0.0) != 0.0 {
        return Some(pan);
    }
    None
}

/// Serialise a bus item: `<head>` {id, {output, kind, -1}}, BUS, PRPS.
pub fn bus_item(cx: &mut Ctx, spec: &BusSpec, o: &Obj, output: Guid, fin: u32, fout: u32, volume: f32, vcas: &[Guid], cc: u32, d0: u32,
    props: &[Prop]) -> Result<()> {
    let (pre, post) = chain(cx, o)?;
    let mut prefx: Vec<Guid> = pre.iter().map(|e| e.id).collect();
    let mut postfx: Vec<Guid> = post.iter().map(|e| e.id).collect();
    // channel format at each effect's input: format changes after a spatialiser / channel mix
    let mut cur = fin;
    let mut prefmt: Vec<u32> = Vec::new();
    let mut postfmt: Vec<u32> = Vec::new();
    for (i, e) in pre.iter().chain(post.iter()).enumerate() {
        if i < pre.len() { prefmt.push(cur) } else { postfmt.push(cur) }
        effect(cx, e, cur, fout)?;
        cur = match e.class.as_str() {
            "SpatialiserEffect" => fout,
            "ChannelMixEffect" => match e.i("outputGrouping", 0) {
                // FMOD_DSP_CHANNELMIX_OUTPUT: 0 default, 1 all mono, 2 all stereo, 3 all quad, 4 all 5.1, 5 all 7.1
                0 => cur,
                1 => 2,
                2 => 3,
                3 => 4,
                4 => 6,
                5 => 7,
                // 6 = ALLLFE: inputs to the LFE of a 5.1 output (game banks: mono in, 5.1 after)
                6 => 6,
                g => return unsupported(format!("channel mix grouping {g}")),
            },
            _ => cur,
        };
    }
    let pan = panner_dsp(cx, o, cur, fout);
    if let Some(pn) = pan {
        postfx.push(pn.id);
        postfmt.push(cur);
    }
    if let Some(pn) = pan {
        panner(cx, pn, fout)?;
    }
    let flags = mixer_level(&o.class) as u32;
    let mut w = W::new();
    w.list(spec.item, |w| {
        w.chunk(spec.head, |c| {
            c.guid(&o.id).sub(|s| {
                s.guid(&output).u32(fout).i32(-1);
            });
        });
        w.chunk(b"BUS ", |c| {
            c.u32(flags).u32(fin).guids(&prefx).guids(&postfx).sub(|s| {
                s.f32(volume).f32(0.0);
            });
            c.guids(vcas).u32(cc).u32(d0);
            c.arr(&prefmt, |w, x| {
                w.u32(*x);
            });
            c.arr(&postfmt, |w, x| {
                w.u32(*x);
            });
            c.i32(-1).u32(0);
        });
        prps(w, props);
    });
    let _ = &mut prefx;
    cx.push(spec.list, o.id, w.b);
    Ok(())
}

const GBUS: BusSpec = BusSpec { list: b"GBSS", head: b"GBSB", item: b"GBUS" };
const RBUS: BusSpec = BusSpec { list: b"RBSS", head: b"RBSB", item: b"RBUS" };
const MBUS: BusSpec = BusSpec { list: b"MBSS", head: b"MBSB", item: b"MBUS" };
pub const IBUS: BusSpec = BusSpec { list: b"IBSS", head: b"IBSB", item: b"IBUS" };
pub const EGBUS: BusSpec = GBUS;
pub const EMBUS: BusSpec = MBUS;
pub const ERBUS: BusSpec = RBUS;

fn bus_path(cx: &Ctx, o: &Obj) -> Option<String> {
    let mut parts = vec![o.name().to_string()];
    let mut cur = o;
    for _ in 0..32 {
        let out = cx.p.of(cur.r1("output"))?;
        if out.class == "MixerMaster" {
            parts.reverse();
            return Some(format!("bus:/{}", parts.join("/")));
        }
        parts.push(out.name().to_string());
        cur = out;
    }
    None
}

pub fn mixer(cx: &mut Ctx, events: &[&Obj]) -> Result<()> {
    let p = cx.p;
    let _ = events;
    // VCAs: controlled buses
    let mut vca_buses: HashMap<Guid, Vec<Guid>> = HashMap::new();
    for o in p.objs.values() {
        for v in o.r("masters") {
            vca_buses.entry(*v).or_default().push(o.id);
        }
    }
    for b in p.class("MixerGroup").into_iter().chain(p.class("MixerReturn")) {
        let (fin, fout) = cx.fmt.get(&b.id).copied().unwrap_or((2, 2));
        let out = b.r1("output").unwrap_or(Guid::ZERO);
        let props = bus_props(cx, b)?;
        let spec = if b.class == "MixerGroup" { &GBUS } else { &RBUS };
        let d0 = if b.class == "MixerGroup" { 2 } else { 0 };
        let cc = b.p("maxInstances").and_then(|v| v.parse::<u32>().ok()).unwrap_or(0x7fff_ffff);
        bus_item(cx, spec, b, out, fin, fout, b.f("volume", 0.0) as f32, b.r("masters"), cc, d0, &props)?;
        if let Some(path) = bus_path(cx, b) {
            cx.strings.push((path, b.id));
        }
    }
    for m in p.class("MixerMaster") {
        let (fin, fout) = cx.fmt.get(&m.id).copied().unwrap_or((6, 6));
        let props = bus_props(cx, m)?;
        let d0 = m.i("instanceStealing", 2) as u32;
        let cc = m.p("maxInstances").and_then(|v| v.parse::<u32>().ok()).unwrap_or(0x7fff_ffff);
        bus_item(cx, &MBUS, m, Guid::ZERO, fin, fout, m.f("volume", 0.0) as f32, &[], cc, d0, &props)?;
        cx.strings.push(("bus:/".to_string(), m.id));
    }
    for v in p.class("MixerVCA") {
        let mut buses = vca_buses.get(&v.id).cloned().unwrap_or_default();
        buses.sort();
        let mut w = W::new();
        w.list(b"VCA ", |w| {
            w.chunk(b"VCAB", |c| {
                c.guid(&v.id).guids(&buses).sub(|s| {
                    s.f32(v.f("volume", 0.0) as f32).u32(0);
                });
                c.guids(&[]);
            });
            prps(w, &[]);
        });
        cx.push(b"VCAS", v.id, w.b);
        cx.strings.push((format!("vca:/{}", v.name()), v.id));
    }
    snapshots(cx)?;
    Ok(())
}

/// PROPs of a bus (volume automation on mixer buses is not used by our projects).
fn bus_props(cx: &mut Ctx, b: &Obj) -> Result<Vec<Prop>> {
    if !b.r("automators").is_empty() || !b.r("modulators").is_empty() {
        return unsupported(format!("automation on mixer bus {}", b.name()));
    }
    let _ = cx;
    Ok(Vec::new())
}

/// Built-in / plugin / send effect on a bus.
pub fn effect(cx: &mut Ctx, e: &Obj, fin: u32, fout: u32) -> Result<()> {
    let _ = fout;
    match e.class.as_str() {
        "MixerSend" => send(cx, e),
        "SpatialiserEffect" => {
            let mut params = Vec::new();
            effects::pan_params(e, true, 6, &mut params);
            builtin(cx, e, 27, params)
        }
        "MixerBusPanner" => panner(cx, e, fin),
        "PluginEffect" => plugin(cx, e),
        cls => {
            let Some(fx) = effects::fx(cls) else {
                return unsupported(format!("effect {cls}"));
            };
            let params = effects::table_params(e, &fx);
            builtin(cx, e, fx.dsp, params)
        }
    }
}

pub fn panner(cx: &mut Ctx, e: &Obj, out: u32) -> Result<()> {
    let mut params = Vec::new();
    effects::pan_params(e, false, out, &mut params);
    builtin(cx, e, 27, params)
}

/// Snapshot targets: (effect id) -> list of (property index, value, snapshot)
fn snapshot_targets(cx: &Ctx) -> HashMap<Guid, Vec<u32>> {
    let mut m: HashMap<Guid, Vec<u32>> = HashMap::new();
    for sp in cx.p.class("SnapshotProperty") {
        if let (Some(t), Some(name)) = (sp.r1("automatableObject"), sp.p("propertyName")) {
            if let Some(o) = cx.p.get(&t) {
                if let Some(i) = effects::param_index(&o.class, name) {
                    let v = m.entry(t).or_default();
                    if !v.contains(&i) {
                        v.push(i);
                    }
                }
            }
        }
    }
    m
}

/// Effect property automations: (property index, curves, modulators).
fn effect_props(cx: &mut Ctx, e: &Obj) -> Result<Vec<Prop>> {
    let mut props: Vec<Prop> = Vec::new();
    let mut idx: Vec<u32> = snapshot_targets(cx).remove(&e.id).unwrap_or_default();
    let mut curves: HashMap<u32, Vec<Guid>> = HashMap::new();
    for a in e.r("automators") {
        let au = cx.obj(a)?;
        let name = au.p("nameOfPropertyBeingAutomated").unwrap_or("");
        let i = if e.class == "MixerSend" && name == "level" {
            0
        } else {
            match effects::param_index(&e.class, name) {
                Some(i) => i,
                None => return unsupported(format!("automation of {}.{name}", e.class)),
            }
        };
        let cs = au.r("automationCurves").to_vec();
        if cs.is_empty() {
            continue;
        }
        curves.entry(i).or_default().extend(cs);
        if !idx.contains(&i) {
            idx.push(i);
        }
    }
    if !e.r("modulators").is_empty() {
        return unsupported(format!("modulator on effect {}", e.class));
    }
    idx.sort();
    for i in idx {
        let cs = curves.remove(&i).unwrap_or_default();
        for c in &cs {
            super::compile_event::curve(cx, c, &e.id, i)?;
        }
        props.push(cx.prop(&e.class, i, cs, Vec::new())?);
    }
    Ok(props)
}

fn effb(w: &mut W, e: &Obj) {
    // bit 0 = bypass (a loudness meter is always bypassed)
    let flags = (e.class == "LoudnessMeter" || e.b("bypass", false)) as u32;
    w.chunk(b"EFFB", |c| {
        c.u32(flags).f32(100.0).f32(0.0).f32(-80.0).f32(0.0);
    });
}

fn builtin(cx: &mut Ctx, e: &Obj, dsp: u32, params: Vec<(u8, Vec<u8>)>) -> Result<()> {
    let props = effect_props(cx, e)?;
    let mut w = W::new();
    w.list(b"BEFF", |w| {
        w.chunk(b"BEFB", |c| {
            c.guid(&e.id).u32(dsp);
        });
        w.chunk_bytes(b"PMEF", &effects::pmef(&params));
        effb(w, e);
        prps(w, &props);
    });
    cx.push(b"BEFX", e.id, w.b);
    Ok(())
}

fn send(cx: &mut Ctx, e: &Obj) -> Result<()> {
    let props = effect_props(cx, e)?;
    let ret = e.r1("mixerReturn").unwrap_or(Guid::ZERO);
    let level = e.f("level", -80.0) as f32;
    let mut w = W::new();
    w.list(b"SEFF", |w| {
        w.chunk(b"SEFB", |c| {
            c.guid(&e.id).guid(&ret).f32(level);
        });
        effb(w, e);
        prps(w, &props);
    });
    cx.push(b"SEFX", e.id, w.b);
    Ok(())
}

fn plugin(cx: &mut Ctx, e: &Obj) -> Result<()> {
    let pl = cx.p.of(e.r1("plugin"));
    let Some(pl) = pl else { return unsupported("plugin effect without plugin") };
    let ident = pl.p("identifier").unwrap_or("");
    if ident != "FMOD Distance Filter" && !ident.is_empty() {
        return unsupported(format!("plugin {ident}"));
    }
    // parameters in pluginParameters order: float value / empty data
    let mut params: Vec<(u8, Vec<u8>)> = Vec::new();
    for pp in pl.r("pluginParameters") {
        let po = cx.obj(pp)?;
        match po.class.as_str() {
            "FloatPluginParameter" => params.push((0, (po.f("value", 0.0) as f32).to_le_bytes().to_vec())),
            "DataPluginParameter" => params.push((3, Vec::new())),
            c => return unsupported(format!("plugin parameter {c}")),
        }
    }
    if !e.r("automators").is_empty() || !e.r("modulators").is_empty() {
        return unsupported("automated plugin effect");
    }
    let mut w = W::new();
    w.list(b"PEFF", |w| {
        w.chunk(b"PEFB", |c| {
            c.guid(&e.id).str(ident).str("");
        });
        w.chunk_bytes(b"PMEF", &effects::pmef(&params));
        effb(w, e);
        prps(w, &[]);
    });
    cx.push(b"PEFX", e.id, w.b);
    Ok(())
}

fn snapshots(cx: &mut Ctx) -> Result<()> {
    let p = cx.p;
    let snaps = p.class("Snapshot");
    // priority: SnapshotList order, top = highest
    let order: Vec<Guid> = p.class("SnapshotList").first().map(|l| l.r("items").to_vec()).unwrap_or_default();
    for s in snaps {
        cx.n_events += 1;
        let tl = s.r1("timeline").unwrap_or(Guid::ZERO);
        let snab = s.id.xor(&g(C_SNAP));
        let mbus = s.id.xor(&g(C_SNAPBUS));
        let gps = super::compile_event::sheet_params(cx, s)?;
        let pinst: Vec<Guid> = gps.iter().map(|gp| s.id.xor(gp)).collect();
        let pids: Vec<(u32, u32)> = gps.iter().map(hash::public_id).collect();
        // EVTB
        let mut w = W::new();
        w.list(b"EVNT", |w| {
            w.chunk(b"EVTB", |c| {
                c.guid(&s.id).guid(&snab).guid(&tl).guid(&Guid::ZERO).guid(&mbus);
                c.u32(0x7fff_ffff).u32(0x80).u8(0).u32(0);
                c.guids(&pinst);
                c.arr_var(&[] as &[u8], |_, _| {});
                c.arr_var(&[] as &[u8], |_, _| {});
                c.u32(0).u32(2).u32(0).u32(5219);
                c.guids(&[]);
                c.arr(&pids, |w, (a, b)| {
                    w.u32(*a).u32(*b);
                });
                c.guids(&[]);
                c.f32(0.0).f32(0.0);
            });
            prps(w, &[]);
        });
        cx.push(b"EVTS", s.id, w.b);
        // master bus
        let mut w = W::new();
        w.list(b"MBUS", |w| {
            w.chunk(b"MBSB", |c| {
                c.guid(&mbus).sub(|s| {
                    s.guid(&Guid::ZERO).u32(2).i32(-1);
                });
            });
            w.chunk(b"BUS ", |c| {
                c.u32(0).u32(2).guids(&[]).guids(&[]).sub(|s| {
                    s.f32(0.0).f32(0.0);
                });
                c.guids(&[]).u32(0x7fff_ffff).u32(0);
                c.arr(&[] as &[u32], |_, _| {});
                c.arr(&[] as &[u32], |_, _| {});
                c.i32(-1).u32(0);
            });
            prps(w, &[]);
        });
        cx.push(b"MBSS", mbus, w.b);
        // timeline
        super::compile_event::empty_timeline(cx, tl);
        // parameter layouts
        for gp in &gps {
            super::compile_event::layout(cx, s.id.xor(gp), *gp, &[], 0, &[]);
        }
        // SNAB
        let pri = order.iter().position(|x| *x == s.id).map(|i| order.len() - 1 - i).unwrap_or(0) as u32;
        let mut props: Vec<(u32, Guid, u32, f32)> = Vec::new();
        for sp in s.r("snapshotProperties") {
            let sp = cx.obj(sp)?;
            let t = sp.r1("automatableObject").unwrap_or(Guid::ZERO);
            let to = cx.obj(&t)?;
            let name = sp.p("propertyName").unwrap_or("");
            let Some(i) = effects::param_index(&to.class, name) else {
                return unsupported(format!("snapshot property {}.{name}", to.class));
            };
            props.push((t.fold() ^ i, t, i, sp.f("value", 0.0) as f32));
        }
        let (blend, beh) = match s.i("behavior", 0) {
            1 => (1u8, 3u32),
            b => return unsupported(format!("snapshot behavior {b}")),
        };
        let mut w = W::new();
        w.list(b"SNAP", |w| {
            w.chunk(b"SNAB", |c| {
                c.guid(&snab).u32(pri);
                c.arr(&props, |w, (k, t, i, v)| {
                    w.u32(*k).guid(t).u32(*i).f32(*v);
                });
                c.u8(blend).u32(beh).f32(s.f("intensity", 100.0) as f32);
            });
            prps(w, &[]);
        });
        cx.push(b"SNAS", snab, w.b);
        cx.strings.push((format!("snapshot:/{}", s.name()), s.id));
    }
    Ok(())
}

/// Global parameters (PRMS) + layouts for global ones; parameter paths.
pub fn parameters(cx: &mut Ctx) -> Result<()> {
    let p = cx.p;
    let mut used: Vec<Guid> = cx.params_used.iter().copied().collect();
    for gp in p.class("GameParameter") {
        if gp.b("isGlobal", false) && !used.contains(&gp.id) {
            used.push(gp.id);
        }
    }
    used.sort();
    used.dedup();
    let mut presets: HashMap<Guid, &Obj> = HashMap::new();
    for pr in p.class("ParameterPreset") {
        if let Some(gp) = pr.r1("parameter") {
            presets.insert(gp, pr);
        }
    }
    for gpid in used {
        let gp = cx.obj(&gpid)?;
        let pr = presets.get(&gpid).copied();
        let name = pr.map(|x| x.name()).unwrap_or("").to_string();
        let (ptype, flags, discrete) = match param_kind(gp) {
            Some(k) => k,
            None => return unsupported(format!("parameterType {} ({name})", gp.i("parameterType", 0))),
        };
        let labels: Vec<String> = gp.pv("enumerationLabels").to_vec();
        let max = gp.f("maximum", 1.0) - if discrete { 1.0 } else { 0.0 };
        let min = gp.f("minimum", 0.0);
        // cursor modulators (seek) -> MODS + PROP 2
        let mut mods: Vec<Guid> = Vec::new();
        for m in gp.r("modulators") {
            let mo = cx.obj(m)?;
            match mo.class.as_str() {
                "SeekModulator" => {
                    seek(cx, mo, gpid)?;
                    mods.push(mo.id);
                }
                c => return unsupported(format!("parameter modulator {c}")),
            }
        }
        if !gp.r("automators").is_empty() && cx.params_used.contains(&gpid) {
            return unsupported(format!("parameter cursor automation ({name})"));
        }
        let mut props = Vec::new();
        if !mods.is_empty() {
            let map = gpid.xor(&g(C_PMAP));
            cx.dyn_maps.insert(map, vec![(0.0, min as f32), (1.0, max as f32)]);
            props.push(Prop { index: 2, a: 3, b: 0, map, curves: Vec::new(), mods });
        }
        let mut w = W::new();
        w.list(b"PARM", |w| {
            w.chunk(b"PRMB", |c| {
                c.guid(&gpid).u32(flags).u32(ptype).str(&name);
                c.f32(min as f32).f32(max as f32).f32(gp.f("initialValue", 0.0) as f32).f32(0.0);
                c.arr_var(&labels, |w, s| {
                    w.str(s);
                });
            });
            prps(w, &props);
        });
        cx.push(b"PRMS", gpid, w.b);
        if gp.b("isGlobal", false) {
            super::compile_event::layout(cx, gpid.xor(&g(C_GLOBAL)), gpid, &[], 1, &[]);
        }
        if let Some(pr) = pr {
            cx.strings.push((format!("parameter:/{}", preset_path(cx, pr)), gpid));
        }
    }
    // Studio also names every parameter that drives another parameter's cursor
    // (GameParameter automators), whether or not either is in the bank.
    let mut extra: Vec<Guid> = Vec::new();
    for gp in p.class("GameParameter") {
        for a in gp.r("automators") {
            for c in p.get(a).map(|a| a.r("automationCurves").to_vec()).unwrap_or_default() {
                if let Some(t) = p.get(&c).and_then(|c| c.r1("parameter")) {
                    extra.push(t);
                }
            }
        }
    }
    for t in extra {
        if cx.strings.iter().any(|s| s.1 == t) {
            continue;
        }
        if let Some(pr) = presets.get(&t).copied() {
            cx.strings.push((format!("parameter:/{}", preset_path(cx, pr)), t));
        }
    }
    Ok(())
}

/// XML parameterType -> (runtime type, flags, discrete). flags: 1 global, 8 discrete, 16 built-in, 32 labeled.
fn param_kind(gp: &Obj) -> Option<(u32, u32, bool)> {
    let (t, mut f, d) = match gp.i("parameterType", 0) {
        0 => (0, 0, false),
        1 => (0, 8, true),
        2 => (0, 8 | 32, true),
        3 => (1, 16, false),
        6 => (2, 16, false),
        _ => return None,
    };
    if gp.b("isGlobal", false) {
        f |= 1;
    }
    if gp.p("isHeld").is_some() {
        return None;
    }
    Some((t, f, d))
}

/// SeekModulator on a GameParameter cursor -> MODB type 4.
fn seek(cx: &mut Ctx, mo: &Obj, target: Guid) -> Result<()> {
    let mut w = W::new();
    w.list(b"MODU", |w| {
        w.chunk(b"MODB", |c| {
            c.sub(|s| {
                s.guid(&mo.id).guid(&target).u32(2).u32(4).u32(0).u32(mo.i("clockSource", 1) as u32);
            });
            c.u32(mo.b("asymmetric", false) as u32).f32(mo.f("speedAscending", 0.0) as f32).f32(mo.f("speedDescending", 0.0) as f32);
        });
        prps(w, &[]);
    });
    cx.push(b"MODS", mo.id, w.b);
    Ok(())
}

fn preset_path(cx: &Ctx, pr: &Obj) -> String {
    let mut parts = vec![pr.name().to_string()];
    let mut cur = cx.p.of(pr.r1("folder"));
    for _ in 0..32 {
        let Some(f) = cur else { break };
        if f.class != "ParameterPresetFolder" {
            break;
        }
        parts.push(f.name().to_string());
        cur = cx.p.of(f.r1("folder"));
    }
    parts.reverse();
    parts.join("/")
}

pub fn unused(_x: &HashSet<Guid>) {}
