//! Tyres: one EVO compound (`TyreCompound_Mod_<n>`) per AC1 compound,
//! front and rear (one file for both when AC1's axles are identical).
//!
//! EVO's tyres are generated per class (eco, road, supercar, hypercar,
//! racing slicks, F1): inside a class most parameters are shared and the
//! grip (`Dy0` / `Dx0` at `Fz0`, load exponents `lsExpY` / `lsExpX`) grows
//! with the size. The load law is AC1's: mu(Fz) = D x (Fz / Fz0)^(ls - 1).
//!
//! Calibration (OURS, docs/FORMATS.md, "Car physics"): on 26 axles of the calibration pairs
//! the stock twin's lateral mu at 3 kN is 0.978 x AC1's (median; most
//! within +-5 %; the outliers are twins Kunos moved to another class, ae86
//! street -> road +16 %, 190E slicks -> road -13 %). So a converted
//! compound keeps the template's model and gets
//!   mu_evo(Fz_static) = 0.98 x mu_ac1(Fz_static)
//! on both axes, Dy0 / Dx0 solved for at the car's own static wheel load.
//! The class is chosen by that target mu and the AC1 name (slicks); the
//! template is the class's stock tyre closest in size; width, radius, rim
//! and the init data come from tyres.ini. Pressures, thermal and wear
//! models stay the template's (they belong to EVO's thermal model, which
//! AC1's numbers do not describe).

use anyhow::Result;

use super::pbx::{self, f};
use super::{Ctx, G};
use crate::ac1::ini::Ini;
use crate::io::pb::{Msg, Val};

/// EVO lateral mu over AC1 lateral mu, both at the static load (OURS, from the pairs).
pub const GRIP_K: f64 = 0.98;

#[derive(Clone, Debug)]
struct Ax {
    name: String,
    short: String,
    width: f64,
    radius: f64,
    rim: f64,
    mu_y: f64,
    mu_x: f64,
}

fn read_ax(t: &Ini, s: &str, fz: f64) -> Option<Ax> {
    if !t.has(s) {
        return None;
    }
    let g = |k: &str| t.num(s, k);
    let radius = g("RADIUS").filter(|v| *v > 0.1 && *v < 1.0)?;
    let width = g("WIDTH").filter(|v| *v > 0.05 && *v < 1.0).unwrap_or(0.2);
    let rim = g("RIM_RADIUS").filter(|v| *v > 0.05 && *v < radius).unwrap_or(radius - 0.1);
    let fz0 = g("FZ0").filter(|v| *v > 100.0).unwrap_or(fz);
    let mu = |dref: Option<f64>, d0: Option<f64>, d1: Option<f64>, ls: Option<f64>| -> f64 {
        match dref {
            Some(d) => d * (fz / fz0).powf(ls.unwrap_or(0.8) - 1.0),
            // pre-v10 tyres: D = DY0 + DY1 x Fz (kN)
            None => d0.unwrap_or(1.2) + d1.unwrap_or(0.0) * fz / 1000.0,
        }
    };
    let mu_y = mu(g("DY_REF"), g("DY0"), g("DY1"), g("LS_EXPY"));
    let mu_x = mu(g("DX_REF"), g("DX0"), g("DX1"), g("LS_EXPX"));
    Some(Ax {
        name: t.get(s, "NAME").unwrap_or("Tyre").to_string(),
        short: t.get(s, "SHORT_NAME").unwrap_or("T").to_string(),
        width,
        radius,
        rim,
        mu_y,
        mu_x,
    })
}

/// The EVO class directory for a compound.
fn class_of(name: &str, mu: f64) -> &'static str {
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

/// AC1 rims are one inch over nominal (tyres.ini: "use 1 inch more than
/// nominal"); EVO stores the nominal rim (14" = 0.1778).
fn nominal_rim(rim: f64) -> f64 {
    let d = 2.0 * rim / 0.0254;
    let nom = (d - 1.0).round();
    if (d - 1.0 - nom).abs() < 0.3 { nom * 0.0254 / 2.0 } else { rim }
}

struct Template {
    path: String,
    msg: Msg,
}

fn size_of(m: &Msg) -> (f64, f64, f64) {
    let t = m.sub(3).unwrap_or_default();
    (pbx::rf(&t, 1).unwrap_or(0.2), pbx::rf(&t, 2).unwrap_or(0.3), pbx::rf(&t, 6).unwrap_or(0.2))
}

