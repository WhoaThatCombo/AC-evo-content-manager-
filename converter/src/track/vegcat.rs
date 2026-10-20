//! The factory vegetation catalogue (requirement track-vegetation), built at
//! run time from the user's own content.kspkg: nothing of it is shipped.
//!
//! Game facts (stock content):
//! - the factory plants live under `content\tracks\common_assets\meshes\
//!   vegetation\`: `big\trees\<species>\...lod0.mesh` (3D trees: 4..5 LOD
//!   bodies, the last a camera-facing card from 150..450 m, lodOut 600..2500
//!   m), `big\trees\billboards\bb_<n>\` and the species cards `...\cf<n>.mesh`
//!   (camera-facing impostors, placed with zero rotation, lodOut 1500..5500
//!   m), `mid\` and `acc\mid\` (shrubs and bushes, lodOut 250..750 m), `low\
//!   grass\tracks_terrain_vegetation\` (the grass tufts every stock carpet
//!   uses: grass_2_b4 / grass_2_b3 hold 10.3M / 3.7M of the stock grass
//!   instances, lodOut 120..150 m);
//! - a mesh's MeshData f4 lodOut is the cull distance stock gives its ISMesh
//!   nodes (beech_7 1200, bush_05_b 450..550, grass_2_b3 120);
//! - mesh bounds (f6 / f7) start at the trunk base (y ~ 0): height = max y,
//!   crown width = the larger xz extent;
//! - leaf materials (Subsurface / Grass shaders) keep the leaf colour and its
//!   cut-out alpha in `txDiffuse`; all of them already read the track's
//!   pigment (`Pigment_Enable` 1).
//!
//! Ours:
//! - which meshes count: a file stem ending in `lod0`, the family from the
//!   folder (above), and size / cull filters per family ([`Family`]); crop
//!   fields (corn, sunflowers, wheat) and house plants (lodOut under 250 m)
//!   are left out, as are grass tufts made for one stock track (a folder
//!   named after a stock track id);
//! - colour: the mean linear colour of the opaque texels (alpha >= 0.5) of
//!   the leaf material's `txDiffuse` (the section with the most triangles
//!   that is not bark), from one small mip;
//! - climate affinities by species family (the table in [`affinity`]):
//!   tropical, subtropical / Mediterranean, temperate, boreal.

use rayon::prelude::*;
use std::collections::{HashMap, HashSet};

use crate::io::kspkg::Pkg;
use crate::io::pb::Msg;

pub const ROOT: &str = "content\\tracks\\common_assets\\meshes\\vegetation\\";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Family {
    /// 3D trees (big\trees)
    Tree,
    /// shrubs / bushes (mid)
    Bush,
    /// far cross impostors (big\trees\billboards)
    Impostor,
    /// grass tufts for the carpet (low\grass\tracks_terrain_vegetation)
    Grass,
}

#[derive(Clone, Debug)]
pub struct Species {
    /// archive path as stored in the package
    pub path: String,
    /// short name for reports (folder + stem)
    pub name: String,
    pub family: Family,
    /// metres, from the mesh bounds
    pub height: f32,
    /// crown width (larger xz extent), metres
    pub width: f32,
    /// MeshData lodOut: the stock cull distance
    pub lod_out: f32,
    /// mean linear leaf colour
    pub colour: [f32; 3],
    pub conifer: bool,
    pub palm: bool,
    pub dead: bool,
    /// tropical, subtropical / Mediterranean, temperate, boreal (0..1)
    pub climate: [f32; 4],
}

impl Species {
    pub fn ratio(&self) -> f32 {
        self.width / self.height.max(0.01)
    }
}

#[derive(Default)]
pub struct Catalogue {
    pub species: Vec<Species>,
    pub notes: Vec<String>,
}

impl Catalogue {
    pub fn of(&self, f: Family) -> impl Iterator<Item = (usize, &Species)> {
        self.species.iter().enumerate().filter(move |(_, s)| s.family == f)
    }
    pub fn count(&self, f: Family) -> usize {
        self.of(f).count()
    }
}

