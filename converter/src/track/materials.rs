//! Track materials: every AC1 material a drawn mesh uses becomes a copy of
//! a stock template of the right shader, patched in place (look.rs decides
//! which; textures.rs makes the images).
//!
//! Templates (read from the game's content.kspkg; a built-in copy of the
//! fields we set when it cannot be read):
//!
//! | look | shader | template |
//! |---|---|---|
//! | opaque, alpha-tested, blended, decal, emissive | DefaultMaterial | `common_assets\meshes\props\road_signs\common\materials\276.material` (Basecolor + Modulation_BaseColor, Emissive*, cullMode) |
//! | AC1 multilayer | DynamicTrackMultilayer | `brands_hatch\materials\curb-grass.material` (Base / Red / Green / Blue, Splatmap_*, Detail_*) |
//! | tarmac (road, pit) | DynamicTrack | `common_assets\materials\asphalt_dynamic_new.material` (the game's road shader: rubber, wetness) |
//! | flashing CSP lamp | Particle | `common_assets\meshes\props\airplanes\small_plane\navigation_lights.material` (Flipbook_*) |
//! | AC1 semaphore lamp | Semaphore | `common_assets\materials\starting_lights.material` |
//!
//! Rules (requirement track-textures-materials):
//! - only variables the template already has are set; every slot is
//!   cleared first, then ours are filled, so nothing points at another
//!   track's files;
//! - the template's parent / override bookkeeping (f20..f22) and render
//!   state are dropped; ours is set from the AC1 material: MaterialData
//!   f2 blendMode (1 alpha to coverage for AC1 alpha-tested, as stock
//!   `brands_hatch\grass-brd` / `treeline`; 2 alpha blend), f6 cullMode
//!   (2 = none, with the `cullMode` variable the shader flips back-face
//!   normals by, both as stock two-sided materials carry them), f9 / f10
//!   alpha-to-coverage scale / cutoff (1.2 / 0.1, the most common stock
//!   pair), f18 renderOrder -5 for decals (stock decals -2..-20);
//! - two-sided clones (`<name>_2s`) for merged back-to-back sheets and for
//!   alpha-tested cards; variants `<name>_v<n>` for CSP emissive / blink and
//!   start lights.

use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::look::{Looks, Shade, Variant};
use super::scan::Scan;
use super::textures::{Job, Kind, MultiBake, Src, Textures};
use crate::io::material::{get_prop, set_prop, set_slot};
use crate::io::pb::{Msg, Val};

pub const T_DEFAULT: &str = "content\\tracks\\common_assets\\meshes\\props\\road_signs\\common\\materials\\276.material";
pub const T_MULTI: &str = "content\\tracks\\brands_hatch\\materials\\curb-grass.material";
pub const T_ROAD: &str = "content\\tracks\\common_assets\\materials\\asphalt_dynamic_new.material";
pub const T_PARTICLE: &str = "content\\tracks\\common_assets\\meshes\\props\\airplanes\\small_plane\\navigation_lights.material";
pub const T_SEMAPHORE: &str = "content\\tracks\\common_assets\\materials\\starting_lights.material";

pub struct Templates {
    pub default: Msg,
    pub multi: Msg,
    pub road: Msg,
    pub particle: Msg,
    pub semaphore: Msg,
    /// templates that came from the built-in fallback
    pub builtin: Vec<&'static str>,
}

fn builtin(shader: &str, vars: &[&str]) -> Msg {
    let mut m = Msg::default();
    m.set_str(1, shader);
    for v in vars {
        set_prop(&mut m, v, &[0.0]);
    }
    m
}

const LAYER_VARS: [&str; 14] = ["Enable", "HasBaseColorMap", "Basecolor", "Triplanar", "WorldAlign", "TriplanarScale", "UVscale", "HasNormalMap", "HasRoughnessMap",
    "HasAmbientOcclusionMap", "Roughness", "AmbientOcclusion", "Reflectance", "Metalness"];

fn builtin_layered(shader: &str) -> Msg {
    let mut vars: Vec<String> = ["Base_HasBaseColorMap", "Base_Basecolor", "Modulation_BaseBaseColor", "Base_UVscale", "Base_Roughness", "Base_HasNormalMap",
        "Splatmap_UseConstant", "Splatmap_Constant", "Splatmap_MultiplyBaseColor", "Splatmap_DisableRestrictBlendToTwoLayers", "Detail_Enable", "Detail_BaseColor",
        "Detail_BaseColorMultiplier", "Vertex_UseVertexAmbientOcclusion", "HasBaseColorMap"].iter().map(|s| s.to_string()).collect();
    for l in ["Red", "Green", "Blue"] {
        for v in LAYER_VARS {
            vars.push(format!("{l}_{v}"));
        }
        vars.push(format!("Modulation_{l}BaseColor"));
    }
    let mut m = builtin(shader, &vars.iter().map(|s| s.as_str()).collect::<Vec<_>>());
    for s in ["txDiffuse", "Base_BaseColorMap", "Base_NormalMap", "SpatMap", "Red_BaseColorMap", "Green_BaseColorMap", "Blue_BaseColorMap"] {
        set_slot(&mut m, s, "");
    }
    m
}

