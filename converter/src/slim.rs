//! Make an installed package smaller without changing what the game shows.
//!
//! Runs on the finished file map (archive path -> bytes), so it works for a
//! fresh conversion and for any existing car `.kspkg` / track folder
//! (`aceconv slim`). Three steps, in this order:
//!
//! 1. **base refs** - a texture whose `.texture` + `.texturemips` are
//!    byte-identical to a pair in the game's own `content.kspkg` (the donor
//!    copies stock tyres, discs, dirt and lights) is pointed at that pair:
//!    every reference is rewritten to the stock path. Converted materials
//!    already reference `content\cars\common_assets\...` textures, and stock
//!    cars share them the same way, so `common` (the default) only adds
//!    references of a kind the game already resolves for every converted
//!    car. `all` also points at car-specific stock files (the Abarth 695 the
//!    donor came from): same mechanism, but those paths are more likely to
//!    change in a game update.
//! 2. **dedupe** - local textures with identical pixels and header (the
//!    source-name field aside) collapse onto one copy.
//! 3. **unreferenced** - a texture no file references by its full path goes,
//!    unless the game finds it by convention: `generated\` (windshield bakes,
//!    thumbnails), `uiresources\` (brand art) and anything whose file name
//!    appears outside a full path (display css/html name images relatively).
//!    Checked against stock cars: under this rule every stock texture is kept.
//!
//! References are rewritten as protobuf: a file is only edited when every
//! level that holds the path parses and re-encodes byte-identically, and a
//! texture is only repointed when ALL its referrers can be rewritten.

use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use crate::io::pb::{Msg, Val};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BaseRefs {
    Off,
    Common,
    All,
}

#[derive(Clone, Copy, Debug)]
pub struct Opts {
    pub base_refs: BaseRefs,
    pub dedupe: bool,
    pub drop_unreferenced: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Opts { base_refs: BaseRefs::Common, dedupe: true, drop_unreferenced: true }
    }
}

impl Opts {
    /// ACECONV_SLIM=0 turns every step off; ACECONV_BASE_REFS=off|common|all.
    pub fn from_env() -> Opts {
        let mut o = Opts::default();
        if std::env::var("ACECONV_SLIM").as_deref() == Ok("0") {
            return Opts { base_refs: BaseRefs::Off, dedupe: false, drop_unreferenced: false };
        }
        match std::env::var("ACECONV_BASE_REFS").as_deref() {
            Ok("off") | Ok("0") => o.base_refs = BaseRefs::Off,
            Ok("all") => o.base_refs = BaseRefs::All,
            _ => {}
        }
        o
    }
    pub fn any(&self) -> bool {
        self.base_refs != BaseRefs::Off || self.dedupe || self.drop_unreferenced
    }
}

#[derive(Default, Debug)]
pub struct Stats {
    pub base: (usize, u64),
    pub dedupe: (usize, u64),
    pub unreferenced: (usize, u64),
    pub kept_unrewritable: usize,
    pub lines: Vec<String>,
}

impl Stats {
    pub fn saved(&self) -> u64 {
        self.base.1 + self.dedupe.1 + self.unreferenced.1
    }
}

fn is_tex(k: &str) -> bool {
    k.len() > 8 && k[k.len() - 8..].eq_ignore_ascii_case(".texture")
}

/// files whose text can hold references (everything but pixels and audio)
fn is_corpus(k: &str) -> bool {
    let l = k.to_ascii_lowercase();
    !(l.ends_with(".texturemips") || l.ends_with(".bank"))
}

/// Found by the game without any reference: keep.
fn by_convention(lc: &str) -> bool {
    lc.starts_with("uiresources\\") || lc.contains("\\generated\\") || lc.contains("\\uiresources\\") || lc.contains("\\irradiance_volumes\\")
}