/// Climate affinity of a species by its family name (OURS: botanical
/// ranges of the genera, coarse). Order: tropical, subtropical /
/// Mediterranean, temperate, boreal.
pub fn affinity(name: &str) -> [f32; 4] {
    let n = name.to_ascii_lowercase();
    let has = |w: &[&str]| w.iter().any(|x| n.contains(x));
    if has(&["palm"]) {
        [1.0, 0.7, 0.05, 0.0]
    } else if has(&["stonepine", "aleppo", "cypress", "cedar"]) {
        [0.3, 1.0, 0.35, 0.05]
    } else if has(&["peppermint", "eucalypt"]) {
        [0.6, 1.0, 0.3, 0.0]
    } else if has(&["spruce", "fir", "larch"]) {
        [0.0, 0.15, 0.5, 1.0]
    } else if has(&["birch", "aspen", "alder"]) {
        [0.0, 0.25, 0.9, 1.0]
    } else if has(&["beech", "maple", "plane", "poplar", "oak"]) {
        [0.15, 0.6, 1.0, 0.45]
    } else if has(&["desert", "saltbush"]) {
        [0.6, 1.0, 0.4, 0.1]
    } else {
        // shrubs, generic bushes, impostors, grass: everywhere
        [0.8, 0.9, 1.0, 0.8]
    }
}

/// Climate weights of a place (OURS): smooth bands of absolute latitude.
pub fn climate_of(latitude: f64, located: bool) -> [f32; 4] {
    if !located {
        return [0.25, 0.5, 1.0, 0.4];
    }
    let a = latitude.abs() as f32;
    let band = |lo: f32, hi: f32, soft: f32| -> f32 {
        let up = ((a - (lo - soft)) / (2.0 * soft)).clamp(0.0, 1.0);
        let down = (((hi + soft) - a) / (2.0 * soft)).clamp(0.0, 1.0);
        up.min(down)
    };
    [band(-90.0, 23.0, 4.0), band(23.0, 41.0, 4.0), band(39.0, 56.0, 4.0), band(55.0, 90.0, 4.0)]
}

fn stem_of(path: &str) -> &str {
    let f = path.rsplit('\\').next().unwrap_or("");
    f.strip_suffix(".mesh").unwrap_or(f)
}

fn stem_is_lod0(path: &str) -> bool {
    let s = stem_of(path);
    s == "lod0" || s.ends_with("_lod0")
}

/// A camera-facing card of a tree species (`cf1`, `cf2`, `cf1_a`, ...).
fn stem_is_card(path: &str) -> bool {
    let s = stem_of(path);
    s.strip_prefix("cf").map(|r| !r.is_empty() && r.chars().next().unwrap().is_ascii_digit() && r.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')).unwrap_or(false)
}

/// The family a vegetation mesh path belongs to (path relative to ROOT,
/// lower case), before the size filters.
pub fn family_of(rel: &str) -> Option<Family> {
    if ["corn", "sunflower", "wheat"].iter().any(|w| rel.contains(w)) {
        return None;
    }
    if rel.starts_with("big\\trees\\billboards\\") || (rel.starts_with("big\\trees\\") && stem_is_card(rel)) {
        Some(Family::Impostor)
    } else if rel.starts_with("big\\") || rel.starts_with("acc\\big\\") {
        Some(Family::Tree)
    } else if rel.starts_with("mid\\") || rel.starts_with("acc\\mid\\") {
        Some(Family::Bush)
    } else if rel.starts_with("low\\grass\\tracks_terrain_vegetation\\") {
        Some(Family::Grass)
    } else {
        None
    }
}

/// The class of any factory vegetation mesh path, for statistics (stock
/// and converted alike): "impostor" (billboards, species cards), "tree"
/// (big), "bush" (mid), "grass" (low: tufts, weeds, flowers).
pub fn stats_class(path: &str) -> Option<&'static str> {
    let l = path.to_lowercase();
    let rel = l.strip_prefix(&ROOT.to_lowercase())?;
    if rel.starts_with("big\\trees\\billboards\\") || (rel.starts_with("big\\") && stem_is_card(rel)) {
        Some("impostor")
    } else if rel.starts_with("big\\") || rel.starts_with("acc\\big\\") {
        Some("tree")
    } else if rel.starts_with("mid\\") || rel.starts_with("acc\\mid\\") {
        Some("bush")
    } else if rel.starts_with("low\\") || rel.starts_with("acc\\low\\") {
        Some("grass")
    } else {
        None
    }
}

fn v3(m: Option<Msg>) -> [f32; 3] {
    let m = m.unwrap_or_default();
    [m.f32(1).unwrap_or(0.0), m.f32(2).unwrap_or(0.0), m.f32(3).unwrap_or(0.0)]
}

struct MeshFacts {
    lo: [f32; 3],
    hi: [f32; 3],
    lod_out: f32,
    lods: usize,
    /// (material path, triangles) of LOD0
    sections: Vec<(String, usize)>,
}

