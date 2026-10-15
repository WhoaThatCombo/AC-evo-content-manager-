//! Dev check: verify the live release's signature with the Rust verifier,
//! reject tampered inputs, and dump a swap script for comparison.
//!     updater_test <out.bat>
use acecm::updater;
use std::path::Path;

fn main() {
    // updater_test swap <dir> <pid> <relaunch 0|1>: a real swap of dir\ACECM.exe
    // with dir\ACECM-update.new once <pid> exits
    let a: Vec<String> = std::env::args().collect();
    if a.get(1).map(|s| s == "swap").unwrap_or(false) {
        let d = Path::new(&a[2]);
        let pid: u32 = a[3].parse().unwrap();
        let relaunch = a.get(4).map(|s| s == "1").unwrap_or(false);
        let (bat, log) = updater::write_swap_script(&d.join("ACECM.exe"), &d.join("ACECM-update.new"), pid,
            Some(d.join("_update.bat")), Some(d.join("_update.log")), relaunch, Some("9.9.9"),
            if relaunch { Some(d.join("ok.flag").to_string_lossy().into_owned()) } else { None }).unwrap();
        updater::run_batch(&bat).unwrap();
        println!("scheduled {} (log {})", bat.display(), log.display());
        return;
    }
    let info = updater::check(true);
    let s = |k: &str| info[k].as_str().unwrap_or("").to_string();
    let (latest, sha, sig) = (s("latest"), s("sha256"), s("sig"));
    println!("latest={latest} sha={} sig_len={}", &sha[..sha.len().min(12)], sig.len());
    println!("genuine:        {:?}", updater::verify_release(updater::ASSET_NAME, &latest, &sha, &sig));
    println!("other version:  {:?}", updater::verify_release(updater::ASSET_NAME, "1.0.20", &sha, &sig));
    println!("other asset:    {:?}", updater::verify_release("ACECM", &latest, &sha, &sig));
    let mut bad = sha.clone().into_bytes();
    bad[0] = if bad[0] == b'0' { b'1' } else { b'0' };
    println!("other sha:      {:?}", updater::verify_release(updater::ASSET_NAME, &latest, &String::from_utf8(bad).unwrap(), &sig));
    println!("garbage sig:    {:?}", updater::verify_release(updater::ASSET_NAME, &latest, &sha, "!!"));
    println!("short sig:      {:?}", updater::verify_release(updater::ASSET_NAME, &latest, &sha, "AAAA"));
    println!("newer: {} {} {}", updater::newer("1.0.10", "1.0.9"), updater::newer("1.0", "1.0.0"), updater::newer("v2", "1.9.9"));
    if let Some(out) = std::env::args().nth(1) {
        let (bat, _) = updater::write_swap_script(Path::new(r"C:\X\ACECM.exe"), Path::new(r"C:\D\ACECM-update.new"), 4242,
            Some(out.into()), Some(r"C:\D\_update.log".into()), true, Some("9.9.9"), Some(r"C:\T\ok.flag".into())).unwrap();
        println!("wrote {}", bat.display());
    }
}