impl Templates {
    pub fn load(pkg: Option<&crate::io::kspkg::Pkg>) -> Templates {
        let mut builtin_used = Vec::new();
        let mut read = |path: &'static str, shader: &str| -> Option<Msg> {
            let m = pkg.and_then(|p| p.find(path).and_then(|e| p.read(e).ok())).and_then(|b| Msg::parse(&b).ok()).filter(|m| m.str(1).as_deref() == Some(shader));
            if m.is_none() {
                builtin_used.push(path);
            }
            m
        };
        let default = read(T_DEFAULT, "DefaultMaterial").unwrap_or_else(|| {
            let mut m = builtin("DefaultMaterial", &["HasBaseColorMap", "Basecolor", "Modulation_BaseColor", "HasNormalMap", "NormalScale", "HasRoughnessMap", "Roughness",
                "Modulation_Roughness", "Metalness", "HasMetalnessMap", "Reflectance", "AmbientOcclusion", "HasAmbientOcclusionMap", "HasEmissiveMap", "EmissiveColor",
                "EmissiveIntensity", "Modulation_Emissive", "cullMode", "UVscale"]);
            for s in ["txDiffuse", "NormalMap", "RoughnessMap", "MetalnessMap", "AmbientOcclusionMap", "EmissiveMap"] {
                set_slot(&mut m, s, "");
            }
            m
        });
        let multi = read(T_MULTI, "DynamicTrackMultilayer").unwrap_or_else(|| builtin_layered("DynamicTrackMultilayer"));
        let road = read(T_ROAD, "DynamicTrack").unwrap_or_else(|| builtin_layered("DynamicTrack"));
        let particle = read(T_PARTICLE, "Particle").unwrap_or_else(|| {
            let mut m = builtin("Particle", &["HasBaseColorMap", "Flipbook_Enable", "Flipbook_Columns", "Flipbook_Rows", "Flipbook_Duration", "Emissive_Intensity", "Emissive_UseBaseColorAsEmissive"]);
            set_slot(&mut m, "txDiffuse", "");
            m
        });
        let semaphore = read(T_SEMAPHORE, "Semaphore").unwrap_or_else(|| {
            let mut m = builtin("Semaphore", &["HasBaseColorMap", "HasEmissiveMap"]);
            for s in ["txDiffuse", "EmissiveMap", "Base_EmissiveMap", "Base_BaseColorMap"] {
                set_slot(&mut m, s, "");
            }
            m.set_varint(2, 2);
            m
        });
        Templates { default, multi, road, particle, semaphore, builtin: builtin_used }
    }
}

/// A file-system-safe, lower-case name.
pub fn safe(s: &str) -> String {
    let mut o: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c.to_ascii_lowercase() } else { '_' }).collect();
    o.truncate(80);
    if o.is_empty() { "x".into() } else { o }
}

/// Is the AC1 material drawn with transparency (alpha test or blend)?
pub fn transparent(scan: &Scan, g: usize) -> bool {
    let m = scan.material(g);
    m.alpha_tested || m.blend_mode != 0
}

/// One material written.
#[derive(Clone, Debug)]
pub struct Written {
    /// archive path of the .material
    pub path: String,
    pub two_sided: bool,
    pub shader: String,
}

pub struct Materials {
    /// (global AC1 material, two-sided, variant) -> written material
    pub by_key: HashMap<(usize, bool, u16), Written>,
    pub textures: usize,
    pub texture_mb: f64,
    pub notes: Vec<String>,
    /// materials per EVO shader
    pub shaders: HashMap<String, usize>,
    /// (material stem, shader, class, estimated mean linear albedo)
    pub albedo: Vec<(String, String, &'static str, [f32; 3])>,
}

fn has(m: &Msg, name: &str) -> bool {
    get_prop(m, name, 1).is_some()
}

/// Set a variable only if the template has it.
fn var(m: &mut Msg, name: &str, v: &[f32]) {
    if has(m, name) {
        set_prop(m, name, v);
    }
}

/// Every texture slot name of a material.
fn slots(m: &Msg) -> Vec<String> {
    m.fields.iter().filter(|f| f.num == 5).filter_map(|f| match &f.val {
        Val::Bytes(b) => Msg::parse(b).ok().and_then(|s| s.str(1)),
        Val::Msg(s) => s.str(1),
        _ => None,
    }).collect()
}

fn slot(m: &mut Msg, name: &str, path: &str) {
    if slots(m).iter().any(|s| s == name) {
        set_slot(m, name, path);
    }
}

/// The template with every slot cleared and its parent / render state dropped.
fn base_of(tpl: &Msg) -> Msg {
    let mut m = Msg::default();
    for f in &tpl.fields {
        if matches!(f.num, 1 | 4 | 5) {
            m.fields.push(f.clone());
        }
    }
    for s in slots(&m) {
        set_slot(&mut m, &s, "");
    }
    m
}

/// Planned slot textures of one material (paths; filled after encoding).
#[derive(Default, Clone)]
struct Planned {
    albedo: Option<String>,
    normal: Option<String>,
    rough: Option<String>,
    emissive: Option<String>,
    splat: Option<String>,
    layers: Vec<(usize, String, f32)>,
    detail_mult: f32,
    /// mask channel means of the layers (for the albedo estimate)
    layer_weight: Vec<f32>,
    /// layers mapped by uv0 x mult instead of world xz
    uv_layers: bool,
}

fn layer_name(i: usize) -> &'static str {
    ["Red", "Green", "Blue"][i]
}

