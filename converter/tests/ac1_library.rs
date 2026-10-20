//! Runs against the user's AC1 install when it is there (skipped otherwise):
//! every car's data decrypts to an ini set with car.ini [BASIC], and cars that
//! ship both forms give identical text from data.acd and from data\ (for
//! every file whose two copies have the same length; a different length is a
//! loose file edited after packing - proview_nodes.ini, tyres.ini).

use std::path::PathBuf;

fn cars_dir() -> Option<PathBuf> {
    let p = std::env::var_os("AC1_CARS").map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)\Steam\steamapps\common\assettocorsa\content\cars"));
    p.is_dir().then_some(p)
}

#[test]
fn every_car_decrypts() {
    let Some(dir) = cars_dir() else { return };
    let (mut acd, mut bad, mut paired, mut same) = (0, Vec::new(), 0, 0);
    let mut cars: Vec<PathBuf> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).filter(|p| p.join("data.acd").is_file()).collect();
    cars.sort();
    for car in cars {
        acd += 1;
        let name = car.file_name().unwrap().to_string_lossy().into_owned();
        let d = match aceconv::ac1::acd::read_car_data(&car) {
            Ok(d) => d,
            Err(e) => {
                bad.push(format!("{name}: {e:#}"));
                continue;
            }
        };
        let ok = d.get("car.ini").map(|t| aceconv::ac1::acd::looks_plain(t) && aceconv::ac1::ini::parse(t).has("BASIC")).unwrap_or(false);
        if !ok {
            bad.push(format!("{name}: car.ini missing, unreadable or without [BASIC]"));
            continue;
        }
        let loose = car.join("data");
        if loose.is_dir() {
            paired += 1;
            let mut all = true;
            for (k, v) in &d {
                if let Ok(b) = std::fs::read(loose.join(k)) {
                    let t: String = b.iter().map(|&c| c as char).collect();
                    if t.len() == v.len() && &t != v {
                        all = false;
                    }
                }
            }
            if all {
                same += 1;
            }
        }
    }
    println!("{acd} cars with data.acd, {} failed, {paired} paired, {same} paired identical", bad.len());
    for b in &bad {
        println!("  {b}");
    }
    assert_eq!(same, paired, "a paired car's data.acd differs from its data folder");
    // mods may pack data.acd for a different folder name: allow a few, report them
    assert!(bad.len() * 20 <= acd, "{} of {acd} cars failed", bad.len());
}

/// Brake discs (EXT_DISC: the game dereferences the slot without a null
/// check). Cars whose disc the body plan must find in the model, and cars
/// with no disc in the model, which get the generated fallback on DISC_xx.
#[test]
fn every_car_gets_a_disc() {
    use aceconv::car::{body, frame::Frame, geometry, materials};
    let Some(dir) = cars_dir() else { return };
    let model = ["ks_ford_mustang_2015", "ks_lotus_72d", "ks_audi_r18_etron_quattro", "ks_maserati_250f_12cyl", "ma_t180_augury", "mg_mga1965"];
    let fallback = ["ks_audi_sport_quattro", "ks_audi_sport_quattro_s1", "ks_porsche_718_spyder_rs", "chevrolet_impala"];
    for name in model.iter().chain(fallback.iter()) {
        let p = dir.join(name);
        let Ok(car) = aceconv::ac1::car::Car::open(&p) else { continue };
        let Ok(k) = car.kn5() else { continue };
        let f = Frame::build(&car, &k);
        let insts = geometry::flatten(&k, &mut Vec::new());
        let hints = materials::Hints::from_data(car.ini("lights.ini"), car.ini("mirrors.ini"), Default::default());
        let plan = body::plan(&k, &insts, &f, &hints, &[], None);
        let parts: Vec<&String> = plan.sections.iter().filter(|s| s.kind == materials::Kind::Disc && !s.interior).flat_map(|s| s.parts.iter().map(|x| &x.1)).collect();
        if model.contains(name) {
            assert!(!parts.is_empty(), "{name}: no disc found in the model");
            assert!(parts.iter().filter(|b| b.starts_with("DISC_")).count() * 2 >= parts.len(), "{name}: discs off DISC_xx {parts:?}");
        } else {
            assert!(parts.is_empty(), "{name}: unexpected model disc {parts:?}");
            let (g, _) = body::fallback_disc(None, &f, &plan.ext_bones);
            assert!(g.tris() > 0, "{name}: empty fallback disc");
            assert!(g.verts.iter().all(|v| plan.ext_bones[v.bone as usize].name.starts_with("DISC_")), "{name}");
        }
    }
}
