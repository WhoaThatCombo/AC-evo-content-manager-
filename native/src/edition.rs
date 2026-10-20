//! Which edition this build is.
//!
//! The normal build is ACECM. Built with `--features dev-edition` it is
//! "ACECM Dev": the same app installed and run BESIDE the normal one without
//! touching it - its own install folder, data folder, port and shortcuts, and
//! no self-updater (an update would replace it with the release build). It is
//! how a release candidate is tried on a real machine before it ships.

#[cfg(not(feature = "dev-edition"))]
mod ids {
    pub const APP: &str = "ACECM";
    pub const TITLE: &str = "ACECM";
    pub const ROOT: &str = "ACECM";
    pub const PORT: u16 = 8092;
}

#[cfg(feature = "dev-edition")]
mod ids {
    pub const APP: &str = "ACECM-Dev";
    pub const TITLE: &str = "ACECM Dev";
    pub const ROOT: &str = "ACECM-Dev";
    pub const PORT: u16 = 8292;
}

/// install folder + exe + shortcut name
pub const APP: &str = ids::APP;
/// window / message box title
pub const TITLE: &str = ids::TITLE;
/// %LOCALAPPDATA%\<ROOT>: config.json, data\, logs
pub const ROOT: &str = ids::ROOT;
/// the UI port
pub const PORT: u16 = ids::PORT;
/// the normal ACECM's data folder - a dev build's first-run settings are
/// seeded from it
pub const MAIN_ROOT: &str = "ACECM";

/// Is this the side-by-side development build?
pub const fn is_dev() -> bool {
    cfg!(feature = "dev-edition")
}

/// Dev build, first run: start from the normal ACECM's settings (game paths,
/// mods folders, theme) with our own port and no update source.
pub fn seed_config() {
    if !is_dev() {
        return;
    }
    let root = crate::paths::root();
    let cfg = root.join("config.json");
    if cfg.exists() || std::env::var("ACECM_ROOT").is_ok() {
        return;
    }
    let main = std::env::var("LOCALAPPDATA").map(std::path::PathBuf::from).unwrap_or_default().join(MAIN_ROOT).join("config.json");
    let mut v: serde_json::Map<String, serde_json::Value> = std::fs::read_to_string(&main).ok()
        .and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    for k in ["update_repo", "update_url", "update_token", "remote_admin", "admin_token"] {
        v.remove(k);
    }
    v.insert("ui_port".into(), serde_json::json!(PORT));
    let _ = std::fs::create_dir_all(&root);
    let _ = std::fs::write(&cfg, serde_json::to_string_pretty(&serde_json::Value::Object(v)).unwrap_or_default());
}
