//! `aceconv source-fit`: the tyre fit of a source car's default compound
//! and the whole-car limits for the source, the pre-fit rule and the fit,
//! without converting anything.

use anyhow::{anyhow, Result};
use std::path::Path;

use super::handling::{self, Model};
use super::sctm::Sctm;
use super::tyres::{self, GRIP_K};
use crate::source::SourceCar;

/// `twin`: a stock EVO car id whose own default tyres are run through the
/// same whole-car model (the AC1 chassis on Kunos's EVO tyres).
pub fn report(src: &mut SourceCar, game: Option<&crate::io::kspkg::Pkg>, twin: Option<&str>) -> Result<serde_json::Value> {
    let fz = [src.wheel_load(0), src.wheel_load(1)];
    let k = src.default_compound;
    let cg = src.cg_front;
    let files = crate::files::Files::default();
    let comp = src.compounds.get_mut(k).ok_or_else(|| anyhow!("no tyre compound"))?;
    let (mf, _) = tyres::source_mu(&comp.tyres[0], fz[0]);
    let (mr, _) = tyres::source_mu(&comp.tyres[1], fz[1]);
    let class = tyres::class_of(&comp.tyres[0].name, GRIP_K * (mf * cg + mr * (1.0 - cg)));
    let mut axles = Vec::new();
    let mut evo: Vec<(Sctm, Sctm)> = Vec::new();
    for a in 0..2 {
        let (_, f) = tyres::fit_one(&files, "", game, &mut comp.tyres[a], fz[a], class)?;
        let s = &f.sctm;
        let b = &f.before;
        axles.push(serde_json::json!({
            "axle": if a == 0 { "front" } else { "rear" },
            "source": comp.tyres[a].notes, "template": f.template, "fz_static": fz[a],
            "err_after": f.err, "err_before": f.err_before,
            "after": {"dy0": s.dy0, "ls_y": s.ls_y, "fla": s.fla, "fla_gain": s.fla_gain, "post_y": s.post_y, "dx0": s.dx0, "ls_x": s.ls_x, "post_x": s.post_x, "brake_dx_mod": s.brake_dx_mod, "fz0": s.fz0},
            "before": {"dy0": b.dy0, "ls_y": b.ls_y, "fla": b.fla, "fla_gain": b.fla_gain, "post_y": b.post_y, "dx0": b.dx0, "ls_x": b.ls_x, "post_x": b.post_x, "brake_dx_mod": b.brake_dx_mod, "fz0": b.fz0},
        }));
        evo.push((f.before, f.sctm));
    }
    let m = Model::from_source(src);
    let comp = &src.compounds[k];
    let s0 = |fz: f64, al: f64, sr: f64, v: f64| comp.tyres[0].curve.force(fz, al, sr, v);
    let s1 = |fz: f64, al: f64, sr: f64, v: f64| comp.tyres[1].curve.force(fz, al, sr, v);
    let t0 = |fz: f64, al: f64, sr: f64, v: f64| { let (y, x) = s0(fz, al, sr, v); (y * GRIP_K, x * GRIP_K) };
    let t1 = |fz: f64, al: f64, sr: f64, v: f64| { let (y, x) = s1(fz, al, sr, v); (y * GRIP_K, x * GRIP_K) };
    let (b0, b1, a0, a1) = (evo[0].0, evo[1].0, evo[0].1, evo[1].1);
    let eb0 = |fz: f64, al: f64, sr: f64, v: f64| b0.force(fz, al, sr, v);
    let eb1 = |fz: f64, al: f64, sr: f64, v: f64| b1.force(fz, al, sr, v);
    let ea0 = |fz: f64, al: f64, sr: f64, v: f64| a0.force(fz, al, sr, v);
    let ea1 = |fz: f64, al: f64, sr: f64, v: f64| a1.force(fz, al, sr, v);
    let twin_tyres: Option<[Sctm; 2]> = twin.zip(game).and_then(|(id, g)| {
        let cd = crate::io::pb::Msg::parse(&g.read(g.find(&format!(r"content\cars\{id}\data\cardata.car"))?).ok()?).ok()?;
        let t = |num: u32| -> Option<Sctm> { Sctm::from_compound(&crate::io::pb::Msg::parse(&g.read(g.find(&cd.str(num)?)?).ok()?).ok()?) };
        Some([t(10)?, t(11)?])
    });
    let twin_limits = twin_tyres.map(|[w0, w1]| {
        let e0 = |fz: f64, al: f64, sr: f64, v: f64| w0.force(fz, al, sr, v);
        let e1 = |fz: f64, al: f64, sr: f64, v: f64| w1.force(fz, al, sr, v);
        handling::limits(&m, [&e0, &e1])
    });
    Ok(serde_json::json!({
        "twin": twin, "twin_limits": twin_limits,
        "car": src.name, "game": src.game, "compound": comp.tyres[0].name, "class": class,
        "mass_static": src.mass_static(), "cg_front": cg, "cg_height": src.cg_height,
        "roll_front": m.roll_front, "cl_area": m.cl_area, "brake_front": m.brake_front,
        "axles": axles,
        "speeds_kmh": handling::SPEEDS_KMH,
        "limits": {
            "source": handling::limits(&m, [&s0, &s1]),
            "target": handling::limits(&m, [&t0, &t1]),
            "before": handling::limits(&m, [&eb0, &eb1]),
            "after": handling::limits(&m, [&ea0, &ea1]),
        },
    }))
}

/// The report of an AC1 car folder.
pub fn report_ac1(dir: &Path, content: Option<&Path>, twin: Option<&str>) -> Result<serde_json::Value> {
    let car = crate::ac1::car::Car::open(dir)?;
    let k = car.kn5()?;
    let frame = crate::car::frame::Frame::build(&car, &k);
    let mut src = crate::source::ac1::read(&car, &frame);
    let game = content.filter(|p| p.is_file()).and_then(|p| crate::io::kspkg::Pkg::open(p).ok());
    report(&mut src, game.as_ref(), twin)
}
