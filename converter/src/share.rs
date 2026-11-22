//! `.acemod` share packages: a converted car `.kspkg` (or a converted
//! track folder) packed for download, and rebuilt EXACTLY on install.
//!
//! An installed package is the worst case for a compressor: every entry but
//! the texture pages is XOR'd with the engine key and the 64 MiB table is
//! mostly XOR'd zeros. The share file stores the decrypted contents instead,
//! and three things make it small:
//!
//! * **base refs** - an entry byte-identical to an entry of the game's own
//!   `content.kspkg` (the donor's stock tyres, discs, dirt, windshield bakes,
//!   fonts...) is stored as path + size + MD5 and read back from the
//!   installer's game. Anyone installing a car mod has the game; if their
//!   copy of that one file differs (a game update), the same bytes are
//!   searched for under any other path before giving up.
//! * **lossless audio** - PCM16 sample data inside an FMOD bank is stored as
//!   per-channel second-order residuals in byte planes (exactly invertible).
//! * **LZMA2** (xz preset 9, its dictionary capped at the segment size) over
//!   segments grouped by kind, about one segment per thread (default: half
//!   the cores, ACECONV_SHARE_THREADS). ACECONV_SHARE_CODEC picks another
//!   codec (xz6 / xz9 / xz9e / zstd<level>); the manifest's `codec` says
//!   which one a package uses, so every package stays readable. Measured on
//!   a converted Miata (481 MB): 9e 31.6 MB in 81 s at 2 threads; 9 with
//!   32 MB segments 32.3 MB in ~10 s at 8 threads; zstd 19 (long matching)
//!   36-38 MB; xz 6 33.3-34 MB.
//! * **Stock digests** are remembered per content.kspkg build
//!   (`share_index\stock_md5_<size>_<mtime>.bin`: each candidate's sampled
//!   windows and, once hashed, its MD5), so a stock file is read once, not
//!   once per package.
//!
//! The rebuilt package is byte-identical to the original: the decrypted
//! table and any bytes between entries travel with it, and `unpack-share`
//! checks the MD5 of the whole result.
//!
//! Layout: `ACEMOD\x01\n`, u32 LE manifest length, manifest JSON, then each
//! segment's xz stream back to back (lengths in the manifest).

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::path::Path;

use crate::slim::{md5, Base};

