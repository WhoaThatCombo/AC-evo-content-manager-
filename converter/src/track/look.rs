//! What each AC1 material should look like in EVO: the shader family it
//! maps to, its brightness gain, its layers, and the per-mesh variants that
//! CSP and the AC1 semaphore add (night emissive, blinking, start lights).
//! materials.rs turns these decisions into `.material` files, textures.rs
//! into textures.
//!
//! # AC1 shading (read off the AC1 install's compiled shaders)
//!
//! `system\shaders\win\*_ps.fxo` disassembled with the Windows SDK's fxc:
//! - every ks* shader: colour = albedo x (ksAmbient x ambient x hemisphere +
//!   ksEmissive + ksDiffuse x sun x N.L x shadow) + specular;
//! - `ksMultilayer`, `ksMultilayer_fresnel_nm`: albedo = txDiffuse(uv0) x
//!   (Σ_c txMask(uv0).c x txDetailC(world.xz x multC)) x magicMult over the
//!   four channels R, G, B, A (the vertex shader passes the world position:
//!   detail layers are planar-mapped from above in world space);
//! - `ksMultilayer_objsp`: the same with object-space coordinates;
//! - the alpha-tested shaders do not read ksAlphaRef (alpha to coverage).
//!
//! # EVO layered track shader (the game's DXIL, `L:\evoview_work\shaderre`)
//!
//! The track uber shader behind DynamicTrack / DynamicTrackMultilayer
//! (pixel shader 0x034e5ce0 and its permutations) weighs a Base layer and
//! three layers Red / Green / Blue by the SpatMap (or Splatmap_Constant):
//! w_base = saturate(1 - r - g - b), then all four are normalised; with
//! Splatmap_DisableRestrictBlendToTwoLayers = 0 the smallest of r, g, b is
//! dropped. Sum mode (Splatmap_MultiplyBaseColor = 0): albedo = w_base x
//! Base + Σ w_c x Layer_c; multiply mode: Π lerp(1, layer, w). Then the
//! "Detail" step: albedo *= lerp(Detail_BaseColorMultiplier x Base, 1,
//! w_base). A layer with X_Triplanar samples (camera + position).xz x
//! (X_TriplanarScale.xz + 0.01).
//!
//! So AC1's multilayer maps exactly onto sum mode: the three heaviest mask
//! channels become Red / Green / Blue (planar world mapping at multC -
//! 0.01), Base = txDiffuse, Detail multiplier = magicMult: where the mask
//! covers the layers, albedo = (Σ w_c L_c) x magicMult x txDiffuse. The
//! fourth channel (and any channel with mult 0, a constant texel) has no
//! layer left: its mean colour x magicMult is baked into the Base map where
//! its mask weight sits (ours), which is also what w_base then shows.
//!
//! # Brightness (requirement track-textures-materials)
//!
//! Each AC1 material is shaded with the AC1 formula above under one fixed
//! reference light, and the result is taken to linear light
//! ([`rendered_light`]); materials.rs then scales each class so that the
//! track's class median lands in the stock interquartile range.
//!
//! The reference light (`tools/ac1_lighting.py`, output in
//! `stockstats/measure_ac1_lighting.txt`):
//! - sun: `system\cfg\colorCurves.ini` [SUN] HIGH = (170, 170, 160) x 20,
//!   luma [`SUN_LUMA`]; at [`SUN_ELEVATION_DEG`] degrees, the most common
//!   SUN_PITCH_ANGLE of the AC1 install's track `lighting.ini` files (22 of
//!   68); the surface faces up (N.L = sin 35 deg), unshadowed;
//! - ambient: [AMBIENT] HIGH = (85, 105, 128) x 10, luma [`AMBIENT_LUMA`];
//!   the shader's hemisphere factor saturate((N.y / 2 + 1/2) / 2 + 1/2) is 1
//!   for an upward surface;
//! - exposure: `ppfilters\default.ini` TONEMAPPING EXPOSURE [`EXPOSURE`].
//!
//! AC1 does that sum on raw (gamma-encoded) texture values and shows the
//! result as gamma-encoded: the shown colour is tex x E x exposure with E =
//! ksAmbient x ambient + ksDiffuse x sun x N.L (ksEmissive is left out: it
//! becomes an EVO emissive, look variants below). In linear light that is
//! lin(tex) x (E x exposure)^2.2; a multilayer adds lin(sum m x detail) x
//! magicMult^2.2 (products of gamma values stay products in linear light).