/// Every full archive path to a `.texture` mentioned in `lc` (lowercase),
/// plus `lc` with those spans blanked (for the relative-name check).
fn texture_refs(lc: &[u8], out: &mut BTreeSet<String>) -> Vec<u8> {
    let mut blank = lc.to_vec();
    let fin = memchr::memmem::Finder::new(b".texture");
    for hit in fin.find_iter(lc) {
        let end = hit + 8;
        if lc.get(end..end + 4) == Some(b"mips") {
            continue;
        }
        // walk back over path characters
        let mut s = hit;
        while s > 0 && hit - s < 240 {
            let c = lc[s - 1];
            if c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.' | b'\\' | b'/' | b' ' | b'(' | b')' | b'+' | b'&' | b'#' | b'@' | b'!' | b'\'') {
                s -= 1;
            } else {
                break;
            }
        }
        let span = &lc[s..end];
        let mut covered = None;
        for j in 0..span.len() {
            if span[j..].starts_with(b"content\\") || span[j..].starts_with(b"uiresources\\") {
                out.insert(String::from_utf8_lossy(&span[j..]).replace('/', "\\"));
                covered.get_or_insert(j);
            }
        }
        if let Some(j) = covered {
            for b in &mut blank[s + j..end] {
                *b = 0;
            }
        }
    }
    blank
}

/// Rewrite every length-delimited field equal (case-insensitively) to an
/// old path. None = a level holding a path did not round-trip, or a path
/// survives the rewrite: the caller must leave this file alone.
fn rewrite(d: &[u8], subs: &[(Vec<u8>, Vec<u8>)], depth: u32) -> Option<Vec<u8>> {
    let lc = d.to_ascii_lowercase();
    let has = |b: &[u8]| subs.iter().any(|(o, _)| memchr::memmem::find(b, o).is_some());
    if !has(&lc) {
        return Some(d.to_vec());
    }
    if depth > 32 {
        return None;
    }
    let mut m = Msg::parse(d).ok()?;
    if m.encode() != d {
        return None;
    }
    for f in m.fields.iter_mut() {
        let Val::Bytes(b) = &mut f.val else { continue };
        let bl = b.to_ascii_lowercase();
        if let Some((_, new)) = subs.iter().find(|(o, _)| *o == bl) {
            *b = new.clone();
        } else if has(&bl) {
            *b = rewrite(b, subs, depth + 1)?;
        }
    }
    let out = m.encode();
    if has(&out.to_ascii_lowercase()) {
        return None;
    }
    Some(out)
}

/// Header with the source-name field (10) removed: two textures that only
/// differ there are the same texture.
fn header_key(h: &[u8]) -> Vec<u8> {
    match Msg::parse(h) {
        Ok(mut m) => {
            m.remove(10);
            m.encode()
        }
        Err(_) => h.to_vec(),
    }
}

/// Cheap pre-check before hashing a stock candidate: three 64-byte windows
/// (8-aligned, so a XOR'd entry decrypts with the key at phase 0).
fn sample_eq(raw: &[u8], plain: bool, mine: &[u8]) -> bool {
    if raw.len() != mine.len() {
        return false;
    }
    let n = mine.len();
    let wins = [0usize, (n / 2) & !7, n.saturating_sub(64) & !7];
    for &o in &wins {
        let e = (o + 64).min(n);
        if plain {
            if raw[o..e] != mine[o..e] {
                return false;
            }
        } else {
            let mut w = raw[o..e].to_vec();
            crate::io::kspkg::xor(&mut w);
            if w[..] != mine[o..e] {
                return false;
            }
        }
    }
    true
}

/// MD5 of the three sample_eq windows, decrypted: equal for equal files, so
/// a remembered one rules a stock candidate out without reading it.
fn sample_sig(raw: &[u8], plain: bool) -> [u8; 16] {
    let n = raw.len();
    let mut w = Vec::with_capacity(192);
    for o in [0usize, (n / 2) & !7, n.saturating_sub(64) & !7] {
        let e = (o + 64).min(n);
        let a = w.len();
        w.extend_from_slice(&raw[o..e]);
        if !plain {
            crate::io::kspkg::xor(&mut w[a..]);
        }
    }
    digest(&w)
}

