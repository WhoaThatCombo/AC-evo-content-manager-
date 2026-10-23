//! The AC1 track scan: one pass over an AC1 track folder into the model
//! every track stage reads.
//!
//! Two steps, because the kn5 data stays in its memory map (nothing is
//! copied): [`Source::open`] finds the layouts and maps every kn5 once, and
//! [`Scan::new`] parses them (once) and flattens the meshes to world space.
//!
//! ```ignore
//! let src = scan::Source::open(Path::new(r"...\content\tracks\vrc_alabama"))?;
//! let scan = scan::Scan::new(&src)?;
//! for m in &scan.meshes { let mesh = scan.mesh(m); /* world = m.world */ }
//! ```
//!
//! # AC1 conventions used here
//!
//! - **Layouts**: every subfolder holding `data\surfaces.ini` is a layout
//!   (its name is the folder name); without one the track root is the only
//!   layout, named "". A layout's models come from `models_<layout>.ini`,
//!   else `models.ini`, else `<folder>.kn5`.
//! - **models*.ini**: `[MODEL_n] FILE / POSITION / ROTATION`; ROTATION is
//!   heading, pitch, roll in degrees (heading about +Y, right-handed).
//! - **kn5 nodes**: row-vector matrices, translation in elements 12..14,
//!   world = local * parent; the model placement is applied last.
//! - **Hidden**: inactive nodes and their subtrees; the AC1 dummies
//!   (`AC_START_n`, `AC_PIT_n`, `AC_HOTLAP_START_n`, `AC_TIME_n_L/R`,
//!   `AC_AUDIO_*`, `AC_CREW_*`, `AC_SPAWN*`) and their subtrees. Other
//!   `AC_*` names (`AC_POBJECT_*` physical props, `AC_START_LIGHT*`-style
//!   objects) are real geometry and stay.
//! - **Physics**: a mesh whose name starts with digits is physical; the
//!   rest of its name starts with its surface KEY (longest key wins; else
//!   the longest key it contains). Kunos's collision meshes are `01WALL`,
//!   `03ROAD`: only a bare `0` prefix means decor without collision (`0FENCE`).
//!   AC1 has no wall category (a wall is a steep physics mesh) and collides
//!   with meshes whose key it does not know; ours: such a mesh becomes an
//!   EVO wall when it is steep (area-weighted |normal.y| < 0.5) or named like
//!   a wall, else the default ROAD surface.
//! - **Surfaces**: each layout's `data\surfaces.ini`, plus AC1's global
//!   `system\data\surfaces.ini` (ROAD, GRASS, KERB, SAND) for keys a track
//!   uses without defining them.
//! - **Lift**: none. Stock EVO tracks drive below y = 0 (measured: spa's
//!   road reaches -29.0 m, suzuka's walls -9.1 m), so positions are kept;
//!   [`LIFT`] is the one place to change that.

use anyhow::{bail, Context, Result};

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::ac1::ai::{self, AiLine};
use crate::ac1::ini::{self, Ini};
use crate::ac1::kn5::{self, Kn5, Material, Mesh, NodeKind, Texture};
use crate::ac1::kn5_decrypt::{self, Kn5Bytes};
use crate::ac1::ui::TrackUi;

/// Vertical offset applied to everything (see the module notes).
pub const LIFT: f32 = 0.0;

/// AC1's global surfaces (`system\data\surfaces.ini` of the AC1 install),
/// used when that file cannot be read.
const AC1_GLOBAL_SURFACES: &str = "[SURFACE_0]\nKEY=ROAD\nFRICTION=1\nDAMPING=0\nWAV=\nIS_VALID_TRACK=1\nSIN_HEIGHT=0\nSIN_LENGTH=0\nIS_PITLANE=0\n\
[SURFACE_1]\nKEY=GRASS\nFRICTION=0.6\nDAMPING=0\nWAV=grass.wav\nIS_VALID_TRACK=0\nSIN_HEIGHT=0.03\nSIN_LENGTH=0.5\nIS_PITLANE=0\nDIRT_ADDITIVE=1\n\
[SURFACE_2]\nKEY=KERB\nFRICTION=0.92\nDAMPING=0\nWAV=kerb.wav\nIS_VALID_TRACK=1\nSIN_HEIGHT=0\nSIN_LENGTH=0\nIS_PITLANE=0\n\
[SURFACE_3]\nKEY=SAND\nFRICTION=0.8\nDAMPING=0.1\nWAV=sand.wav\nIS_VALID_TRACK=0\nSIN_HEIGHT=0.04\nSIN_LENGTH=0.5\nIS_PITLANE=0\nDIRT_ADDITIVE=1\n";

/// Node-name prefixes of AC1 dummies (never drawn; their subtree neither).
pub const DUMMY_PREFIXES: [&str; 7] = ["AC_START_", "AC_PIT_", "AC_HOTLAP_START_", "AC_TIME_", "AC_AUDIO_", "AC_CREW_", "AC_SPAWN"];

pub fn is_dummy(name: &str) -> bool {
    let u = name.to_uppercase();
    DUMMY_PREFIXES.iter().any(|p| u.starts_with(p))
}

