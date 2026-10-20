//! Gearbox, final drive, differential(s) and clutch from drivetrain.ini.
//!
//! Stock twins keep AC1's numbers: ae86_sprinter_trueno's gears, final
//! (4.3) and diff (power 0.25 / coast 0.25 / preload 5) are AC1's. Shift
//! times are milliseconds on both sides. The autoblip profile is (ms, level)
//! (stock mx5_na: 0|0, 17|0.89, 195|0.78, 250|0 for AC1 POINT_0..2 = 10 /
//! 150 / 180 ms at LEVEL 0.7); the autoclutch profiles are (seconds, clutch
//! engagement 1 = closed).
//!
//! Traction (docs/FORMATS.md, "Drivetrain"). EVO `TractionType` is 0 RWD,
//! 1 FWD, 2 FourWD (a centre differential with a fixed `front_share`), 3 AWD
//! (rear-driven; each front wheel joins through its own clutch,
//! `awd_clutches` position 0 / 1), 4 AWDF (front-driven; rear wheels through
//! clutches 2 / 3). Facts: every stock car's enum matches the real car
//! (Abarth, Junior, A290, Mini, R5, Golfs, i30 N FWD; quattro, Delta,
//! Escort Cosworth, 205 T16, RS 6 FourWD; R8 V10 and M8 AWD with front
//! clutches; RS 3 and Golf R AWDF with rear clutches), and the game's
//! drivetrain set-up (exe 0x1422e23d0 / 0x1422e3e00) wires exactly that:
//! the lock controllers it builds per type, and clutch slots 0..3 on the
//! wheels LF, RF, LR, RR, freed (slipping) for 0 / 1 under AWD and 2 / 3
//! under AWDF.
//!
//! AC1 -> EVO:
//! - RWD / FWD: `differential_data` = [DIFFERENTIAL]; no four-wheel parts.
//! - AWD (a centre differential): FourWD; front / rear diffs = [AWD]
//!   FRONT_ / REAR_DIFF_*, the centre an EpicyclicLSD (the epicyclic types
//!   are the ones that split unevenly; every stock FourWD centre is one) with
//!   CENTRE_DIFF_* and `front_share` = FRONT_SHARE / 100 (AC1 is always
//!   percent: Gallardo 1, A1 quattro 98, Sport quattro 50).
//! - AWD2 (rear-driven, the front axle through an electronic centre clutch,
//!   AC1's Haldex-style model used by R8, Huracan, GT-R, 991 Turbo, ...):
//!   AWD; front / rear diffs = [AWD2] FRONT_ / REAR_DIFF_*; two front clutches.
//!   With ctrl_awd2.ini (its output is the centre lock torque in N m) each
//!   clutch carries that controller at half the torque (two wheel clutches
//!   share the one AC1 centre clutch); without it each clutch gets a preload
//!   of min(CENTRE_RAMP_TORQUE, CENTRE_MAX_TORQUE) / 2 (OURS).
//! - ctrl_single_lock.ini (AC1's electronic diff lock): the driven axle's
//!   lock controller, rear (f5) or front (f8) by type; an AWD car's
//!   ctrl_awd_center_lock.ini: the FourWD centre lock controller (f7).
//! - For the four-wheel types `differential_data` repeats the rear diff and
//!   the setup screen edits it (stock R8 V10: setup diff = rear diff).
//! - Locks are 0..1 on both sides (power = coast = 1 -> Spool); preload N m.
//!
//! Auto-shift defaults, when drivetrain.ini has no [AUTO_SHIFTER] UP / DOWN:
//! the stock medians measured by tools/aifit.py (tools/aifit_summary.txt,
//! "auto-shift rpm over the limiter"; the 69 stock cars with a redline):
//! up = GearBoxData.autoshifter.up / engine limiter, down = AiGears
//! downshift / redline (no stock gearbox carries a down threshold).

use anyhow::Result;

use super::pbx::{self, at, f, i};
use super::Ctx;
use crate::ac1::ini::Ini;
use crate::io::pb::{Msg, Val};