fn mesh_facts(b: &[u8]) -> Option<MeshFacts> {
    let m = Msg::parse(b).ok()?;
    let lods = m.subs(5);
    let l0 = lods.first()?;
    let sections = l0.subs(4).iter().map(|s| (s.str(4).unwrap_or_default(), s.varint(3).unwrap_or(0) as usize / 3)).collect();
    Some(MeshFacts { lo: v3(m.sub(6)), hi: v3(m.sub(7)), lod_out: m.f32(4).unwrap_or(0.0), lods: lods.len(), sections })
}

/// Mean linear colour of the opaque texels of a `.texture` in the package.
fn texture_colour(pkg: &Pkg, index: &HashMap<String, usize>, path: &str) -> Option<[f32; 3]> {
    let h = pkg.read(&pkg.entries[*index.get(&path.to_lowercase())?]).ok()?;
    let mips_path = format!("{}mips", path.to_lowercase());
    let me = &pkg.entries[*index.get(&mips_path)?];
    let raw = pkg.raw(me)?;
    let plain = Pkg::is_plain(me);
    let page = |i: usize| -> Option<Vec<u8>> {
        let mut p = raw.get(i * 65536..(i + 1) * 65536)?.to_vec();
        if !plain {
            // pages of an XOR'd file: the key restarts at the file start, and
            // 65536 is a multiple of 8, so a page decodes on its own
            crate::io::kspkg::xor(&mut p);
        }
        Some(p)
    };
    let (_, _, px, srgb) = crate::stats::albedo::texture_small(&h, &page)?;
    opaque_mean(&px, srgb)
}

/// Mean linear colour of the texels with alpha >= 0.5 (all texels when none is).
pub fn opaque_mean(px: &[[u8; 4]], srgb: bool) -> Option<[f32; 3]> {
    let conv = |v: u8| if srgb { super::textures::lin(v) } else { v as f32 / 255.0 };
    let mut acc = [0f64; 3];
    let mut n = 0usize;
    for pass in 0..2 {
        for p in px {
            if pass == 0 && p[3] < 128 {
                continue;
            }
            for c in 0..3 {
                acc[c] += conv(p[c]) as f64;
            }
            n += 1;
        }
        if n > 0 {
            break;
        }
    }
    (n > 0).then(|| acc.map(|a| (a / n as f64) as f32))
}

/// Leaf colour of a mesh: its largest non-bark section's diffuse texture.
fn leaf_colour(pkg: &Pkg, index: &HashMap<String, usize>, sections: &[(String, usize)]) -> Option<[f32; 3]> {
    let mut s: Vec<&(String, usize)> = sections.iter().filter(|(p, _)| {
        let l = p.to_lowercase();
        !l.contains("bark") && !l.contains("trunk")
    }).collect();
    if s.is_empty() {
        s = sections.iter().collect();
    }
    s.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    for (mat, _) in s {
        let Some(&i) = index.get(&mat.to_lowercase()) else { continue };
        let Ok(b) = pkg.read(&pkg.entries[i]) else { continue };
        let Ok(m) = Msg::parse(&b) else { continue };
        for slot in ["txDiffuse", "Base_BaseColorMap"] {
            if let Some(t) = crate::io::material::get_slot(&m, slot) {
                if t.to_lowercase().starts_with("editor\\") {
                    continue;
                }
                if let Some(c) = texture_colour(pkg, index, &t) {
                    return Some(c);
                }
            }
        }
    }
    None
}