/// One `[MODEL_n]` of a layout.
#[derive(Clone, Debug)]
pub struct ModelRef {
    pub file: String,
    pub position: [f32; 3],
    /// heading, pitch, roll (degrees)
    pub rotation: [f32; 3],
}

/// A layout as found on disk (before parsing).
#[derive(Clone, Debug)]
pub struct LayoutDef {
    /// AC1 layout folder name ("" = the track root)
    pub name: String,
    pub data_dir: PathBuf,
    pub ai_dir: PathBuf,
    pub ui_json: Option<PathBuf>,
    pub models: Vec<ModelRef>,
}

/// The memory-mapped AC1 track (owner of every kn5 byte the scan borrows).
pub struct Source {
    pub dir: PathBuf,
    pub folder: String,
    pub layouts: Vec<LayoutDef>,
    /// (file name as written in the ini, map), one per distinct kn5
    pub maps: Vec<(String, Kn5Bytes)>,
    /// models skipped (listed but missing)
    pub notes: Vec<String>,
}

fn read_text(p: &Path) -> Option<String> {
    std::fs::read(p).ok().map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// A case-insensitive child lookup (AC1 mods mix cases freely).
fn child_ci(dir: &Path, name: &str) -> Option<PathBuf> {
    let p = dir.join(name);
    if p.exists() {
        return Some(p);
    }
    let want = name.to_lowercase();
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| p.file_name().map(|f| f.to_string_lossy().to_lowercase() == want).unwrap_or(false))
}

fn triple(v: Option<&str>) -> [f32; 3] {
    let n = v.map(ini::nums_of).unwrap_or_default();
    [n.first().copied().unwrap_or(0.0) as f32, n.get(1).copied().unwrap_or(0.0) as f32, n.get(2).copied().unwrap_or(0.0) as f32]
}

fn models_of(ini: &Ini) -> Vec<ModelRef> {
    ini.with_prefix("MODEL_")
        .filter_map(|(_, s)| {
            let file = s.get("FILE")?.trim().to_string();
            (!file.is_empty()).then(|| ModelRef { file, position: triple(s.get("POSITION").map(|x| x.as_str())), rotation: triple(s.get("ROTATION").map(|x| x.as_str())) })
        })
        .collect()
}

impl Source {
    /// Keep only the layouts whose AC1 folder name is in `names` (case-insensitive).
    pub fn keep_layouts(&mut self, names: &[String]) -> Result<()> {
        self.layouts.retain(|l| names.iter().any(|n| n.eq_ignore_ascii_case(&l.name)));
        if self.layouts.is_empty() {
            bail!("{}: none of the chosen layouts exist ({})", self.folder, names.join(", "));
        }
        Ok(())
    }

    /// Find the layouts of the AC1 track at `dir` and map its kn5 files.
    /// Fails on a CSP-protected kn5 (those are encrypted by their author).
    pub fn open(dir: &Path) -> Result<Source> {
        let folder = dir.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        if !dir.is_dir() {
            bail!("{} is not a folder", dir.display());
        }
        let mut subs: Vec<PathBuf> = std::fs::read_dir(dir)?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        subs.sort();
        let skip = ["ui", "data", "ai", "skins", "extension", "texture", "textures", "sfx", "ext"];
        let mut layouts = Vec::new();
        for s in &subs {
            let name = s.file_name().unwrap().to_string_lossy().into_owned();
            if skip.contains(&name.to_lowercase().as_str()) {
                continue;
            }
            let Some(data) = child_ci(s, "data") else { continue };
            if child_ci(&data, "surfaces.ini").is_none() {
                continue;
            }
            let models = child_ci(dir, &format!("models_{name}.ini")).or_else(|| child_ci(dir, "models.ini"));
            let models = models.and_then(|p| read_text(&p)).map(|t| models_of(&ini::parse(&t))).unwrap_or_default();
            let ui = child_ci(dir, "ui").and_then(|u| child_ci(&u, &name)).and_then(|u| child_ci(&u, "ui_track.json"))
                .or_else(|| child_ci(dir, "ui").and_then(|u| child_ci(&u, "ui_track.json")));
            layouts.push(LayoutDef { name: name.clone(), data_dir: data, ai_dir: child_ci(s, "ai").unwrap_or_else(|| s.join("ai")), ui_json: ui, models });
        }
        if layouts.is_empty() {
            let models = child_ci(dir, "models.ini").and_then(|p| read_text(&p)).map(|t| models_of(&ini::parse(&t))).unwrap_or_default();
            layouts.push(LayoutDef {
                name: String::new(),
                data_dir: child_ci(dir, "data").unwrap_or_else(|| dir.join("data")),
                ai_dir: child_ci(dir, "ai").unwrap_or_else(|| dir.join("ai")),
                ui_json: child_ci(dir, "ui").and_then(|u| child_ci(&u, "ui_track.json")),
                models,
            });
        }
        for l in &mut layouts {
            if l.models.is_empty() {
                l.models.push(ModelRef { file: format!("{folder}.kn5"), position: [0.0; 3], rotation: [0.0; 3] });
            }
        }
        let mut maps: Vec<(String, Kn5Bytes)> = Vec::new();
        let mut notes = Vec::new();
        for l in &layouts {
            for m in &l.models {
                if maps.iter().any(|(f, _)| f.eq_ignore_ascii_case(&m.file)) {
                    continue;
                }
                let Some(p) = child_ci(dir, &m.file) else {
                    notes.push(format!("model {} listed by layout {:?} is missing: skipped", m.file, l.name));
                    continue;
                };
                let map = kn5_decrypt::open(&p).with_context(|| format!("{}: {}", folder, m.file))?;
                if kn5::protected(&map) {
                    bail!("{}: {} is protected by its author (CSP kn5 encryption) - it cannot be converted", folder, m.file);
                }
                maps.push((m.file.clone(), map));
            }
        }
        for l in &layouts {
            if !l.models.iter().any(|m| maps.iter().any(|(f, _)| f.eq_ignore_ascii_case(&m.file))) {
                bail!("{}: layout {:?} has none of its models", folder, l.name);
            }
        }
        Ok(Source { dir: dir.to_path_buf(), folder, layouts, maps, notes })
    }
}

