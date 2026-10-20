//! FSB5 (version 1, PCM16) writer as FMOD Studio's fsbank_writer_fsb5.cpp lays it out.
//! docs/FORMATS.md, "FSB5".

use anyhow::{bail, Result};
use md5::{Digest, Md5};

pub struct Sample {
    pub name: String,
    pub rate: u32,
    pub channels: u16,
    /// interleaved PCM16 little-endian
    pub pcm: Vec<u8>,
    /// WAV cue points (sample offset, label) in cue order
    pub sync: Vec<(u32, String)>,
}

/// A decoded PCM16 WAV.
pub struct Wav {
    pub channels: u16,
    pub rate: u32,
    pub pcm: Vec<u8>,
    pub sync: Vec<(u32, String)>,
}

fn rate_code(r: u32) -> Option<u64> {
    Some(match r {
        4000 => 0, 8000 => 1, 11000 => 2, 12000 => 3, 16000 => 4, 22050 => 5, 24000 => 6, 32000 => 7, 44100 => 8, 48000 => 9,
        96000 => 10,
        _ => return None,
    })
}

fn chan_code(c: u16) -> Option<u64> {
    Some(match c {
        1 => 0,
        2 => 1,
        6 => 2,
        8 => 3,
        _ => return None,
    })
}

/// FSB5 file bytes for `samples` (already in bank order).
pub fn fsb5(samples: &[Sample]) -> Result<Vec<u8>> {
    let mut sh = Vec::new();
    let mut off: u64 = 0;
    for s in samples {
        let blk = 2 * s.channels as usize;
        if s.pcm.len() % blk != 0 {
            bail!("{}: PCM length not a whole number of frames", s.name);
        }
        let frames = (s.pcm.len() / blk) as u64;
        // channel counts outside the 1/2/6/8 codes: code 0 plus a type-14 chunk with the count (after the peak)
        let cc_x = chan_code(s.channels);
        let (Some(rc), cc) = (rate_code(s.rate), cc_x.unwrap_or(0)) else {
            return super::compile::unsupported(format!("{}: FSB5 sample rate {} Hz", s.name, s.rate));
        };
        if s.channels == 0 || s.channels > 32 {
            return super::compile::unsupported(format!("{}: FSB5 channel count {}", s.name, s.channels));
        }
        if off / 32 > 0x7ff_ffff || frames > 0x3fff_ffff {
            return super::compile::unsupported("FSB5 too large");
        }
        let v: u64 = ((((frames << 27 | (off / 32)) << 2 | cc) << 4 | rc) << 1) | 1;
        sh.extend_from_slice(&v.to_le_bytes());
        // chunk 13 = peak: max |sample| / 32768 with the abs taken in i16 (so -32768
        // wraps negative and never wins), as fsbank does
        let mut peak: i16 = 0;
        for c in s.pcm.chunks_exact(2) {
            let x = i16::from_le_bytes([c[0], c[1]]).wrapping_abs();
            if x > peak {
                peak = x;
            }
        }
        let peak = peak as f32 / 32768.0;
        // sync points: type 4 (offset + 256-byte name) when any cue has a label, else type 5 (offsets)
        if !s.sync.is_empty() {
            let named = s.sync.iter().any(|x| !x.1.is_empty());
            let mut pl = Vec::new();
            for (o, n) in &s.sync {
                pl.extend_from_slice(&o.to_le_bytes());
                if named {
                    let mut nm = [0u8; 256];
                    let b = n.as_bytes();
                    nm[..b.len().min(255)].copy_from_slice(&b[..b.len().min(255)]);
                    pl.extend_from_slice(&nm);
                }
            }
            let t: u32 = if named { 4 } else { 5 };
            let ch: u32 = (((pl.len() as u32) & 0xffffff) | t << 24) << 1 | 1;
            sh.extend_from_slice(&ch.to_le_bytes());
            sh.extend_from_slice(&pl);
        }
        let ch: u32 = ((4u32 & 0xffffff) | 13 << 24) << 1 | cc_x.is_none() as u32;
        sh.extend_from_slice(&ch.to_le_bytes());
        sh.extend_from_slice(&peak.to_le_bytes());
        if cc_x.is_none() {
            let ch: u32 = ((4u32 & 0xffffff) | 14 << 24) << 1;
            sh.extend_from_slice(&ch.to_le_bytes());
            sh.extend_from_slice(&(s.channels as u32).to_le_bytes());
        }
        off += (s.pcm.len() as u64 + 16 * blk as u64 + 31) / 32 * 32;
    }
    let n = samples.len();
    let mut nt = Vec::new();
    let mut names = Vec::new();
    for s in samples {
        nt.extend_from_slice(&((4 * n + names.len()) as u32).to_le_bytes());
        names.extend_from_slice(s.name.as_bytes());
        names.push(0);
    }
    nt.extend_from_slice(&names);
    while (60 + sh.len() + nt.len()) % 32 != 0 {
        nt.push(0);
    }
    let ds = off as u32;
    let mut hdr = Vec::with_capacity(60);
    hdr.extend_from_slice(b"FSB5");
    for v in [1u32, n as u32, sh.len() as u32, nt.len() as u32, ds, 2] {
        hdr.extend_from_slice(&v.to_le_bytes());
    }
    hdr.extend_from_slice(&[0u8; 32]);
    let mut h = Md5::new();
    h.update(&sh);
    h.update(&hdr);
    let d = h.finalize();
    hdr[44..60].copy_from_slice(&d);
    let mut out = Vec::with_capacity(60 + sh.len() + nt.len() + ds as usize);
    out.extend_from_slice(&hdr);
    out.extend_from_slice(&sh);
    out.extend_from_slice(&nt);
    // each sample: PCM + 16 zero frames, then zero-padded to 32 bytes
    for s in samples {
        out.extend_from_slice(&s.pcm);
        out.resize(out.len() + 32 * s.channels as usize, 0);
        while out.len() % 32 != 0 {
            out.push(0);
        }
    }
    Ok(out)
}