/// Default auto-shift up rpm over the limiter (stock median, tools/aifit.py).
pub const AUTO_UP_OF_LIMITER: f64 = 0.9538461538461539;
/// Default auto-shift down rpm over the limiter (stock median, tools/aifit.py).
pub const AUTO_DOWN_OF_LIMITER: f64 = 0.5716034271725826;

pub const TRACTION_NAMES: [&str; 5] = ["RWD", "FWD", "FourWD", "AWD", "AWDF"];

/// AC1 [TRACTION] TYPE (comments and anything after the first word dropped).
pub fn ac1_traction(dt: &Ini) -> String {
    let v = dt.get("TRACTION", "TYPE").unwrap_or("RWD").to_uppercase();
    let w: String = v.trim().chars().take_while(|ch| ch.is_ascii_alphanumeric()).collect();
    if w.is_empty() { "RWD".into() } else { w }
}

/// AC1 traction -> EVO TractionType.
pub fn evo_traction(ac1: &str) -> u8 {
    match ac1 {
        "FWD" => 1,
        "AWD" | "4WD" => 2,
        "AWD2" => 3,
        _ => 0,
    }
}

/// (power, coast, preload)
pub type Lock = [f64; 3];

pub struct Diffs {
    /// `differential_data` (and the setup's)
    pub main: Lock,
    pub front: Option<Lock>,
    pub rear: Option<Lock>,
    /// centre lock and front share 0..1
    pub centre: Option<(Lock, f64)>,
    /// AWD front clutches: (preload per clutch N m, controller stages)
    pub clutches: Option<(f64, Vec<super::ctrl::Stage>)>,
    /// ctrl_single_lock.ini stages
    pub single_lock: Vec<super::ctrl::Stage>,
    /// ctrl_awd_center_lock.ini stages (AWD only)
    pub centre_lock: Vec<super::ctrl::Stage>,
}

/// AC1 locks above 1 (some mods write percent: 85 / 65) lock the diff
/// fully in AC1 too; they are clamped here so the diff and the setup agree.
fn lock3(dt: &Ini, sec: &str, pre: &str, d: Lock) -> Lock {
    [
        dt.num(sec, &format!("{pre}POWER")).unwrap_or(d[0]).clamp(0.0, 1.0),
        dt.num(sec, &format!("{pre}COAST")).unwrap_or(d[1]).clamp(0.0, 1.0),
        dt.num(sec, &format!("{pre}PRELOAD")).unwrap_or(d[2]).max(0.0),
    ]
}

/// Every differential the car gets, from drivetrain.ini (+ ctrl files).
pub fn diffs(c: &mut Ctx, dt: &Ini, trac: &str) -> Diffs {
    let single = lock3(dt, "DIFFERENTIAL", "", [0.0, 0.0, 0.0]);
    let single_lock = super::ctrl::read(c, "ctrl_single_lock.ini", "diff lock").unwrap_or_default();
    match trac {
        "AWD" | "4WD" => {
            let rear = lock3(dt, "AWD", "REAR_DIFF_", single);
            let share = dt.num("AWD", "FRONT_SHARE").unwrap_or(40.0);
            Diffs {
                main: rear,
                front: Some(lock3(dt, "AWD", "FRONT_DIFF_", [0.0, 0.0, 0.0])),
                rear: Some(rear),
                centre: Some((lock3(dt, "AWD", "CENTRE_DIFF_", [0.0, 0.0, 0.0]), (share / 100.0).clamp(0.0, 1.0))),
                clutches: None,
                single_lock,
                centre_lock: super::ctrl::read(c, "ctrl_awd_center_lock.ini", "AWD centre lock").unwrap_or_default(),
            }
        }
        "AWD2" => {
            let rear = lock3(dt, "AWD2", "REAR_DIFF_", single);
            let ramp = dt.num("AWD2", "CENTRE_RAMP_TORQUE").unwrap_or(0.0).max(0.0);
            let max = dt.num("AWD2", "CENTRE_MAX_TORQUE").unwrap_or(ramp).max(0.0);
            let clutches = match super::ctrl::read(c, "ctrl_awd2.ini", "AWD2 centre") {
                Some(st) if !st.is_empty() => (0.0, super::ctrl::scaled(&st, 0.5)),
                _ => (ramp.min(max) / 2.0, Vec::new()),
            };
            Diffs {
                main: rear,
                front: Some(lock3(dt, "AWD2", "FRONT_DIFF_", [0.0, 0.0, 0.0])),
                rear: Some(rear),
                centre: None,
                clutches: Some(clutches),
                single_lock,
                centre_lock: Vec::new(),
            }
        }
        _ => Diffs { main: single, front: None, rear: None, centre: None, clutches: None, single_lock, centre_lock: Vec::new() },
    }
}