fn digest(b: &[u8]) -> [u8; 16] {
    use md5::{Digest, Md5};
    Md5::digest(b).into()
}

/// The game's content.kspkg, indexed for "is this exact texture stock?".
pub struct Base {
    pkg: crate::io::kspkg::Pkg,
    /// texturemips size -> entry indices
    by_size: HashMap<usize, Vec<usize>>,
    by_path: HashMap<String, usize>,
    /// MD5 of stock entries already hashed, by (offset, size): kept on disk
    /// per content.kspkg build (its size + modification time), so a stock
    /// file is hashed once, not once per package
    memo: std::sync::Mutex<HashMap<(u64, u64), Memo>>,
    memo_file: Option<std::path::PathBuf>,
    memo_loaded: usize,
}

/// Where the stock-digest memo lives: ACECONV_SHARE_INDEX_DIR, else beside
/// the generated donor (`<donor dir>\..\share_index`).
fn memo_path(content: &Path) -> Option<std::path::PathBuf> {
    let md = std::fs::metadata(content).ok()?;
    let mt = md.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    let dir = match std::env::var_os("ACECONV_SHARE_INDEX_DIR") {
        Some(d) => std::path::PathBuf::from(d),
        None => crate::donor::cache_dir().ok()?.parent()?.join("share_index"),
    };
    Some(dir.join(format!("stock_md5_{}_{mt}.bin", md.len())))
}

/// What is remembered of one stock entry: its sample signature, and its
/// whole MD5 once it was hashed.
#[derive(Clone, Copy)]
struct Memo {
    sample: [u8; 16],
    full: Option<[u8; 16]>,
}

const MEMO_REC: usize = 49;

fn memo_load(p: &Path) -> HashMap<(u64, u64), Memo> {
    let mut m = HashMap::new();
    if let Ok(b) = std::fs::read(p) {
        for r in b.chunks_exact(MEMO_REC) {
            let off = u64::from_le_bytes(r[0..8].try_into().unwrap());
            let size = u64::from_le_bytes(r[8..16].try_into().unwrap());
            let sample: [u8; 16] = r[16..32].try_into().unwrap();
            let full = (r[48] == 1).then(|| r[32..48].try_into().unwrap());
            m.insert((off, size), Memo { sample, full });
        }
    }
    m
}

impl Base {
    pub fn open(p: &Path) -> Result<Base> {
        let pkg = crate::io::kspkg::Pkg::open(p)?;
        let mut by_size: HashMap<usize, Vec<usize>> = HashMap::new();
        let mut by_path = HashMap::new();
        for (i, e) in pkg.entries.iter().enumerate() {
            by_path.insert(e.path.to_ascii_lowercase(), i);
            if e.path.len() > 12 && e.path[e.path.len() - 12..].eq_ignore_ascii_case(".texturemips") {
                by_size.entry(e.size).or_default().push(i);
            }
        }
        let memo_file = memo_path(p);
        let memo = memo_file.as_deref().map(memo_load).unwrap_or_default();
        let memo_loaded = memo.len();
        Ok(Base { pkg, by_size, by_path, memo: std::sync::Mutex::new(memo), memo_file, memo_loaded })
    }

