//! Tyres: one EVO compound (`TyreCompound_Mod_<n>`) per source compound,
//! front and rear (one file for both when the axles are identical).
//!
//! EVO's tyres are generated per class (eco, road, supercar, hypercar,
//! racing slicks, F1). The converter starts from the class's stock tyre
//! closest in size (the template: its thermal, wear, pressure, speed
//! sensitivity, combined-slip form and brush exponent stay) and FITS the
//! force-curve fields to the source tyre (`tyrefit`): Dy0 / Dx0, the load
//! exponents, the peak slip angle and its load gain, the post-peak falloff
//! and the brake modifier, so the EVO tyre reproduces the source's
//! lateral and longitudinal force against slip at 0.4 .. 1.9 x the static
//! wheel load. Fz0 becomes the static wheel load.
//!
//! The grip level target is the source's x GRIP_K (OURS, docs/FORMATS.md,
//! "Car physics"): on 26 axles of the stock twins the EVO tyre's lateral
//! mu at 3 kN is 0.978 x AC1's (median; most within +-5 %). The class is
//! chosen by that target mu and the source name (slicks).
//!
//! Before the fit (master 18b6d27) only Dy0 / Dx0 were scaled so mu at the
//! static load matched; the template's peak slip angle, load sensitivity
//! and falloff stayed (`scaled_template` keeps that rule for reports).

use anyhow::Result;

use super::pbx::{self, f};
use super::sctm::Sctm;
use super::Ctx;
use crate::io::pb::{Msg, Val};
use crate::source::{Shape, Tyre};

/// EVO lateral mu over source lateral mu (OURS, from the stock twins).
pub const GRIP_K: f64 = 0.98;

/// Lateral / longitudinal peak mu of a source tyre at a load (from its curve).
pub fn source_mu(t: &Tyre, fz: f64) -> (f64, f64) {
    let mut my = 0.0f64;
    let mut a = 0.5f64;
    while a < 30.0 {
        my = my.max(t.curve.force(fz, a.to_radians(), 0.0, super::tyrefit::SPEED).0);
        a += 0.25;
    }
    let mut mx = 0.0f64;
    let mut s = 0.005f64;
    while s < 0.6 {
        mx = mx.max(t.curve.force(fz, 0.0, s, super::tyrefit::SPEED).1);
        s *= 1.05;
    }
    (my / fz, mx / fz)
}

/// The EVO class directory for a compound.
pub fn class_of(name: &str, mu: f64) -> &'static str {
    let n = name.to_lowercase();
    let slick = n.contains("slick") && !n.contains("semi");
    if mu >= 1.70 {
        "f1_2004"
    } else if slick || mu >= 1.45 {
        "racing_slicks"
    } else if mu < 1.12 {
        "eco"
    } else if mu < 1.32 {
        "road"
    } else if mu < 1.40 {
        "supercar"
    } else {
        "hypercar"
    }
}

pub struct Template {
    pub path: String,
    pub msg: Msg,
}

fn size_of(m: &Msg) -> (f64, f64, f64) {
    let t = m.sub(3).unwrap_or_default();
    (pbx::rf(&t, 1).unwrap_or(0.2), pbx::rf(&t, 2).unwrap_or(0.3), pbx::rf(&t, 6).unwrap_or(0.2))
}

