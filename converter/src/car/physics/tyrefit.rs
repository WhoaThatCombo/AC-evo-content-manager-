//! Fit EVO's tyre model (SCTM) to a source tyre's force curves.
//!
//! The source curve (any `source::TyreCurve`) is sampled at five loads
//! around the axle's static wheel load (0.4 .. 1.9 x, covering load
//! transfer and aero), pure lateral slip 0.5 .. 80 deg and pure
//! longitudinal slip under braking and traction, all at 20 m/s road speed
//! (EVO's slip-speed falloff is active, as in a corner). The EVO compound
//! the converter starts from (the class template) keeps everything the
//! fit does not set: brush exponent, combined-slip form, thermal, wear,
//! pressure, speed sensitivity. Fitted (Nelder-Mead on the squared force
//! error, normalised by the source's peak at each load):
//!   Dy0, lsExpY, friction_limit_angle, its load gain, lateral
//!   falloff_level / falloff_slope; Dx0, lsExpX, longitudinal falloff
//!   level / slope, brakeDXMod; Fz0 is set to the static wheel load.
//! Where the source does not fix the longitudinal slip stiffness
//! (`long_stiffness_known`), longitudinal force is compared in slip
//! normalised to each curve's own peak and the template's stiffness stays.

use super::sctm::Sctm;
use crate::source::TyreCurve;

pub const SPEED: f64 = 20.0;
const LOADS: [f64; 5] = [0.4, 0.7, 1.0, 1.4, 1.9];
/// lateral samples (deg); the deep ones (slides, drifts) at a lower weight
/// keep the post-peak fall from collapsing past the sampled range
const ALPHAS: [f64; 18] = [0.5, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 12.0, 15.0, 20.0, 28.0, 40.0, 60.0, 80.0];
/// longitudinal samples in units of the peak slip ratio
const SR_U: [f64; 12] = [0.1, 0.25, 0.5, 0.75, 0.9, 1.0, 1.1, 1.3, 1.6, 2.0, 3.0, 5.0];

/// How close an EVO tyre is to the source, at the axle's static load.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Error {
    /// RMS of (F_evo - F_src) / (peak F_src) over the whole grid, %
    pub rms_y: f64,
    pub rms_x: f64,
    /// peak lateral force at the static load, EVO / source - 1, %
    pub peak_y: f64,
    /// slip angle at the peak, EVO - source, deg
    pub peak_alpha: f64,
    /// force at 2.5 x the source's peak slip angle, EVO / source - 1, %
    pub falloff_y: f64,
    /// lateral mu at 0.5 x and 1.6 x the static load, EVO / source - 1, %
    pub mu_y_light: f64,
    pub mu_y_heavy: f64,
    /// peak braking force at the static load, EVO / source - 1, %
    pub peak_x_brake: f64,
    pub peak_x_drive: f64,
}

fn lat(t: &dyn Fn(f64, f64, f64) -> (f64, f64), fz: f64, deg: f64) -> f64 {
    t(fz, deg.to_radians(), 0.0).0
}

/// (peak |Fx|, slip ratio there) on one side (sign -1 braking)
fn long_peak(t: &dyn Fn(f64, f64, f64) -> (f64, f64), fz: f64, sign: f64) -> (f64, f64) {
    let mut best = (0.0, 0.05);
    let mut s = 0.002;
    while s < 0.6 {
        let f = t(fz, 0.0, sign * s).1.abs();
        if f > best.0 {
            best = (f, s);
        }
        s *= 1.04;
    }
    best
}

fn lat_peak(t: &dyn Fn(f64, f64, f64) -> (f64, f64), fz: f64) -> (f64, f64) {
    let mut best = (0.0, 1.0);
    let mut a = 0.2;
    while a < 45.0 {
        let f = lat(t, fz, a);
        if f > best.0 {
            best = (f, a);
        }
        a += 0.05;
    }
    best
}

struct Target {
    fz: Vec<f64>,
    /// per load: lateral samples, peak
    y: Vec<(Vec<f64>, f64)>,
    /// per load, per side (brake, drive): peak slip, samples
    x: Vec<[(f64, Vec<f64>); 2]>,
}

