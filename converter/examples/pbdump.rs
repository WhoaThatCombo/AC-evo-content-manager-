//! Print the field tree of stock files (schema-less): every path of
//! content.kspkg containing `filter`, one indented line per field. A
//! length-delimited value is shown as a string when it is printable text,
//! else as a sub-message when it parses as one, else as raw bytes. Runs of
//! more than `max` repeats of one field are elided.
//!
//! cargo run --release --example pbdump -- <content.kspkg> <path filter> [max repeats]

use aceconv::io::kspkg::Pkg;
use aceconv::io::pb::{Msg, Val};

fn printable(b: &[u8]) -> bool {
    !b.is_empty() && b.iter().all(|c| (0x20..0x7f).contains(c))
}

fn dump(m: &Msg, depth: usize, max: usize) {
    let pad = "  ".repeat(depth);
    let mut run: (u32, usize) = (0, 0);
    for f in &m.fields {
        if f.num == run.0 {
            run.1 += 1;
        } else {
            run = (f.num, 1);
        }
        if run.1 == max + 1 {
            let n = m.fields.iter().filter(|x| x.num == f.num).count();
            println!("{pad}f{} ... ({n} in all)", f.num);
        }
        if run.1 > max {
            continue;
        }
        match &f.val {
            Val::Varint(v) => println!("{pad}f{} = {v}", f.num),
            Val::F32(b) => println!("{pad}f{} = {}f", f.num, f32::from_le_bytes(*b)),
            Val::F64(b) => println!("{pad}f{} = {}d", f.num, f64::from_le_bytes(*b)),
            Val::Msg(s) => {
                println!("{pad}f{} {{", f.num);
                dump(s, depth + 1, max);
            }
            Val::Bytes(b) => {
                if printable(b) {
                    println!("{pad}f{} = \"{}\"", f.num, String::from_utf8_lossy(b));
                } else if let Ok(s) = Msg::parse(b) {
                    println!("{pad}f{} {{ ({} bytes)", f.num, b.len());
                    dump(&s, depth + 1, max);
                } else {
                    println!("{pad}f{} = <{} bytes>", f.num, b.len());
                }
            }
        }
    }
}

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let pkg = Pkg::open(std::path::Path::new(&a[1]))?;
    let filt = a[2].to_lowercase();
    let max: usize = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(3);
    for e in pkg.entries.iter().filter(|e| e.path.to_lowercase().contains(&filt)) {
        println!("== {}", e.path);
        match pkg.read(e).ok().and_then(|b| Msg::parse(&b).ok()) {
            Some(m) => dump(&m, 1, max),
            None => println!("  (not protobuf)"),
        }
    }
    Ok(())
}
