//! Project graph -> runtime bank model items -> RIFF bytes.

use anyhow::{anyhow, Result};
use std::collections::{BTreeMap, HashMap, HashSet};

use super::effects::{self, g};
use super::fsb;
use super::prj::{Guid, Obj, Project};
use super::strings;
use super::w::{Code, W};
use super::Banks;

/// A project feature the in-house writer does not handle (caller falls back to fmodstudiocl).
#[derive(Debug)]
pub struct Unsupported(pub String);
impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unsupported by the in-house bank writer: {}", self.0)
    }
}
impl std::error::Error for Unsupported {}

pub fn unsupported<T>(what: impl Into<String>) -> Result<T> {
    Err(anyhow::Error::new(Unsupported(what.into())))
}

pub const C_GLOBAL: &str = "{d04fb8b4-a6aa-4f2a-a9fa-dbbeeb5abb4f}";
pub const C_SNAP: &str = "{9b8f058f-e215-491b-808b-ba061f27afd8}";
pub const C_SNAPBUS: &str = "{44dc1587-6b49-42fe-905d-6983c57b6b8f}";
pub const C_STRINGS: &str = "{981d20d4-717b-4c42-8649-8afef1f30698}";
pub const C_PMAP: &str = "{81a9378f-8927-40bc-a1bc-51e8312b25f0}";

/// Object list ids in PROJ order.
pub const LISTS: [&[u8; 4]; 30] = [b"IBSS", b"GBSS", b"RBSS", b"PRTS", b"MBSS", b"BEFX", b"PEFX", b"SEFX", b"SCFX", b"SSFX", b"VCAS", b"EVTS",
    b"TLNS", b"PMLS", b"PRMS", b"CTRS", b"CRVS", b"MPGS", b"MUIS", b"SPIS", b"PRIS", b"EVIS", b"WAIS", b"EFIS", b"CMDS", b"SLNS", b"LWVS",
    b"WAVS", b"SNAS", b"MODS"];

pub struct Ctx<'a> {
    pub p: &'a Project,
    /// list id -> (sort guid, serialised LIST item)
    pub items: BTreeMap<[u8; 4], Vec<(Guid, Vec<u8>)>>,
    /// mapping guid -> points
    pub maps: BTreeMap<Guid, &'static [(f32, f32)]>,
    pub dyn_maps: BTreeMap<Guid, Vec<(f32, f32)>>,
    pub strings: Vec<(String, Guid)>,
    /// bus id -> (input speaker mode, output speaker mode)
    pub fmt: HashMap<Guid, (u32, u32)>,
    pub n_events: u32,
    pub bank: Guid,
    pub params_used: HashSet<Guid>,
    /// GameParameter -> automation curves driven by it (layout CTRO)
    pub layout_ctrls: HashMap<Guid, Vec<Guid>>,
    /// (event, GameParameter) -> automation curves of that event driven by it
    pub event_ctrls: HashMap<(Guid, Guid), Vec<Guid>>,
    pub cur_event: Option<Guid>,
    /// (object with trigger conditions, condition GameParameter)
    pub cond_params: Vec<(Guid, Guid)>,
    /// fade curves of the current event's timeline instruments (timeline CTRO)
    pub timeline_ctrls: Vec<Guid>,
}

impl<'a> Ctx<'a> {
    pub fn obj(&self, g: &Guid) -> Result<&'a Obj> {
        self.p.get(g).ok_or_else(|| anyhow!("dangling reference {g}"))
    }
    pub fn push(&mut self, list: &[u8; 4], id: Guid, item: Vec<u8>) {
        self.items.entry(*list).or_default().push((id, item));
    }
}

pub fn build(p: &Project) -> Result<Banks> {
    build_with(p, super::vorbisenc::quality())
}