use std::collections::HashMap;

use super::classes::{material_classes, MatClass};
use super::scan::{Role, Scan};
use crate::ac1::ini::{self, Ini};

/// AC1 shades in gamma space (its shaders multiply raw texture samples by
/// the material and light factors, and the tonemapper writes the result
/// with no encoding curve): a scalar factor k there is k^2.2 in linear
/// light. Products of two textures need no change ((ab)^2.2 = a^2.2 b^2.2).
pub fn gamma_factor(k: f32) -> f32 {
    k.max(0.0).powf(2.2)
}

/// luma of colorCurves.ini [SUN] HIGH: (0.2126 x 170 + 0.7152 x 170 +
/// 0.0722 x 160) x 20 / 255
pub const SUN_LUMA: f32 = 13.2767;
/// luma of colorCurves.ini [AMBIENT] HIGH: (85, 105, 128) x 10 / 255
pub const AMBIENT_LUMA: f32 = 4.0160;
/// sun elevation: the mode of the AC1 tracks' SUN_PITCH_ANGLE
pub const SUN_ELEVATION_DEG: f32 = 35.0;
/// ppfilters\default.ini [TONEMAPPING] EXPOSURE
pub const EXPOSURE: f32 = 0.28;

/// What the AC1 reference light makes of a material with these factors, in
/// linear light, per unit of linear texture albedo: (E x exposure)^2.2.
pub fn rendered_light(ks_ambient: f32, ks_diffuse: f32) -> f32 {
    let e = ks_ambient * AMBIENT_LUMA + ks_diffuse * SUN_LUMA * SUN_ELEVATION_DEG.to_radians().sin();
    gamma_factor(e * EXPOSURE)
}

/// One AC1 multilayer, as EVO layers.
#[derive(Clone, Debug)]
pub struct Multi {
    /// global texture of txDiffuse / txMask
    pub diffuse: Option<usize>,
    pub mask: Option<usize>,
    /// up to three layers: (mask channel 0..4, detail texture, mult)
    pub layers: Vec<(usize, usize, f32)>,
    /// channels without a layer: (mask channel, detail texture or None)
    pub baked: Vec<(usize, Option<usize>)>,
    pub magic: f32,
    /// planar world mapping (ksMultilayer, _fresnel_nm); object-space
    /// multilayers (`_objsp`) bake every channel instead
    pub world: bool,
    /// layers mapped by uv0 x mult (the MultiMap detail) instead of world xz
    pub uv: bool,
    /// the part no channel covers shows the base as it is (the MultiMap
    /// detail lerp); a multilayer's uncovered part is black, as in AC1
    pub remainder: bool,
}

#[derive(Clone, Debug)]
pub enum Shade {
    /// DefaultMaterial, opaque
    Plain,
    /// DefaultMaterial with alpha to coverage (AC1 alpha-tested)
    AlphaTest,
    /// DefaultMaterial alpha-blended (AC1 blended glass / nets with depth write)
    Blend,
    /// DefaultMaterial alpha-blended drawn before the surface under it
    /// (AC1 blended without depth write: skid marks, grass edges, decals)
    Decal,
    /// DefaultMaterial with the layers' brightness baked into its map
    Baked(Multi),
    /// DynamicTrackMultilayer
    Layered(Multi),
    /// DynamicTrack (tarmac); layers when the AC1 road is a multilayer
    Road(Option<Multi>),
}

/// A material variant some meshes need beyond their AC1 material.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Variant {
    /// static night emissive (CSP MATERIAL_ADJUSTMENT ksEmissive under a
    /// night condition): linear colour, EVO intensity
    pub emissive: Option<([f32; 3], f32)>,
    /// flashing (CSP condition with FLASHING_FREQUENCY): Hz, colour
    pub blink: Option<(f32, [f32; 3])>,
    /// AC1 semaphore object type (0 disqualified, 1 ready / red, 2 start / green)
    pub start_light: Option<u8>,
    /// human-readable origin, for the report
    pub why: String,
}

pub struct Looks {
    pub shade: Vec<Shade>,
    /// per global material
    pub gain: Vec<f32>,
    pub roughness: Vec<f32>,
    pub class: Vec<MatClass>,
    /// index 0 is the plain material
    pub variants: Vec<Variant>,
    /// per scan mesh: index into `variants`
    pub of_mesh: Vec<u16>,
    pub notes: Vec<String>,
}