    /// Write the stock-digest memo back when it grew (stale builds' files go).
    pub fn save_memo(&self) {
        let Some(p) = &self.memo_file else { return };
        let m = self.memo.lock().unwrap();
        if m.len() == self.memo_loaded {
            return;
        }
        let mut b = Vec::with_capacity(m.len() * MEMO_REC);
        for ((off, size), d) in m.iter() {
            b.extend_from_slice(&off.to_le_bytes());
            b.extend_from_slice(&size.to_le_bytes());
            b.extend_from_slice(&d.sample);
            b.extend_from_slice(&d.full.unwrap_or_default());
            b.push(d.full.is_some() as u8);
        }
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
            if let Ok(rd) = std::fs::read_dir(dir) {
                for e in rd.flatten() {
                    let n = e.file_name().to_string_lossy().into_owned();
                    if n.starts_with("stock_md5_") && e.path() != *p {
                        let _ = std::fs::remove_file(e.path());
                    }
                }
            }
        }
        let tmp = p.with_extension("bin.part");
        if std::fs::write(&tmp, &b).is_ok() {
            let _ = std::fs::rename(&tmp, p);
        }
    }

    /// Is this stock entry the same file as `data` (whose sample signature
    /// and MD5 are given)? The memo answers without reading when it can;
    /// what had to be read is remembered.
    fn same(&self, e: &crate::io::kspkg::Entry, my_sample: &[u8; 16], mine: &[u8; 16]) -> bool {
        let key = (e.offset() as u64, e.size as u64);
        let known = self.memo.lock().unwrap().get(&key).copied();
        if let Some(m) = known {
            if m.sample != *my_sample {
                return false;
            }
            if let Some(f) = m.full {
                return f == *mine;
            }
        }
        let Some(raw) = self.pkg.raw(e) else { return false };
        let plain = crate::io::kspkg::Pkg::is_plain(e);
        let sample = known.map(|m| m.sample).unwrap_or_else(|| sample_sig(raw, plain));
        let mut rec = Memo { sample, full: None };
        let hit = if sample == *my_sample {
            let d = if plain { digest(raw) } else { let mut v = raw.to_vec(); crate::io::kspkg::xor(&mut v); digest(&v) };
            rec.full = Some(d);
            d == *mine
        } else {
            false
        };
        self.memo.lock().unwrap().insert(key, rec);
        hit
    }

    /// Stock `.texture` path whose pair equals (header, mips), same file
    /// name first.
    fn find_texture(&self, name: &str, header: &[u8], mips: &[u8], allow: impl Fn(&str) -> bool) -> Option<String> {
        let cands = self.by_size.get(&mips.len())?;
        let want = name.to_ascii_lowercase();
        let mut order: Vec<usize> = cands.iter().copied().filter(|&i| allow(&self.pkg.entries[i].path)).collect();
        order.sort_by_key(|&i| {
            let p = &self.pkg.entries[i].path;
            let stem = p.rsplit('\\').next().unwrap_or(p).to_ascii_lowercase();
            (stem.trim_end_matches("mips") != want, i)
        });
        let mine = digest(mips);
        let hk = header_key(header);
        for i in order.into_iter().take(4096) {
            let e = &self.pkg.entries[i];
            let Some(raw) = self.pkg.raw(e) else { continue };
            if !sample_eq(raw, crate::io::kspkg::Pkg::is_plain(e), mips) {
                continue;
            }
            let plain = if crate::io::kspkg::Pkg::is_plain(e) { raw.to_vec() } else { let mut v = raw.to_vec(); crate::io::kspkg::xor(&mut v); v };
            if plain.len() != mips.len() || digest(&plain) != mine {
                continue;
            }
            let tex = &e.path[..e.path.len() - 4];
            let Some(&ti) = self.by_path.get(&tex.to_ascii_lowercase()) else { continue };
            let Ok(h) = self.pkg.read(&self.pkg.entries[ti]) else { continue };
            if header_key(&h) == hk {
                return Some(self.pkg.entries[ti].path.clone());
            }
        }
        None
    }

    /// Exact stock entry for any file (share packages): same size, same
    /// digest; same file name tried first.
    pub fn find_file(&self, name: &str, data: &[u8], mine: [u8; 16], index: &HashMap<usize, Vec<usize>>) -> Option<String> {
        let cands = index.get(&data.len())?;
        let want = name.to_ascii_lowercase();
        let mut order: Vec<usize> = cands.clone();
        order.sort_by_key(|&i| {
            let p = &self.pkg.entries[i].path;
            let stem = p.rsplit('\\').next().unwrap_or(p).to_ascii_lowercase();
            // common_assets first among equals: the most stable stock paths
            (stem != want, !p.to_ascii_lowercase().contains("\\common_assets\\"), i)
        });
        let my_sample = sample_sig(data, true);
        for i in order.into_iter().take(4096) {
            let e = &self.pkg.entries[i];
            if self.same(e, &my_sample, &mine) {
                return Some(e.path.clone());
            }
        }
        None
    }

    /// size -> entries, for `find_file` (built once per package)
    pub fn size_index(&self) -> HashMap<usize, Vec<usize>> {
        let mut m: HashMap<usize, Vec<usize>> = HashMap::new();
        for (i, e) in self.pkg.entries.iter().enumerate() {
            m.entry(e.size).or_default().push(i);
        }
        m
    }

    /// Bytes of a stock entry by path (decrypted).
    pub fn read(&self, path: &str) -> Option<Vec<u8>> {
        let i = *self.by_path.get(&path.to_ascii_lowercase())?;
        self.pkg.read(&self.pkg.entries[i]).ok()
    }

    /// Bytes of the first stock entry with this size and digest.
    pub fn read_by_digest(&self, size: usize, md5: [u8; 16], index: &HashMap<usize, Vec<usize>>) -> Option<(String, Vec<u8>)> {
        for &i in index.get(&size)? {
            let e = &self.pkg.entries[i];
            let Ok(b) = self.pkg.read(e) else { continue };
            if digest(&b) == md5 {
                return Some((e.path.clone(), b));
            }
        }
        None
    }
}

