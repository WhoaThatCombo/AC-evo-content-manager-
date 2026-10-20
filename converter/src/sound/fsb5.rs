//! FSB5 sample blobs (inside FMOD banks): header + sample-header parse and
//! decode in-house (Vorbis via lewton + the game's setup-header table,
//! FADPCM, PCM16). Layout: docs/FORMATS.md, "FSB5".
//!
//! A second decoder drives FMOD Core from the game's own fmod.dll
//! (CreateSound OPENMEMORY|OPENONLY|CREATESTREAM, one subsound per sample,
//! ReadData) to cross-check the in-house one: ACECONV_FMOD_DECODE=1.

use anyhow::{anyhow, bail, Result};
use std::ffi::{c_char, c_int, c_uint, c_void};
use std::path::{Path, PathBuf};


/// `b[a..e]`, clamped to the buffer.
pub fn sl(b: &[u8], a: usize, e: usize) -> &[u8] {
    let e = e.min(b.len());
    &b[a.min(e)..e]
}

pub fn u32_(b: &[u8], o: usize) -> Result<u32> {
    b.get(o..o + 4).map(|x| u32::from_le_bytes(x.try_into().unwrap())).ok_or_else(|| anyhow!("read past end at {o:#x}"))
}

pub fn u64_(b: &[u8], o: usize) -> Result<u64> {
    b.get(o..o + 8).map(|x| u64::from_le_bytes(x.try_into().unwrap())).ok_or_else(|| anyhow!("read past end at {o:#x}"))
}

pub const HDR: usize = 0x3C;
pub const MODE_PCM16: u32 = 2;
pub const MODE_VORBIS: u32 = 0x0F;
pub const MODE_FADPCM: u32 = 0x10;

/// FSB5 sample-rate index (unknown -> 44100).
pub fn rate_of(fi: u32) -> u32 {
    match fi {
        1 => 8000, 2 => 11000, 3 => 11025, 4 => 16000, 5 => 22050, 6 => 24000,
        7 => 32000, 8 => 44100, 9 => 48000, 10 => 96000, _ => 44100,
    }
}

pub struct BlobChunk {
    pub ctype: u32,
    pub size: u32,
    pub payload: Vec<u8>,
}

pub struct BlobSample {
    pub freq_idx: u32,
    pub channels: u32,
    pub frames: u64,
    pub chunks: Vec<BlobChunk>,
    /// chunk 0x0B (Vorbis setup header CRC32), not re-emitted
    pub crc32: Option<u32>,
    /// data offset relative to the blob's data start
    pub data_off: usize,
}

pub struct BlobInfo {
    pub num_samples: usize,
    pub name_table_start: usize,
    pub name_table_size: usize,
    pub samples: Vec<BlobSample>,
    pub header_tail: Vec<u8>,
    pub mode: u32,
    pub data_start: usize,
    pub data_size: usize,
}

/// Header + sample headers. The Vorbis chunk 0x0B (setup CRC) is dropped
/// from `chunks`, so the re-emit leaves it out.
pub fn parse_blob(b: &[u8], base: usize) -> Result<BlobInfo> {
    if sl(b, base, base + 4) != b"FSB5" {
        bail!("FSB5 magic mismatch");
    }
    let ver = u32_(b, base + 4)?;
    if ver != 1 {
        bail!("unsupported FSB5 version {ver}");
    }
    let n = u32_(b, base + 8)? as usize;
    let shs = u32_(b, base + 12)? as usize;
    let nts = u32_(b, base + 16)? as usize;
    let mut samples = Vec::with_capacity(n);
    let mut i = base + HDR;
    for _ in 0..n {
        let v = u64_(b, i)?;
        i += 8;
        let mut more = v & 1;
        let mut s = BlobSample { freq_idx: ((v >> 1) & 0xF) as u32, channels: ((v >> 5) & 1) as u32 + 1, frames: (v >> 34) & 0x3FFF_FFFF, chunks: vec![],
            crc32: None, data_off: (((v >> 6) & 0xFFF_FFFF) * 16) as usize };
        while more != 0 {
            let z = u32_(b, i)?;
            i += 4;
            more = (z & 1) as u64;
            let size = (z >> 1) & 0xFF_FFFF;
            let ctype = (z >> 25) & 0x7F;
            if ctype != 0x0B {
                s.chunks.push(BlobChunk { ctype, size, payload: sl(b, i, i + size as usize).to_vec() });
            } else if size >= 4 {
                s.crc32 = Some(u32_(b, i)?);
            }
            i += size as usize;
        }
        samples.push(s);
    }
    Ok(BlobInfo { num_samples: n, name_table_start: base + HDR + shs, name_table_size: nts, samples, header_tail: sl(b, base + 0x1C, base + HDR).to_vec(),
        mode: u32_(b, base + 0x18)?, data_start: base + HDR + shs + nts, data_size: u32_(b, base + 0x14)? as usize })
}