/// Build the catalogue from a package.
pub fn load(pkg: &Pkg) -> Catalogue {
    let mut cat = Catalogue::default();
    let index: HashMap<String, usize> = pkg.entries.iter().enumerate().map(|(i, e)| (e.path.to_lowercase(), i)).collect();
    // stock track ids (grass made for one track is left out)
    let tracks: HashSet<String> = pkg.entries.iter().filter_map(|e| {
        let l = e.path.to_lowercase();
        l.strip_prefix("content\\tracks\\").and_then(|r| r.split_once('\\')).map(|(id, _)| id.to_string())
    }).filter(|t| t != "common_assets").collect();
    let mut cands: Vec<(usize, String, Family)> = pkg.entries.iter().enumerate().filter_map(|(i, e)| {
        let l = e.path.to_lowercase();
        let rel = l.strip_prefix(&ROOT.to_lowercase())?.to_string();
        if !rel.ends_with(".mesh") || !(stem_is_lod0(&rel) || stem_is_card(&rel)) || rel.contains("collider") {
            return None;
        }
        let f = family_of(&rel)?;
        if f == Family::Grass && rel.split('\\').any(|p| tracks.contains(p)) {
            return None;
        }
        Some((i, rel, f))
    }).collect();
    cands.sort_by(|a, b| a.1.cmp(&b.1));
    let found: Vec<Option<Species>> = cands.par_iter().map(|(i, rel, f)| {
        let e = &pkg.entries[*i];
        let b = pkg.read(e).ok()?;
        let mf = mesh_facts(&b)?;
        let height = mf.hi[1] - mf.lo[1].max(0.0);
        let width = (mf.hi[0] - mf.lo[0]).max(mf.hi[2] - mf.lo[2]);
        let ok = match f {
            Family::Tree => mf.lods >= 3 && (5.0..=45.0).contains(&height) && mf.lod_out >= 500.0,
            Family::Bush => (1.0..=5.0).contains(&height) && mf.lod_out >= 250.0 && width >= 0.4 * height,
            Family::Impostor => (5.0..=30.0).contains(&height) && mf.lod_out >= 1000.0,
            Family::Grass => (0.08..=0.8).contains(&height) && (80.0..=250.0).contains(&mf.lod_out),
        };
        if !ok || !width.is_finite() || width <= 0.0 {
            return None;
        }
        let colour = leaf_colour(pkg, &index, &mf.sections)?;
        let name = ["acc\\", "big\\trees\\billboards\\", "big\\trees\\", "big\\", "mid\\", "low\\grass\\tracks_terrain_vegetation\\"].iter()
            .fold(rel.trim_end_matches(".mesh").to_string(), |n, p| n.strip_prefix(p).map(str::to_string).unwrap_or(n)).replace('\\', "/");
        let conifer = ["spruce", "fir", "pine", "cedar", "cypress", "larch"].iter().any(|w| rel.contains(w)) && !rel.contains("stonepine") && !rel.contains("aleppo");
        Some(Species {
            path: e.path.clone(), name, family: *f, height, width, lod_out: mf.lod_out, colour,
            conifer, palm: rel.contains("palm"), dead: rel.contains("dead"), climate: affinity(rel),
        })
    }).collect();
    // duplicates (the same model exported twice) keep the first path
    let mut seen = HashSet::new();
    for s in found.into_iter().flatten() {
        let stem = s.name.rsplit('/').next().unwrap_or("").to_string();
        let key = (s.family, (s.height * 100.0) as i32, (s.width * 100.0) as i32, stem);
        if seen.insert(key) {
            cat.species.push(s);
        }
    }
    for f in [Family::Tree, Family::Bush, Family::Impostor, Family::Grass] {
        if cat.count(f) == 0 {
            cat.notes.push(format!("vegetation catalogue: no {f:?} meshes found in the game package"));
        }
    }
    cat
}

/// A catalogue from explicit species (tests, offline runs).
pub fn from_species(species: Vec<Species>) -> Catalogue {
    Catalogue { species, notes: Vec::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families_by_folder() {
        assert_eq!(family_of("big\\trees\\beech_7\\lod0.mesh"), Some(Family::Tree));
        assert_eq!(family_of("big\\trees\\billboards\\bb_3\\bb_3_lod0.mesh"), Some(Family::Impostor));
        assert_eq!(family_of("acc\\mid\\bush_05\\bush_05_b_lod0.mesh"), Some(Family::Bush));
        assert_eq!(family_of("mid\\corn\\corn_lp_agemature_a_lod0.mesh"), None);
        assert_eq!(family_of("low\\grass\\tracks_terrain_vegetation\\grass_2\\grass_2_b3_lod0.mesh"), Some(Family::Grass));
        assert!(stem_is_lod0("big\\trees\\beech_7\\lod0.mesh") && stem_is_lod0("x\\shrub_04_lod0.mesh") && !stem_is_lod0("x\\lod0_1.mesh"));
    }

    #[test]
    fn climate_bands() {
        let c = climate_of(43.0, true);
        assert!(c[2] > 0.5 && c[1] > 0.0 && c[0] == 0.0);
        let t = climate_of(3.0, true);
        assert!(t[0] == 1.0 && t[2] == 0.0);
        assert!(affinity("big\\trees\\palm_2\\palm_2_lod0.mesh")[0] > 0.9);
        assert!(affinity("spruce_norway_custom_1")[3] > 0.9);
    }
}
