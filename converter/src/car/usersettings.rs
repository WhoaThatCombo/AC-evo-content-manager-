//! The game's per-car user settings in `Saved Games\ACE` (read here; only
//! `reset` writes, and only when asked).
//!
//! # Why a converted car can keep old cameras
//!
//! `CarCameras.carcamerausersettings` (CarCameraUserSettings, Camera
//! Properties.proto) keeps, per car id, the six drivable cameras' positions and
//! rotations (`drivableSettings.mapCarCameraData[id]`) and the onboard ones.
//! The game's drivable-camera loader (AssettoCorsaEVO.exe RVA 0x8a4ba0, build
//! of 2026-09-17) looks the car id up; when the id is missing it creates the
//! entry from the actor's `drivable_cameras`, and then, found or created, it
//! copies the entry's positions / rotations over the actor's cameras
//! (0x8a4e27..0x8a4f08). The entry is never refreshed from the actor, so a mod
//! reinstalled under the same id keeps the cameras of the first version the
//! game loaded: ks_mazda_miata_i2m9y kept its first bonnet camera at
//! (0, 0.45, 0.75) in the engine bay after the actor moved it over the
//! windscreen. `CarMirrors.carmirrorsusersettings` keeps per-id mirror
//! adjustments the same way.
//!
//! `reset` removes one id's entries (the game must be closed; the files are
//! backed up first); the game then rebuilds them from the installed actor.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::io::pb::{Msg, Val};

pub const CAMERAS: &str = "CarCameras.carcamerausersettings";
pub const MIRRORS: &str = "CarMirrors.carmirrorsusersettings";

/// `Saved Games\ACE` (ACECONV_SAVED_GAMES overrides it).
pub fn saved_dir() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ACECONV_SAVED_GAMES") {
        return Some(PathBuf::from(p));
    }
    let home = std::env::var_os("USERPROFILE")?;
    let d = PathBuf::from(home).join("Saved Games").join("ACE");
    d.is_dir().then_some(d)
}

fn v3(m: &Msg) -> [f32; 3] {
    [m.f32(1).unwrap_or(0.0), m.f32(2).unwrap_or(0.0), m.f32(3).unwrap_or(0.0)]
}

/// The saved drivable cameras of `id`: (position, rotation) per camera slot.
pub fn saved_drivable(settings: &[u8], id: &str) -> Option<Vec<([f32; 3], [f32; 3])>> {
    let top = Msg::parse(settings).ok()?;
    let entry = top.sub(1)?.subs(2).into_iter().find(|e| e.str(1).as_deref() == Some(id))?;
    let info = entry.sub(2).unwrap_or_default();
    let (p, r) = (info.subs(1), info.subs(2));
    Some(p.iter().zip(r.iter()).map(|(p, r)| (v3(p), v3(r))).collect())
}

/// The actor's drivable cameras in order: (name, position, rotation).
pub fn actor_drivable(actor: &[u8]) -> Vec<(String, [f32; 3], [f32; 3])> {
    let Some(car) = Msg::parse(actor).ok().and_then(|a| a.sub(103)) else { return vec![] };
    car.subs(10).iter().map(|c| {
        let name = c.sub(2).and_then(|d| d.str(2)).unwrap_or_default();
        let (p, r) = c.sub(1).map(|t| crate::car::fx::transform_of(&t)).unwrap_or_default();
        (name, p, r)
    }).collect()
}

/// The cameras whose saved copy differs from the actor (> 1 cm, > 0.1 deg).
pub fn stale_cameras(settings: &[u8], id: &str, actor: &[u8]) -> Vec<String> {
    let Some(saved) = saved_drivable(settings, id) else { return vec![] };
    let mut out = Vec::new();
    for ((name, p, r), (sp, sr)) in actor_drivable(actor).iter().zip(saved.iter()) {
        let dp = (0..3).map(|k| (p[k] - sp[k]).abs()).fold(0.0f32, f32::max);
        let dr = (0..3).map(|k| (r[k] - sr[k]).abs()).fold(0.0f32, f32::max);
        if dp > 0.01 || dr > 0.1 {
            out.push(format!("{name} saved at ({:.2}, {:.2}, {:.2}) pitch {:.1}, actor ({:.2}, {:.2}, {:.2}) pitch {:.1}", sp[0], sp[1], sp[2], sr[0], p[0], p[1], p[2], r[0]));
        }
    }
    out
}

