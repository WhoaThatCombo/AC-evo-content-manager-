//! Histogram of the stock `.texture` header fields per texture format, for
//! the header writer (src/io/texture.rs `header`). Over every `.texture` of
//! content.kspkg that has a page table (root f12), grouped by root f4
//! (format enum): for each root field and each source (f10) / page-table
//! (f12) sub-field, how many headers carry it and its most common values.
//! Per-texture values (sizes, mip count, page lists, the source path's text)
//! are summarised by kind instead of value.
//!
//! cargo run --release --example texhdr -- <content.kspkg> > tools/texhdr_stock.tsv

use aceconv::io::kspkg::Pkg;
use aceconv::io::pb::{Msg, Val};
use std::collections::BTreeMap;

fn val(v: &Val) -> String {
    match v {
        Val::Varint(x) => format!("v{x}"),
        Val::F32(b) => format!("f{}", f32::from_le_bytes(*b)),
        Val::F64(b) => format!("d{}", f64::from_le_bytes(*b)),
        Val::Bytes(b) => {
            // packed varint lists: summarise as the set of distinct values
            let vs = aceconv::io::pb::varints(b);
            let mut u: Vec<u64> = vs.clone();
            u.sort();
            u.dedup();
            if u.len() <= 2 && !vs.is_empty() {
                format!("list of {:?}", u)
            } else {
                "bytes".into()
            }
        }
        Val::Msg(_) => "msg".into(),
    }
}

/// The kind of a source path (f10.f1): drive letter, separators, extension.
fn path_kind(s: &str) -> String {
    let drive = s.len() > 2 && s.as_bytes()[1] == b':';
    let ext = s.rsplit('.').next().filter(|e| e.len() <= 4 && !e.contains('\\') && !e.contains('/')).unwrap_or("-").to_lowercase();
    let sep = if s.contains('\\') { "\\" } else if s.contains('/') { "/" } else { "none" };
    format!("path drive={drive} sep={sep} ext={ext}")
}

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let pkg = Pkg::open(std::path::Path::new(&a[1]))?;
    // format -> (count, field -> value -> n)
    let mut h: BTreeMap<u64, (usize, BTreeMap<String, BTreeMap<String, usize>>)> = BTreeMap::new();
    let mut examples: BTreeMap<String, String> = BTreeMap::new();
    for e in pkg.entries.iter().filter(|e| e.path.to_lowercase().ends_with(".texture")) {
        let Some(m) = pkg.read(e).ok().and_then(|b| Msg::parse(&b).ok()) else { continue };
        let Some(pt) = m.sub(12) else { continue };
        let fmt = m.varint(4).unwrap_or(u64::MAX);
        let ent = h.entry(fmt).or_default();
        ent.0 += 1;
        let mut add = |k: String, v: String| *ent.1.entry(k).or_default().entry(v).or_default() += 1;
        for f in &m.fields {
            let v = match f.num {
                1 | 2 | 3 => "per texture".into(),
                10 | 11 | 12 => "msg".into(),
                _ => val(&f.val),
            };
            add(format!("f{}", f.num), v);
        }
        if let Some(src) = m.sub(10) {
            for f in &src.fields {
                let v = match f.num {
                    1 => {
                        let s = src.str(1).unwrap_or_default();
                        let k = path_kind(&s);
                        examples.entry(k.clone()).or_insert(s);
                        k
                    }
                    2 | 3 => "per texture".into(),
                    _ => val(&f.val),
                };
                add(format!("f10.f{}", f.num), v);
            }
        }
        for f in &pt.fields {
            let v = match f.num {
                4 | 5 => "per texture".into(),
                _ => val(&f.val),
            };
            add(format!("f12.f{}", f.num), v);
        }
    }
    println!("format\ttextures\tfield\theaders_with_it\tvalue\tcount");
    for (fmt, (n, fields)) in &h {
        for (k, vals) in fields {
            let with: usize = vals.values().sum();
            let mut vs: Vec<(&String, &usize)> = vals.iter().collect();
            vs.sort_by(|a, b| b.1.cmp(a.1));
            for (v, c) in vs.iter().take(6) {
                println!("{fmt}\t{n}\t{k}\t{with}\t{v}\t{c}");
            }
        }
    }
    println!();
    for (k, s) in &examples {
        println!("# example {k}: {s}");
    }
    Ok(())
}
