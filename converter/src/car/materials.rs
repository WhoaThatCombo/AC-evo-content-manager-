//! AC1 kn5 materials -> EVO `.material` files and textures.
//!
//! # Kinds (which section family a part goes to; ours)
//!
//! From AC1 shader semantics and names: car paint (`ksPerPixelMultiMap_damage_dirt`,
//! or a material whose diffuse an AC1 skin replaces and whose name says
//! paint / skin / body), glass (alpha-blended reflective / `ksWindscreen`
//! glass), lamps (meshes `lights.ini` names with a real colour, outside the
//! cockpit and not a needle / gauge part), mirrors (`mirrors.ini`), brake
//! discs (`ksBrakeDisc`), tyres / rims / rim blur (the wheel stage), emissive
//! cockpit parts (`ksEmissive` set) and everything else ("generic").
//!
//! # PBR values (ours, literature-based)
//!
//! - roughness from the Blinn-Phong exponent n: GGX alpha = sqrt(2 / (n + 2))
//!   (the usual Blinn-Phong <-> Beckmann / GGX correspondence, Walter et al.
//!   2007), perceptual roughness = sqrt(alpha); a weak `ksSpecular` (under
//!   0.5) moves it up to half way towards 1; `txMaps` (multimap) G scales the
//!   exponent and R the specular per texel, baked into a roughness map
//!   (stock: a roughness map replaces the scalar);
//! - metalness from the Fresnel F0 AC1 calls `fresnelC`: dielectrics are
//!   about 0.04, metals 0.5 and up; smoothstep 0.15..0.5, raised for names
//!   that say chrome / metal;
//! - base colour: the diffuse map; (ksDiffuse + ksAmbient) under 0.6 darkens
//!   it proportionally (AC1's way of making a part darker); a missing or
//!   near-black diffuse map gives a neutral 0.18 grey;
//! - alpha-tested / alpha-blended non-glass parts: blendMode 2 with the
//!   diffuse alpha as opacity map, as stock decals do;
//! - everything is clamped into the range stock car materials of the same
//!   shader use (measured over the extracted stock cars: UberVehicleMaterial
//!   Base_Roughness p10..p90 0.15..0.65, Base_Reflectance 0.5, VehicleGlass
//!   Basecolor alpha 0.24..0.72).

use anyhow::{anyhow, Result};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ac1::kn5::{Kn5, Material};
use crate::files::Files;
use crate::io::material::{set_prop, set_slot};
use crate::io::pb::Msg;
use crate::io::texture::{self, Fmt, Image};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Paint,
    Glass,
    Lamp,
    Mirror,
    Emissive,
    Disc,
    Tyre,
    RimBlur,
    Rim,
    Generic,
    /// a shift light / dash LED (car functions stage: VehicleLed + LedMask)
    Led,
    /// a clear lamp lens (glass over a lamp that lights.ini does not name):
    /// stock EXT_LIGHTS_GLASS*, VehicleGlass with the lamp-lens parameters
    LampGlass,
    /// gauge / instrument glass inside the cockpit: stock INT_GLASS
    GaugeGlass,
}

/// The stock batch slots of a rim / rim-blur mesh. The game's car renderer
/// looks these up by exact batch name (EXT_RIM, EXT_RIM_BLUR,
/// EXT_RIM_BLUR_STATIC are dereferenced without a null check; the decals
/// are optional). Every stock rim / blur batch uses VehicleRim: the rim
/// with InverseVelocity (shown at rest, fading out with wheel speed), the
/// blur set with Velocity (hidden at rest, fading in), EXT_RIM_BLUR alpha
/// blended (UseBlend 1, blendMode 1), EXT_RIM_BLUR_STATIC opaque.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RimSlot {
    Rim,
    Decal,
    Blur,
    BlurStatic,
    DecalBlur,
}

impl RimSlot {
    pub fn batch(self) -> &'static str {
        match self {
            RimSlot::Rim => "EXT_RIM",
            RimSlot::Decal => "EXT_RIM_DECAL",
            RimSlot::Blur => "EXT_RIM_BLUR",
            RimSlot::BlurStatic => "EXT_RIM_BLUR_STATIC",
            RimSlot::DecalBlur => "EXT_RIM_DECAL_BLUR",
        }
    }
}

/// Name lists from the AC1 data that decide kinds.
#[derive(Default)]
pub struct Hints {
    /// lower-case mesh names of lights.ini lamps with a real colour
    pub lamps: HashSet<String>,
    /// lower-case mesh names of mirrors.ini
    pub mirrors: HashSet<String>,
    /// lower-case texture names some skin replaces
    pub skin_textures: HashSet<String>,
    /// car functions (lights / LEDs): lower-case node -> section tag, so
    /// parts with different functions get sections of their own
    pub groups: HashMap<String, String>,
    /// lower-case cockpit node names that are LEDs
    pub leds: HashSet<String>,
    /// lower-case cockpit node names lit as dash backlights
    pub backlights: HashSet<String>,
    /// lower-case node names left out of the car (AC1-only effects such as
    /// fake light cones)
    pub drop: HashSet<String>,
    /// lower-case material names taken as the paint when no material is
    /// car paint by the usual rule (see paint_fallback)
    pub paint: HashSet<String>,
}

fn re(p: &str) -> regex::Regex {
    regex::Regex::new(p).unwrap()
}

impl Hints {
    pub fn from_data(lights: &crate::ac1::ini::Ini, mirrors: &crate::ac1::ini::Ini, skin_textures: HashSet<String>) -> Hints {
        let dash = re(r"(?i)(needle|arrow|gauge|dash|cockpit|display|digit|led|rpm|speedo|lcd|interior|int_)");
        let mut lamps = HashSet::new();
        for (sec, kv) in lights.sections.iter() {
            if !(sec.starts_with("LIGHT_") || sec.starts_with("BRAKE_")) {
                continue;
            }
            let Some(name) = kv.get("NAME") else { continue };
            let col = kv.get("COLOR").map(|c| crate::ac1::ini::nums_of(c)).unwrap_or_default();
            let lit = col.iter().any(|v| *v > 0.05);
            if lit && !dash.is_match(name) {
                lamps.insert(name.to_lowercase());
            }
        }
        let mirrors = mirrors.sections.iter().filter(|(s, _)| s.starts_with("MIRROR")).filter_map(|(_, kv)| kv.get("NAME")).map(|n| n.to_lowercase()).collect();
        Hints { lamps, mirrors, skin_textures, ..Default::default() }
    }
}

/// The paint of a car with no car-paint material by the usual rule (a
/// damage_dirt shader, or a skin-replaced texture with a body-like name):
/// mods that bake one livery into the body texture and use a plain shader,
/// e.g. `redbull_paint` on ksPerPixelMultiMap. Without a paint section EVO
/// has nothing to put paint and liveries on and the car fails the check.
/// Takes opaque exterior materials whose NAME says paint / livery / body.
pub fn paint_fallback(mats: &[Material], h: &Hints) -> HashSet<String> {
    let body = re(r"(paint|skin|body|chassis|livery|carpaint|coach)");
    let usual = mats.iter().any(|m| {
        let tex = m.texture("txDiffuse").map(|t| t.to_lowercase()).unwrap_or_default();
        m.shader.to_lowercase().contains("damage_dirt") || (h.skin_textures.contains(&tex) && body.is_match(&m.name.to_lowercase()))
    });
    if usual {
        return HashSet::new();
    }
    let not_body = re(r"(int_|interior|cockpit|glass|window|tyre|tire|rim|wheel|disc|light|lamp|driver|helmet|suit|glove|decal|number|logo)");
    let named = re(r"(paint|livery|carpaint|bodywork|body|coach)");
    mats.iter().filter(|m| {
        let n = m.name.to_lowercase();
        let sh = m.shader.to_lowercase();
        m.blend_mode == 0 && !m.alpha_tested && named.is_match(&n) && !not_body.is_match(&n)
            && !(sh.contains("tyre") || sh.contains("glass") || sh.contains("windscreen"))
    }).map(|m| m.name.to_lowercase()).collect()
}

