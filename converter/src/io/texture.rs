//! Source images (kn5-embedded DDS / PNG / JPG / TGA) -> EVO `.texture`
//! (protobuf header) + `.texturemips` (pixels).
//!
//! `.texturemips` layout (checked against the stock files of content.kspkg): each mip
//! is cut into tiles (BC7/BC6H 256x256, BC1 512x256, RGBA8 128x128); EVERY
//! tile fills one whole 64 KiB page, padded on both axes; tiles row-major;
//! inside a tile the row pitch is the tile's row pitch. Header field 12 is
//! the page table: tile w/h, first page per mip, pages per mip, and one
//! "1024" (64 KiB in 64-byte units) per page.

use anyhow::{anyhow, bail, Result};

use crate::io::pb::{self, Msg, Val};

pub const PAGE: usize = 65536;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fmt {
    /// colour / masks with alpha (header enum 34)
    Bc7,
    /// track normal maps: BC7, linear (33)
    Bc7Linear,
    /// normal maps (26)
    Bc6h,
    /// BC1 for `_d` style maps (13)
    Bc1D,
    /// BC1, linear (12)
    Bc1,
    /// BC4 single channel: neutrals, badges (27)
    Bc4,
    /// uncompressed RGBA8 (6 / 5)
    Rgba8,
    Rgba8b,
    /// UI art (brand badges / logos): uncompressed 8-bit sRGB stored B, G,
    /// R, A, with the full mip chain - header enum 6 with import settings
    /// {f3: 2}, as every stock `uiresources\branding\oem\*\*.texture`
    Ui,
}

impl Fmt {
    fn code(self) -> u64 {
        match self { Fmt::Bc7 => 34, Fmt::Bc7Linear => 33, Fmt::Bc6h => 26, Fmt::Bc1D => 13, Fmt::Bc1 => 12, Fmt::Bc4 => 27, Fmt::Rgba8 | Fmt::Ui => 6, Fmt::Rgba8b => 5 }
    }
    fn tile(self) -> (u32, u32) {
        match self { Fmt::Bc7 | Fmt::Bc7Linear | Fmt::Bc6h => (256, 256), Fmt::Bc1D | Fmt::Bc1 | Fmt::Bc4 => (512, 256), _ => (128, 128) }
    }
    fn block(self) -> bool { !matches!(self, Fmt::Rgba8 | Fmt::Rgba8b | Fmt::Ui) }
    fn row_bytes(self, w: u32) -> usize {
        match self {
            Fmt::Bc7 | Fmt::Bc7Linear | Fmt::Bc6h => (w.div_ceil(4).max(1) * 16) as usize,
            Fmt::Bc1 | Fmt::Bc1D | Fmt::Bc4 => (w.div_ceil(4).max(1) * 8) as usize,
            _ => (w * 4) as usize,
        }
    }
    /// Header field 11 (import settings) for an image written at its source
    /// size (`levels` = mip count). Measured over the 38.5k `.texture`
    /// headers of a stock content.kspkg (`aceconv stockstats`, "textures"):
    ///
    /// | enum | f1 | f9 | f11 | f13 | most common stock use |
    /// |---|---|---|---|---|---|
    /// | 34 BC7 sRGB | - | 1 | 0.5 | - | car `_ao`, `_c`, track albedo |
    /// | 33 BC7 linear | 1 | 1 | 0.5 | - | track `_mask`, car `_wet` |
    /// | 26 BC6H | 3 | 1 | 0.5 | - | every `_nm` / `_normal` |
    /// | 13 BC1 sRGB | 8 | 1 | 0.5 | - | car `_d`, track `_alb` |
    /// | 12 BC1 linear | 5 | 1 | 0.5 | - | car `_map`, `_wet`, track `_nm` |
    /// | 27 BC4 | 2 | 1 | 0.5 | - | `_r`, `_m`, `_at`, track roughness / ao |
    /// | 6 RGBA8 sRGB | - | - | 0.5 | levels | car `ext_lights_f_*` masks |
    /// | 5 RGBA8 linear | 2 | - | 0.5 | levels | car `int_*_f` function masks |
    ///
    /// f2 (present on a third of the stock files) is log2(source / texture
    /// size) and is left out: the converter records the size it encodes.
    fn import_settings(self, levels: usize) -> Msg {
        let mut m = Msg::default();
        let kind = match self {
            Fmt::Bc7 => None,
            Fmt::Bc7Linear => Some(1),
            Fmt::Bc6h => Some(3),
            Fmt::Bc1D => Some(8),
            Fmt::Bc1 => Some(5),
            Fmt::Bc4 => Some(2),
            Fmt::Rgba8 | Fmt::Ui => None,
            Fmt::Rgba8b => Some(2),
        };
        if self == Fmt::Ui {
            m.set_varint(3, 2);
            m.set_f32(11, 0.5);
            return m;
        }
        if let Some(k) = kind {
            m.set_varint(1, k);
        }
        if self.block() {
            m.set_varint(9, 1);
        }
        m.set_f32(11, 0.5);
        if !self.block() {
            m.set_varint(13, levels as u64);
        }
        m
    }
}