/// Total byte length of an FSB5 blob.
pub fn blob_len(b: &[u8], base: usize) -> Result<usize> {
    Ok(HDR + u32_(b, base + 12)? as usize + u32_(b, base + 16)? as usize + u32_(b, base + 20)? as usize)
}

// ---------------------------------------------------------------- in-house decode
/// Vorbis float -> i16: round_ties_even(clamp(x) * 32767), the same as the
/// fmod.dll path below applies to the PCMFLOAT FMOD hands back.
pub fn f32_to_i16(x: f32) -> i16 {
    ((x as f64).clamp(-1.0, 1.0) * 32767.0).round_ties_even() as i16
}

/// Sample k's compressed bytes: dataOff[k] .. dataOff[k+1] (last: dataSize).
pub fn sample_data<'a>(b: &'a [u8], info: &BlobInfo, k: usize) -> &'a [u8] {
    let s = info.samples[k].data_off;
    let e = info.samples.get(k + 1).map(|n| n.data_off).unwrap_or(info.data_size).max(s);
    sl(b, info.data_start + s, info.data_start + e)
}

/// One blob -> per sample interleaved PCM16, sized frames*ch*2 (missing tail = 0).
pub fn decode_blob(b: &[u8], info: &BlobInfo) -> Result<Vec<Vec<u8>>> {
    // every sample decodes on its own (nohesi_bmw_m140: 87 Vorbis samples,
    // 9 s in a row): in parallel, collected in order
    use rayon::prelude::*;
    info.samples.par_iter().enumerate().map(|(k, s)| -> Result<Vec<u8>> {
        let d = sample_data(b, info, k);
        let frames = s.frames as usize;
        let pcm: Vec<i16> = match info.mode {
            MODE_PCM16 => d.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect(),
            MODE_VORBIS => {
                let crc = s.crc32.ok_or_else(|| anyhow!("Vorbis sample {k} has no setup CRC chunk"))?;
                let chs = super::vorbis::decode(d, s.channels, rate_of(s.freq_idx), crc, frames).map_err(|e| anyhow!("sample {k}: {e}"))?;
                let n = chs[0].len();
                let mut v = Vec::with_capacity(n * chs.len());
                for t in 0..n {
                    for c in &chs {
                        v.push(f32_to_i16(c[t]));
                    }
                }
                v
            }
            MODE_FADPCM => super::fadpcm::decode(d, s.channels, frames),
            m => bail!("unsupported FSB5 codec mode {m:#x}"),
        };
        let mut p: Vec<u8> = pcm.iter().flat_map(|x| x.to_le_bytes()).collect();
        p.resize(frames * s.channels as usize * 2, 0);
        Ok(p)
    }).collect()
}

/// ACECONV_FMOD_DECODE=1: decode through the game's fmod.dll instead.
pub fn use_fmod() -> bool {
    std::env::var("ACECONV_FMOD_DECODE").as_deref() == Ok("1")
}

// ---------------------------------------------------------------- FMOD decode (cross-check)
#[repr(C)]
struct ExInfo {
    cbsize: c_int, length: c_uint, fileoffset: c_uint, numchannels: c_int, defaultfrequency: c_int, format: c_int,
    decodebuffersize: c_uint, initialsubsound: c_int, numsubsounds: c_int, inclusionlist: *mut c_void, inclusionlistnum: c_int,
    pcmreadcallback: *mut c_void, pcmsetposcallback: *mut c_void, nonblockcallback: *mut c_void, dlsname: *const c_char,
    encryptionkey: *const c_char, maxpolyphony: c_int, userdata: *mut c_void, suggestedsoundtype: c_int,
    fileuseropen: *mut c_void, fileuserclose: *mut c_void, fileuserread: *mut c_void, fileuserseek: *mut c_void,
    fileuserasyncread: *mut c_void, fileuserasynccancel: *mut c_void, fileuserdata: *mut c_void, filebuffersize: c_int,
    channelorder: c_int, initialsoundgroup: *mut c_void, initialseekposition: c_uint, initialseekpostype: c_int,
    ignoresetfilesystem: c_int, audioqueuepolicy: c_uint, minmidigranularity: c_uint, nonblockthreadid: c_int, fsbguid: *mut c_void,
}

