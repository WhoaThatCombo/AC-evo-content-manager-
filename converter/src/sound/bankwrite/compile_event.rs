//! Events: buses, timelines, parameter sheets (layouts), instruments, automation curves.

use anyhow::Result;
use std::collections::{HashMap, HashSet};

use super::compile::{prps, unsupported, Ctx, Prop};
use super::compile_mixer::{self, bus_item, EGBUS, EMBUS, ERBUS, IBUS};
use super::effects::{self, speaker_mode};
use super::fsb;
use super::hash;
use super::prj::{Guid, Obj};
use super::strings;
use super::w::{Code, W};

/// Timeline / sample position unit (Studio stores timeline positions in 48 kHz samples).
pub const TL_RATE: f64 = 48000.0;

/// GameParameters of an event / snapshot's parameter sheets, in `parameters` order.
pub fn sheet_params(cx: &mut Ctx, ev: &Obj) -> Result<Vec<Guid>> {
    let mut out = Vec::new();
    for pp in ev.r("parameters") {
        let pp = cx.obj(pp)?;
        let gp = proxy_param(cx, pp)?;
        cx.params_used.insert(gp);
        out.push(gp);
    }
    Ok(out)
}

/// ParameterProxy.preset -> GameParameter id (the preset may point at the parameter directly).
pub fn proxy_param(cx: &Ctx, pp: &Obj) -> Result<Guid> {
    let Some(pr) = pp.r1("preset") else { return unsupported("parameter sheet without preset") };
    let o = cx.obj(&pr)?;
    match o.class.as_str() {
        "GameParameter" => Ok(o.id),
        "ParameterPreset" => o.r1("parameter").ok_or_else(|| anyhow::anyhow!("preset without parameter")),
        c => unsupported(format!("sheet preset class {c}")),
    }
}

/// A GameParameter reference (curve / condition) resolved through presets.
pub fn param_of(cx: &Ctx, g: Guid) -> Result<Guid> {
    let o = cx.obj(&g)?;
    Ok(if o.class == "ParameterPreset" { o.r1("parameter").unwrap_or(g) } else { g })
}

pub fn empty_timeline(cx: &mut Ctx, id: Guid) {
    timeline_item(cx, id, &[], &[], &[]);
}

fn timeline_item(cx: &mut Ctx, id: Guid, insts: &[(Guid, u32, u32)], markers: &[(Guid, u32, String, u32)], ctrls: &[Guid]) {
    timeline_item_t(cx, id, insts, markers, ctrls, &[]);
}

fn timeline_item_t(cx: &mut Ctx, id: Guid, insts: &[(Guid, u32, u32)], markers: &[(Guid, u32, String, u32)], ctrls: &[Guid], trans: &[Vec<u8>]) {
    let mut w = W::new();
    w.list(b"TMLN", |w| {
        w.chunk(b"TLNB", |c| {
            c.guid(&id);
            c.arr(&[] as &[u8], |_, _| {});
            c.arr(insts, |w, (g, s, l)| {
                w.guid(g).u32(*s).u32(*l);
            });
            c.arr_var(&[] as &[u8], |_, _| {});
            c.arr_var(markers, |w, (g, p, n, l)| {
                w.guid(g).u32(*p).str(n).u32(*l);
            });
            c.arr(&[] as &[u8], |_, _| {});
        });
        w.list(b"TRNS", |w| {
            if !trans.is_empty() {
                w.chunk(b"LCNT", |c| {
                    c.u32(trans.len() as u32);
                });
                for t in trans {
                    w.bytes(t);
                }
            }
        });
        w.chunk(b"CTRO", |c| {
            c.guids(ctrls);
        });
    });
    cx.push(b"TLNS", id, w.b);
}

/// PMLO: parameter layout `id` on GameParameter `gp`, instruments, flag, controllers.
pub fn layout(cx: &mut Ctx, id: Guid, gp: Guid, instruments: &[Guid], flag: u32, ctrls: &[Guid]) {
    let mut ctrls = ctrls.to_vec();
    if flag == 1 {
        if let Some(v) = cx.layout_ctrls.get(&gp) {
            for c in v {
                if !ctrls.contains(c) {
                    ctrls.push(*c);
                }
            }
        }
    }
    ctrls.sort();
    ctrls.dedup();
    let mut instruments = instruments.to_vec();
    instruments.sort();
    instruments.dedup();
    let mut w = W::new();
    w.list(b"PMLO", |w| {
        w.chunk(b"PMLB", |c| {
            c.guid(&id).guid(&gp).guids(&instruments).u32(flag);
        });
        w.chunk(b"CTRO", |c| {
            c.guids(&ctrls);
        });
    });
    cx.push(b"PMLS", id, w.b);
}

/// Piecewise-linear inverse of a mapping (value -> curve domain), in f32 like Studio:
/// t = (v - y0) / (y1 - y0); x = x0 + t * (x1 - x0).
pub(crate) fn unmap(pts: &[(f32, f32)], v: f32) -> f32 {
    if v <= pts[0].1 {
        return pts[0].0;
    }
    let n = pts.len();
    for (i, w) in pts.windows(2).enumerate() {
        let (x0, y0) = w[0];
        let (x1, y1) = w[1];
        if v <= y1 || i == n - 2 {
            if y1 == y0 {
                return x0;
            }
            let t = (v - y0) / (y1 - y0);
            return x0 + t * (x1 - x0);
        }
    }
    pts[n - 1].0
}