/// The stock tyre of `class` closest in size; offline: the donor's.
pub fn template(files: &crate::files::Files, base: &str, game: Option<&crate::io::kspkg::Pkg>, class: &str, t: &Tyre) -> Option<Template> {
    let score = |m: &Msg| {
        let (w, r, rim) = size_of(m);
        ((w - t.width) / 0.05).powi(2) + ((r - t.radius) / 0.02).powi(2) + ((rim - t.rim_radius) / 0.0254).powi(2)
    };
    let mut cands: Vec<(String, Vec<u8>)> = Vec::new();
    if let Some(g) = game {
        let pre = format!("content\\cars\\common_phsx\\tyres\\{class}\\");
        for e in g.entries.iter().filter(|e| e.path.to_lowercase().starts_with(&pre) && e.path.to_lowercase().ends_with(".tyre")) {
            if let Ok(b) = g.read(e) {
                cands.push((e.path.clone(), b));
            }
        }
    }
    if cands.is_empty() {
        // offline: the donor's own (eco / road / supercar 215/35 R18)
        let alt = match class { "eco" => "eco", "road" => "road", _ => "supercar" };
        let pre = format!("{base}\\tyres\\{alt}\\").to_lowercase();
        for (p, b) in files.map.iter().filter(|(k, _)| k.to_lowercase().starts_with(&pre) && k.to_lowercase().ends_with(".tyre")) {
            cands.push((p.clone(), b.clone()));
        }
    }
    let mut best: Option<(f64, Template)> = None;
    for (path, b) in cands {
        if let Ok(m) = Msg::parse(&b) {
            let s = score(&m);
            if best.as_ref().map(|(bs, _)| s < *bs).unwrap_or(true) {
                best = Some((s, Template { path, msg: m }));
            }
        }
    }
    best.map(|b| b.1)
}

/// Every `content\...\*.curve` string in a file.
fn curve_refs(b: &[u8]) -> Vec<String> {
    let re = regex::bytes::Regex::new(r"(?i)content\\[ -~]*?\.curve").unwrap();
    re.find_iter(b).map(|m| String::from_utf8_lossy(m.as_bytes()).into_owned()).collect()
}

/// The template's forms for what the source leaves undocumented.
pub fn shape_of(t: &Sctm) -> Shape {
    let ky = 1.0 / t.tan_peak(t.fz0);
    let kx = if t.sr_peak > 0.0 { 1.0 / t.sr_peak } else { t.cf_x_mult * ky };
    Shape {
        brush_exp: t.brush_exp,
        post_exp_y: if t.post_y[2] > 0.0 { t.post_y[2] } else { 1.0 },
        post_exp_x: if t.post_x[2] > 0.0 { t.post_x[2] } else { 1.0 },
        kx_over_ky: kx / ky,
    }
}

/// The pre-fit rule (master 18b6d27): the template with Dy0 / Dx0 scaled so
/// mu at the static load is GRIP_K x the source's. For reports.
pub fn scaled_template(t: &Sctm, src: &Tyre, fz: f64) -> Sctm {
    let (my, mx) = source_mu(src, fz);
    let mut o = *t;
    o.dy0 *= GRIP_K * my / t.d_y(fz);
    o.dx0 *= GRIP_K * mx / t.d_x(fz);
    o
}

/// The fitted compound and how close it came.
pub struct Fitted {
    pub path: String,
    pub rate: f64,
    pub class: &'static str,
    pub template: String,
    pub sctm: Sctm,
    pub before: Sctm,
    pub err: super::tyrefit::Error,
    pub err_before: super::tyrefit::Error,
}

/// Fit one source tyre onto `class`'s nearest template; the EVO tyre file.
pub fn fit_one(files: &crate::files::Files, base: &str, game: Option<&crate::io::kspkg::Pkg>, a: &mut Tyre, fz: f64, class: &'static str) -> Result<(Template, Fitted)> {
    let t = template(files, base, game, class, a).ok_or_else(|| anyhow::anyhow!("no tyre template for class {class} (no game content and no donor tyre)"))?;
    let tm = Sctm::from_compound(&t.msg).ok_or_else(|| anyhow::anyhow!("template {} has no tyre model", t.path))?;
    a.curve.set_shape(&shape_of(&tm));
    let (fit, err) = super::tyrefit::fit(a.curve.as_ref(), GRIP_K, fz, &tm);
    let before = scaled_template(&tm, a, fz);
    let err_before = super::tyrefit::error(a.curve.as_ref(), GRIP_K, fz, &before);
    let path = t.path.clone();
    Ok((t, Fitted { path: String::new(), rate: 0.0, class, template: path, sctm: fit, before, err, err_before }))
}