/// AC1 normal map of a material, when its shader reads one.
fn normal_tex(scan: &Scan, g: usize) -> Option<usize> {
    let sh = scan.material(g).shader.to_ascii_lowercase();
    (sh.contains("nm") || sh.contains("multimap")).then(|| scan.material_texture(g, "txNormal")).flatten()
}

/// Plan the textures of one material.
fn plan_textures(scan: &Scan, id: &str, looks: &Looks, tx: &mut Textures, g: usize, v: &Variant, lift: f32) -> Planned {
    let mat = scan.material(g);
    let gain = looks.gain[g] * lift;
    let alpha = super::materials::transparent(scan, g);
    let mut p = Planned { detail_mult: 1.0, ..Default::default() };
    let diffuse = scan.material_texture(g, "txDiffuse");
    let name = |t: usize| scan.textures[t].name.clone();
    if v.blink.is_some() {
        if let Some(t) = diffuse {
            p.albedo = Some(tx.want(scan, id, Job { src: Src::Flipbook { tex: t }, kind: Kind::Flipbook, gain: 1.0, name: name(t) }));
        }
        return p;
    }
    let shade = if v.emissive.is_some() || v.start_light.is_some() { &Shade::Plain } else { &looks.shade[g] };
    let layered = matches!(shade, Shade::Layered(_) | Shade::Road(Some(_)));
    let multi = match shade {
        Shade::Layered(m) | Shade::Road(Some(m)) | Shade::Baked(m) => Some(m),
        _ => None,
    };
    match multi {
        Some(ml) => {
            // the detail means: kept layers' averages, baked channels' colour
            // (a detail with mult 0 samples its first texel in AC1). Every
            // layer is written normalised to a mean of LAYER_MEAN (stock
            // layer maps are made to be multiplied: Detail_BaseColorMultiplier
            // 2.5 / 3 on most stock DynamicTrack materials), the Detail
            // multiplier is 1 / LAYER_MEAN, and the brightness - txDiffuse x
            // magicMult x the layers' level - goes into the Base map
            let mults = ["multR", "multG", "multB", "multA"].map(|k| mat.prop(k).map(|p| if p.value != 0.0 { p.value } else { p.v2[0] }).unwrap_or(0.0));
            let lum = |c: &[f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
            let kept: Vec<(usize, [f32; 3], f32, f32)> = ml.layers.iter().map(|&(c, t, _)| {
                let m = tx.mean(scan, t, false);
                let k = (LAYER_MEAN / lum(&m).max(1e-3)).min(LAYER_GAIN_MAX);
                (c, m, (lum(&m) * k).min(1.0), k)
            }).collect();
            let baked: Vec<(usize, [f32; 3])> = ml.baked.iter().map(|&(c, t)| (c, t.map(|t| tx.mean(scan, t, mults.get(c).copied().unwrap_or(1.0) == 0.0)).unwrap_or([0.0; 3]))).collect();
            let mm = ml.mask.map(|t| tx.mask_mean(scan, t)).unwrap_or([1.0, 0.0, 0.0, 0.0]);
            let bake = MultiBake { diffuse: ml.diffuse, mask: ml.mask, kept: kept.iter().map(|x| (x.0, x.1, x.2)).collect(), baked, magic: ml.magic,
                detail_mult: 1.0 / LAYER_MEAN, remainder: ml.remainder };
            let nm = ml.diffuse.map(name).unwrap_or_else(|| format!("{}_base", scan.materials[g].name));
            p.albedo = Some(tx.want(scan, id, Job { src: Src::Multi(Box::new(bake)), kind: Kind::Albedo { alpha: false }, gain, name: format!("{nm}_ml") }));
            if !ml.layers.is_empty() {
                if let Some(mask) = ml.mask {
                    let chans: Vec<usize> = ml.layers.iter().map(|x| x.0).collect();
                    p.splat = Some(tx.want(scan, id, Job { src: Src::Splat { mask, chans }, kind: Kind::Splat, gain: 1.0, name: name(mask) }));
                }
                for (i, (&(c, t, mult), kx)) in ml.layers.iter().zip(&kept).enumerate() {
                    p.layers.push((i, tx.want(scan, id, Job { src: Src::Ac1(t), kind: Kind::Layer, gain: kx.3, name: name(t) }), mult));
                    p.layer_weight.push(if c == 4 { 1.0 - mm[3] } else { mm[c] });
                }
                p.uv_layers = ml.uv;
                p.detail_mult = 1.0 / LAYER_MEAN;
            }
        }
        None => {
            if let Some(t) = diffuse {
                p.albedo = Some(tx.want(scan, id, Job { src: Src::Ac1(t), kind: Kind::Albedo { alpha }, gain, name: name(t) }));
            }
        }
    }
    if let Some(t) = normal_tex(scan, g).filter(|_| !layered) {
        p.normal = Some(tx.want(scan, id, Job { src: Src::Ac1(t), kind: Kind::Normal, gain: 1.0, name: name(t) }));
    }
    if !layered && mat.shader.to_ascii_lowercase().contains("multimap") {
        if let Some(t) = scan.material_texture(g, "txMaps") {
            let spec = mat.prop("ksSpecular").map(|p| p.value).unwrap_or(0.1);
            let exp = mat.prop("ksSpecularEXP").map(|p| p.value).unwrap_or(10.0);
            p.rough = Some(tx.want(scan, id, Job { src: Src::Rough { maps: t, spec, exp }, kind: Kind::Rough, gain: 1.0, name: name(t) }));
        }
    }
    if v.emissive.is_some() {
        // an AC1 emissive map when the shader has one, else the base colour
        p.emissive = match scan.material_texture(g, "txEmissive") {
            Some(t) => Some(tx.want(scan, id, Job { src: Src::Ac1(t), kind: Kind::Albedo { alpha: false }, gain: 1.0, name: name(t) })),
            None => p.albedo.clone(),
        };
    }
    p
}

fn set_render_state(m: &mut Msg, shade: &Shade, two: bool) {
    let two = two || matches!(shade, Shade::AlphaTest);
    match shade {
        Shade::AlphaTest => {
            m.set_varint(2, 1);
            m.set_f32(9, 1.2);
            m.set_f32(10, 0.1);
        }
        Shade::Blend => m.set_varint(2, 2),
        Shade::Decal => {
            m.set_varint(2, 2);
            m.set_varint(18, (-5i64) as u64);
        }
        _ => {}
    }
    if two {
        m.set_varint(6, 2);
        var(m, "cullMode", &[2.0]);
    }
    var(m, "blendMode", &[m.varint(2).unwrap_or(0) as f32]);
}

fn ok(tx: &Textures, p: &Option<String>) -> Option<String> {
    p.as_ref().filter(|p| tx.ok(p)).cloned()
}

fn default_material(tpl: &Msg, tx: &Textures, p: &Planned, rough: f32, v: &Variant) -> Msg {
    let mut m = base_of(tpl);
    let albedo = ok(tx, &p.albedo);
    match &albedo {
        Some(a) => {
            slot(&mut m, "txDiffuse", a);
            var(&mut m, "HasBaseColorMap", &[1.0]);
            var(&mut m, "Basecolor", &[1.0, 1.0, 1.0, 1.0]);
            var(&mut m, "Modulation_BaseColor", &[0.0]);
        }
        None => {
            var(&mut m, "HasBaseColorMap", &[0.0]);
            var(&mut m, "Basecolor", &[0.5, 0.5, 0.5, 1.0]);
        }
    }
    let normal = ok(tx, &p.normal);
    if let Some(n) = &normal {
        slot(&mut m, "NormalMap", n);
    }
    var(&mut m, "HasNormalMap", &[normal.is_some() as u8 as f32]);
    var(&mut m, "NormalScale", &[1.0]);
    let rmap = ok(tx, &p.rough);
    if let Some(r) = &rmap {
        slot(&mut m, "RoughnessMap", r);
        var(&mut m, "Modulation_Roughness", &[0.0]);
    }
    var(&mut m, "HasRoughnessMap", &[rmap.is_some() as u8 as f32]);
    var(&mut m, "Roughness", &[rough]);
    for (k, x) in [("Metalness", 0.0), ("HasMetalnessMap", 0.0), ("HasAmbientOcclusionMap", 0.0), ("AmbientOcclusion", 1.0), ("Reflectance", 0.5),
        ("HasHeightMap", 0.0), ("HasReflectanceMap", 0.0), ("HasAnisotropyMap", 0.0), ("HasGlossMap", 0.0), ("LightWrap", 0.0)] {
        var(&mut m, k, &[x]);
    }
    let emis = ok(tx, &p.emissive);
    match (v.emissive, &emis) {
        (Some((col, int)), Some(e)) => {
            slot(&mut m, "EmissiveMap", e);
            var(&mut m, "HasEmissiveMap", &[1.0]);
            var(&mut m, "Modulation_Emissive", &[1.0]);
            var(&mut m, "EmissiveColor", &[col[0], col[1], col[2], 1.0]);
            var(&mut m, "EmissiveIntensity", &[int]);
        }
        _ => {
            var(&mut m, "HasEmissiveMap", &[0.0]);
            var(&mut m, "EmissiveIntensity", &[0.0]);
        }
    }
    m
}

fn layered_material(tpl: &Msg, tx: &Textures, p: &Planned, rough: f32) -> Msg {
    let mut m = base_of(tpl);
    let albedo = ok(tx, &p.albedo);
    if let Some(a) = &albedo {
        slot(&mut m, "Base_BaseColorMap", a);
        slot(&mut m, "txDiffuse", a);
    }
    let on = albedo.is_some() as u8 as f32;
    for (k, x) in [("Base_HasBaseColorMap", on), ("HasBaseColorMap", on), ("Modulation_BaseBaseColor", 1.0), ("Base_UVchannel", 0.0), ("Base_Triplanar", 0.0),
        ("Base_WorldAlign", 0.0), ("Base_RotateUV", 0.0), ("Base_UVcropU", 0.0), ("Base_UVcropV", 0.0), ("Base_HasRoughnessMap", 0.0), ("Base_Roughness", rough),
        ("Base_HasAmbientOcclusionMap", 0.0), ("Base_AmbientOcclusion", 1.0), ("Base_HasMetalnessMap", 0.0), ("Base_Metalness", 0.0), ("Base_HasEmissiveMap", 0.0),
        ("Base_HasHeightMap", 0.0), ("Base_HasOpacityMap", 0.0), ("Base_HasMap", 0.0), ("Base_ModulateBaseColor", 0.0), ("Base_ModulateRoughness", 0.0),
        ("Base_Reflectance", 0.5), ("Base_NormalScale", 1.0), ("Base_HasNormalMap", 0.0), ("Base_CustomNormalUV", 0.0), ("Base_CustomPbrUV", 0.0),
        ("HasNormalMap", 0.0), ("HasRoughnessMap", 0.0), ("HasAmbientOcclusionMap", 0.0), ("HasMetalnessMap", 0.0), ("HasGlossMap", 0.0), ("HasHeightMap", 0.0),
        ("HasReflectanceMap", 0.0), ("HasEmissiveMap", 0.0), ("Roughness", rough)] {
        var(&mut m, k, &[x]);
    }
    var(&mut m, "Base_Basecolor", &[1.0, 1.0, 1.0, 1.0]);
    var(&mut m, "Base_UVscale", &[1.0, 1.0]);
    var(&mut m, "Base_UVpan", &[0.0, 0.0]);
    // layers
    let splat = ok(tx, &p.splat);
    let mut any = false;
    for i in 0..3 {
        let l = layer_name(i);
        let layer = p.layers.iter().find(|x| x.0 == i).and_then(|x| tx.ok(&x.1).then(|| (x.1.clone(), x.2)));
        let on = layer.is_some() && splat.is_some();
        any |= on;
        if let (true, Some((path, mult))) = (on, &layer) {
            slot(&mut m, &format!("{l}_BaseColorMap"), path);
            if p.uv_layers {
                var(&mut m, &format!("{l}_UVscale"), &[*mult, *mult]);
            } else {
                let s = (mult - 0.01).max(0.001);
                var(&mut m, &format!("{l}_TriplanarScale"), &[s, s, s]);
            }
        }
        let f = on as u8 as f32;
        let planar = if p.uv_layers { 0.0 } else { f };
        for (k, x) in [("Enable", f), ("HasBaseColorMap", f), ("Triplanar", planar), ("WorldAlign", planar), ("UVchannel", 0.0), ("RotateUV", 0.0), ("UVcropU", 0.0), ("UVcropV", 0.0),
            ("HasNormalMap", 0.0), ("HasRoughnessMap", 0.0), ("HasAmbientOcclusionMap", 0.0), ("HasMetalnessMap", 0.0), ("HasHeightMap", 0.0), ("HasOpacityMap", 0.0),
            ("HasDetailMaskMap", 0.0), ("HasEmissiveMap", 0.0), ("Roughness", rough), ("AmbientOcclusion", 1.0), ("Reflectance", 0.5), ("Metalness", 0.0),
            ("NormalScale", 1.0), ("BlendContrast", 0.0), ("HeightPow", 0.0)] {
            var(&mut m, &format!("{l}_{k}"), &[x]);
        }
        var(&mut m, &format!("{l}_Basecolor"), &[1.0, 1.0, 1.0, 1.0]);
        if !(on && p.uv_layers) {
            var(&mut m, &format!("{l}_UVscale"), &[1.0, 1.0]);
        }
        var(&mut m, &format!("Modulation_{l}BaseColor"), &[1.0]);
        for k in ["AmbientOcclusion", "BaseColor", "Height", "LightWrap", "Metalness", "MicroFiber", "MicroShadows", "Normal", "Reflectance", "Roughness"] {
            var(&mut m, &format!("{l}Ignore_{k}"), &[0.0]);
        }
    }
    if let (true, Some(s)) = (any, &splat) {
        slot(&mut m, "SpatMap", s);
    }
    let constant = !any;
    for (k, x) in [("Splatmap_UseConstant", constant as u8 as f32), ("Splatmap_UseVertexInsteadOfTexture", 0.0), ("Splatmap_AddVertexColor", 0.0),
        ("Splatmap_MultiplyVertexColor", 0.0), ("Splatmap_UVchannel", 0.0), ("Splatmap_RotateUV", 0.0), ("Splatmap_UVcropU", 0.0), ("Splatmap_UVcropV", 0.0),
        ("Splatmap_MultiplyBaseColor", 0.0), ("Splatmap_MultiplyRoughness", 0.0), ("Splatmap_MultiplyAmbientOcclusion", 0.0), ("Splatmap_UseMultiply", 0.0),
        ("Splatmap_UseHeightBlend", 0.0), ("Splatmap_DisableRestrictBlendToTwoLayers", 1.0)] {
        var(&mut m, k, &[x]);
    }
    var(&mut m, "Splatmap_Constant", &[0.0, 0.0, 0.0, 0.0]);
    var(&mut m, "Splatmap_UVscale", &[1.0, 1.0]);
    var(&mut m, "Splatmap_UVpan", &[0.0, 0.0]);
    var(&mut m, "Splatmap_VertexColorScale", &[0.0, 0.0, 0.0, 0.0]);
    // the Detail step multiplies the layers by Base x magicMult (look.rs)
    for (k, x) in [("Detail_Enable", any as u8 as f32), ("Detail_BaseColor", any as u8 as f32), ("Detail_BaseColorMultiplier", p.detail_mult), ("Detail_UseDetailMasks", 0.0),
        ("Detail_Normal", 0.0), ("Detail_AmbientOcclusion", 0.0), ("Detail_Roughness", 0.0), ("Detail_Metalness", 0.0), ("Detail_Emissive", 0.0), ("Detail_HasMap", 0.0),
        ("Detail_HasNormalMap", 0.0), ("Detail_ModulateBaseColor", 0.0), ("Detail_ModulateRoughness", 0.0), ("Detail_ModulateAmbientOcclusion", 0.0),
        ("Variation_Enable", 0.0), ("Imperfections_Enable", 0.0), ("InteriorMapping_Enable", 0.0),
        // vertex AO from the colour stream's fourth component (ao / 255,
        // track::ao; neutral 1/255 when not baked): saturate(255 x c.w).
        // The road / kerb templates have no Vertex_UseVertexVariation.
        ("Vertex_UseVertexAmbientOcclusion", 1.0), ("Vertex_AmbientOcclusionChannel", 3.0), ("Vertex_AmbientOcclusionIntensity", 255.0),
        ("Vertex_AmbientOcclusionOffset", 0.0), ("Vertex_UseVertexVariation", 0.0), ("Vertex_UseVertexEmissiveMask", 0.0)] {
        var(&mut m, k, &[x]);
    }
    m
}

fn particle_material(tpl: &Msg, tx: &Textures, p: &Planned, v: &Variant) -> Msg {
    let mut m = base_of(tpl);
    let (hz, _) = v.blink.unwrap_or((1.0, [1.0; 3]));
    if let Some(a) = ok(tx, &p.albedo) {
        slot(&mut m, "txDiffuse", &a);
        var(&mut m, "HasBaseColorMap", &[1.0]);
    }
    var(&mut m, "Flipbook_Enable", &[1.0]);
    var(&mut m, "Flipbook_Columns", &[2.0]);
    var(&mut m, "Flipbook_Rows", &[1.0]);
    var(&mut m, "Flipbook_FrameBlending", &[0.0]);
    var(&mut m, "Flipbook_Duration", &[(1.0 / hz.max(0.05)).clamp(0.1, 20.0)]);
    var(&mut m, "Emissive_UseBaseColorAsEmissive", &[1.0]);
    var(&mut m, "Emissive_Intensity", &[v.emissive.map(|e| e.1).unwrap_or(1.0)]);
    m
}

fn semaphore_material(tpl: &Msg, tx: &Textures, p: &Planned) -> Msg {
    // the stock lamp keeps its render state (blend, order) and LED colours
    let mut m = tpl.clone();
    m.fields.retain(|f| !matches!(f.num, 20 | 21 | 22));
    for s in slots(&m) {
        set_slot(&mut m, &s, "");
    }
    if let Some(a) = ok(tx, &p.albedo) {
        for s in ["txDiffuse", "Base_BaseColorMap", "EmissiveMap", "Base_EmissiveMap"] {
            slot(&mut m, s, &a);
        }
    }
    m
}

/// A material's class for the brightness report: road, grass, kerb, tree,
/// building / wall, other (ours: physics surface first, then names).
pub fn class_of(scan: &Scan, looks: &Looks, g: usize) -> &'static str {
    let c = &looks.class[g];
    if c.tarmac {
        return "road";
    }
    if c.grassy {
        return "grass";
    }
    if matches!(c.kind, Some(super::classes::Kind::Kerb | super::classes::Kind::FlatKerb)) {
        return "kerb";
    }
    let mut text = scan.materials[g].name.to_ascii_lowercase();
    for (_, t) in &scan.material(g).textures {
        text.push(' ');
        text += &t.to_ascii_lowercase();
    }
    let any = |w: &[&str]| w.iter().any(|x| text.contains(x));
    if any(&["tree", "leaf", "leaves", "bush", "branch", "pine", "foliage", "forest"]) {
        "tree"
    } else if any(&["grass", "erba", "lawn", "field", "terrain", "ground"]) {
        "grass"
    } else if any(&["build", "wall", "house", "stand", "pit", "concrete", "brick", "roof", "tower", "garage", "box"]) {
        "building"
    } else {
        "other"
    }
}