/// CTRL + CURV for AutomationCurve `cid` automating property `prop` of `target`;
/// returns the driving GameParameter.
pub fn curve(cx: &mut Ctx, cid: &Guid, target: &Guid, prop: u32) -> Result<Guid> {
    let c = cx.obj(cid)?;
    let tcls = cx.obj(target)?.class.clone();
    let gp = param_of(cx, c.r1("parameter").unwrap_or(Guid::ZERO))?;
    cx.params_used.insert(gp);
    match cx.cur_event {
        Some(e) => cx.event_ctrls.entry((e, gp)).or_default().push(*cid),
        None => cx.layout_ctrls.entry(gp).or_default().push(*cid),
    }
    let m = effects::mapping(&tcls, prop);
    // Points on a discrete (or labeled) parameter are stepped: flag 3, whatever isSCurve says.
    let discrete = matches!(cx.obj(&gp)?.i("parameterType", 0), 1 | 2);
    let mut pts: Vec<(f32, f32, f32, u32)> = Vec::new();
    for pid in c.r("automationPoints") {
        let pt = cx.obj(pid)?;
        let x = pt.f("position", 0.0) as f32;
        let mut y = pt.f("value", 0.0) as f32;
        if let Some((_, _, _, mp)) = m {
            y = unmap(mp, y);
        }
        let shape = pt.f("curveShape", 0.0) as f32;
        let s = if discrete { 3 } else { pt.b("isSCurve", false) as u32 };
        pts.push((x, y, shape, s));
    }
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut w = W::new();
    w.chunk(b"CTRL", |k| {
        k.guid(cid).guid(target).guid(cid).u32(prop);
    });
    cx.push(b"CTRS", *cid, w.b);
    let mut w = W::new();
    w.chunk(b"CURV", |k| {
        k.guid(cid).guid(cid).arr(&pts, |w, (x, y, z, s)| {
            w.f32(*x).f32(*y).f32(*z).u32(*s);
        });
    });
    cx.push(b"CRVS", *cid, w.b);
    Ok(gp)
}

/// Speaker mode of an instrument's audio.
fn inst_fmt(cx: &Ctx, o: &Obj) -> Result<u32> {
    Ok(match o.class.as_str() {
        "SingleSound" => {
            let af = cx.p.of(o.r1("audioFile"));
            let ch = af.map(|a| a.i("channelCount", 1)).unwrap_or(1);
            ch_mode(ch)
        }
        "MultiSound" => {
            let mut m = 2;
            for s in o.r("sounds") {
                let f = inst_fmt(cx, cx.obj(s)?)?;
                if rank(f) > rank(m) {
                    m = f;
                }
            }
            m
        }
        "SoundScatterer" => match o.r1("sound") {
            Some(ms) => inst_fmt(cx, cx.obj(&ms)?)?,
            None => 2,
        },
        _ => 2,
    })
}

fn ch_mode(ch: i64) -> u32 {
    match ch {
        1 => 2,
        2 => 3,
        4 => 4,
        5 => 5,
        6 => 6,
        8 => 7,
        _ => 2,
    }
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

struct Ev<'a> {
    o: &'a Obj,
    /// instrument id -> (track bus id)
    inst_bus: HashMap<Guid, Guid>,
    /// instrument id -> sheet GameParameter (None = timeline)
    inst_sheet: HashMap<Guid, Option<Guid>>,
    timeline: Guid,
}

pub fn events(cx: &mut Ctx, events: &[&Obj], samples: &mut Vec<(Guid, fsb::Sample)>) -> Result<()> {
    let mut audio: HashSet<Guid> = HashSet::new();
    for e in events {
        event(cx, e, &mut audio)?;
    }
    // audio files
    for af in audio {
        let a = cx.obj(&af)?;
        let path = a.p("assetPath").unwrap_or("");
        let file = cx.p.dir.join("assets").join(path.replace('/', "\\"));
        let bytes = std::fs::read(&file).map_err(|e| anyhow::anyhow!("audio {}: {e}", file.display()))?;
        let wav = fsb::read_wav(&bytes).map_err(|e| anyhow::anyhow!("{}: {e}", file.display()))?;
        let name = std::path::Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        samples.push((af, fsb::Sample { name, rate: wav.rate, channels: wav.channels, pcm: wav.pcm, sync: wav.sync }));
    }
    Ok(())
}

fn event_path(cx: &Ctx, e: &Obj) -> String {
    let mut parts = vec![e.name().to_string()];
    let mut cur = cx.p.of(e.r1("folder"));
    for _ in 0..64 {
        let Some(f) = cur else { break };
        if f.class != "EventFolder" {
            break;
        }
        parts.push(f.name().to_string());
        cur = cx.p.of(f.r1("folder"));
    }
    parts.reverse();
    format!("event:/{}", parts.join("/"))
}