pub fn build_with(p: &Project, vorbis: Option<f32>) -> Result<Banks> {
    let banks = p.class("Bank");
    let bank = match banks.as_slice() {
        [b] => *b,
        _ => return unsupported(format!("{} Bank objects (need exactly one)", banks.len())),
    };
    if !bank.b("isMasterBank", false) {
        return unsupported("bank is not a master bank");
    }
    let name = bank.name().to_string();
    let mut cx = Ctx { p, items: BTreeMap::new(), maps: BTreeMap::new(), dyn_maps: BTreeMap::new(), strings: Vec::new(), fmt: HashMap::new(),
        n_events: 0, bank: bank.id, params_used: HashSet::new(), layout_ctrls: HashMap::new(), event_ctrls: HashMap::new(), cur_event: None, cond_params: Vec::new(), timeline_ctrls: Vec::new() };
    cx.strings.push((format!("bank:/{name}"), bank.id));
    cx.strings.push((format!("bank:/{name}.strings"), bank.id.xor(&g(C_STRINGS))));
    let events: Vec<&Obj> = p.class("Event").into_iter().filter(|e| e.r("banks").contains(&bank.id)).collect();
    // streamed assets live outside the FSB (and switch events to scheduling mode 2): not handled
    for f in p.class("MasterAssetFolder") {
        if f.f("streamingAssetThresholdLength", 10.0) >= 0.0 {
            return unsupported("asset streaming enabled (MasterAssetFolder streamingAssetThresholdLength >= 0)");
        }
    }
    if p.objs.values().any(|o| o.b("isStreaming", false)) {
        return unsupported("streaming asset");
    }
    super::compile_mixer::formats(&mut cx, &events)?;
    super::compile_mixer::mixer(&mut cx, &events)?;
    let mut samples: Vec<(Guid, fsb::Sample)> = Vec::new();
    super::compile_event::events(&mut cx, &events, &mut samples)?;
    super::compile_mixer::parameters(&mut cx)?;
    // mappings
    let maps: Vec<(Guid, Vec<(f32, f32)>)> = cx.maps.iter().map(|(k, v)| (*k, v.to_vec())).chain(cx.dyn_maps.iter().map(|(k, v)| (*k, v.clone()))).collect();
    for (id, pts) in maps {
        let mut w = W::new();
        w.chunk(b"MAP ", |c| {
            c.guid(&id).arr(&pts, |w, (x, y)| {
                w.f32(*x).f32(*y);
            });
        });
        cx.push(b"MPGS", id, w.b);
    }
    // waveforms + FSB
    samples.sort_by_key(|s| s.0);
    for (i, (id, _)) in samples.iter().enumerate() {
        let mut w = W::new();
        w.chunk(b"WAV ", |c| {
            c.guid(id).sub(|s| {
                s.u32(0).u32(i as u32).u32(0);
            });
        });
        cx.push(b"WAVS", *id, w.b);
    }
    let fsb = if samples.is_empty() {
        Vec::new()
    } else {
        let v: Vec<fsb::Sample> = samples.into_iter().map(|s| s.1).collect();
        // ACECONV_BANK_VORBIS: Vorbis samples when every one can be (else PCM16)
        match vorbis {
            Some(q) => match super::vorbisenc::fsb5_vorbis(&v, q)? {
                Some(b) => b,
                None => {
                    eprintln!("  sound: a sample cannot be FMOD Vorbis (rate/channels/setup) - bank stays PCM16");
                    fsb::fsb5(&v)?
                }
            },
            None => fsb::fsb5(&v)?,
        }
    };
    let bankb = emit(&cx, bank.id, &fsb, false);
    let strs = emit_strings(&cx, bank.id.xor(&g(C_STRINGS)));
    Ok(Banks { bank: bankb, strings: strs, name })
}

fn proj_head(w: &mut W, bank: Guid, n_events: u32) {
    w.chunk(b"BNKI", |c| {
        c.guid(&bank).u32(0).u32(0).u32(n_events).u32(7);
    });
}

