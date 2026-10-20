//! FSB5 with Vorbis samples (mode 15) instead of PCM16: ~1/6 of the size.
//!
//! FSB5 keeps no Vorbis headers. The runtime rebuilds the identification
//! header itself (blocksizes 256/2048) and finds the setup header by the
//! CRC32 stored in each sample's chunk 11 - in a table built into fmod.dll
//! (the same setups `sound::vorbis` reads from the user's fmod.dll and
//! decodes with: 161 in the 2.03 build the game ships). So a sample can only
//! be stored as Vorbis when our encoder's setup header is one of them:
//! measured, libvorbis/aoTuV at quality >= 0.6 for 44.1 / 48 kHz mono and
//! stereo produce exactly those (blocksizes 256/2048). Anything else (other
//! rates, more channels, a setup the table lacks) returns None and the bank
//! stays PCM16 - FSB5 has one codec for all its samples.
//!
//! Sample data = the audio packets as `u16 len + bytes` (as AC1's own FMOD
//! banks store them), zero-padded to 32 bytes. Chunk 11 = setup CRC32, the
//! byte length of the seek table, then (granule, byte offset) pairs about
//! once a second, as FMOD's fsbank writes them.

use anyhow::{anyhow, Result};
use std::num::{NonZeroU32, NonZeroU8};

use super::fsb::Sample;

/// One encoded sample: packet stream (FSB framing), setup CRC, seek table.
struct Enc {
    data: Vec<u8>,
    crc: u32,
    seek: Vec<(u32, u32)>,
}

/// Packets of an Ogg stream, in order.
fn ogg_packets(b: &[u8]) -> Vec<Vec<u8>> {
    let (mut pk, mut cur, mut i) = (Vec::new(), Vec::new(), 0usize);
    while i + 27 <= b.len() && &b[i..i + 4] == b"OggS" {
        let nseg = b[i + 26] as usize;
        let lac = &b[i + 27..(i + 27 + nseg).min(b.len())];
        let mut p = i + 27 + nseg;
        for &l in lac {
            let e = (p + l as usize).min(b.len());
            cur.extend_from_slice(&b[p..e]);
            p = e;
            if l < 255 {
                pk.push(std::mem::take(&mut cur));
            }
        }
        i = p;
    }
    pk
}

fn encode_one(s: &Sample, q: f32) -> Result<Option<Enc>> {
    if !matches!(s.rate, 44100 | 48000) || !(1..=2).contains(&s.channels) {
        return Ok(None);
    }
    let ch = s.channels as usize;
    let frames = s.pcm.len() / (2 * ch);
    let mut planes: Vec<Vec<f32>> = vec![Vec::with_capacity(frames); ch];
    for f in 0..frames {
        for c in 0..ch {
            let i = (f * ch + c) * 2;
            planes[c].push(i16::from_le_bytes([s.pcm[i], s.pcm[i + 1]]) as f32 / 32768.0);
        }
    }
    let mut ogg = Vec::new();
    {
        let mut b = vorbis_rs::VorbisEncoderBuilder::new_with_serial(NonZeroU32::new(s.rate).unwrap(), NonZeroU8::new(ch as u8).unwrap(), &mut ogg, 1);
        b.bitrate_management_strategy(vorbis_rs::VorbisBitrateManagementStrategy::QualityVbr { target_quality: q });
        let mut e = b.build().map_err(|e| anyhow!("vorbis encoder: {e}"))?;
        for chunk in (0..frames).step_by(1 << 16) {
            let end = (chunk + (1 << 16)).min(frames);
            let block: Vec<&[f32]> = planes.iter().map(|p| &p[chunk..end]).collect();
            e.encode_audio_block(&block).map_err(|e| anyhow!("vorbis encode: {e}"))?;
        }
        e.finish().map_err(|e| anyhow!("vorbis finish: {e}"))?;
    }
    let pk = ogg_packets(&ogg);
    if pk.len() < 3 || pk[0].len() < 30 || pk[0][28] != 0xb8 {
        return Ok(None); // blocksizes other than 256/2048: FMOD would decode garbage
    }
    let mut h = flate2::Crc::new();
    h.update(&pk[2]);
    let crc = h.sum();
    match crate::sound::vorbis::setup_header(crc) {
        Some(t) if t == &pk[2][..] => {}
        _ => return Ok(None),
    }
    let ident = lewton::header::read_header_ident(&pk[0]).map_err(|e| anyhow!("ident: {e:?}"))?;
    let setup = lewton::header::read_header_setup(&pk[2], ident.audio_channels, (ident.blocksize_0, ident.blocksize_1)).map_err(|e| anyhow!("setup: {e:?}"))?;
    let mut data = Vec::new();
    let mut seek = Vec::new();
    let mut granule: u64 = 0;
    let mut next_mark = s.rate as u64;
    let mut first = true;
    for p in &pk[3..] {
        if p.is_empty() || p.len() > 0xffff {
            if p.len() > 0xffff {
                return Ok(None);
            }
            continue;
        }
        let n = lewton::audio::get_decoded_sample_count(&ident, &setup, p).map_err(|e| anyhow!("packet: {e:?}"))? as u64;
        // the first packet only primes the window
        let n = if first { 0 } else { n };
        first = false;
        if granule >= next_mark && granule < frames as u64 {
            seek.push((granule as u32, data.len() as u32));
            next_mark += s.rate as u64;
        }
        data.extend_from_slice(&(p.len() as u16).to_le_bytes());
        data.extend_from_slice(p);
        granule += n;
    }
    Ok(Some(Enc { data, crc, seek }))
}

