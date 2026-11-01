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
    // evoview's FSR upscaler, loaded from beside evoview.exe when picked:
    // AMD's signed FidelityFX DLLs, NOT in git (CREDITS.md; AMD's licence
    // ships as tools/licenses/AMD-FidelityFX-LICENSE.md). A build without
    // them only warns, and evoview falls back to TAA. DLSS is not bundled:
    // NVIDIA's terms need their approval first.
    for name in ["amd_fidelityfx_loader_vk.dll", "amd_fidelityfx_upscaler_vk.dll", "licenses/AMD-FidelityFX-LICENSE.md"] {
        let p = repo.join("tools").join(name);
        println!("cargo:rerun-if-changed={}", p.display());
        if p.is_file() {
            files.push((format!("tools/{name}"), p));
        } else {
            println!("cargo:warning=bundle: tools/{name} is missing (evoview upscaler)");
        }
    }
    // the AC1 -> EVO converter (converter/, its own crate), built here
    if let Some(p) = converter(&repo) {
        // a run from the source tree (no unpacked bundle) finds it here
        println!("cargo:rustc-env=ACECM_CONVERTER_BUILT={}", p.display());
        files.push(("tools/aceconv.exe".into(), p));
    }
    let rev = repo.join("converter").join("UPSTREAM_REV");
    println!("cargo:rerun-if-changed={}", rev.display());
    let rev = std::fs::read_to_string(&rev).map(|s| s.trim().to_string()).unwrap_or_default();
    println!("cargo:rustc-env=ACECM_CONVERTER_REV={rev}");
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

/// The converter exe to bundle. ACECONV_EXE names a ready one; otherwise
/// converter/ is built with its own cargo run, its own target folder (beside
/// this build's) and its own .cargo/config.toml (static C runtime), so the
/// installed app needs no Visual C++ runtime for it. ACECM_NO_CONVERTER=1
/// skips it (the Convert page then says the converter is missing).
fn converter(repo: &Path) -> Option<PathBuf> {
    println!("cargo:rerun-if-env-changed=ACECONV_EXE");
    println!("cargo:rerun-if-env-changed=ACECM_NO_CONVERTER");
    if let Ok(p) = std::env::var("ACECONV_EXE") {
        let p = PathBuf::from(p);
        if p.is_file() {
            println!("cargo:rerun-if-changed={}", p.display());
            return Some(p);
        }
        println!("cargo:warning=bundle: ACECONV_EXE {} is not a file", p.display());
        return None;
    }
    if std::env::var("ACECM_NO_CONVERTER").map(|v| v == "1").unwrap_or(false) {
        println!("cargo:warning=bundle: ACECM_NO_CONVERTER=1 - built without the converter");
        return None;
    }
    let dir = repo.join("converter");
    if !dir.join("Cargo.toml").is_file() {
        println!("cargo:warning=bundle: converter/ is missing - built without the converter");
        return None;
    }
    for p in ["src", "Cargo.toml", "Cargo.lock", ".cargo", "data"] {
        println!("cargo:rerun-if-changed={}", dir.join(p).display());
    }
    // OUT_DIR = <target>/<profile>/build/<pkg>/out
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let target = out.ancestors().nth(4).map(|t| t.join("converter")).unwrap_or_else(|| out.join("converter"));
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut cmd = std::process::Command::new(cargo);
    cmd.args(["build", "--release", "--locked", "--bin", "aceconv"]).current_dir(&dir);
    // ⚠ this build script's own cargo environment must not leak in: the
    // encoded RUSTFLAGS would replace the converter's static-CRT flags, and
    // the per-package variables describe ACECM, not the converter
    for (k, _) in std::env::vars() {
        let drop = k == "CARGO_ENCODED_RUSTFLAGS" || k == "RUSTFLAGS" || k == "CARGO_TARGET_DIR" || k == "CARGO_MANIFEST_DIR"
            || k == "CARGO_MANIFEST_PATH" || k == "CARGO_MANIFEST_LINKS" || k.starts_with("CARGO_PKG_") || k.starts_with("CARGO_CFG_")
            || k.starts_with("CARGO_FEATURE_") || k.starts_with("DEP_")
            || matches!(k.as_str(), "OUT_DIR" | "TARGET" | "HOST" | "PROFILE" | "OPT_LEVEL" | "DEBUG" | "RUSTC" | "RUSTDOC" | "RUSTC_LINKER" | "CARGO_PRIMARY_PACKAGE");
        if drop {
            cmd.env_remove(&k);
        }
    }
    // after the clean-up above, which drops this build's own target dir
    cmd.env("CARGO_TARGET_DIR", &target);
    match cmd.status() {
        Ok(s) if s.success() => {}
        Ok(s) => panic!("building the converter (converter/) failed: {s}"),
        Err(e) => panic!("could not run cargo for the converter: {e}"),
    }
    let exe = target.join("release").join(if cfg!(windows) { "aceconv.exe" } else { "aceconv" });
    if !exe.is_file() {
        panic!("the converter build made no {}", exe.display());
    }
    Some(exe)
}

fn main() {
    bundle();
    tauri_build::build()
}
