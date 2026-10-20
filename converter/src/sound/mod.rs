//! FMOD: FSB5 sample decoding, the Studio 2.03 bank reader / writer, the
//! template project generated from the game's own banks, and the car sound
//! stage (AC1 FMOD 1.08 bank -> the car's own EVO banks + actor slots).

pub mod ac1bank;
pub mod bankread;
pub mod bankwrite;
pub mod fadpcm;
pub mod fsb5;
pub mod map;
pub mod mkfmod;
pub mod render;
pub mod slots;
pub mod vorbis;

use anyhow::Result;
use std::path::Path;

/// The car sound stage: the AC1 car's bank -> `content\cars\<id>\sfx\master_bank(.strings).bank`
/// and the actor's audio slots. `work` is a scratch folder (removed afterwards).
pub fn car_stage(files: &mut crate::files::Files, id: &str, ac1_dir: &Path, content: &Path, work: &Path) -> Result<Vec<String>> {
    let o = map::Opts { car_id: id.to_string(), content: content.to_path_buf(), limiter: map::limiter_rpm(ac1_dir), work: work.to_path_buf(), vorbis: map::vorbis_env() };
    let r = map::build(ac1_dir, &o);
    let _ = std::fs::remove_dir_all(work);
    let b = r?;
    let base = format!("content\\cars\\{id}\\sfx");
    files.put(&format!("{base}\\master_bank.bank"), b.bank);
    files.put(&format!("{base}\\master_bank.strings.bank"), b.strings);
    let mut notes = b.notes;
    notes.extend(slots::apply(files, id, &b.events)?);
    Ok(notes)
}
