//! The in-memory file map every stage edits: archive path (backslashes, as
//! stored in a .kspkg) -> bytes, with a journal so a failed stage can be
//! undone. Nothing is extracted to disk and packed back.

use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Archive path (backslashes, as stored in a .kspkg) -> bytes.
#[derive(Default)]
pub struct Files {
    pub map: BTreeMap<String, Vec<u8>>,
    /// prior values of entries touched since `begin` (first touch only)
    journal: Option<BTreeMap<String, Option<Vec<u8>>>>,
}

impl Files {
    pub fn get(&self, p: &str) -> Option<&Vec<u8>> {
        self.map.get(&norm(p))
    }
    fn note(&mut self, k: &str) {
        if let Some(j) = &mut self.journal {
            if !j.contains_key(k) {
                j.insert(k.to_string(), self.map.get(k).cloned());
            }
        }
    }
    pub fn put(&mut self, p: &str, data: Vec<u8>) {
        let k = norm(p);
        self.note(&k);
        self.map.insert(k, data);
    }
    pub fn remove(&mut self, p: &str) -> Option<Vec<u8>> {
        let k = norm(p);
        self.note(&k);
        self.map.remove(&k)
    }
    /// Start recording put/remove so a failed stage can be undone.
    /// ⚠ direct `map` edits bypass the journal
    pub fn begin(&mut self) {
        self.journal = Some(BTreeMap::new());
    }
    pub fn commit(&mut self) {
        self.journal = None;
    }
    pub fn rollback(&mut self) {
        for (k, v) in self.journal.take().unwrap_or_default() {
            match v {
                Some(b) => {
                    self.map.insert(k, b);
                }
                None => {
                    self.map.remove(&k);
                }
            }
        }
    }
    /// Paths under a directory prefix (case-insensitive).
    pub fn under(&self, dir: &str) -> Vec<String> {
        let d = norm(dir).to_lowercase() + "\\";
        self.map.keys().filter(|k| k.to_lowercase().starts_with(&d)).cloned().collect()
    }
    pub fn find_ci(&self, p: &str) -> Option<String> {
        let want = norm(p).to_lowercase();
        self.map.keys().find(|k| k.to_lowercase() == want).cloned()
    }
}

pub fn norm(p: &str) -> String {
    p.replace('/', "\\").trim_start_matches('\\').to_string()
}

/// A donor car (`content\cars\<donor_id>\...`), from a zip or an extracted folder.
pub fn load_donor(src: &Path, donor_id: &str) -> Result<Files> {
    let mut files = Files::default();
    if src.is_dir() {
        let mut stack = vec![src.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d)? {
                let p = e?.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    let rel = p.strip_prefix(src).unwrap().to_string_lossy().into_owned();
                    files.put(&rel, std::fs::read(&p)?);
                }
            }
        }
    } else {
        let f = std::fs::File::open(src).with_context(|| format!("open {}", src.display()))?;
        let mut z = zip::ZipArchive::new(f)?;
        for i in 0..z.len() {
            let mut e = z.by_index(i)?;
            if e.is_dir() {
                continue;
            }
            let name = e.name()?.to_string();
            let mut buf = Vec::with_capacity(e.size() as usize);
            e.read_to_end(&mut buf)?;
            files.put(&name, buf);
        }
    }
    if files.under(&format!("content\\cars\\{donor_id}")).is_empty() {
        bail!("{} holds no content\\cars\\{donor_id}", src.display());
    }
    Ok(files)
}

/// Small random source (time + counter), enough for GUIDs and ids.
pub struct Rng(u64);
impl Default for Rng {
    fn default() -> Self {
        Rng::new()
    }
}
impl Rng {
    pub fn new() -> Rng {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64;
        Rng(t ^ 0x9e3779b97f4a7c15 ^ (std::process::id() as u64) << 32)
    }
    /// A fixed seed (tests, reproducible runs).
    pub fn seeded(s: u64) -> Rng {
        Rng(s | 1)
    }
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    pub fn guid(&mut self) -> String {
        let (a, b) = (self.next(), self.next());
        format!("{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}", a >> 32, (a >> 16) & 0xffff, a & 0xfff,
            ((b >> 48) & 0x3fff) | 0x8000, b & 0xffff_ffff_ffff)
    }
}

/// What the `<id>.json` sidecar of a car mod carries.
pub struct ModMeta<'a> {
    pub id: &'a str,
    pub mech_preset: &'a str,
    pub display_name: &'a str,
    pub perf: f64,
    /// the AC1 folder the mod was converted from (lets a later run find and
    /// replace its own earlier conversion, see `ids::legacy`)
    pub source_folder: Option<&'a str>,
    /// the AC1 drivetrain.ini [TRACTION] TYPE (check-car compares the
    /// package's EVO traction type with it)
    pub ac1_traction: Option<&'a str>,
}

/// Write `<out>/<id>.kspkg` + `<id>.json`.
pub fn write_mod(files: &mut Files, out_dir: &Path, m: &ModMeta) -> Result<PathBuf> {
    std::fs::create_dir_all(out_dir)?;
    let mut list: Vec<(String, Vec<u8>)> = std::mem::take(&mut files.map).into_iter().collect();
    let pkg = out_dir.join(format!("{}.kspkg", m.id));
    crate::io::kspkg::write(&pkg, &mut list)?;
    files.map = list.into_iter().collect();
    let mut car = serde_json::json!({
        "name": m.mech_preset, "display_name": m.display_name, "performance_indicator": m.perf,
        "property_1": 0, "property_2": 2, "property_3": 0,
    });
    if let Some(s) = m.source_folder {
        car["aceconv_source"] = serde_json::Value::String(s.to_string());
    }
    if let Some(t) = m.ac1_traction {
        car["aceconv_ac1_traction"] = serde_json::Value::String(t.to_string());
    }
    let json = serde_json::json!({ "cars": [car] });
    std::fs::write(out_dir.join(format!("{}.json", m.id)), serde_json::to_string_pretty(&json)?)?;
    Ok(pkg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_rolls_back() {
        let mut f = Files::default();
        f.put("a/b.txt", b"1".to_vec());
        f.begin();
        f.put("a\\b.txt", b"2".to_vec());
        f.put("c.txt", b"3".to_vec());
        f.remove("a/b.txt");
        f.rollback();
        assert_eq!(f.get("a\\b.txt").unwrap(), b"1");
        assert!(f.get("c.txt").is_none());
    }

    #[test]
    fn guid_shape() {
        let g = Rng::seeded(7).guid();
        assert_eq!(g.len(), 36);
        assert_eq!(&g[14..15], "4");
    }
}