fn shade_used(looks: &Looks, g: usize, v: &Variant) -> Shade {
    if v.emissive.is_some() || v.start_light.is_some() || v.blink.is_some() { Shade::Plain } else { looks.shade[g].clone() }
}

/// Mean linear albedo the material shows (ours, from the written textures
/// and the layered shader's maths in look.rs; 0.5 grey without a map).
fn estimate(tx: &Textures, p: &Planned, shade: &Shade) -> [f32; 3] {
    let base = p.albedo.as_ref().and_then(|a| tx.written_mean.get(a)).copied().unwrap_or([0.5; 3]);
    match shade {
        Shade::Layered(_) | Shade::Road(Some(_)) if !p.layers.is_empty() => {
            let w: f32 = p.layer_weight.iter().sum();
            let mut lay = [0f32; 3];
            for ((_, path, _), wt) in p.layers.iter().zip(&p.layer_weight) {
                let m = tx.written_mean.get(path).copied().unwrap_or([0.5; 3]);
                for c in 0..3 {
                    lay[c] += m[c] * wt / w.max(1.0);
                }
            }
            let open = (1.0 - w).max(0.0);
            [0, 1, 2].map(|c| base[c] * (lay[c] * p.detail_mult * (1.0 - open) + open))
        }
        _ => base,
    }
}