const FMOD_VERSION: c_uint = 0x0002_0313;
const FMOD_OPENMEMORY: c_uint = 0x800;
const FMOD_OPENONLY: c_uint = 0x8;
const FMOD_CREATESTREAM: c_uint = 0x80;
const FMOD_TIMEUNIT_PCM: c_uint = 2;
const FMOD_OUTPUTTYPE_NOSOUND_NRT: c_int = 4;
const FMT_PCM16: c_int = 2;
const FMT_PCMFLOAT: c_int = 5;

pub fn game_dir() -> PathBuf {
    std::env::var_os("ACE_EVO_DIR").map(PathBuf::from)
        .unwrap_or_else(crate::io::gamedir::evo_dir)
}

type Sys = *mut c_void;
type Snd = *mut c_void;

/// The game's fmod.dll, NOSOUND_NRT output (nothing reaches the speakers).
pub struct Decoder {
    lib: libloading::Library,
    sys: Sys,
}

macro_rules! sym {
    ($lib:expr, $name:literal, $t:ty) => {
        unsafe { *$lib.get::<$t>($name).map_err(|e| anyhow!("{}: {e}", String::from_utf8_lossy($name)))? }
    };
}

fn ck(r: c_int, what: &str) -> Result<()> {
    if r == 0 { Ok(()) } else { bail!("FMOD {what} failed: error {r}") }
}

impl Decoder {
    pub fn new() -> Result<Decoder> {
        let dll = std::env::var_os("FMOD_DLL").map(PathBuf::from).unwrap_or_else(|| game_dir().join("fmod.dll"));
        Self::open(&dll)
    }
    fn open(dll: &Path) -> Result<Decoder> {
        let lib = unsafe { libloading::Library::new(dll) }.map_err(|e| anyhow!("{}: {e}", dll.display()))?;
        let create = sym!(lib, b"FMOD_System_Create", unsafe extern "system" fn(*mut Sys, c_uint) -> c_int);
        let set_out = sym!(lib, b"FMOD_System_SetOutput", unsafe extern "system" fn(Sys, c_int) -> c_int);
        let init = sym!(lib, b"FMOD_System_Init", unsafe extern "system" fn(Sys, c_int, c_uint, *mut c_void) -> c_int);
        let mut sys: Sys = std::ptr::null_mut();
        ck(unsafe { create(&mut sys, FMOD_VERSION) }, "System_Create")?;
        let d = Decoder { lib, sys };
        ck(unsafe { set_out(d.sys, FMOD_OUTPUTTYPE_NOSOUND_NRT) }, "SetOutput")?;
        ck(unsafe { init(d.sys, 8, 0, std::ptr::null_mut()) }, "System_Init")?;
        Ok(d)
    }