/// An RGBA8 image.
#[derive(Clone)]
pub struct Image {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

// ---------------------------------------------------------------- decoding --

fn u32le(b: &[u8], o: usize) -> u32 { u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) }

/// texture2ddecoder writes 0xAARRGGBB words.
fn argb_words_to_rgba(words: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(words.len() * 4);
    for &w in words {
        out.extend_from_slice(&[(w >> 16) as u8, (w >> 8) as u8, w as u8, (w >> 24) as u8]);
    }
    out
}

/// Decode a DDS (DXT1/3/5, ATI1/2, BC7 via DX10, uncompressed 32-bit) to
/// RGBA8, top mip only.
pub fn decode_dds(b: &[u8]) -> Result<Image> {
    if b.len() < 128 || &b[0..4] != b"DDS " {
        bail!("not a DDS");
    }
    let h = u32le(b, 12);
    let w = u32le(b, 16);
    let pf_flags = u32le(b, 80);
    let fourcc = &b[84..88];
    let bits = u32le(b, 88);
    let mut off = 128;
    if pf_flags & 0x20 != 0 {
        // palette-indexed: AC1's own renderer cannot load these either
        bail!("palette-indexed DDS (unreadable in AC1 too)");
    }
    let mut kind = if pf_flags & 4 != 0 { String::from_utf8_lossy(fourcc).into_owned() } else { format!("RGB{bits}") };
    if pf_flags & 4 == 0 && pf_flags & 0x2 != 0 && pf_flags & 0x40 == 0 && pf_flags & 0x20000 == 0 {
        // alpha-only
        kind = format!("RGB{bits}");
    }
    if kind == "DX10" {
        let dxgi = u32le(b, 128);
        off = 148;
        kind = match dxgi {
            71 | 72 => "DXT1".into(),
            74 | 75 => "DXT3".into(),
            77 | 78 => "DXT5".into(),
            80 | 81 => "ATI1".into(),
            83 | 84 => "ATI2".into(),
            98 | 99 => "BC7".into(),
            87 | 91 => "BGRA".into(),
            28 | 29 => "RGBA".into(),
            x => bail!("unsupported DXGI format {x}"),
        };
    }
    let data = &b[off..];
    let mut words = vec![0u32; (w * h) as usize];
    let (wu, hu) = (w as usize, h as usize);
    let r = match kind.as_str() {
        "DXT1" => texture2ddecoder::decode_bc1(data, wu, hu, &mut words),
        "DXT2" | "DXT3" => texture2ddecoder::decode_bc2(data, wu, hu, &mut words),
        "DXT4" | "DXT5" => texture2ddecoder::decode_bc3(data, wu, hu, &mut words),
        "ATI1" | "BC4U" => texture2ddecoder::decode_bc4(data, wu, hu, &mut words),
        "ATI2" | "BC5U" => texture2ddecoder::decode_bc5(data, wu, hu, &mut words),
        "BC7" => texture2ddecoder::decode_bc7(data, wu, hu, &mut words),
        "BGRA" | "RGBA" => {
            let n = wu * hu * 4;
            if data.len() < n {
                bail!("uncompressed DDS truncated");
            }
            let mut px = data[..n].to_vec();
            if kind == "BGRA" {
                for p in px.chunks_exact_mut(4) {
                    p.swap(0, 2);
                }
            }
            return Ok(Image { w, h, px });
        }
        k if k.starts_with("RGB") => {
            // legacy uncompressed (incl. 8/16-bit luminance): decode through
            // the header's bit masks
            let bpp = (bits / 8).max(1) as usize;
            let mut masks = [u32le(b, 92), u32le(b, 96), u32le(b, 100), if pf_flags & 1 != 0 { u32le(b, 104) } else { 0 }];
            if pf_flags & 0x20000 != 0 {
                // luminance: one mask drives R, G and B
                let l = if masks[0] != 0 { masks[0] } else { (1u32 << (8 * bpp.min(3))) - 1 };
                masks = [l, l, l, masks[3]];
            }
            let n = wu * hu * bpp;
            if data.len() < n {
                bail!("{kind} DDS truncated");
            }
            let chan = |v: u32, m: u32| -> u8 {
                if m == 0 {
                    return 255;
                }
                let shift = m.trailing_zeros();
                let max = m >> shift;
                (((v & m) >> shift) * 255 / max) as u8
            };
            let mut px = Vec::with_capacity(wu * hu * 4);
            for p in data[..n].chunks_exact(bpp) {
                let mut v = 0u32;
                for (i, &byte) in p.iter().enumerate() {
                    v |= (byte as u32) << (8 * i);
                }
                px.extend_from_slice(&[chan(v, masks[0]), chan(v, masks[1]), chan(v, masks[2]), chan(v, masks[3])]);
            }
            return Ok(Image { w, h, px });
        }
        k => bail!("unsupported DDS kind {k}"),
    };
    r.map_err(|e| anyhow!("decode {kind}: {e}"))?;
    Ok(Image { w, h, px: argb_words_to_rgba(&words) })
}

