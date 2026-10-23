//! One AC1 car folder, opened once and shared by every car stage:
//! the data files (data.acd or data\), the parsed ini set, the LOD0 kn5
//! (memory-mapped; `kn5()` parses it without copying), the author LODs from
//! `lods.ini`, the collider, `ui\ui_car.json` and the skins.
//!
//! LOD0 is `lods.ini [LOD_0] FILE`, else the largest kn5 that is not the
//! collider and has no `_lod_` in its name. A kn5 encrypted with Custom Shaders
//! Patch (tail envelope) is decrypted in memory; other protection is refused.

use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::ac1::ini::{self, Ini};
use crate::ac1::kn5;
use crate::ac1::kn5_decrypt::{self, Kn5Bytes};

pub struct Lod {
    pub file: PathBuf,
    /// lods.ini IN / OUT (metres)
    pub lod_in: f32,
    pub lod_out: f32,
}

pub struct Car {
    pub dir: PathBuf,
    /// the folder name as on disk
    pub folder: String,
    /// lower-case file name -> latin-1 text
    pub data: BTreeMap<String, String>,
    pub kn5_path: PathBuf,
    kn5_map: Kn5Bytes,
    /// author LODs after LOD0 (lods.ini order), when lods.ini lists them
    pub lods: Vec<Lod>,
    /// LOD0's own OUT distance from lods.ini
    pub lod0_out: Option<f32>,
    pub collider: Option<PathBuf>,
    /// `ui\ui_car.json` text (AC1 json is often not strict)
    pub ui_text: String,
    /// skin folders in folder order (sorted by name, as AC1 lists them)
    pub skins: Vec<String>,
    /// the ini files parsed once: lower-case name -> Ini
    inis: BTreeMap<String, Ini>,
}

fn is_kn5(p: &Path) -> bool {
    p.extension().map(|e| e.eq_ignore_ascii_case("kn5")).unwrap_or(false)
}

/// A file in `dir` by name, case-insensitively.
pub fn find_ci(dir: &Path, name: &str) -> Option<PathBuf> {
    let direct = dir.join(name);
    if direct.is_file() {
        return Some(direct);
    }
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| p.file_name().map(|f| f.to_string_lossy().eq_ignore_ascii_case(name)).unwrap_or(false))
}

/// The LOD0 model of the car in `dir` (lods.ini LOD_0, else the largest
/// kn5 that is not a collider or a LOD), its OUT distance and the other LODs.
fn models(dir: &Path, inis: &BTreeMap<String, Ini>) -> Result<(PathBuf, Option<f32>, Vec<Lod>)> {
    // lods.ini is a data file (inside data.acd on Kunos cars); some mods
    // keep a copy in the car folder
    let lods_ini = inis.get("lods.ini").cloned()
        .or_else(|| find_ci(dir, "lods.ini").and_then(|p| std::fs::read(p).ok()).map(|b| ini::parse(&String::from_utf8_lossy(&b))));
    let mut lods = Vec::new();
    let mut lod0: Option<PathBuf> = None;
    let mut lod0_out = None;
    if let Some(li) = &lods_ini {
        let mut idx: Vec<(usize, &str)> = li.names().filter_map(|n| n.strip_prefix("LOD_").and_then(|x| x.parse().ok()).map(|i: usize| (i, n))).collect();
        idx.sort();
        for (i, name) in idx {
            let Some(file) = li.get(name, "FILE").and_then(|f| find_ci(dir, f)) else { continue };
            let lod_in = li.num(name, "IN").unwrap_or(0.0) as f32;
            let lod_out = li.num(name, "OUT").unwrap_or(0.0) as f32;
            if i == 0 && lod0.is_none() {
                lod0 = Some(file);
                lod0_out = Some(lod_out);
            } else {
                lods.push(Lod { file, lod_in, lod_out });
            }
        }
    }
    let kn5_path = match lod0 {
        Some(p) => p,
        None => {
            let mut best: Option<(u64, PathBuf)> = None;
            for e in std::fs::read_dir(dir)?.flatten() {
                let p = e.path();
                let n = p.file_name().unwrap().to_string_lossy().to_lowercase();
                if !is_kn5(&p) || n == "collider.kn5" || n.contains("_lod") {
                    continue;
                }
                let sz = e.metadata().map(|m| m.len()).unwrap_or(0);
                if best.as_ref().map(|(b, _)| sz > *b).unwrap_or(true) {
                    best = Some((sz, p));
                }
            }
            best.map(|(_, p)| p).ok_or_else(|| anyhow::anyhow!("{}: no kn5 model", dir.display()))?
        }
    };
    Ok((kn5_path, lod0_out, lods))
}