fn emit(cx: &Ctx, bank: Guid, fsb: &[u8], _strings: bool) -> Vec<u8> {
    let mut proj = W::new();
    proj.bytes(b"PROJ");
    proj_head(&mut proj, bank, cx.n_events);
    for l in LISTS {
        let mut items: Vec<&(Guid, Vec<u8>)> = cx.items.get(l).map(|v| v.iter().collect()).unwrap_or_default();
        items.sort_by_key(|x| x.0);
        proj.list(l, |w| {
            if !items.is_empty() {
                w.chunk(b"LCNT", |c| {
                    c.u32(items.len() as u32);
                });
                for it in items {
                    w.bytes(&it.1);
                }
            }
        });
    }
    // SNDH, filled once the FSB offset is known
    let sndh_at = proj.b.len();
    proj.chunk(b"SNDH", |c| {
        if !fsb.is_empty() {
            c.u16(3).u16(8).u32(0).u32(fsb.len() as u32);
        }
    });
    proj.chunk(b"STDT", |_| {});
    proj.chunk(b"STBL", |_| {});
    proj.chunk(b"HASH", |c| {
        c.arr(&[] as &[u8], |_, _| {});
    });
    proj.chunk(b"DEL ", |_| {});
    proj.chunk(b"MUTE", |_| {});
    proj.chunk(b"REFI", |_| {});
    let plat = cx.p.class("Platform");
    proj.chunk(b"PLAT", |c| {
        if let Some(pl) = plat.first() {
            c.guid(&pl.id);
        }
    });
    let mut out = W::new();
    out.bytes(b"RIFF").u32(0).bytes(b"FEV ");
    out.chunk(b"FMT ", |c| {
        c.u32(0x92).u32(0x92);
    });
    out.bytes(b"LIST").u32(proj.b.len() as u32);
    let proj_at = out.b.len();
    out.bytes(&proj.b);
    if !fsb.is_empty() {
        let snd_body = out.b.len() + 8;
        let pad = (32 - snd_body % 32) % 32;
        let fsb_at = snd_body + pad;
        let o = proj_at + sndh_at + 8 + 4;
        out.b[o..o + 4].copy_from_slice(&(fsb_at as u32).to_le_bytes());
        out.bytes(b"SND ").u32((pad + fsb.len()) as u32);
        out.bytes(&vec![0u8; pad]).bytes(fsb);
    }
    let n = out.b.len() as u32 - 8;
    out.b[4..8].copy_from_slice(&n.to_le_bytes());
    out.b
}

fn emit_strings(cx: &Ctx, sbank: Guid) -> Vec<u8> {
    let mut proj = W::new();
    proj.bytes(b"PROJ");
    proj_head(&mut proj, sbank, 0);
    for l in LISTS {
        proj.list(l, |_| {});
    }
    proj.chunk(b"SNDH", |_| {});
    let body = strings::stdt(&cx.strings);
    proj.chunk_bytes(b"STDT", &body);
    proj.chunk(b"STBL", |_| {});
    proj.chunk(b"HASH", |c| {
        c.arr(&[] as &[u8], |_, _| {});
    });
    proj.chunk(b"DEL ", |_| {});
    proj.chunk(b"MUTE", |_| {});
    proj.chunk(b"REFI", |_| {});
    proj.chunk(b"PLAT", |_| {});
    let mut out = W::new();
    out.bytes(b"RIFF").u32(0).bytes(b"FEV ");
    out.chunk(b"FMT ", |c| {
        c.u32(0x92).u32(0x92);
    });
    out.bytes(b"LIST").u32(proj.b.len() as u32).bytes(&proj.b);
    let n = out.b.len() as u32 - 8;
    out.b[4..8].copy_from_slice(&n.to_le_bytes());
    out.b
}

// ------------------------------------------------------------------ shared helpers

/// PROP entry for an automated / modulated / snapshotted property.
pub struct Prop {
    pub index: u32,
    pub a: u16,
    pub b: u16,
    pub map: Guid,
    pub curves: Vec<Guid>,
    pub mods: Vec<Guid>,
}

pub fn prps(w: &mut W, props: &[Prop]) {
    w.list(b"PRPS", |w| {
        if props.is_empty() {
            return;
        }
        w.chunk(b"LCNT", |c| {
            c.u32(props.len() as u32);
        });
        for p in props {
            let mut cs = p.curves.clone();
            cs.sort();
            let mut ms = p.mods.clone();
            ms.sort();
            w.chunk(b"PROP", |c| {
                c.u32(p.index).u16(p.a).u16(p.b).guid(&p.map).guids(&cs).guids(&ms);
            });
        }
    });
}

impl<'a> Ctx<'a> {
    /// A PROP for (owner class, property index) using the fixed mapping table.
    pub fn prop(&mut self, class: &str, index: u32, curves: Vec<Guid>, mods: Vec<Guid>) -> Result<Prop> {
        let Some((a, b, m, pts)) = effects::mapping(class, index) else {
            return unsupported(format!("no mapping known for {class} property {index}"));
        };
        let map = g(m);
        self.maps.insert(map, pts);
        Ok(Prop { index, a, b, map, curves, mods })
    }
}

pub fn code_write(w: &mut W, c: &Code) {
    c.write(w);
}