/// A DifferentialData's lock: type Spool when fully locked, else the given
/// type (None = the default LSD).
fn diff(m: &mut Msg, l: Lock, ty: Option<i64>) {
    m.fields.retain(|x| ![1, 2, 3, 4, 11, 12, 13].contains(&x.num));
    let (power, coast, preload) = (l[0], l[1], l[2]);
    if power >= 0.999 && coast >= 0.999 {
        i(m, 4, 1); // Spool
    } else if let Some(t) = ty {
        i(m, 4, t);
    }
    f(m, 1, power.clamp(0.0, 1.0));
    f(m, 2, coast.clamp(0.0, 1.0));
    f(m, 3, preload.max(0.0));
    m.fields.sort_by_key(|x| x.num);
}

fn put_files(c: &mut Ctx, files: Vec<(String, Vec<u8>)>) {
    for (p, b) in files {
        c.put(&p, b);
    }
}

/// DrivetrainData (CarData f3): type, diffs, AWD clutches, lock controllers.
/// Every four-wheel and controller field is rewritten, so nothing of the
/// donor's (an FWD Abarth) drivetrain survives into another layout.
fn write_drivetrain(c: &mut Ctx, tt: u8, ds: &Diffs) -> Result<()> {
    let base = c.base.clone();
    let mut files = Vec::new();
    {
        let d = at(&mut c.cd, &[3])?;
        match tt {
            0 => d.remove(2),
            t => i(d, 2, t as i64),
        }
        diff(d.msg_mut(39)?, ds.main, None);
        let w = d.msg_mut(15)?;
        for (k, l, ty) in [(1u32, ds.front, None), (2, ds.rear, None), (3, ds.centre.map(|x| x.0), Some(4))] {
            match l {
                Some(l) => diff(w.msg_mut(k)?, l, ty),
                None => w.set(k, Val::Msg(Msg::default())),
            }
        }
        if let Some((_, share)) = ds.centre {
            let ce = w.msg_mut(3)?;
            f(ce, 13, share);
            ce.fields.sort_by_key(|x| x.num);
        }
        w.fields.sort_by_key(|x| x.num);
        // lock controllers: the donor's are emptied; AC1's single lock goes on the driven axle
        for k in [5u32, 6, 7, 8] {
            d.set(k, Val::Msg(Msg::default()));
        }
        d.remove(50);
        d.remove(51);
        d.remove(45);
        if !ds.single_lock.is_empty() {
            let (m, fl) = super::ctrl::message(&ds.single_lock, &format!("{base}\\data\\ctrl_single_lock_controller"));
            d.set(if tt == 1 { 8 } else { 5 }, Val::Msg(m));
            files.extend(fl);
        }
        if !ds.centre_lock.is_empty() {
            let (m, fl) = super::ctrl::message(&ds.centre_lock, &format!("{base}\\data\\ctrl_awd_center_lock_controller"));
            d.set(7, Val::Msg(m));
            files.extend(fl);
        }
        if let Some((preload, stages)) = &ds.clutches {
            for pos in [0i64, 1] {
                let mut cl = Msg::default();
                if pos != 0 {
                    i(&mut cl, 1, pos);
                }
                if *preload > 0.0 {
                    f(&mut cl, 2, *preload);
                }
                if !stages.is_empty() {
                    let (m, fl) = super::ctrl::message(stages, &format!("{base}\\data\\ctrl_awd2_clutch{pos}_controller"));
                    cl.set(3, Val::Msg(m));
                    files.extend(fl);
                }
                d.push(45, Val::Msg(cl));
            }
        }
        d.fields.sort_by_key(|x| x.num);
    }
    put_files(c, files);
    if let Some((pre, st)) = &ds.clutches {
        c.note(if st.is_empty() {
            format!("physics: AWD2 -> AWD, front clutches with {pre:.0} N m each (no ctrl_awd2.ini)")
        } else {
            format!("physics: AWD2 -> AWD, front clutches driven by ctrl_awd2.ini ({} stages, half torque each)", st.len())
        });
    }
    Ok(())
}

