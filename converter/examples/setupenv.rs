//! The game-wide envelope of the stock setup limits: over every stock
//! `.carsetuplimits` (CarSetupDataLimits), per limit (SetupSettingLimits
//! {1 step, 2 min, 3 max, 4 is_modifiable}) the number of cars, how many make
//! it modifiable, the smallest min, the largest max and the smallest step,
//! over all limits, over the modifiable ones, and over the ranges (max > min,
//! whether flagged modifiable or not) and over the fixed values (min = max,
//! not modifiable). The envelope the converter clamps to is the values in
//! use: the union of the modifiable ranges and the fixed values
//! (`min_in_use` / `max_in_use`), with the smallest modifiable step.
//! Envelope items group the limits the converter clamps together
//! (src/car/physics/setup.rs `ENVELOPE`).
//!
//! cargo run --release --example setupenv -- <content.kspkg> > tools/setupenv_stock.tsv

use aceconv::car::physics::pbx::rf;
use aceconv::io::kspkg::Pkg;
use aceconv::io::pb::Msg;
use std::collections::BTreeMap;

/// (limit path, name, envelope item)
const LIMITS: [(&[u32], &str, &str); 37] = [
    (&[1, 1], "mechanical_balance.arbs", "arbs"),
    (&[1, 2], "mechanical_balance.steer_ratio", "steer_ratio"),
    (&[1, 3, 1], "brakes.front_bias", "front_bias"),
    (&[1, 3, 2], "brakes.torque_multiplier", "torque_multiplier"),
    (&[1, 3, 3], "brakes.brake_ducts", "-"),
    (&[1, 4, 1], "differential.power", "diff"),
    (&[1, 4, 2], "differential.coast", "diff"),
    (&[1, 4, 3], "differential.preload", "preload"),
    (&[2, 1], "suspensions.wheel_rate", "wheel_rate"),
    (&[2, 2, 1], "suspensions.bump_stop_up.range", "-"),
    (&[2, 2, 2], "suspensions.bump_stop_up.rate", "bump_rate"),
    (&[2, 3, 1], "suspensions.bump_stop_down.range", "-"),
    (&[2, 3, 2], "suspensions.bump_stop_down.rate", "bump_rate"),
    (&[2, 4], "suspensions.helper_rate", "-"),
    (&[2, 5], "suspensions.helper_range", "-"),
    (&[3, 1], "dampers.slow_bump", "damper"),
    (&[3, 2], "dampers.fast_bump", "damper"),
    (&[3, 3], "dampers.slow_rebound", "damper"),
    (&[3, 4], "dampers.fast_rebound", "damper"),
    (&[4, 1], "alignments.pressure", "pressure"),
    (&[4, 2], "alignments.camber", "camber"),
    (&[4, 3], "alignments.toe", "toe"),
    (&[4, 4], "alignments.caster", "-"),
    (&[4, 5], "alignments.static_camber", "camber"),
    (&[4, 6], "alignments.toe_out_linear", "-"),
    (&[4, 7], "alignments.compound", "-"),
    (&[5, 1], "electronics.tc1", "tc"),
    (&[5, 2], "electronics.tc2", "tc"),
    (&[5, 3], "electronics.abs", "abs"),
    (&[5, 4], "electronics.engine_map", "-"),
    (&[5, 6], "electronics.esc", "-"),
    (&[5, 7], "electronics.turbo_boost_lv", "turbo"),
    (&[6, 1], "aero.collar_positions", "-"),
    (&[6, 2], "aero.front_target_height", "target_height"),
    (&[6, 3], "aero.rear_target_height", "target_height"),
    (&[6, 4], "aero.front_wing_angle", "-"),
    (&[7, 1], "fuel_strategy.fuel", "fuel"),
];

#[derive(Default)]
struct Acc {
    n: usize,
    modifiable: usize,
    lo: f64,
    hi: f64,
    lo_mod: f64,
    hi_mod: f64,
    step: f64,
    ranges: usize,
    lo_rng: f64,
    hi_rng: f64,
    fixed: usize,
    lo_fix: f64,
    hi_fix: f64,
}

