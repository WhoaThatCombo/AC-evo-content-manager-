//! Offline renders through the game's own FMOD Studio runtime (fmodstudio.dll / fmod.dll
//! from the user's install), used to set a converted car's level against stock cars.
//! Output goes to FMOD's WAV writer (WAVWRITER_NRT): nothing reaches the speakers.
//! The same harness for people: tools/fmodrender.py.

use anyhow::{anyhow, bail, Result};
use std::ffi::{c_char, c_int, c_uint, c_void, CString};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const VERSION: c_uint = 0x0002_0313;
const RATE: c_int = 48000;
const BLOCK: c_uint = 512;

#[repr(C)]
#[derive(Clone, Copy)]
struct Vec3 {
    x: f32,
    y: f32,
    z: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Attr3d {
    position: Vec3,
    velocity: Vec3,
    forward: Vec3,
    up: Vec3,
}

fn attr(z: f32) -> Attr3d {
    let v = |x, y, z| Vec3 { x, y, z };
    Attr3d { position: v(0.0, 0.0, z), velocity: v(0.0, 0.0, 0.0), forward: v(0.0, 0.0, 1.0), up: v(0.0, 1.0, 0.0) }
}

/// One render request: the event, where it sits (metres ahead of the listener; the car faces
/// away, as seen from a chase camera), how long, and parameter ramps (name, start, end).
pub struct Shot<'a> {
    pub event: &'a str,
    pub distance: f32,
    pub secs: f32,
    pub ramps: Vec<(&'a str, f32, f32)>,
}

type P = *mut c_void;

macro_rules! sym {
    ($lib:expr, $name:literal, $t:ty) => {
        unsafe { *$lib.get::<$t>($name).map_err(|e| anyhow!("{}: {e}", String::from_utf8_lossy($name)))? }
    };
}

fn ck(r: c_int, what: &str) -> Result<()> {
    if r == 0 { Ok(()) } else { bail!("FMOD {what}: error {r}") }
}

/// RMS (dBFS, both channels) of each shot, with `banks` (file bytes, strings banks first) loaded.
/// The game folder holds fmod.dll, fmodstudio.dll and the DSP plugins.
pub fn rms(game: &Path, banks: &[&[u8]], shots: &[Shot], tmp: &Path) -> Result<Vec<f32>> {
    // FMOD's state is per process: one render at a time
    static LOCK: Mutex<()> = Mutex::new(());
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let core = unsafe { libloading::Library::new(game.join("fmod.dll")) }.map_err(|e| anyhow!("{}: {e}", game.join("fmod.dll").display()))?;
    let st = unsafe { libloading::Library::new(game.join("fmodstudio.dll")) }.map_err(|e| anyhow!("{}: {e}", game.join("fmodstudio.dll").display()))?;
    let create = sym!(st, b"FMOD_Studio_System_Create", unsafe extern "system" fn(*mut P, c_uint) -> c_int);
    let get_core = sym!(st, b"FMOD_Studio_System_GetCoreSystem", unsafe extern "system" fn(P, *mut P) -> c_int);
    let init = sym!(st, b"FMOD_Studio_System_Initialize", unsafe extern "system" fn(P, c_int, c_uint, c_uint, *mut c_void) -> c_int);
    let release = sym!(st, b"FMOD_Studio_System_Release", unsafe extern "system" fn(P) -> c_int);
    let update = sym!(st, b"FMOD_Studio_System_Update", unsafe extern "system" fn(P) -> c_int);
    let load_mem = sym!(st, b"FMOD_Studio_System_LoadBankMemory", unsafe extern "system" fn(P, *const c_char, c_int, c_int, c_uint, *mut P) -> c_int);
    let get_event = sym!(st, b"FMOD_Studio_System_GetEvent", unsafe extern "system" fn(P, *const c_char, *mut P) -> c_int);
    let listener = sym!(st, b"FMOD_Studio_System_SetListenerAttributes", unsafe extern "system" fn(P, c_int, *const Attr3d, *const Vec3) -> c_int);
    let load_samples = sym!(st, b"FMOD_Studio_EventDescription_LoadSampleData", unsafe extern "system" fn(P) -> c_int);
    let loading = sym!(st, b"FMOD_Studio_EventDescription_GetSampleLoadingState", unsafe extern "system" fn(P, *mut c_int) -> c_int);
    let instance = sym!(st, b"FMOD_Studio_EventDescription_CreateInstance", unsafe extern "system" fn(P, *mut P) -> c_int);
    let set3d = sym!(st, b"FMOD_Studio_EventInstance_Set3DAttributes", unsafe extern "system" fn(P, *const Attr3d) -> c_int);
    let set_param = sym!(st, b"FMOD_Studio_EventInstance_SetParameterByName", unsafe extern "system" fn(P, *const c_char, f32, c_int) -> c_int);
    let start = sym!(st, b"FMOD_Studio_EventInstance_Start", unsafe extern "system" fn(P) -> c_int);
    let stop = sym!(st, b"FMOD_Studio_EventInstance_Stop", unsafe extern "system" fn(P, c_int) -> c_int);
    let inst_release = sym!(st, b"FMOD_Studio_EventInstance_Release", unsafe extern "system" fn(P) -> c_int);
    let set_output = sym!(core, b"FMOD_System_SetOutput", unsafe extern "system" fn(P, c_int) -> c_int);
    let set_format = sym!(core, b"FMOD_System_SetSoftwareFormat", unsafe extern "system" fn(P, c_int, c_int, c_int) -> c_int);
    let set_dsp = sym!(core, b"FMOD_System_SetDSPBufferSize", unsafe extern "system" fn(P, c_uint, c_int) -> c_int);
    let load_plugin = sym!(core, b"FMOD_System_LoadPlugin", unsafe extern "system" fn(P, *const c_char, *mut c_uint, c_uint) -> c_int);
    std::fs::create_dir_all(tmp)?;
    let mut out = Vec::new();
    for (k, shot) in shots.iter().enumerate() {
        let wav: PathBuf = tmp.join(format!("level_{k}.wav"));
        let wc = CString::new(wav.to_string_lossy().as_bytes())?;
        let mut sys: P = std::ptr::null_mut();
        ck(unsafe { create(&mut sys, VERSION) }, "Studio System_Create")?;
        let res = (|| -> Result<usize> {
            let mut cs: P = std::ptr::null_mut();
            ck(unsafe { get_core(sys, &mut cs) }, "GetCoreSystem")?;
            ck(unsafe { set_output(cs, 5) }, "SetOutput(WAVWRITER_NRT)")?;
            ck(unsafe { set_format(cs, RATE, 3, 0) }, "SetSoftwareFormat")?;
            ck(unsafe { set_dsp(cs, BLOCK, 4) }, "SetDSPBufferSize")?;
            for pl in ["fmod_distance_filter.dll", "fmod_gain.dll", "resonanceaudio.dll"] {
                if let Ok(c) = CString::new(game.join(pl).to_string_lossy().as_bytes()) {
                    let mut h: c_uint = 0;
                    unsafe { load_plugin(cs, c.as_ptr(), &mut h, 0) };
                }
            }
            // studio SYNCHRONOUS_UPDATE, core NORMAL; extra driver data = the WAV path
            ck(unsafe { init(sys, 128, 2, 0, wc.as_ptr() as *mut c_void) }, "Initialize")?;
            for b in banks {
                let mut bk: P = std::ptr::null_mut();
                ck(unsafe { load_mem(sys, b.as_ptr() as *const c_char, b.len() as c_int, 0, 0, &mut bk) }, "LoadBankMemory")?;
            }
            let en = CString::new(shot.event)?;
            let mut ed: P = std::ptr::null_mut();
            ck(unsafe { get_event(sys, en.as_ptr(), &mut ed) }, &format!("GetEvent {}", shot.event))?;
            ck(unsafe { load_samples(ed) }, "LoadSampleData")?;
            let mut pre = 0usize;
            for _ in 0..4000 {
                let mut s: c_int = 0;
                unsafe { loading(ed, &mut s) };
                if s == 3 {
                    break;
                }
                ck(unsafe { update(sys) }, "Update")?;
                pre += 1;
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            let mut ins: P = std::ptr::null_mut();
            ck(unsafe { instance(ed, &mut ins) }, "CreateInstance")?;
            let lis = attr(0.0);
            unsafe { listener(sys, 0, &lis, std::ptr::null()) };
            let a = attr(shot.distance);
            unsafe { set3d(ins, &a) };
            let names: Vec<CString> = shot.ramps.iter().map(|r| CString::new(r.0).unwrap()).collect();
            let apply = |t: f32| {
                for (r, n) in shot.ramps.iter().zip(&names) {
                    let v = r.1 + (r.2 - r.1) * (t / shot.secs).clamp(0.0, 1.0);
                    unsafe { set_param(ins, n.as_ptr(), v, 0) };
                }
            };
            apply(0.0);
            ck(unsafe { start(ins) }, "Start")?;
            let dt = BLOCK as f32 / RATE as f32;
            let mut t = 0.0;
            while t < shot.secs {
                apply(t);
                ck(unsafe { update(sys) }, "Update")?;
                t += dt;
            }
            unsafe {
                stop(ins, 1);
                inst_release(ins);
                update(sys);
            }
            Ok(pre)
        })();
        unsafe { release(sys) };
        let pre = res?;
        out.push(wav_rms(&wav, pre * BLOCK as usize)?);
        let _ = std::fs::remove_file(&wav);
    }
    Ok(out)
}

/// RMS in dBFS of a 16-bit / float WAV, skipping `skip` frames.
fn wav_rms(p: &Path, skip: usize) -> Result<f32> {
    let b = std::fs::read(p)?;
    let mut i = 12;
    let (mut ch, mut bits, mut fmt) = (2usize, 16u16, 1u16);
    while i + 8 <= b.len() {
        let id = &b[i..i + 4];
        let n = u32::from_le_bytes(b[i + 4..i + 8].try_into().unwrap()) as usize;
        let body = &b[i + 8..(i + 8 + n).min(b.len())];
        if id == b"fmt " && body.len() >= 16 {
            fmt = u16::from_le_bytes([body[0], body[1]]);
            ch = u16::from_le_bytes([body[2], body[3]]) as usize;
            bits = u16::from_le_bytes([body[14], body[15]]);
        }
        if id == b"data" {
            let fs = ch * bits as usize / 8;
            let d = &body[(skip * fs).min(body.len())..];
            let (mut s, mut n) = (0f64, 0usize);
            if bits == 16 {
                for c in d.chunks_exact(2) {
                    let x = i16::from_le_bytes([c[0], c[1]]) as f64 / 32768.0;
                    s += x * x;
                    n += 1;
                }
            } else if bits == 32 && fmt == 3 {
                for c in d.chunks_exact(4) {
                    let x = f32::from_le_bytes(c.try_into().unwrap()) as f64;
                    s += x * x;
                    n += 1;
                }
            } else {
                bail!("WAV format {fmt}/{bits} bits");
            }
            return Ok((10.0 * (s / n.max(1) as f64 + 1e-18).log10()) as f32);
        }
        i += 8 + n + (n & 1);
    }
    bail!("{}: no data chunk", p.display())
}