/// Drop the map entries keyed `id` from `m.f<map>` (entries {f1 key, f2 value}).
fn drop_key(m: &mut Msg, map: u32, id: &str) -> usize {
    let before = m.fields.len();
    m.fields.retain(|f| {
        if f.num != map {
            return true;
        }
        let key = match &f.val {
            Val::Bytes(b) => Msg::parse(b).ok().and_then(|e| e.str(1)),
            Val::Msg(e) => e.str(1),
            _ => None,
        };
        key.as_deref() != Some(id)
    });
    before - m.fields.len()
}

/// Remove `id`'s saved cameras (drivable and onboard) and mirrors from the
/// settings in `dir`. Returns what was (or, with `dry`, would be) removed.
/// Each changed file is copied to `<file>.aceconv.bak` first.
pub fn reset(dir: &Path, id: &str, dry: bool) -> Result<Vec<String>> {
    let mut done = Vec::new();
    for (file, maps) in [(CAMERAS, &[(1u32, "drivable cameras"), (2, "onboard cameras")][..]), (MIRRORS, &[(0u32, "mirrors")][..])] {
        let p = dir.join(file);
        let Ok(b) = std::fs::read(&p) else { continue };
        let mut top = Msg::parse(&b).with_context(|| format!("parse {}", p.display()))?;
        let mut n = 0;
        for (sub, what) in maps {
            let k = if *sub == 0 {
                drop_key(&mut top, 1, id)
            } else {
                let Some(i) = top.idx(*sub).first().copied() else { continue };
                drop_key(top.open(i)?, 2, id)
            };
            if k > 0 {
                done.push(format!("{file}: {what} of {id}"));
                n += k;
            }
        }
        if n > 0 && !dry {
            std::fs::copy(&p, p.with_file_name(format!("{file}.aceconv.bak")))?;
            std::fs::write(&p, top.encode()).with_context(|| format!("write {}", p.display()))?;
        }
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vec3(x: f32, y: f32, z: f32) -> Msg {
        let mut m = Msg::default();
        m.set_f32(1, x);
        m.set_f32(2, y);
        m.set_f32(3, z);
        m
    }

    fn settings(ids: &[&str]) -> Vec<u8> {
        let mut drv = Msg::default();
        drv.push(1, Val::Bytes(crate::io::pb::f32_bytes(&[55.0; 6])));
        for id in ids {
            let mut info = Msg::default();
            info.push(1, Val::Msg(vec3(0.0, 0.45, 0.75)));
            info.push(2, Val::Msg(vec3(0.0, 180.0, 0.0)));
            let mut e = Msg::default();
            e.set_str(1, id);
            e.push(2, Val::Msg(info));
            drv.push(2, Val::Msg(e));
        }
        let mut top = Msg::default();
        top.push(1, Val::Msg(drv));
        top.encode()
    }

    #[test]
    fn reset_drops_only_that_id() {
        let dir = std::env::temp_dir().join(format!("aceconv_us_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(CAMERAS), settings(&["a_car", "b_car"])).unwrap();
        assert_eq!(saved_drivable(&settings(&["a_car"]), "a_car").unwrap()[0].0, [0.0, 0.45, 0.75]);
        let r = reset(&dir, "a_car", false).unwrap();
        assert_eq!(r.len(), 1);
        let after = std::fs::read(dir.join(CAMERAS)).unwrap();
        assert!(saved_drivable(&after, "a_car").is_none());
        assert!(saved_drivable(&after, "b_car").is_some());
        assert!(dir.join(format!("{CAMERAS}.aceconv.bak")).is_file());
        std::fs::remove_dir_all(&dir).ok();
    }
}
