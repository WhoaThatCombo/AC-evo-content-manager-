//! In-house FMOD Studio 2.03 bank writer: an FMOD Studio project (metadata XML +
//! PCM16 WAV assets) -> master_bank.bank + master_bank.strings.bank, without
//! fmodstudiocl. Format: docs/FORMATS.md, "FMOD Studio banks".
//!
//! Studio compiles the project into a "runtime bank model" (the same structures the
//! runtime's fmodstudio.dll loads) and serialises that; this module does the same:
//! `compile` builds [`Bank`] item lists from the project graph, `emit` writes the RIFF.
//! Features the compiler does not know return [`Unsupported`] so the caller can fall
//! back to fmodstudiocl.

pub(crate) mod compile;
pub(crate) mod compile_event;
mod compile_mixer;
pub(crate) mod effects;
mod fsb;
pub mod vorbisenc;
mod hash;
pub mod prj;
pub mod strings;
pub mod studioxml;
mod w;

use anyhow::Result;
use std::path::Path;

pub use compile::Unsupported;

pub struct Banks {
    pub bank: Vec<u8>,
    pub strings: Vec<u8>,
    pub name: String,
}

/// Build the (single, master) bank of the project at `dir`.
pub fn build_dir(dir: &Path) -> Result<Banks> {
    let p = prj::Project::load(dir)?;
    compile::build(&p)
}

/// `build_dir` with the sample codec chosen by the caller: Some(quality) = Vorbis (when every
/// sample can be), None = PCM16 (ACECONV_BANK_VORBIS is not consulted).
pub fn build_dir_with(dir: &Path, vorbis: Option<f32>) -> Result<Banks> {
    let p = prj::Project::load(dir)?;
    compile::build_with(&p, vorbis)
}