/// A brake disc by name: disc / disk / rotor in the material or node name,
/// or brake (freno, bremse, frein) in the material name. The node name alone
/// does not count for "brake": AC1 wheels name whole rim nodes RIM_xx_brake.
pub fn disc_like(m: &Material, node: &str) -> bool {
    let name = m.name.to_lowercase();
    let both = format!("{name} {}", node.to_lowercase());
    re(r"(disc|disk|rotor)").is_match(&both) || re(r"(brake|freno|bremse|frein)").is_match(&name)
}

/// The kind of one placed mesh (`inst` facts: node name, wheel / cockpit position).
pub fn kind_of(m: &Material, node: &str, under_wheel: bool, blur: bool, cockpit: bool, h: &Hints) -> Kind {
    let name = m.name.to_lowercase();
    let node_l = node.to_lowercase();
    let sh = m.shader.to_lowercase();
    let both = format!("{name} {node_l}");
    if h.mirrors.contains(&node_l) {
        return Kind::Mirror;
    }
    if under_wheel {
        if sh.contains("tyre") || sh.contains("tire") || re(r"(tyre|tire)").is_match(&both) && !blur {
            return Kind::Tyre;
        }
        if sh.contains("brakedisc") || disc_like(m, node) {
            return Kind::Disc;
        }
        if blur || both.contains("blur") {
            return Kind::RimBlur;
        }
        return Kind::Rim;
    }
    if sh.contains("brakedisc") {
        return Kind::Disc;
    }
    if !cockpit && h.lamps.contains(&node_l) {
        return Kind::Lamp;
    }
    if cockpit && h.leds.contains(&node_l) {
        return Kind::Led;
    }
    let glassy = sh.contains("windscreen") || sh.contains("glass")
        || (m.blend_mode == 1 && (sh.contains("reflection") || sh.contains("perpixel")) && re(r"(glass|window|windscreen|windshield|vetro|cristal|screen)").is_match(&both));
    if glassy && !sh.contains("broken") {
        let gauge = re(r"(dash|gauge|instrument|cluster|speedo|tacho|clock|meter|dial)").is_match(&both);
        let window = !gauge && (re(r"(window|windshield|windscreen|screen|vetro|cristal|scheibe|vitre|finestr)").is_match(&both) || sh.contains("windscreen"));
        // cockpit glass that is not a window: the instrument glass (stock INT_GLASS)
        if cockpit && !window {
            return Kind::GaugeGlass;
        }
        // a clear lens over a lamp (stock EXT_LIGHTS_GLASS*), not a window
        if !cockpit && !window && re(r"(light|lamp|lght|indicator|blinker|brake|head|tail|fog|reverse|lens|signal|turn|beam|fanal|faro|luce|feu|clign|bulb|reflector)").is_match(&both) {
            return Kind::LampGlass;
        }
        return Kind::Glass;
    }
    let tex = m.texture("txDiffuse").map(|t| t.to_lowercase()).unwrap_or_default();
    if !cockpit && (sh.contains("damage_dirt") || h.paint.contains(&name)
        || (h.skin_textures.contains(&tex) && re(r"(paint|skin|body|chassis|livery|carpaint|coach)").is_match(&both))) {
        return Kind::Paint;
    }
    let em = m.prop("ksEmissive").map(|p| p.v3[0].max(p.v3[1]).max(p.v3[2])).unwrap_or(0.0);
    if cockpit && em > 0.05 {
        return Kind::Emissive;
    }
    Kind::Generic
}

// ----------------------------------------------------------- PBR values --

pub fn prop(m: &Material, n: &str) -> Option<f32> {
    m.prop(n).map(|p| p.value)
}

/// Perceptual roughness from a Blinn-Phong exponent and specular strength.
pub fn roughness(exp: f32, spec: f32) -> f32 {
    let n = exp.max(1.0);
    let alpha = (2.0 / (n + 2.0)).sqrt();
    let r = alpha.sqrt();
    let weak = (1.0 - (spec / 0.5).clamp(0.0, 1.0)) * 0.5;
    r + (1.0 - r) * weak
}

fn smooth(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub fn metalness(m: &Material) -> f32 {
    let f0 = prop(m, "fresnelC").unwrap_or(0.04);
    let mut v = smooth(0.15, 0.5, f0);
    let both = m.name.to_lowercase();
    if re(r"(chrome|cromo|polish|mirror)").is_match(&both) {
        v = v.max(0.95);
    } else if re(r"(metal|alu|steel|iron|exhaust|titan)").is_match(&both) {
        v = v.max(0.6);
    }
    v
}

/// Whether AC1 lays the detail map under the diffuse: the multimap /
/// damage shaders do, unless the material switches it off (`useDetail` 0:
/// ks_toyota_ae86's rims carry a dark rim_detailmap.dds they never show).
pub fn uses_detail(m: &Material) -> bool {
    let sh = m.shader.to_lowercase();
    (sh.contains("multimap") || sh.contains("damage")) && prop(m, "useDetail").map(|v| v > 0.5).unwrap_or(true)
}

pub struct Pbr {
    pub rough: f32,
    pub metal: f32,
    pub tint: f32,
}

pub fn pbr(m: &Material, kind: Kind) -> Pbr {
    let exp = prop(m, "ksSpecularEXP").unwrap_or(20.0);
    let spec = prop(m, "ksSpecular").unwrap_or(0.3);
    let lum = prop(m, "ksDiffuse").unwrap_or(0.4) + prop(m, "ksAmbient").unwrap_or(0.3);
    let tint = (lum / 0.6).clamp(0.15, 1.0);
    let (lo, hi) = match kind {
        Kind::Paint => (0.2, 0.55),
        Kind::Disc => (0.3, 0.7),
        _ => (0.15, 0.85),
    };
    Pbr { rough: roughness(exp, spec).clamp(lo, hi), metal: metalness(m), tint }
}

// ------------------------------------------------------------ textures --

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Role {
    Color,
    Normal,
    /// roughness baked from a multimap: exponent and specular x 1000
    Rough(u32, u32),
    Alpha,
    /// a grey mask: the brightest channel, scaled so the mask peaks at 1
    /// (AC1's coloured brake-glow map -> the stock white glow mask)
    Mask,
    /// an AC1 tiled detail map as a layer colour map (true: paint, with the
    /// detail's share of the paint tone curve)
    Detail(bool),
    /// the detail layer's splat weight from the diffuse alpha (1 - a)
    Splat,
}

/// An AC1 tiled detail map laid as the EVO Blue layer (ours; see
/// `Tex::detail_layer`).
pub struct DetailLayer {
    /// the detail as a colour map (textured details) ...
    pub map: Option<String>,
    /// ... else its flat colour (linear); [1; 3] with a map
    pub colour: [f32; 3],
    /// detail normal map and its strength (AC1 detailNormalBlend)
    pub normal: Option<(String, f32)>,
    /// the AC1 detailUVMultiplier: the layer's UV scale
    pub mult: f32,
    /// splat map (B = 1 - diffuse alpha); None: the whole surface (constant)
    pub splat: Option<String>,
}

/// The variance of an image's colour (sRGB 0..1), sampled.
pub fn colour_var(d: &Image) -> f32 {
    let mu = mean(d);
    let n = ((d.w * d.h) as usize / 4096).max(1);
    let mut s = 0f32;
    let mut c = 0f32;
    for p in d.px.chunks_exact(4).step_by(n) {
        s += (0..3).map(|k| (p[k] as f32 / 255.0 - mu[k]).powi(2)).sum::<f32>();
        c += 1.0;
    }
    s / c.max(1.0)
}

/// kn5 / skin texture -> encoded EVO texture, once per source and role.
pub struct Tex<'a> {
    k: &'a Kn5<'a>,
    by_name: HashMap<String, usize>,
    /// content\cars\<id>
    pub base: String,
    done: HashMap<(u64, Role), Option<String>>,
    used_names: HashSet<String>,
    /// mean linear colour of baked albedo textures, by path
    means: HashMap<String, [f32; 3]>,
    /// the same over (1 - diffuse alpha): the share a detail layer scales
    means_under: HashMap<String, [f32; 3]>,
    /// longest side for colour maps / everything else (ours: stock cars top
    /// out at 4096; ACECONV_TEXMAX lowers both)
    pub max_color: u32,
    pub max_other: u32,
    /// the default skin's folder (its textures are the car's own; other
    /// skins' variants are capped at 2048)
    pub default_skin: Option<String>,
    pub count: usize,
    pub bytes: usize,
    pub notes: Vec<String>,
}

