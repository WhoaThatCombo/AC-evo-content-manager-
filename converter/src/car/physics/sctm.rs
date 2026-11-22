//! EVO's tyre model (SCTM), evaluated from a `.tyre` file (TyresCompoundData).
//!
//! A port of evoview's `src/sctm.rs` (same GPL project family; that file is
//! itself a port of the game's solver: steady-state forces of one tyre at a
//! load, slip angle and slip ratio). Only what the source fit and the
//! whole-car check need is kept: peak friction with load sensitivity, the
//! brush curve to the peak, the post-peak fall (falloff_level / slope /
//! shape, f80..85), the slip-speed falloff (TyresSpeedSensitivityData f8),
//! the brake Dx modifier and both combined-slip formulas. Camber, wear,
//! water, grain, pressure and the aligning moment are left out: the fit
//! compares the tyre at zero camber, fresh, dry, at its ideal pressure.
//!
//! Field numbers (TyresModelData = f4 of the compound): 1 Dy0, 2 Dx0,
//! 3 lsExpY, 4 lsExpX, 5 Fz0, 6 friction_limit_angle (deg), 7 its load
//! gain, 8 cf_x_mult, 9 brakeDXMod, 10 combinedFactor, 13 gripSlipFactor,
//! 23 brushExponent, 60 / 70 / 71 slip-ratio peak, its gain, its switch,
//! 73 new_combined, 74..77 fx_shift_SA / fx_scale_SA / fy_shift_SR /
//! fy_scale_SR, 80..82 lateral falloff level / slope / shape, 83..85
//! longitudinal.

use crate::io::pb::Msg;

use super::pbx::rf;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Falloff {
    pub mu0t: f64,
    pub sens: f64,
    pub t_sens: f64,
    pub t_ref: f64,
    pub ref_speed: f64,
}