fn event(cx: &mut Ctx, e: &Obj, audio: &mut HashSet<Guid>) -> Result<()> {
    cx.n_events += 1;
    cx.cur_event = Some(e.id);
    let p = cx.p;
    let tl = e.r1("timeline").unwrap_or(Guid::ZERO);
    let mut ev = Ev { o: e, inst_bus: HashMap::new(), inst_sheet: HashMap::new(), timeline: tl };
    // tracks: master + groups
    let mt = cx.obj(&e.r1("masterTrack").unwrap_or(Guid::ZERO))?;
    let master_bus = mt.r1("mixerGroup").unwrap_or(Guid::ZERO);
    let mut tracks: Vec<&Obj> = vec![mt];
    for t in e.r("groupTracks") {
        tracks.push(cx.obj(t)?);
    }
    for t in e.r("returnTracks") {
        tracks.push(cx.obj(t)?);
    }
    for t in &tracks {
        let bus = t.r1("mixerGroup").or(t.r1("mixerReturn")).unwrap_or(Guid::ZERO);
        for m in t.r("modules") {
            ev.inst_bus.insert(*m, bus);
        }
    }
    // sheets
    let gps = sheet_params(cx, e)?;
    let mut sheet_insts: Vec<(Guid, Vec<Guid>)> = Vec::new();
    for (pp, gp) in e.r("parameters").iter().zip(gps.iter()) {
        let pp = cx.obj(pp)?;
        let mods: Vec<Guid> = pp.r("modules").to_vec();
        for m in &mods {
            ev.inst_sheet.insert(*m, Some(*gp));
        }
        sheet_insts.push((*gp, mods));
    }
    let tlo = cx.obj(&tl)?;
    for m in tlo.r("modules") {
        ev.inst_sheet.insert(*m, None);
    }
    // ---- buses
    let ifmt: HashMap<Guid, u32> = {
        let mut m = HashMap::new();
        for (i, b) in &ev.inst_bus {
            let f = inst_fmt(cx, cx.obj(i)?)?;
            let e = m.entry(*b).or_insert(2u32);
            if rank(f) > rank(*e) {
                *e = f;
            }
        }
        m
    };
    let out_fmt = speaker_mode(e.i("outputFormat", 0));
    // group buses (EventMixerGroup / EventMixerReturn) and their outputs
    let mut buses: Vec<&Obj> = Vec::new();
    for t in &tracks[1..] {
        if let Some(b) = t.r1("mixerGroup").or(t.r1("mixerReturn")) {
            buses.push(cx.obj(&b)?);
        }
    }
    let mbus = cx.obj(&master_bus)?;
    fn bus_in(cx: &Ctx, id: Guid, buses: &[&Obj], ifmt: &HashMap<Guid, u32>, depth: u32, ev: u32) -> u32 {
        let o = cx.p.get(&id).unwrap();
        if let Some(v) = o.p("overridingInputFormat").and_then(|v| v.parse::<i64>().ok()) {
            return speaker_mode(v);
        }
        let mut m = ifmt.get(&id).copied().unwrap_or(2);
        if depth < 32 {
            for b in buses {
                if b.r1("output") == Some(id) {
                    let f = bus_out(cx, b.id, buses, ifmt, depth + 1, ev);
                    if rank(f) > rank(m) {
                        m = f;
                    }
                }
            }
        }
        m
    }
    fn bus_out(cx: &Ctx, id: Guid, buses: &[&Obj], ifmt: &HashMap<Guid, u32>, depth: u32, ev: u32) -> u32 {
        let fin = bus_in(cx, id, buses, ifmt, depth, ev);
        let o = cx.p.get(&id).unwrap();
        compile_mixer::bus_fout(cx, o, fin, ev)
    }
    for b in &buses {
        let fin = bus_in(cx, b.id, &buses, &ifmt, 0, out_fmt);
        let fout = bus_out(cx, b.id, &buses, &ifmt, 0, out_fmt);
        cx.fmt.insert(b.id, (fin, fout));
    }
    let mfin = bus_in(cx, master_bus, &buses, &ifmt, 0, out_fmt);
    let mfout = compile_mixer::bus_fout(cx, mbus, mfin, out_fmt);
    cx.fmt.insert(master_bus, (mfin, mfout));
    // mixer input
    let mi = cx.obj(&e.r1("mixerInput").unwrap_or(Guid::ZERO))?;
    let mi_out = mi.r1("output").unwrap_or(Guid::ZERO);
    bus_item(cx, &IBUS, mi, mi_out, out_fmt, out_fmt, mi.f("volume", 0.0) as f32, &[], 0x7fff_ffff, 0, &[])?;
    // event buses
    for b in &buses {
        let (fin, fout) = cx.fmt[&b.id];
        let props = event_bus_props(cx, b)?;
        let out = b.r1("output").unwrap_or(master_bus);
        let spec = if b.class == "EventMixerReturn" { &ERBUS } else { &EGBUS };
        bus_item(cx, spec, b, out, fin, fout, b.f("volume", 0.0) as f32, &[], 0x7fff_ffff, 2, &props)?;
    }
    let props = event_bus_props(cx, mbus)?;
    bus_item(cx, &EMBUS, mbus, Guid::ZERO, mfin, mfout, mbus.f("volume", 0.0) as f32, &[], 0x7fff_ffff, 2, &props)?;
    // ---- instruments
    let mut tl_insts: Vec<(Guid, u32, u32)> = Vec::new();
    let all_insts: Vec<Guid> = ev.inst_bus.keys().copied().collect();
    for i in &all_insts {
        let o = cx.obj(i)?;
        if matches!(o.class.as_str(), "TransitionSourceSound" | "TransitionDestinationSound") {
            continue;
        }
        instrument(cx, &ev, o, audio)?;
        if ev.inst_sheet.get(i).copied().flatten().is_none() {
            let s = (o.f("start", 0.0) * TL_RATE).round() as u32;
            let l = (o.f("length", 0.0) * TL_RATE).round() as u32;
            tl_insts.push((*i, s, l));
        }
    }
    tl_insts.sort_by_key(|x| x.0);
    // ---- timeline: markers / transitions grouped by marker track (Event.markerTracks order),
    // GUID-descending inside a track
    let on_tl: Vec<&Obj> = cx.p.objs.values().filter(|x| x.r1("timeline") == Some(tl) && x.class != "Event").collect();
    let mut markers: Vec<(Guid, u32, String, u32)> = Vec::new();
    let mut trans: Vec<Vec<u8>> = Vec::new();
    let tracks_m: Vec<Guid> = e.r("markerTracks").to_vec();
    let sec = |v: f64| (v * TL_RATE).round() as u32;
    let by_track = |mt: &Guid| {
        let mut ms: Vec<&Obj> = on_tl.iter().copied().filter(|x| x.r1("markerTrack") == Some(*mt)).collect();
        ms.sort_by(|a, b| strings::fmod_guid_key(&b.id).cmp(&strings::fmod_guid_key(&a.id)));
        ms
    };
    for mt in &tracks_m {
        for mo in by_track(mt) {
            match mo.class.as_str() {
                "NamedMarker" => markers.push((mo.id, sec(mo.f("position", 0.0)), mo.name().to_string(), 0)),
                "LoopRegion" => markers.push((mo.id, sec(mo.f("position", 0.0)), String::new(), sec(mo.f("length", 0.0)))),
                _ => {}
            }
        }
    }
    // transitions: regions + markers + loop-backs in track order
    for mt in &tracks_m {
        for mo in by_track(mt) {
            match mo.class.as_str() {
                "TransitionRegion" | "TransitionMarker" | "LoopRegion" => trans.push(transition(cx, e, mo, sec)?),
                "NamedMarker" => {}
                c => return unsupported(format!("timeline marker {c}")),
            }
        }
    }
    let mut tctrls = std::mem::take(&mut cx.timeline_ctrls);
    tctrls.sort();
    timeline_item_t(cx, tl, &tl_insts, &markers, &tctrls, &trans);
    // ---- EVTB
    let mut pinst: Vec<(Guid, Guid)> = gps.iter().map(|gp| (e.id.xor(gp), *gp)).collect();
    pinst.sort();
    let pids: Vec<(u32, u32)> = pinst.iter().map(|(_, gp)| hash::public_id(gp)).collect();
    let pinst: Vec<Guid> = pinst.into_iter().map(|x| x.0).collect();
    let mut evbuses: Vec<Guid> = buses.iter().map(|b| b.id).collect();
    evbuses.sort();
    let ap = p.of(e.r1("automatableProperties"));
    let apf = |k: &str, d: f64| ap.map(|a| a.f(k, d)).unwrap_or(d);
    let api = |k: &str, d: i64| ap.map(|a| a.i(k, d)).unwrap_or(d);
    let apb = |k: &str, d: bool| ap.map(|a| a.b(k, d)).unwrap_or(d);
    let (steal, steal_flag) = match api("voiceStealing", 0) {
        0 => (2u32, 0u8),
        1 => (3, 0),
        2 => (1, 0),
        3 => (0, 1),
        4 => (4, 0),
        v => return unsupported(format!("voiceStealing {v}")),
    };
    let prio = (4 - api("priority", 2).clamp(0, 4)) as u32 * 64;
    let doppler = apb("dopplerEnabled", false);
    let persistent = apb("isPersistent", false);
    // 0x20 = 3D: a spatialiser in the event, or a built-in (distance / cone angle / ...) parameter in use
    let spatial = p.objs.values().any(|o| o.file == e.file && matches!(o.class.as_str(), "SpatialiserEffect" | "ObjectSpatialiserEffect"));
    // (only parameters something in the event refers to - an unused sheet proxy does not count)
    let resolve = |g: Guid| -> Option<&Obj> {
        let o = p.of(Some(g))?;
        match o.class.as_str() {
            "GameParameter" => Some(o),
            "ParameterPreset" => p.of(o.r1("parameter")),
            "ParameterProxy" => p.of(o.r1("preset")).and_then(|pr| if pr.class == "GameParameter" { Some(pr) } else { p.of(pr.r1("parameter")) }),
            _ => None,
        }
    };
    let builtin = p.objs.values().filter(|o| o.file == e.file && o.class != "Event").flat_map(|o| o.r("parameter").iter().copied())
        .any(|g| resolve(g).map(|gp| gp.i("parameterType", 0) >= 3).unwrap_or(false));
    let mut flags: u32 = 0x1441;
    if spatial || builtin {
        flags |= 0x20;
    }
    if persistent {
        flags |= 2;
    }
    if doppler {
        flags |= 0x100;
    }
    let maxv = ap.and_then(|a| a.p("maxVoices")).and_then(|v| v.parse::<u32>().ok()).unwrap_or(0x7fff_ffff);
    let mut evprops: Vec<Prop> = Vec::new();
    if let Some(ap) = ap {
        let mut by: Vec<(u32, Vec<Guid>)> = Vec::new();
        for a in ap.r("automators") {
            let au = cx.obj(a)?;
            let i = match au.p("nameOfPropertyBeingAutomated").unwrap_or("") {
                "minimumDistance" => 30,
                "maximumDistance" => 31,
                n => return unsupported(format!("event property automation {n}")),
            };
            let cs = au.r("automationCurves").to_vec();
            match by.iter_mut().find(|x| x.0 == i) {
                Some(x) => x.1.extend(cs),
                None => by.push((i, cs)),
            }
        }
        by.sort_by_key(|x| x.0);
        cx.cur_event = Some(e.id);
        for (i, cs) in by {
            for c in &cs {
                curve(cx, c, &e.id, i)?;
            }
            evprops.push(cx.prop("Event", i, cs, Vec::new())?);
        }
    }
    // scheduling mode (ModelBuilder: calculateSchedulingModeForEvent): 3 noLookahead,
    // 1 lookahead when the event has an EventSound / SoundScatterer / looping MultiSound or
    // more than one instrument, else 0 (streaming-asset mode 2 is rejected earlier)
    let e4 = if apb("noLookahead", false) {
        3u32
    } else {
        let mut n = 0;
        let mut force = false;
        for i in ev.inst_bus.keys() {
            let o = cx.obj(i)?;
            match o.class.as_str() {
                "TransitionSourceSound" | "TransitionDestinationSound" | "SnapshotModule" => {}
                "EventSound" | "SoundScatterer" => force = true,
                "MultiSound" if o.b("looping", false) => force = true,
                _ => n += 1,
            }
        }
        (force || n > 1) as u32
    };
    let mut w = W::new();
    w.list(b"EVNT", |w| {
        w.chunk(b"EVTB", |c| {
            c.guid(&e.id).guid(&Guid::ZERO).guid(&tl).guid(&mi.id).guid(&master_bus);
            c.u32(maxv).u32(prio).u8(steal_flag).u32(e4);
            c.guids(&pinst);
            c.arr_var(&[] as &[u8], |_, _| {});
            c.arr_var(&[] as &[u8], |_, _| {});
            c.f32(if doppler { apf("dopplerScale", 100.0) as f32 } else { 0.0 }).u32(steal).u32(0).u32(flags);
            c.guids(&evbuses);
            c.arr(&pids, |w, (a, b)| {
                w.u32(*a).u32(*b);
            });
            c.guids(&[]);
            c.f32(apf("minimumDistance", 1.0) as f32).f32(apf("maximumDistance", 20.0) as f32);
        });
        prps(w, &evprops);
    });
    cx.push(b"EVTS", e.id, w.b);
    // ---- sheets (after all curves of the event are known)
    let insts_here: HashSet<Guid> = ev.inst_bus.keys().copied().collect();
    for (gp, mods) in &sheet_insts {
        let mut ins = mods.clone();
        for (o, cgp) in &cx.cond_params {
            if cgp == gp && insts_here.contains(o) {
                ins.push(*o);
            }
        }
        let ctrls = cx.event_ctrls.get(&(e.id, *gp)).cloned().unwrap_or_default();
        layout(cx, e.id.xor(gp), *gp, &ins, 0, &ctrls);
    }
    let path = event_path(cx, e);
    cx.strings.push((path, e.id));
    let _ = (&ev.o, ev.timeline);
    cx.cur_event = None;
    Ok(())
}