pub fn h64(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    // sampled FNV over at most 1 MB + length: fast and enough to tell images apart
    let step = (b.len() / (1 << 20)).max(1);
    for c in b.iter().step_by(step) {
        h = (h ^ *c as u64).wrapping_mul(0x100000001b3);
    }
    h ^ (b.len() as u64).wrapping_mul(0x9e3779b97f4a7c15)
}

pub fn slug(s: &str) -> String {
    let stem = s.rsplit_once('.').map(|(a, _)| a).unwrap_or(s);
    let mut o = String::new();
    for c in stem.chars() {
        if c.is_ascii_alphanumeric() {
            o.push(c.to_ascii_lowercase());
        } else if !o.ends_with('_') {
            o.push('_');
        }
    }
    let o = o.trim_matches('_').to_string();
    if o.is_empty() { "tex".into() } else { o.chars().take(48).collect() }
}

/// The mean of an image (linear 0..1 per channel, alpha last).
pub fn mean(img: &Image) -> [f32; 4] {
    let mut s = [0f64; 4];
    let n = (img.w * img.h).max(1) as f64;
    let step = ((img.w * img.h) as usize / 65536).max(1);
    let mut cnt = 0f64;
    for p in img.px.chunks_exact(4).step_by(step) {
        for k in 0..4 {
            s[k] += p[k] as f64 / 255.0;
        }
        cnt += 1.0;
    }
    let _ = n;
    [(s[0] / cnt) as f32, (s[1] / cnt) as f32, (s[2] / cnt) as f32, (s[3] / cnt) as f32]
}

/// Colour maps as stock cars store them: BC1 sRGB (`_d` maps) when the
/// image is opaque, BC7 sRGB when it has alpha or is a paint livery
/// (measured: stock cars hold 2..35 BC1 sRGB and 10..48 BC7 sRGB textures).
pub fn colour_format(img: &Image, paint: bool) -> Fmt {
    let opaque = img.px.chunks_exact(4).step_by(7).all(|c| c[3] == 255);
    if opaque && !paint { Fmt::Bc1D } else { Fmt::Bc7 }
}

