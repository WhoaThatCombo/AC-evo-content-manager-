//! Car physics: every EVO physics, setup and AI file of the converted car,
//! built from the AC1 data (`data.acd` / `data\`) and the car frame.
//!
//! The donor's files are edited field by field (`io::pb`), so fields this
//! stage does not know survive. CarData (`data\cardata.car`) carries inline
//! copies of the engine, drivetrain, gearbox, clutch, brakes, coilovers,
//! suspension geometries and wings; every stage edits the inline copy and
//! `sync` writes each standalone file from it, so the two always agree
//! (stock: 40 / 71 engines, 65 / 71 geometries agree; the rest are stale
//! copies Kunos left behind).
//!
//! The mapping is AC1 unit -> the EVO field of the same meaning. Stock twins
//! (docs/FORMATS.md, "Car physics") show EVO kept AC1's physics vocabulary: on
//! ks_toyota_ae86 -> ks_toyota_ae86_sprinter_trueno the springs, dampers,
//! anti-roll bars, track, wheelbase, gears, final drive, diff and fuel are
//! the AC1 numbers unchanged; ks_mazda_miata -> ks_mazda_mx5_na keeps the
//! double-wishbone hard points to the last digit and the torque curve.
//! What EVO added (tyre model, ride-height targets, AI hints, setup ranges)
//! is derived and marked OURS where no stock fact fixes it.

pub mod aero;
pub mod ai;
pub mod chassis;
pub mod check;
pub mod ctrl;
pub mod drivetrain;
pub mod dtcheck;
pub mod engine;
pub mod pbx;
pub mod setup;
pub mod stats;
pub mod suspension;
pub mod tyres;

use anyhow::{anyhow, Result};
use std::path::Path;

use crate::ac1::car::Car;
use crate::ac1::ini::Ini;
use crate::car::frame::Frame;
use crate::files::Files;
use crate::io::kspkg::Pkg;
use crate::io::pb::{Msg, Val};

pub const G: f64 = 9.81;
pub const KG_PER_LITRE: f64 = 0.755;

/// Numbers one stage derives and later stages (setup, AI, checks) use.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Derived {
    /// TOTALMASS (driver, no fuel) and with the default fuel
    pub mass: f64,
    pub mass_static: f64,
    pub fuel: f64,
    pub max_fuel: f64,
    /// front weight share 0..1
    pub cg_front: f64,
    pub wheelbase: f64,
    pub track: [f64; 2],
    /// full-throttle crank torque before boost (rpm, N m)
    pub torque: Vec<(f64, f64)>,
    /// steady boost at full throttle and full spool (bar), per rpm (1 + boost multiplies torque)
    pub boost: Vec<(f64, f64)>,
    pub idle: f64,
    pub limiter: f64,
    /// forward ratios, 1st first
    pub gears: Vec<f64>,
    pub reverse: f64,
    pub final_drive: f64,
    pub shift_up_rpm: f64,
    pub shift_down_rpm: f64,
    /// 0 RWD, 1 FWD, 2 four-wheel drive (any of the EVO four-wheel types)
    pub traction: u8,
    /// the EVO TractionType written (0 RWD, 1 FWD, 2 FourWD, 3 AWD, 4 AWDF)
    pub traction_type: u8,
    /// AC1 drivetrain.ini [TRACTION] TYPE as read (RWD / FWD / AWD / AWD2)
    pub ac1_traction: String,
    /// the setup screen's differential (power, coast 0..1, preload N m): the
    /// diff `differential_data` and the setup edit (see drivetrain.rs)
    pub diff: [f64; 3],
    /// drivetrain efficiency used by the AI / PI models (OURS)
    pub efficiency: f64,
    /// tyre radius / width / rim radius front, rear (metres)
    pub tyre_radius: [f64; 2],
    /// lateral / longitudinal friction coefficient of the default compound at the static wheel load
    pub mu_y: [f64; 2],
    pub mu_x: [f64; 2],
    pub tyre_rate: [f64; 2],
    pub compounds: usize,
    pub default_compound: usize,
    pub brake_torque: f64,
    pub front_bias: f64,
    /// sum of wing CL x area (positive = downforce) and CD x area, m^2
    pub cl_area: f64,
    pub cd_area: f64,
    pub spring: [f64; 2],
    pub collar: [f64; 2],
    pub bump_up: [f64; 2],
    pub bump_dn: [f64; 2],
    pub sag: [f64; 2],
    pub rest_x: [f64; 2],
    pub target_height_mm: [f64; 2],
    pub steer_lock: f64,
    pub steer_ratio: f64,
    pub has_abs: bool,
    pub abs_on: bool,
    pub has_tc: bool,
    pub tc_on: bool,
    pub tc_levels: usize,
    pub turbo: bool,
    pub turbo_adjustable: bool,
    pub engine_maps: usize,
}

