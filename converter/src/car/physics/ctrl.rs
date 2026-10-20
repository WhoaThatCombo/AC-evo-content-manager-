//! AC1 dynamic controllers (`ctrl_*.ini`) -> EVO DynamicControllerData.
//!
//! AC1 controller sections `[CONTROLLER_n]` (INPUT, COMBINATOR ADD / MULT,
//! LUT, FILTER, UP_LIMIT, DOWN_LIMIT) map one to one onto
//! DynamicControllerStageData (1 inputVar, 2 combinatorMode 1 Add / 2 Mult,
//! 3 lut path, 4 filterGain, 5 upLimit, 6 downLimit, 8 constValue). Stock
//! Kunos cars converted that way keep AC1's file names: ks_bmw_m8_competition's
//! rear lock controller stages are `ctrl_single_lockCONTROLLER_<n>.curve`.
//! The input enum is the game's DynamicControllerStageData.Input.

use super::pbx::{self, f, i};
use super::Ctx;
use crate::io::pb::{Msg, Val};

/// AC1 controller INPUT -> DynamicControllerStageData.Input.
pub fn input_code(name: &str) -> Option<u64> {
    let n = name.trim().to_uppercase().replace(['_', ' '], "");
    Some(match n.as_str() {
        "BRAKE" => 1,
        "GAS" | "THROTTLE" => 2,
        "LATG" => 3,
        "LONG" | "LONGG" => 4,
        "STEER" => 5,
        "SPEEDKMH" | "SPEED" => 6,
        "GEAR" => 7,
        "SLIPANGLEFRONTAVERAGE" | "SLIPANGLEFRONTAVG" => 10,
        "SLIPANGLEREARAVERAGE" | "SLIPANGLEREARAVG" => 11,
        "SLIPANGLEFRONTMAX" => 12,
        "SLIPANGLEREARMAX" => 13,
        "OVERSTEERFACTOR" => 14,
        "REARSPEEDRATIO" => 15,
        "STEERDEG" => 16,
        "CONST" => 17,
        "RPMS" | "RPM" => 18,
        _ => return None,
    })
}

#[derive(Clone, Debug)]
pub struct Stage {
    pub input: u64,
    pub add: bool,
    pub lut: Vec<(f64, f64)>,
    pub filter: f64,
    pub up: Option<f64>,
    pub down: Option<f64>,
}

/// The stages of an AC1 controller file (None when the car has no such file).
/// Stages whose input has no EVO equivalent are dropped with a note.
pub fn read(c: &mut Ctx, file: &str, what: &str) -> Option<Vec<Stage>> {
    let t = c.data_text(file).map(crate::ac1::ini::parse)?;
    let mut out = Vec::new();
    for (s, _) in t.with_prefix("CONTROLLER_") {
        let input = t.get(s, "INPUT").unwrap_or("").split_whitespace().next().unwrap_or("").to_string();
        let Some(code) = input_code(&input) else {
            c.note(format!("physics: {what} controller input {input} has no EVO equivalent; stage dropped"));
            continue;
        };
        let lut = t.get(s, "LUT").map(|v| c.lut(v)).unwrap_or_default();
        if lut.is_empty() && code != 17 {
            continue;
        }
        out.push(Stage {
            input: code,
            add: !t.get(s, "COMBINATOR").unwrap_or("ADD").trim().eq_ignore_ascii_case("MULT"),
            lut,
            filter: t.num(s, "FILTER").unwrap_or(0.95),
            up: t.num(s, "UP_LIMIT"),
            down: t.num(s, "DOWN_LIMIT"),
        });
    }
    Some(out)
}

/// Scale a controller's output by `k` (ADD stage values and every limit;
/// MULT factors stay): the clamps are linear, so the output scales exactly.
pub fn scaled(stages: &[Stage], k: f64) -> Vec<Stage> {
    stages.iter().map(|s| Stage {
        lut: if s.add { s.lut.iter().map(|p| (p.0, p.1 * k)).collect() } else { s.lut.clone() },
        up: s.up.map(|v| v * k),
        down: s.down.map(|v| v * k),
        ..s.clone()
    }).collect()
}

/// DynamicControllerData {2 stages} with one `.curve` per stage, named
/// `<prefix><n>.curve`; the curve files are returned for the caller to put.
pub fn message(stages: &[Stage], prefix: &str) -> (Msg, Vec<(String, Vec<u8>)>) {
    let mut dc = Msg::default();
    let mut files = Vec::new();
    for (j, s) in stages.iter().enumerate() {
        let p = format!("{prefix}{j}.curve");
        let mut st = Msg::default();
        i(&mut st, 1, s.input as i64);
        i(&mut st, 2, if s.add { 1 } else { 2 });
        st.set_str(3, &p);
        f(&mut st, 4, s.filter);
        pbx::fo(&mut st, 5, s.up);
        pbx::fo(&mut st, 6, s.down);
        if s.input == 17 {
            f(&mut st, 8, s.lut.first().map(|p| p.1).unwrap_or(0.0));
        }
        dc.push(2, Val::Msg(st));
        files.push((p, pbx::curve(&s.lut)));
    }
    (dc, files)
}
