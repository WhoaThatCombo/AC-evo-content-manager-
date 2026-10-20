//! Where Assetto Corsa EVO is installed: ACECONV_GAME_CONTENT (a content.kspkg
//! path) wins, else every Steam library in libraryfolders.vdf is searched, else
//! the default Steam location. ACECM passes the path it detected; this is for
//! the converter run on its own.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const GAME: &str = "Assetto Corsa EVO";

/// The game folder (the one holding content.kspkg).
pub fn evo_dir() -> PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        if let Some(c) = std::env::var_os("ACECONV_GAME_CONTENT").map(PathBuf::from) {
            if let Some(d) = c.parent() {
                return d.to_path_buf();
            }
        }
        steam_roots().iter().flat_map(|r| libraries(r)).map(|l| l.join("steamapps").join("common").join(GAME))
            .find(|d| d.join("content.kspkg").is_file())
            .unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)\Steam\steamapps\common").join(GAME))
    }).clone()
}

/// The Assetto Corsa (AC1) install: ACECONV_AC1_DIR wins, else every Steam
/// library is searched for steamapps\common\assettocorsa. None when absent.
pub fn ac1_dir() -> Option<PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        if let Some(d) = std::env::var_os("ACECONV_AC1_DIR").map(PathBuf::from) {
            return Some(d);
        }
        steam_roots().iter().flat_map(|r| libraries(r)).map(|l| l.join("steamapps").join("common").join("assettocorsa"))
            .find(|d| d.join("content").join("cars").is_dir())
    }).clone()
}

/// The game's content.kspkg.
pub fn evo_content() -> PathBuf {
    std::env::var_os("ACECONV_GAME_CONTENT").map(PathBuf::from).unwrap_or_else(|| evo_dir().join("content.kspkg"))
}

/// Steam install folders worth trying (no registry access needed).
fn steam_roots() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = Vec::new();
    for var in ["ProgramFiles(x86)", "ProgramFiles"] {
        if let Some(p) = std::env::var_os(var) {
            v.push(PathBuf::from(p).join("Steam"));
        }
    }
    for d in ["C", "D", "E", "F", "G"] {
        v.push(PathBuf::from(format!(r"{d}:\Steam")));
        v.push(PathBuf::from(format!(r"{d}:\Program Files (x86)\Steam")));
    }
    v.dedup();
    v.into_iter().filter(|p| p.join("steamapps").is_dir()).collect()
}

/// The root plus every "path" in steamapps\libraryfolders.vdf.
fn libraries(root: &Path) -> Vec<PathBuf> {
    let mut v = vec![root.to_path_buf()];
    if let Ok(t) = std::fs::read_to_string(root.join("steamapps").join("libraryfolders.vdf")) {
        for line in t.lines() {
            let parts: Vec<&str> = line.split('"').collect();
            // \t\t"path"\t\t"D:\\SteamLibrary"
            if parts.len() >= 4 && parts[1].eq_ignore_ascii_case("path") {
                v.push(PathBuf::from(parts[3].replace(r"\\", r"\")));
            }
        }
    }
    v
}