pub struct Ctx<'a> {
    pub files: &'a mut Files,
    pub id: String,
    pub stem: String,
    /// `content\cars\<id>`
    pub base: String,
    pub car: &'a Car,
    pub frame: &'a Frame,
    /// the game's content.kspkg (stock tyre templates); None = offline
    pub game: Option<Pkg>,
    pub cd: Msg,
    pub cd_path: String,
    pub notes: Vec<String>,
    pub d: Derived,
}

impl<'a> Ctx<'a> {
    pub fn ini(&self, name: &str) -> &Ini {
        self.car.ini(name)
    }
    pub fn data_path(&self, file: &str) -> String {
        format!("{}\\data\\{file}", self.base)
    }
    /// A file of the package or (read-only) of the game.
    pub fn read_any(&self, path: &str) -> Option<Vec<u8>> {
        if let Some(k) = self.files.find_ci(path) {
            return self.files.get(&k).cloned();
        }
        let g = self.game.as_ref()?;
        let e = g.find(path)?;
        g.read(e).ok()
    }
    /// An AC1 data file's text by name (case-insensitive).
    pub fn data_text(&self, name: &str) -> Option<&str> {
        self.car.data.get(&name.trim().to_lowercase()).map(|s| s.as_str())
    }
    /// An AC1 lut by file name, or an inline `(|x=y|)` value.
    pub fn lut(&self, v: &str) -> Vec<(f64, f64)> {
        let v = v.trim();
        if v.starts_with('(') {
            return crate::ac1::ini::lut(v);
        }
        self.data_text(v).map(crate::ac1::ini::lut).unwrap_or_default()
    }
    /// Put a file, reusing the existing key when only the case differs.
    pub fn put(&mut self, path: &str, data: Vec<u8>) {
        let k = self.files.find_ci(path).unwrap_or_else(|| path.to_string());
        self.files.put(&k, data);
    }
    pub fn note(&mut self, s: impl Into<String>) {
        self.notes.push(s.into());
    }
    /// A CarData path field's value (a donor file renamed by the identity stage).
    pub fn cd_path_of(&self, num: u32) -> Option<String> {
        self.cd.str(num)
    }
}

pub struct Report {
    pub notes: Vec<String>,
    pub perf: f64,
    pub derived: Derived,
}

/// CarData inline message <-> standalone path (both CarData fields).
const COPIES: [(u32, u32); 6] = [(4, 20), (3, 22), (23, 24), (25, 26), (5, 27), (7, 77)];
/// SuspensionsData inline <-> path: coilovers, geometries.
const SUSP_COPIES: [(u32, u32); 4] = [(8, 20), (9, 21), (40, 22), (41, 23)];

fn encoded(v: &Val) -> Option<Vec<u8>> {
    match v {
        Val::Bytes(b) => Some(b.clone()),
        Val::Msg(m) => Some(m.encode()),
        _ => None,
    }
}

/// Write every standalone file from its CarData inline copy.
fn sync(c: &mut Ctx) -> Result<()> {
    for (inl, path) in COPIES {
        let (Some(v), Some(p)) = (c.cd.get(inl).and_then(encoded), c.cd.str(path)) else { continue };
        c.put(&p, v);
    }
    let s = c.cd.sub(2).ok_or_else(|| anyhow!("CarData has no suspensions"))?;
    for (inl, path) in SUSP_COPIES {
        let (Some(v), Some(p)) = (s.get(inl).and_then(encoded), s.str(path)) else { continue };
        c.put(&p, v);
    }
    if let Some(a) = c.cd.sub(12) {
        let wings: Vec<Vec<u8>> = a.all(3).filter_map(encoded).collect();
        let paths: Vec<String> = a.all(10).filter_map(|v| match v { Val::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()), _ => None }).collect();
        for (w, p) in wings.iter().zip(&paths) {
            c.put(p, w.clone());
        }
    }
    // turbos: the inline TurboData list <-> turbos_to_load files
    if let Some(e) = c.cd.sub(4) {
        let t: Vec<Vec<u8>> = e.all(24).filter_map(encoded).collect();
        let p: Vec<String> = e.all(29).filter_map(|v| match v { Val::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()), _ => None }).collect();
        for (t, p) in t.iter().zip(&p) {
            c.put(p, t.clone());
        }
    }
    let b = c.cd.encode();
    c.files.put(&c.cd_path.clone(), b);
    Ok(())
}