pub const C_LOOPTR: &str = "{071550f9-589b-4f5f-9936-a8a83cbc041e}";

/// Trigger condition code entries; discrete parameters use [min, max + 1) with an exclusive bound.
/// `prior` = terms already in `code` (the parameter-sheet range). Studio's layout: with a prior
/// term, 0x1002 before the conditions and 0x30 after them; each condition after the first is
/// preceded by the join (0x1002 all, 0x1102 any = triggerConditionMode 1) and followed by 0x30.
fn conditions(cx: &mut Ctx, o: &Obj, code: &mut Code, prior: usize) -> Result<()> {
    let n = o.r("triggerConditions").len();
    let join = if o.i("triggerConditionMode", 0) == 0 { 0x1002 } else { 0x1102 };
    if n > 0 && prior > 0 {
        code.raw(0x1002);
    }
    for (k, tc) in o.r("triggerConditions").iter().enumerate() {
        if k > 0 {
            code.raw(join);
        }
        let c = cx.obj(tc)?;
        if c.class != "ParameterCondition" {
            return unsupported(format!("trigger condition {}", c.class));
        }
        let gp = param_of(cx, c.r1("parameter").unwrap_or(Guid::ZERO))?;
        cx.params_used.insert(gp);
        let lo = c.f("minimum", 0.0) as f32;
        let hi = c.f("maximum", 0.0) as f32;
        let discrete = matches!(cx.obj(&gp)?.i("parameterType", 0), 1 | 2);
        // isInverted adds 0x100 (Studio: 0x5320 discrete, 0x5120 continuous)
        let inv = if c.b("isInverted", false) { 0x100 } else { 0 };
        if discrete {
            code.param(&gp).range(0x5220 | inv, lo, hi + 1.0);
        } else {
            code.param(&gp).range(0x5020 | inv, lo, hi);
        }
        cx.cond_params.push((o.id, gp));
        if k > 0 {
            code.raw(0x30);
        }
    }
    if n > 0 && prior > 0 {
        code.raw(0x30);
    }
    Ok(())
}

