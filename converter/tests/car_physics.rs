//! Car physics on real data (skipped when the AC1 install or the donor is
//! not on this machine): convert the physics of a few Kunos AC1 cars into
//! the cloned donor and check that
//! - every CarData inline copy (engine, drivetrain, gearbox, clutch, brakes,
//!   coilovers, geometries, wings) equals its standalone file;
//! - every gear reaches the limiter, the setup sits inside its limits, the
//!   physics files parse (`physics::stats::validate` reports no error);
//! - the AC1 numbers EVO keeps unchanged arrive unchanged.

use std::path::PathBuf;

use aceconv::car::{frame::Frame, identity, physics};

fn cars_dir() -> Option<PathBuf> {
    let p = std::env::var_os("AC1_CARS").map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)\Steam\steamapps\common\assettocorsa\content\cars"));
    p.is_dir().then_some(p)
}

fn donor() -> Option<PathBuf> {
    let p = std::env::var_os("ACECONV_DONOR").map(PathBuf::from).or_else(|| aceconv::donor::cache_dir().ok().map(|d| d.join(aceconv::donor::DONOR_ZIP)))?;
    p.is_file().then_some(p)
}

#[test]
fn kunos_cars_convert_consistently() {
    let (Some(dir), Some(dz)) = (cars_dir(), donor()) else { return };
    let content = aceconv::io::gamedir::evo_content();
    for name in ["ks_mazda_miata", "ks_toyota_ae86", "ks_audi_sport_quattro"] {
        let Ok(car) = aceconv::ac1::car::Car::open(&dir.join(name)) else { continue };
        let k = car.kn5().unwrap();
        let frame = Frame::build(&car, &k);
        let ident = identity::identity(&car, None, &Default::default(), &Default::default());
        let files = identity::clone_donor(aceconv::files::load_donor(&dz, identity::DONOR_ID).unwrap(), &ident);
        let mut files = files;
        let rep = physics::write(&mut files, &ident.id, &ident.stem, &car, &frame, content.is_file().then_some(content.as_path())).unwrap();
        let (errors, _warnings) = physics::stats::validate(&files.map, None);
        assert!(errors.is_empty(), "{name}: {errors:#?}");
        let d = &rep.derived;
        let s = car.ini("suspensions.ini");
        assert_eq!(d.spring[0], s.num("FRONT", "SPRING_RATE").unwrap(), "{name} front spring");
        assert_eq!(d.wheelbase, s.num("BASIC", "WHEELBASE").unwrap(), "{name} wheelbase");
        assert_eq!(d.limiter, car.ini("engine.ini").num("ENGINE_DATA", "LIMITER").unwrap(), "{name} limiter");
        assert!(rep.perf > 5.0 && rep.perf < 30.0, "{name} PI {}", rep.perf);
    }
}
