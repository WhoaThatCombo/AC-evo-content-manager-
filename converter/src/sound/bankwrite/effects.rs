//! Built-in effect tables: DSP type, parameter list (XML property, type, default) and the
//! per-property mapping curves Studio attaches to automatable properties. All values
//! observed in fmodstudiocl 2.03.13 output (docs/FORMATS.md, "FMOD Studio banks").

use super::prj::{Guid, Obj};
use super::w::W;

#[derive(Clone, Copy)]
pub enum P {
    F(&'static str, f32),
    I(&'static str, i32),
    B(&'static str, bool),
    /// data parameter (empty unless noted)
    D(&'static [u8]),
}

pub struct Fx {
    pub dsp: u32,
    pub params: &'static [P],
}

const FADER: &[(f32, f32)] = &[(-42.0, -80.0), (-39.0, -60.0), (-36.0, -50.0), (-32.0, -40.0), (-27.0, -30.0), (-20.0, -20.0), (-10.0, -10.0), (0.0, 0.0), (10.0, 10.0)];
const DIST: &[(f32, f32)] = &[(0.0, 0.0), (2.0, 1.0), (5.0, 5.0), (10.0, 10.0), (15.0, 100.0), (20.0, 1000.0), (27.0, 10000.0)];
const FREQ22K: &[(f32, f32)] = &[(0.0, 10.0), (1.0, 100.0), (2.0, 400.0), (3.0, 750.0), (4.0, 1250.0), (5.0, 2000.0), (6.0, 3200.0), (7.0, 5500.0), (8.0, 10000.0), (9.0, 18000.0), (10.0, 22000.0)];
const FREQ20K: &[(f32, f32)] = &[(0.2, 20.0), (1.0, 100.0), (2.0, 400.0), (3.0, 750.0), (4.0, 1250.0), (5.0, 2000.0), (6.0, 3200.0), (7.0, 5500.0), (8.0, 10000.0), (9.0, 18000.0), (9.5, 20000.0)];

/// (owner class, property index) -> (a, b, mapping guid, mapping points).
pub fn mapping(class: &str, prop: u32) -> Option<(u16, u16, &'static str, &'static [(f32, f32)])> {
    Some(match (class, prop) {
        ("SingleSound" | "MultiSound" | "EventSound" | "SoundScatterer" | "ProgrammerSound", 0) => (2, 1, "{7400f0ce-d180-44f9-94a2-dbe1fe7dbaf4}", FADER),
        ("SingleSound" | "MultiSound" | "EventSound" | "SoundScatterer" | "ProgrammerSound", 1) => (2, 0, "{c18c2107-7bb9-4cdc-8453-19b881db2e14}", &[(-1.0, -24.0), (1.0, 24.0)]),
        ("SingleSound" | "MultiSound" | "EventSound" | "SoundScatterer" | "ProgrammerSound", 4) => (4, 0, "{678d3693-0732-4013-83fc-6262aa4b587e}", &[(0.0, 0.0), (1.0, 1.0)]),
        ("SingleSound" | "MultiSound" | "EventSound" | "SoundScatterer" | "ProgrammerSound", 11) => (1, 0, "{7f2a7f25-64d1-4e57-90f6-27cfc24462fe}", &[(0.0, 0.0), (1.0, 100.0)]),
        ("EventMixerGroup" | "EventMixerMaster" | "EventMixerReturn", 0) => (2, 1, "{d65ba128-a6f5-41d9-b6a1-a7c402f329ac}", FADER),
        ("MixerSend", 0) => (2, 1, "{c021a555-2747-4628-a01b-b732941315e2}", FADER),
        ("Event", 30) => (3, 0, "{d762499c-381b-4585-b9ba-6e5d09a7315a}", &[(0.0, 0.0), (2.0, 1.0), (5.0, 5.0), (10.0, 10.0), (15.0, 100.0), (20.0, 1000.0), (27.0, 10000.0)]),
        ("Event", 31) => (3, 0, "{c79ba1fd-6e75-4873-a3a4-479b9f08ebd5}", &[(0.0, 0.0), (2.0, 1.0), (5.0, 5.0), (10.0, 10.0), (15.0, 100.0), (20.0, 1000.0), (27.0, 10000.0)]),
        ("SFXReverbEffect", 1000) => (3, 0, "{9b372eae-c0f8-49f1-9a39-6f54d11332c4}", &[(0.0, 100.0), (0.5, 300.0), (1.0, 1000.0), (2.0, 5000.0), (3.0, 20000.0)]),
        ("SFXReverbEffect", 1001) => (3, 0, "{8d9af343-395f-4ff1-96b5-239562cf7211}", &[(0.0, 0.0), (1.0, 30.0), (2.0, 100.0), (3.0, 300.0)]),
        ("SFXReverbEffect", 1002) => (3, 0, "{78a08fbe-aae4-4944-b98a-994818b65632}", &[(0.0, 0.0), (1.0, 30.0), (2.0, 100.0)]),
        ("SFXReverbEffect", 1003) => (3, 0, "{f8a22e7b-793f-455e-add8-b5fc9d713930}", FREQ20K),
        ("SFXReverbEffect", 1004) => (3, 0, "{f8b453f2-c376-4b48-abb1-45055301ea70}", &[(0.0, 10.0), (1.0, 100.0)]),
        ("SFXReverbEffect", 1005) => (3, 0, "{8173de7f-1d68-4b95-a079-1b88f805dd40}", &[(0.0, 0.0), (1.0, 100.0)]),
        ("SFXReverbEffect", 1006) => (3, 0, "{62a25927-ac61-4775-b1ff-1e7f5ad1031c}", &[(0.0, 0.0), (1.0, 100.0)]),
        ("SFXReverbEffect", 1007) => (3, 0, "{429af1b8-f9e5-4c16-8654-f61db83c226b}", &[(0.2, 20.0), (1.0, 100.0), (2.0, 400.0), (3.0, 750.0), (3.8, 1000.0)]),
        ("SFXReverbEffect", 1008) => (2, 0, "{0c835252-303e-42f4-84d5-9fed6268490d}", &[(-36.0, -36.0), (0.0, 0.0), (12.0, 12.0)]),
        ("SFXReverbEffect", 1009) => (0, 0, "{b789576b-27ff-4d15-98e1-941636afddad}", FREQ20K),
        ("SFXReverbEffect", 1010) => (3, 0, "{58648de7-042b-4792-8ef0-d2690cf54479}", &[(0.0, 0.0), (1.0, 100.0)]),
        ("SFXReverbEffect", 1011) => (2, 1, "{3d15fffc-3deb-4d9e-9288-e75bc0f1e368}", FADER),
        ("SFXReverbEffect", 1012) => (2, 1, "{4b1f6e2f-0c38-42bf-958c-37c6a555ced8}", FADER),
        ("DistortionEffect", 1000) => (3, 0, "{58b3c021-465c-4a0f-8215-a01c9195f33b}", &[(0.0, 0.0), (1.0, 1.0)]),
        ("ThreeEQEffect", 1000) => (2, 1, "{723dff38-333e-478a-a236-22ae108b7df7}", FADER),
        ("ThreeEQEffect", 1001) => (2, 1, "{b85730eb-7a94-45ae-9def-28f4ff5bec8b}", FADER),
        ("ThreeEQEffect", 1002) => (2, 1, "{6f55275f-5a98-4c18-a538-aa4aed112d9d}", FADER),
        ("ThreeEQEffect", 1003) => (3, 0, "{f4b9b090-0f1d-4264-a9c8-ba5e065b8327}", &[(0.0, 10.0), (1.0, 100.0), (2.0, 400.0), (3.0, 750.0),
            (4.0, 1250.0), (5.0, 2000.0), (6.0, 3200.0), (7.0, 5500.0), (8.0, 10000.0), (9.0, 18000.0), (9.3, 22000.0)]),
        ("ParamEqEffect", 1000) => (3, 0, "{577e50f7-d4bd-488e-889e-4b118d7aa7d1}", &[(0.0, 20.0), (1.0, 100.0), (2.0, 400.0), (3.0, 750.0),
            (4.0, 1250.0), (5.0, 2000.0), (6.0, 3200.0), (7.0, 5500.0), (8.0, 10000.0), (9.0, 18000.0), (10.0, 22000.0)]),
        ("ParamEqEffect", 1002) => (2, 0, "{1763d487-40e2-4bf2-ae75-63a72e6c1890}", &[(0.0, -30.0), (1.0, 0.0), (2.0, 30.0)]),
        ("LowpassEffect", 1000) => (0, 0, "{afb85753-9f2c-4304-86ca-dd51c48722c3}", FREQ22K),
        ("LowpassSimpleEffect", 1000) => (1, 0, "{55132a92-d55d-4b98-857b-f57c52272edf}", FREQ22K),
        ("SpatialiserEffect", 1016) => (1, 0, "{c478cc5f-1a76-4553-b46e-384c5e9ce557}", &[(0.0, 0.0), (1.0, 360.0)]),
        ("ChannelMixEffect", 1001) => (2, 1, "{781c2456-510b-4a1d-8e10-77b10e673519}", FADER),
        ("ChannelMixEffect", 1002) => (2, 1, "{3805c21e-1854-431e-832f-36bff000acdf}", FADER),
        ("ChannelMixEffect", 1003) => (2, 1, "{2431a32b-282f-4e5f-97a8-be953a4297c9}", FADER),
        ("ChannelMixEffect", 1004) => (2, 1, "{da3f649e-6c40-4abe-a368-7f9417eee216}", FADER),
        // from the game's own banks (Kunos builds)
        ("ChannelMixEffect", 1005) => (2, 1, "{15e37282-c8d1-4ef8-ba12-78476af2dffe}", FADER),
        ("ChannelMixEffect", 1006) => (2, 1, "{f01c962e-6f05-4d1d-8b1f-46ebe19ac355}", FADER),
        ("SpatialiserEffect", 1012) => (3, 0, "{ddd0ed14-5cb5-4a86-b80f-9b33da817f73}", DIST),
        ("SpatialiserEffect", 1013) => (3, 0, "{506e7c91-5c0b-4f7b-ad8e-aa7dfca0f069}", DIST),
        _ => return None,
    })
}

const PAN_NAMES: [&str; 24] = ["", "stereoPosition", "surroundPanDirection", "surroundPanExtent", "surroundPanRotation", "LFELevel",
    "stereoToSurroundPanMode", "stereoSeparation", "stereoAxis", "", "", "distanceRolloffType", "minimumDistance", "maximumDistance",
    "extentMode", "soundSize", "minimumExtent", "", "LFEUpmixEnabled", "", "", "", "", "overrideRange"];

/// Studio property name of a built-in effect parameter index (1000 + i) and the reverse.
pub fn param_index(class: &str, name: &str) -> Option<u32> {
    if class == "SpatialiserEffect" || class == "MixerBusPanner" {
        let n = match name {
            "userLFELevel" | "surroundLFELevel" => "LFELevel",
            "stereoToSurroundUserPanMode" => "stereoToSurroundPanMode",
            n => n,
        };
        return PAN_NAMES.iter().position(|x| !x.is_empty() && *x == n).map(|i| 1000 + i as u32);
    }
    if class == "ChannelMixEffect" {
        if name == "outputGrouping" {
            return Some(1000);
        }
        if let Some(k) = name.strip_prefix("gain").and_then(|k| k.parse::<u32>().ok()) {
            return (k < 32).then_some(1001 + k);
        }
        return None;
    }
    let fx = fx(class)?;
    fx.params.iter().position(|p| match p {
        P::F(n, _) | P::I(n, _) | P::B(n, _) => *n == name,
        P::D(_) => false,
    }).map(|i| 1000 + i as u32)
}

/// Inverse of `param_index`: the Studio property name of effect parameter `index` (1000 + i).
/// The pan DSP's LFE / stereo-to-surround names differ between a spatialiser and a bus panner.
pub fn param_name(class: &str, index: u32) -> Option<String> {
    let i = index.checked_sub(1000)? as usize;
    if class == "SpatialiserEffect" || class == "MixerBusPanner" {
        let n = *PAN_NAMES.get(i).filter(|n| !n.is_empty())?;
        let sp = class == "SpatialiserEffect";
        return Some(match n {
            "LFELevel" => if sp { "userLFELevel" } else { "surroundLFELevel" },
            "stereoToSurroundPanMode" => if sp { "stereoToSurroundUserPanMode" } else { "stereoToSurroundPanMode" },
            n => n,
        }.to_string());
    }
    if class == "ChannelMixEffect" {
        return Some(if i == 0 { "outputGrouping".to_string() } else if i <= 32 { format!("gain{:02}", i - 1) } else { return None });
    }
    match fx(class)?.params.get(i)? {
        P::F(n, _) | P::I(n, _) | P::B(n, _) if !n.is_empty() => Some(n.to_string()),
        _ => None,
    }
}

/// Built-in effect class of a DSP type (the pan DSP 27 is resolved by the caller).
pub fn class_of_dsp(dsp: u32) -> Option<&'static str> {
    ["SFXReverbEffect", "ThreeEQEffect", "LowpassEffect", "ParamEqEffect", "HighpassEffect", "LowpassSimpleEffect", "DistortionEffect", "CompressorEffect",
        "LoudnessMeter", "GainEffect", "ChannelMixEffect"].into_iter().find(|c| fx(c).map(|f| f.dsp) == Some(dsp))
}

const CHANNEL_MIX: [P; 65] = {
    let mut a = [P::F("", 0.0); 65];
    a[0] = P::I("outputGrouping", 0);
    const G: [&str; 32] = ["gain00", "gain01", "gain02", "gain03", "gain04", "gain05", "gain06", "gain07", "gain08", "gain09", "gain10",
        "gain11", "gain12", "gain13", "gain14", "gain15", "gain16", "gain17", "gain18", "gain19", "gain20", "gain21", "gain22", "gain23",
        "gain24", "gain25", "gain26", "gain27", "gain28", "gain29", "gain30", "gain31"];
    let mut i = 0;
    while i < 32 {
        a[1 + i] = P::F(G[i], 0.0);
        a[33 + i] = P::I("", i as i32);
        i += 1;
    }
    a
};

pub fn fx(class: &str) -> Option<Fx> {
    use P::*;
    Some(match class {
        "SFXReverbEffect" => Fx { dsp: 19, params: &[F("decayTime", 1500.0), F("earlyDelay", 20.0), F("lateDelay", 40.0), F("HFReference", 5000.0),
            F("HFDecayRatio", 50.0), F("diffusion", 100.0), F("density", 100.0), F("lowShelfFrequency", 250.0), F("lowShelfGain", 0.0),
            F("highCut", 20000.0), F("earlyLateMix", 50.0), F("wetLevel", -6.0), F("dryLevel", 0.0)] },
        "ThreeEQEffect" => Fx { dsp: 28, params: &[F("lowGain", 0.0), F("midGain", 0.0), F("highGain", 0.0), F("lowCrossover", 400.0),
            F("highCrossover", 4000.0), I("crossoverSlope", 1)] },
        "LowpassEffect" => Fx { dsp: 3, params: &[F("cutoff", 2000.0), F("resonance", 1.0)] },
        "ParamEqEffect" => Fx { dsp: 12, params: &[F("centreFrequency", 2000.0), F("bandwidth", 1.0), F("gain", 0.0)] },
        "HighpassEffect" => Fx { dsp: 5, params: &[F("cutoff", 2000.0), F("resonance", 1.0)] },
        "LowpassSimpleEffect" => Fx { dsp: 20, params: &[F("cutoff", 2000.0)] },
        "DistortionEffect" => Fx { dsp: 9, params: &[F("level", 0.5)] },
        "CompressorEffect" => Fx { dsp: 18, params: &[F("threshold", 0.0), F("ratio", 2.5), F("attackTime", 20.0), F("releaseTime", 200.0),
            F("gain", 0.0), D(&[0, 0, 0, 0]), B("linked", true)] },
        "LoudnessMeter" => Fx { dsp: 30, params: &[I("", 1), D(&[]), D(&[])] },
        "GainEffect" => Fx { dsp: 7, params: &[F("gain", 0.0), D(&[])] },
        "ChannelMixEffect" => Fx { dsp: 33, params: &CHANNEL_MIX },
        _ => return None,
    })
}

/// FMOD speaker mode of a Studio channel-format enum (0 mono, 1 stereo, 2 surround=5.1, 3 quad, 4 5.0, 5 5.1, 6 7.1).
pub fn speaker_mode(fmt: i64) -> u32 {
    match fmt {
        0 => 2,
        1 => 3,
        2 => 6,
        3 => 4,
        4 => 5,
        5 => 6,
        6 => 7,
        7 => 8,
        _ => 6,
    }
}

/// Pan DSP (MixerBusPanner / SpatialiserEffect) parameter block; `out` = output speaker mode.
pub fn pan_params(o: &Obj, spatialiser: bool, out: u32, w: &mut Vec<(u8, Vec<u8>)>) {
    let f = |k: &str, d: f64| o.f(k, d) as f32;
    let mode: i32 = match out {
        2 => 0,
        3 => 1,
        _ => 2,
    };
    let pf = |v: f32| (0u8, v.to_le_bytes().to_vec());
    let pi = |v: i32| (1u8, v.to_le_bytes().to_vec());
    let pd = |v: &[u8]| (3u8, v.to_vec());
    let (lfe, stereo_mode) = if spatialiser {
        (f("userLFELevel", 0.0), o.i("stereoToSurroundUserPanMode", 1) as i32)
    } else {
        (f("surroundLFELevel", 0.0), o.i("stereoToSurroundPanMode", 1) as i32)
    };
    w.push(pi(mode));
    w.push(pf(f("stereoPosition", 0.0)));
    w.push(pf(f("surroundPanDirection", 0.0)));
    w.push(pf(f("surroundPanExtent", 360.0)));
    w.push(pf(f("surroundPanRotation", 0.0)));
    w.push(pf(lfe));
    w.push(pi(stereo_mode));
    w.push(pf(f("stereoSeparation", 60.0)));
    w.push(pf(f("stereoAxis", 0.0)));
    w.push(pi(4095));
    w.push(pd(&[]));
    w.push(pi(o.i("distanceRolloffType", 0) as i32));
    w.push(pf(f("minimumDistance", 1.0)));
    w.push(pf(f("maximumDistance", 20.0)));
    w.push(pi(o.i("extentMode", 0) as i32));
    w.push(pf(f("soundSize", 0.0)));
    w.push(pf(f("minimumExtent", 0.0)));
    w.push(pf(if spatialiser { 1.0 } else { 0.0 }));
    w.push(pi(o.b("LFEUpmixEnabled", false) as i32));
    w.push(pd(&[]));
    w.push(pi(out as i32));
    w.push(pf(0.0));
    w.push(pd(&[]));
    w.push((2u8, vec![if spatialiser { o.b("overrideRange", false) as u8 } else { 1 }]));
}

/// PMEF chunk body from (type, value bytes) params.
pub fn pmef(params: &[(u8, Vec<u8>)]) -> Vec<u8> {
    let mut w = W::new();
    w.u32(params.len() as u32);
    for (t, v) in params {
        w.u32(*t as u32);
        if *t == 3 {
            w.vcount(v.len() as u32);
        }
        w.bytes(v);
    }
    w.u8(0);
    w.b
}

/// Generic built-in effect params from the table.
pub fn table_params(o: &Obj, fx: &Fx) -> Vec<(u8, Vec<u8>)> {
    fx.params.iter().map(|p| match p {
        P::F(n, d) => (0u8, (o.f(n, *d as f64) as f32).to_le_bytes().to_vec()),
        P::I(n, d) => (1u8, (o.i(n, *d as i64) as i32).to_le_bytes().to_vec()),
        P::B(n, d) => (2u8, vec![o.b(n, *d) as u8]),
        P::D(v) => (3u8, v.to_vec()),
    }).collect()
}

pub fn g(s: &str) -> Guid {
    Guid::parse(s).unwrap()
}