/// TRAN item for a TransitionRegion / TransitionMarker / LoopRegion (loop-back).
fn transition(cx: &mut Ctx, e: &Obj, mo: &Obj, sec: impl Fn(f64) -> u32) -> Result<Vec<u8>> {
    let mut code = Code::default();
    conditions(cx, mo, &mut code, 0)?;
    let (id, dest, a, b, flags) = match mo.class.as_str() {
        "TransitionRegion" => {
            let p = mo.f("position", 0.0);
            (mo.id, mo.r1("destination").unwrap_or(Guid::ZERO), sec(p), sec(p + mo.f("length", 0.0)), 0u32)
        }
        "TransitionMarker" => {
            let p = sec(mo.f("position", 0.0));
            (mo.id, mo.r1("destination").unwrap_or(Guid::ZERO), p, p, 1)
        }
        _ => {
            let end = sec(mo.f("position", 0.0) + mo.f("length", 0.0));
            let f = if mo.r1("transitionTimeline").is_some() { 0 } else { 2 };
            (mo.id.xor(&super::effects::g(C_LOOPTR)), mo.id, end, end, f)
        }
    };
    if mo.p("probability").is_some() || mo.p("quantizationInterval").is_some() {
        return unsupported("transition probability / quantization");
    }
    let trtl = match mo.r1("transitionTimeline") {
        Some(t) => transition_timeline(cx, e, cx.obj(&t)?, &sec)?,
        None => Vec::new(),
    };
    let mut w = W::new();
    w.list(b"TRAN", |w| {
        w.chunk(b"TRNB", |c| {
            c.guid(&id).guid(&dest).u32(a).u32(b);
            code.write(c);
            c.u32(0).u32(0).f32(100.0).u32(flags);
        });
        w.chunk_bytes(b"TRTL", &trtl);
    });
    Ok(w.b)
}

