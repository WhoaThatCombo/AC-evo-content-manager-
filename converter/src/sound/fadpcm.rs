//! FMOD FADPCM (FSB5 mode 0x10) -> PCM16, pure Rust. No fmod.dll.
//!
//! Frames of 0x8C bytes per channel, channels interleaved frame by frame:
//! u32 coef nibbles, u32 shift nibbles, i16 hist1, i16 hist2, then 8 groups of
//! 16 bytes (32 nibbles each, low nibble first) = 256 samples. XA-style
//! predictor: s = ((nib << 28) >> (22 - shift)) + h1*c1 - h2*c2, then >> 6,
//! clamped to i16. Header samples are not output.

const FRAME: usize = 0x8C;
const SPF: usize = 256;
const COEFS: [[i32; 2]; 8] = [[0, 0], [60, 0], [122, 60], [115, 52], [98, 55], [0, 0], [0, 0], [0, 0]];

fn frame(f: &[u8], out: &mut Vec<i16>) {
    let rd = |o: usize| u32::from_le_bytes(f[o..o + 4].try_into().unwrap());
    let coefs = rd(0);
    let shifts = rd(4);
    let mut h1 = i16::from_le_bytes([f[8], f[9]]) as i32;
    let mut h2 = i16::from_le_bytes([f[10], f[11]]) as i32;
    for i in 0..8 {
        let [c1, c2] = COEFS[(((coefs >> (i * 4)) & 0xF) % 7) as usize];
        let shift = 22 - ((shifts >> (i * 4)) & 0xF) as i32;
        for j in 0..4 {
            let nib = rd(0x0C + 0x10 * i + 4 * j);
            for k in 0..8 {
                let n = (((nib >> (k * 4)) & 0xF) << 28) as i32;
                let s = (((n >> shift) - h2 * c2 + h1 * c1) >> 6).clamp(-32768, 32767);
                out.push(s as i16);
                h2 = h1;
                h1 = s;
            }
        }
    }
}

/// One sample's data -> interleaved PCM16 cut to `frames` (no padding added).
pub fn decode(data: &[u8], channels: u32, frames: usize) -> Vec<i16> {
    let ch = channels.max(1) as usize;
    let nblk = data.len() / (FRAME * ch);
    let mut per: Vec<Vec<i16>> = vec![Vec::with_capacity(nblk * SPF); ch];
    for b in 0..nblk {
        for (c, p) in per.iter_mut().enumerate() {
            let o = (b * ch + c) * FRAME;
            frame(&data[o..o + FRAME], p);
        }
    }
    let n = frames.min(nblk * SPF);
    let mut out = Vec::with_capacity(n * ch);
    for t in 0..n {
        for p in &per {
            out.push(p[t]);
        }
    }
    out
}