/// PCM16 WAV (fmt, data, cue points + adtl labels).
pub fn read_wav(b: &[u8]) -> Result<Wav> {
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        bail!("not a RIFF WAVE file");
    }
    let rd = |x: &[u8], o: usize| u32::from_le_bytes(x[o..o + 4].try_into().unwrap());
    let mut i = 12;
    let mut fmt: Option<(u16, u16, u32, u16)> = None;
    let mut data: Option<Vec<u8>> = None;
    let mut cues: Vec<(u32, u32)> = Vec::new();
    let mut labels: Vec<(u32, String)> = Vec::new();
    while i + 8 <= b.len() {
        let id = &b[i..i + 4];
        let sz = rd(b, i + 4) as usize;
        let body = &b[i + 8..(i + 8 + sz).min(b.len())];
        if id == b"fmt " && body.len() >= 16 {
            fmt = Some((
                u16::from_le_bytes([body[0], body[1]]),
                u16::from_le_bytes([body[2], body[3]]),
                rd(body, 4),
                u16::from_le_bytes([body[14], body[15]]),
            ));
        } else if id == b"data" {
            data = Some(body.to_vec());
        } else if id == b"cue " && body.len() >= 4 {
            let n = rd(body, 0) as usize;
            for k in 0..n {
                let o = 4 + 24 * k;
                if o + 24 <= body.len() {
                    cues.push((rd(body, o), rd(body, o + 20)));
                }
            }
        } else if id == b"LIST" && body.len() >= 4 && &body[0..4] == b"adtl" {
            let mut j = 4;
            while j + 8 <= body.len() {
                let sid = &body[j..j + 4];
                let n = rd(body, j + 4) as usize;
                let sb = &body[j + 8..(j + 8 + n).min(body.len())];
                if sid == b"labl" && sb.len() >= 4 {
                    let t = &sb[4..];
                    let t = &t[..t.iter().position(|c| *c == 0).unwrap_or(t.len())];
                    labels.push((rd(sb, 0), String::from_utf8_lossy(t).into_owned()));
                }
                j += 8 + n + (n & 1);
            }
        }
        i += 8 + sz + (sz & 1);
    }
    let Some((tag, ch, rate, bits)) = fmt else { bail!("WAV without fmt") };
    let Some(pcm) = data else { bail!("WAV without data chunk") };
    if tag != 1 || bits != 16 {
        return super::compile::unsupported(format!("WAV is not PCM16 (format {tag}, {bits} bits)"));
    }
    let frames = (pcm.len() / (2 * ch.max(1) as usize)) as i64;
    let sync = cues.iter().map(|(id, off)| {
        let o = (*off as i32 as i64).clamp(0, frames) as u32;
        (o, labels.iter().find(|l| l.0 == *id).map(|l| l.1.clone()).unwrap_or_default())
    }).collect();
    Ok(Wav { channels: ch, rate, pcm, sync })
}