pub fn md5(b: &[u8]) -> [u8; 16] {
    digest(b)
}

/// Archive path of a map key: car packages store full paths already; track
/// folders are relative to `content\tracks\<id>\`.
fn full(prefix: &str, k: &str) -> String {
    format!("{prefix}{k}")
}

/// Slim `files` in place. `prefix` = what the map keys are relative to
/// ("" for a car package, `content\tracks\<id>\` for a track folder).
pub fn slim(files: &mut BTreeMap<String, Vec<u8>>, prefix: &str, base: Option<&Base>, o: &Opts) -> Result<Stats> {
    let mut st = Stats::default();
    let mips_of = |files: &BTreeMap<String, Vec<u8>>, k: &str| files.get(&format!("{k}mips")).map(|m| m.len() as u64).unwrap_or(0);
    // repointed textures: their local copies go in step 3 whatever its setting
    let mut credited: BTreeSet<String> = BTreeSet::new();
    let t0 = std::time::Instant::now();

    // ---- index of references (recomputed after edits)
    let index = |files: &BTreeMap<String, Vec<u8>>| -> (BTreeSet<String>, HashMap<String, Vec<String>>, Vec<u8>) {
        use rayon::prelude::*;
        let parts: Vec<(String, BTreeSet<String>, Vec<u8>)> = files.par_iter().filter(|(k, _)| is_corpus(k)).map(|(k, v)| {
            let mut s = BTreeSet::new();
            let mut blank = texture_refs(&v.to_ascii_lowercase(), &mut s);
            // a .texture header names its own source image (field 10): not a reference
            if is_tex(k) {
                blank.clear();
            }
            (k.clone(), s, blank)
        }).collect();
        let mut all = BTreeSet::new();
        let mut who: HashMap<String, Vec<String>> = HashMap::new();
        let mut blob = Vec::new();
        for (k, s, b) in parts {
            for r in s {
                who.entry(r.clone()).or_default().push(k.clone());
                all.insert(r);
            }
            blob.extend_from_slice(&b);
            blob.push(0);
        }
        (all, who, blob)
    };

    // repoint `old` (map key of a .texture) -> `new` (full archive path); all or nothing
    let repoint = |files: &mut BTreeMap<String, Vec<u8>>, who: &HashMap<String, Vec<String>>, old: &str, new: &str| -> bool {
        let old_full = full(prefix, old).to_ascii_lowercase();
        let Some(refs) = who.get(&old_full) else { return false };
        let subs = vec![(old_full.as_bytes().to_vec(), new.as_bytes().to_vec())];
        let mut edits = Vec::new();
        for r in refs {
            match files.get(r).and_then(|d| rewrite(d, &subs, 0)) {
                Some(n) => edits.push((r.clone(), n)),
                None => return false,
            }
        }
        for (r, n) in edits {
            files.insert(r, n);
        }
        true
    };

    // ---- 1. base refs
    if let (Some(base), true) = (base, o.base_refs != BaseRefs::Off) {
        let (_, who, _) = index(files);
        let allow = |p: &str| {
            let l = p.to_ascii_lowercase();
            match o.base_refs {
                BaseRefs::Common => l.starts_with("content\\cars\\common_assets\\"),
                _ => l.starts_with("content\\"),
            }
        };
        let texs: Vec<String> = files.keys().filter(|k| is_tex(k)).cloned().collect();
        for k in texs {
            let lc = full(prefix, &k).to_ascii_lowercase();
            if by_convention(&lc) || !who.contains_key(&lc) {
                continue;
            }
            let (Some(h), Some(m)) = (files.get(&k), files.get(&format!("{k}mips"))) else { continue };
            if m.len() < 65536 {
                continue;
            }
            let name = k.rsplit('\\').next().unwrap_or(&k);
            let Some(stock) = base.find_texture(name, h, m, allow) else { continue };
            let hl = h.len() as u64;
            if repoint(files, &who, &k, &stock) {
                st.base.0 += 1;
                st.base.1 += mips_of(files, &k) + hl;
                credited.insert(k.clone());
                st.lines.push(format!("base ref: {k} -> {stock}"));
            } else {
                st.kept_unrewritable += 1;
                st.lines.push(format!("base ref kept local (a referrer would not rewrite): {k}"));
            }
        }
    }

    st.lines.push(format!("time: base refs {:.2} s", t0.elapsed().as_secs_f64()));
    // ---- 2. dedupe
    if o.dedupe {
        let (_, who, _) = index(files);
        let mut groups: BTreeMap<([u8; 16], Vec<u8>), Vec<String>> = BTreeMap::new();
        for k in files.keys().filter(|k| is_tex(k)) {
            let lc = full(prefix, k).to_ascii_lowercase();
            if by_convention(&lc) || !who.contains_key(&lc) {
                continue;
            }
            let Some(m) = files.get(&format!("{k}mips")) else { continue };
            groups.entry((digest(m), header_key(&files[k]))).or_default().push(k.clone());
        }
        for (_, g) in groups.into_iter().filter(|g| g.1.len() > 1) {
            let keep = full(prefix, &g[0]);
            for k in &g[1..] {
                let sz = mips_of(files, k);
                if repoint(files, &who, k, &keep) {
                    st.dedupe.0 += 1;
                    st.dedupe.1 += sz;
                    credited.insert(k.clone());
                    st.lines.push(format!("dedupe: {k} -> {}", g[0]));
                } else {
                    st.kept_unrewritable += 1;
                }
            }
        }
    }

    st.lines.push(format!("time: + dedupe {:.2} s", t0.elapsed().as_secs_f64()));
    // ---- 3. unreferenced
    {
        let (all, _, blob) = index(files);
        let gone: Vec<String> = files.keys().filter(|k| is_tex(k)).filter(|k| {
            let lc = full(prefix, k).to_ascii_lowercase();
            if credited.contains(*k) {
                return !all.contains(&lc);
            }
            if !o.drop_unreferenced {
                return false;
            }
            if by_convention(&lc) || all.contains(&lc) {
                return false;
            }
            // a relative / bare-name mention (display css, html) keeps it
            let stem = lc.rsplit('\\').next().unwrap_or(&lc).trim_end_matches(".texture").to_string();
            memchr::memmem::find(&blob, stem.as_bytes()).is_none()
        }).cloned().collect();
        for k in gone {
            let m = files.remove(&format!("{k}mips")).map(|m| m.len() as u64).unwrap_or(0);
            let h = files.remove(&k).map(|h| h.len() as u64).unwrap_or(0);
            if credited.contains(&k) {
                continue;
            }
            st.unreferenced.0 += 1;
            st.unreferenced.1 += m + h;
            st.lines.push(format!("unreferenced: {k}"));
        }
    }
    st.lines.push(format!("time: + unreferenced {:.2} s", t0.elapsed().as_secs_f64()));
    Ok(st)
}