fn sample(src: &dyn Fn(f64, f64, f64) -> (f64, f64), fz_static: f64) -> Target {
    let fz: Vec<f64> = LOADS.iter().map(|k| k * fz_static).collect();
    let y = fz.iter().map(|&f| {
        let v: Vec<f64> = ALPHAS.iter().map(|&a| lat(src, f, a)).collect();
        (v, lat_peak(src, f).0)
    }).collect();
    let x = fz.iter().map(|&f| {
        [-1.0, 1.0].map(|sg| {
            let (_, sp) = long_peak(src, f, sg);
            let srs: Vec<f64> = SR_U.iter().map(|u| u * sp).collect();
            (sp, srs.iter().map(|&s| src(f, 0.0, sg * s).1.abs()).collect())
        })
    }).collect();
    Target { fz, y, x }
}

/// Sum of squared normalised errors of `evo` against the target (and the parts).
fn cost(t: &Target, evo: &Sctm, norm_x: bool) -> (f64, f64, f64) {
    let e = |fz: f64, a: f64, s: f64| evo.force(fz, a, s, SPEED);
    let (mut cy, mut cx, mut ny, mut nx) = (0.0, 0.0, 0.0, 0.0);
    for (i, &fz) in t.fz.iter().enumerate() {
        let (ys, peak) = &t.y[i];
        for (k, &a) in ALPHAS.iter().enumerate() {
            let w = if a > 45.0 { 0.3 } else if a > 20.0 { 0.5 } else { 1.0 };
            cy += w * ((lat(&e, fz, a) - ys[k]) / peak).powi(2);
            ny += w;
        }
        for (side, sg) in [-1.0, 1.0].iter().enumerate() {
            let (sp_src, xs) = &t.x[i][side];
            let peak = xs.iter().cloned().fold(1.0, f64::max);
            let sp = if norm_x { long_peak(&e, fz, *sg).1 } else { *sp_src };
            for (k, u) in SR_U.iter().enumerate() {
                cx += ((e(fz, 0.0, sg * u * sp).1.abs() - xs[k]) / peak).powi(2);
                nx += 1.0;
            }
        }
    }
    (cy + 0.7 * cx, (cy / ny).sqrt(), (cx / nx).sqrt())
}

/// The fitted parameters, in fixed order.
fn decode(base: &Sctm, p: &[f64]) -> Sctm {
    let mut s = *base;
    s.dy0 = p[0].clamp(0.3, 3.5);
    s.ls_y = p[1].clamp(0.4, 1.1);
    s.fla = p[2].clamp(1.5, 20.0);
    s.fla_gain = p[3].clamp(-0.5, 1.0);
    s.post_y[0] = p[4].clamp(0.3, 1.0);
    s.post_y[1] = p[5].clamp(0.0, 30.0);
    s.dx0 = p[6].clamp(0.3, 3.5);
    s.ls_x = p[7].clamp(0.4, 1.2);
    s.post_x[0] = p[8].clamp(0.3, 1.0);
    s.post_x[1] = p[9].clamp(0.0, 30.0);
    s.brake_dx_mod = p[10].clamp(-0.3, 0.5);
    if s.post_y[2] <= 0.0 {
        s.post_y[2] = 1.0;
    }
    if s.post_x[2] <= 0.0 {
        s.post_x[2] = 1.0;
    }
    s
}

