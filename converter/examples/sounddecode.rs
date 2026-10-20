//! Decode-side checks for the in-house FSB5 decoders (developer tool).
//!   sounddecode fmod <bank>          in-house vs the game's fmod.dll
//!   sounddecode scan <cars dir>      decode every car's bank, report failures
use aceconv::sound::fsb5;
use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};

fn stats(a: &[i16], b: &[i16]) -> (usize, i32, f64) {
    let mut ne = 0;
    let mut md = 0i32;
    let (mut sig, mut err) = (0f64, 0f64);
    for (x, y) in a.iter().zip(b) {
        let d = *x as i32 - *y as i32;
        if d != 0 {
            ne += 1;
        }
        md = md.max(d.abs());
        sig += (*y as f64).powi(2);
        err += (d as f64).powi(2);
    }
    (ne, md, if err == 0.0 { f64::INFINITY } else { 10.0 * (sig / err).log10() })
}

fn i16s(b: &[u8]) -> Vec<i16> {
    b.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect()
}

fn vs_fmod(bank: &Path) -> Result<()> {
    let b = std::fs::read(bank)?;
    let dec = fsb5::Decoder::new()?;
    for base in fsb5::blobs_of(&b)? {
        let info = fsb5::parse_blob(&b, base)?;
        if info.mode == fsb5::MODE_PCM16 {
            continue;
        }
        let len = fsb5::blob_len(&b, base)?;
        let f = dec.decode_fsb(&b[base..base + len])?;
        let t = std::time::Instant::now();
        let ours = fsb5::decode_blob(&b, &info)?;
        let ms = t.elapsed().as_millis();
        let mut same = 0;
        for (k, s) in info.samples.iter().enumerate() {
            let mut r = f[k].0.clone();
            r.resize(s.frames as usize * s.channels as usize * 2, 0);
            let (ne, md, snr) = stats(&i16s(&ours[k]), &i16s(&r));
            if ne == 0 && f[k].1 as u64 == s.frames {
                same += 1;
            } else {
                println!("  sample {k}: ch {} frames {} fmod frames {} neq {ne} maxd {md} snr {snr:.1}", s.channels, s.frames, f[k].1);
            }
        }
        println!("blob @{base:#x} mode {:#x}: {} samples, {same} bit-identical to fmod.dll (in-house {ms} ms)", info.mode, info.num_samples);
    }
    Ok(())
}

fn pick_bank(sfx: &Path) -> Option<PathBuf> {
    let mut c: Vec<(PathBuf, u64)> = Vec::new();
    for e in std::fs::read_dir(sfx).ok()?.flatten() {
        let n = e.file_name().to_string_lossy().to_lowercase();
        if n.ends_with(".bank") && !n.ends_with(".strings.bank") {
            if let Ok(b) = std::fs::read(e.path()) {
                if memchr::memmem::find(&b, b"FSB5").is_some() {
                    c.push((e.path(), b.len() as u64));
                }
            }
        }
    }
    c.sort_by_key(|x| std::cmp::Reverse(x.1));
    let car = sfx.parent()?.file_name()?.to_string_lossy().to_lowercase();
    c.iter().find(|(p, _)| p.file_name().unwrap().to_string_lossy().to_lowercase() == format!("{car}.bank")).or(c.first()).map(|x| x.0.clone())
}

fn scan(cars: &Path) -> Result<()> {
    let (mut ncar, mut nbank, mut ntrans, mut nsamp) = (0, 0, 0, 0usize);
    let mut fails: Vec<String> = Vec::new();
    let t0 = std::time::Instant::now();
    let mut dms = 0u128;
    let mut dirs: Vec<_> = std::fs::read_dir(cars)?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    for d in dirs {
        ncar += 1;
        let Some(bp) = pick_bank(&d.join("sfx")) else { continue };
        nbank += 1;
        let raw = std::fs::read(&bp)?;
        let t = std::time::Instant::now();
        let r = fsb5::blobs_of(&raw).and_then(|blobs| {
            let mut n = 0;
            for base in blobs {
                let info = fsb5::parse_blob(&raw, base)?;
                if info.mode != fsb5::MODE_PCM16 {
                    fsb5::decode_blob(&raw, &info)?;
                    n += info.num_samples;
                }
            }
            Ok(n)
        });
        let ms = t.elapsed().as_millis();
        match r {
            Ok(n) if n > 0 => {
                ntrans += 1;
                nsamp += n;
                dms += ms;
                println!("{}: {n} samples decoded in {ms} ms", d.file_name().unwrap().to_string_lossy());
            }
            Ok(_) => {}
            Err(e) => fails.push(format!("{}: {e:#}", d.file_name().unwrap().to_string_lossy())),
        }
    }
    println!("cars {ncar}, with bank {nbank}, transcoded {ntrans} ({nsamp} samples, decode {dms} ms), failures {}, total {:.1} s", fails.len(), t0.elapsed().as_secs_f64());
    for f in &fails {
        println!("FAIL {f}");
    }
    Ok(())
}

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(|s| s.as_str()) {
        Some("fmod") => vs_fmod(Path::new(&a[2])),
        Some("scan") => scan(Path::new(&a[2])),
        _ => Err(anyhow!("usage: sounddecode fmod|scan ...")),
    }
}