/// One AC1 physics surface (`[SURFACE_n]`).
#[derive(Clone, Debug, Default)]
pub struct Surface {
    pub key: String,
    pub friction: f32,
    pub damping: f32,
    pub wav: String,
    pub wav_pitch: f32,
    pub dirt_additive: f32,
    pub valid_track: bool,
    pub pitlane: bool,
    pub black_flag_time: f32,
    pub sin_height: f32,
    pub sin_length: f32,
    pub vibration_gain: f32,
    pub vibration_length: f32,
    /// came from AC1's global surfaces, not the track
    pub global: bool,
    /// layouts (bit i = layout i) whose surfaces.ini defines it
    pub layouts: u64,
}

fn surfaces_of(ini: &Ini, global: bool) -> Vec<Surface> {
    let f = |s: &indexmap::IndexMap<String, String>, k: &str| s.get(k).and_then(|v| ini::num_of(v)).unwrap_or(0.0) as f32;
    ini.with_prefix("SURFACE_")
        .filter_map(|(_, s)| {
            let key = s.get("KEY")?.trim().to_uppercase();
            (!key.is_empty()).then(|| Surface {
                key,
                friction: s.get("FRICTION").and_then(|v| ini::num_of(v)).unwrap_or(1.0) as f32,
                damping: f(s, "DAMPING"),
                wav: s.get("WAV").cloned().unwrap_or_default().to_lowercase(),
                wav_pitch: f(s, "WAV_PITCH"),
                dirt_additive: f(s, "DIRT_ADDITIVE"),
                valid_track: f(s, "IS_VALID_TRACK") != 0.0,
                pitlane: f(s, "IS_PITLANE") != 0.0,
                black_flag_time: f(s, "BLACK_FLAG_TIME"),
                sin_height: f(s, "SIN_HEIGHT"),
                sin_length: f(s, "SIN_LENGTH"),
                vibration_gain: f(s, "VIBRATION_GAIN"),
                vibration_length: f(s, "VIBRATION_LENGTH"),
                global,
                layouts: 0,
            })
        })
        .collect()
}

/// A kn5 node, flattened (world matrix in the kn5's own space).
pub struct FlatNode<'s> {
    pub name: String,
    pub parent: Option<usize>,
    /// world matrix inside its kn5 (before the model placement)
    pub world: [f32; 16],
    /// drawn: active, and neither it nor an ancestor is inactive or a dummy
    pub shown: bool,
    pub mesh: Option<Mesh<'s>>,
}

/// One parsed kn5.
pub struct Model<'s> {
    pub file: String,
    pub version: i32,
    pub textures: Vec<Texture<'s>>,
    pub materials: Vec<Material>,
    pub nodes: Vec<FlatNode<'s>>,
    /// local material id -> global material index
    pub mat_global: Vec<usize>,
    /// local texture index -> global texture index (None = inactive)
    pub tex_global: Vec<Option<usize>>,
}

fn flatten<'s>(n: kn5::Node<'s>, parent: Option<usize>, parent_world: [f32; 16], parent_shown: bool, out: &mut Vec<FlatNode<'s>>) {
    let (world, mesh) = match n.kind {
        NodeKind::Transform(m) => (kn5::mul(&m, &parent_world), None),
        NodeKind::Mesh(m) => (parent_world, Some(m)),
        NodeKind::Other => (parent_world, None),
    };
    let shown = parent_shown && n.active && !is_dummy(&n.name);
    let idx = out.len();
    out.push(FlatNode { name: n.name, parent, world, shown, mesh });
    for c in n.children {
        flatten(c, Some(idx), world, shown, out);
    }
}

/// A global (deduplicated) material.
#[derive(Clone, Debug)]
pub struct MatRef {
    /// unique across the track (renamed with the kn5 stem on a collision)
    pub name: String,
    pub model: usize,
    pub local: usize,
}

/// A global (deduplicated) texture.
#[derive(Clone, Debug)]
pub struct TexRef {
    /// unique across the track
    pub name: String,
    pub model: usize,
    pub local: usize,
    pub hash: u64,
    /// a CM skin file that replaces a placeholder (1x1 / 2x2) kn5 texture
    pub skin_file: Option<PathBuf>,
}