/// `aceconv slim <in.kspkg | track dir> <out.kspkg | dir>`
pub fn slim_path(src: &Path, dst: &Path, game: Option<&Path>, o: &Opts) -> Result<Stats> {
    let base = match (o.base_refs, game) {
        (BaseRefs::Off, _) | (_, None) => None,
        (_, Some(g)) => Some(Base::open(g)?),
    };
    if src.is_dir() {
        let id = src.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let prefix = format!("content\\tracks\\{id}\\");
        let mut files = read_dir(src)?;
        let st = slim(&mut files, &prefix, base.as_ref(), o)?;
        write_dir(dst, &files)?;
        Ok(st)
    } else {
        let mut files: BTreeMap<String, Vec<u8>> = crate::io::kspkg::read(src, "")?.into_iter().collect();
        let st = slim(&mut files, "", base.as_ref(), o)?;
        let mut list: Vec<(String, Vec<u8>)> = files.into_iter().collect();
        crate::io::kspkg::write(dst, &mut list)?;
        // the car's <id>.json goes along
        if let Ok(j) = std::fs::read(src.with_extension("json")) {
            std::fs::write(dst.with_extension("json"), j)?;
        }
        Ok(st)
    }
}

/// Every file under `root`, keyed by its relative path with backslashes.
pub fn read_dir(root: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d)? {
            let e = e?;
            let p = e.path();
            if e.file_type()?.is_dir() {
                stack.push(p);
            } else {
                let rel = p.strip_prefix(root)?.to_string_lossy().replace('/', "\\");
                out.insert(rel, std::fs::read(&p)?);
            }
        }
    }
    Ok(out)
}