/// Mean linear luma every layer map is normalised to (ours; its inverse,
/// 2.5, is the Detail multiplier stock DynamicTrack materials use most).
pub const LAYER_MEAN: f32 = 0.4;
/// A dark layer is brightened at most this much (the rest stays in the base).
pub const LAYER_GAIN_MAX: f32 = 8.0;

/// Stock per-class albedo, `aceconv albedo <content.kspkg>` over the 17
/// stock circuits (road_atlanta left out; stockstats/measure_albedo.txt):
/// (class, p25, p75 of the tracks' class medians, p05, p95 of all the
/// class's stock materials pooled). The track's class median is scaled into
/// p25..p75 (not for tree: its stock p25..p75 is a single value, from 1..4
/// materials on 8 tracks, so trees keep RENDER_TO_ALBEDO); a single
/// material never leaves p05..p95.
pub const STOCK_CLASS: [(&str, f32, f32, f32, f32); 6] = [
    ("road", 0.114, 0.183, 0.039, 0.510),
    ("grass", 0.161, 0.216, 0.083, 0.429),
    ("kerb", 0.206, 0.347, 0.079, 0.419),
    ("building", 0.254, 0.283, 0.097, 0.624),
    ("tree", 0.216, 0.216, 0.034, 0.321),
    ("other", 0.216, 0.237, 0.053, 0.564),
];
/// AC1 rendered linear value -> EVO albedo, before the class fit (ours,
/// calibrated on the calibration pair ks_brands_hatch -> brands_hatch: the
/// geometric mean, over the classes both have (building, grass, kerb,
/// other, road), of the stock class median over the converted rendered
/// median; tools/brightness_k.py, stockstats/measure_brightness_k.txt).
/// Tracks keep their AC1 differences inside the stock interquartile range.
pub const RENDER_TO_ALBEDO: f32 = 1.079;