/// What a placed mesh is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// drawn, no collision
    Visual,
    /// collides with surface `surface`; `drawn` says whether it is also visible
    Physics { surface: usize, drawn: bool },
}

/// One mesh placed in the world.
#[derive(Clone, Debug)]
pub struct MeshInst {
    pub model: usize,
    pub node: usize,
    /// node world matrix times the model placement (row vectors), lifted
    pub world: [f32; 16],
    /// bit i = present in layout i
    pub layouts: u64,
    pub material: usize,
    pub role: Role,
    /// the AC1 node name
    pub name: String,
}

/// An AC1 dummy (transform node) of a layout, world space.
#[derive(Clone, Debug)]
pub struct Dummy {
    pub name: String,
    pub world: [f32; 16],
}

impl Dummy {
    pub fn position(&self) -> [f32; 3] {
        [self.world[12], self.world[13], self.world[14]]
    }
    /// The dummy's local +Z axis in world space (AC1 spawn dummies point
    /// along it), normalised.
    pub fn forward(&self) -> [f32; 3] {
        norm3([self.world[8], self.world[9], self.world[10]])
    }
    pub fn up(&self) -> [f32; 3] {
        norm3([self.world[4], self.world[5], self.world[6]])
    }
}

pub fn norm3(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 1e-12 && l.is_finite() { [v[0] / l, v[1] / l, v[2] / l] } else { [0.0, 0.0, 1.0] }
}

/// A layout, parsed.
pub struct Layout {
    pub name: String,
    pub ui: TrackUi,
    pub surfaces_ini: Ini,
    pub fast_lane: Option<AiLine>,
    pub pit_lane: Option<AiLine>,
    /// `AC_*` dummies of this layout's models (world space, lifted)
    pub dummies: Vec<Dummy>,
    /// other data\*.ini files by lower-case name (cameras, groove, drs_zones,
    /// audio_sources, semaphore / startinglights, ...)
    pub data: HashMap<String, Ini>,
}

impl Layout {
    pub fn dummies_with(&self, prefix: &str) -> Vec<&Dummy> {
        let p = prefix.to_uppercase();
        self.dummies.iter().filter(|d| d.name.to_uppercase().starts_with(&p)).collect()
    }
}

/// The whole AC1 track, parsed once.
pub struct Scan<'s> {
    pub src: &'s Source,
    pub folder: String,
    pub layouts: Vec<Layout>,
    pub models: Vec<Model<'s>>,
    pub materials: Vec<MatRef>,
    pub textures: Vec<TexRef>,
    pub surfaces: Vec<Surface>,
    pub meshes: Vec<MeshInst>,
    /// the track's own `extension\ext_config.ini`, else the CSP companion
    /// config of the AC1 install (`extension\config\tracks\[loaded\]<folder>.ini`)
    pub csp: Option<(PathBuf, Ini)>,
    /// things worth telling the user (skipped items, guesses)
    pub notes: Vec<String>,
    /// kn5 bytes read since the maps were last trimmed (see `Source::trim`)
    touched: std::sync::atomic::AtomicUsize,
}

/// Trim the kn5 maps from the working set after this many bytes were read.
const TRIM_EVERY: usize = 1 << 30;

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn VirtualUnlock(addr: *const core::ffi::c_void, size: usize) -> i32;
}

impl Source {
    /// Drop the kn5 maps' pages from the process working set. They are clean
    /// file pages: the OS keeps them in its standby cache and a later read
    /// faults them back in without disk work. Huge maps (glensheil: 2.9 GB
    /// of kn5) would otherwise keep every page they ever read resident.
    pub fn trim(&self) {
        #[cfg(windows)]
        for (_, m) in &self.maps {
            // VirtualUnlock on unlocked pages removes them from the working set
            unsafe { VirtualUnlock(m.as_ptr() as *const core::ffi::c_void, m.len()) };
        }
    }
}