impl Car {
    /// The file `open` would load as the model, without reading it.
    pub fn model_path(dir: &Path) -> Result<PathBuf> {
        let data = crate::ac1::acd::read_car_data(dir)?;
        let inis: BTreeMap<String, Ini> = data.iter().filter(|(k, _)| k.ends_with(".ini")).map(|(k, v)| (k.clone(), ini::parse(v))).collect();
        Ok(models(dir, &inis)?.0)
    }

    pub fn open(dir: &Path) -> Result<Car> {
        let folder = dir.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        let data = crate::ac1::acd::read_car_data(dir)?;
        let inis: BTreeMap<String, Ini> = data.iter().filter(|(k, _)| k.ends_with(".ini")).map(|(k, v)| (k.clone(), ini::parse(v))).collect();
        let (kn5_path, lod0_out, lods) = models(dir, &inis)?;
        let kn5_map = kn5_decrypt::open(&kn5_path).with_context(|| format!("{}: opening the kn5", kn5_path.display()))?;
        if kn5::protected(&kn5_map) {
            bail!("{}: the model is protected by its author (encrypted kn5) and cannot be converted", kn5_path.display());
        }
        let collider = find_ci(dir, "collider.kn5");
        let ui_text = find_ci(&dir.join("ui"), "ui_car.json").and_then(|p| std::fs::read(p).ok()).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        let mut skins: Vec<String> = std::fs::read_dir(dir.join("skins")).map(|r| r.flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
        skins.sort_by_key(|s| s.to_lowercase());
        Ok(Car { dir: dir.to_path_buf(), folder, data, kn5_path, kn5_map, lods, lod0_out, collider, ui_text, skins, inis })
    }

    /// LOD0, parsed (slices into the memory map).
    pub fn kn5(&self) -> Result<kn5::Kn5<'_>> {
        kn5::parse(&self.kn5_map).with_context(|| format!("{}", self.kn5_path.display()))
    }

    /// A data ini by file name (`car.ini`); empty when missing.
    pub fn ini(&self, name: &str) -> &Ini {
        static EMPTY: std::sync::OnceLock<Ini> = std::sync::OnceLock::new();
        self.inis.get(&name.to_lowercase()).unwrap_or_else(|| EMPTY.get_or_init(Ini::default))
    }

    /// A ui_car.json string field ("name", "brand", "year", "class", ...).
    pub fn ui(&self, key: &str) -> Option<String> {
        ui_field(&self.ui_text, key)
    }

    /// The skin folder's path.
    pub fn skin_dir(&self, skin: &str) -> PathBuf {
        self.dir.join("skins").join(skin)
    }
}

/// `"key": "value"` or `"key": 1999` out of lenient AC1 json.
pub fn ui_field(text: &str, key: &str) -> Option<String> {
    let re = regex::Regex::new(&format!(r#""{}"\s*:\s*("([^"]*)"|(-?[0-9.]+))"#, regex::escape(key))).ok()?;
    let c = re.captures(text)?;
    let v = c.get(2).or_else(|| c.get(3))?.as_str().trim().to_string();
    (!v.is_empty()).then_some(v)
}

#[cfg(test)]
mod tests {
    #[test]
    fn ui_fields() {
        let t = "{\n\t\"name\": \"Mazda MX-5 NA\",\n\t\"brand\": \"Mazda\",\n\t\"year\": 1989,\n\t\"tags\": [\"#Street\",],\n}";
        assert_eq!(super::ui_field(t, "name").as_deref(), Some("Mazda MX-5 NA"));
        assert_eq!(super::ui_field(t, "year").as_deref(), Some("1989"));
        assert_eq!(super::ui_field(t, "brand").as_deref(), Some("Mazda"));
        assert!(super::ui_field(t, "class").is_none());
    }
}
