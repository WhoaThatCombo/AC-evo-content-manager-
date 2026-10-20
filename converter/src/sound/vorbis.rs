//! FSB5 Vorbis (mode 0x0F) -> f32, pure Rust (lewton).
//!
//! FSB5 strips the three Vorbis headers:
//! - the identification header is rebuilt from the sample's channels and
//!   rate, per the Vorbis I spec (section 4.2.2): blocksizes 256 / 2048
//!   (byte 28 = 0xB8), framing bit set;
//! - the setup header is found by the CRC32 stored in the sample's chunk 11,
//!   in the table FMOD keeps inside fmod.dll. That table is read at run time
//!   from the user's own game install ([`setups`]); nothing of FMOD's is
//!   shipped. Layout: docs/FORMATS.md, "FMOD Vorbis setup table".
//! - audio packets are `u16 len + bytes` (len 0 = zero padding to the end).

use anyhow::{anyhow, bail, Result};
use lewton::audio::{read_audio_packet_generic, PreviousWindowRight};
use lewton::header::{read_header_ident, read_header_setup};
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

/// CRC-32 (IEEE, reflected, as zlib's `crc32`).
pub fn crc32(b: &[u8]) -> u32 {
    static T: OnceLock<[u32; 256]> = OnceLock::new();
    let t = T.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, v) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *v = c;
        }
        t
    });
    let mut c = !0u32;
    for &x in b {
        c = t[((c ^ x as u32) & 0xff) as usize] ^ (c >> 8);
    }
    !c
}

struct Pe<'a> {
    d: &'a [u8],
    base: u64,
    /// (rva, virtual size, file offset, raw size)
    secs: Vec<(u64, u64, u64, u64)>,
}

impl<'a> Pe<'a> {
    fn parse(d: &'a [u8]) -> Option<Pe<'a>> {
        let rd16 = |o: usize| d.get(o..o + 2).map(|x| u16::from_le_bytes(x.try_into().unwrap()));
        let rd32 = |o: usize| d.get(o..o + 4).map(|x| u32::from_le_bytes(x.try_into().unwrap()));
        let pe = rd32(0x3c)? as usize;
        if d.get(pe..pe + 4)? != b"PE\0\0" {
            return None;
        }
        let ns = rd16(pe + 6)? as usize;
        let opt = rd16(pe + 20)? as usize;
        let base = u64::from_le_bytes(d.get(pe + 24 + 24..pe + 24 + 32)?.try_into().ok()?);
        let mut secs = Vec::new();
        for i in 0..ns {
            let s = pe + 24 + opt + 40 * i;
            secs.push((rd32(s + 12)? as u64, rd32(s + 8)? as u64, rd32(s + 20)? as u64, rd32(s + 16)? as u64));
        }
        Some(Pe { d, base, secs })
    }
    /// file bytes at virtual address `va`, `n` long
    fn at(&self, va: u64, n: usize) -> Option<&'a [u8]> {
        let r = va.checked_sub(self.base)?;
        for &(rva, vs, off, rs) in &self.secs {
            if r >= rva && r < rva + vs.max(rs) {
                let o = (r - rva + off) as usize;
                return self.d.get(o..o + n);
            }
        }
        None
    }
}

/// The setup headers of an fmod.dll, by CRC32.
///
/// The table is an array of 32-byte entries: {u64 pointer, u32 length,
/// u32 crc32} then {u64 base pointer, u32 offset, u32 count}. With a null
/// base the header is the `length` bytes at `pointer`; otherwise it is the
/// base header's first `length` bytes with `count` bytes at `offset`
/// replaced by the bytes at `pointer`. Each entry is accepted only when the
/// rebuilt header starts with `\x05vorbis` and its CRC32 equals the stored
/// one, so a different FMOD build either yields its own table or nothing.
pub fn setups_from_dll(d: &[u8]) -> HashMap<u32, Vec<u8>> {
    let mut out = HashMap::new();
    let Some(pe) = Pe::parse(d) else { return out };
    let rd = |o: usize| -> (u64, u32, u32) {
        (u64::from_le_bytes(d[o..o + 8].try_into().unwrap()), u32::from_le_bytes(d[o + 8..o + 12].try_into().unwrap()), u32::from_le_bytes(d[o + 12..o + 16].try_into().unwrap()))
    };
    let mut o = 0;
    while o + 32 <= d.len() {
        let (p, len, crc) = rd(o);
        if p >> 32 != pe.base >> 32 || len < 16 || len > 0x10000 {
            o += 8;
            continue;
        }
        let (q, off, cnt) = rd(o + 16);
        let len = len as usize;
        let hdr = if q == 0 {
            if off != 0 || cnt != 0 {
                o += 8;
                continue;
            }
            pe.at(p, len).map(|s| s.to_vec())
        } else {
            let (off, cnt) = (off as usize, cnt as usize);
            match (pe.at(q, len), pe.at(p, cnt)) {
                (Some(b), Some(patch)) if off + cnt <= len => {
                    let mut v = b.to_vec();
                    v[off..off + cnt].copy_from_slice(patch);
                    Some(v)
                }
                _ => None,
            }
        };
        match hdr {
            Some(h) if h.starts_with(b"\x05vorbis") && crc32(&h) == crc => {
                out.insert(crc, h);
                o += 32;
            }
            _ => o += 8,
        }
    }
    out
}

