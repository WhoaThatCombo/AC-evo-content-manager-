//! Library-wide test run: convert every AC1 car or track, validate the
//! result, record it, delete the output. Each conversion is a child process
//! (a crash cannot end the run), hidden, below normal priority, with its temp
//! folder inside the work dir. Results append to <work>\report_<kind>.jsonl;
//! items already there are skipped, so an interrupted run resumes.

use anyhow::Result;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use crate::validate::{self, Game};

pub struct Opts<'a> {
    pub kind: &'a str, // "cars" | "tracks"
    pub ac1: &'a Path,
    pub work: &'a Path,
    pub donor: &'a Path,
    pub refmats: &'a Path,
    pub game_content: &'a Path,
    pub limit: usize,
    pub only: Option<String>,
    pub threads: usize,
}

fn candidates(kind: &str, ac1: &Path) -> Vec<String> {
    let dir = ac1.join("content").join(if kind == "cars" { "cars" } else { "tracks" });
    let mut v: Vec<String> = std::fs::read_dir(&dir).map(|r| r.flatten().filter(|e| e.path().is_dir()).filter(|e| {
        let p = e.path();
        if kind == "cars" {
            (p.join("data.acd").is_file() || p.join("data").is_dir()) && std::fs::read_dir(&p).map(|r| r.flatten().any(|f| f.path().extension().map(|x| x.eq_ignore_ascii_case("kn5")).unwrap_or(false))).unwrap_or(false)
        } else {
            p.join("data").join("surfaces.ini").is_file() || std::fs::read_dir(&p).map(|r| r.flatten().any(|f| f.path().join("data").join("surfaces.ini").is_file())).unwrap_or(false)
        }
    }).map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    v.sort_by_key(|x| x.to_lowercase());
    v
}

fn done(report: &Path) -> std::collections::HashSet<String> {
    std::fs::read_to_string(report).unwrap_or_default().lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|v| v["folder"].as_str().map(String::from)).collect()
}