fn rate_code(r: u32) -> u64 {
    if r == 44100 { 8 } else { 9 }
}

/// FSB5 (mode 15) for `samples`, or None if any sample cannot be Vorbis.
pub fn fsb5_vorbis(samples: &[Sample], q: f32) -> Result<Option<Vec<u8>>> {
    use rayon::prelude::*;
    let enc: Vec<Option<Enc>> = samples.par_iter().map(|s| encode_one(s, q)).collect::<Result<Vec<_>>>()?;
    if enc.iter().any(|e| e.is_none()) {
        return Ok(None);
    }
    let enc: Vec<Enc> = enc.into_iter().map(|e| e.unwrap()).collect();
    let mut sh = Vec::new();
    let mut off: u64 = 0;
    for (s, e) in samples.iter().zip(&enc) {
        let frames = (s.pcm.len() / (2 * s.channels as usize)) as u64;
        let cc = if s.channels == 2 { 1 } else { 0 };
        let v: u64 = ((((frames << 27 | (off / 32)) << 2 | cc) << 4 | rate_code(s.rate)) << 1) | 1;
        sh.extend_from_slice(&v.to_le_bytes());
        // chunk 11: crc, seek table bytes, (granule, offset) pairs
        let mut pl = Vec::new();
        pl.extend_from_slice(&e.crc.to_le_bytes());
        pl.extend_from_slice(&((e.seek.len() * 8) as u32).to_le_bytes());
        for (g, o) in &e.seek {
            pl.extend_from_slice(&g.to_le_bytes());
            pl.extend_from_slice(&o.to_le_bytes());
        }
        let ch: u32 = (((pl.len() as u32) & 0xffffff) | 11 << 24) << 1 | 1;
        sh.extend_from_slice(&ch.to_le_bytes());
        sh.extend_from_slice(&pl);
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
        let mut peak: i16 = 0;
        for c in s.pcm.chunks_exact(2) {
            let x = i16::from_le_bytes([c[0], c[1]]).wrapping_abs();
            if x > peak {
                peak = x;
            }
        }
        let ch: u32 = ((4u32 & 0xffffff) | 13 << 24) << 1;
        sh.extend_from_slice(&ch.to_le_bytes());
        sh.extend_from_slice(&(peak as f32 / 32768.0).to_le_bytes());
        off += (e.data.len() as u64 + 31) / 32 * 32;
        if off / 32 > 0x7ff_ffff {
            return Ok(None);
        }
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
    for v in [1u32, n as u32, sh.len() as u32, nt.len() as u32, ds, 15] {
        hdr.extend_from_slice(&v.to_le_bytes());
    }
    // flags 1 at 0x1C: without it fmod.dll 2.03 refuses a Vorbis FSB5
    // (FMOD_ERR_VERSION); AC1's Vorbis banks carry it too
    hdr.extend_from_slice(&1u32.to_le_bytes());
    hdr.extend_from_slice(&[0u8; 28]);
    use md5::{Digest, Md5};
    let mut h = Md5::new();
    h.update(&sh);
    h.update(&hdr);
    let d = h.finalize();
    hdr[44..60].copy_from_slice(&d);
    let mut out = Vec::with_capacity(60 + sh.len() + nt.len() + ds as usize);
    out.extend_from_slice(&hdr);
    out.extend_from_slice(&sh);
    out.extend_from_slice(&nt);
    for e in &enc {
        out.extend_from_slice(&e.data);
        while out.len() % 32 != 0 {
            out.push(0);
        }
    }
    Ok(Some(out))
}

/// ACECONV_BANK_VORBIS=<quality 0.6..1.0> turns Vorbis banks on ("1" = 0.8).
pub fn quality() -> Option<f32> {
    let v = std::env::var("ACECONV_BANK_VORBIS").ok()?;
    let q: f32 = match v.as_str() {
        "0" | "" => return None,
        "1" => 0.8,
        x => x.parse().ok()?,
    };
    Some(q.clamp(0.6, 1.0))
}