pub fn decode_any(b: &[u8]) -> Result<Image> {
    if b.starts_with(b"DDS ") {
        return decode_dds(b);
    }
    let img = image::load_from_memory(b).map_err(|e| anyhow!("decode image: {e}"))?.to_rgba8();
    Ok(Image { w: img.width(), h: img.height(), px: img.into_raw() })
}

// ------------------------------------------------------------------- mips --

fn srgb_to_lin(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn lin_to_srgb(l: f32) -> u8 {
    let c = if l <= 0.0031308 { l * 12.92 } else { 1.055 * l.powf(1.0 / 2.4) - 0.055 };
    (c * 255.0 + 0.5).clamp(0.0, 255.0) as u8
}

fn lin_lut() -> &'static [u8; 4096] {
    static L: std::sync::OnceLock<[u8; 4096]> = std::sync::OnceLock::new();
    L.get_or_init(|| {
        let mut t = [0u8; 4096];
        for (i, v) in t.iter_mut().enumerate() {
            *v = lin_to_srgb(i as f32 / 4095.0);
        }
        t
    })
}

fn srgb_lut() -> &'static [f32; 256] {
    static L: std::sync::OnceLock<[f32; 256]> = std::sync::OnceLock::new();
    L.get_or_init(|| {
        let mut t = [0f32; 256];
        for (i, v) in t.iter_mut().enumerate() {
            *v = srgb_to_lin(i as u8);
        }
        t
    })
}

