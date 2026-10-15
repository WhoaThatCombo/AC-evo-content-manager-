use std::path::{Path, PathBuf};

/// The files a shipped exe carries inside itself (it has no source tree to
/// read them from): the UI, the catalogs the server folder may lack, and the
/// car viewer. Mirrors build.py's stage_web()/harden_tools() data payload.
fn bundle() {
    let repo = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).parent().unwrap().to_path_buf();
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    // the UI: .html / .css / .js only (backups and stray files stay out)
    let web = repo.join("acecm").join("web");
    fn walk(base: &Path, d: &Path, out: &mut Vec<(String, PathBuf)>) {
        let Ok(rd) = std::fs::read_dir(d) else { return };
        let mut ents: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        ents.sort();
        for p in ents {
            if p.is_dir() {
                if p.file_name().map(|n| n != "__pycache__").unwrap_or(false) {
                    walk(base, &p, out);
                }
            } else if matches!(p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(), Some("html" | "css" | "js")) {
                let rel = p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
                out.push((format!("web/{rel}"), p));
            }
        }
    }
    walk(&web, &web, &mut files);
    println!("cargo:rerun-if-changed={}", web.display());
    for name in ["cars.json", "events_practice.json", "events_race_weekend.json", "evoview.exe"] {
        let p = repo.join("tools").join(name);
        println!("cargo:rerun-if-changed={}", p.display());
        if p.is_file() {
            files.push((format!("tools/{name}"), p));
        } else {
            println!("cargo:warning=bundle: tools/{name} is missing");
        }
    }
    let mut src = String::from("pub static FILES: &[(&str, &[u8])] = &[\n");
    for (rel, p) in &files {
        src += &format!("    ({rel:?}, include_bytes!({:?})),\n", p.to_string_lossy());
    }
    src += "];\n";
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("bundle_files.rs");
    std::fs::write(out, src).unwrap();
}

fn main() {
    bundle();
    tauri_build::build()
}