fn fnv(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for c in b {
        h = (h ^ *c as u64).wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Placement matrix of a `[MODEL_n]` (row vectors): heading about +Y, then
/// pitch about X, then roll about Z, then the translation (and the lift).
pub fn placement(position: [f32; 3], rotation: [f32; 3]) -> [f32; 16] {
    let (h, p, r) = (rotation[0].to_radians(), rotation[1].to_radians(), rotation[2].to_radians());
    // column-vector rotation Rc = Ry(h) * Rx(p) * Rz(r)
    let ry = [[h.cos(), 0.0, h.sin()], [0.0, 1.0, 0.0], [-h.sin(), 0.0, h.cos()]];
    let rx = [[1.0, 0.0, 0.0], [0.0, p.cos(), -p.sin()], [0.0, p.sin(), p.cos()]];
    let rz = [[r.cos(), -r.sin(), 0.0], [r.sin(), r.cos(), 0.0], [0.0, 0.0, 1.0]];
    let m3 = |a: [[f32; 3]; 3], b: [[f32; 3]; 3]| {
        let mut o = [[0f32; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                o[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
            }
        }
        o
    };
    let rc = m3(m3(ry, rx), rz);
    let mut m = kn5::IDENTITY;
    for i in 0..3 {
        for j in 0..3 {
            // row-vector form is the transpose
            m[i * 4 + j] = rc[j][i];
        }
    }
    m[12] = position[0];
    m[13] = position[1] + LIFT;
    m[14] = position[2];
    m
}

/// Width / height of an embedded DDS or PNG (for placeholder detection).
pub fn image_dims(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() >= 20 && &b[0..4] == b"DDS " {
        let h = u32::from_le_bytes(b[12..16].try_into().ok()?);
        let w = u32::from_le_bytes(b[16..20].try_into().ok()?);
        return Some((w, h));
    }
    if b.len() >= 24 && &b[1..4] == b"PNG" {
        let w = u32::from_be_bytes(b[16..20].try_into().ok()?);
        let h = u32::from_be_bytes(b[20..24].try_into().ok()?);
        return Some((w, h));
    }
    None
}

impl<'s> Scan<'s> {
    pub fn new(src: &'s Source) -> Result<Scan<'s>> {
        let mut notes = src.notes.clone();
        // ---- kn5 files, parsed once
        let mut models: Vec<Model<'s>> = Vec::new();
        for (file, map) in &src.maps {
            let k: Kn5<'s> = kn5::parse(map).with_context(|| format!("{}: {file}", src.folder))?;
            let mut nodes = Vec::new();
            flatten(k.root, None, kn5::IDENTITY, true, &mut nodes);
            models.push(Model { file: file.clone(), version: k.version, textures: k.textures, materials: k.materials, nodes, mat_global: Vec::new(), tex_global: Vec::new() });
        }
        // ---- textures: one global entry per (name, content)
        let skin_dir = child_ci(&src.dir, "skins").and_then(|s| child_ci(&s, "cm_skins")).and_then(|d| {
            let mut v: Vec<PathBuf> = std::fs::read_dir(d).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
            v.sort();
            v.into_iter().next()
        });
        let mut textures: Vec<TexRef> = Vec::new();
        let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
        for (mi, m) in models.iter_mut().enumerate() {
            let stem = m.file.rsplit_once('.').map(|x| x.0).unwrap_or(&m.file).to_string();
            for (ti, t) in m.textures.iter().enumerate() {
                if !t.active {
                    m.tex_global.push(None);
                    continue;
                }
                let h = fnv(t.data);
                let low = t.name.to_lowercase();
                let same = by_name.get(&low).and_then(|v| v.iter().find(|&&g| textures[g].hash == h).copied());
                let g = match same {
                    Some(g) => g,
                    None => {
                        let taken = by_name.contains_key(&low);
                        let name = if taken { format!("{stem}__{}", t.name) } else { t.name.clone() };
                        let tiny = image_dims(t.data).map(|(w, h)| w <= 2 && h <= 2).unwrap_or(false);
                        let skin_file = if tiny { skin_dir.as_ref().and_then(|d| child_ci(d, &t.name)) } else { None };
                        textures.push(TexRef { name, model: mi, local: ti, hash: h, skin_file });
                        by_name.entry(low).or_default().push(textures.len() - 1);
                        textures.len() - 1
                    }
                };
                m.tex_global.push(Some(g));
            }
        }
        // ---- materials: one global entry per (name, content)
        let mut materials: Vec<MatRef> = Vec::new();
        let mut mat_sig: Vec<u64> = Vec::new();
        let mut mat_names: HashMap<String, Vec<usize>> = HashMap::new();
        for mi in 0..models.len() {
            let stem = models[mi].file.rsplit_once('.').map(|x| x.0).unwrap_or(&models[mi].file).to_string();
            let mut glob = Vec::new();
            for (li, mat) in models[mi].materials.iter().enumerate() {
                let mut sig = format!("{}|{}|{}|{}|", mat.shader, mat.blend_mode, mat.alpha_tested, mat.depth_mode);
                for (n, p) in &mat.props {
                    sig += &format!("{n}={:?}{:?}{:?}{:?};", p.value, p.v2, p.v3, p.v4);
                }
                for (slot, tex) in &mat.textures {
                    // the texture's content, not just its name
                    let th = models[mi].textures.iter().position(|t| t.active && t.name.eq_ignore_ascii_case(tex)).and_then(|i| models[mi].tex_global[i]).map(|g| textures[g].hash).unwrap_or(0);
                    sig += &format!("{slot}:{tex}:{th:x};");
                }
                let h = fnv(sig.as_bytes());
                let low = mat.name.to_lowercase();
                let found = mat_names.get(&low).and_then(|v| v.iter().find(|&&g| mat_sig[g] == h).copied());
                let g = match found {
                    Some(g) => g,
                    None => {
                        let name = if mat_names.contains_key(&low) { format!("{}__{stem}", mat.name) } else { mat.name.clone() };
                        materials.push(MatRef { name, model: mi, local: li });
                        mat_sig.push(h);
                        mat_names.entry(low).or_default().push(materials.len() - 1);
                        materials.len() - 1
                    }
                };
                glob.push(g);
            }
            models[mi].mat_global = glob;
        }
        // ---- layouts: ui, surfaces, ai lines, data ini files
        let ac1_root = src.dir.parent().and_then(|p| p.parent()).and_then(|p| p.parent()).map(|p| p.to_path_buf());
        let global_ini = ac1_root.as_ref().and_then(|r| read_text(&r.join("system").join("data").join("surfaces.ini"))).unwrap_or_else(|| AC1_GLOBAL_SURFACES.to_string());
        let mut surfaces: Vec<Surface> = Vec::new();
        let mut layouts = Vec::new();
        for (li, l) in src.layouts.iter().enumerate() {
            let surfaces_ini = child_ci(&l.data_dir, "surfaces.ini").and_then(|p| read_text(&p)).map(|t| ini::parse(&t)).unwrap_or_default();
            for mut s in surfaces_of(&surfaces_ini, false) {
                match surfaces.iter_mut().find(|x| x.key == s.key) {
                    Some(x) => x.layouts |= 1 << li,
                    None => {
                        s.layouts = 1 << li;
                        surfaces.push(s);
                    }
                }
            }
            let mut data = HashMap::new();
            if let Ok(rd) = std::fs::read_dir(&l.data_dir) {
                for e in rd.flatten() {
                    let p = e.path();
                    let n = p.file_name().unwrap().to_string_lossy().to_lowercase();
                    if n.ends_with(".ini") && n != "surfaces.ini" {
                        if let Some(t) = read_text(&p) {
                            data.insert(n, ini::parse(&t));
                        }
                    }
                }
            }
            let ai_of = |n: &str| -> Option<AiLine> {
                let p = child_ci(&l.ai_dir, n)?;
                match std::fs::read(&p).map_err(anyhow::Error::from).and_then(|b| ai::parse(&b)) {
                    Ok(mut a) => {
                        for pt in &mut a.points {
                            pt.pos[1] += LIFT;
                        }
                        Some(a)
                    }
                    Err(_) => None,
                }
            };
            let ui = l.ui_json.as_ref().and_then(|p| TrackUi::read(p)).unwrap_or_default();
            layouts.push(Layout { name: l.name.clone(), ui, surfaces_ini, fast_lane: ai_of("fast_lane.ai"), pit_lane: ai_of("pit_lane.ai"), dummies: Vec::new(), data });
        }
        let globals = surfaces_of(&ini::parse(&global_ini), true);
        // ---- placed meshes and dummies
        let mut meshes: Vec<MeshInst> = Vec::new();
        let mut placed: HashMap<(String, [u32; 6]), Vec<usize>> = HashMap::new();
        let mut unknown_keys: HashMap<String, usize> = HashMap::new();
        for (li, l) in src.layouts.iter().enumerate() {
            for mref in &l.models {
                let Some(mi) = models.iter().position(|m| m.file.eq_ignore_ascii_case(&mref.file)) else { continue };
                let place = placement(mref.position, mref.rotation);
                let key = (mref.file.to_lowercase(), [mref.position[0].to_bits(), mref.position[1].to_bits(), mref.position[2].to_bits(), mref.rotation[0].to_bits(), mref.rotation[1].to_bits(), mref.rotation[2].to_bits()]);
                for n in &models[mi].nodes {
                    if n.name.to_uppercase().starts_with("AC_") && n.mesh.is_none() {
                        layouts[li].dummies.push(Dummy { name: n.name.clone(), world: kn5::mul(&n.world, &place) });
                    }
                }
                if let Some(ids) = placed.get(&key) {
                    for &i in ids {
                        meshes[i].layouts |= 1 << li;
                    }
                    continue;
                }
                let mut ids = Vec::new();
                for (ni, n) in models[mi].nodes.iter().enumerate() {
                    let Some(m) = &n.mesh else { continue };
                    if !n.shown || m.vertex_count == 0 || m.index_count() < 3 {
                        continue;
                    }
                    let material = models[mi].mat_global.get(m.material_id.max(0) as usize).copied().unwrap_or(0);
                    let drawn = m.visible && m.renderable;
                    let role = match physics_key(&n.name, &surfaces, &globals) {
                        Some(Ok(k)) => {
                            let si = match surfaces.iter().position(|s| s.key == k) {
                                Some(i) => i,
                                None => {
                                    surfaces.push(globals.iter().find(|g| g.key == k).cloned().unwrap());
                                    surfaces.len() - 1
                                }
                            };
                            Role::Physics { surface: si, drawn }
                        }
                        Some(Err(rest)) => {
                            // no surface key: AC1 still collides with it (it has no
                            // wall category; a "wall" is a steep physics mesh). Ours:
                            // steep or wall-named -> an EVO wall, else the default ROAD
                            *unknown_keys.entry(rest.clone()).or_default() += 1;
                            let wall = has_wall_word(&rest) || horizontal_share(m, &kn5::mul(&n.world, &place)) < 0.5;
                            let key = if wall { "WALL" } else { "ROAD" };
                            let si = match surfaces.iter().position(|s| s.key == key) {
                                Some(i) => i,
                                None => {
                                    let s = globals.iter().find(|g| g.key == key).cloned()
                                        .unwrap_or(Surface { key: key.into(), friction: 1.0, global: true, ..Default::default() });
                                    surfaces.push(s);
                                    surfaces.len() - 1
                                }
                            };
                            Role::Physics { surface: si, drawn }
                        }
                        None => Role::Visual,
                    };
                    if role == Role::Visual && !drawn {
                        continue;
                    }
                    meshes.push(MeshInst { model: mi, node: ni, world: kn5::mul(&n.world, &place), layouts: 1 << li, material, role, name: n.name.clone() });
                    ids.push(meshes.len() - 1);
                }
                placed.insert(key, ids);
            }
        }
        if !unknown_keys.is_empty() {
            let mut v: Vec<(String, usize)> = unknown_keys.into_iter().collect();
            v.sort();
            notes.push(format!("physics meshes without a surface key (steep -> WALL, else ROAD): {}", v.iter().take(12).map(|(k, n)| format!("{k} x{n}")).collect::<Vec<_>>().join(", ")));
        }
        // ---- CSP config
        let csp = child_ci(&src.dir, "extension").and_then(|e| child_ci(&e, "ext_config.ini")).or_else(|| {
            let c = ac1_root.as_ref()?.join("extension").join("config").join("tracks");
            child_ci(&c.join("loaded"), &format!("{}.ini", src.folder)).or_else(|| child_ci(&c, &format!("{}.ini", src.folder)))
        }).and_then(|p| read_text(&p).map(|t| (p, ini::parse(&t))));
        src.trim();
        Ok(Scan { src, folder: src.folder.clone(), layouts, models, materials, textures, surfaces, meshes, csp, notes, touched: Default::default() })
    }

    /// The kn5 mesh of a placed instance.
    pub fn mesh(&self, m: &MeshInst) -> &Mesh<'s> {
        self.models[m.model].nodes[m.node].mesh.as_ref().unwrap()
    }

    /// The kn5 material of a global material index.
    pub fn material(&self, g: usize) -> &Material {
        let r = &self.materials[g];
        &self.models[r.model].materials[r.local]
    }

    /// The image bytes of a global texture (a CM skin file for placeholders).
    pub fn texture_bytes(&self, g: usize) -> std::borrow::Cow<'_, [u8]> {
        let t = &self.textures[g];
        self.touch(self.models[t.model].textures[t.local].data.len());
        if let Some(b) = t.skin_file.as_ref().and_then(|p| std::fs::read(p).ok()) {
            return std::borrow::Cow::Owned(b);
        }
        std::borrow::Cow::Borrowed(self.models[t.model].textures[t.local].data)
    }

    /// The global texture a material slot points at (`txDiffuse`, ...).
    pub fn material_texture(&self, g: usize, slot: &str) -> Option<usize> {
        let r = &self.materials[g];
        let m = &self.models[r.model];
        let name = m.materials[r.local].texture(slot)?;
        let i = m.textures.iter().position(|t| t.active && t.name.eq_ignore_ascii_case(name))?;
        m.tex_global[i]
    }

    /// Count kn5 bytes read; trim the maps every TRIM_EVERY bytes.
    pub fn touch(&self, bytes: usize) {
        use std::sync::atomic::Ordering;
        if self.touched.fetch_add(bytes, Ordering::Relaxed) + bytes > TRIM_EVERY {
            self.touched.store(0, Ordering::Relaxed);
            self.src.trim();
        }
    }

    /// World-space positions of a placed mesh's vertices.
    pub fn positions(&self, m: &MeshInst) -> Vec<[f32; 3]> {
        let mesh = self.mesh(m);
        // a kn5 vertex is 44 bytes (position, normal, uv, tangent)
        self.touch(mesh.vertex_count * 44 + mesh.index_count() * 2);
        (0..mesh.vertex_count).map(|v| kn5::xform(&m.world, mesh.position(v))).collect()
    }

    /// Triangle index triples of a placed mesh (degenerate / out-of-range dropped).
    pub fn triangles(&self, m: &MeshInst) -> Vec<[u32; 3]> {
        let mesh = self.mesh(m);
        let nv = mesh.vertex_count as u32;
        (0..mesh.index_count() / 3).map(|t| [mesh.index(t * 3) as u32, mesh.index(t * 3 + 1) as u32, mesh.index(t * 3 + 2) as u32])
            .filter(|t| t.iter().all(|&i| i < nv) && t[0] != t[1] && t[1] != t[2] && t[0] != t[2])
            .collect()
    }

    /// Layout bit mask with every layout set.
    pub fn all_layouts(&self) -> u64 {
        if self.layouts.len() >= 64 { u64::MAX } else { (1u64 << self.layouts.len()) - 1 }
    }

    /// One-line summary per layout (scan report).
    pub fn report(&self) -> Vec<String> {
        let mut out = Vec::new();
        out.push(format!("{}: {} kn5, {} materials, {} textures, {} placed meshes ({} physics), {} surfaces",
            self.folder, self.models.len(), self.materials.len(), self.textures.len(), self.meshes.len(),
            self.meshes.iter().filter(|m| matches!(m.role, Role::Physics { .. })).count(), self.surfaces.len()));
        for (i, l) in self.layouts.iter().enumerate() {
            let n = |p: &str| l.dummies_with(p).len();
            out.push(format!("  layout {:?} \"{}\": {} meshes, start {} pit {} hotlap {} time {}, fast_lane {} pts ({:.0} m), pit_lane {} pts",
                l.name, l.ui.name, self.meshes.iter().filter(|m| m.layouts & (1 << i) != 0).count(),
                n("AC_START_"), n("AC_PIT_"), n("AC_HOTLAP_START_"), n("AC_TIME_"),
                l.fast_lane.as_ref().map(|a| a.points.len()).unwrap_or(0), l.fast_lane.as_ref().map(|a| a.length()).unwrap_or(0.0),
                l.pit_lane.as_ref().map(|a| a.points.len()).unwrap_or(0)));
        }
        out.extend(self.notes.iter().map(|n| format!("  note: {n}")));
        out
    }
}

/// The surface key of a physics mesh name: `Some(Ok(key))` when it is
/// physical with a known key (the longest key the name starts with after
/// its digits, else the longest key of 3+ characters it contains),
/// `Some(Err(rest))` for a digit prefix without a key, `None` for no digit
/// prefix or the bare `0` prefix (decor). Kunos's own collision meshes are
/// `01WALL`, `03ROAD`...: only a lone `0` means decor (`0FENCE` on Barber).
pub fn physics_key(name: &str, track: &[Surface], global: &[Surface]) -> Option<std::result::Result<String, String>> {
    let digits = name.bytes().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 || &name[..digits] == "0" {
        return None;
    }
    let rest = name[digits..].to_uppercase();
    let all = || track.iter().chain(global.iter());
    let best = all().filter(|s| rest.starts_with(&s.key)).max_by_key(|s| s.key.len())
        .or_else(|| all().filter(|s| s.key.len() >= 3 && rest.contains(&s.key)).max_by_key(|s| s.key.len()));
    Some(match best {
        Some(s) => Ok(s.key.clone()),
        None => Err(rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect()),
    })
}

fn has_wall_word(rest: &str) -> bool {
    ["WALL", "FENCE", "BARRIER", "GUARD", "RAIL", "ARMCO", "TYRE", "TIRE"].iter().any(|w| rest.contains(w))
}

/// Area-weighted |normal.y| of a mesh's triangles (1 = flat ground, 0 = a wall).
fn horizontal_share(m: &Mesh, world: &[f32; 16]) -> f32 {
    let nv = m.vertex_count;
    let p: Vec<[f32; 3]> = (0..nv).map(|v| kn5::xform(world, m.position(v))).collect();
    let (mut a, mut ay) = (0f64, 0f64);
    for t in 0..m.index_count() / 3 {
        let (i, j, k) = (m.index(t * 3) as usize, m.index(t * 3 + 1) as usize, m.index(t * 3 + 2) as usize);
        if i >= nv || j >= nv || k >= nv {
            continue;
        }
        let u = [p[j][0] - p[i][0], p[j][1] - p[i][1], p[j][2] - p[i][2]];
        let v = [p[k][0] - p[i][0], p[k][1] - p[i][1], p[k][2] - p[i][2]];
        let c = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        let l = ((c[0] * c[0] + c[1] * c[1] + c[2] * c[2]) as f64).sqrt();
        if l.is_finite() {
            a += l;
            ay += (c[1] as f64).abs();
        }
    }
    if a > 0.0 { (ay / a) as f32 } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_convention() {
        // heading 90: AC1 local +Z (forward) turns to world +X, translation last
        let m = placement([10.0, 2.0, -5.0], [90.0, 0.0, 0.0]);
        let p = kn5::xform(&m, [0.0, 0.0, 1.0]);
        assert!((p[0] - 11.0).abs() < 1e-5 && (p[1] - 2.0 - LIFT).abs() < 1e-5 && (p[2] + 5.0).abs() < 1e-5, "{p:?}");
        // a node matrix composes before the placement (world = local * place)
        let mut node = kn5::IDENTITY;
        node[12] = 1.0;
        let w = kn5::mul(&node, &m);
        let q = kn5::xform(&w, [0.0; 3]);
        assert!((q[0] - 10.0).abs() < 1e-5 && (q[2] + 6.0).abs() < 1e-5, "{q:?}");
    }

    #[test]
    fn physics_names() {
        let s = |k: &str| Surface { key: k.into(), ..Default::default() };
        let track = vec![s("ROAD"), s("ROADX"), s("KERB")];
        assert_eq!(physics_key("1ROADX_12", &track, &[]), Some(Ok("ROADX".into())));
        assert_eq!(physics_key("12KERB", &track, &[]), Some(Ok("KERB".into())));
        assert_eq!(physics_key("0ROAD", &track, &[]), None);
        // Kunos collision meshes: 01WALL, 03ROAD
        assert_eq!(physics_key("03ROAD", &track, &[]), Some(Ok("ROAD".into())));
        assert_eq!(physics_key("15CNC_KERB", &track, &[]), Some(Ok("KERB".into())));
        assert_eq!(physics_key("ROAD", &track, &[]), None);
        assert_eq!(physics_key("3WALL", &track, &[]), Some(Err("WALL".into())));
        assert_eq!(physics_key("3GRASS", &track, &[s("GRASS")]), Some(Ok("GRASS".into())));
    }

    #[test]
    fn dummies() {
        assert!(is_dummy("AC_START_0") && is_dummy("ac_time_1_L") && !is_dummy("AC_POBJECT_cone"));
    }
}