const MAGIC: &[u8; 8] = b"ACEMOD\x01\n";
const TABLE: usize = 64 * 1024 * 1024;
/// split big segments so two threads have work and memory stays bounded
const CHUNK: usize = 96 << 20;
const MIN_CHUNK: usize = 32 << 20;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Blob {
    /// first archive path (or folder-relative path) that names these bytes
    pub path: String,
    pub size: u64,
    pub md5: String,
    /// "raw" | "base" | "pcm"
    pub how: String,
    /// kspkg: offset in the data region, record kind (0 = plaintext)
    #[serde(default)]
    pub off: u64,
    #[serde(default)]
    pub kind: u16,
    /// stock path for "base"
    #[serde(default)]
    pub base: String,
    /// segment + offset of the payload inside the segment's raw stream
    #[serde(default)]
    pub seg: usize,
    #[serde(default)]
    pub at: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Segment {
    pub name: String,
    pub raw: u64,
    pub packed: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Manifest {
    pub version: u32,
    /// "kspkg" | "dir"
    pub kind: String,
    pub name: String,
    pub out_size: u64,
    pub out_md5: String,
    pub blobs: Vec<Blob>,
    pub segments: Vec<Segment>,
    /// kspkg: (offset, length) of bytes no entry covers, stored in the "gaps" payload
    #[serde(default)]
    pub gaps: Vec<(u64, u64)>,
    #[serde(default)]
    pub gaps_seg: usize,
    #[serde(default)]
    pub gaps_at: u64,
    #[serde(default)]
    pub table_seg: usize,
    #[serde(default)]
    pub table_at: u64,
    /// the game build the base refs were taken from (content.kspkg size)
    #[serde(default)]
    pub base_from: u64,
    /// files that travel beside a kspkg (the car's `<id>.json`): name, size,
    /// offset in the misc stream
    #[serde(default)]
    pub extras: Vec<(String, u64, u64)>,
    /// the segments' codec: "xz" (also when absent: version 1) or "zstd"
    #[serde(default)]
    pub codec: String,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Result<[u8; 16]> {
    let mut o = [0u8; 16];
    if s.len() != 32 {
        bail!("bad md5 {s}");
    }
    for i in 0..16 {
        o[i] = u8::from_str_radix(&s[2 * i..2 * i + 2], 16)?;
    }
    Ok(o)
}

// ------------------------------------------------------------ PCM planes --

/// PCM16 sample regions of every FSB5 inside an FMOD bank: (offset, frames, channels).
pub fn pcm_regions(b: &[u8]) -> Vec<(usize, usize, usize)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(rel) = memchr::memmem::find(&b[from..], b"FSB5") {
        let i = from + rel;
        from = i + 4;
        let u = |o: usize| b.get(o..o + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap()) as usize);
        let (Some(ver), Some(n), Some(shs), Some(nts), Some(ds), Some(mode)) = (u(i + 4), u(i + 8), u(i + 12), u(i + 16), u(i + 20), u(i + 24)) else { break };
        if mode != 2 || ver > 1 {
            continue; // PCM16 only
        }
        let hdr = if ver == 1 { 0x3c } else { 0x40 };
        let data0 = i + hdr + shs + nts;
        if data0 + ds > b.len() {
            continue;
        }
        let mut p = i + hdr;
        let mut ok = true;
        let mut rs = Vec::new();
        for _ in 0..n {
            let Some(v) = b.get(p..p + 8).map(|s| u64::from_le_bytes(s.try_into().unwrap())) else { ok = false; break };
            p += 8;
            let mut next = v & 1 == 1;
            let mut ch = match (v >> 5) & 3 { 0 => 1, 1 => 2, 2 => 6, _ => 8 };
            let off = (((v >> 7) & 0x7ff_ffff) * 32) as usize;
            let frames = (v >> 34) as usize;
            while next {
                let Some(c) = u(p) else { ok = false; break };
                p += 4;
                next = c & 1 == 1;
                let len = (c >> 1) & 0xff_ffff;
                if c >> 25 == 1 {
                    ch = b.get(p).copied().unwrap_or(1) as usize;
                }
                p += len;
            }
            if !ok || p > i + hdr + shs {
                ok = false;
                break;
            }
            rs.push((data0 + off, frames, ch.max(1)));
        }
        if ok {
            out.extend(rs.into_iter().filter(|r| r.0 + r.1 * r.2 * 2 <= data0 + ds));
        }
    }
    out
}

/// interleaved PCM16 -> per channel 2nd-order residuals, low bytes then high bytes
fn planes_fwd(src: &[u8], ch: usize) -> Vec<u8> {
    let frames = src.len() / (2 * ch);
    let n = frames * ch;
    let mut out = vec![0u8; n * 2];
    for c in 0..ch {
        let (mut p1, mut p2) = (0i16, 0i16);
        for f in 0..frames {
            let i = (f * ch + c) * 2;
            let x = i16::from_le_bytes([src[i], src[i + 1]]);
            let pred = p1.wrapping_mul(2).wrapping_sub(p2);
            let r = x.wrapping_sub(pred) as u16;
            let k = c * frames + f;
            out[k] = r as u8;
            out[n + k] = (r >> 8) as u8;
            p2 = p1;
            p1 = x;
        }
    }
    out
}

fn planes_inv(src: &[u8], ch: usize) -> Vec<u8> {
    let n = src.len() / 2;
    let frames = n / ch;
    let mut out = vec![0u8; n * 2];
    for c in 0..ch {
        let (mut p1, mut p2) = (0i16, 0i16);
        for f in 0..frames {
            let k = c * frames + f;
            let r = (src[k] as u16 | (src[n + k] as u16) << 8) as i16;
            let x = r.wrapping_add(p1.wrapping_mul(2).wrapping_sub(p2));
            let i = (f * ch + c) * 2;
            out[i..i + 2].copy_from_slice(&x.to_le_bytes());
            p2 = p1;
            p1 = x;
        }
    }
    out
}

/// Bank with its PCM regions in plane form (same length). The FSB headers
/// stay untouched, so the inverse finds the same regions.
fn bank_fwd(b: &[u8]) -> Option<Vec<u8>> {
    let rs = pcm_regions(b);
    if rs.is_empty() {
        return None;
    }
    let mut out = b.to_vec();
    for (o, fr, ch) in rs {
        let len = fr * ch * 2;
        out[o..o + len].copy_from_slice(&planes_fwd(&b[o..o + len], ch));
    }
    Some(out)
}

fn bank_inv(b: &[u8]) -> Vec<u8> {
    let mut out = b.to_vec();
    for (o, fr, ch) in pcm_regions(b) {
        let len = fr * ch * 2;
        out[o..o + len].copy_from_slice(&planes_inv(&b[o..o + len], ch));
    }
    out
}

// ---------------------------------------------------------------- xz --

fn xz(data: &[u8]) -> Result<Vec<u8>> {
    xz_at(data, 9 | liblzma::stream::PRESET_EXTREME)
}

fn xz_at(data: &[u8], preset: u32) -> Result<Vec<u8>> {
    // the preset's dictionary, but no bigger than the input: the encoder
    // needs ~10x its dictionary in memory (674 MB at preset 9), so a 16 MB
    // segment would otherwise cost as much as a 96 MB one - per thread. The
    // output is the same: a dictionary larger than the input never helps.
    use liblzma::stream::{Check, Filters, LzmaOptions, Stream};
    let mut o = LzmaOptions::new_preset(preset)?;
    let want = (data.len().max(1 << 20) as u64).next_power_of_two().min(u32::MAX as u64) as u32;
    // liblzma's preset dictionaries (xz(1) "-0" .. "-9")
    let preset_dict: u32 = [256 << 10, 1 << 20, 2 << 20, 4 << 20, 4 << 20, 8 << 20, 8 << 20, 16 << 20, 32 << 20, 64 << 20][(preset & 0x1f).min(9) as usize];
    o.dict_size(want.min(preset_dict));
    let mut f = Filters::new();
    f.lzma2(&o);
    let s = Stream::new_stream_encoder(&f, Check::Crc64)?;
    let mut e = liblzma::read::XzEncoder::new_stream(data, s);
    let mut out = Vec::new();
    e.read_to_end(&mut out)?;
    Ok(out)
}

/// The segment codec: the manifest's `codec` ("xz" when absent, as every
/// version-1 package) says how to read them back.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Codec {
    /// xz / LZMA2 at a preset (6, 9, or 9 | PRESET_EXTREME)
    Xz(u32),
    /// zstd at a level, long-distance matching over a 128 MiB window
    Zstd(i32),
}