pub fn srgb_lin(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

pub fn lin_srgb(l: f32) -> f32 {
    let l = l.max(0.0);
    if l <= 0.0031308 { l * 12.92 } else { 1.055 * l.powf(1.0 / 2.4) - 0.055 }
}

/// AC1 paint albedo (linear) -> EVO paint albedo (ours): 0.85 x v^0.75,
/// calibrated on the ks_mazda_miata / ks_mazda_mx5_na pair: the six AC1
/// skin colours (classic red, mariner blue, crystal white, brilliant black,
/// sunburst yellow, marina green; diffuse x detail, linear) against the stock
/// EVO colours of the same names (mazda_paint_*.oemmultilayercolor): bright
/// colours land within about 15 %, dark ones are lifted the way Kunos
/// lifted them (black 0.002 -> 0.008, stock 0.013).
pub fn paint_tone(v: f32) -> f32 {
    0.85 * v.max(0.0).powf(0.75)
}

fn is_flat_normal(img: &Image) -> bool {
    let step = ((img.w * img.h) as usize / 65536).max(1);
    let mut dev = 0f64;
    let mut n = 0f64;
    for p in img.px.chunks_exact(4).step_by(step) {
        dev += ((p[0] as f64 - 128.0).abs() + (p[1] as f64 - 128.0).abs()) / 255.0;
        n += 1.0;
    }
    dev / n.max(1.0) < 0.01
}

impl<'a> Tex<'a> {
    pub fn new(k: &'a Kn5<'a>, id: &str) -> Tex<'a> {
        let by_name = k.textures.iter().enumerate().filter(|(_, t)| t.active).map(|(i, t)| (t.name.to_lowercase(), i)).collect();
        let lim = std::env::var("ACECONV_TEXMAX").ok().and_then(|v| v.parse().ok()).unwrap_or(4096u32);
        Tex {
            k, by_name, base: format!("content\\cars\\{id}"), done: HashMap::new(), used_names: HashSet::new(), means: HashMap::new(), means_under: HashMap::new(),
            max_color: lim.min(4096), max_other: lim.min(2048), default_skin: None, count: 0, bytes: 0, notes: Vec::new(),
        }
    }

    /// The source bytes of a texture: the skin's own file when the skin
    /// replaces it, else the kn5's embedded one.
    pub fn source(&self, name: &str, skin: Option<&Path>) -> Option<(Vec<u8>, bool)> {
        if name.is_empty() || name.eq_ignore_ascii_case("null.dds") {
            return None;
        }
        if let Some(dir) = skin {
            if let Some(p) = crate::ac1::car::find_ci(dir, name) {
                if let Ok(b) = std::fs::read(&p) {
                    return Some((b, true));
                }
            }
        }
        let i = *self.by_name.get(&name.to_lowercase())?;
        Some((self.k.textures[i].data.to_vec(), false))
    }

    pub fn image(&self, name: &str, skin: Option<&Path>) -> Option<(Image, bool, u64)> {
        let (b, from_skin) = self.source(name, skin)?;
        let h = h64(&b);
        match texture::decode_any(&b) {
            Ok(img) if img.w > 0 && img.h > 0 => Some((img, from_skin, h)),
            _ => None,
        }
    }

    /// The longest side for a new texture (ours): the caps above; a
    /// non-default skin's variant at most 2048; past 560 MB of mips at most
    /// 1024, past 700 MB at most 512 (stock cars carry 132..553 MB, p90 369:
    /// the budget only stops a car from going past the largest stock one;
    /// at 200 / 320 MB it cut the Pagani's 2048 skins to 1024 / 512 in
    /// conversion order, below both AC1 and stock).
    fn cap(&self, colour: bool, skin: Option<&str>) -> u32 {
        let mut c = if colour { self.max_color } else { self.max_other };
        if let (Some(s), Some(d)) = (skin, &self.default_skin) {
            if !s.eq_ignore_ascii_case(d) {
                c = c.min(2048);
            }
        }
        if self.bytes > 700 << 20 {
            c = c.min(512);
        } else if self.bytes > 560 << 20 {
            c = c.min(1024);
        }
        c
    }

    fn unique_path(&mut self, dir: &str, stem: &str) -> String {
        let mut p = format!("{dir}\\{stem}.texture");
        let mut n = 2;
        while self.used_names.contains(&p.to_lowercase()) {
            p = format!("{dir}\\{stem}_{n}.texture");
            n += 1;
        }
        self.used_names.insert(p.to_lowercase());
        p
    }

    /// `src`: the AC1 texture's relative name (the header's source path).
    fn put(&mut self, files: &mut Files, path: &str, img: &Image, fmt: Fmt, srgb: bool, src: &str) {
        let e = texture::encode(img, fmt, srgb, src);
        self.count += 1;
        self.bytes += e.mips.len();
        files.put(path, e.header);
        files.put(&format!("{path}mips"), e.mips);
    }

    /// The material's AC1 detail map when it goes to a layer
    /// (`detail_layer`) instead of the base map: a multimap detail with
    /// detailUVMultiplier >= 1. Baked, a tiled detail's texels are averaged
    /// away (multipliers of 50..400 put hundreds of tiles across one base
    /// texel: carbon weave, leather grain, alcantara turn flat), and every
    /// detail variant of one diffuse duplicates the whole base map (the
    /// Pagani: six 2048 copies of EXT_Skin per skin, which then hit the
    /// texture budget and came out at 1024 / 512). A multiplier under 1
    /// (0 = one constant texel) stays baked, and so does a flat detail on
    /// paint. Returns (image, hash, multiplier, textured: colour variance >
    /// 0.0002; else a flat colour).
    pub fn tiled_detail(&self, m: &Material, skin: Option<&Path>, paint: bool) -> Option<(Image, u64, f32, bool)> {
        let name = m.texture("txDetail").filter(|_| uses_detail(m))?;
        let mult = prop(m, "detailUVMultiplier").unwrap_or(1.0).abs();
        if mult < 1.0 {
            return None;
        }
        let (d, _, h) = self.image(name, skin)?;
        let textured = colour_var(&d) > 0.0002;
        // a flat detail on paint is the livery colour (Kunos street cars):
        // it stays in the base map, where no paint channel can touch it
        if paint && !textured {
            return None;
        }
        Some((d, h, mult, textured))
    }

    /// The base colour of a material as AC1 shows it: the diffuse map times
    /// the detail map where the diffuse alpha is 0 (AC1 multimap rule:
    /// colour = diffuse x lerp(detail, 1, diffuse alpha); Kunos street cars
    /// keep their paint colour in the detail map: ks_mazda_miata's Skin_00 is
    /// a light grey with alpha 0 and each skin's metal_detail.dds is the
    /// colour). A detail that goes to a layer (`tiled_detail`) is left out
    /// of the map. `paint`: also apply the paint tone curve (see
    /// `paint_tone`). Returns (path, mean linear colour as shown, layer
    /// included).
    pub fn albedo(&mut self, files: &mut Files, m: &Material, skin: Option<(&str, &Path)>, paint: bool) -> Option<(String, [f32; 3])> {
        let dname = m.texture("txDiffuse").unwrap_or("").to_string();
        let (img, from_skin, h) = self.image(&dname, skin.map(|s| s.1))?;
        let detail_name = m.texture("txDetail").filter(|_| uses_detail(m)).unwrap_or("").to_string();
        let layered = self.tiled_detail(m, skin.map(|s| s.1), paint);
        let det = self.image(&detail_name, skin.map(|s| s.1));
        let mult = prop(m, "detailUVMultiplier").unwrap_or(1.0).abs().max(0.01);
        let dh = det.as_ref().map(|d| d.2).unwrap_or(0);
        // a layered detail is not in the base map: one base per diffuse
        let key = if layered.is_some() {
            (h ^ 0x5eed_1a7e ^ (paint as u64) << 63, Role::Color)
        } else {
            (h ^ dh.rotate_left(17) ^ (paint as u64) << 63 ^ (mult.to_bits() as u64) << 7, Role::Color)
        };
        let lut: Vec<f32> = (0..256).map(|i| srgb_lin(i as f32 / 255.0)).collect();
        // the layer's mean (linear, toned for paint): only for the mean colour
        let layer_mean = layered.as_ref().map(|(d, _, _, _)| {
            let mu = mean(d);
            let f = |c: f32| if paint { srgb_lin(c).powf(0.75) } else { srgb_lin(c) };
            [f(mu[0]), f(mu[1]), f(mu[2])]
        });
        // shown mean = mean(v a) + layer mean x mean(v (1 - a))
        let shown = |s: &Self, p: &str| -> [f32; 3] {
            let all = s.means.get(p).copied().unwrap_or([0.5; 3]);
            match (&layer_mean, s.means_under.get(p)) {
                (Some(lm), Some(u)) => [all[0] - u[0] + lm[0] * u[0], all[1] - u[1] + lm[1] * u[1], all[2] - u[2] + lm[2] * u[2]],
                _ => all,
            }
        };
        if let Some(Some(p)) = self.done.get(&key) {
            let p = p.clone();
            let c = shown(self, &p);
            return Some((p, c));
        }
        let img = texture::resize_to(&img, self.cap(true, skin.map(|s| s.0)), true);
        let det_img = if layered.is_some() { None } else { det.map(|d| d.0) };
        let (dmean, tiled) = match &det_img {
            Some(d) => {
                let mu = mean(d);
                ([srgb_lin(mu[0]), srgb_lin(mu[1]), srgb_lin(mu[2])], colour_var(d) > 0.002 && mult <= 16.0)
            }
            None => ([1.0; 3], false),
        };
        let has_detail = det_img.is_some() || layered.is_some();
        let dmean = if layered.is_some() { [1.0; 3] } else { dmean };
        let (w, hh) = (img.w, img.h);
        let mut px = vec![0u8; (w * hh * 4) as usize];
        let mut sum = [0f64; 3];
        let mut under = [0f64; 3];
        for y in 0..hh {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                let c = &img.px[i..i + 4];
                let a = if has_detail { c[3] as f32 / 255.0 } else { 1.0 };
                let d = if tiled {
                    let dd = det_img.as_ref().unwrap();
                    let dx = ((x as f32 / w as f32 * mult * dd.w as f32) as u32) % dd.w;
                    let dy = ((y as f32 / hh as f32 * mult * dd.h as f32) as u32) % dd.h;
                    let j = ((dy * dd.w + dx) * 4) as usize;
                    [lut[dd.px[j] as usize], lut[dd.px[j + 1] as usize], lut[dd.px[j + 2] as usize]]
                } else {
                    dmean
                };
                for k in 0..3 {
                    let mut v = lut[c[k] as usize] * (d[k] + (1.0 - d[k]) * a);
                    if paint {
                        v = paint_tone(v);
                    }
                    sum[k] += v as f64;
                    under[k] += (v * (1.0 - a)) as f64;
                    px[i + k] = (lin_srgb(v) * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
                }
                px[i + 3] = if has_detail { 255 } else { c[3] };
            }
        }
        let n = (w * hh).max(1) as f64;
        let mean_lin = [(sum[0] / n) as f32, (sum[1] / n) as f32, (sum[2] / n) as f32];
        let out = Image { w, h: hh, px };
        let dir = match (from_skin, skin) {
            (true, Some((sname, _))) => format!("{}\\skins\\{}", self.base, slug(sname)),
            _ => match skin {
                // a detail map from the skin also makes this skin-specific
                Some((sname, sp)) if layered.is_none() && crate::ac1::car::find_ci(sp, &detail_name).is_some() => format!("{}\\skins\\{}", self.base, slug(sname)),
                _ => format!("{}\\texture", self.base),
            },
        };
        let p = self.unique_path(&dir, &slug(&dname));
        let fmt = colour_format(&out, paint);
        let src = ac1_name(&dname, from_skin, skin.map(|s| s.0));
        self.put(files, &p, &out, fmt, true, &src);
        self.done.insert(key, Some(p.clone()));
        self.means.insert(p.clone(), mean_lin);
        self.means_under.insert(p.clone(), [(under[0] / n) as f32, (under[1] / n) as f32, (under[2] / n) as f32]);
        let c = shown(self, &p);
        Some((p, c))
    }

    /// The EVO path of texture `name` in `role` (None: missing, flat or black
    /// where that means "no map"). `skin`: the skin folder that may replace it.
    pub fn get(&mut self, files: &mut Files, name: &str, role: Role, skin: Option<(&str, &Path)>) -> Option<String> {
        let (img, from_skin, h) = self.image(name, skin.map(|s| s.1))?;
        let key = (h, role);
        if let Some(p) = self.done.get(&key) {
            return p.clone();
        }
        let dir = match (from_skin, skin) {
            (true, Some((sname, _))) => format!("{}\\skins\\{}", self.base, slug(sname)),
            _ => format!("{}\\texture", self.base),
        };
        let stem = slug(name);
        let src = ac1_name(name, from_skin, skin.map(|s| s.0));
        let out = match role {
            Role::Color => {
                let img = texture::resize_to(&img, self.cap(true, skin.map(|s| s.0)), true);
                let p = self.unique_path(&dir, &stem);
                let fmt = colour_format(&img, false);
                self.put(files, &p, &img, fmt, true, &src);
                Some(p)
            }
            Role::Normal => {
                if is_flat_normal(&img) {
                    None
                } else {
                    let img = texture::resize_to(&img, self.cap(false, skin.map(|s| s.0)), false);
                    let p = self.unique_path(&dir, &format!("{stem}_nm"));
                    self.put(files, &p, &img, Fmt::Bc6h, false, &src);
                    Some(p)
                }
            }
            Role::Alpha => {
                let img = texture::resize_to(&img, self.cap(false, skin.map(|s| s.0)), false);
                let px: Vec<u8> = img.px.chunks_exact(4).flat_map(|c| [c[3], c[3], c[3], 255]).collect();
                let a = Image { w: img.w, h: img.h, px };
                let p = self.unique_path(&dir, &format!("{stem}_at"));
                self.put(files, &p, &a, Fmt::Bc4, false, &src);
                Some(p)
            }
            Role::Mask => {
                let img = texture::resize_to(&img, self.cap(false, skin.map(|s| s.0)), false);
                let peak = img.px.chunks_exact(4).map(|c| c[0].max(c[1]).max(c[2])).max().unwrap_or(0).max(1) as f32;
                let px: Vec<u8> = img.px.chunks_exact(4).flat_map(|c| {
                    let v = (c[0].max(c[1]).max(c[2]) as f32 * 255.0 / peak).round().min(255.0) as u8;
                    [v, v, v, 255]
                }).collect();
                let a = Image { w: img.w, h: img.h, px };
                let p = self.unique_path(&dir, &format!("{stem}_mask"));
                self.put(files, &p, &a, Fmt::Bc4, false, &src);
                Some(p)
            }
            Role::Detail(paint) => {
                // the AC1 detail colours as they multiply the base (linear);
                // paint: its share of the paint tone curve, d^0.75 (0.85 is
                // already in the base: tone(b d) = tone(b) x d^0.75)
                let img = texture::resize_to(&img, self.cap(true, skin.map(|s| s.0)), true);
                let img = if paint {
                    let px = img.px.chunks_exact(4).flat_map(|c| {
                        let f = |v: u8| (lin_srgb(srgb_lin(v as f32 / 255.0).powf(0.75)) * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
                        [f(c[0]), f(c[1]), f(c[2]), 255]
                    }).collect();
                    Image { w: img.w, h: img.h, px }
                } else {
                    let px = img.px.chunks_exact(4).flat_map(|c| [c[0], c[1], c[2], 255]).collect();
                    Image { w: img.w, h: img.h, px }
                };
                let p = self.unique_path(&dir, &format!("{stem}_detail"));
                self.put(files, &p, &img, Fmt::Bc1D, true, &src);
                Some(p)
            }
            Role::Splat => {
                // where the detail shows (1 - diffuse alpha), linear, in every
                // channel: the detail layer reads its own, the template's
                // other layers are off
                let img = texture::resize_to(&img, self.cap(false, skin.map(|s| s.0)), false);
                let px: Vec<u8> = img.px.chunks_exact(4).flat_map(|c| [255 - c[3], 255 - c[3], 255 - c[3], 255]).collect();
                let a = Image { w: img.w, h: img.h, px };
                let p = self.unique_path(&dir, &format!("{stem}_splat"));
                self.put(files, &p, &a, Fmt::Bc1, false, &src);
                Some(p)
            }
            Role::Rough(e, s) => {
                let (exp, spec) = (e as f32 / 1000.0, s as f32 / 1000.0);
                let img = texture::resize_to(&img, self.cap(false, skin.map(|s| s.0)), false);
                // R = specular mask, G = glossiness (exponent multiplier)
                let px: Vec<u8> = img.px.chunks_exact(4).flat_map(|c| {
                    let r = roughness((exp * (c[1] as f32 / 255.0)).max(1.0), spec * (c[0] as f32 / 255.0));
                    let v = (r.clamp(0.0, 1.0) * 255.0).round() as u8;
                    [v, v, v, 255]
                }).collect();
                let rimg = Image { w: img.w, h: img.h, px };
                let p = self.unique_path(&dir, &format!("{stem}_r"));
                self.put(files, &p, &rimg, Fmt::Bc4, false, &src);
                Some(p)
            }
        };
        self.done.insert(key, out.clone());
        out
    }
}

impl<'a> Tex<'a> {
    /// The tiled-detail layer of a multimap material (`tiled_detail`):
    /// AC1 shows `diffuse x lerp(detail(uv x detailUVMultiplier), 1, diffuse
    /// alpha)`; EVO's layer blend `lerp(base, layer x base, w)` with the
    /// layer's colour map at UV scale = the multiplier, multiply mode and
    /// w = 1 - diffuse alpha gives exactly that over the un-detailed base
    /// map (`albedo` leaves the detail out when this applies). The detail
    /// normal map (txNormalDetail, strength detailNormalBlend) rides on the
    /// same layer. None when the detail is baked instead, or the diffuse
    /// alpha hides it everywhere.
    pub fn detail_layer(&mut self, files: &mut Files, m: &Material, skin: Option<(&str, &Path)>, paint: bool) -> Option<DetailLayer> {
        let (dimg, _, mult, textured) = self.tiled_detail(m, skin.map(|s| s.1), paint)?;
        let dname = m.texture("txDiffuse")?.to_string();
        let (img, _, _) = self.image(&dname, skin.map(|s| s.1))?;
        let (mut lo, mut hi) = (255u8, 0u8);
        for c in img.px.chunks_exact(4).step_by(3) {
            lo = lo.min(c[3]);
            hi = hi.max(c[3]);
        }
        if lo > 247 {
            return None;
        }
        // textured: the detail as the layer's colour map; flat: its colour
        // as the layer's constant (linear; paint: d^0.75, see Role::Detail)
        let (map, colour) = if textured {
            (Some(self.get(files, m.texture("txDetail")?, Role::Detail(paint), skin)?), [1.0; 3])
        } else {
            let mu = mean(&dimg);
            let f = |c: f32| if paint { srgb_lin(c).powf(0.75) } else { srgb_lin(c) };
            (None, [f(mu[0]), f(mu[1]), f(mu[2])])
        };
        let splat = if hi < 8 { None } else { Some(self.get(files, &dname, Role::Splat, skin)?) };
        let normal = m.texture("txNormalDetail").and_then(|n| self.get(files, n, Role::Normal, skin))
            .map(|p| (p, prop(m, "detailNormalBlend").unwrap_or(1.0).clamp(0.0, 1.0)));
        Some(DetailLayer { map, colour, normal, mult, splat })
    }
}

/// Lay `d` as the material's Blue layer: every attribute multiplies (the
/// layer's own value 1 leaves it as the base has it) except the colour,
/// which takes the detail map; normals combine. Blue: the layer no paint
/// channel drives (the converter's designs customise the primary channel
/// only, which recolours Red).
pub fn apply_detail(s: &mut Msg, d: &DetailLayer) {
    // ACECONV_DETAIL_LAYER=Red: a dev switch for previews in viewers that
    // draw only the Red layer's map (Red is the primary paint channel; do
    // not ship it)
    let l = std::env::var("ACECONV_DETAIL_LAYER").ok().filter(|v| v == "Red" || v == "Green").unwrap_or_else(|| "Blue".into());
    let l = l.as_str();
    let k = |n: &str| format!("{l}{n}");
    setp(s, &k("_Enable"), &[1.0]);
    // colour multiplies; every other attribute has neither flag, which
    // leaves it as the base has it (stock liveries rely on that: a layer
    // with neither flag does not touch the attribute)
    const ATTRS: [&str; 13] = ["BaseColor", "Roughness", "Metalness", "Reflectance", "AmbientOcclusion", "ClearCoat", "ClearCoatRoughness", "MicroShadows", "LightWrap", "Anisotropy", "MicroFiber", "Height", "Emissive"];
    for a in ATTRS {
        setp(s, &k(&format!("OverrideReplace_{a}")), &[0.0]);
        setp(s, &k(&format!("OverrideMultiply_{a}")), &[(a == "BaseColor") as u8 as f32]);
    }
    for a in ["Roughness", "Metalness", "Reflectance", "AmbientOcclusion", "ClearCoat", "ClearCoatRoughness", "MicroShadows", "LightWrap"] {
        setp(s, &k(&format!("_{a}")), &[1.0]);
    }
    setp(s, &k("_Basecolor"), &[d.colour[0], d.colour[1], d.colour[2], 1.0]);
    for f in ["Metalness", "Roughness", "AmbientOcclusion", "Opacity", "ClearCoat", "ClearCoatNormal", "ClearCoatRoughness", "Anisotropy", "Emissive", "DetailMask"] {
        setp(s, &k(&format!("_Has{f}Map")), &[0.0]);
    }
    setp(s, &k("_HasBaseColorMap"), &[d.map.is_some() as u8 as f32]);
    set_slot(s, &k("_BaseColorMap"), d.map.as_deref().unwrap_or(""));
    for uv in ["_UVscale", "_UVscaleNormal", "_UVscalePbr"] {
        setp(s, &k(uv), &[d.mult, d.mult]);
    }
    setp(s, &k("OverrideCombine_Normal"), &[1.0]);
    setp(s, &k("OverrideCombine_ClearCoatNormal"), &[0.0]);
    match &d.normal {
        Some((p, n)) => {
            set_slot(s, &k("_NormalMap"), p);
            setp(s, &k("_HasNormalMap"), &[1.0]);
            setp(s, &k("_NormalScale"), &[*n]);
        }
        None => setp(s, &k("_HasNormalMap"), &[0.0]),
    }
    let w = match l { "Red" => [1.0, 0.0, 0.0, 0.0], "Green" => [0.0, 1.0, 0.0, 0.0], _ => [0.0, 0.0, 1.0, 0.0] };
    match &d.splat {
        None => {
            setp(s, "Splatmap_UseConstant", &[1.0]);
            setp(s, "Splatmap_Constant", &w);
        }
        Some(p) => {
            set_slot(s, "SpatMap", p);
            setp(s, "Splatmap_UseConstant", &[0.0]);
            setp(s, "Splatmap_UVscale", &[1.0, 1.0]);
            setp(s, "Splatmap_Tighten", &[0.0]);
            setp(s, "Splatmap_UseVertexInsteadOfTexture", &[0.0]);
            setp(s, "Splatmap_AddVertexColor", &[0.0]);
        }
    }
}

// ------------------------------------------------------------ templates --

/// Where the templates come from: the generated `refmats` folder and the
/// cloned donor (already renamed to the mod id).
pub struct Templates {
    pub simple: Msg,
    /// `simple` on UberVehicleInteriorMaterial: every stock interior batch
    /// that is not glass / LED / mirror uses that shader, and the game
    /// throws "Interiors are missing car renderer cache" without it
    pub simple_int: Msg,
    pub emissive: Msg,
    pub glass_ext: Msg,
    pub glass_int: Msg,
    /// interior windscreen (VehicleGlassInterior, IsWindshield 1)
    pub windshield_int: Msg,
    pub lamp_glass: Msg,
    pub lamp: Msg,
    /// VehicleDisc (brake glow)
    pub disc: Msg,
    /// VehicleRim per slot, bared to one plain base layer
    pub rim: HashMap<RimSlot, Msg>,
}

fn load(p: &Path) -> Result<Msg> {
    let b = std::fs::read(p).map_err(|e| anyhow!("{}: {e}", p.display()))?;
    Msg::parse(&b)
}

fn donor_mat(files: &Files, id: &str, name: &str) -> Result<Msg> {
    let p = format!("content\\cars\\{id}\\materials\\{name}");
    let b = files.get(&p).ok_or_else(|| anyhow!("donor has no {p}"))?;
    Msg::parse(b)
}

impl Templates {
    pub fn load(refmats: &Path, files: &Files, id: &str) -> Result<Templates> {
        let mut lamp_glass = donor_mat(files, id, "EXT_LIGHTS_GLASS.material")?;
        let mut lamp = donor_mat(files, id, "EXT_LIGHTS_CHROME.material")?;
        // the donor's function masks / maps belong to the Abarth's lamps
        for m in [&mut lamp_glass, &mut lamp] {
            let names: Vec<String> = m.subs(5).iter().filter_map(|s| s.str(1)).collect();
            for n in names {
                if n.starts_with("Rain_") {
                    continue;
                }
                set_slot(m, &n, "");
            }
        }
        let plain = |name: &str| -> Result<Msg> {
            let p = format!("content\\cars\\{id}\\materials\\{name}");
            crate::donor::plain(files.get(&p).ok_or_else(|| anyhow!("donor has no {p}"))?)
        };
        let mut rim = HashMap::new();
        for (slot, f) in [(RimSlot::Rim, "EXT_RIM"), (RimSlot::Decal, "EXT_RIM_DECAL"), (RimSlot::Blur, "EXT_RIM_BLUR"), (RimSlot::BlurStatic, "EXT_RIM_BLUR_STATIC"), (RimSlot::DecalBlur, "EXT_RIM_DECAL_BLUR")] {
            rim.insert(slot, plain(&format!("{f}.material"))?);
        }
        // the Abarth's own windscreen dirt / refraction masks follow its UVs
        let mut windshield_int = donor_mat(files, id, "INT_WINDSHIELD.material")?;
        for n in ["DRS_CustomizedMap", "Rain_RefractionMask"] {
            set_slot(&mut windshield_int, n, "");
        }
        Ok(Templates {
            simple: load(&refmats.join("simple_refmat.material"))?,
            simple_int: plain("INT_ELEC_BLANKING.material")?,
            emissive: load(&refmats.join("emi_refmat.material"))?,
            glass_ext: donor_mat(files, id, "EXT_WINDOWS.material")?,
            glass_int: donor_mat(files, id, "INT_WINDOWS.material")?,
            windshield_int,
            lamp_glass,
            lamp,
            disc: donor_mat(files, id, "EXT_DISC.material")?,
            rim,
        })
    }
}

/// What one section's material is made from.
pub struct MatIn<'m> {
    pub kind: Kind,
    pub m: &'m Material,
    pub interior: bool,
    /// the skin (name, folder) whose textures replace the kn5's
    pub skin: Option<(&'m str, PathBuf)>,
    /// paint: metallic finish (from the skin name)
    pub metallic: bool,
}

fn has(m: &Msg, name: &str) -> bool {
    crate::io::material::get_prop(m, name, 1).is_some()
}

fn setp(m: &mut Msg, name: &str, v: &[f32]) {
    // only parameters the template's shader has (stock materials of the
    // same shader are the whitelist)
    if has(m, name) {
        set_prop(m, name, v);
    }
}

/// The `.material` bytes of one section.
pub fn build(files: &mut Files, tex: &mut Tex, t: &Templates, inp: &MatIn) -> Msg {
    let m = inp.m;
    let skin = inp.skin.as_ref().map(|(n, p)| (*n, p.as_path()));
    let diffuse = m.texture("txDiffuse").unwrap_or("").to_string();
    match inp.kind {
        Kind::Glass => glass(tex, if inp.interior { &t.glass_int } else { &t.glass_ext }, m, &diffuse, skin),
        Kind::LampGlass => {
            // the stock lens (clear, refractive); the AC1 lens texture only
            // where it carries colour (black.dds lenses are reflection only)
            // VehicleGlass reads its colour from txDiffuse (HasBaseColorMap);
            // its Base_* slots hold the editor defaults on every stock lens
            let mut l = t.lamp_glass.clone();
            setp(&mut l, "HasBaseColorMap", &[0.0]);
            if let Some((img, _, _)) = tex.image(&diffuse, skin.map(|s| s.1)) {
                let mu = mean(&img);
                if mu[0].max(mu[1]).max(mu[2]) > 0.08 {
                    if let Some(p) = tex.get(files, &diffuse, Role::Color, skin) {
                        set_slot(&mut l, "txDiffuse", &p);
                        setp(&mut l, "HasBaseColorMap", &[1.0]);
                    }
                }
            }
            for n in ["Base_HasBaseColorMap", "Base_HasNormalMap", "HasNormalMap", "HasAmbientOcclusionMap", "Base_HasAmbientOcclusionMap", "Base_HasRoughnessMap", "HasOpacityMap", "ksHasNormalMap", "ksHasRoughnessMap"] {
                setp(&mut l, n, &[0.0]);
            }
            l
        }
        Kind::Lamp => {
            let glass = m.blend_mode == 1;
            let mut l = if glass { t.lamp_glass.clone() } else { t.lamp.clone() };
            if let Some(p) = tex.get(files, &diffuse, Role::Color, skin) {
                if glass {
                    // VehicleGlass: txDiffuse + HasBaseColorMap (see LampGlass)
                    set_slot(&mut l, "txDiffuse", &p);
                    setp(&mut l, "HasBaseColorMap", &[1.0]);
                } else {
                    set_slot(&mut l, "Base_BaseColorMap", &p);
                    setp(&mut l, "Base_HasBaseColorMap", &[1.0]);
                }
            }
            setp(&mut l, "Base_HasNormalMap", &[0.0]);
            setp(&mut l, "HasAmbientOcclusionMap", &[0.0]);
            setp(&mut l, "Base_HasAmbientOcclusionMap", &[0.0]);
            setp(&mut l, "Base_HasRoughnessMap", &[0.0]);
            setp(&mut l, "ksHasNormalMap", &[0.0]);
            setp(&mut l, "ksHasRoughnessMap", &[0.0]);
            if glass {
                // the template's slots are cleared: no flag may point at one
                for n in ["HasNormalMap", "HasOpacityMap", "HasRoughnessMap"] {
                    setp(&mut l, n, &[0.0]);
                }
            }
            l
        }
        Kind::GaugeGlass => glass(tex, &t.glass_int, m, &diffuse, skin),
        Kind::Emissive => {
            // emi_refmat: int_elec_blanking, already UberVehicleInteriorMaterial
            let mut e = t.emissive.clone();
            if let Some(p) = tex.get(files, &diffuse, Role::Color, skin) {
                set_slot(&mut e, "Base_BaseColorMap", &p);
            }
            let em = m.prop("ksEmissive").map(|p| p.v3[0].max(p.v3[1]).max(p.v3[2])).unwrap_or(1.0);
            setp(&mut e, "Base_EmissiveIntensity", &[em.clamp(0.2, 4.0)]);
            e
        }
        Kind::Disc => {
            // the stock disc shader (brake glow through its emissive map);
            // the donor's disc-specific maps are dropped for the AC1 ones
            let mut d = t.disc.clone();
            if let Some((p, _)) = tex.albedo(files, m, skin, false) {
                set_slot(&mut d, "Base_BaseColorMap", &p);
                setp(&mut d, "Base_HasBaseColorMap", &[1.0]);
            }
            for n in ["Base_HasMetalnessMap", "Base_HasRoughnessMap", "Base_HasAmbientOcclusionMap", "Base_HasNormalMap", "Base_HasClearCoatNormalMap", "Red_Enable", "Scuff_Enable"] {
                setp(&mut d, n, &[0.0]);
            }
            for n in ["SpatMap", "Base_MetalnessMap", "Base_RoughnessMap", "Base_AmbientOcclusionMap", "Base_NormalMap", "Base_ClearCoatNormalMap", "Scuff_IntensityMap"] {
                set_slot(&mut d, n, "");
            }
            // the stock glow map is a white mask (the shader tints it with the
            // brake temperature colour); AC1's txGlow is the coloured glow
            if let Some(g) = m.texture("txGlow") {
                if let Some(gp) = tex.get(files, g, Role::Mask, skin) {
                    set_slot(&mut d, "Base_EmissiveMap", &gp);
                }
            }
            // the blur set: VehicleDisc blends each map to its *Blur twin by
            // saturate(Velocity * BlurBias) where Base_Has*Blur is set (the
            // stock shader; 48 of 71 stock discs, the MX-5 NA among them, set
            // the colour one). AC1's txBlur is that map; the other blur maps
            // stay off, as the static maps they would pair with are.
            let blur = m.texture("txBlur").and_then(|b| tex.get(files, b, Role::Color, skin));
            let blur = blur.or_else(|| crate::io::material::get_slot(&d, "Base_BaseColorMap").filter(|p| !p.is_empty()));
            if let Some(bp) = blur {
                set_slot(&mut d, "Base_BaseColorMapBlur", &bp);
                setp(&mut d, "Base_HasBaseColorMapBlur", &[1.0]);
            }
            for n in ["Base_HasNormalMapBlur", "Base_HasRoughnessMapBlur", "Base_HasMetalnessMapBlur", "Base_HasAmbientOcclusionMapBlur", "Base_HasClearCoatNormalMapBlur"] {
                setp(&mut d, n, &[0.0]);
            }
            for n in ["Base_NormalMapBlur", "Base_RoughnessMapBlur", "Base_MetalnessMapBlur", "Base_AmbientOcclusionMapBlur", "Base_ClearCoatNormalMapBlur", "SplatMapBlur"] {
                set_slot(&mut d, n, "");
            }
            let p = pbr(m, Kind::Disc);
            setp(&mut d, "Base_Metalness", &[0.9]);
            setp(&mut d, "Base_Roughness", &[p.rough.max(0.45)]);
            d
        }
        _ => generic(files, tex, if inp.interior { &t.simple_int } else { &t.simple }, inp, &diffuse, skin, None),
    }
}

/// `glass` on a given stock template (the interior windscreen).
pub fn glass_on(tex: &mut Tex, tmpl: &Msg, m: &Material, skin: Option<(&str, &Path)>) -> Msg {
    glass(tex, tmpl, m, m.texture("txDiffuse").unwrap_or(""), skin)
}

/// Window / gauge glass: the stock glass of that role as it is (the donor's
/// VehicleGlass / VehicleGlassInterior: every stock EXT_WINDOWS carries a
/// light tint around 0.53 with alpha 0.235 and OpacityOverride 0.25, every
/// INT_WINDOWS / INT_WINDSHIELD 0.475 / 0.235). AC1 window glass is a near
/// black reflector whose alpha is its coverage, so its transmission is
/// (1 - alpha); the tint is darkened only where that is darker than stock
/// glass of alpha 0.24 (ours: tinted / privacy glass), never lightened.
fn glass(tex: &mut Tex, tmpl: &Msg, _m: &Material, diffuse: &str, skin: Option<(&str, &Path)>) -> Msg {
    let mut g = tmpl.clone();
    let base = crate::io::material::get_prop(&g, "Basecolor", 4).unwrap_or_else(|| vec![0.5, 0.5, 0.5, 0.235]);
    let a = tex.image(diffuse, skin.map(|s| s.1)).map(|(img, _, _)| mean(&img)[3]).unwrap_or(0.24);
    let k = ((1.0 - a.clamp(0.0, 1.0)) / 0.76).clamp(0.35, 1.0);
    setp(&mut g, "Basecolor", &[base[0] * k, base[1] * k, base[2] * k, base[3]]);
    g
}

/// One rim / rim-blur batch's material: the stock VehicleRim of that slot
/// (its Velocity / InverseVelocity / blend switches as stock), one plain
/// base layer carrying the AC1 rim's colour, normal and roughness maps.
pub fn build_rim(files: &mut Files, tex: &mut Tex, t: &Templates, m: &Material, skin: Option<(&str, PathBuf)>, slot: RimSlot) -> Msg {
    let kind = if matches!(slot, RimSlot::Rim | RimSlot::Decal) { Kind::Rim } else { Kind::RimBlur };
    let inp = MatIn { kind, m, interior: false, skin, metallic: false };
    let sk = inp.skin.as_ref().map(|(n, p)| (*n, p.as_path()));
    let diffuse = m.texture("txDiffuse").unwrap_or("").to_string();
    let mut s = generic(files, tex, &t.rim[&slot], &inp, &diffuse, sk, Some(slot));
    if slot == RimSlot::Blur {
        // the blur disc is see-through where the AC1 texture's alpha says
        // so, whatever AC1's blend flag (stock: Red/Green/Blue_OpacityMap,
        // here the base layer's)
        if let Some((img, _, _)) = tex.image(&diffuse, sk.map(|x| x.1)) {
            if mean(&img)[3] < 0.98 {
                if let Some(ap) = tex.get(files, &diffuse, Role::Alpha, sk) {
                    set_slot(&mut s, "Base_OpacityMap", &ap);
                    setp(&mut s, "Base_HasOpacityMap", &[1.0]);
                }
            }
        }
        setp(&mut s, "blendMode", &[1.0]);
        setp(&mut s, "UseBlend", &[1.0]);
    }
    s
}

fn generic(files: &mut Files, tex: &mut Tex, tmpl: &Msg, inp: &MatIn, diffuse: &str, skin: Option<(&str, &Path)>, rim: Option<RimSlot>) -> Msg {
    let m = inp.m;
    let mut s = tmpl.clone();
    let p = pbr(m, inp.kind);
    let mut base = [p.tint, p.tint, p.tint, 1.0];
    let multimap = m.texture("txDetail").is_some() && uses_detail(m);
    match tex.image(diffuse, skin.map(|x| x.1)) {
        Some((img, _, _)) if { let mu = mean(&img); mu[0].max(mu[1]).max(mu[2]) > 0.02 || multimap } => {
            if let Some((path, _)) = tex.albedo(files, m, skin, inp.kind == Kind::Paint) {
                set_slot(&mut s, "Base_BaseColorMap", &path);
                setp(&mut s, "Base_HasBaseColorMap", &[1.0]);
            }
            if let Some(d) = tex.detail_layer(files, m, skin, inp.kind == Kind::Paint) {
                apply_detail(&mut s, &d);
            }
            if inp.kind == Kind::Paint {
                base = [1.0, 1.0, 1.0, 1.0];
            }
            // alpha cut-outs (decals, grilles); a multimap's alpha is its detail mask
            let mu = mean(&img);
            if !multimap && (m.alpha_tested || m.blend_mode == 1) && mu[3] < 0.98 {
                if let Some(ap) = tex.get(files, diffuse, Role::Alpha, skin) {
                    set_slot(&mut s, "Base_OpacityMap", &ap);
                    setp(&mut s, "Base_HasOpacityMap", &[1.0]);
                    // stock decals: blendMode 2; the blur disc keeps its slot's 1
                    if rim != Some(RimSlot::Blur) {
                        setp(&mut s, "blendMode", &[2.0]);
                    }
                }
            }
        }
        _ => {
            setp(&mut s, "Base_HasBaseColorMap", &[0.0]);
            base = [0.18, 0.18, 0.18, 1.0];
        }
    }
    setp(&mut s, "Base_Basecolor", &base);
    // the damage shaders' txNormal is the dent map shown with damage
    let damage = m.shader.to_lowercase().contains("damage");
    if let Some(n) = m.texture("txNormal").filter(|_| !damage) {
        if let Some(np) = tex.get(files, n, Role::Normal, skin) {
            set_slot(&mut s, "Base_NormalMap", &np);
            setp(&mut s, "Base_HasNormalMap", &[1.0]);
            setp(&mut s, "Base_NormalScale", &[1.0]);
        }
    }
    let mut rough = p.rough;
    let mut metal = p.metal;
    match inp.kind {
        Kind::Paint => {
            // the stock paint finish (median of the stock .oemmultilayercolor
            // paints, e.g. the Mazda set: solid metalness 0, roughness 0.3,
            // clear coat 0.5..0.6, clear-coat roughness 0; metallic / mica
            // 0.85..0.9, 0.45, 0.7, 0); the AC1 maps' roughness is dropped
            // on paint (stock paint carries none)
            let (mm, rr, cc) = if inp.metallic { (0.85, 0.45, 0.7) } else { (0.0, 0.3, 0.55) };
            metal = mm;
            rough = rr;
            setp(&mut s, "Base_ClearCoat", &[cc]);
            setp(&mut s, "Base_ClearCoatRoughness", &[0.0]);
        }
        Kind::Disc => {
            metal = 0.9;
            rough = rough.max(0.45);
        }
        Kind::Rim | Kind::RimBlur => {
            // stock rim layers: metalness 0.9..0.95, clear coat 0.6..1,
            // clear-coat roughness 0.25..0.27
            metal = metal.max(0.6);
            setp(&mut s, "Base_ClearCoat", &[0.6]);
            setp(&mut s, "Base_ClearCoatRoughness", &[0.27]);
        }
        _ => {}
    }
    // multimap: a baked roughness map (it replaces the scalar)
    if let Some(maps) = m.texture("txMaps") {
        let exp = prop(m, "ksSpecularEXP").unwrap_or(20.0);
        let spec = prop(m, "ksSpecular").unwrap_or(0.3);
        // quantised (exponent in 25 % steps, specular in 0.05 steps) so
        // materials sharing a maps texture share the roughness map
        let qe = 1.25f32.powf((exp.max(1.0).ln() / 1.25f32.ln()).round());
        let qs = (spec / 0.05).round() * 0.05;
        let role = Role::Rough((qe * 1000.0) as u32, (qs * 1000.0) as u32);
        if inp.kind != Kind::RimBlur && inp.kind != Kind::Paint {
            if let Some(rp) = tex.get(files, maps, role, skin) {
                set_slot(&mut s, "Base_RoughnessMap", &rp);
                setp(&mut s, "Base_HasRoughnessMap", &[1.0]);
            }
        }
    }
    setp(&mut s, "Base_Roughness", &[rough]);
    setp(&mut s, "Base_Metalness", &[metal.clamp(0.0, 1.0)]);
    setp(&mut s, "Base_Reflectance", &[0.5]);
    s
}

/// An AC1 texture's name relative to the car folder, for the `.texture`
/// header's source path: `skins/<skin>/<name>` when the skin folder supplies
/// it, else `texture/<name>` (embedded in the kn5).
fn ac1_name(name: &str, from_skin: bool, skin: Option<&str>) -> String {
    match (from_skin, skin) {
        (true, Some(s)) => format!("skins/{s}/{name}"),
        _ => format!("texture/{name}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roughness_law() {
        // sharp highlights are smooth, broad ones rough
        assert!(roughness(500.0, 1.0) < 0.3);
        assert!(roughness(10.0, 1.0) > 0.6);
        assert!(roughness(50.0, 0.05) > roughness(50.0, 1.0));
        assert_eq!(slug("rim blur_D.dds"), "rim_blur_d");
    }

    fn mat(name: &str, shader: &str) -> Material {
        Material { name: name.into(), shader: shader.into(), blend_mode: 0, alpha_tested: false, depth_mode: 0, props: vec![], textures: vec![] }
    }

    #[test]
    fn disc_names() {
        let h = Hints::default();
        // library cases: ks_ford_mustang_2015 (DISC_xx/geo_disc_LF), ks_audi_r18 (WHEEL_LF/ar18_inner_lf on "Brakes"),
        // ks_maserati_250f (drum on EXT_Brake under WHEEL_xx, its rim node RIM_LF_brake on EXT_Rim)
        assert!(disc_like(&mat("EXT_Disc", "ksPerPixelNM"), "geo_disc_LF"));
        assert_eq!(kind_of(&mat("Brakes", "ksPerPixelMultiMap"), "ar18_inner_lf", true, false, false, &h), Kind::Disc);
        assert_eq!(kind_of(&mat("EXT_Brake__brake_D", "ksPerPixelNM_UVMult"), "RIM_LF_brake_SUB0", true, false, false, &h), Kind::Disc);
        assert_eq!(kind_of(&mat("EXT_Rim__Rim_D_png", "ksPerPixelNM_UVMult"), "RIM_LF_brake_SUB1", true, false, false, &h), Kind::Rim);
        // a tyre stays a tyre; a body-level brake-named part is not a disc by itself
        assert_eq!(kind_of(&mat("tyre_brake", "ksTyres"), "TYRE_LF", true, false, false, &h), Kind::Tyre);
        assert_eq!(kind_of(&mat("quattro_brakes", "ksCarPaintSimple"), "CALIPER_LF_MESH", false, false, false, &h), Kind::Generic);
        // a baked-livery body on a plain shader: only when nothing is car paint
        let mats = vec![mat("redbull_paint", "ksPerPixelMultiMap"), mat("redbull_carbon1", "ksPerPixelMultiMap"),
                        mat("redbull_decals", "ksPerPixelMultiMap"), mat("interior_body", "ksPerPixelMultiMap")];
        let mut h2 = Hints::default();
        h2.paint = paint_fallback(&mats, &h2);
        assert_eq!(h2.paint.iter().cloned().collect::<Vec<_>>(), vec!["redbull_paint".to_string()]);
        assert_eq!(kind_of(&mats[0], "body_sub0", false, false, false, &h2), Kind::Paint);
        assert_eq!(kind_of(&mats[0], "body_sub0", false, false, true, &h2), Kind::Generic);
        let with_paint = vec![mat("redbull_paint", "ksPerPixelMultiMap"), mat("body", "ksPerPixelMultiMap_damage_dirt")];
        assert!(paint_fallback(&with_paint, &h).is_empty());
    }
}