/// Nelder-Mead, minimising `f` from `x0` with initial steps `step`.
pub fn nelder_mead(f: &dyn Fn(&[f64]) -> f64, x0: &[f64], step: &[f64], iters: usize) -> Vec<f64> {
    let n = x0.len();
    let mut pts: Vec<(Vec<f64>, f64)> = (0..=n).map(|i| {
        let mut x = x0.to_vec();
        if i > 0 {
            x[i - 1] += step[i - 1];
        }
        let v = f(&x);
        (x, v)
    }).collect();
    for _ in 0..iters {
        pts.sort_by(|a, b| a.1.total_cmp(&b.1));
        let c: Vec<f64> = (0..n).map(|j| pts[..n].iter().map(|p| p.0[j]).sum::<f64>() / n as f64).collect();
        let at = |t: f64| -> Vec<f64> { (0..n).map(|j| c[j] + t * (pts[n].0[j] - c[j])).collect() };
        let xr = at(-1.0);
        let fr = f(&xr);
        if fr < pts[0].1 {
            let xe = at(-2.0);
            let fe = f(&xe);
            pts[n] = if fe < fr { (xe, fe) } else { (xr, fr) };
        } else if fr < pts[n - 1].1 {
            pts[n] = (xr, fr);
        } else {
            let xc = at(if fr < pts[n].1 { -0.5 } else { 0.5 });
            let fc = f(&xc);
            if fc < pts[n].1.min(fr) {
                pts[n] = (xc, fc);
            } else {
                let best = pts[0].0.clone();
                for p in pts.iter_mut().skip(1) {
                    p.0 = (0..n).map(|j| best[j] + 0.5 * (p.0[j] - best[j])).collect();
                    p.1 = f(&p.0);
                }
            }
        }
    }
    pts.sort_by(|a, b| a.1.total_cmp(&b.1));
    pts.swap_remove(0).0
}

/// Fit `template`'s fitted fields to `src` (scaled by `grip_k`) around the
/// static wheel load. Returns the fitted model and its error.
pub fn fit(src: &dyn TyreCurve, grip_k: f64, fz_static: f64, template: &Sctm) -> (Sctm, Error) {
    let s = |fz: f64, a: f64, sr: f64| {
        let (y, x) = src.force(fz, a, sr, SPEED);
        (y * grip_k, x * grip_k)
    };
    let norm_x = !src.long_stiffness_known();
    let t = sample(&s, fz_static);
    // a start from the source's own curve: mu and peak angle at three loads
    let (p1, a1) = lat_peak(&s, fz_static);
    let (p_lo, _) = lat_peak(&s, 0.7 * fz_static);
    let (p_hi, a_hi) = lat_peak(&s, 1.4 * fz_static);
    let mu = |p: f64, f: f64| p / f;
    let ls_y = 1.0 + (mu(p_hi, 1.4 * fz_static) / mu(p_lo, 0.7 * fz_static)).ln() / 2f64.ln();
    let (xb, _) = long_peak(&s, fz_static, 1.0);
    let (xb_hi, _) = long_peak(&s, 1.4 * fz_static, 1.0);
    let (xb_lo, _) = long_peak(&s, 0.7 * fz_static, 1.0);
    let ls_x = 1.0 + (mu(xb_hi, 1.4 * fz_static) / mu(xb_lo, 0.7 * fz_static)).ln() / 2f64.ln();
    let deep = lat(&s, fz_static, 40.0) / p1;
    let mut base = *template;
    base.fz0 = fz_static;
    let x0 = [
        mu(p1, fz_static), ls_y, a1, ((a_hi / a1 - 1.0) / 0.4).clamp(-0.4, 0.9), deep.clamp(0.35, 1.0), 3.0,
        mu(xb, fz_static), ls_x, deep.clamp(0.35, 1.0), 3.0,
        long_peak(&s, fz_static, -1.0).0 / xb - 1.0,
    ];
    let step = [0.05, 0.03, 0.5, 0.05, 0.05, 1.0, 0.05, 0.03, 0.05, 1.0, 0.02];
    let f = |p: &[f64]| cost(&t, &decode(&base, p), norm_x).0;
    let mut best = nelder_mead(&f, &x0, &step, 1500);
    // one restart from the result (the simplex collapses on ridges)
    best = nelder_mead(&f, &best, &step.map(|v| v * 0.3), 1500);
    let out = decode(&base, &best);
    let err = error(src, grip_k, fz_static, &out);
    (out, err)
}