/// The stock tyre of `class` closest in size; offline: the donor's.
fn template(c: &Ctx, class: &str, a: &Ax) -> Option<Template> {
    let score = |m: &Msg| {
        let (w, r, rim) = size_of(m);
        ((w - a.width) / 0.05).powi(2) + ((r - a.radius) / 0.02).powi(2) + ((rim - nominal_rim(a.rim)) / 0.0254).powi(2)
    };
    let mut cands: Vec<(String, Vec<u8>)> = Vec::new();
    if let Some(g) = &c.game {
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
        let pre = format!("{}\\tyres\\{alt}\\", c.base).to_lowercase();
        for (p, b) in c.files.map.iter().filter(|(k, _)| k.to_lowercase().starts_with(&pre) && k.to_lowercase().ends_with(".tyre")) {
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

fn mu_at(m: &Msg, fz: f64, x: bool) -> f64 {
    let md = m.sub(4).unwrap_or_default();
    let (d, ls) = if x { (pbx::rf(&md, 2).unwrap_or(1.2), pbx::rf(&md, 4).unwrap_or(0.85)) } else { (pbx::rf(&md, 1).unwrap_or(1.2), pbx::rf(&md, 3).unwrap_or(0.85)) };
    let fz0 = pbx::rf(&md, 5).unwrap_or(4000.0);
    d * (fz / fz0).powf(ls - 1.0)
}

/// Build one EVO tyre file; returns (path, rate).
fn build(c: &mut Ctx, a: &Ax, fz: f64, file: &str, class: &'static str) -> Result<(String, f64, &'static str)> {
    let target_y = GRIP_K * a.mu_y;
    let target_x = GRIP_K * a.mu_x;
    let t = template(c, class, a).ok_or_else(|| anyhow::anyhow!("no tyre template for class {class} (no game content and no donor tyre)"))?;
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
    let rim = nominal_rim(a.rim);
    m.set_str(1, &a.name);
    m.set_str(2, &a.short.chars().take(3).collect::<String>());
    let (tw, tr, _) = size_of(&t.msg);
    {
        let td = m.msg_mut(3)?;
        let ai = pbx::rf(td, 5).unwrap_or(1.5);
        f(td, 1, a.width);
        f(td, 2, a.radius);
        f(td, 6, rim);
        // angular inertia scales with width x radius^2
        f(td, 5, ai * (a.width / tw) * (a.radius / tr).powi(2));
    }
    // grip: solve D at the static load for the target mu
    let cur_y = mu_at(&m, fz, false);
    let cur_x = mu_at(&m, fz, true);
    {
        let md = m.msg_mut(4)?;
        let dy = pbx::rf(md, 1).unwrap_or(1.2);
        let dx = pbx::rf(md, 2).unwrap_or(1.2);
        f(md, 1, dy * target_y / cur_y);
        f(md, 2, dx * target_x / cur_x);
    }
    {
        let init = m.msg_mut(12)?;
        f(init, 1, (a.width * 1000.0).round());
        f(init, 2, ((a.radius - rim) / a.width * 100.0).round().clamp(20.0, 90.0));
        f(init, 3, (rim * 2.0 / 0.0254).round());
    }
    let rate = m.sub(3).and_then(|t| pbx::rf(&t, 3)).unwrap_or(280000.0);
    let path = format!("{dir}\\{file}");
    c.put(&path, m.encode());
    c.note(format!("physics: tyre {file} from template {}", t.path));
    Ok((path, rate, class))
}

pub fn write(c: &mut Ctx) -> Result<()> {
    let t = c.ini("tyres.ini").clone();
    let fz = [c.d.mass_static * G * c.d.cg_front / 2.0, c.d.mass_static * G * (1.0 - c.d.cg_front) / 2.0];
    let default = t.num("COMPOUND_DEFAULT", "INDEX").unwrap_or(0.0).max(0.0) as usize;
    let mut compounds: Vec<(Ax, Ax)> = Vec::new();
    for k in 0..16 {
        let (sf, sr) = if k == 0 { ("FRONT".to_string(), "REAR".to_string()) } else { (format!("FRONT_{k}"), format!("REAR_{k}")) };
        let (Some(fa), Some(ra)) = (read_ax(&t, &sf, fz[0]), read_ax(&t, &sr, fz[1])) else {
            if k == 0 {
                anyhow::bail!("tyres.ini has no [FRONT] / [REAR] compound");
            }
            continue;
        };
        compounds.push((fa, ra));
    }
    // the game's mod compounds are TyreCompound_Mod_1..10
    if compounds.len() > 10 {
        c.note(format!("physics: {} AC1 compounds; the first 10 are kept", compounds.len()));
        compounds.truncate(10);
    }
    let default = default.min(compounds.len() - 1);
    let mut physic: Vec<(String, String)> = Vec::new();
    for (k, (fa, ra)) in compounds.iter().enumerate() {
        let same = (fa.width - ra.width).abs() < 1e-4 && (fa.radius - ra.radius).abs() < 1e-4 && (fa.mu_y - ra.mu_y).abs() < 1e-3;
        let slug = |s: &str| s.to_lowercase().chars().map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' }).collect::<String>();
        // one class per compound (both axles), from the weight-averaged target mu
        let class = class_of(&fa.name, GRIP_K * (fa.mu_y * c.d.cg_front + ra.mu_y * (1.0 - c.d.cg_front)));
        let (pf, rate_f, cl_f) = build(c, fa, fz[0], &format!("{}_{k}_{}_front.tyre", c.stem, slug(&fa.short)), class)?;
        let (pr, rate_r, cl_r) = if same && (fz[0] - fz[1]).abs() / fz[0] < 0.15 {
            (pf.clone(), rate_f, cl_f)
        } else {
            build(c, ra, fz[1], &format!("{}_{k}_{}_rear.tyre", c.stem, slug(&ra.short)), class)?
        };
        c.note(format!(
            "physics: compound {k} {:?}: {} / {}, mu_y {:.3} / {:.3} (AC1 {:.3} / {:.3} at {:.0} / {:.0} N)",
            fa.name, cl_f, cl_r, GRIP_K * fa.mu_y, GRIP_K * ra.mu_y, fa.mu_y, ra.mu_y, fz[0], fz[1]
        ));
        if k == default {
            c.d.tyre_radius = [fa.radius, ra.radius];
            c.d.mu_y = [GRIP_K * fa.mu_y, GRIP_K * ra.mu_y];
            c.d.mu_x = [GRIP_K * fa.mu_x, GRIP_K * ra.mu_x];
            c.d.tyre_rate = [rate_f, rate_r];
        }
        physic.push((pf, pr));
    }
    c.d.compounds = physic.len();
    c.d.default_compound = default;
    // CarData: the default compound's files
    c.cd.remove(10);
    c.cd.remove(11);
    c.cd.push(10, Val::Bytes(physic[default].0.clone().into_bytes()));
    c.cd.push(11, Val::Bytes(physic[default].1.clone().into_bytes()));
    // compatible tyres: Mod_1.. in AC1 order, each with the car's tyre textures
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
