//! An AC1 tyre compound as a force curve: lateral / longitudinal force
//! against slip angle, slip ratio and load, read from tyres.ini.
//!
//! What AC1's own data files document (key comments in Kunos's tyres.ini
//! and the SDK's `sdk\dev\v1.5_tyres_ac` files) is used as documented:
//! - peak friction: DY_REF / DX_REF at FZ0 with the load exponents
//!   LS_EXPY / LS_EXPX, mu(Fz) = D (Fz / FZ0)^(LS - 1) (the same law EVO's
//!   TyresModelData uses with Dy0 / Fz0 / lsExpY); files without FZ0 use
//!   the older "MU coefficient" DY0 + "load sensitivity" DY1 per kN;
//! - FRICTION_LIMIT_ANGLE: "slip angle peak" (degrees) at FZ0;
//! - FLEX_GAIN: the peak slip angle grows with load ("the bigger the flex,
//!   the bigger the added slipangle with load"); read as FLA x (1 +
//!   FLEX_GAIN x (Fz / FZ0 - 1)), the linear form EVO's
//!   friction_limit_angle gain takes;
//! - FALLOFF_LEVEL / FALLOFF_SPEED: past the peak the force falls toward
//!   FALLOFF_LEVEL of the peak at FALLOFF_SPEED (EVO's falloff_level /
//!   falloff_slope carry the same names and magnitudes: AC1 0.85..0.90 /
//!   2..7, EVO slicks 0.75..0.8 / 2.5..5);
//! - BRAKE_DX_MOD: extra longitudinal grip under braking (D x (1 + it));
//! - COMBINED_FACTOR: the combined-slip norm (absent = 2, euclidean).
//!
//! What the files do not pin down is taken from the EVO compound the
//! converter starts from (`Shape`): the brush exponent below the peak, the
//! post-peak shape exponent and the longitudinal slip stiffness (AC1's
//! CX_MULT x the lateral stiffness would put a street tyre's longitudinal
//! peak at slip ratio ~0.16 against EVO's ~0.07; its meaning is not
//! documented, so the fit matches longitudinal force in slip normalised
//! to the peak instead). XMU, SPEED_SENSITIVITY, camber, thermal and
//! pressure terms are not part of the curve (zero camber, fresh, warm,
//! ideal pressure, the speed the fit evaluates at).

use crate::ac1::ini::Ini;

pub use crate::source::Shape;

#[derive(Clone, Debug)]
pub struct Ac1Tyre {
    pub name: String,
    pub short: String,
    pub width: f64,
    pub radius: f64,
    pub rim_radius: f64,
    /// vertical rate N/m
    pub rate: f64,
    /// reference load (FZ0, or the static load for the older files)
    pub fz0: f64,
    /// (D at fz0, exponent) or the older linear law (DY0, DY1 per kN)
    pub dy: Law,
    pub dx: Law,
    pub fla: f64,
    pub flex_gain: f64,
    pub falloff_level: f64,
    pub falloff_speed: f64,
    pub brake_dx_mod: f64,
    pub combined: f64,
    pub cx_mult: f64,
    /// the keys this compound has (for the audit)
    pub keys: Vec<String>,
    pub shape: Shape,
}

#[derive(Clone, Copy, Debug)]
pub enum Law {
    Power { d: f64, ls: f64, fz0: f64 },
    Linear { d0: f64, d1: f64 },
}

impl Law {
    pub fn mu(&self, fz: f64) -> f64 {
        match *self {
            Law::Power { d, ls, fz0 } => d * (fz.max(1.0) / fz0).powf(ls - 1.0),
            Law::Linear { d0, d1 } => (d0 + d1 * fz / 1000.0).max(0.05),
        }
    }
}

impl Ac1Tyre {
    /// One compound section (`FRONT`, `REAR_1`, ...); `fz_static` stands in
    /// for FZ0 where the file has none.
    pub fn read(t: &Ini, sec: &str, fz_static: f64) -> Option<Ac1Tyre> {
        if !t.has(sec) {
            return None;
        }
        let g = |k: &str| t.num(sec, k);
        let radius = g("RADIUS").filter(|v| *v > 0.1 && *v < 1.0)?;
        let width = g("WIDTH").filter(|v| *v > 0.05 && *v < 1.0).unwrap_or(0.2);
        let rim_radius = g("RIM_RADIUS").filter(|v| *v > 0.05 && *v < radius).unwrap_or(radius - 0.1);
        let fz0 = g("FZ0").filter(|v| *v > 100.0);
        let law = |dref: Option<f64>, ls: Option<f64>, d0: Option<f64>, d1: Option<f64>| match (dref, fz0) {
            (Some(d), Some(f)) => Law::Power { d, ls: ls.unwrap_or(0.8), fz0: f },
            _ => Law::Linear { d0: d0.unwrap_or(1.2), d1: d1.unwrap_or(0.0) },
        };
        Some(Ac1Tyre {
            name: t.get(sec, "NAME").unwrap_or("Tyre").to_string(),
            short: t.get(sec, "SHORT_NAME").unwrap_or("T").to_string(),
            width,
            radius,
            rim_radius,
            rate: g("RATE").filter(|v| *v > 1e4).unwrap_or(250_000.0),
            fz0: fz0.unwrap_or(fz_static),
            dy: law(g("DY_REF"), g("LS_EXPY"), g("DY0"), g("DY1")),
            dx: law(g("DX_REF"), g("LS_EXPX"), g("DX0"), g("DX1")),
            fla: g("FRICTION_LIMIT_ANGLE").filter(|v| *v > 1.0 && *v < 30.0).unwrap_or(7.5),
            flex_gain: g("FLEX_GAIN").unwrap_or(0.0),
            falloff_level: g("FALLOFF_LEVEL").filter(|v| *v > 0.0 && *v <= 1.0).unwrap_or(1.0),
            falloff_speed: g("FALLOFF_SPEED").filter(|v| *v > 0.0).unwrap_or(0.0),
            brake_dx_mod: g("BRAKE_DX_MOD").unwrap_or(0.0),
            combined: g("COMBINED_FACTOR").filter(|v| *v > 0.0).unwrap_or(2.0),
            cx_mult: g("CX_MULT").unwrap_or(1.0),
            keys: t.section(sec).map(|s| s.keys().cloned().collect()).unwrap_or_default(),
            shape: Shape::default(),
        })
    }