impl Acc {
    fn add(&mut self, l: &Msg) {
        let (lo, hi) = (rf(l, 2).unwrap_or(0.0), rf(l, 3).unwrap_or(0.0));
        let m = l.varint(4).unwrap_or(0) != 0;
        if self.n == 0 {
            (self.lo, self.hi, self.lo_mod, self.hi_mod, self.step) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN, f64::MAX);
            (self.lo_rng, self.hi_rng, self.lo_fix, self.hi_fix) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
        }
        if hi == lo && !m {
            self.fixed += 1;
            self.lo_fix = self.lo_fix.min(lo);
            self.hi_fix = self.hi_fix.max(hi);
        }
        if hi > lo {
            self.ranges += 1;
            self.lo_rng = self.lo_rng.min(lo);
            self.hi_rng = self.hi_rng.max(hi);
        }
        self.n += 1;
        self.lo = self.lo.min(lo);
        self.hi = self.hi.max(hi);
        if m {
            self.modifiable += 1;
            self.lo_mod = self.lo_mod.min(lo);
            self.hi_mod = self.hi_mod.max(hi);
            if let Some(s) = rf(l, 1).filter(|s| *s > 0.0) {
                self.step = self.step.min(s);
            }
        }
    }
    fn cols(&self) -> String {
        // f32 on the wire: print the shortest f32 form
        let o = |v: f64| if v.abs() == f64::MAX { "-".to_string() } else { format!("{}", v as f32) };
        let (use_lo, use_hi) = (self.lo_mod.min(self.lo_fix), self.hi_mod.max(self.hi_fix));
        [
            self.n.to_string(), self.modifiable.to_string(), o(self.lo), o(self.hi), o(self.lo_mod), o(self.hi_mod), o(self.step),
            self.ranges.to_string(), o(self.lo_rng), o(self.hi_rng), self.fixed.to_string(), o(self.lo_fix), o(self.hi_fix), o(use_lo), o(use_hi),
        ]
        .join("\t")
    }
}

/// Every message at `path` (repeated fields at any step all count).
fn at(m: &Msg, path: &[u32]) -> Vec<Msg> {
    match path.split_first() {
        None => vec![m.clone()],
        Some((h, rest)) => m.subs(*h).iter().flat_map(|s| at(s, rest)).collect(),
    }
}

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let pkg = Pkg::open(std::path::Path::new(&a[1]))?;
    let mut per: BTreeMap<&str, Acc> = BTreeMap::new();
    let mut env: BTreeMap<&str, Acc> = BTreeMap::new();
    let mut files = 0;
    for e in pkg.entries.iter().filter(|e| {
        let l = e.path.to_lowercase();
        l.starts_with("content\\cars\\") && l.ends_with(".carsetuplimits")
    }) {
        let Some(m) = pkg.read(e).ok().and_then(|b| Msg::parse(&b).ok()) else { continue };
        files += 1;
        for (path, name, item) in LIMITS {
            for l in at(&m, path) {
                per.entry(name).or_default().add(&l);
                if item != "-" {
                    env.entry(item).or_default().add(&l);
                }
            }
        }
    }
    println!("# {files} stock .carsetuplimits");
    println!("limit\tlimits\tmodifiable\tmin\tmax\tmin_modifiable\tmax_modifiable\tsmallest_step_modifiable\tranges\tmin_range\tmax_range\tfixed\tmin_fixed\tmax_fixed\tmin_in_use\tmax_in_use");
    for (name, acc) in &per {
        println!("{name}\t{}", acc.cols());
    }
    println!();
    println!("envelope_item\tlimits\tmodifiable\tmin\tmax\tmin_modifiable\tmax_modifiable\tsmallest_step_modifiable\tranges\tmin_range\tmax_range\tfixed\tmin_fixed\tmax_fixed\tmin_in_use\tmax_in_use");
    for (item, acc) in &env {
        println!("{item}\t{}", acc.cols());
    }
    Ok(())
}
