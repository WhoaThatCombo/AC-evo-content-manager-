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
    // the licence the version resource points at ("LICENSE.txt in this
    // application")
    let lic = repo.join("LICENSE.txt");
    println!("cargo:rerun-if-changed={}", lic.display());
    if lic.is_file() {
        files.push(("LICENSE.txt".into(), lic));
    } else {
        println!("cargo:warning=bundle: LICENSE.txt is missing");
    }
    for name in ["cars.json", "events_practice.json", "events_race_weekend.json", "evoview.exe"] {
        let p = repo.join("tools").join(name);
        println!("cargo:rerun-if-changed={}", p.display());
        if p.is_file() {
            files.push((format!("tools/{name}"), p));
        } else {
            println!("cargo:warning=bundle: tools/{name} is missing");
        }
    }
    // deflated: the viewer alone shrinks 8.0 -> 3.0 MB, the UI ~4x
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let mut src = String::from("/// (path, deflated bytes, original size)\npub static FILES: &[(&str, &[u8], usize)] = &[\n");
    for (i, (rel, p)) in files.iter().enumerate() {
        use std::io::Write;
        let raw = std::fs::read(p).unwrap();
        let mut z = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
        z.write_all(&raw).unwrap();
        let blob = out_dir.join(format!("bundle_{i}.deflate"));
        std::fs::write(&blob, z.finish().unwrap()).unwrap();
        src += &format!("    ({rel:?}, include_bytes!({:?}), {}),\n", blob.to_string_lossy(), raw.len());
    }
    src += "];\n";
    std::fs::write(out_dir.join("bundle_files.rs"), src).unwrap();
}

fn main() {
    bundle();
    tauri_build::build()
}