/// Write a map from `read_dir` back out (into a fresh folder, swapped in).
pub fn write_dir(dst: &Path, files: &BTreeMap<String, Vec<u8>>) -> Result<()> {
    let tmp = dst.with_extension("part");
    if tmp.exists() {
        std::fs::remove_dir_all(&tmp)?;
    }
    for (k, v) in files {
        let p = tmp.join(k.replace('\\', "/"));
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(&p, v)?;
    }
    if dst.exists() {
        std::fs::remove_dir_all(dst)?;
    }
    std::fs::rename(&tmp, dst)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_and_blanking() {
        let mut s = BTreeSet::new();
        let b = texture_refs(b"\x4fcontent\\cars\\x\\texture\\a_c.texture*\x02 url(images/bgr.png)", &mut s);
        assert!(s.contains("content\\cars\\x\\texture\\a_c.texture"));
        assert!(memchr::memmem::find(&b, b"a_c").is_none());
        assert!(memchr::memmem::find(&b, b"bgr").is_some());
    }

    #[test]
    fn rewrite_nested() {
        let mut inner = Msg::default();
        inner.set_str(1, "content\\cars\\x\\texture\\A_C.texture");
        inner.set_varint(2, 7);
        let mut outer = Msg::default();
        outer.set_str(1, "name");
        outer.set(4, Val::Msg(inner));
        let d = outer.encode();
        let subs = vec![(b"content\\cars\\x\\texture\\a_c.texture".to_vec(), b"content\\cars\\common_assets\\b.texture".to_vec())];
        let n = rewrite(&d, &subs, 0).unwrap();
        let m = Msg::parse(&n).unwrap();
        let i = m.sub(4).unwrap();
        assert_eq!(i.str(1).as_deref(), Some("content\\cars\\common_assets\\b.texture"));
        assert_eq!(i.varint(2), Some(7));
        // not protobuf around the path -> refuse
        assert!(rewrite(b"xx content\\cars\\x\\texture\\a_c.texture yy", &subs, 0).is_none());
    }
}