/// TRTL body: transition timeline sounds (bus, sound) pairs and their fade curves.
fn transition_timeline(cx: &mut Ctx, e: &Obj, tt: &Obj, sec: &impl Fn(f64) -> u32) -> Result<Vec<u8>> {
    let mut bus_of: HashMap<Guid, Guid> = HashMap::new();
    let mt = cx.obj(&e.r1("masterTrack").unwrap_or(Guid::ZERO))?;
    let mut tracks: Vec<&Obj> = vec![mt];
    for t in e.r("groupTracks") {
        tracks.push(cx.obj(t)?);
    }
    for t in &tracks {
        if let Some(b) = t.r1("mixerGroup") {
            for m in t.r("modules") {
                bus_of.insert(*m, b);
            }
        }
    }
    let mut src: Vec<(Guid, Guid)> = Vec::new();
    let mut dst: Vec<(Guid, Guid)> = Vec::new();
    let (mut src_len, mut dst_len) = (0u32, 0u32);
    for m in tt.r("modules") {
        let so = cx.obj(m)?;
        let bus = bus_of.get(m).copied().unwrap_or(Guid::ZERO);
        let len = sec(so.f("length", 0.0));
        let curve_rel = match so.class.as_str() {
            "TransitionSourceSound" => {
                src_len = src_len.max(len);
                src.push((bus, so.id));
                "fadeOutCurve"
            }
            "TransitionDestinationSound" => {
                dst_len = dst_len.max(len);
                dst.push((bus, so.id));
                "fadeInCurve"
            }
            c => return unsupported(format!("transition timeline module {c}")),
        };
        {
            let mut pts = Vec::new();
            if let Some(fc) = so.r1(curve_rel) {
                let fco = cx.obj(&fc)?;
                for k in ["startPoint", "endPoint"] {
                    if let Some(pid) = fco.r1(k) {
                        let pt = cx.obj(&pid)?;
                        pts.push((sec(pt.f("position", 0.0)), pt.f("value", 0.0) as f32, pt.f("curveShape", 0.0) as f32));
                    }
                }
            } else {
                let st = sec(so.f("start", 0.0));
                let en = (st + len).max(1);
                let (a, b) = if curve_rel == "fadeOutCurve" { (1.0, 0.0) } else { (0.0, 1.0) };
                pts.push((st, a, 0.0));
                pts.push((en, b, 0.0));
            }
            let mut w = W::new();
            w.chunk(b"CURV", |k| {
                k.guid(&so.id).guid(&Guid::ZERO).arr(&pts, |w, (x, y, z)| {
                    w.u32(*x).f32(*y).f32(*z).u32(0);
                });
            });
            cx.push(b"CRVS", so.id, w.b);
        }
    }
    src.sort();
    dst.sort();
    let mut w = W::new();
    w.u32(sec(tt.f("length", 0.0)));
    w.arr(&[] as &[u8], |_, _| {});
    w.arr(&[] as &[u8], |_, _| {});
    w.arr(&[] as &[u8], |_, _| {});
    w.u32(src_len).u32(dst_len);
    w.arr(&src, |w, (b, s)| {
        w.guid(b).guid(s);
    });
    w.arr(&dst, |w, (b, s)| {
        w.guid(b).guid(s);
    });
    w.guid(&Guid::ZERO);
    w.arr(&[] as &[u8], |_, _| {});
    Ok(w.b)
}

/// PROPs (volume automation) of an event-internal bus.
fn event_bus_props(cx: &mut Ctx, b: &Obj) -> Result<Vec<Prop>> {
    let mut props = Vec::new();
    let mut by_prop: Vec<(u32, Vec<Guid>)> = Vec::new();
    for a in b.r("automators") {
        let au = cx.obj(a)?;
        let name = au.p("nameOfPropertyBeingAutomated").unwrap_or("");
        let i = match name {
            "volume" => 0,
            n => return unsupported(format!("automation of {}.{n}", b.class)),
        };
        let cs = au.r("automationCurves").to_vec();
        if cs.is_empty() {
            continue;
        }
        match by_prop.iter_mut().find(|x| x.0 == i) {
            Some(x) => x.1.extend(cs),
            None => by_prop.push((i, cs)),
        }
    }
    // volume ADSR on an event bus (MODB type 0 on the bus, PROP 0)
    let mut mods: Vec<Guid> = Vec::new();
    for m in b.r("modulators") {
        let mo = cx.obj(m)?;
        if mo.class != "ADSRModulator" || mo.p("nameOfPropertyBeingModulated").unwrap_or("volume") != "volume" {
            return unsupported(format!("{} on {}", mo.class, b.class));
        }
        adsr(cx, mo, b.id, 0)?;
        mods.push(mo.id);
    }
    if !mods.is_empty() && !by_prop.iter().any(|x| x.0 == 0) {
        by_prop.push((0, Vec::new()));
    }
    by_prop.sort_by_key(|x| x.0);
    for (i, cs) in by_prop {
        for c in &cs {
            curve(cx, c, &b.id, i)?;
        }
        let ms = if i == 0 { mods.clone() } else { Vec::new() };
        props.push(cx.prop(&b.class, i, cs, ms)?);
    }
    Ok(props)
}

fn instrument(cx: &mut Ctx, ev: &Ev, o: &Obj, audio: &mut HashSet<Guid>) -> Result<()> {
    instrument_x(cx, ev, o, audio, false)
}

