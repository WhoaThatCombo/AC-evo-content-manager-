//! Seat belts: AC1 `CINTURE_ON` / `CINTURE_OFF` -> the actor's seatbelt
//! meshes.
//!
//! AC1 shows `CINTURE_ON` (the belt across the driver) while driving and
//! `CINTURE_OFF` (the belt hanging) without a driver. Stock EVO cars carry
//! the same pair as base meshes `seatbelt_on` (f12) / `seatbelt_off` (f22)
//! of their own (mx5_na: `belt_auto.mesh`, `belt_off.mesh`, one LOD, one
//! INT_BELT section; `belt_off` without bones) beside the interior. Left in
//! the interior mesh both AC1 belts would show at once; here each goes to
//! its own bone-less, one-LOD mesh in car space (ours: the belt-on mesh stays
//! rigid, shaped to the AC1 driver, where stock skins it to the driver).

use std::collections::BTreeMap;

use anyhow::Result;

use crate::ac1::kn5::Kn5;
use crate::car::frame::Frame;
use crate::car::geometry::{self, Built, Geo, Inst, Section};
use crate::car::materials::{self, Kind, MatIn, Tex, Templates};
use crate::files::Files;
use crate::io::pb::{Msg, Val};

/// Which belt an instance path belongs to (a path segment CINTURE_ON* /
/// CINTURE_OFF*).
pub fn belt_of(path: &str) -> Option<bool> {
    for s in path.split('/') {
        let u = s.to_ascii_uppercase();
        if u.starts_with("CINTURE_ON") {
            return Some(true);
        }
        if u.starts_with("CINTURE_OFF") {
            return Some(false);
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
pub fn write(files: &mut Files, id: &str, stem: &str, k: &Kn5, insts: &[Inst], f: &Frame, tex: &mut Tex, t: &Templates, default_skin: Option<(&str, std::path::PathBuf)>) -> Result<Vec<String>> {
    let base = format!("content\\cars\\{id}");
    let mut notes = Vec::new();
    let mut paths: [Option<String>; 2] = [None, None];
    let mut mats: BTreeMap<usize, String> = BTreeMap::new();
    for (on, file, slot) in [(true, format!("{stem}_belt_on"), 0usize), (false, format!("{stem}_belt_off"), 1)] {
        let mut by_mat: BTreeMap<usize, Geo> = BTreeMap::new();
        for i in insts.iter().filter(|i| belt_of(&i.path) == Some(on)) {
            let g = by_mat.entry(i.material).or_default();
            geometry::append(g, i.mesh, &f.evo_m(&i.world), 0);
        }
        if by_mat.is_empty() {
            continue;
        }
        let mut secs = Vec::new();
        for (mi, mut g) in by_mat {
            geometry::split_double_sided(&mut g, 0.0004);
            geometry::compact(&mut g);
            if g.tris() == 0 {
                continue;
            }
            let Some(m) = k.materials.get(mi) else { continue };
            let path = match mats.get(&mi) {
                Some(p) => p.clone(),
                None => {
                    let n = mats.len() + 1;
                    let name = if n == 1 { "INT_BELT".to_string() } else { format!("INT_BELT_{n}") };
                    let inp = MatIn { kind: Kind::Generic, m, interior: true, skin: default_skin.clone(), metallic: false };
                    let mm = materials::build(files, tex, t, &inp);
                    let p = format!("{base}\\materials\\{name}.material");
                    files.put(&p, mm.encode());
                    mats.insert(mi, p.clone());
                    p
                }
            };
            let name = path.rsplit('\\').next().unwrap_or("INT_BELT").trim_end_matches(".material").to_string();
            secs.push(Section { name, material: path, geo: g });
        }
        if secs.is_empty() {
            continue;
        }
        let b = Built { bones: vec![], lods: vec![secs], lod_in: vec![0.0], skinned: false };
        let mp = format!("{base}\\meshes\\{file}.mesh");
        let tris = b.tris(0);
        files.put(&mp, b.encode(None));
        notes.push(format!("belts: {} -> {} ({tris} tris)", if on { "CINTURE_ON" } else { "CINTURE_OFF" }, mp.rsplit('\\').next().unwrap_or("")));
        paths[slot] = Some(mp);
    }
    let ap = format!("{base}\\{id}.actor");
    let Some(mut actor) = files.get(&ap).and_then(|b| Msg::parse(b).ok()) else { return Ok(notes) };
    {
        let bm = actor.msg_mut(103)?.msg_mut(12)?;
        for (f, p) in [(12u32, &paths[0]), (22, &paths[1])] {
            let Some(p) = p else { continue };
            let mut m = Msg::default();
            m.set_str(1, p);
            match bm.fields.iter().position(|x| x.num == f) {
                Some(i) => bm.fields[i].val = Val::Msg(m),
                None => {
                    let at = bm.fields.iter().position(|x| x.num > f).unwrap_or(bm.fields.len());
                    bm.fields.insert(at, crate::io::pb::Field { num: f, val: Val::Msg(m) });
                }
            }
        }
    }
    files.put(&ap, actor.encode());
    if notes.is_empty() {
        notes.push("belts: no CINTURE_ON / CINTURE_OFF in the kn5".into());
    }
    Ok(notes)
}

#[cfg(test)]
mod tests {
    #[test]
    fn belt_paths() {
        assert_eq!(super::belt_of("COCKPIT_HR/CINTURE_ON/GEO_CINTURE_ON"), Some(true));
        assert_eq!(super::belt_of("COCKPIT_HR/CINTURE_OFF_SUB005"), Some(false));
        assert_eq!(super::belt_of("COCKPIT_HR/seat"), None);
    }
}