/// Build every material message from planned textures: (message, render
/// shade, file stem), in `keys` order.
fn build_all(scan: &Scan, looks: &Looks, tpl: &Templates, tx: &Textures, keys: &[(usize, bool, u16)], planned: &[Planned]) -> Vec<(Msg, Shade, String)> {
    let mut names: HashSet<String> = HashSet::new();
    let mut out = Vec::with_capacity(keys.len());
    for (&(g, two, vi), p) in keys.iter().zip(planned) {
        let v = &looks.variants[vi as usize];
        let rough = looks.roughness[g];
        let (mut m, shade) = if v.blink.is_some() && ok(tx, &p.albedo).is_some() {
            (particle_material(&tpl.particle, tx, p, v), Shade::Plain)
        } else if v.start_light.is_some() {
            (semaphore_material(&tpl.semaphore, tx, p), Shade::Plain)
        } else if v.emissive.is_some() {
            let sh = match &looks.shade[g] {
                Shade::Layered(_) | Shade::Road(_) | Shade::Baked(_) => Shade::Plain,
                s => s.clone(),
            };
            (default_material(&tpl.default, tx, p, rough, v), sh)
        } else {
            match &looks.shade[g] {
                Shade::Layered(_) => (layered_material(&tpl.multi, tx, p, rough), looks.shade[g].clone()),
                Shade::Road(_) => (layered_material(&tpl.road, tx, p, rough), looks.shade[g].clone()),
                s => (default_material(&tpl.default, tx, p, rough, v), s.clone()),
            }
        };
        if v.start_light.is_none() {
            set_render_state(&mut m, &shade, two);
        } else if two {
            m.set_varint(6, 2);
            var(&mut m, "cullMode", &[2.0]);
        }
        let mut stem = safe(&scan.materials[g].name);
        if two {
            stem += "_2s";
        }
        if vi != 0 {
            stem += &format!("_v{vi}");
        }
        while !names.insert(stem.clone()) {
            stem.push('_');
        }
        // keep the stock field order: f1 shader, f2 blend, then the rest
        m.fields.sort_by_key(|f| match f.num { 1 => 0, 2 => 1, 4 => 3, 5 => 4, _ => 2 });
        out.push((m, shade, stem));
    }
    out
}

