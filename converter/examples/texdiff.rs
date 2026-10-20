//! Compare the textures of two converted packages: every `.texturemips`
//! must be byte-identical, and each `.texture` header may differ only in the
//! listed fields. Prints the differing header field paths with counts.
//!
//! cargo run --release --example texdiff -- <a.kspkg> <b.kspkg>

use aceconv::io::pb::{Msg, Val};
use std::collections::BTreeMap;

fn flat(m: &Msg, pre: &str, out: &mut BTreeMap<String, String>) {
    let mut n: BTreeMap<u32, usize> = BTreeMap::new();
    for f in &m.fields {
        let k = n.entry(f.num).or_default();
        let p = format!("{pre}f{}[{k}]", f.num);
        *k += 1;
        match &f.val {
            Val::Bytes(b) => match Msg::parse(b) {
                Ok(s) if !b.iter().all(|c| (0x20..0x7f).contains(c)) && !s.fields.is_empty() => flat(&s, &format!("{p}."), out),
                _ => {
                    out.insert(p, format!("{b:?}"));
                }
            },
            Val::Msg(s) => flat(s, &format!("{p}."), out),
            v => {
                out.insert(p, format!("{v:?}"));
            }
        }
    }
}

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let read = |p: &str| -> anyhow::Result<BTreeMap<String, Vec<u8>>> {
        Ok(aceconv::io::kspkg::read(std::path::Path::new(p), "")?.into_iter().filter(|(k, _)| k.to_lowercase().contains(".texture")).map(|(k, v)| (k.to_lowercase(), v)).collect())
    };
    let (x, y) = (read(&a[1])?, read(&a[2])?);
    let (mut mips_same, mut mips_diff, mut hdr) = (0, 0, 0);
    let mut fields: BTreeMap<String, usize> = BTreeMap::new();
    for (k, va) in &x {
        let Some(vb) = y.get(k) else { println!("only in a: {k}"); continue };
        if k.ends_with("mips") {
            if va == vb { mips_same += 1 } else { mips_diff += 1; println!("mips differ: {k}") }
            continue;
        }
        hdr += 1;
        let (mut fa, mut fb) = (BTreeMap::new(), BTreeMap::new());
        flat(&Msg::parse(va)?, "", &mut fa);
        flat(&Msg::parse(vb)?, "", &mut fb);
        for p in fa.keys().chain(fb.keys()) {
            if fa.get(p) != fb.get(p) {
                *fields.entry(p.split('[').next().unwrap().to_string() + "..." + p.rsplit('.').next().unwrap()).or_default() += 1;
            }
        }
    }
    for k in y.keys().filter(|k| !x.contains_key(*k)) {
        println!("only in b: {k}");
    }
    println!("texturemips: {mips_same} identical, {mips_diff} different; {hdr} headers compared");
    for (f, n) in fields {
        println!("  header field differs: {f} ({n} entries)");
    }
    Ok(())
}