fn prop(scan: &Scan, g: usize, name: &str) -> Option<f32> {
    scan.material(g).prop(name).map(|p| p.value)
}

fn prop4(scan: &Scan, g: usize, name: &str) -> Option<[f32; 4]> {
    scan.material(g).prop(name).map(|p| p.v4)
}

/// Perceptual roughness from the AC1 Blinn-Phong exponent and level (ours):
/// alpha = sqrt(2 / (n + 2)), r = sqrt(alpha); a weak specular level pulls
/// towards 0.9 (stock track roughness is mostly 0.6..0.95).
pub fn roughness_of(spec: f32, exp: f32) -> f32 {
    let n = exp.max(1.0);
    let r = (2.0 / (n + 2.0)).sqrt().sqrt();
    let w = (spec / 0.3).clamp(0.0, 1.0);
    (0.9 + (r - 0.9) * w).clamp(0.3, 0.95)
}

/// The global texture in a slot, if it is a real image.
fn tex(scan: &Scan, g: usize, slot: &str) -> Option<usize> {
    scan.material_texture(g, slot)
}

fn multilayer(scan: &Scan, g: usize) -> Option<Multi> {
    let m = scan.material(g);
    let sh = m.shader.to_ascii_lowercase();
    if !sh.starts_with("ksmultilayer") {
        return None;
    }
    let world = !sh.contains("objsp");
    let mults = [prop(scan, g, "multR").unwrap_or(0.0), prop(scan, g, "multG").unwrap_or(0.0), prop(scan, g, "multB").unwrap_or(0.0),
        m.prop("multA").map(|p| if p.value != 0.0 { p.value } else { p.v2[0] }).unwrap_or(0.0)];
    let slots = ["txDetailR", "txDetailG", "txDetailB", "txDetailA"];
    let dets: Vec<Option<usize>> = slots.iter().map(|s| tex(scan, g, s)).collect();
    // channel weight = how much of the mask it covers (unknown here: the
    // mask is decoded later); order by a cheap proxy: channels with a real
    // detail and a non-zero mult first, R G B A order otherwise
    let mut chans: Vec<usize> = (0..4).collect();
    chans.sort_by_key(|&c| (!(dets[c].is_some() && mults[c] > 0.0 && world), c));
    let mut layers = Vec::new();
    let mut baked = Vec::new();
    for c in chans {
        match dets[c] {
            Some(t) if mults[c] > 0.0 && world && layers.len() < 3 => layers.push((c, t, mults[c])),
            d => baked.push((c, d)),
        }
    }
    Some(Multi { diffuse: tex(scan, g, "txDiffuse"), mask: tex(scan, g, "txMask"), layers, baked, magic: gamma_factor(prop(scan, g, "magicMult").unwrap_or(1.0)), world, uv: false, remainder: false })
}

/// The ksPerPixelMultiMap* detail (fxc of the AC1 shader): with useDetail,
/// albedo = lerp(D, D x txDetail(uv0 x detailUVMultiplier), 1 - D.alpha) -
/// a detail layer weighted by the diffuse's inverse alpha (mask channel 4
/// here). `layered`: as a uv-mapped layer; else its mean is baked.
fn detail_multi(scan: &Scan, g: usize, layered: bool) -> Option<Multi> {
    let m = scan.material(g);
    if !m.shader.to_ascii_lowercase().contains("multimap") || prop(scan, g, "useDetail").unwrap_or(0.0) < 0.5 || m.alpha_tested || m.blend_mode != 0 {
        return None;
    }
    let det = tex(scan, g, "txDetail")?;
    let d = tex(scan, g, "txDiffuse")?;
    let mult = prop(scan, g, "detailUVMultiplier").unwrap_or(0.0);
    if mult <= 0.0 {
        return None;
    }
    let (layers, baked) = if layered { (vec![(4, det, mult)], vec![]) } else { (vec![], vec![(4, Some(det))]) };
    Some(Multi { diffuse: Some(d), mask: Some(d), layers, baked, magic: 1.0, world: false, uv: true, remainder: true })
}

/// CSP condition analysis: (static at night, flashing frequency).
struct Conditions {
    by_name: HashMap<String, (bool, f32)>,
}