    /// One FSB5 blob -> per sample (interleaved PCM16, FMOD frame count, channels).
    pub fn decode_fsb(&self, blob: &[u8]) -> Result<Vec<(Vec<u8>, u32, u32)>> {
        let l = &self.lib;
        let create = sym!(l, b"FMOD_System_CreateSound", unsafe extern "system" fn(Sys, *const c_char, c_uint, *mut ExInfo, *mut Snd) -> c_int);
        let nsubs = sym!(l, b"FMOD_Sound_GetNumSubSounds", unsafe extern "system" fn(Snd, *mut c_int) -> c_int);
        let getsub = sym!(l, b"FMOD_Sound_GetSubSound", unsafe extern "system" fn(Snd, c_int, *mut Snd) -> c_int);
        let format = sym!(l, b"FMOD_Sound_GetFormat", unsafe extern "system" fn(Snd, *mut c_int, *mut c_int, *mut c_int, *mut c_int) -> c_int);
        let length = sym!(l, b"FMOD_Sound_GetLength", unsafe extern "system" fn(Snd, *mut c_uint, c_uint) -> c_int);
        let seek = sym!(l, b"FMOD_Sound_SeekData", unsafe extern "system" fn(Snd, c_uint) -> c_int);
        let read = sym!(l, b"FMOD_Sound_ReadData", unsafe extern "system" fn(Snd, *mut c_void, c_uint, *mut c_uint) -> c_int);
        let release = sym!(l, b"FMOD_Sound_Release", unsafe extern "system" fn(Snd) -> c_int);
        let mut ex: ExInfo = unsafe { std::mem::zeroed() };
        ex.cbsize = std::mem::size_of::<ExInfo>() as c_int;
        ex.length = blob.len() as c_uint;
        let mut snd: Snd = std::ptr::null_mut();
        ck(unsafe { create(self.sys, blob.as_ptr() as *const c_char, FMOD_OPENMEMORY | FMOD_OPENONLY | FMOD_CREATESTREAM, &mut ex, &mut snd) }, "CreateSound")?;
        let run = || -> Result<Vec<(Vec<u8>, u32, u32)>> {
            let mut n: c_int = 0;
            ck(unsafe { nsubs(snd, &mut n) }, "GetNumSubSounds")?;
            let mut out = Vec::new();
            for k in 0..n {
                // stream subsounds belong to the parent: never released here
                let mut sub: Snd = std::ptr::null_mut();
                ck(unsafe { getsub(snd, k, &mut sub) }, "GetSubSound")?;
                let (mut typ, mut fmt, mut ch, mut bits) = (0, 0, 0, 0);
                ck(unsafe { format(sub, &mut typ, &mut fmt, &mut ch, &mut bits) }, "GetFormat")?;
                let mut ln: c_uint = 0;
                ck(unsafe { length(sub, &mut ln, FMOD_TIMEUNIT_PCM) }, "GetLength")?;
                let bps = match fmt { FMT_PCM16 => 2, FMT_PCMFLOAT => 4, f => bail!("unexpected FMOD decode format {f}") };
                let want = ln as usize * ch as usize * bps;
                let mut data = vec![0u8; want];
                let mut got_total = 0usize;
                unsafe { seek(sub, 0) };
                while got_total < want {
                    let mut got: c_uint = 0;
                    let r = unsafe { read(sub, data.as_mut_ptr().add(got_total) as *mut c_void, (want - got_total) as c_uint, &mut got) };
                    got_total += got as usize;
                    if r != 0 || got == 0 {
                        break; // FMOD_ERR_FILE_EOF
                    }
                }
                data.truncate(got_total);
                if fmt == FMT_PCMFLOAT {
                    data = data.chunks_exact(4).flat_map(|c| {
                        let x = f32::from_le_bytes(c.try_into().unwrap()) as f64;
                        ((x.clamp(-1.0, 1.0) * 32767.0).round_ties_even() as i16).to_le_bytes()
                    }).collect();
                }
                out.push((data, ln, ch as u32));
            }
            Ok(out)
        };
        let r = run();
        unsafe { release(snd) };
        r
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        if let Ok(f) = unsafe { self.lib.get::<unsafe extern "system" fn(Sys) -> c_int>(b"FMOD_System_Release") } {
            unsafe { f(self.sys) };
        }
    }
}

// ---------------------------------------------------------------- locating + WAV
/// Offsets of every FSB5 blob in a bank (or any buffer): the `FSB5` magic,
/// version 1, and a header whose sizes fit the buffer.
pub fn blobs_of(b: &[u8]) -> Result<Vec<usize>> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(p) = memchr::memmem::find(&b[i..], b"FSB5") {
        let o = i + p;
        let ok = u32_(b, o + 4).map(|v| v == 1).unwrap_or(false) && blob_len(b, o).map(|n| o + n <= b.len()).unwrap_or(false);
        if ok {
            out.push(o);
            i = o + blob_len(b, o)?;
        } else {
            i = o + 4;
        }
    }
    Ok(out)
}

/// A 44-byte-header PCM16 WAV.
pub fn wav(pcm: &[u8], ch: u32, rate: u32) -> Vec<u8> {
    let mut h = b"RIFF".to_vec();
    h.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    h.extend_from_slice(b"WAVEfmt ");
    h.extend_from_slice(&16u32.to_le_bytes());
    h.extend_from_slice(&1u16.to_le_bytes());
    h.extend_from_slice(&(ch as u16).to_le_bytes());
    h.extend_from_slice(&rate.to_le_bytes());
    h.extend_from_slice(&(rate * ch * 2).to_le_bytes());
    h.extend_from_slice(&((ch * 2) as u16).to_le_bytes());
    h.extend_from_slice(&16u16.to_le_bytes());
    h.extend_from_slice(b"data");
    h.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    h.extend_from_slice(pcm);
    h
}