impl Codec {
    /// "xz6", "xz9", "xz9e", "zstd15", "zstd19", ...
    pub fn parse(s: &str) -> Option<Codec> {
        let s = s.trim().to_ascii_lowercase();
        if let Some(l) = s.strip_prefix("zstd") {
            return l.parse().ok().filter(|l| (1..=22).contains(l)).map(Codec::Zstd);
        }
        let l = s.strip_prefix("xz")?;
        let (n, e) = match l.strip_suffix('e') { Some(n) => (n, true), None => (l, false) };
        let n: u32 = n.parse().ok().filter(|n| *n <= 9)?;
        Some(Codec::Xz(if e { n | liblzma::stream::PRESET_EXTREME } else { n }))
    }

    /// what the manifest records
    pub fn family(&self) -> &'static str {
        match self { Codec::Xz(_) => "xz", Codec::Zstd(_) => "zstd" }
    }

    pub fn compress(&self, data: &[u8]) -> Result<Vec<u8>> {
        match *self {
            Codec::Xz(p) => xz_at(data, p),
            Codec::Zstd(level) => {
                use std::io::Write as _;
                let mut e = zstd::stream::Encoder::new(Vec::with_capacity(data.len() / 4), level)?;
                // a known size lets zstd shrink its window / tables to the input
                e.set_pledged_src_size(Some(data.len() as u64))?;
                e.long_distance_matching(true)?;
                e.window_log(ZSTD_WINDOW_LOG)?;
                e.write_all(data)?;
                Ok(e.finish()?)
            }
        }
    }
}

const ZSTD_WINDOW_LOG: u32 = 27;