impl Conditions {
    fn new(csp: Option<&Ini>) -> Conditions {
        let mut by_name = HashMap::new();
        if let Some(c) = csp {
            for (_, s) in c.with_prefix("CONDITION") {
                let Some(name) = s.get("NAME") else { continue };
                let input = s.get("INPUT").map(|v| v.to_ascii_uppercase()).unwrap_or_default();
                let freq = s.get("FLASHING_FREQUENCY").and_then(|v| ini::num_of(v)).unwrap_or(0.0) as f32;
                let night = input.contains("SUN") || input.contains("NIGHT") || input.is_empty();
                by_name.insert(name.to_ascii_uppercase(), (night, freq));
            }
        }
        Conditions { by_name }
    }
    /// (converted as night / always on, blink Hz); None = skip (seasons,
    /// weather, gameplay inputs we cannot follow)
    fn of(&self, cond: Option<&str>) -> Option<(bool, f32)> {
        let Some(c) = cond.map(|c| c.trim().to_ascii_uppercase()).filter(|c| !c.is_empty()) else { return Some((true, 0.0)) };
        if let Some(&(night, f)) = self.by_name.get(&c) {
            // a flicker with noise only (spectators, windows) is static
            return night.then_some((true, if f >= 0.05 { f } else { 0.0 }));
        }
        // the CSP common conditions by name
        if c.contains("SEASON") || c.contains("RAIN") || c.contains("WET") || c.contains("FLAG") {
            return None;
        }
        if c.contains("FLASH") || c.contains("BLINK") || c.contains("HAZARD") || c.contains("AMBULANCE") || c.contains("DISCO") {
            return Some((true, 1.0));
        }
        if c.contains("NIGHT") || c.contains("SUN") || c.contains("DARK") || c.contains("LIGHT") || c.contains("EVENING") {
            return Some((true, 0.0));
        }
        None
    }
}

/// CSP colour "r, g, b[, mult]" (0..255 or 0..1) -> linear colour, brightness.
pub fn csp_colour(v: &str) -> Option<([f32; 3], f32)> {
    let n = ini::nums_of(v);
    if n.len() < 3 {
        return None;
    }
    let scale = if n[..3].iter().any(|x| *x > 1.0) { 255.0 } else { 1.0 };
    let c = [n[0] / scale, n[1] / scale, n[2] / scale].map(|x| x.max(0.0) as f32);
    let mult = n.get(3).copied().unwrap_or(1.0).max(0.0) as f32;
    let peak = c.iter().fold(0f32, |a, b| a.max(*b));
    if peak <= 0.0 {
        return Some(([0.0; 3], 0.0));
    }
    Some(([c[0] / peak, c[1] / peak, c[2] / peak], peak * mult))
}

/// AC1 emissive brightness (CSP / ksEmissive units) -> EVO emissive
/// intensity (ours, calibrated on stock: lamp glow `lights.material` 0.5,
/// signs 1.0, a dim window 0.01..0.05; CSP lamp glow is ~0.5, windows
/// 0.003..0.01): x2, clamped 0.005..4.
pub fn emissive_intensity(csp: f32) -> f32 {
    (csp * 2.0).clamp(0.005, 4.0)
}

/// CSP mesh / material name patterns: comma separated, `?` and `*` wildcards,
/// case-insensitive.
pub fn name_match(pattern: &str, name: &str) -> bool {
    fn m(p: &[u8], n: &[u8]) -> bool {
        match (p.first(), n.first()) {
            (None, None) => true,
            (Some(b'*'), _) => m(&p[1..], n) || (!n.is_empty() && m(p, &n[1..])),
            (Some(b'?'), Some(_)) => m(&p[1..], &n[1..]),
            (Some(a), Some(b)) if a.eq_ignore_ascii_case(b) => m(&p[1..], &n[1..]),
            _ => false,
        }
    }
    pattern.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).any(|p| {
        let p = p.trim_start_matches('{').trim_end_matches('}');
        m(p.as_bytes(), name.as_bytes())
    })
}

/// The scan mesh's node name and its ancestors' names (CSP and the AC1
/// semaphore name dummies as well as meshes).
pub fn names_of(scan: &Scan, mi: usize) -> Vec<String> {
    let m = &scan.meshes[mi];
    let nodes = &scan.models[m.model].nodes;
    let mut out = Vec::new();
    let mut n = Some(m.node);
    while let Some(i) = n {
        out.push(nodes[i].name.clone());
        n = nodes[i].parent;
    }
    out
}