    /// Peak slip angle (deg) at load.
    pub fn peak_angle(&self, fz: f64) -> f64 {
        self.fla * (1.0 + self.flex_gain * (fz / self.fz0 - 1.0))
    }

    fn curve(&self, d: f64, s: f64, s_peak: f64, post_exp: f64) -> f64 {
        if s >= s_peak {
            let r = self.falloff_speed;
            if r > 0.0 && self.falloff_level < 1.0 {
                let fall = ((s - s_peak) * r).powf(post_exp);
                return d * ((1.0 - self.falloff_level) / (fall + 1.0) + self.falloff_level);
            }
            return d;
        }
        let x = 1.0 - s / s_peak;
        d * (1.0 - x.powf(self.shape.brush_exp))
    }
}

impl crate::source::TyreCurve for Ac1Tyre {
    fn force(&self, fz: f64, alpha: f64, sr: f64, _speed: f64) -> (f64, f64) {
        if fz <= 0.0 {
            return (0.0, 0.0);
        }
        let t_peak = self.peak_angle(fz).to_radians().tan();
        let k_y = 1.0 / t_peak;
        let k_x = self.shape.kx_over_ky * k_y;
        // slip normalised to its peak on each axis, then the combined norm
        let ny = alpha.tan().abs() * k_y;
        let nx = sr.abs() * k_x;
        let n = self.combined;
        let s = (ny.powf(n) + nx.powf(n)).powf(1.0 / n);
        if s <= 0.0 {
            return (0.0, 0.0);
        }
        let mut dx = self.dx.mu(fz);
        if sr < 0.0 {
            dx *= 1.0 + self.brake_dx_mod;
        }
        let dy = self.dy.mu(fz);
        // the curve in units of the peak slip (s = 1 at the peak); the
        // fall rate acts on tan-slip like EVO's, so scale back by the peak
        let fy = self.curve(dy, s / k_y, t_peak, self.shape.post_exp_y) * (ny / s) * fz * alpha.signum();
        let fx = self.curve(dx, s / k_y, t_peak, self.shape.post_exp_x) * (nx / s) * fz * sr.signum();
        (fy, fx)
    }
    fn fz_ref(&self) -> f64 {
        self.fz0
    }
    fn long_stiffness_known(&self) -> bool {
        false
    }
    fn set_shape(&mut self, s: &Shape) {
        self.shape = *s;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::TyreCurve;

    fn miata() -> Ac1Tyre {
        let ini = crate::ac1::ini::parse("[FRONT]\nRADIUS=0.2888\nWIDTH=0.185\nDY_REF=1.21\nDX_REF=1.22\nFZ0=1761\nLS_EXPY=0.7227\nLS_EXPX=0.7902\nFRICTION_LIMIT_ANGLE=9.45\nFLEX_GAIN=0.0318\nFALLOFF_LEVEL=0.86\nFALLOFF_SPEED=4\nBRAKE_DX_MOD=0.02\n");
        Ac1Tyre::read(&ini, "FRONT", 2500.0).unwrap()
    }

    #[test]
    fn peak_is_dyref_at_fz0_and_the_limit_angle() {
        let t = miata();
        let (fy, _) = t.force(1761.0, 9.45f64.to_radians(), 0.0, 20.0);
        assert!((fy / 1761.0 - 1.21).abs() < 1e-3, "{fy}");
        let (lo, _) = t.force(1761.0, 5f64.to_radians(), 0.0, 20.0);
        let (hi, _) = t.force(1761.0, 30f64.to_radians(), 0.0, 20.0);
        assert!(lo < fy && hi < fy && hi > 0.86 * fy);
        // load sensitivity: mu falls with load
        let (f2, _) = t.force(3522.0, 9.45f64.to_radians(), 0.0, 20.0);
        assert!(f2 / 3522.0 < 1.21);
    }

    #[test]
    fn braking_gets_the_brake_mod() {
        let t = miata();
        let peak = |s: f64| (1..400).map(|i| t.force(1761.0, 0.0, s * i as f64 * 0.001, 20.0).1.abs()).fold(0.0, f64::max);
        assert!((peak(-1.0) / peak(1.0) - 1.02).abs() < 1e-3);
    }
}
