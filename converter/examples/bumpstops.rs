//! Every stock coilover's bump stops (CoiloverData 3 bumpStopUp / 4
//! bumpStopDown, BumpStopData {1 Range, 2 Reference, 3 Force, 4 Gamma,
//! 5 length, 6 damping}): one TSV row per stop, then per field and side how
//! many stops carry it and its median / min / max over those that do.
//! Read from the `.coilover` files CarData names (SuspensionData 20 / 21).
//!
//! cargo run --release --example bumpstops -- <content.kspkg> > tools/bumpstops_stock.tsv

use aceconv::car::physics::pbx::rf;
use aceconv::car::physics::stats::cardata_of;
use aceconv::io::kspkg::Pkg;
use aceconv::io::pb::Msg;
use std::collections::{BTreeMap, HashMap};

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let pkg = Pkg::open(std::path::Path::new(&a[1]))?;
    let idx: HashMap<String, usize> = pkg.entries.iter().enumerate().map(|(i, e)| (e.path.to_lowercase(), i)).collect();
    let get = |p: &str| idx.get(&p.to_lowercase()).and_then(|&i| pkg.read(&pkg.entries[i]).ok());
    let mut cars: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for e in &pkg.entries {
        let l = e.path.to_lowercase();
        if let Some((id, _)) = l.strip_prefix("content\\cars\\").and_then(|r| r.split_once('\\')) {
            cars.entry(id.to_string()).or_default().push(e.path.clone());
        }
    }
    // (side, field) -> values over the stops that carry it; side -> stop count
    let mut vals: BTreeMap<(&str, u32), Vec<f64>> = BTreeMap::new();
    let mut stops: BTreeMap<&str, usize> = BTreeMap::new();
    println!("id\taxle\tside\tf1_range\tf2_reference\tf3_force\tf4_gamma\tf5_length\tf6_damping");
    for (id, paths) in &cars {
        if !paths.iter().any(|p| p.to_lowercase() == format!("content\\cars\\{id}\\{id}.actor")) {
            continue;
        }
        let Some(cd) = cardata_of(paths.iter().map(|s| s.as_str())).and_then(|p| get(&p)).and_then(|b| Msg::parse(&b).ok()) else { continue };
        let s = cd.sub(2).unwrap_or_default();
        for (axle, file, inline) in [("front", 20u32, 8u32), ("rear", 21, 9)] {
            let co = s.str(file).and_then(|p| get(&p)).and_then(|b| Msg::parse(&b).ok()).or_else(|| s.sub(inline)).unwrap_or_default();
            for (side, num) in [("up", 3u32), ("down", 4)] {
                let Some(b) = co.sub(num) else { continue };
                *stops.entry(side).or_default() += 1;
                let mut row = vec![id.clone(), axle.into(), side.into()];
                for k in 1..=6u32 {
                    match rf(&b, k) {
                        Some(v) => {
                            vals.entry((side, k)).or_default().push(v);
                            row.push(format!("{v}"));
                        }
                        None => row.push("-".into()),
                    }
                }
                println!("{}", row.join("\t"));
            }
        }
    }
    println!();
    println!("# side\tfield\tstops_with_it\tstops\tmedian\tmin\tmax");
    for ((side, k), mut v) in vals {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let med = if v.len() % 2 == 1 { v[v.len() / 2] } else { (v[v.len() / 2 - 1] + v[v.len() / 2]) / 2.0 };
        println!("# {side}\tf{k}\t{}\t{}\t{med}\t{}\t{}", v.len(), stops[side], v[0], v[v.len() - 1]);
    }
    Ok(())
}