fn build(c: &mut Ctx, a: &mut Tyre, fz: f64, file: &str, class: &'static str) -> Result<Fitted> {
    let (t, mut out) = fit_one(c.files, &c.base, c.game.as_ref(), a, fz, class)?;
    let fit = out.sctm;
    let dir = format!("{}\\tyres\\{class}", c.base);
    // the template's curves (wear, thermal performance) move into the car folder
    let tbytes = t.msg.encode();
    let mut pairs: Vec<(String, String)> = Vec::new();
    for r in curve_refs(&tbytes) {
        let name = r.rsplit('\\').next().unwrap_or("x.curve").to_lowercase();
        let dst = format!("{dir}\\{name}");
        if !r.eq_ignore_ascii_case(&dst) {
            if let Some(b) = c.read_any(&r) {
                c.put(&dst, b);
            }
            pairs.push((r, dst));
        }
    }
    let pr: Vec<(&str, &str)> = pairs.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let mut m = Msg::parse(&crate::donor::rename_content(file, &tbytes, &pr))?;
    m.set_str(1, &a.name);
    m.set_str(2, &a.short.chars().take(3).collect::<String>());
    let (tw, tr, _) = size_of(&t.msg);
    {
        let td = m.msg_mut(3)?;
        let ai = pbx::rf(td, 5).unwrap_or(1.5);
        f(td, 1, a.width);
        f(td, 2, a.radius);
        f(td, 6, a.rim_radius);
        // angular inertia scales with width x radius^2
        f(td, 5, ai * (a.width / tw) * (a.radius / tr).powi(2));
    }
    {
        let md = m.msg_mut(4)?;
        for (num, v) in [(1u32, fit.dy0), (2, fit.dx0), (3, fit.ls_y), (4, fit.ls_x), (5, fit.fz0), (6, fit.fla), (7, fit.fla_gain), (9, fit.brake_dx_mod)] {
            f(md, num, v);
        }
        for (k, v) in fit.post_y.iter().chain(fit.post_x.iter()).enumerate() {
            f(md, 80 + k as u32, *v);
        }
        md.fields.sort_by_key(|x| x.num);
    }
    {
        let init = m.msg_mut(12)?;
        f(init, 1, (a.width * 1000.0).round());
        f(init, 2, ((a.radius - a.rim_radius) / a.width * 100.0).round().clamp(20.0, 90.0));
        f(init, 3, (a.rim_radius * 2.0 / 0.0254).round());
    }
    out.rate = m.sub(3).and_then(|t| pbx::rf(&t, 3)).unwrap_or(280000.0);
    out.path = format!("{dir}\\{file}");
    c.put(&out.path, m.encode());
    let (e, b) = (&out.err, &out.err_before);
    c.note(format!(
        "physics: tyre {file} fitted on {}: lateral RMS {:.1} % (template rule {:.1}), peak {:+.1} % at {:+.2} deg, falloff {:+.1} %, load mu {:+.1} / {:+.1} %; braking peak {:+.1} % (was {:+.1})",
        t.path, e.rms_y, b.rms_y, e.peak_y, e.peak_alpha, e.falloff_y, e.mu_y_light, e.mu_y_heavy, e.peak_x_brake, b.peak_x_brake
    ));
    Ok(out)
}

