//! aceconv: Assetto Corsa (AC1) cars and tracks -> Assetto Corsa EVO mods.
//!
//! Format notes for everything read or written here: docs/FORMATS.md.

pub mod io {
    pub mod gamedir;
    pub mod kspkg;
    pub mod material;
    pub mod pb;
    pub mod texture;
}
pub mod ac1 {
    pub mod acd;
    pub mod ai;
    pub mod car;
    pub mod ini;
    pub mod kn5;
    pub mod kn5_decrypt;
    pub mod knh;
    pub mod ksanim;
    pub mod ui;
}
pub mod batch;
pub mod car;
pub mod donor;
pub mod files;
pub mod ids;
pub mod share;
pub mod slim;
pub mod sound;
pub mod stats;
pub mod track;
pub mod validate;

/// `std::_Xlength_error`, the only C++-runtime function intel_tex_2's prebuilt
/// ASTC object needs (it was built against the DLL runtime, which this static
/// build leaves out). ASTC is never used here; abort like the original would.
#[cfg(all(windows, target_env = "msvc"))]
#[export_name = "?_Xlength_error@std@@YAXPEBD@Z"]
pub extern "C" fn xlength_error(_what: *const u8) -> ! {
    std::process::abort()
}

/// The same ASTC object imports the DLL runtime's `_wassert` (through its
/// `__imp_` pointer). Never reached; abort like a failed assert.
#[cfg(all(windows, target_env = "msvc"))]
extern "C" fn wassert(_msg: *const u16, _file: *const u16, _line: u32) {
    std::process::abort()
}

#[cfg(all(windows, target_env = "msvc"))]
#[used]
#[export_name = "__imp__wassert"]
pub static IMP_WASSERT: extern "C" fn(*const u16, *const u16, u32) = wassert;
