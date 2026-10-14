//! Dev tool: write_archive / write_inplace with a fixed change set, for a
//! byte comparison against kspkg_write.py.
//!     kspkg_write_test copy <src> <dst> <replace-path>
//!     kspkg_write_test inplace <archive> <replace-path>
fn changes(replace: &str) -> Vec<(String, Vec<u8>)> {
    vec![
        (r"content\acecm_test\hello.json".to_string(), br#"{"hello": "from acecm"}"#.to_vec()),
        (replace.to_string(), b"replaced by the write test".to_vec()),
        (r"content\acecm_test\zz_last.bin".to_string(), (0..=255u8).collect()),
    ]
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let r = match a[1].as_str() {
        "copy" => acecm::kspkg_write::write_archive(a[2].as_ref(), a[3].as_ref(), &changes(&a[4])),
        _ => acecm::kspkg_write::write_inplace(a[2].as_ref(), &changes(&a[3])),
    };
    println!("{r:?}");
    let target = if a[1] == "copy" { &a[3] } else { &a[2] };
    println!("{}", acecm::kspkg_write::verify(target.as_ref()));
}