/// Next mip by 2x2 box filter (sRGB-correct for colour data), rows in
/// parallel, sRGB <-> linear through lookup tables.
pub fn half(img: &Image, srgb: bool) -> Image {
    use rayon::prelude::*;
    let (w, h) = ((img.w / 2).max(1), (img.h / 2).max(1));
    let mut px = vec![0u8; (w * h * 4) as usize];
    let at = |x: u32, y: u32| ((y.min(img.h - 1) * img.w + x.min(img.w - 1)) * 4) as usize;
    let to_lin = srgb_lut();
    let to_srgb = lin_lut();
    px.par_chunks_mut((w * 4) as usize).enumerate().for_each(|(y, row)| {
        let y = y as u32;
        for x in 0..w {
            let s = [at(2 * x, 2 * y), at(2 * x + 1, 2 * y), at(2 * x, 2 * y + 1), at(2 * x + 1, 2 * y + 1)];
            let o = (x * 4) as usize;
            for c in 0..4 {
                if srgb && c < 3 {
                    let l: f32 = s.iter().map(|&i| to_lin[img.px[i + c] as usize]).sum::<f32>() * 0.25;
                    row[o + c] = to_srgb[((l * 4095.0 + 0.5) as usize).min(4095)];
                } else {
                    let v: u32 = s.iter().map(|&i| img.px[i + c] as u32).sum();
                    row[o + c] = ((v + 2) / 4) as u8;
                }
            }
        }
    });
    Image { w, h, px }
}

pub fn resize_to(img: &Image, maxdim: u32, srgb: bool) -> Image {
    let mut cur = img.clone();
    while cur.w > maxdim || cur.h > maxdim {
        cur = half(&cur, srgb);
    }
    cur
}

// --------------------------------------------------------------- encoding --

fn pad4(img: &Image) -> Image {
    let (w, h) = (img.w.div_ceil(4) * 4, img.h.div_ceil(4) * 4);
    if (w, h) == (img.w, img.h) {
        return img.clone();
    }
    let mut px = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let s = ((y.min(img.h - 1) * img.w + x.min(img.w - 1)) * 4) as usize;
            let d = ((y * w + x) * 4) as usize;
            px[d..d + 4].copy_from_slice(&img.px[s..s + 4]);
        }
    }
    Image { w, h, px }
}

fn f16(v: f32) -> u16 {
    // finite, non-negative inputs in [0, 1]: no NaN/inf/denormal handling needed
    let b = v.max(0.0).to_bits();
    let exp = ((b >> 23) & 0xff) as i32 - 127 + 15;
    if exp <= 0 {
        return 0;
    }
    if exp >= 31 {
        return 0x7bff;
    }
    ((exp as u16) << 10) | (((b >> 13) & 0x3ff) as u16)
}

/// One mip -> encoded bytes, row-major (blocks or pixels).
/// ⚠ Large mips are cut into strips of block rows and encoded in parallel:
/// BC blocks are stored row-major, so the strips simply concatenate.
fn encode_mip(img: &Image, fmt: Fmt) -> Vec<u8> {
    use rayon::prelude::*;
    if fmt == Fmt::Ui {
        let mut px = img.px.clone();
        for p in px.chunks_exact_mut(4) {
            p.swap(0, 2);
        }
        return px;
    }
    if !fmt.block() {
        return img.px.clone();
    }
    let p = pad4(img);
    const STRIP: u32 = 64;
    if p.h <= STRIP || p.w * p.h < 256 * 256 {
        return encode_rows(&p, fmt);
    }
    let strips: Vec<(u32, u32)> = (0..p.h).step_by(STRIP as usize).map(|y| (y, STRIP.min(p.h - y))).collect();
    let parts: Vec<Vec<u8>> = strips.par_iter().map(|&(y, h)| {
        let s = (y * p.w * 4) as usize;
        let e = ((y + h) * p.w * 4) as usize;
        encode_rows(&Image { w: p.w, h, px: p.px[s..e].to_vec() }, fmt)
    }).collect();
    parts.concat()
}

fn very_fast() -> bool {
    std::env::var("ACECONV_BC7").as_deref() != Ok("fast")
}

