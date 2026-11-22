//! Stock twins' tyres side by side: for each `<evo car id>=<AC1 car dir>`
//! pair, the AC1 tyres.ini model keys of the default compound and the
//! stock EVO default compound's TyresModelData (f4), camber (f7), speed
//! sensitivity (f8) and pressure (f6) fields. With no pairs it lists the
//! stock car ids. Read-only on both installs.
//!
//! cargo run --release --example tyretwins -- <content.kspkg> [evo_id=ac1_dir ...]

use aceconv::io::kspkg::Pkg;
use aceconv::io::pb::{Msg, Val};

fn scalars(m: &Msg) -> String {
    let mut out = Vec::new();
    for f in &m.fields {
        match &f.val {
            Val::Msg(_) | Val::Bytes(_) => {}
            _ => {
                if let Some(v) = aceconv::car::physics::pbx::rf(m, f.num) {
                    out.push(format!("{}={}", f.num, (v * 1e5).round() / 1e5));
                }
            }
        }
    }
    out.join(" ")
}

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let pkg = Pkg::open(std::path::Path::new(&a[1]))?;
    if a.len() < 3 {
        let mut ids: Vec<String> = pkg.entries.iter().filter_map(|e| {
            let l = e.path.to_lowercase();
            let r = l.strip_prefix("content\\cars\\")?;
            r.ends_with("\\data\\cardata.car").then(|| r.split('\\').next().unwrap().to_string())
        }).collect();
        ids.sort();
        ids.dedup();
        println!("{}", ids.join("\n"));
        return Ok(());
    }
    for pair in &a[2..] {
        let (evo, ac1) = pair.split_once('=').unwrap();
        println!("=== {evo} <- {ac1}");
        let car = aceconv::ac1::car::Car::open(std::path::Path::new(ac1))?;
        let t = car.ini("tyres.ini");
        let idx = t.num("COMPOUND_DEFAULT", "INDEX").unwrap_or(0.0) as usize;
        for side in ["FRONT", "REAR"] {
            let sec = if idx == 0 { side.to_string() } else { format!("{side}_{idx}") };
            let keys = ["NAME", "WIDTH", "RADIUS", "DY_REF", "DX_REF", "DY0", "DY1", "DX0", "DX1", "FZ0", "LS_EXPY", "LS_EXPX", "FRICTION_LIMIT_ANGLE", "FLEX", "FLEX_GAIN", "XMU", "FALLOFF_LEVEL", "FALLOFF_SPEED", "CX_MULT", "BRAKE_DX_MOD", "COMBINED_FACTOR", "CAMBER_GAIN", "DCAMBER_0", "DCAMBER_1", "SPEED_SENSITIVITY", "RELAXATION_LENGTH", "PRESSURE_IDEAL", "PRESSURE_D_GAIN", "PRESSURE_FLEX_GAIN", "RATE"];
            let v: Vec<String> = keys.iter().filter_map(|k| t.get(&sec, k).map(|x| format!("{k}={x}"))).collect();
            println!("AC1 [{sec}] {}", v.join(" "));
        }
        let pre = format!("content\\cars\\{evo}\\data\\cardata.car");
        let Some(e) = pkg.find(&pre) else { println!("no {pre}"); continue };
        let cd = Msg::parse(&pkg.read(e)?)?;
        for (num, side) in [(10u32, "front"), (11, "rear")] {
            let Some(p) = cd.str(num) else { continue };
            let Some(te) = pkg.find(&p) else { println!("EVO {side} {p} missing"); continue };
            let m = Msg::parse(&pkg.read(te)?)?;
            println!("EVO {side} {p} name={:?}", m.str(1));
            for (sub, label) in [(3u32, "tyre"), (4, "model"), (6, "pressure"), (7, "camber"), (8, "speed")] {
                if let Some(s) = m.sub(sub) {
                    println!("   f{sub} {label}: {}", scalars(&s));
                }
            }
        }
    }
    Ok(())
}
