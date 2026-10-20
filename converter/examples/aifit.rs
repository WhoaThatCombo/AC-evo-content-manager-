//! The stock cars' AI hint inputs and outputs, one TSV row per car, for the
//! AI hint fit (tools/aifit.py) and the auto-shift ratio (docs/FORMATS.md,
//! "Car physics").
//!
//! Inputs, from CarData and the files it names (src/car/physics/stats.rs
//! `derive`, the same model the converter and the checker use):
//! mass with the default fuel, CG front share, each axle's tyre (first
//! compound: D_y, D_x, load exponents, Fz0, radius) and its static-load
//! mu_y / mu_x, the wings' summed CL x gain x chord x span at their angle
//! (CL.A), whether the car also has CZ aero maps (AeroData 7 / 8), the brake torque and front bias, the engine limiter and the gearbox auto-shift
//! rpm (GearBoxData.autoshifter {1 up, 5 down_rpm_threshold}).
//!
//! Outputs, from the shipped `.aicardata` (AiCarData): gears {1 upshift,
//! 2 downshift, 5 redline}; physics_hints {1 aero_hint, 4 / 5 / 6 / 7 the
//! low / high speed lateral plateau start / end (g, tyre slip), 8 top speed,
//! 10 forward g, 11 braking g}; and the top speed of the converter's
//! longitudinal model (check.rs `run`) on the same car.
//!
//! cargo run --release --example aifit -- <content.kspkg> > tools/aifit_stock.tsv

use aceconv::car::physics::pbx::rf;
use aceconv::car::physics::stats::{cardata_of, derive};
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
    let cols = [
        "id", "mass_static", "cg_front", "dy_f", "dx_f", "ny_f", "nx_f", "fz0_f", "dy_r", "dx_r", "ny_r", "nx_r", "fz0_r",
        "mu_y_f", "mu_y_r", "mu_x_f", "mu_x_r", "mu_y", "mu_x", "cl_area", "aero_map", "brake_torque", "front_bias", "tyre_radius", "limiter", "gb_up", "gb_down",
        "ai_up", "ai_down", "ai_redline", "aero_hint", "lat_low_start", "slip_low_start", "lat_low", "slip_low",
        "lat_high_start", "slip_high_start", "lat_high", "slip_high", "top_kmh", "forward", "braking", "model_top_kmh",
    ];
    println!("{}", cols.join("\t"));
    for (id, paths) in &cars {
        if !paths.iter().any(|p| p.to_lowercase() == format!("content\\cars\\{id}\\{id}.actor")) {
            continue;
        }
        let Some(cdp) = cardata_of(paths.iter().map(|s| s.as_str())) else { continue };
        let Some(cd) = get(&cdp).and_then(|b| Msg::parse(&b).ok()) else { continue };
        let d = derive(&cd, &get);
        let tyre = |num: u32| -> [f64; 5] {
            let md = cd.str(num).and_then(|p| get(&p)).and_then(|b| Msg::parse(&b).ok()).and_then(|t| t.sub(4)).unwrap_or_default();
            [1, 2, 3, 4, 5].map(|k| rf(&md, k).unwrap_or(f64::NAN))
        };
        let (tf, tr) = (tyre(10), tyre(11));
        let ai = cd.str(29).and_then(|p| get(&p)).and_then(|b| Msg::parse(&b).ok()).unwrap_or_default();
        let gears = ai.sub(1).unwrap_or_default();
        let h = ai.sub(5).unwrap_or_default();
        let pt = |k: u32| -> [f64; 2] {
            let p = h.sub(k).unwrap_or_default();
            [rf(&p, 1).unwrap_or(f64::NAN), rf(&p, 2).unwrap_or(f64::NAN)]
        };
        // AeroData 7 / 8: front / rear CZ maps (surface3d by ride height): downforce
        // the wings' CL.A does not see
        let aero_map = cd.sub(12).map(|a| a.str(7).is_some() || a.str(8).is_some()).unwrap_or(false) as u8 as f64;
        let gb = cd.str(24).and_then(|p| get(&p)).and_then(|b| Msg::parse(&b).ok()).or_else(|| cd.sub(23)).unwrap_or_default();
        let gb_up = gb.sub(13).and_then(|s| rf(&s, 1)).unwrap_or(f64::NAN);
        let gb_down = gb.sub(13).and_then(|s| rf(&s, 5)).unwrap_or(f64::NAN);
        // averaged over the axles by static weight
        let mu_y = d.mu_y[0] * d.cg_front + d.mu_y[1] * (1.0 - d.cg_front);
        let mu_x = d.mu_x[0] * d.cg_front + d.mu_x[1] * (1.0 - d.cg_front);
        let mut row: Vec<String> = vec![id.clone()];
        let mut v = vec![d.mass_static, d.cg_front];
        v.extend(tf);
        v.extend(tr);
        v.extend([d.mu_y[0], d.mu_y[1], d.mu_x[0], d.mu_x[1], mu_y, mu_x, d.cl_area, aero_map, d.brake_torque, d.front_bias, (d.tyre_radius[0] + d.tyre_radius[1]) / 2.0, d.limiter, gb_up, gb_down]);
        v.extend([1, 2, 5].map(|k| rf(&gears, k).unwrap_or(f64::NAN)));
        v.push(rf(&h, 1).unwrap_or(f64::NAN));
        for k in 4..=7 {
            v.extend(pt(k));
        }
        v.extend([8, 10, 11].map(|k| rf(&h, k).unwrap_or(f64::NAN)));
        // the converter's longitudinal model (check.rs) on the same car
        v.push(aceconv::car::physics::check::run(&d).top_speed * 3.6);
        row.extend(v.iter().map(|x| format!("{x}")));
        println!("{}", row.join("\t"));
    }
    Ok(())
}