fn encode_rows(p: &Image, fmt: Fmt) -> Vec<u8> {
    use intel_tex_2::{bc1, bc4, bc6h, bc7, RSurface, RgbaSurface};
    match fmt {
        Fmt::Bc4 => {
            let r: Vec<u8> = p.px.chunks_exact(4).map(|c| c[0]).collect();
            let s = RSurface { data: &r, width: p.w, height: p.h, stride: p.w };
            bc4::compress_blocks(&s)
        }
        Fmt::Bc7 | Fmt::Bc7Linear => {
            let s = RgbaSurface { data: &p.px, width: p.w, height: p.h, stride: p.w * 4 };
            let opaque = p.px.chunks_exact(4).all(|c| c[3] == 255);
            // "fast": 65 dB on a 4096 livery vs 67.7 for "basic", 2.4x quicker
            // "very_fast": 63.6 dB on a 4096 livery vs 64.1 for "fast", 1.6x
            // quicker (ACECONV_BC7=fast for the old profile)
            let vf = very_fast();
            let set = match (opaque, vf) {
                (true, false) => bc7::opaque_fast_settings(),
                (false, false) => bc7::alpha_fast_settings(),
                (true, true) => bc7::opaque_very_fast_settings(),
                (false, true) => bc7::alpha_very_fast_settings(),
            };
            bc7::compress_blocks(&set, &s)
        }
        Fmt::Bc6h => {
            let mut half = Vec::with_capacity(p.px.len() * 2);
            for c in p.px.chunks_exact(4) {
                for &v in &c[..3] {
                    half.extend_from_slice(&f16(v as f32 / 255.0).to_le_bytes());
                }
                half.extend_from_slice(&f16(1.0).to_le_bytes());
            }
            let s = RgbaSurface { data: &half, width: p.w, height: p.h, stride: p.w * 8 };
            bc6h::compress_blocks(&bc6h::basic_settings(), &s)
        }
        _ => {
            let s = RgbaSurface { data: &p.px, width: p.w, height: p.h, stride: p.w * 4 };
            bc1::compress_blocks(&s)
        }
    }
}

pub struct Encoded {
    pub header: Vec<u8>,
    pub mips: Vec<u8>,
}

// ---------------------------------------------------------- encode cache --

/// bump when the encoder's output changes (cached textures are then remade)
const CACHE_VERSION: u64 = 2;
/// past this the oldest cached textures go
const CACHE_CAP: u64 = 40 << 30;
static CACHE_DIR: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();

/// Turn the encode cache on (`dir`) for this process; ACECONV_TEXCACHE
/// overrides it (`0` = off). Encoding is deterministic, so a cached texture
/// is byte-identical to a fresh one: converting the same car / track again
/// skips the BC7 work.
pub fn set_cache_dir(dir: Option<std::path::PathBuf>) {
    let d = match std::env::var("ACECONV_TEXCACHE") {
        Ok(v) if v == "0" => None,
        Ok(v) if !v.is_empty() => Some(std::path::PathBuf::from(v)),
        _ => dir,
    };
    if let Some(d) = &d {
        let _ = std::fs::create_dir_all(d);
        prune_cache(d);
    }
    let _ = CACHE_DIR.set(d);
}

/// oldest first past CACHE_CAP (hits touch their file)
fn prune_cache(d: &std::path::Path) {
    let Ok(rd) = std::fs::read_dir(d) else { return };
    let mut v: Vec<(std::time::SystemTime, u64, std::path::PathBuf)> = rd.flatten().filter_map(|e| {
        let m = e.metadata().ok()?;
        Some((m.modified().ok()?, m.len(), e.path()))
    }).collect();
    let mut total: u64 = v.iter().map(|x| x.1).sum();
    if total <= CACHE_CAP {
        return;
    }
    v.sort();
    for (_, n, p) in v {
        if total <= CACHE_CAP * 3 / 4 {
            break;
        }
        if std::fs::remove_file(&p).is_ok() {
            total -= n;
        }
    }
}

