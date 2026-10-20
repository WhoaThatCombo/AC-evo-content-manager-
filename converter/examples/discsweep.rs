//! Body-only dry run over an AC1 car library: does the body plan give each
//! convertible car an exterior EXT_DISC section (the brake discs)? For the
//! ones without, the disc-like meshes the kn5 has (names / materials /
//! shaders) and where they hang.
//!
//! "fallback" cars get the generated EXT_DISC (`body::fallback_disc`) from
//! the donor's exterior mesh; a car whose fallback comes out empty, or whose
//! model disc lands mostly off the DISC_xx bones, is listed as LACKING.
//!
//! cargo run --release --example discsweep -- <AC1 content\cars dir> [name filter]

use aceconv::ac1::car::Car;
use aceconv::car::{body, frame::Frame, geometry, materials};

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let dir = std::path::PathBuf::from(&a[1]);
    let filt = a.get(2).cloned().unwrap_or_default().to_lowercase();
    let (donor_zip, _) = aceconv::batch::donor_paths()?;
    let donor = aceconv::files::load_donor(&donor_zip, aceconv::car::identity::DONOR_ID)?;
    let donor_mesh = donor.map.iter().find(|(k, _)| k.to_lowercase().ends_with("_exterior.mesh")).map(|(_, v)| v.clone());
    let mut n_lacking = 0;
    let mut cars: Vec<_> = std::fs::read_dir(&dir)?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    cars.sort();
    let (mut n_ok, mut n_disc, mut n_fallback, mut n_skip) = (0, 0, 0, 0);
    let re = regex::Regex::new(r"(?i)(disc|disk|brake|rotor|freno|bremse|frein)").unwrap();
    for p in cars {
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        if !name.to_lowercase().contains(&filt) {
            continue;
        }
        let has_kn5 = std::fs::read_dir(&p).map(|r| r.flatten().any(|f| f.path().extension().map(|x| x.eq_ignore_ascii_case("kn5")).unwrap_or(false))).unwrap_or(false);
        if !has_kn5 {
            continue;
        }
        let car = match Car::open(&p) {
            Ok(c) => c,
            Err(e) => {
                println!("SKIP\t{name}\tcar: {e}");
                n_skip += 1;
                continue;
            }
        };
        let k = match car.kn5() {
            Ok(k) => k,
            Err(e) => {
                println!("SKIP\t{name}\tkn5: {e}");
                n_skip += 1;
                continue;
            }
        };
        n_ok += 1;
        let f = Frame::build(&car, &k);
        let mut notes = Vec::new();
        let insts = geometry::flatten(&k, &mut notes);
        let hints = materials::Hints::from_data(car.ini("lights.ini"), car.ini("mirrors.ini"), Default::default());
        let plan = body::plan(&k, &insts, &f, &hints, &[], None);
        let discs: Vec<&body::Sec> = plan.sections.iter().filter(|s| s.kind == materials::Kind::Disc && !s.interior && !s.parts.is_empty()).collect();
        let on_disc = discs.iter().flat_map(|s| s.parts.iter()).filter(|x| x.1.starts_with("DISC_")).count();
        let total: usize = discs.iter().map(|s| s.parts.len()).sum();
        let sources = if discs.is_empty() {
            n_fallback += 1;
            let (g, note) = body::fallback_disc(donor_mesh.as_deref(), &f, &plan.ext_bones);
            if g.tris() == 0 { n_lacking += 1; "LACKING: fallback empty".to_string() } else { format!("fallback {} tris: {note}", g.tris()) }
        } else {
            n_disc += 1;
            if on_disc * 2 < total { n_lacking += 1; format!("LACKING: {on_disc} of {total} disc parts on DISC_xx") } else { format!("model, {on_disc} of {total} parts on DISC_xx") }
        };
        // a corner whose wheel parts lost every rim to the disc rule
        let mut rims = std::collections::BTreeSet::new();
        for &w in &plan.wheel_parts {
            let i = &insts[w];
            if materials::kind_of(&k.materials[i.material], &i.node, true, i.blur, false, &hints) == materials::Kind::Rim {
                rims.insert(i.corner.unwrap_or(""));
            }
        }
        let sources = if rims.len() < 4 { format!("{sources}\tRIMS ONLY AT {rims:?}") } else { sources };
        let corners: Vec<String> = discs.iter().flat_map(|s| s.parts.iter().map(|x| x.1.clone())).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
        println!("{}\t{name}\t{} disc sections\t{}\t{sources}", if discs.is_empty() { "NODISC" } else { "DISC" }, discs.len(), corners.join(","));
        if discs.is_empty() || std::env::var("DISCSWEEP_ALL").is_ok() {
            for i in &insts {
                let m = &k.materials[i.material.min(k.materials.len().saturating_sub(1))];
                if re.is_match(&i.path) || re.is_match(&m.name) || m.shader.to_lowercase().contains("brakedisc") {
                    println!("    cand {:?}/{:?} {} tris mat={} sh={} {}", i.under, i.corner, i.mesh.index_count() / 3, m.name, m.shader, i.path);
                }
            }
        }
    }
    println!("TOTAL convertible {n_ok}: model disc {n_disc}, fallback {n_fallback}, lacking a disc {n_lacking}; skipped {n_skip}");
    Ok(())
}