pub fn write(c: &mut Ctx) -> Result<()> {
    let dt: Ini = c.ini("drivetrain.ini").clone();
    let n = dt.num("GEARS", "COUNT").unwrap_or(0.0) as usize;
    let mut gears: Vec<f64> = (1..=n).filter_map(|k| dt.num("GEARS", &format!("GEAR_{k}"))).filter(|r| *r > 0.0).collect();
    if gears.is_empty() {
        gears = vec![3.2, 2.1, 1.5, 1.2, 1.0];
        c.note("physics: drivetrain.ini has no gears; a generic 5-speed");
    }
    let reverse = dt.num("GEARS", "GEAR_R").map(|r| r.abs()).filter(|r| *r > 0.1).unwrap_or(gears[0]);
    let final_drive = dt.num("GEARS", "FINAL").filter(|v| *v > 0.1).unwrap_or(4.0);
    if dt.get("GEARS", "USE_GEARSET").is_some() || c.data_text("ratios.rto").is_some() {
        c.note("physics: AC1 gear sets (ratios.rto / final.rto) are not offered; the default ratios are used");
    }
    let trac = ac1_traction(&dt);
    let traction_type = evo_traction(&trac);
    let traction: u8 = match traction_type { 0 => 0, 1 => 1, _ => 2 };
    c.d.gears = gears.clone();
    c.d.reverse = reverse;
    c.d.final_drive = final_drive;
    c.d.traction = traction;
    c.d.traction_type = traction_type;
    c.d.ac1_traction = trac.clone();
    c.d.efficiency = match traction { 1 => 0.88, 2 => 0.82, _ => 0.85 };
    c.d.shift_up_rpm = dt.num("AUTO_SHIFTER", "UP").filter(|v| *v > c.d.idle && *v <= c.d.limiter + 1.0).unwrap_or(c.d.limiter * AUTO_UP_OF_LIMITER);
    c.d.shift_down_rpm = dt.num("AUTO_SHIFTER", "DOWN").filter(|v| *v > c.d.idle).unwrap_or(c.d.limiter * AUTO_DOWN_OF_LIMITER);
    let num = |s: &str, k: &str, d: f64| dt.num(s, k).unwrap_or(d);
    // profiles
    let gb_cd = c.cd.sub(23).unwrap_or_default();
    let blip_path = gb_cd.sub(12).and_then(|b| b.str(1)).unwrap_or_else(|| format!("{}\\data\\autoblipprofile.curve", c.base));
    let lvl = num("AUTOBLIP", "LEVEL", 0.7);
    let blip = vec![(0.0, 0.0), (num("AUTOBLIP", "POINT_0", 10.0), lvl), (num("AUTOBLIP", "POINT_1", 150.0), lvl), (num("AUTOBLIP", "POINT_2", 180.0), 0.0)];
    c.put(&blip_path, pbx::curve(&blip));
    let cl_cd = c.cd.sub(25).and_then(|x| x.sub(3)).unwrap_or_default();
    let up_path = cl_cd.str(1).unwrap_or_else(|| format!("{}\\data\\upshiftprofile.curve", c.base));
    let dn_path = cl_cd.str(2).unwrap_or_else(|| format!("{}\\data\\downshiftprofile.curve", c.base));
    let prof = |sec: &str, d: [f64; 3]| -> Vec<(f64, f64)> {
        let p = [num(sec, "POINT_0", d[0]), num(sec, "POINT_1", d[1]), num(sec, "POINT_2", d[2])];
        vec![(0.0, 1.0), (p[0] / 1000.0, 0.0), (p[1].max(p[0]) / 1000.0, 0.0), (p[2].max(p[1]) / 1000.0, 1.0)]
    };
    c.put(&up_path, pbx::curve(&prof("UPSHIFT_PROFILE", [30.0, 80.0, 100.0])));
    c.put(&dn_path, pbx::curve(&prof("DOWNSHIFT_PROFILE", [50.0, 300.0, 600.0])));
    // GearBoxData (CarData f23)
    let g = at(&mut c.cd, &[23])?;
    i(g, 1, gears.len() as i64);
    g.remove(2);
    let mut push = |name: &str, r: f64| {
        let mut m = Msg::default();
        m.set_str(1, name);
        if r != 0.0 {
            f(&mut m, 2, r);
        }
        g.push(2, Val::Msg(m));
    };
    push("R", -reverse);
    push("N", 0.0);
    for (k, r) in gears.iter().enumerate() {
        push(&(k + 1).to_string(), *r);
    }
    f(g, 3, final_drive);
    f(g, 4, num("GEARBOX", "CHANGE_UP_TIME", 200.0));
    f(g, 5, num("GEARBOX", "CHANGE_DN_TIME", 250.0));
    pbx::fo(g, 6, dt.num("GEARBOX", "AUTO_CUTOFF_TIME").filter(|v| *v > 0.0));
    pbx::b(g, 7, num("GEARBOX", "SUPPORTS_SHIFTER", 1.0) > 0.0);
    f(g, 9, num("GEARBOX", "VALID_SHIFT_RPM_WINDOW", 800.0));
    f(g, 10, num("GEARBOX", "CONTROLS_WINDOW_GAIN", 0.4));
    f(g, 11, num("GEARBOX", "INERTIA", 0.02));
    {
        let b = g.msg_mut(12)?;
        b.set_str(1, &blip_path);
        pbx::b(b, 2, num("AUTOBLIP", "ELECTRONIC", 0.0) > 0.0);
    }
    let up = c.d.shift_up_rpm;
    let down = c.d.shift_down_rpm;
    {
        let a = at(&mut c.cd, &[23, 13])?;
        i(a, 1, up.round() as i64);
        i(a, 5, down.round() as i64);
        f(a, 3, num("AUTO_SHIFTER", "SLIP_THRESHOLD", 0.95));
        f(a, 4, num("AUTO_SHIFTER", "GAS_CUTOFF_TIME", 0.2));
        a.fields.sort_by_key(|x| x.num);
    }
    // DrivetrainData (CarData f3)
    let diffs = diffs(c, &dt, &trac);
    c.d.diff = diffs.main;
    write_drivetrain(c, traction_type, &diffs)?;
    let (p, co, pre) = (diffs.main[0], diffs.main[1], diffs.main[2]);
    // ClutchData (CarData f25)
    let cl = at(&mut c.cd, &[25])?;
    // the SDK sample's clutch carries a stale whole-car message in fields 1 /
    // 2 (length-delimited where ClutchData has floats): dropped
    cl.fields.retain(|x| !((x.num == 1 || x.num == 2) && matches!(x.val, Val::Bytes(_) | Val::Msg(_))));
    f(cl, 1, num("GEARBOX", "INERTIA", 0.02));
    f(cl, 2, num("CLUTCH", "MAX_TORQUE", 400.0));
    let a = cl.msg_mut(3)?;
    a.set_str(1, &up_path);
    a.set_str(2, &dn_path);
    pbx::b(a, 3, num("AUTOCLUTCH", "FORCED_ON", 0.0) > 0.0);
    f(a, 4, num("AUTOCLUTCH", "MIN_RPM", 1000.0));
    f(a, 5, num("AUTOCLUTCH", "MAX_RPM", 1800.0));
    pbx::b(a, 6, num("AUTOCLUTCH", "USE_ON_CHANGES", 0.0) > 0.0);
    a.fields.sort_by_key(|x| x.num);
    c.note(format!("physics: drivetrain {} (AC1 {}) {} gears {:?}, final {final_drive:.3}, diff {p:.2}/{co:.2}/{pre:.0}", TRACTION_NAMES[traction_type as usize], trac, gears.len(), gears));
    Ok(())
}