pub fn assign(scan: &Scan) -> Looks {
    let mut notes = Vec::new();
    let class = material_classes(scan);
    let nm = scan.materials.len();
    // ---- drawn materials; the AC1 reference light of each (look.rs head)
    let mut drawn = vec![false; nm];
    for m in &scan.meshes {
        if !matches!(m.role, Role::Physics { drawn: false, .. }) {
            drawn[m.material] = true;
        }
    }
    // a material without both variables (not a ks shader) takes the
    // median light of the track's materials that have them
    let lit: Vec<Option<f32>> = (0..nm).map(|g| Some(rendered_light(prop(scan, g, "ksAmbient")?, prop(scan, g, "ksDiffuse")?))).collect();
    let mut known: Vec<f32> = lit.iter().flatten().copied().collect();
    known.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let typical = known.get(known.len() / 2).copied().unwrap_or_else(|| rendered_light(1.0, 1.0));
    let gain: Vec<f32> = lit.iter().map(|l| l.unwrap_or(typical)).collect();
    let roughness: Vec<f32> = (0..nm).map(|g| roughness_of(prop(scan, g, "ksSpecular").unwrap_or(0.1), prop(scan, g, "ksSpecularEXP").unwrap_or(10.0))).collect();
    // ---- shader family
    let shade: Vec<Shade> = (0..nm).map(|g| {
        let m = scan.material(g);
        let ground = class[g].tarmac || class[g].grassy || class[g].drivable;
        let ml = multilayer(scan, g).or_else(|| detail_multi(scan, g, ground));
        if class[g].tarmac && m.blend_mode == 0 && !m.alpha_tested {
            return Shade::Road(ml);
        }
        if let Some(ml) = ml {
            if m.blend_mode == 0 && !m.alpha_tested {
                // no layer left (object-space multilayer, a detail on decor):
                // the bake alone, on a DefaultMaterial
                return if ml.layers.is_empty() { Shade::Baked(ml) } else { Shade::Layered(ml) };
            }
        }
        if m.blend_mode != 0 {
            return if m.depth_mode != 0 { Shade::Decal } else { Shade::Blend };
        }
        if m.alpha_tested {
            return Shade::AlphaTest;
        }
        Shade::Plain
    }).collect();
    // ---- variants: CSP night emissive / blink, AC1 constant ksEmissive, semaphore
    let mut variants = vec![Variant::default()];
    let mut of_mesh = vec![0u16; scan.meshes.len()];
    let mut index: HashMap<String, u16> = HashMap::new();
    let mut add = |v: Variant, variants: &mut Vec<Variant>| -> u16 {
        let key = format!("{:?}", (v.emissive, v.blink, v.start_light));
        *index.entry(key).or_insert_with(|| {
            variants.push(v);
            (variants.len() - 1) as u16
        })
    };
    let names: Vec<Vec<String>> = (0..scan.meshes.len()).map(|mi| names_of(scan, mi)).collect();
    // AC1 constant emissive (ksEmissive in the kn5 itself: always lit)
    for (mi, m) in scan.meshes.iter().enumerate() {
        if let Some(e) = prop4(scan, m.material, "ksEmissive").map(|v| [v[0], v[1], v[2]]).or_else(|| prop(scan, m.material, "ksEmissive").map(|v| [v; 3])) {
            let peak = e.iter().fold(0f32, |a, b| a.max(*b));
            if peak > 0.001 && drawn[m.material] {
                let v = Variant { emissive: Some(([e[0] / peak, e[1] / peak, e[2] / peak], emissive_intensity(peak))), why: "kn5 ksEmissive".into(), ..Default::default() };
                of_mesh[mi] = add(v, &mut variants);
            }
        }
    }
    let csp = scan.csp.as_ref().map(|c| &c.1);
    let conds = Conditions::new(csp);
    let (mut emissive_n, mut blink_n, mut skipped) = (0usize, 0usize, Vec::new());
    if let Some(c) = csp {
        for (sec, s) in c.with_prefix("MATERIAL_ADJUSTMENT") {
            if s.get("ACTIVE").map(|v| v.trim() == "0").unwrap_or(false) {
                continue;
            }
            // only the emissive key: ksEmissive under KEY_n / VALUE_n
            let mut emis = None;
            for k in 0..8 {
                if s.get(&format!("KEY_{k}")).map(|v| v.trim().eq_ignore_ascii_case("ksEmissive")).unwrap_or(false) {
                    emis = s.get(&format!("VALUE_{k}")).and_then(|x| csp_colour(x));
                }
            }
            let Some((col, lvl)) = emis else { continue };
            if lvl <= 0.0 {
                continue;
            }
            let Some((_, hz)) = conds.of(s.get("CONDITION").map(|x| x.as_str())) else {
                skipped.push(format!("{sec} (condition {})", s.get("CONDITION").map(|x| x.as_str()).unwrap_or("")));
                continue;
            };
            let meshes = s.get("MESHES").map(|x| x.as_str()).unwrap_or("");
            let mats = s.get("MATERIALS").map(|x| x.as_str()).unwrap_or("");
            let v = if hz > 0.0 {
                Variant { blink: Some((hz, col)), emissive: Some((col, emissive_intensity(lvl))), why: sec.to_string(), ..Default::default() }
            } else {
                Variant { emissive: Some((col, emissive_intensity(lvl))), why: sec.to_string(), ..Default::default() }
            };
            let mut hit = 0;
            for (mi, m) in scan.meshes.iter().enumerate() {
                if !drawn[m.material] || matches!(m.role, Role::Physics { drawn: false, .. }) {
                    continue;
                }
                let by_mesh = !meshes.is_empty() && names[mi].iter().any(|n| name_match(meshes, n));
                let by_mat = !mats.is_empty() && name_match(mats, &scan.material(m.material).name);
                if by_mesh || by_mat {
                    let id = add(v.clone(), &mut variants);
                    of_mesh[mi] = id;
                    hit += 1;
                }
            }
            if hit > 0 {
                if hz > 0.0 { blink_n += 1 } else { emissive_n += 1 }
            }
        }
        for (sec, s) in c.with_prefix("SCRIPTABLE_DISPLAY") {
            skipped.push(format!("{sec} (video / scripted display: not converted)"));
            let _ = s;
        }
    }
    // AC1 semaphore: data\semaphore.ini objects of any layout, else KS_START_LIGHT* / AC_SEMAPHORE* names
    let mut sem: Vec<(String, u8)> = Vec::new();
    for l in &scan.layouts {
        if let Some(ini) = l.data.get("semaphore.ini") {
            for (_, s) in ini.with_prefix("OBJECT") {
                if let (Some(n), Some(t)) = (s.get("NAME"), s.get("TYPE").and_then(|t| ini::num_of(t))) {
                    sem.push((n.trim().to_string(), t as u8));
                }
            }
        }
    }
    let mut start_n = 0;
    for (mi, m) in scan.meshes.iter().enumerate() {
        if matches!(m.role, Role::Physics { .. }) {
            continue;
        }
        let ty = names[mi].iter().find_map(|n| {
            sem.iter().find(|(s, _)| s.eq_ignore_ascii_case(n)).map(|x| x.1).or_else(|| {
                let u = n.to_ascii_uppercase();
                (u.starts_with("KS_START_LIGHT") || u.starts_with("AC_START_LIGHT")).then_some(1).or_else(|| {
                    u.starts_with("AC_SEMAPHORE").then(|| if u.contains("GREEN") { 2 } else { 1 })
                })
            })
        });
        if let Some(t) = ty {
            of_mesh[mi] = add(Variant { start_light: Some(t.min(2)), why: "semaphore".into(), ..Default::default() }, &mut variants);
            start_n += 1;
        }
    }
    if emissive_n + blink_n > 0 {
        notes.push(format!("CSP: {emissive_n} night emissive and {blink_n} blinking material adjustment(s) converted"));
    }
    if start_n > 0 {
        notes.push(format!("start lights: {start_n} mesh(es) tagged starting_lights"));
    }
    if !skipped.is_empty() {
        notes.push(format!("CSP skipped: {}", skipped.iter().take(8).cloned().collect::<Vec<_>>().join(", ")));
    }
    Looks { shade, gain, roughness, class, variants, of_mesh, notes }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert!(name_match("zzzlight?", "zzzlight1") && !name_match("zzzlight?", "zzzlight12"));
        assert!(name_match("a, b*", "BOX") && name_match("{x}", "x"));
        let (c, l) = csp_colour("255, 192, 128, 0.045").unwrap();
        assert!((c[0] - 1.0).abs() < 1e-6 && (l - 0.045).abs() < 1e-6);
        assert!(roughness_of(0.0, 10.0) > 0.85 && roughness_of(0.5, 200.0) < 0.4);
        assert!((emissive_intensity(0.5) - 1.0).abs() < 1e-6);
    }
}