/// 64-bit hash of the inputs that decide the output (wyhash-style mixing)
fn cache_key(img: &Image, fmt: Fmt, srgb: bool) -> u64 {
    #[inline]
    fn mix(a: u64, b: u64) -> u64 {
        let r = (a as u128).wrapping_mul(b as u128);
        (r as u64) ^ ((r >> 64) as u64)
    }
    const P0: u64 = 0xa076_1d64_78bd_642f;
    const P1: u64 = 0xe703_7ed1_a0b4_28db;
    let very_fast = very_fast();
    let mut h = mix(CACHE_VERSION ^ P0, ((img.w as u64) << 32 | img.h as u64) ^ P1);
    h = mix(h ^ fmt.code(), (srgb as u64) << 1 | very_fast as u64 ^ P0);
    // pixels: 4 lanes of 64-bit words, folded at the end
    use rayon::prelude::*;
    let lanes: Vec<u64> = img.px.par_chunks(1 << 20).enumerate().map(|(i, part)| {
        let mut a = P0 ^ i as u64;
        let mut it = part.chunks_exact(16);
        for c in &mut it {
            let x = u64::from_le_bytes(c[..8].try_into().unwrap());
            let y = u64::from_le_bytes(c[8..].try_into().unwrap());
            a = mix(x ^ P0, y ^ a);
        }
        for &b in it.remainder() {
            a = mix(a ^ b as u64, P1);
        }
        a
    }).collect();
    for l in lanes {
        h = mix(h ^ l, P1);
    }
    h
}

/// The paged mips of a texture: everything but the header, which carries
/// the source name (so identical pixels under two names share this).
struct Paged {
    levels: usize,
    first: Vec<u32>,
    count: Vec<u32>,
    out: Vec<u8>,
}

fn cache_read(p: &std::path::Path) -> Option<Paged> {
    let b = std::fs::read(p).ok()?;
    let u = |i: usize| -> Option<u32> { Some(u32::from_le_bytes(b.get(i * 4..i * 4 + 4)?.try_into().ok()?)) };
    let (levels, n) = (u(0)? as usize, u(1)? as usize);
    let first = (0..n).map(|i| u(2 + i)).collect::<Option<Vec<_>>>()?;
    let count = (0..n).map(|i| u(2 + n + i)).collect::<Option<Vec<_>>>()?;
    let out = b.get((2 + 2 * n) * 4..)?.to_vec();
    // a hit is recent use: keep it past the next prune
    let _ = std::fs::File::options().write(true).open(p).and_then(|f| f.set_modified(std::time::SystemTime::now()));
    Some(Paged { levels, first, count, out })
}