/// The game's fmod.dll: FMOD_DLL, else `<game>\fmod.dll`.
pub fn fmod_dll() -> std::path::PathBuf {
    std::env::var_os("FMOD_DLL").map(std::path::PathBuf::from).unwrap_or_else(|| super::fsb5::game_dir().join("fmod.dll"))
}

/// The setup table of the game's fmod.dll, read once.
pub fn setups() -> &'static HashMap<u32, Vec<u8>> {
    static S: OnceLock<HashMap<u32, Vec<u8>>> = OnceLock::new();
    S.get_or_init(|| load(&fmod_dll()))
}

fn load(p: &Path) -> HashMap<u32, Vec<u8>> {
    std::fs::read(p).map(|d| setups_from_dll(&d)).unwrap_or_default()
}

/// Setup header bytes for a CRC32.
pub fn setup_header(crc: u32) -> Option<&'static [u8]> {
    setups().get(&crc).map(|v| v.as_slice())
}

/// 30-byte Vorbis identification header (Vorbis I spec 4.2.2).
fn ident_header(channels: u32, rate: u32) -> [u8; 30] {
    let mut h = [0u8; 30];
    h[..7].copy_from_slice(b"\x01vorbis");
    // version 0 (bytes 7..11)
    h[11] = channels as u8;
    h[12..16].copy_from_slice(&rate.to_le_bytes());
    // bitrates max / nominal / min left 0 (bytes 16..28)
    h[28] = 0xB8; // blocksize_0 = 2^8, blocksize_1 = 2^11
    h[29] = 1; // framing
    h
}

/// FSB5 packet framing: `u16 len + bytes`; a zero length skips zero padding.
fn packets(d: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut h = 0usize;
    while h + 2 <= d.len() {
        let a = u16::from_le_bytes([d[h], d[h + 1]]) as usize;
        if a == 0 {
            let mut t = h;
            while t < d.len() && d[t] == 0 {
                t += 1;
            }
            if t >= d.len() {
                break;
            }
            h = t;
            continue;
        }
        if h + 2 + a > d.len() {
            break;
        }
        out.push(&d[h + 2..h + 2 + a]);
        h += 2 + a;
    }
    out
}

/// One sample's packets -> per-channel f32, cut to `frames` (no padding added).
pub fn decode(data: &[u8], channels: u32, rate: u32, crc: u32, frames: usize) -> Result<Vec<Vec<f32>>> {
    let setup = setup_header(crc).ok_or_else(|| anyhow!("Vorbis setup header crc32 {crc:#010x} is not in {}", fmod_dll().display()))?;
    let ident = read_header_ident(&ident_header(channels, rate)).map_err(|e| anyhow!("vorbis id header: {e:?}"))?;
    let setup = read_header_setup(setup, ident.audio_channels, (ident.blocksize_0, ident.blocksize_1))
        .map_err(|e| anyhow!("vorbis setup header {crc:#010x}: {e:?}"))?;
    let mut pwr = PreviousWindowRight::new();
    let mut out: Vec<Vec<f32>> = vec![Vec::with_capacity(frames); channels as usize];
    for p in packets(data) {
        let dec: Vec<Vec<f32>> = read_audio_packet_generic(&ident, &setup, p, &mut pwr).map_err(|e| anyhow!("vorbis packet: {e:?}"))?;
        if dec.len() != out.len() {
            bail!("vorbis packet channel count {} != {}", dec.len(), out.len());
        }
        for (o, d) in out.iter_mut().zip(dec) {
            o.extend_from_slice(&d);
        }
        if out[0].len() >= frames {
            break;
        }
    }
    for o in &mut out {
        o.truncate(frames);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_known() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }

    /// Runs where the game is installed: the table must parse and every
    /// header must be one lewton accepts.
    #[test]
    fn game_table() {
        let p = fmod_dll();
        if !p.is_file() {
            return;
        }
        let t = load(&p);
        assert!(t.len() >= 100, "only {} setups in {}", t.len(), p.display());
        let ident = read_header_ident(&ident_header(2, 48000)).unwrap();
        let ok = t.values().filter(|h| read_header_setup(h, 2, (ident.blocksize_0, ident.blocksize_1)).is_ok()).count();
        assert!(ok > 0);
    }
}
