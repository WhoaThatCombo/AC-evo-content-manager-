//! Ids of converted mods, and finding earlier conversions of the same source.
//!
//! # The id scheme (ours)
//!
//! Measured on a stock content.kspkg (`aceconv stockstats`, "ids"):
//! - car ids: 74 cars, every one `ks_` + `[a-z0-9_]`, 13..37 characters;
//! - track ids: 20 tracks, `[a-z0-9_]`, 3..14 characters, no prefix.
//!
//! A converted id is a pure function of the AC1 folder name, so converting
//! the same folder again replaces the same mod:
//! - car: `ks_` + slug (at most 20 chars) + `_` + 5 base36 chars (more
//!   when the id would be shorter than 13), 13..29 characters;
//! - track: slug (at most 9 chars, starting with a letter) + `_` + 4
//!   base36 chars, at most 14 characters.
//!
//! slug = the lower-cased folder name with every run of other characters
//! turned into one `_` (a leading `ks_` dropped for cars, it is added back);
//! the suffix is FNV-1a-64 of the lower-cased folder name in base 36.
//! Folders that share a slug prefix still differ in the suffix (36^5 = 60
//! million car suffixes, 36^4 = 1.7 million track suffixes per slug).
//! [`car_id`] / [`track_id`] also take the ids already in use (the stock
//! ones from content.kspkg): on a clash the suffix grows by one character
//! until it is free.
//!
//! No fixed length: the game accepts ids of any length in that range, and
//! the donor clone renames through protobuf (length prefixes re-encoded,
//! see `donor.rs`), so the id does not have to match the donor's length.

use anyhow::Result;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

const B36: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

pub fn fnv1a64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn base36(mut v: u64, n: usize) -> String {
    let mut out = vec![b'0'; n];
    for c in out.iter_mut().rev() {
        *c = B36[(v % 36) as usize];
        v /= 36;
    }
    String::from_utf8(out).unwrap()
}

/// Lower-case `[a-z0-9]` runs joined by single `_`.
pub fn slug(s: &str) -> String {
    let mut o = String::new();
    for c in s.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            o.push(c);
        } else if !o.is_empty() && !o.ends_with('_') {
            o.push('_');
        }
    }
    o.trim_end_matches('_').to_string()
}

fn cut(s: &str, n: usize) -> String {
    s.chars().take(n).collect::<String>().trim_end_matches('_').to_string()
}

fn with_suffix(base: &str, folder: &str, n: usize, taken: &HashSet<String>) -> String {
    let h = fnv1a64(&folder.to_lowercase());
    for extra in 0..8 {
        let id = format!("{base}_{}", base36(h, n + extra));
        if !taken.contains(&id) {
            return id;
        }
    }
    format!("{base}_{}", base36(h, 13))
}

/// The id of a converted AC1 car folder.
pub fn car_id(folder: &str, taken: &HashSet<String>) -> String {
    let s = slug(folder);
    let s = s.strip_prefix("ks_").unwrap_or(&s);
    let mut base = cut(s, 20);
    if base.is_empty() {
        base = "car".into();
    }
    let base = format!("ks_{base}");
    // never shorter than the shortest stock id (13): a longer suffix fills up
    let n = 5usize.max(13usize.saturating_sub(base.len() + 1));
    with_suffix(&base, folder, n, taken)
}

/// The id of a converted AC1 track folder.
pub fn track_id(folder: &str, taken: &HashSet<String>) -> String {
    let mut base = cut(&slug(folder), 9);
    if !base.starts_with(|c: char| c.is_ascii_lowercase()) {
        base = cut(&format!("t{base}"), 9);
    }
    with_suffix(&base, folder, 4, taken)
}

/// `kind` = "cars" | "tracks".
pub fn id_for(kind: &str, folder: &str, taken: &HashSet<String>) -> String {
    if kind == "cars" { car_id(folder, taken) } else { track_id(folder, taken) }
}

/// Is `id` a valid id of that kind (the stock character set and length range)?
pub fn valid(kind: &str, id: &str) -> bool {
    let chars = !id.is_empty() && id.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
        && id.starts_with(|c: char| c.is_ascii_lowercase()) && !id.ends_with('_');
    if kind == "cars" {
        chars && id.starts_with("ks_") && (13..=37).contains(&id.len())
    } else {
        chars && (3..=16).contains(&id.len())
    }
}

// ------------------------------------------------------------ legacy ids --

/// An installed mod that looks like a conversion of the same AC1 source.
#[derive(Debug, Clone)]
pub struct Earlier {
    pub id: String,
    /// the package (cars) or folder (tracks)
    pub path: PathBuf,
    /// why it matched, strongest first
    pub why: Vec<String>,
    /// 2 = it names the same source folder; 1 = same display name only
    pub strength: u8,
}