impl Falloff {
    /// Peak friction `d` with the patch sliding at `v_slip` m/s, the tread at
    /// the falloff's reference temperature (as evoview runs it).
    pub fn apply(&self, v_slip: f64, d: f64) -> f64 {
        if v_slip <= 0.0 {
            return d;
        }
        let x = (v_slip / (self.ref_speed.max(0.0) + 1e-4)).abs();
        let g = ((-x).exp() + x).ln();
        let keep = self.mu0t * d;
        (-(g * self.sens) * (g * self.sens)).exp() * (d - keep) + keep
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sctm {
    pub dy0: f64,
    pub dx0: f64,
    pub ls_y: f64,
    pub ls_x: f64,
    pub fz0: f64,
    /// friction_limit_angle (deg) and its load gain
    pub fla: f64,
    pub fla_gain: f64,
    /// slip ratio at the peak and its gain (0 = cf_x_mult x the lateral stiffness)
    pub sr_peak: f64,
    pub sr_gain: f64,
    pub cf_x_mult: f64,
    pub brake_dx_mod: f64,
    pub combined_exp: f64,
    pub brush_exp: f64,
    pub new_combined: bool,
    pub fx_shift_sa: f64,
    pub fx_scale_sa: f64,
    pub fy_shift_sr: f64,
    pub fy_scale_sr: f64,
    /// [level, slope, shape]
    pub post_y: [f64; 3],
    pub post_x: [f64; 3],
    pub falloff_y: Falloff,
    pub falloff_x: Falloff,
}

impl Sctm {
    /// From a TyresCompoundData message, with the game's defaults for absent fields.
    pub fn from_compound(td: &Msg) -> Option<Sctm> {
        let md = td.sub(4)?;
        let g = |f: u32| rf(&md, f).unwrap_or(0.0);
        let (dy0, ls_y, ls_x, fz0) = (g(1), g(3), g(4), g(5));
        if dy0 <= 0.0 || fz0 <= 0.0 {
            return None;
        }
        let dx0 = if g(2) == 0.0 { 1.2 * dy0 } else { g(2) };
        let use_sr = g(71) != 0.0;
        let spd = td.sub(8).unwrap_or_default();
        let sg = |f: u32, d: f64| rf(&spd, f).filter(|v| *v != 0.0).unwrap_or(d);
        let fy = Falloff { mu0t: sg(11, 0.9), sens: sg(9, 0.01), t_sens: sg(12, 0.4), t_ref: sg(13, 70.0), ref_speed: sg(8, 2.0) };
        let fx = if sg(112, 0.0) > 0.0 {
            Falloff { mu0t: sg(112, 0.9), sens: sg(114, 0.01), t_sens: sg(115, 0.4), t_ref: sg(116, 70.0), ref_speed: sg(113, 2.0) }
        } else {
            fy
        };
        let nz = |v: f64, d: f64| if v != 0.0 { v } else { d };
        Some(Sctm {
            dy0,
            dx0,
            ls_y,
            ls_x,
            fz0,
            fla: if g(6) > 0.0 { g(6) } else { 7.5 },
            fla_gain: g(7),
            sr_peak: if use_sr { g(60) } else { 0.0 },
            sr_gain: g(70),
            cf_x_mult: nz(g(8), 1.0),
            brake_dx_mod: g(9),
            combined_exp: g(10),
            brush_exp: if g(23) >= 1.0 { g(23) } else { 3.0 },
            new_combined: g(73) != 0.0,
            fx_shift_sa: g(74),
            fx_scale_sa: g(75),
            fy_shift_sr: g(76),
            fy_scale_sr: g(77),
            post_y: [g(80), g(81), g(82)],
            post_x: [g(83), g(84), g(85)],
            falloff_y: fy,
            falloff_x: fx,
        })
    }

    /// Peak friction at load (the game's K Fz^ls / Fz, K = D0 Fz0 / Fz0^ls).
    pub fn d_y(&self, fz: f64) -> f64 {
        self.dy0 * (fz / self.fz0).powf(self.ls_y - 1.0)
    }
    pub fn d_x(&self, fz: f64) -> f64 {
        self.dx0 * (fz / self.fz0).powf(self.ls_x - 1.0)
    }

    /// tan of the peak slip angle at load
    pub fn tan_peak(&self, fz: f64) -> f64 {
        let dl = (fz - self.fz0) / self.fz0;
        let t0 = self.fla.to_radians().tan();
        let t1 = (self.fla * (1.0 + self.fla_gain)).to_radians().tan();
        (t1 - t0) * dl + t0
    }

    fn brush(&self, d: f64, k: f64, s: f64, post: [f64; 3]) -> f64 {
        if s > 1.0 / k {
            let [min, rate, exp] = post;
            if rate > 0.0 && exp > 0.0 {
                return ((1.0 - min) / (((s - 1.0 / k) * rate).powf(exp) + 1.0) + min) * d;
            }
            return d;
        }
        let x = 1.0 - k * s;
        (1.0 - x.abs().powf(self.brush_exp) * x.signum()) * d
    }

    /// (Fy, Fx) in N at load `fz`, slip angle `alpha` (rad), slip ratio
    /// `sr` (negative braking), road speed `speed` m/s.
    pub fn force(&self, fz: f64, alpha: f64, sr: f64, speed: f64) -> (f64, f64) {
        if fz <= 0.0 || (alpha == 0.0 && sr == 0.0) {
            return (0.0, 0.0);
        }
        let mut dy = self.d_y(fz);
        let mut dx = self.d_x(fz);
        if sr < 0.0 {
            dx *= 1.0 + self.brake_dx_mod;
        }
        let v_slip = ((speed * alpha.sin()).powi(2) + (alpha.cos() * speed * sr).powi(2)).sqrt();
        dy = self.falloff_y.apply(v_slip, dy);
        dx = self.falloff_x.apply(v_slip, dx);
        let dl = (fz - self.fz0) / self.fz0;
        let ky = 1.0 / self.tan_peak(fz);
        let kx = if self.sr_peak > 0.0 { 1.0 / ((self.sr_peak * self.sr_gain) * dl + self.sr_peak) } else { self.cf_x_mult * ky };
        let sy = alpha.sin() / alpha.cos().abs().max(0.2);
        let sx = sr;
        let n = self.combined_exp;
        let comb = |a: f64, b: f64| if n <= 0.0 || n == 2.0 { (a * a + b * b).sqrt() } else { (a.abs().powf(n) + b.abs().powf(n)).powf(1.0 / n) };
        let s = comb(sx, sy);
        if s <= 0.0 {
            return (0.0, 0.0);
        }
        if !self.new_combined {
            return (self.brush(dy, ky, s, self.post_y) * (sy / s) * fz, self.brush(dx, kx, s, self.post_x) * (sx / s) * fz);
        }
        let ys = comb(self.fy_shift_sr * sx, sy);
        let xs = comb(sx, self.fx_shift_sa * sy);
        let fy = if ys > 0.0 { (1.0 - (sx.abs() * self.fy_scale_sr).min(0.99)) * self.brush(dy, ky, s, self.post_y) * (sy / ys) * fz } else { 0.0 };
        let fx = if xs > 0.0 { (1.0 - (sy.abs() * self.fx_scale_sa).min(0.99)) * self.brush(dx, kx, s, self.post_x) * (sx / xs) * fz } else { 0.0 };
        (fy, fx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn tyre() -> Sctm {
        let f = Falloff { mu0t: 0.9, sens: 0.01, t_sens: 0.4, t_ref: 70.0, ref_speed: 2.0 };
        Sctm {
            dy0: 1.5, dx0: 1.6, ls_y: 0.85, ls_x: 0.85, fz0: 4000.0, fla: 8.0, fla_gain: 0.125, sr_peak: 0.0, sr_gain: 0.0,
            cf_x_mult: 1.0, brake_dx_mod: 0.0, combined_exp: 0.0, brush_exp: 3.0, new_combined: false,
            fx_shift_sa: 0.0, fx_scale_sa: 0.0, fy_shift_sr: 0.0, fy_scale_sr: 0.0, post_y: [0.0; 3], post_x: [0.0; 3],
            falloff_y: f, falloff_x: f,
        }
    }

    #[test]
    fn peaks_at_d_fz_at_the_limit_angle() {
        let t = tyre();
        let (fy, _) = t.force(4000.0, 8f64.to_radians(), 0.0, 0.0);
        // the brush peaks at s = tan(fla); sin/max(cos) ~ tan below 78 deg
        assert!((fy - 6000.0).abs() < 5.0, "{fy}");
        let (fy2, _) = t.force(4000.0, 4f64.to_radians(), 0.0, 0.0);
        assert!(fy2 < fy && fy2 > 0.7 * fy);
    }

    #[test]
    fn falls_past_the_peak() {
        let t = Sctm { post_y: [0.75, 5.0, 1.5], ..tyre() };
        let (p, _) = t.force(4000.0, 8f64.to_radians(), 0.0, 0.0);
        let (d, _) = t.force(4000.0, 40f64.to_radians(), 0.0, 0.0);
        assert!(d < 0.9 * p && d > 0.7 * p, "{p} {d}");
    }
}
