//! Dev: write_inplace + undo_inplace on a COPY must give back identical bytes.
//!     undo_test <archive copy>
use md5::Digest;

fn md5(p: &std::path::Path) -> String {
    let mut h = md5::Md5::new();
    let mut f = std::fs::File::open(p).unwrap();
    std::io::copy(&mut f, &mut h).unwrap();
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn main() {
    let a = std::path::PathBuf::from(std::env::args().nth(1).expect("archive copy"));
    let before = (md5(&a), std::fs::metadata(&a).unwrap().len());
    let w = acecm::kspkg_write::write_inplace(&a, &[("system\\zz_undo_test.bin".into(), vec![1, 2, 3, 4, 5])]).unwrap();
    let mid = std::fs::metadata(&a).unwrap().len();
    println!("written: added={} size {} -> {}", w["added"], before.1, mid);
    acecm::kspkg_write::undo_inplace(&a, &w).unwrap();
    let after = (md5(&a), std::fs::metadata(&a).unwrap().len());
    println!("identical after undo: {}", before == after);
}