/// Squash a name for comparison: lower-case letters and digits only.
pub fn squash(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

/// The `"name"` of an AC1 ui json (AC1 json is often not strict json).
pub fn ui_name(json_text: &str) -> Option<String> {
    let re = regex::Regex::new(r#""name"\s*:\s*"([^"]*)""#).unwrap();
    re.captures(json_text).map(|c| c[1].trim().to_string()).filter(|s| !s.is_empty())
}

/// The AC1 source's own ui name: `ui\ui_car.json`, or for a track
/// `ui\ui_track.json` / the first `ui\<layout>\ui_track.json`.
pub fn ac1_ui_name(src: &Path, kind: &str) -> Option<String> {
    let read = |p: PathBuf| std::fs::read(p).ok().map(|b| String::from_utf8_lossy(&b).into_owned()).and_then(|t| ui_name(&t));
    if kind == "cars" {
        return read(src.join("ui").join("ui_car.json"));
    }
    if let Some(n) = read(src.join("ui").join("ui_track.json")) {
        return Some(n);
    }
    let mut subs: Vec<PathBuf> = std::fs::read_dir(src.join("ui")).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    subs.sort();
    subs.into_iter().find_map(|d| read(d.join("ui_track.json")))
}

/// The names a car mod's `<id>.json` sidecar and `.moddedcarcontent` give.
fn car_mod_names(pkg: &Path, id: &str) -> (Option<String>, Vec<String>) {
    let mut source = None;
    let mut names = Vec::new();
    if let Ok(t) = std::fs::read_to_string(pkg.with_extension("json")) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
            for c in v["cars"].as_array().into_iter().flatten() {
                if let Some(s) = c["aceconv_source"].as_str() {
                    source = Some(s.to_string());
                }
                if let Some(s) = c["display_name"].as_str() {
                    names.push(s.to_string());
                }
            }
        }
    }
    if let Ok(p) = crate::io::kspkg::Pkg::open(pkg) {
        let want = format!("content\\cars\\{id}\\{id}.moddedcarcontent");
        if let Some(e) = p.find(&want) {
            if let Some(n) = p.read(e).ok().and_then(|b| crate::io::pb::Msg::parse(&b).ok()).and_then(|m| m.str(4)) {
                names.push(n);
            }
        }
    }
    (source, names)
}

/// Earlier conversions of the AC1 folder `src` among the mods in `mods`
/// (`<mods>\<id>.kspkg` + `<id>.json` for cars, `<mods>\content\tracks\<id>\`
/// for tracks). `kind` = "cars" | "tracks". Nothing is changed.
pub fn legacy(mods: &Path, kind: &str, src: &Path) -> Result<Vec<Earlier>> {
    let folder = src.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    let want_src = folder.to_lowercase();
    let want_name = ac1_ui_name(src, kind).map(|n| squash(&n)).filter(|n| !n.is_empty());
    let mut out = Vec::new();
    if kind == "cars" {
        for e in std::fs::read_dir(mods)?.flatten() {
            let p = e.path();
            if !p.extension().map(|x| x.eq_ignore_ascii_case("kspkg")).unwrap_or(false) {
                continue;
            }
            let id = p.file_stem().unwrap().to_string_lossy().into_owned();
            let (source, names) = car_mod_names(&p, &id);
            let mut why = Vec::new();
            let mut strength = 0;
            if source.as_deref().map(|s| s.to_lowercase() == want_src).unwrap_or(false) {
                why.push(format!("{id}.json names source folder {folder}"));
                strength = 2;
            }
            if let Some(w) = &want_name {
                if let Some(n) = names.iter().find(|n| squash(n) == *w) {
                    why.push(format!("display name \"{n}\" = the AC1 ui name"));
                    strength = strength.max(1);
                }
            }
            if strength > 0 {
                out.push(Earlier { id, path: p, why, strength });
            }
        }
    } else {
        let root = mods.join("content").join("tracks");
        for e in std::fs::read_dir(&root).into_iter().flatten().flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            let id = e.file_name().to_string_lossy().into_owned();
            let mut why = Vec::new();
            let mut strength = 0;
            for j in std::fs::read_dir(p.join("ui")).into_iter().flatten().flatten() {
                let jp = j.path();
                if !jp.extension().map(|x| x == "json").unwrap_or(false) {
                    continue;
                }
                let Ok(t) = std::fs::read_to_string(&jp) else { continue };
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
                    if v["aceconv_source"].as_str().map(|s| s.to_lowercase() == want_src).unwrap_or(false) {
                        why.push(format!("{} names source folder {folder}", jp.file_name().unwrap().to_string_lossy()));
                        strength = 2;
                    }
                }
                if let (Some(w), Some(n)) = (&want_name, ui_name(&t)) {
                    if squash(&n) == *w {
                        why.push(format!("ui name \"{n}\" = the AC1 ui name"));
                        strength = strength.max(1);
                    }
                }
            }
            if strength > 0 {
                out.push(Earlier { id, path: p, why, strength });
            }
        }
    }
    out.sort_by(|a, b| b.strength.cmp(&a.strength).then(a.id.cmp(&b.id)));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_valid_and_stable() {
        let none = HashSet::new();
        for f in ["ks_mazda_miata", "Honda S2000 (A-Spec)", "rss_formula_hybrid_2023_extremely_long_name", "1000km", "--", "ks_toyota_ae86"] {
            let c = car_id(f, &none);
            assert!(valid("cars", &c), "{c}");
            assert_eq!(c, car_id(f, &none));
            let t = track_id(f, &none);
            assert!(valid("tracks", &t), "{t}");
            assert!(t.len() <= 14);
        }
        assert_ne!(car_id("abc_1", &none), car_id("abc-1", &none));
        assert!(car_id("ks_mazda_miata", &none).starts_with("ks_mazda_miata_"));
    }

    #[test]
    fn clash_grows_suffix() {
        let first = car_id("x", &HashSet::new());
        let taken: HashSet<String> = [first.clone()].into();
        let second = car_id("x", &taken);
        assert_ne!(first, second);
        assert_eq!(second.len(), first.len() + 1);
        assert!(valid("cars", &second));
    }

    #[test]
    fn ui_names() {
        assert_eq!(ui_name("{\n\t\"name\": \"Mazda MX-5 NA\",\n}").as_deref(), Some("Mazda MX-5 NA"));
        assert_eq!(squash("Mazda MX-5 NA"), "mazdamx5na");
    }
}