/// What `aceconv albedo` would measure for one written material: (class,
/// luma), with its own rule and maths (stats/albedo.rs), from the probe
/// means of the planned textures (Textures::probe_means); None for what it
/// leaves out.
fn measured(m: &Msg, stem: &str, means: &HashMap<String, [f32; 4]>) -> Option<(&'static str, f32)> {
    use crate::stats::albedo;
    let shader = m.str(1).unwrap_or_default();
    if albedo::skipped(stem, &shader, m) {
        return None;
    }
    let tex = |p: &str| means.get(p).copied();
    albedo::material_albedo(m, &tex).map(|a| (albedo::class_of(stem, &shader), albedo::luma(a)))
}

/// One step of the brightness fit (look.rs head, requirement
/// track-textures-materials). `seen` = each material's albedo as `aceconv
/// albedo` measures it with the textures planned so far. Per class, one
/// factor moves the median into the middle half of the stock p25..p75
/// (ours: the half keeps the measurement's rounding inside); then each
/// material stays inside the class's stock p05..p95. Returns the factor of
/// every key and, per class, (median, class factor).
fn fit(seen: &[Option<(&'static str, f32)>]) -> (Vec<f32>, Vec<(&'static str, f32, f32)>) {
    let mut by: HashMap<&'static str, Vec<f32>> = HashMap::new();
    for (c, e) in seen.iter().flatten() {
        if *e > 0.0 {
            by.entry(c).or_default().push(*e);
        }
    }
    let mut scale: HashMap<&'static str, f32> = HashMap::new();
    let mut rep = Vec::new();
    for (class, lo, hi, _, _) in STOCK_CLASS {
        let Some(v) = by.get_mut(class) else { continue };
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let med = v[v.len() / 2];
        // a zero-width stock p25..p75 (tree) is not a range to fit into
        let s = if hi - lo > 1e-3 {
            let q = (hi - lo) / 4.0;
            med.clamp(lo + q, hi - q) / med
        } else {
            1.0
        };
        scale.insert(class, s);
        rep.push((class, med, s));
    }
    let factors = seen.iter().map(|f| {
        let Some((c, e)) = f else { return 1.0 };
        let s = scale.get(c).copied().unwrap_or(1.0);
        let Some(&(_, _, _, lo, hi)) = STOCK_CLASS.iter().find(|x| x.0 == *c) else { return s };
        if *e <= 0.0 {
            return s;
        }
        (e * s).clamp(lo, hi) / e
    }).collect();
    (factors, rep)
}

/// Probe rounds of the brightness fit at most (the gain clips bright
/// texels, so a scale does not move the measured albedo exactly in
/// proportion: the fit is repeated on the re-planned textures until every
/// class factor is within 1 %).
const FIT_ROUNDS: usize = 4;

/// Write the textures and materials the drawn meshes need into `out` (the
/// track folder). `used` = (global material, two-sided, variant).
pub fn write(scan: &Scan, id: &str, out: &Path, used: &HashSet<(usize, bool, u16)>, tpl: &Templates, looks: &Looks) -> Result<Materials> {
    std::fs::create_dir_all(out.join("materials"))?;
    let mut keys: Vec<(usize, bool, u16)> = used.iter().copied().collect();
    keys.sort();
    // ---- brightness: probe at the reference light, fit, re-plan, repeat
    let mut lifts = vec![RENDER_TO_ALBEDO; keys.len()];
    let mut first_report: Option<String> = None;
    let mut rounds = 0;
    let mut last = String::new();
    for _ in 0..FIT_ROUNDS {
        rounds += 1;
        let mut probe = Textures::default();
        let planned: Vec<Planned> = keys.iter().zip(&lifts).map(|(&(g, _, v), &lift)| plan_textures(scan, id, looks, &mut probe, g, &looks.variants[v as usize], lift)).collect();
        let means = probe.probe_means(scan);
        let seen: Vec<Option<(&'static str, f32)>> = build_all(scan, looks, tpl, &probe, &keys, &planned).iter().map(|(m, _, stem)| measured(m, stem, &means)).collect();
        let (factors, rep) = fit(&seen);
        first_report.get_or_insert_with(|| rep.iter().map(|(c, med, s)| format!("{c} rendered median {:.3} x{s:.2}", med / RENDER_TO_ALBEDO)).collect::<Vec<_>>().join(", "));
        // done when every class median sits in its range (a material held
        // at its p05 / p95 bound may keep a small residue from clipping)
        let done = rep.iter().all(|(_, _, s)| (s - 1.0).abs() < 0.01);
        last = rep.iter().map(|(c, med, _)| format!("{c} {med:.3}")).collect::<Vec<_>>().join(", ");
        for (l, f) in lifts.iter_mut().zip(&factors) {
            *l *= f;
        }
        if done {
            break;
        }
    }
    let mut tx = Textures::default();
    let planned: Vec<Planned> = keys.iter().zip(&lifts).map(|(&(g, _, v), &lift)| plan_textures(scan, id, looks, &mut tx, g, &looks.variants[v as usize], lift)).collect();
    let fit_report = format!("{}; {rounds} probe round(s), last measured medians {last}", first_report.unwrap_or_default());
    tx.write_all(scan, out)?;
    let mut by_key = HashMap::new();
    let mut shaders: HashMap<String, usize> = HashMap::new();
    let mut albedo = Vec::new();
    for ((&(g, two, vi), p), (m, shade, stem)) in keys.iter().zip(&planned).zip(build_all(scan, looks, tpl, &tx, &keys, &planned)) {
        let v = &looks.variants[vi as usize];
        std::fs::write(out.join("materials").join(format!("{stem}.material")), m.encode())?;
        let shader = m.str(1).unwrap_or_default();
        *shaders.entry(shader.clone()).or_default() += 1;
        let _ = shade;
        albedo.push((stem.clone(), shader.clone(), class_of(scan, looks, g), estimate(&tx, p, &shade_used(looks, g, v))));
        by_key.insert((g, two, vi), Written { path: format!("content\\tracks\\{id}\\materials\\{stem}.material"), two_sided: two, shader });
    }
    let mut notes = tx.notes.clone();
    notes.push(format!("brightness: AC1 reference light, class fit to stock p25..p75: {fit_report}"));
    if !tpl.builtin.is_empty() {
        notes.push(format!("templates not found in content.kspkg (built-in stand-ins used): {}", tpl.builtin.join(", ")));
    }
    Ok(Materials { by_key, textures: tx.count, texture_mb: tx.bytes as f64 / 1048576.0, notes, shaders, albedo })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_and_state() {
        let t = Templates::load(None);
        assert_eq!(t.default.str(1).as_deref(), Some("DefaultMaterial"));
        let mut m = base_of(&t.default);
        set_render_state(&mut m, &Shade::AlphaTest, false);
        assert_eq!(m.varint(2), Some(1));
        assert_eq!(m.varint(6), Some(2));
        assert_eq!(get_prop(&m, "cullMode", 1), Some(vec![2.0]));
        // never invent a variable the template lacks
        var(&mut m, "NotAVariable", &[1.0]);
        assert!(!has(&m, "NotAVariable"));
        let mut d = base_of(&t.default);
        set_render_state(&mut d, &Shade::Decal, false);
        assert_eq!(d.varint(18), Some((-5i64) as u64));
    }
}
