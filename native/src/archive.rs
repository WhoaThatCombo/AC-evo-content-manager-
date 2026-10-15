//! Zip and tar (plain or gzip) behind one face - what install.py did with
//! zipfile and `tarfile.open(path, "r:*")`.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

pub struct Entry {
    pub name: String,
    pub size: u64,
    /// compressed size (zip only)
    pub packed: Option<u64>,
}

fn is_zip(path: &Path) -> bool {
    path.to_string_lossy().to_lowercase().ends_with(".zip")
}

fn tar_reader(path: &Path) -> std::io::Result<Box<dyn Read>> {
    let mut f = BufReader::new(File::open(path)?);
    let mut magic = [0u8; 2];
    let n = f.read(&mut magic)?;
    let f = BufReader::new(File::open(path)?);
    let _ = n;
    if magic == [0x1f, 0x8b] {
        Ok(Box::new(flate2::read::GzDecoder::new(f)))
    } else {
        Ok(Box::new(f))
    }
}

fn tar_name(e: &tar::Entry<'_, Box<dyn Read>>) -> String {
    e.path().map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default()
}

fn tar_is_file(e: &tar::Entry<'_, Box<dyn Read>>) -> bool {
    matches!(e.header().entry_type(), tar::EntryType::Regular | tar::EntryType::Continuous)
}

/// Every FILE in the archive (directories left out).
pub fn entries(path: &Path) -> Result<Vec<Entry>, String> {
    if is_zip(path) {
        let mut z = zip::ZipArchive::new(File::open(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for i in 0..z.len() {
            let f = z.by_index_raw(i).map_err(|e| e.to_string())?;
            if f.is_dir() {
                continue;
            }
            out.push(Entry { name: f.name().to_string(), size: f.size(), packed: Some(f.compressed_size()) });
        }
        return Ok(out);
    }
    let mut t = tar::Archive::new(tar_reader(path).map_err(|e| e.to_string())?);
    let mut out = Vec::new();
    for e in t.entries().map_err(|e| e.to_string())? {
        let e = e.map_err(|e| e.to_string())?;
        if tar_is_file(&e) {
            out.push(Entry { name: tar_name(&e), size: e.header().size().unwrap_or(0), packed: None });
        }
    }
    Ok(out)
}

/// Every member name, files only (install._archive_names).
pub fn names(path: &Path) -> Vec<String> {
    entries(path).map(|v| v.into_iter().map(|e| e.name).collect()).unwrap_or_default()
}

/// Call `f(name, size, reader)` for each file, in archive order; `f`
/// returns false to stop early.
pub fn each_file(path: &Path, mut f: impl FnMut(&str, u64, &mut dyn Read) -> Result<bool, String>) -> Result<(), String> {
    if is_zip(path) {
        let mut z = zip::ZipArchive::new(File::open(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        for i in 0..z.len() {
            let mut m = z.by_index(i).map_err(|e| e.to_string())?;
            if m.is_dir() {
                continue;
            }
            let (name, size) = (m.name().to_string(), m.size());
            if !f(&name, size, &mut m)? {
                break;
            }
        }
        return Ok(());
    }
    let mut t = tar::Archive::new(tar_reader(path).map_err(|e| e.to_string())?);
    for e in t.entries().map_err(|e| e.to_string())? {
        let mut e = e.map_err(|e| e.to_string())?;
        if !tar_is_file(&e) {
            continue;
        }
        let (name, size) = (tar_name(&e), e.header().size().unwrap_or(0));
        if !f(&name, size, &mut e)? {
            break;
        }
    }
    Ok(())
}

/// One member's bytes by exact name.
pub fn read_member(path: &Path, want: &str) -> Option<Vec<u8>> {
    let mut out = None;
    let _ = each_file(path, |name, _size, r| {
        if name == want {
            let mut b = Vec::new();
            r.read_to_end(&mut b).map_err(|e| e.to_string())?;
            out = Some(b);
            return Ok(false);
        }
        Ok(true)
    });
    out
}