pub fn write(c: &mut Ctx) -> Result<()> {
    let fz = [c.src.wheel_load(0), c.src.wheel_load(1)];
    let mut comps = std::mem::take(&mut c.src.compounds);
    if comps.is_empty() {
        anyhow::bail!("the source car has no tyre compound (tyres.ini [FRONT] / [REAR])");
    }
    // the game's mod compounds are TyreCompound_Mod_1..10
    if comps.len() > 10 {
        c.note(format!("physics: {} source compounds; the first 10 are kept", comps.len()));
        comps.truncate(10);
    }
    let default = c.src.default_compound.min(comps.len() - 1);
    let cg = c.src.cg_front;
    let mut physic: Vec<(String, String)> = Vec::new();
    let mut fits: Vec<[Fitted; 2]> = Vec::new();
    let slug = |s: &str| s.to_lowercase().chars().map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' }).collect::<String>();
    for (k, comp) in comps.iter_mut().enumerate() {
        let [fa, ra] = &mut comp.tyres;
        let (mf, _) = source_mu(fa, fz[0]);
        let (mr, _) = source_mu(ra, fz[1]);
        // one class per compound (both axles), from the weight-averaged target mu
        let class = class_of(&fa.name, GRIP_K * (mf * cg + mr * (1.0 - cg)));
        let stem = c.stem.clone();
        let ff = build(c, fa, fz[0], &format!("{stem}_{k}_{}_front.tyre", slug(&fa.short)), class)?;
        let fr = build(c, ra, fz[1], &format!("{stem}_{k}_{}_rear.tyre", slug(&ra.short)), class)?;
        c.note(format!("physics: compound {k} {:?}: {}, mu_y {:.3} / {:.3} (source {:.3} / {:.3} at {:.0} / {:.0} N)",
            fa.name, class, ff.sctm.d_y(fz[0]), fr.sctm.d_y(fz[1]), mf, mr, fz[0], fz[1]));
        if k == default {
            c.d.tyre_radius = [fa.radius, ra.radius];
            c.d.mu_y = [ff.sctm.d_y(fz[0]), fr.sctm.d_y(fz[1])];
            c.d.mu_x = [ff.sctm.d_x(fz[0]), fr.sctm.d_x(fz[1])];
            c.d.tyre_rate = [ff.rate, fr.rate];
        }
        physic.push((ff.path.clone(), fr.path.clone()));
        fits.push([ff, fr]);
    }
    c.src.compounds = comps;
    c.fits = fits;
    c.d.compounds = physic.len();
    c.d.default_compound = default;
    // CarData: the default compound's files
    c.cd.remove(10);
    c.cd.remove(11);
    c.cd.push(10, Val::Bytes(physic[default].0.clone().into_bytes()));
    c.cd.push(11, Val::Bytes(physic[default].1.clone().into_bytes()));
    // compatible tyres: Mod_1.. in source order, each with the car's tyre textures
    let pre = format!("{}\\presets\\", c.base).to_lowercase();
    let ct: Vec<String> = c.files.map.keys().filter(|k| k.to_lowercase().starts_with(&pre) && k.to_lowercase().ends_with(".compatibletyres")).cloned().collect();
    for p in ct {
        let mut m = Msg::parse(c.files.get(&p).unwrap())?;
        let tex = m.subs(3).first().and_then(|g| g.str(2));
        m.remove(2);
        m.remove(3);
        for (k, (pf, pr)) in physic.iter().enumerate() {
            let mut e = Msg::default();
            pbx::i(&mut e, 1, 5001 + k as i64);
            e.set_str(2, pf);
            e.set_str(3, pr);
            m.push(2, Val::Msg(e));
            let mut g = Msg::default();
            pbx::i(&mut g, 1, 5001 + k as i64);
            if let Some(t) = &tex {
                g.set_str(2, t);
            }
            m.push(3, Val::Msg(g));
        }
        c.files.put(&p, m.encode());
    }
    // the mechanical preset's default compound
    let mp: Vec<String> = c.files.map.keys().filter(|k| k.to_lowercase().starts_with(&pre) && k.to_lowercase().ends_with(".mechanicalcarpreset")).cloned().collect();
    for p in mp {
        let mut m = Msg::parse(c.files.get(&p).unwrap())?;
        pbx::i(&mut m, 7, 5001 + default as i64);
        c.files.put(&p, m.encode());
    }
    Ok(())
}