/// The error of an EVO tyre against the source (scaled by `grip_k`).
pub fn error(src: &dyn TyreCurve, grip_k: f64, fz_static: f64, evo: &Sctm) -> Error {
    let s = |fz: f64, a: f64, sr: f64| {
        let (y, x) = src.force(fz, a, sr, SPEED);
        (y * grip_k, x * grip_k)
    };
    let e = |fz: f64, a: f64, sr: f64| evo.force(fz, a, sr, SPEED);
    let norm_x = !src.long_stiffness_known();
    let t = sample(&s, fz_static);
    let (_, rms_y, rms_x) = cost(&t, evo, norm_x);
    let (ps, as_) = lat_peak(&s, fz_static);
    let (pe, ae) = lat_peak(&e, fz_static);
    let pct = |a: f64, b: f64| (a / b - 1.0) * 100.0;
    let mu_at = |k: f64| pct(lat_peak(&e, k * fz_static).0, lat_peak(&s, k * fz_static).0);
    Error {
        rms_y: rms_y * 100.0,
        rms_x: rms_x * 100.0,
        peak_y: pct(pe, ps),
        peak_alpha: ae - as_,
        falloff_y: pct(lat(&e, fz_static, 2.5 * as_), lat(&s, fz_static, 2.5 * as_)),
        mu_y_light: mu_at(0.5),
        mu_y_heavy: mu_at(1.6),
        peak_x_brake: pct(long_peak(&e, fz_static, -1.0).0, long_peak(&s, fz_static, -1.0).0),
        peak_x_drive: pct(long_peak(&e, fz_static, 1.0).0, long_peak(&s, fz_static, 1.0).0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Evo(Sctm);
    impl TyreCurve for Evo {
        fn force(&self, fz: f64, a: f64, sr: f64, v: f64) -> (f64, f64) {
            self.0.force(fz, a, sr, v)
        }
        fn fz_ref(&self) -> f64 {
            self.0.fz0
        }
    }

    fn base() -> Sctm {
        let f = super::super::sctm::Falloff { mu0t: 0.9, sens: 0.01, t_sens: 0.4, t_ref: 70.0, ref_speed: 2.0 };
        Sctm {
            dy0: 1.2, dx0: 1.25, ls_y: 0.85, ls_x: 0.88, fz0: 4000.0, fla: 8.0, fla_gain: 0.02, sr_peak: 0.0, sr_gain: 0.0,
            cf_x_mult: 2.0, brake_dx_mod: 0.05, combined_exp: 0.0, brush_exp: 2.9, new_combined: false,
            fx_shift_sa: 0.0, fx_scale_sa: 0.0, fy_shift_sr: 0.0, fy_scale_sr: 0.0, post_y: [0.0; 3], post_x: [0.0; 3],
            falloff_y: f, falloff_x: f,
        }
    }

    #[test]
    fn recovers_a_known_evo_tyre() {
        // an EVO tyre as the source: the fit lands back on its parameters
        let truth = Sctm { dy0: 1.35, ls_y: 0.75, fla: 6.5, fla_gain: 0.1, post_y: [0.82, 4.0, 1.0], post_x: [0.85, 3.0, 1.0], fz0: 3000.0, ..base() };
        let (got, err) = fit(&Evo(truth), 1.0, 3000.0, &base());
        assert!(err.rms_y < 1.0 && err.rms_x < 2.0, "{err:?} {got:?}");
        assert!((got.dy0 - 1.35).abs() < 0.02 && (got.ls_y - 0.75).abs() < 0.03 && (got.fla - 6.5).abs() < 0.3, "{got:?}");
    }

    #[test]
    fn fits_an_ac1_tyre() {
        let ini = crate::ac1::ini::parse("[FRONT]\nRADIUS=0.2888\nWIDTH=0.185\nDY_REF=1.21\nDX_REF=1.22\nFZ0=1761\nLS_EXPY=0.7227\nLS_EXPX=0.7902\nFRICTION_LIMIT_ANGLE=9.45\nFLEX_GAIN=0.0318\nFALLOFF_LEVEL=0.86\nFALLOFF_SPEED=4\nBRAKE_DX_MOD=0.02\n");
        let t = crate::ac1::tyre::Ac1Tyre::read(&ini, "FRONT", 2400.0).unwrap();
        let (got, err) = fit(&t, 0.98, 2400.0, &base());
        assert!(err.rms_y < 2.0 && err.peak_y.abs() < 1.0 && err.peak_alpha.abs() < 0.5, "{err:?} {got:?}");
        assert!(err.mu_y_light.abs() < 2.0 && err.mu_y_heavy.abs() < 2.0, "{err:?}");
    }
}