/// The packing codec: ACECONV_SHARE_CODEC, else the default.
pub fn codec() -> Codec {
    std::env::var("ACECONV_SHARE_CODEC").ok().and_then(|s| Codec::parse(&s)).unwrap_or(DEFAULT_CODEC)
}

pub const DEFAULT_CODEC: Codec = Codec::Xz(9);

fn decompress(family: &str, data: &[u8], raw: usize) -> Result<Vec<u8>> {
    match family {
        "" | "xz" => unxz(data, raw),
        "zstd" => {
            let mut d = zstd::stream::Decoder::new(data)?;
            d.window_log_max(31)?;
            let mut out = Vec::with_capacity(raw);
            d.read_to_end(&mut out)?;
            Ok(out)
        }
        c => bail!("this package uses codec {c}, which this converter cannot read - update it"),
    }
}

fn unxz(data: &[u8], raw: usize) -> Result<Vec<u8>> {
    let mut d = liblzma::read::XzDecoder::new(data);
    let mut out = Vec::with_capacity(raw);
    d.read_to_end(&mut out)?;
    Ok(out)
}

/// A pool of `n` threads for the heavy work (the user is on the machine).
fn pool(n: usize) -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new().num_threads(n.max(1)).build().unwrap()
}

fn threads() -> usize {
    // default: half the cores (the converter runs at idle priority)
    std::env::var("ACECONV_SHARE_THREADS").ok().and_then(|v| v.parse().ok())
        .unwrap_or_else(|| (std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2) / 2).max(1))
}

fn group_of(path: &str) -> &'static str {
    let l = path.to_ascii_lowercase();
    if l.ends_with(".texturemips") {
        "tex"
    } else if l.ends_with(".mesh") || l.ends_with(".tyremesh") || l.ends_with(".collider") {
        "mesh"
    } else if l.ends_with(".bank") {
        "audio"
    } else {
        "misc"
    }
}

#[derive(Default, Debug)]
pub struct PackReport {
    pub out_bytes: u64,
    pub base: (usize, u64),
    pub pcm: (usize, u64),
    pub raw: (usize, u64),
    pub segments: Vec<Segment>,
    pub secs: f64,
}

/// Inputs as (path, kind, offset, bytes); kspkg extras.
struct Input {
    kind: &'static str,
    name: String,
    blobs: Vec<(String, u16, u64, Vec<u8>)>,
    table: Vec<u8>,
    gaps: Vec<(u64, Vec<u8>)>,
    whole_md5: [u8; 16],
    whole_size: u64,
    /// sidecar files next to a kspkg: (file name, bytes)
    extras: Vec<(String, Vec<u8>)>,
}

fn read_kspkg(p: &Path) -> Result<Input> {
    let f = std::fs::File::open(p).with_context(|| format!("open {}", p.display()))?;
    let map = unsafe { memmap2::Mmap::map(&f)? };
    if map.len() < TABLE {
        bail!("{} is smaller than its table", p.display());
    }
    let data_len = map.len() - TABLE;
    let mut table = map[data_len..].to_vec();
    crate::io::kspkg::xor(&mut table);
    // unique (offset, size) blobs, first path wins
    let mut seen: BTreeMap<(u64, u64), (String, u16)> = BTreeMap::new();
    for r in table.chunks_exact(256) {
        let kind = u16::from_le_bytes([r[228], r[229]]);
        let end = r[..228].iter().position(|&b| b == 0).unwrap_or(228);
        if end == 0 || kind == 1 {
            continue;
        }
        let size = u32::from_le_bytes(r[240..244].try_into().unwrap()) as u64;
        let off = u64::from_le_bytes(r[248..256].try_into().unwrap());
        if off + size > data_len as u64 {
            bail!("{}: an entry runs past the data region", p.display());
        }
        let path = String::from_utf8_lossy(&r[..end]).into_owned();
        seen.entry((off, size)).or_insert((path, kind));
    }
    let mut blobs = Vec::new();
    let mut gaps = Vec::new();
    let mut cur = 0u64;
    for ((off, size), (path, kind)) in &seen {
        if *off > cur {
            gaps.push((cur, map[cur as usize..*off as usize].to_vec()));
        }
        let mut d = map[*off as usize..(*off + *size) as usize].to_vec();
        if *kind != 0 {
            crate::io::kspkg::xor(&mut d);
        }
        blobs.push((path.clone(), *kind, *off, d));
        cur = cur.max(off + size);
    }
    if (cur as usize) < data_len {
        gaps.push((cur, map[cur as usize..data_len].to_vec()));
    }
    let whole_md5 = md5(&map);
    Ok(Input {
        kind: "kspkg",
        name: p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
        blobs, table, gaps, whole_md5, whole_size: map.len() as u64,
        // the car's <id>.json (display name, presets) sits beside its package
        extras: p.with_extension("json").file_name().and_then(|n| {
            let b = std::fs::read(p.with_extension("json")).ok()?;
            Some(vec![(n.to_string_lossy().into_owned(), b)])
        }).unwrap_or_default(),
    })
}