fn cache_write(p: &std::path::Path, e: &Paged) {
    let tmp = p.with_extension(format!("tmp{}", std::process::id()));
    let mut b = Vec::with_capacity(8 + 8 * e.first.len() + e.out.len());
    for v in [e.levels as u32, e.first.len() as u32].iter().chain(&e.first).chain(&e.count) {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(&e.out);
    if std::fs::write(&tmp, &b).is_ok() && std::fs::rename(&tmp, p).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// The paged mips of one image, through the disk cache when one is set.
fn paged_cached(img: &Image, fmt: Fmt, srgb: bool, key: u64) -> Paged {
    let file = CACHE_DIR.get().and_then(|d| d.as_ref()).map(|d| d.join(format!("{key:016x}.tex")));
    if let Some(e) = file.as_ref().and_then(|p| cache_read(p)) {
        return e;
    }
    let e = paged(img, fmt, srgb);
    if let Some(p) = &file {
        cache_write(p, &e);
    }
    e
}

/// Full chain: mips down to 1x1, encode, tile into pages, header.
pub fn encode(img: &Image, fmt: Fmt, srgb: bool, source_name: &str) -> Encoded {
    let p = paged_cached(img, fmt, srgb, cache_key(img, fmt, srgb));
    Encoded { header: header(img, fmt, source_name, &p), mips: p.out }
}

/// `encode` over many images, identical pixels encoded once (a livery set
/// shares suits, gloves, helmets). ⚠ The duplicates are grouped BEFORE the
/// parallel work: an earlier version made copies wait on a lock inside the
/// rayon pool and a worker could wait on itself (a car hung for 2 h).
pub fn encode_batch(items: &[(&Image, Fmt, bool, &str)]) -> Vec<Encoded> {
    use rayon::prelude::*;
    let keys: Vec<u64> = items.par_iter().map(|(img, fmt, srgb, _)| cache_key(img, *fmt, *srgb)).collect();
    let mut first: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    for (i, k) in keys.iter().enumerate() {
        first.entry(*k).or_insert(i);
    }
    let mut uniq: Vec<(u64, usize)> = first.into_iter().collect();
    uniq.sort_by_key(|x| x.1);
    let mut done: std::collections::HashMap<u64, Paged> = uniq.par_iter().map(|&(k, i)| {
        let (img, fmt, srgb, _) = items[i];
        (k, paged_cached(img, fmt, srgb, k))
    }).collect();
    // the last user of an encoding takes it (no second copy of every mip chain)
    let mut left: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    for k in &keys {
        *left.entry(*k).or_default() += 1;
    }
    items.iter().zip(&keys).map(|((img, fmt, _, name), k)| {
        let n = left.get_mut(k).unwrap();
        *n -= 1;
        let h = header(img, *fmt, name, &done[k]);
        let mips = if *n == 0 { std::mem::take(&mut done.get_mut(k).unwrap().out) } else { done[k].out.clone() };
        Encoded { header: h, mips }
    }).collect()
}

fn paged(img: &Image, fmt: Fmt, srgb: bool) -> Paged {
    let mut levels = vec![img.clone()];
    // raw masks (function / LED masks, fB) carry a single level
    while (fmt.block() || fmt == Fmt::Ui) && (levels.last().unwrap().w > 1 || levels.last().unwrap().h > 1) {
        let n = half(levels.last().unwrap(), srgb);
        levels.push(n);
    }
    let (tw, th) = fmt.tile();
    let trb = fmt.row_bytes(tw);
    let block_rows = |h: u32| if fmt.block() { h.div_ceil(4).max(1) } else { h };
    let mut out: Vec<u8> = Vec::new();
    let (mut first, mut count) = (Vec::new(), Vec::new());
    for lvl in &levels {
        let enc = encode_mip(lvl, fmt);
        let rb = fmt.row_bytes(lvl.w);
        let cols = lvl.w.div_ceil(tw).max(1);
        let rows = lvl.h.div_ceil(th).max(1);
        first.push((out.len() / PAGE) as u32);
        count.push(cols * rows);
        let rows_per_tile = block_rows(th) as usize;
        let total_rows = block_rows(lvl.h) as usize;
        for r in 0..rows as usize {
            for c in 0..cols as usize {
                let mut page = vec![0u8; PAGE];
                let keep_w = fmt.row_bytes(tw.min(lvl.w - c as u32 * tw));
                for y in 0..rows_per_tile {
                    let sy = r * rows_per_tile + y;
                    if sy >= total_rows {
                        break;
                    }
                    let src = sy * rb + c * trb;
                    let dst = y * trb;
                    if src + keep_w <= enc.len() && dst + keep_w <= PAGE {
                        page[dst..dst + keep_w].copy_from_slice(&enc[src..src + keep_w]);
                    }
                }
                out.extend_from_slice(&page);
            }
        }
    }
    Paged { levels: levels.len(), first, count, out }
}

/// Stock `.texture` header constants per format enum, the most common
/// value of each field over the stock headers with a page table
/// (examples/texhdr.rs, tools/texhdr_stock.tsv): (enum, source f6 pixel
/// layout, page-table f7 units per page). Every other constant field has
/// one most common value on all these formats: root f6 = 1 and f13 = 1;
/// source f4 = 1 and f5 = 1; page table f3 = 1; root f7 / f20 and page
/// table f6 are absent on most headers and left out.
const STOCK_HEADER: [(u64, u64, u64); 8] = [
    (5, 5, 512),   // 108 headers: f10.f6 5 on all, f12.f7 512 on all
    (6, 5, 512),   // 232: 5 on all, 512 on all
    (12, 2, 1024), // 1072: 2 on 534, 1024 on all
    (13, 5, 1024), // 2717: 5 on 1292, 1024 on 2715
    (26, 5, 1024), // 1971: 5 on 933, 1024 on 1966
    (27, 2, 1024), // 5346: 2 on 1810, 1024 on 5345
    (33, 2, 1024), // 461: 2 on 268, 1024 on 459
    (34, 5, 1024), // 5186: 5 on 3033, 1024 on 5185
];

/// The `.texture` header: f1/f2 size, f3 mips, f4 format, f6, f10 source
/// {1 path, 2 / 3 size, 4, 5, 6 pixel layout}, f11 import settings
/// (`import_settings`), f12 page table {1 / 2 tile w / h, 3, 4 first page
/// per mip, 5 pages per mip, 7 one value per page}, f13; the constants are
/// the stock per-format most common values (`STOCK_HEADER`). The source
/// path is our own: the name the caller gives (the AC1 texture's relative
/// name for car textures, e.g. `texture/EXT_Skin.dds`); stock holds the
/// authoring machine's absolute path there.
fn header(img: &Image, fmt: Fmt, source_name: &str, p: &Paged) -> Vec<u8> {
    let (tw, th) = fmt.tile();
    let (first, count) = (&p.first, &p.count);
    let pages = p.out.len() / PAGE;
    let code = fmt.code();
    let (_, layout, per_page) = *STOCK_HEADER.iter().find(|r| r.0 == code).expect("every format has stock header constants");
    let mut h = Msg::default();
    h.set_varint(1, img.w as u64);
    h.set_varint(2, img.h as u64);
    h.set_varint(3, p.levels as u64);
    h.set_varint(4, code);
    h.set_varint(6, 1);
    let mut src = Msg::default();
    src.set_str(1, source_name);
    src.set_varint(2, img.w as u64);
    src.set_varint(3, img.h as u64);
    src.set_varint(4, 1);
    src.set_varint(5, 1);
    src.set_varint(6, layout);
    h.set(10, Val::Msg(src));
    h.set(11, Val::Msg(fmt.import_settings(p.levels)));
    let mut pt = Msg::default();
    pt.set_varint(1, tw as u64);
    pt.set_varint(2, th as u64);
    pt.set_varint(3, 1);
    pt.set(4, Val::Bytes(pb::varint_bytes(first)));
    pt.set(5, Val::Bytes(pb::varint_bytes(count)));
    pt.set(7, Val::Bytes(pb::varint_bytes(&vec![per_page as u32; pages])));
    h.set(12, Val::Msg(pt));
    h.set_varint(13, 1);
    h.encode()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_and_pages_agree() {
        let img = Image { w: 300, h: 200, px: vec![128; 300 * 200 * 4] };
        for fmt in [Fmt::Bc7, Fmt::Bc1, Fmt::Bc4, Fmt::Rgba8b] {
            let e = encode(&img, fmt, true, "t.png");
            assert_eq!(e.mips.len() % PAGE, 0);
            let h = Msg::parse(&e.header).unwrap();
            assert_eq!(h.varint(4), Some(fmt.code()));
            let pt = h.sub(12).unwrap();
            let per = pb::varints(pt.bytes(7).unwrap());
            assert_eq!(per.len() * PAGE, e.mips.len());
            let imp = h.sub(11).unwrap();
            assert_eq!(imp.f32(11), Some(0.5));
            if !fmt.block() {
                assert_eq!(imp.varint(13), h.varint(3));
            }
        }
    }

    #[test]
    fn header_source_is_ours_and_constants_are_stock() {
        let img = Image { w: 64, h: 64, px: vec![200; 64 * 64 * 4] };
        for (fmt, layout, per) in [(Fmt::Bc7, 5, 1024), (Fmt::Bc1, 2, 1024), (Fmt::Bc4, 2, 1024), (Fmt::Rgba8b, 5, 512), (Fmt::Ui, 5, 512)] {
            let e = encode(&img, fmt, true, "texture/EXT_Skin.dds");
            let h = Msg::parse(&e.header).unwrap();
            let src = h.sub(10).unwrap();
            assert_eq!(src.str(1).as_deref(), Some("texture/EXT_Skin.dds"));
            assert_eq!(src.varint(6), Some(layout));
            assert!(pb::varints(h.sub(12).unwrap().bytes(7).unwrap()).iter().all(|v| *v == per));
        }
    }
}