/// Every file of the package a physics file names that is gone or empty.
fn drop_unreferenced(c: &mut Ctx) -> usize {
    let data = format!("{}\\data\\", c.base).to_lowercase();
    let tyres = format!("{}\\tyres\\", c.base).to_lowercase();
    let exts = [".curve", ".wing", ".turbo", ".tyre", ".carengine", ".gearbox", ".drivetrain", ".suspension", ".coilover", ".tuningpart"];
    let mut removed = 0;
    loop {
        let mut text: Vec<u8> = Vec::new();
        for (p, b) in &c.files.map {
            let l = p.to_lowercase();
            if l.ends_with(".texturemips") || l.ends_with(".texture") || l.ends_with(".mesh") {
                continue;
            }
            text.extend_from_slice(&b.to_ascii_lowercase());
        }
        let gone: Vec<String> = c.files.map.keys().filter(|k| {
            let l = k.to_lowercase();
            (l.starts_with(&data) || l.starts_with(&tyres)) && exts.iter().any(|e| l.ends_with(e)) && !l.contains("\\setup\\")
                && memchr::memmem::find(&text, l.as_bytes()).is_none()
        }).cloned().collect();
        if gone.is_empty() {
            break;
        }
        for g in gone {
            c.files.remove(&g);
            removed += 1;
        }
    }
    removed
}

/// Convert the AC1 physics into the cloned donor's files.
pub fn write(files: &mut Files, id: &str, stem: &str, car: &Car, frame: &Frame, content: Option<&Path>) -> Result<Report> {
    let base = format!("content\\cars\\{id}");
    let cd_path = files.find_ci(&format!("{base}\\data\\cardata.car")).ok_or_else(|| anyhow!("the donor has no data\\cardata.car"))?;
    let cd = Msg::parse(files.get(&cd_path).unwrap())?;
    let game = content.filter(|p| p.is_file()).and_then(|p| Pkg::open(p).ok());
    let mut c = Ctx { files, id: id.to_string(), stem: stem.to_string(), base, car, frame, game, cd, cd_path, notes: Vec::new(), d: Derived::default() };
    chassis::general(&mut c)?;
    engine::write(&mut c)?;
    drivetrain::write(&mut c)?;
    chassis::brakes_steering(&mut c)?;
    chassis::electronics(&mut c)?;
    tyres::write(&mut c)?;
    suspension::write(&mut c)?;
    aero::write(&mut c)?;
    setup::write(&mut c)?;
    let perf = ai::write(&mut c)?;
    sync(&mut c)?;
    let removed = drop_unreferenced(&mut c);
    if removed > 0 {
        c.note(format!("physics: {removed} donor physics files nothing references removed"));
    }
    let mut notes = c.notes;
    let d = c.d;
    notes.push(format!(
        "physics: {:.0} kg, {:.0} % front, {:.0} kW peak at {:.0} rpm, limiter {:.0}, {} gears, final {:.2}, PI {:.1}",
        d.mass, d.cg_front * 100.0, check::peak_power(&d).0 / 1000.0, check::peak_power(&d).1, d.limiter, d.gears.len(), d.final_drive, perf
    ));
    for p in check::gearing(&d).problems {
        notes.push(format!("physics check: {p}"));
    }
    Ok(Report { notes, perf, derived: d })
}

/// AC1 `x, y, z` value.
pub fn ac1_v3(ini: &Ini, s: &str, k: &str) -> Option<[f64; 3]> {
    let v = ini.nums(s, k);
    (v.len() >= 3).then(|| [v[0], v[1], v[2]])
}