pub fn run(o: &Opts) -> Result<()> {
    std::fs::create_dir_all(o.work)?;
    let tmp = o.work.join("tmp");
    let out_root = o.work.join("out");
    let logs = o.work.join("logs");
    for d in [&tmp, &out_root, &logs] {
        std::fs::create_dir_all(d)?;
    }
    let report = o.work.join(format!("report_{}.jsonl", o.kind));
    let have = done(&report);
    let game = Game::open(o.game_content)?;
    // stock ids stay out of reach of converted ones
    let taken = game.ids(o.kind);
    let me = std::env::current_exe()?;
    let mut list = candidates(o.kind, o.ac1);
    if let Some(f) = &o.only {
        let want: Vec<String> = f.split(',').map(|x| x.trim().to_lowercase()).collect();
        list.retain(|x| want.contains(&x.to_lowercase()));
    }
    let todo: Vec<String> = list.into_iter().filter(|x| !have.contains(x)).take(if o.limit > 0 { o.limit } else { usize::MAX }).collect();
    println!("{} {}: {} to do ({} already in {})", todo.len(), o.kind, todo.len(), have.len(), report.display());
    let mut rep = std::fs::OpenOptions::new().create(true).append(true).open(&report)?;
    let (mut ok, mut bad) = (0, 0);
    for (n, folder) in todo.iter().enumerate() {
        let id = crate::ids::id_for(o.kind, folder, &taken);
        let src = o.ac1.join("content").join(if o.kind == "cars" { "cars" } else { "tracks" }).join(folder);
        let out = out_root.join(&id);
        let _ = std::fs::remove_dir_all(&out);
        std::fs::create_dir_all(&out)?;
        let mut args: Vec<String> = vec![if o.kind == "cars" { "car" } else { "track" }.into(), src.to_string_lossy().into_owned()];
        if o.kind == "cars" {
            // aceconv car <src> <out> --id <id> --donor <zip> --refmats <dir>
            args.extend([out.to_string_lossy().into_owned(), "--id".into(), id.clone(), "--donor".into(), o.donor.to_string_lossy().into_owned(),
                "--refmats".into(), o.refmats.to_string_lossy().into_owned(), "--content".into(), o.game_content.to_string_lossy().into_owned()]);
        } else {
            args.extend([id.clone(), out.to_string_lossy().into_owned()]);
        }
        let t = Instant::now();
        let mut cmd = Command::new(&me);
        cmd.args(&args).current_dir(&out).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped())
            .env("TEMP", &tmp).env("TMP", &tmp).env("ACECONV_THREADS", o.threads.to_string()).env("ACECONV_GAME_CONTENT", o.game_content)
            .env("ACECONV_TRACK_INSTALL_ROOT", o.work.join("install"))
            // one cache for the whole run (each item's out dir goes after it)
            .env("ACECONV_TEXCACHE", o.work.join("texcache"));
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000 | 0x0000_4000);
        }
        let output = cmd.output();
        let secs = t.elapsed().as_secs_f64();
        let (code, text) = match &output {
            Ok(x) => (x.status.code().unwrap_or(-1), format!("{}{}", String::from_utf8_lossy(&x.stdout), String::from_utf8_lossy(&x.stderr))),
            Err(e) => (-1, format!("spawn failed: {e}")),
        };
        // validate what was produced
        let r = if code != 0 {
            None
        } else if o.kind == "cars" {
            validate::car(&out.join(format!("{id}.kspkg")), &game).ok()
        } else {
            validate::track(&out.join(&id), &id, &game).ok()
        };
        let status = match (&r, code) {
            (_, c) if c != 0 => "convert_failed",
            (None, _) => "validate_failed",
            (Some(r), _) if !r.ok() => "invalid",
            _ => "ok",
        };
        if status == "ok" { ok += 1 } else { bad += 1 }
        let notes: Vec<String> = text.lines().filter(|l| l.contains("skipped") || l.contains("note:") || l.contains("Error") || l.contains("error") || l.contains("panicked")).take(12).map(|s| s.trim().to_string()).collect();
        let line = serde_json::json!({
            "folder": folder, "id": id, "status": status, "code": code, "secs": (secs * 10.0).round() / 10.0,
            "errors": r.as_ref().map(|r| r.errors.clone()).unwrap_or_default(),
            "warnings": r.as_ref().map(|r| r.warnings.iter().take(10).cloned().collect::<Vec<_>>()).unwrap_or_default(),
            "files": r.as_ref().map(|r| r.files).unwrap_or(0), "notes": notes,
            "tail": if code != 0 { text.lines().rev().take(8).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n") } else { String::new() },
        });
        writeln!(rep, "{line}")?;
        rep.flush()?;
        if status != "ok" {
            let _ = std::fs::write(logs.join(format!("{folder}.log")), &text);
        }
        println!("[{}/{}] {folder} -> {id}: {status} in {:.1} s{}", n + 1, todo.len(), secs,
            r.as_ref().map(|r| format!(" ({} errors, {} warnings)", r.errors.len(), r.warnings.len())).unwrap_or_default());
        // outputs are big: keep nothing but the report and failure logs
        let _ = std::fs::remove_dir_all(&out);
        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::create_dir_all(&tmp);
    }
    println!("done: {ok} ok, {bad} not ok -> {}", report.display());
    Ok(())
}

/// (donor zip, refmats dir). Our own donor, built from the game's
/// content.kspkg into the donor cache (`donor::ensure`: first use, and
/// again after a game update). ACECONV_DONOR=<zip> / ACECONV_REFMATS=<dir>
/// override either one explicitly.
pub fn donor_paths() -> Result<(PathBuf, PathBuf)> {
    let donor = std::env::var_os("ACECONV_DONOR").map(PathBuf::from);
    let refmats = std::env::var_os("ACECONV_REFMATS").map(PathBuf::from);
    if let (Some(d), Some(r)) = (&donor, &refmats) {
        return Ok((d.clone(), r.clone()));
    }
    let (d, r) = crate::donor::ensure(&crate::io::gamedir::evo_content())?;
    Ok((donor.unwrap_or(d), refmats.unwrap_or(r)))
}

/// `donor_paths`, reporting a failure and handing back the cache paths
/// (the conversion then stops on the missing donor with a clear error).
pub fn default_paths() -> (PathBuf, PathBuf) {
    match donor_paths() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("donor: {e:#}");
            let dir = crate::donor::cache_dir().unwrap_or_default();
            (dir.join(crate::donor::DONOR_ZIP), dir.join("refmats"))
        }
    }
}
