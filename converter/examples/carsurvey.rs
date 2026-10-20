//! Survey the stock cars of content.kspkg: per actor base-mesh slot the batch
//! names and their material shaders, the actor mirrors, and every batch of
//! the body / interior meshes with its shader. Used to calibrate the
//! validator's batch checks (src/validate/batches.rs).
//!
//! cargo run --release --example carsurvey -- <content.kspkg> [car id filter]

use aceconv::io::kspkg::Pkg;
use aceconv::io::pb::Msg;
use std::collections::HashMap;

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let pkg = Pkg::open(std::path::Path::new(&a[1]))?;
    let filt = a.get(2).cloned().unwrap_or_default();
    let idx: HashMap<String, usize> = pkg.entries.iter().enumerate().map(|(i, e)| (e.path.to_lowercase(), i)).collect();
    let get = |p: &str| idx.get(&p.to_lowercase()).and_then(|&i| pkg.read(&pkg.entries[i]).ok());
    let mut shader_cache: HashMap<String, String> = HashMap::new();
    let mut shader = |p: &str| -> String {
        if let Some(s) = shader_cache.get(p) {
            return s.clone();
        }
        let s = get(p).and_then(|b| Msg::parse(&b).ok()).and_then(|m| m.str(1)).unwrap_or_else(|| "?MISSING".into());
        shader_cache.insert(p.to_string(), s.clone());
        s
    };
    let mut ids: Vec<String> = pkg.entries.iter().filter_map(|e| {
        let l = e.path.to_lowercase();
        let r = l.strip_prefix("content\\cars\\")?;
        let (id, file) = r.split_once('\\')?;
        (file == format!("{id}.actor")).then(|| id.to_string())
    }).collect();
    ids.sort();
    ids.dedup();
    let slots: [(u32, &str); 15] = [(1, "body"), (2, "tyre_lf"), (3, "tyre_rf"), (4, "tyre_lr"), (5, "tyre_rr"), (6, "rim_lf"), (7, "rim_rf"), (8, "rim_lr"), (9, "rim_rr"), (18, "blur_lf"), (19, "blur_rf"), (20, "blur_lr"), (21, "blur_rr"), (10, "interior"), (11, "steer")];
    for id in ids.iter().filter(|i| i.contains(&filt)) {
        let Some(ab) = get(&format!("content\\cars\\{id}\\{id}.actor")) else { continue };
        let Ok(act) = Msg::parse(&ab) else { continue };
        let Some(car) = act.sub(103) else { println!("{id}\tNO_CAR_DATA"); continue };
        println!("CAR {id}");
        let bm = car.sub(12).unwrap_or_default();
        for (f, name) in slots {
            let path = if f == 1 { bm.str(1) } else { bm.sub(f).and_then(|s| s.str(1)) };
            let Some(path) = path else { println!("  slot {name}: -"); continue };
            let Some(mb) = get(&path) else { println!("  slot {name}: {path} MISSING"); continue };
            let Ok(top) = Msg::parse(&mb) else { continue };
            let lods = top.subs(5);
            let mut per: Vec<String> = Vec::new();
            for (li, l) in lods.iter().enumerate() {
                let b: Vec<String> = l.subs(4).iter().map(|s| {
                    let mat = s.str(4).unwrap_or_default();
                    format!("{}={}", s.str(1).unwrap_or_default(), shader(&mat))
                }).collect();
                if name == "body" || name == "interior" {
                    if li == 0 {
                        for x in &b {
                            println!("  {name}\t{x}");
                        }
                    }
                } else {
                    per.push(b.join(","));
                }
            }
            if !per.is_empty() {
                per.dedup();
                println!("  slot {name}\t{}\t{}", path.rsplit('\\').next().unwrap_or(""), per.join(" | "));
            }
        }
        for m in car.subs(23) {
            println!("  mirror\ttype={}\tbatch={}\tbone={}", m.varint(1).unwrap_or(0), m.str(2).unwrap_or_default(), m.str(3).unwrap_or_default());
        }
    }
    Ok(())
}