/// An instrument item (WAIT / MUIT); `member` = a MultiSound playlist entry (no bus, no trigger).
fn instrument_x(cx: &mut Ctx, ev: &Ev, o: &Obj, audio: &mut HashSet<Guid>, member: bool) -> Result<()> {
    let bus = if member { Guid::ZERO } else { ev.inst_bus.get(&o.id).copied().unwrap_or(Guid::ZERO) };
    let sheet = if member { None } else { ev.inst_sheet.get(&o.id).copied().flatten() };
    let fmt = inst_fmt(cx, o)?;
    // trigger code
    let mut code = Code::default();
    if let Some(gp) = sheet {
        let start = o.f("start", 0.0) as f32;
        // end computed in double, then rounded (as Studio does)
        let hi = (o.f("start", 0.0) + o.f("length", 1.0)) as f32;
        // 0x200 = upper bound exclusive: set unless the region reaches the parameter maximum
        let gmax = cx.obj(&gp)?.f("maximum", 1.0) as f32;
        code.param(&gp).range(if hi < gmax { 0x5220 } else { 0x5020 }, start, hi);
    }
    conditions(cx, o, &mut code, sheet.is_some() as usize)?;
    // modulators: autopitch is folded into INST, the rest become MODS entries
    let mut ap_param = Guid::ZERO;
    let mut ap_root = 1.0f32;
    let mut ap_min = 0.0f32;
    let mut mods: Vec<(u32, Guid)> = Vec::new();
    for m in o.r("modulators") {
        let mo = cx.obj(m)?;
        match mo.class.as_str() {
            "AutopitchModulator" => {
                ap_param = sheet.unwrap_or(Guid::ZERO);
                ap_root = mo.f("root", 1.0) as f32;
                ap_min = mo.f("pitchAtMinimum", 0.0) as f32;
            }
            "ADSRModulator" => {
                let prop = match mo.p("nameOfPropertyBeingModulated").unwrap_or("volume") {
                    "volume" => 0,
                    n => return unsupported(format!("ADSR on {n}")),
                };
                adsr(cx, mo, o.id, prop)?;
                mods.push((prop, mo.id));
            }
            "RandomizerModulator" => {
                // volume randomizers carry a=1 and a sign convention we have not confirmed
                let prop = match mo.p("nameOfPropertyBeingModulated").unwrap_or("volume") {
                    "pitch" => 1,
                    "startOffset" => 11,
                    n => return unsupported(format!("Randomizer on {n}")),
                };
                randomizer(cx, mo, o.id, prop);
                mods.push((prop, mo.id));
            }
            c => return unsupported(format!("instrument modulator {c}")),
        }
    }
    let fades = if member { Vec::new() } else { fade_curves(cx, o, sheet)? };
    let props = inst_props(cx, o, &mods, &fades)?;
    let loops: i32 = if o.b("looping", false) { -1 } else { 0 };
    let maxv = o.p("maxVoices").and_then(|v| v.parse::<u32>().ok()).unwrap_or(0x7fff_ffff);
    let tl_off = (o.f("timelockedOffset", 0.0) * TL_RATE).round() as u32;
    let g60 = if sheet.is_none() && !member { ev.timeline } else { Guid::ZERO };
    // Trigger probability only counts when enabled; delay only for delayType 0 (time, seconds).
    let prob = if o.b("triggerProbabilityEnabled", false) { o.f("triggerProbability", 100.0) as f32 } else { 100.0 };
    let (dmin, dmax) = if o.i("delayType", 1) == 0 {
        (o.f("minimumTimeDelay", 0.0) as f32, o.f("maximumTimeDelay", 0.0) as f32)
    } else {
        (0.0, 0.0)
    };
    let inst = |w: &mut W| {
        w.chunk(b"INST", |c| {
            c.guid(&g60).f32(o.f("volume", 0.0) as f32).f32(o.f("pitch", 0.0) as f32).i32(loops).u32(o.b("isCutoff", false) as u32);
            c.f32(0.0).f32(prob).f32(dmin).f32(dmax).u32(0).u32(0);
            c.guid(&ap_param).f32(ap_root).f32(ap_min).u32(maxv);
            c.sub(|s| {
                s.guid(&bus).u32(fmt).i32(-1);
            });
            c.u32(if o.i("voiceStealing", 0) == 3 { 0 } else { 2 }).u32(tl_off).f32(o.f("startOffset", 0.0) as f32).f32(0.0);
            code.write(c);
        });
        prps(w, &props);
    };
    let mut w = W::new();
    match o.class.as_str() {
        "SingleSound" => {
            let af = o.r1("audioFile").unwrap_or(Guid::ZERO);
            audio.insert(af);
            w.list(b"WAIT", |w| {
                w.chunk(b"WAIB", |c| {
                    c.guid(&o.id).guid(&af);
                });
                inst(w);
            });
            cx.push(b"WAIS", o.id, w.b);
        }
        "MultiSound" => {
            let sounds = o.r("sounds").to_vec();
            let n = sounds.len().max(1) as f32;
            // play percentages: PlayPercentage {percentage, sound}; sounds without one share the rest
            let mut pct: HashMap<Guid, f32> = HashMap::new();
            for pp in o.r("playPercentages") {
                let p = cx.obj(pp)?;
                if let Some(s) = p.r1("sound") {
                    pct.insert(s, p.f("percentage", 0.0) as f32);
                }
            }
            if !pct.is_empty() && pct.len() != sounds.len() {
                return unsupported("MultiSound with play percentages on only some sounds");
            }
            let mode = o.i("playlistMode", 2) as u32;
            w.list(b"MUIT", |w| {
                w.chunk(b"MUIB", |c| {
                    c.guid(&o.id);
                });
                w.chunk(b"PLST", |c| {
                    c.u32(mode).u32(1).arr(&sounds, |w, g| {
                        w.guid(g).f32(pct.get(g).copied().unwrap_or(100.0 / n));
                    });
                });
                inst(w);
            });
            cx.push(b"MUIS", o.id, w.b);
            for sd in &sounds {
                let so = cx.obj(sd)?;
                instrument_x(cx, ev, so, audio, true)?;
            }
        }
        "SoundScatterer" => {
            // Spawn-rate scattering only: tempo-based spawning, 3D scatter distance and the built-in
            // volume/pitch randomisation all add data we have not mapped.
            if o.i("spawnDelayType", 0) != 0 {
                return unsupported("SoundScatterer tempo-based spawn delay");
            }
            let steal = o.i("soundStealing", 0);
            if !(0..=1).contains(&steal) {
                return unsupported(format!("SoundScatterer soundStealing {steal}"));
            }
            if o.f("minimumScatterDistance", 1.0) != 0.0 || o.f("maximumScatterDistance", 20.0) != 0.0 {
                return unsupported("SoundScatterer scatter distance");
            }
            if o.f("volumeRandomization", 0.0) != 0.0 || o.f("pitchRandomization", 0.0) != 0.0 {
                return unsupported("SoundScatterer volume/pitch randomization");
            }
            let Some(ms) = o.r1("sound") else {
                return unsupported("SoundScatterer without a sound");
            };
            w.list(b"SPIT", |w| {
                w.chunk(b"SPIB", |c| {
                    c.guid(&o.id);
                    c.u32(o.i("polyphony", 16) as u32).u32(o.i("totalSounds", 33) as u32);
                    c.f32((o.f("minimumTimeBetweenSounds", 500.0) / 1000.0) as f32);
                    c.f32((o.f("maximumTimeBetweenSounds", 1000.0) / 1000.0) as f32);
                    c.u32(steal as u32 + 2).f32(o.f("spawnRate", 100.0) as f32).u32(0).u32(0);
                });
                w.chunk(b"PLST", |c| {
                    c.u32(0).u32(1).arr(&[ms], |w, g| {
                        w.guid(g).f32(1.0);
                    });
                });
                inst(w);
            });
            cx.push(b"SPIS", o.id, w.b);
            let mo = cx.obj(&ms)?;
            instrument_x(cx, ev, mo, audio, true)?;
        }
        c => return unsupported(format!("instrument {c}")),
    }
    Ok(())
}