/// A folder: digest of the sorted (path, bytes) list.
fn dir_digest(files: &BTreeMap<String, Vec<u8>>) -> [u8; 16] {
    use md5::{Digest, Md5};
    let mut h = Md5::new();
    for (k, v) in files {
        h.update(k.as_bytes());
        h.update([0]);
        h.update((v.len() as u64).to_le_bytes());
        h.update(v);
    }
    h.finalize().into()
}

fn read_dir_input(p: &Path) -> Result<Input> {
    let files = crate::slim::read_dir(p)?;
    let whole_md5 = dir_digest(&files);
    let whole_size = files.values().map(|v| v.len() as u64).sum();
    Ok(Input {
        kind: "dir",
        name: p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
        blobs: files.into_iter().map(|(k, v)| (k, 0, 0, v)).collect(),
        table: Vec::new(), gaps: Vec::new(), whole_md5, whole_size, extras: Vec::new(),
    })
}

/// `aceconv pack-share <kspkg | track dir> <out.acemod>`
pub fn pack(src: &Path, out: &Path, game: Option<&Path>) -> Result<PackReport> {
    let t = std::time::Instant::now();
    let mut inp = if src.is_dir() { read_dir_input(src)? } else { read_kspkg(src)? };
    let base = match game {
        Some(g) if g.is_file() => Some(Base::open(g)?),
        _ => None,
    };
    let sidx = base.as_ref().map(|b| b.size_index());
    let pl = pool(threads());
    let cod = codec();
    let timing = std::env::var("ACECONV_SHARE_TIMING").is_ok();
    if timing {
        eprintln!("timing: read + base open {:.2} s", t.elapsed().as_secs_f64());
    }

    // decide per blob (base lookups + PCM transform in parallel)
    use rayon::prelude::*;
    let decided: Vec<(String, Option<String>, Option<Vec<u8>>, [u8; 16])> = pl.install(|| {
        inp.blobs.par_iter().map(|(path, _, _, d)| {
            let m = md5(d);
            let name = path.rsplit('\\').next().unwrap_or(path);
            let stock = match (&base, &sidx) {
                (Some(b), Some(ix)) if d.len() >= 4096 => b.find_file(name, d, m, ix),
                _ => None,
            };
            let pcm = if stock.is_none() && path.to_ascii_lowercase().ends_with(".bank") {
                bank_fwd(d).filter(|t| bank_inv(t) == *d)
            } else {
                None
            };
            (path.clone(), stock, pcm, m)
        }).collect()
    });

    if let Some(b) = &base {
        b.save_memo();
    }
    if timing {
        eprintln!("timing: decide {:.2} s", t.elapsed().as_secs_f64());
    }
    // payload streams per group
    let mut streams: BTreeMap<&'static str, Vec<u8>> = BTreeMap::new();
    let mut blobs = Vec::new();
    let mut rep = PackReport::default();
    // order inside a group: by extension then path (similar data together)
    let mut order: Vec<usize> = (0..inp.blobs.len()).collect();
    order.sort_by_key(|&i| {
        let p = inp.blobs[i].0.to_ascii_lowercase();
        let ext = p.rsplit('.').next().unwrap_or("").to_string();
        (group_of(&p), ext, p)
    });
    let mut pending: Vec<(usize, Blob)> = Vec::new();
    for i in order {
        // the bytes move into their stream (peak memory = one copy of the package)
        let d = std::mem::take(&mut inp.blobs[i].3);
        let (path, kind, off, _) = &inp.blobs[i];
        let (_, stock, pcm, m) = &decided[i];
        let mut b = Blob { path: path.clone(), size: d.len() as u64, md5: hex(m), how: "raw".into(), off: *off, kind: *kind, base: String::new(), seg: 0, at: 0 };
        let g = group_of(path);
        if let Some(s) = stock {
            b.how = "base".into();
            b.base = s.clone();
            rep.base.0 += 1;
            rep.base.1 += d.len() as u64;
        } else {
            let s = streams.entry(g).or_default();
            b.at = s.len() as u64;
            if let Some(t) = pcm {
                b.how = "pcm".into();
                s.extend_from_slice(t);
                rep.pcm.0 += 1;
                rep.pcm.1 += d.len() as u64;
            } else {
                s.extend_from_slice(&d);
                rep.raw.0 += 1;
                rep.raw.1 += d.len() as u64;
            }
        }
        pending.push((i, b));
    }
    // sidecars, then (kspkg) table + gaps, ride in misc
    let mut extras = Vec::new();
    for (n, b) in &inp.extras {
        let s = streams.entry("misc").or_default();
        extras.push((n.clone(), b.len() as u64, s.len() as u64));
        s.extend_from_slice(b);
    }
    let (mut table_at, mut gaps_at, mut gaps) = (0u64, 0u64, Vec::new());
    if inp.kind == "kspkg" {
        let s = streams.entry("misc").or_default();
        table_at = s.len() as u64;
        s.extend_from_slice(&inp.table);
        gaps_at = s.len() as u64;
        for (o, g) in &inp.gaps {
            gaps.push((*o, g.len() as u64));
            s.extend_from_slice(g);
        }
    }
    // cut streams into chunks = segments: about one per thread, never under
    // MIN_CHUNK (each chunk starts with an empty dictionary: 16 MB chunks
    // cost a car ~6 % in size, 32 MB ~2 %) nor over CHUNK (memory)
    let total: usize = streams.values().map(|s| s.len()).sum();
    let chunk = std::env::var("ACECONV_SHARE_CHUNK_MB").ok().and_then(|v| v.parse::<usize>().ok()).map(|m| m.max(1) << 20)
        .unwrap_or_else(|| (total / threads().max(1)).clamp(MIN_CHUNK, CHUNK));
    let mut segs: Vec<(String, &'static str, usize, usize)> = Vec::new(); // name, group, start, end
    for (g, s) in &streams {
        let mut a = 0;
        loop {
            let e = (a + chunk).min(s.len());
            segs.push((format!("{g}{}", segs.iter().filter(|x| x.1 == *g).count()), g, a, e));
            if e >= s.len() {
                break;
            }
            a = e;
        }
    }
    let seg_of = |g: &str, at: u64| -> (usize, u64) {
        for (i, s) in segs.iter().enumerate() {
            if s.1 == g && (at as usize) >= s.2 && ((at as usize) < s.3 || s.2 == s.3) {
                return (i, at - s.2 as u64);
            }
        }
        (usize::MAX, 0)
    };
    // a payload may straddle a chunk boundary: store the segment of its first
    // byte and the offset into the CONCATENATION of that group's segments
    for (_, b) in pending.iter_mut() {
        if b.how != "base" {
            let g = group_of(&b.path);
            let (si, _) = seg_of(g, b.at);
            b.seg = si;
        }
    }
    let (ts, _) = seg_of("misc", table_at);
    let (gs, _) = seg_of("misc", gaps_at);
    let packed: Vec<Vec<u8>> = pl.install(|| {
        segs.par_iter().map(|(_, g, a, e)| cod.compress(&streams[*g][*a..*e])).collect::<Result<Vec<_>>>()
    })?;
    if timing {
        eprintln!("timing: compress {:.2} s ({} segments)", t.elapsed().as_secs_f64(), segs.len());
    }
    let segments: Vec<Segment> = segs.iter().zip(&packed).map(|(s, p)| Segment { name: s.0.clone(), raw: (s.3 - s.2) as u64, packed: p.len() as u64 }).collect();
    pending.sort_by_key(|x| x.0);
    blobs.extend(pending.into_iter().map(|x| x.1));
    let man = Manifest {
        version: if cod.family() == "xz" { 1 } else { 2 }, codec: cod.family().into(), kind: inp.kind.into(), name: inp.name.clone(), out_size: inp.whole_size, out_md5: hex(&inp.whole_md5),
        blobs, segments: segments.clone(), gaps, gaps_seg: gs, gaps_at, table_seg: ts, table_at, extras,
        base_from: game.and_then(|g| std::fs::metadata(g).ok()).map(|m| m.len()).unwrap_or(0),
    };
    let js = serde_json::to_vec(&man)?;
    let tmp = out.with_extension("acemod.part");
    {
        let mut w = std::io::BufWriter::new(std::fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?);
        w.write_all(MAGIC)?;
        let jz = xz(&js)?;
        w.write_all(&(jz.len() as u32).to_le_bytes())?;
        w.write_all(&jz)?;
        for p in &packed {
            w.write_all(p)?;
        }
        w.flush()?;
    }
    std::fs::rename(&tmp, out)?;
    rep.out_bytes = std::fs::metadata(out)?.len();
    rep.segments = segments;
    rep.secs = t.elapsed().as_secs_f64();
    Ok(rep)
}

pub fn read_manifest(p: &Path) -> Result<(Manifest, Vec<u8>, usize)> {
    let b = std::fs::read(p).with_context(|| format!("read {}", p.display()))?;
    if b.len() < 12 || &b[..8] != MAGIC {
        bail!("{} is not an .acemod", p.display());
    }
    let n = u32::from_le_bytes(b[8..12].try_into().unwrap()) as usize;
    let js = unxz(b.get(12..12 + n).ok_or_else(|| anyhow!("truncated manifest"))?, 1 << 20)?;
    let man: Manifest = serde_json::from_slice(&js)?;
    Ok((man, b, 12 + n))
}

#[derive(Default, Debug)]
pub struct UnpackReport {
    pub secs: f64,
    pub base_files: usize,
    pub base_moved: Vec<String>,
    pub identical: bool,
}

/// `aceconv unpack-share <in.acemod> <out.kspkg | out dir>`
pub fn unpack(src: &Path, out: &Path, game: Option<&Path>) -> Result<UnpackReport> {
    let t = std::time::Instant::now();
    let (man, file, mut pos) = read_manifest(src)?;
    let mut rep = UnpackReport::default();
    let pl = pool(threads());
    // segments -> raw, then each group's segments concatenated
    let mut ranges = Vec::new();
    for s in &man.segments {
        ranges.push((pos, pos + s.packed as usize, s.raw as usize));
        pos += s.packed as usize;
    }
    use rayon::prelude::*;
    let raw: Vec<Vec<u8>> = pl.install(|| ranges.par_iter().map(|&(a, e, n)| decompress(&man.codec, &file[a..e], n)).collect::<Result<Vec<_>>>())?;
    drop(file);
    // group streams: a blob's `at` is relative to its group's concatenation
    let group = |name: &str| name.trim_end_matches(|c: char| c.is_ascii_digit()).to_string();
    let mut streams: HashMap<String, Vec<u8>> = HashMap::new();
    for (s, r) in man.segments.iter().zip(raw) {
        let g = streams.entry(group(&s.name)).or_default();
        g.extend_from_slice(&r);
    }
    let base = if man.blobs.iter().any(|b| b.how == "base") {
        let g = game.filter(|g| g.is_file()).ok_or_else(|| anyhow!("this package uses files from the game's content.kspkg - the game was not found"))?;
        Some(Base::open(g)?)
    } else {
        None
    };
    let sidx = base.as_ref().map(|b| b.size_index());
    let payload = |b: &Blob| -> Result<Vec<u8>> {
        let seg = &man.segments[b.seg];
        let s = &streams[&group(&seg.name)];
        let a = b.at as usize;
        let d = s.get(a..a + b.size as usize).ok_or_else(|| anyhow!("{}: payload out of range", b.path))?.to_vec();
        Ok(if b.how == "pcm" { bank_inv(&d) } else { d })
    };
    let bytes_of = |b: &Blob| -> Result<(Vec<u8>, Option<String>)> {
        if b.how == "base" {
            let base = base.as_ref().unwrap();
            let want = unhex(&b.md5)?;
            if let Some(d) = base.read(&b.base).filter(|d| md5(d) == want) {
                return Ok((d, None));
            }
            let (p, d) = base.read_by_digest(b.size as usize, want, sidx.as_ref().unwrap())
                .ok_or_else(|| anyhow!("{} needs the game's {} - this game build has different bytes there (game updated?)", b.path, b.base))?;
            return Ok((d, Some(p)));
        }
        let d = payload(b)?;
        if hex(&md5(&d)) != b.md5 {
            bail!("{}: checksum mismatch", b.path);
        }
        Ok((d, None))
    };
    if man.kind == "kspkg" {
        let tmp = out.with_extension("kspkg.part");
        // read + write: a mapping needs both
        let f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&tmp).with_context(|| format!("create {}", tmp.display()))?;
        f.set_len(man.out_size)?;
        let mut mm = unsafe { memmap2::MmapMut::map_mut(&f)? };
        let data_len = man.out_size as usize - TABLE;
        let got: Vec<Result<(Vec<u8>, Option<String>)>> = pl.install(|| man.blobs.par_iter().map(|b| bytes_of(b)).collect());
        for (b, r) in man.blobs.iter().zip(got) {
            let (mut d, moved) = r?;
            if b.kind != 0 {
                crate::io::kspkg::xor(&mut d);
            }
            let o = b.off as usize;
            mm[o..o + d.len()].copy_from_slice(&d);
            if let Some(p) = moved {
                rep.base_moved.push(format!("{} <- {p}", b.path));
            }
        }
        let misc = &streams["misc"];
        let ta = man.table_at as usize;
        let mut table = misc[ta..ta + TABLE].to_vec();
        crate::io::kspkg::xor(&mut table);
        mm[data_len..].copy_from_slice(&table);
        let mut ga = man.gaps_at as usize;
        for (o, n) in &man.gaps {
            mm[*o as usize..(*o + *n) as usize].copy_from_slice(&misc[ga..ga + *n as usize]);
            ga += *n as usize;
        }
        mm.flush()?;
        rep.identical = hex(&md5(&mm)) == man.out_md5;
        drop(mm);
        drop(f);
        if !rep.identical {
            let _ = std::fs::remove_file(&tmp);
            bail!("rebuilt package does not match the original (MD5)");
        }
        std::fs::rename(&tmp, out)?;
        // sidecars beside the package, named after it (`<out stem>.json`)
        for (n, size, at) in &man.extras {
            let ext = Path::new(n).extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
            let b = misc.get(*at as usize..(*at + *size) as usize).ok_or_else(|| anyhow!("{n}: out of range"))?;
            std::fs::write(out.with_extension(ext), b)?;
        }
    } else {
        let mut files = BTreeMap::new();
        let got: Vec<Result<(Vec<u8>, Option<String>)>> = pl.install(|| man.blobs.par_iter().map(|b| bytes_of(b)).collect());
        for (b, r) in man.blobs.iter().zip(got) {
            let (d, moved) = r?;
            if let Some(p) = moved {
                rep.base_moved.push(format!("{} <- {p}", b.path));
            }
            files.insert(b.path.clone(), d);
        }
        rep.identical = hex(&dir_digest(&files)) == man.out_md5;
        if !rep.identical {
            bail!("rebuilt folder does not match the original (MD5)");
        }
        crate::slim::write_dir(out, &files)?;
    }
    rep.base_files = man.blobs.iter().filter(|b| b.how == "base").count();
    rep.secs = t.elapsed().as_secs_f64();
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planes_roundtrip() {
        let mut pcm = Vec::new();
        for i in 0..1000i32 {
            let a = ((i as f32 * 0.05).sin() * 30000.0) as i16;
            let b = (i * 37 % 65536 - 32768) as i16;
            pcm.extend_from_slice(&a.to_le_bytes());
            pcm.extend_from_slice(&b.to_le_bytes());
        }
        let f = planes_fwd(&pcm, 2);
        assert_eq!(planes_inv(&f, 2), pcm);
    }
}