/// RandomizerModulator -> MODB type 1: just the amount.
fn randomizer(cx: &mut Ctx, mo: &Obj, target: Guid, prop: u32) {
    let mut w = W::new();
    w.list(b"MODU", |w| {
        w.chunk(b"MODB", |c| {
            c.sub(|s| {
                s.guid(&mo.id).guid(&target).u32(prop).u32(1).u32(0).u32(0);
            });
            c.f32(mo.f("amount", 0.0) as f32);
        });
        prps(w, &[]);
    });
    cx.push(b"MODS", mo.id, w.b);
}

/// ADSRModulator -> MODB type 0.
fn adsr(cx: &mut Ctx, mo: &Obj, target: Guid, prop: u32) -> Result<()> {
    let f = |k: &str, d: f64| mo.f(k, d) as f32;
    let mut w = W::new();
    w.list(b"MODU", |w| {
        w.chunk(b"MODB", |c| {
            c.sub(|s| {
                s.guid(&mo.id).guid(&target).u32(prop).u32(0).u32(1).u32(0);
            });
            c.f32(f("initialValue", 0.0)).f32(f("peakValue", 0.0)).f32(f("sustainValue", 0.0));
            c.f32(f("attackTime", 1000.0)).f32(f("holdTime", 1000.0)).f32(f("decayTime", 1000.0)).f32(f("releaseTime", 1000.0));
            c.f32(f("attackShape", 0.0)).f32(f("decayShape", 0.0)).f32(f("releaseShape", 0.0)).f32(f("finalValue", 0.0));
        });
        prps(w, &[]);
    });
    cx.push(b"MODS", mo.id, w.b);
    Ok(())
}

fn inst_props(cx: &mut Ctx, o: &Obj, mods: &[(u32, Guid)], fades: &[Guid]) -> Result<Vec<Prop>> {
    let mut by_prop: Vec<(u32, Vec<Guid>, Vec<Guid>)> = Vec::new();
    if !fades.is_empty() {
        by_prop.push((4, Vec::new(), Vec::new()));
    }
    for a in o.r("automators") {
        let au = cx.obj(a)?;
        let name = au.p("nameOfPropertyBeingAutomated").unwrap_or("");
        let i = match name {
            "volume" => 0,
            "pitch" => 1,
            "startOffset" => 11,
            n => return unsupported(format!("automation of {}.{n}", o.class)),
        };
        let cs = au.r("automationCurves").to_vec();
        if cs.is_empty() {
            continue;
        }
        match by_prop.iter_mut().find(|x| x.0 == i) {
            Some(x) => x.1.extend(cs),
            None => by_prop.push((i, cs, Vec::new())),
        }
    }
    for (i, m) in mods {
        match by_prop.iter_mut().find(|x| x.0 == *i) {
            Some(x) => x.2.push(*m),
            None => by_prop.push((*i, Vec::new(), vec![*m])),
        }
    }
    by_prop.sort_by_key(|x| x.0);
    let mut props = Vec::new();
    for (i, cs, ms) in by_prop {
        for c in &cs {
            curve(cx, c, &o.id, i)?;
        }
        let cs = if i == 4 { fades.to_vec() } else { cs };
        props.push(cx.prop(&o.class, i, cs, ms)?);
    }
    let _ = compile_mixer::unused;
    Ok(props)
}

/// Module fade-in / fade-out curves (FadeCurve: startPoint, endPoint) -> automation of
/// instrument property 4 (identity mapping): CTRL + CURV with the FadeCurve id, driven by the
/// instrument's sheet parameter (layout CTRO) or the timeline (timeline CTRO).
fn fade_curves(cx: &mut Ctx, o: &Obj, sheet: Option<Guid>) -> Result<Vec<Guid>> {
    let mut out = Vec::new();
    for rel in ["fadeInCurve", "fadeOutCurve"] {
        let Some(fc) = o.r1(rel) else { continue };
        let f = cx.obj(&fc)?;
        let discrete = match sheet {
            Some(gp) => matches!(cx.obj(&gp)?.i("parameterType", 0), 1 | 2),
            None => false,
        };
        let mut pts: Vec<(f32, f32, f32, u32)> = Vec::new();
        for k in ["startPoint", "endPoint"] {
            if let Some(pid) = f.r1(k) {
                let pt = cx.obj(&pid)?;
                let s = if discrete { 3 } else { pt.b("isSCurve", false) as u32 };
                pts.push((pt.f("position", 0.0) as f32, pt.f("value", 0.0) as f32, pt.f("curveShape", 0.0) as f32, s));
            }
        }
        pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        match (sheet, cx.cur_event) {
            (Some(gp), Some(e)) => cx.event_ctrls.entry((e, gp)).or_default().push(fc),
            _ => cx.timeline_ctrls.push(fc),
        }
        let mut w = W::new();
        w.chunk(b"CTRL", |k| {
            k.guid(&fc).guid(&o.id).guid(&fc).u32(4);
        });
        cx.push(b"CTRS", fc, w.b);
        let mut w = W::new();
        w.chunk(b"CURV", |k| {
            k.guid(&fc).guid(&fc).arr(&pts, |w, (x, y, z, s)| {
                w.f32(*x).f32(*y).f32(*z).u32(*s);
            });
        });
        cx.push(b"CRVS", fc, w.b);
        out.push(fc);
    }
    Ok(out)
}
