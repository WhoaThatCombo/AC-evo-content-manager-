//! Car brands: which stock brand an AC1 brand is, and the files a brand the
//! game does not ship needs.
//!
//! # What the game does (facts: content.kspkg + the UI scripts in it)
//!
//! - A car's brand is a plain string: `.moddedcarcontent` f10, every
//!   `.mechanicalcarpreset` / `.visualcarpreset` f1, `.design` / `.rimmesh`
//!   f102. Stock cars carry the brand's NAME there (`system\cars.table` f10:
//!   "Mazda", "Alfa Romeo", "Mercedes-Benz"; the stock Datsun 240Z says
//!   "Nissan").
//! - Car select groups cars by that string (`Manufacturers.manufacturers`,
//!   one tile per name) and derives everything else from it in the UI
//!   (`uiresources\js\components.js`, ksUI):
//!   - id = `name.toLowerCase().replace(" ", "_").replace("-", "_")` - JS
//!     string replace, so only the FIRST space and the FIRST hyphen;
//!   - badge `/branding/oem/<id>/badge.texture` (brand tiles, car info),
//!     logo `/branding/oem/<id>/logo.texture` (`getBrandLogoTexturePath`,
//!     `getLogoForBrand`);
//!   - label = loc key `brand_` + `name.toLowerCase()` with every run of
//!     spaces / hyphens as `_` (`getLocId`), from `en.cars.loc`.
//!   A missing texture is only an empty CSS background (no error path in
//!   the UI code); a missing loc key shows the raw key.
//! - Stock brands: `uiresources\branding\oem\<key>\{badge,logo}.texture`
//!   (+ `.texturemips`), 26 keys, every one exactly these 4 files: badge
//!   1024x1024, logo 128x128, header enum 6 (8-bit sRGB stored BGRA) with
//!   the full mip chain (`texture::Fmt::Ui`). Labels in
//!   `uiresources\localization\en.cars.loc` `brand_<key>=` (27 rows: also
//!   `brand_datsun`, which has no branding folder).
//! - `system\brands.table` (PresetTableData f2 -> lines f3 -> BrandData
//!   f11 {f1 name}) lists 63 names (cars, parts and tyre makers, with or
//!   without branding). It is a stock table inside content.kspkg; mods
//!   cannot add rows (a loose or mod file never overrides an archive
//!   path), and no stock UI path reads it for car select, which is built
//!   from the cars' own brand strings. A new brand is therefore NOT
//!   registered anywhere: it exists because a car names it.
//! - The UI fetches `coui://uiresources/branding/oem/<key>/badge.texture`;
//!   the exe's coui handler looks that path up in the game file system
//!   (content.kspkg, then every `mods\*.kspkg`, then loose files under
//!   `Saved Games\ACE\mods\`), by its archive ROOT path. A car-local
//!   `content\cars\<id>\uiresources\branding\...` is NOT served (in game:
//!   "Failed loading resource ... branding/oem/pagani/badge.texture" with
//!   the files there). So the brand goes to the root path
//!   `uiresources\branding\oem\<key>\` inside the mod package
//!   (docs/FORMATS.md, "Car UI resources"): one path per brand key, shared by
//!   every car of that brand; a stock path always wins over a mod's copy
//!   (content.kspkg is mounted first).
//!
//! # Ours
//!
//! - Matching (`match_stock`): case and punctuation are ignored (only
//!   `[a-z0-9]` compared) against each stock key, its stock name and its
//!   English label, plus `ALIASES`; then the longest leading run of words
//!   that matches ("BMW M" -> bmw, "Porsche Motorsport" -> porsche).
//! - A stock brand is written as its stock name, so the car joins the stock
//!   brand's tile; its branding files are the game's.
//! - Any other brand: key = slug of the AC1 brand (`ac1` when empty), and
//!   the NAME written into the car is that key, so the UI's id and loc key
//!   both equal it whatever the AC1 spelling ("Aston Martin Racing" would
//!   otherwise give id `aston_martin racing`). Its label is
//!   `brand_<key>=<AC1 brand>` in the car's `en.cars.loc`.
//! - Two mods of the same non-stock brand write the same paths. The brand is
//!   shown per brand, not per car (one tile per name, one URL per key), so
//!   the key is NOT namespaced per car: that would split one make into
//!   several identical tiles. Instead the files are a pure function of
//!   (key, label, logo bytes): two conversions with the same logo write
//!   byte-identical brand files, and with different logos whichever the game
//!   mounts serves the whole brand - a valid badge either way.
//! - The badge source: AC1 `logo.png`, else `ui\badge.png`, else a text
//!   badge of the brand name (`text_badge`). A source is unusable when it
//!   does not decode, is under 32 px on its long side, or has under 0.5 %
//!   (or 64) texels with alpha >= 16. A very dark mark (mean luminance of
//!   its opaque texels < 0.12: black-on-white AC1 art) is inverted to light,
//!   as every stock badge reads on the dark UI.

use std::collections::BTreeMap;

use crate::io::pb::Msg;
use crate::io::texture::Image;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StockBrand {
    /// the branding folder / loc key (`mercedes_benz`)
    pub key: String,
    /// the name stock cars carry (`Mercedes-Benz`)
    pub name: String,
    /// the English label (`Mercedes-AMG`)
    pub label: String,
}

#[derive(Clone, Debug, Default)]
pub struct Brands {
    pub list: Vec<StockBrand>,
}

/// Other spellings of stock brands (ours; each target is a stock key).
/// - datsun -> nissan: the stock `ks_datsun_240z_fairlady` carries "Nissan";
/// - mercedes / amg / mercedes amg: the stock name is "Mercedes-Benz", its
///   label "Mercedes-AMG";
/// - vw, alfa, chevy: the usual short forms.
pub const ALIASES: [(&str, &str); 8] = [
    ("datsun", "nissan"),
    ("mercedes", "mercedes_benz"),
    ("amg", "mercedes_benz"),
    ("mercedesamg", "mercedes_benz"),
    ("vw", "volkswagen"),
    ("alfa", "alfa_romeo"),
    ("chevy", "chevrolet"),
    ("mercedesbenzamg", "mercedes_benz"),
];

/// `[a-z0-9]` only.
pub fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_lowercase()).collect()
}

/// The UI's brand id of a name (`ksUI.getManufacturerId`: the first space
/// and the first hyphen become `_`).
pub fn ui_id(name: &str) -> String {
    name.to_lowercase().replacen(' ', "_", 1).replacen('-', "_", 1)
}

/// The UI's loc key of a name (`ksUI.getLocId(name, "brand")`).
pub fn ui_loc_key(name: &str) -> String {
    let low = name.to_lowercase();
    let mut o = String::new();
    let mut run = false;
    for c in low.chars() {
        if c == ' ' || c == '-' {
            if !run {
                o.push('_');
            }
            run = true;
        } else {
            o.push(c);
            run = false;
        }
    }
    format!("brand_{}", o.trim())
}

fn table_names(b: &[u8], field: u32, inner: u32) -> Vec<String> {
    let Ok(m) = Msg::parse(b) else { return Vec::new() };
    let mut out = Vec::new();
    for w in m.subs(2) {
        for line in w.subs(3) {
            for c in line.subs(field) {
                if let Some(s) = c.str(inner) {
                    out.push(s);
                }
            }
        }
    }
    out
}

/// `key=value` rows of a `.loc` file.
pub fn loc_rows(b: &[u8]) -> BTreeMap<String, String> {
    String::from_utf8_lossy(b).lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_string())).collect()
}

impl Brands {
    /// The brands the game ships with branding, their stock names and labels.
    pub fn from_game(game: &crate::validate::Game) -> Brands {
        let mut keys: Vec<String> = game.paths_with_prefix("uiresources\\branding\\oem\\").into_iter().filter_map(|p| p.split('\\').nth(3).map(|s| s.to_string())).collect();
        keys.sort();
        keys.dedup();
        // names: what stock cars carry first, then the brands table
        let mut names: Vec<String> = game.read("system\\cars.table").map(|b| table_names(&b, 1, 10)).unwrap_or_default();
        names.extend(game.read("system\\brands.table").map(|b| table_names(&b, 11, 1)).unwrap_or_default());
        let loc = game.read("uiresources\\localization\\en.cars.loc").map(|b| loc_rows(&b)).unwrap_or_default();
        let list = keys.into_iter().map(|key| {
            let label = loc.get(&format!("brand_{key}")).cloned().unwrap_or_else(|| key.clone());
            let name = names.iter().find(|n| ui_id(n) == key).cloned().unwrap_or_else(|| label.clone());
            StockBrand { key, name, label }
        }).collect();
        Brands { list }
    }

    fn by_norm(&self, n: &str) -> Option<&StockBrand> {
        if n.is_empty() {
            return None;
        }
        self.list.iter().find(|b| norm(&b.key) == n || norm(&b.name) == n || norm(&b.label) == n)
            .or_else(|| ALIASES.iter().find(|(a, _)| *a == n).and_then(|(_, k)| self.list.iter().find(|b| b.key == *k)))
    }

    /// The stock brand an AC1 brand string is, if any.
    pub fn match_stock(&self, ac1: &str) -> Option<&StockBrand> {
        let words: Vec<&str> = ac1.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
        (1..=words.len()).rev().find_map(|n| self.by_norm(&norm(&words[..n].concat())))
    }
}

/// The brand of one car.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Brand {
    /// branding folder and loc key
    pub key: String,
    /// the string written into the car's files
    pub name: String,
    /// English label
    pub label: String,
    pub stock: bool,
}

/// The label of an AC1 brand string: letters, digits and common
/// punctuation, single spaces (mods prefix names with invisible symbols to
/// sort them first in AC1's lists).
pub fn clean_label(s: &str) -> String {
    let kept: String = s.chars().map(|c| if c.is_alphanumeric() || "-&.'/+()!".contains(c) { c } else { ' ' }).collect();
    kept.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The brand of a car from its ui_car.json "brand" (and, when that is
/// missing, a stock brand its "name" starts with).
pub fn resolve(ac1_brand: Option<&str>, car_name: Option<&str>, brands: &Brands) -> Brand {
    let stock = |b: &StockBrand| Brand { key: b.key.clone(), name: b.name.clone(), label: b.label.clone(), stock: true };
    let label = match ac1_brand.map(clean_label).filter(|s| !s.is_empty()) {
        Some(l) => l,
        None => match car_name.and_then(|n| brands.match_stock(&clean_label(n))) {
            Some(b) => return stock(b),
            None => "Assetto Corsa".into(),
        },
    };
    if let Some(b) = brands.match_stock(&label) {
        return stock(b);
    }
    let mut key = crate::ids::slug(&label).chars().take(32).collect::<String>().trim_end_matches('_').to_string();
    if key.is_empty() {
        key = "ac1".into();
    }
    Brand { name: key.clone(), key, label, stock: false }
}

// ------------------------------------------------------------- badge art --

/// A usable logo image, prepared (cropped to its mark, dark marks lightened).
pub fn usable_logo(img: &Image) -> Option<Image> {
    if img.w.max(img.h) < 32 || img.w.min(img.h) < 4 {
        return None;
    }
    let n = (img.w * img.h) as usize;
    let opaque = img.px.chunks_exact(4).filter(|p| p[3] >= 16).count();
    if opaque < 64 || (opaque as f64) < 0.005 * n as f64 {
        return None;
    }
    // crop to the mark
    let (mut x0, mut y0, mut x1, mut y1) = (img.w, img.h, 0, 0);
    for y in 0..img.h {
        for x in 0..img.w {
            if img.px[((y * img.w + x) * 4 + 3) as usize] >= 16 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    let (w, h) = (x1 - x0 + 1, y1 - y0 + 1);
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for y in y0..=y1 {
        let s = ((y * img.w + x0) * 4) as usize;
        px.extend_from_slice(&img.px[s..s + (w * 4) as usize]);
    }
    let mut out = Image { w, h, px };
    let (mut lum, mut cnt) = (0.0f64, 0.0f64);
    for p in out.px.chunks_exact(4).filter(|p| p[3] >= 16) {
        lum += (0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64) / 255.0;
        cnt += 1.0;
    }
    if lum / cnt < 0.12 {
        for p in out.px.chunks_exact_mut(4) {
            for c in &mut p[..3] {
                *c = 255 - *c;
            }
        }
    }
    Some(out)
}

/// `img` scaled to fill `fill` of a transparent `size` square, centred.
pub fn fit_square(img: &Image, size: u32, fill: f32) -> Image {
    let s = fill * size as f32 / img.w.max(img.h) as f32;
    let (w, h) = (((img.w as f32 * s).round() as u32).clamp(1, size), ((img.h as f32 * s).round() as u32).clamp(1, size));
    // premultiplied resample: no dark fringes from transparent texels
    let mut pre = img.px.clone();
    for p in pre.chunks_exact_mut(4) {
        let a = p[3] as u32;
        for c in &mut p[..3] {
            *c = ((*c as u32 * a + 127) / 255) as u8;
        }
    }
    let src = image::RgbaImage::from_raw(img.w, img.h, pre).expect("image size");
    let r = image::imageops::resize(&src, w, h, image::imageops::FilterType::CatmullRom);
    let mut px = vec![0u8; (size * size * 4) as usize];
    let (ox, oy) = ((size - w) / 2, (size - h) / 2);
    for y in 0..h {
        for x in 0..w {
            let p = r.get_pixel(x, y).0;
            let a = p[3] as u32;
            let d = (((y + oy) * size + x + ox) * 4) as usize;
            if a > 0 {
                for c in 0..3 {
                    px[d + c] = ((p[c] as u32 * 255 + a / 2) / a).min(255) as u8;
                }
            }
            px[d + 3] = p[3];
        }
    }
    bleed(&mut px);
    Image { w: size, h: size, px }
}

/// Transparent texels take the mean colour of the visible ones, so the mips
/// do not darken the edges.
fn bleed(px: &mut [u8]) {
    let (mut s, mut n) = ([0u64; 3], 0u64);
    for p in px.chunks_exact(4).filter(|p| p[3] > 0) {
        for c in 0..3 {
            s[c] += p[c] as u64 * p[3] as u64;
        }
        n += p[3] as u64;
    }
    if n == 0 {
        return;
    }
    let m = [(s[0] / n) as u8, (s[1] / n) as u8, (s[2] / n) as u8];
    for p in px.chunks_exact_mut(4).filter(|p| p[3] == 0) {
        p[..3].copy_from_slice(&m);
    }
}

/// Stroke glyphs on a 4 x 6 cell (y down): (advance, polylines).
fn glyph(c: char) -> Option<(f32, &'static [&'static [(f32, f32)]])> {
    const O: &[(f32, f32)] = &[(1.0, 0.0), (3.0, 0.0), (4.0, 1.0), (4.0, 5.0), (3.0, 6.0), (1.0, 6.0), (0.0, 5.0), (0.0, 1.0), (1.0, 0.0)];
    const P: &[(f32, f32)] = &[(0.0, 6.0), (0.0, 0.0), (3.0, 0.0), (4.0, 1.0), (4.0, 2.0), (3.0, 3.0), (0.0, 3.0)];
    Some(match c {
        'A' => (4.0, &[&[(0.0, 6.0), (2.0, 0.0), (4.0, 6.0)], &[(0.67, 4.0), (3.33, 4.0)]]),
        'B' => (4.0, &[&[(0.0, 3.0), (0.0, 0.0), (3.0, 0.0), (4.0, 1.0), (4.0, 2.0), (3.0, 3.0), (0.0, 3.0), (0.0, 6.0), (3.0, 6.0), (4.0, 5.0), (4.0, 4.0), (3.0, 3.0)]]),
        'C' => (4.0, &[&[(4.0, 1.0), (3.0, 0.0), (1.0, 0.0), (0.0, 1.0), (0.0, 5.0), (1.0, 6.0), (3.0, 6.0), (4.0, 5.0)]]),
        'D' => (4.0, &[&[(0.0, 0.0), (0.0, 6.0), (2.5, 6.0), (4.0, 4.5), (4.0, 1.5), (2.5, 0.0), (0.0, 0.0)]]),
        'E' => (4.0, &[&[(4.0, 0.0), (0.0, 0.0), (0.0, 6.0), (4.0, 6.0)], &[(0.0, 3.0), (3.0, 3.0)]]),
        'F' => (4.0, &[&[(4.0, 0.0), (0.0, 0.0), (0.0, 6.0)], &[(0.0, 3.0), (3.0, 3.0)]]),
        'G' => (4.0, &[&[(4.0, 1.0), (3.0, 0.0), (1.0, 0.0), (0.0, 1.0), (0.0, 5.0), (1.0, 6.0), (3.0, 6.0), (4.0, 5.0), (4.0, 3.0), (2.0, 3.0)]]),
        'H' => (4.0, &[&[(0.0, 0.0), (0.0, 6.0)], &[(4.0, 0.0), (4.0, 6.0)], &[(0.0, 3.0), (4.0, 3.0)]]),
        'I' => (0.0, &[&[(0.0, 0.0), (0.0, 6.0)]]),
        'J' => (4.0, &[&[(4.0, 0.0), (4.0, 5.0), (3.0, 6.0), (1.0, 6.0), (0.0, 5.0)]]),
        'K' => (4.0, &[&[(0.0, 0.0), (0.0, 6.0)], &[(4.0, 0.0), (0.0, 3.5)], &[(1.3, 2.6), (4.0, 6.0)]]),
        'L' => (4.0, &[&[(0.0, 0.0), (0.0, 6.0), (4.0, 6.0)]]),
        'M' => (5.0, &[&[(0.0, 6.0), (0.0, 0.0), (2.5, 3.5), (5.0, 0.0), (5.0, 6.0)]]),
        'N' => (4.0, &[&[(0.0, 6.0), (0.0, 0.0), (4.0, 6.0), (4.0, 0.0)]]),
        'O' => (4.0, &[O]),
        'P' => (4.0, &[P]),
        'Q' => (4.0, &[O, &[(2.5, 4.5), (4.0, 6.0)]]),
        'R' => (4.0, &[P, &[(2.0, 3.0), (4.0, 6.0)]]),
        'S' => (4.0, &[&[(4.0, 1.0), (3.0, 0.0), (1.0, 0.0), (0.0, 1.0), (0.0, 2.0), (1.0, 3.0), (3.0, 3.0), (4.0, 4.0), (4.0, 5.0), (3.0, 6.0), (1.0, 6.0), (0.0, 5.0)]]),
        'T' => (4.0, &[&[(0.0, 0.0), (4.0, 0.0)], &[(2.0, 0.0), (2.0, 6.0)]]),
        'U' => (4.0, &[&[(0.0, 0.0), (0.0, 5.0), (1.0, 6.0), (3.0, 6.0), (4.0, 5.0), (4.0, 0.0)]]),
        'V' => (4.0, &[&[(0.0, 0.0), (2.0, 6.0), (4.0, 0.0)]]),
        'W' => (5.0, &[&[(0.0, 0.0), (1.25, 6.0), (2.5, 2.0), (3.75, 6.0), (5.0, 0.0)]]),
        'X' => (4.0, &[&[(0.0, 0.0), (4.0, 6.0)], &[(4.0, 0.0), (0.0, 6.0)]]),
        'Y' => (4.0, &[&[(0.0, 0.0), (2.0, 3.0), (4.0, 0.0)], &[(2.0, 3.0), (2.0, 6.0)]]),
        'Z' => (4.0, &[&[(0.0, 0.0), (4.0, 0.0), (0.0, 6.0), (4.0, 6.0)]]),
        '0' => (4.0, &[O, &[(3.4, 0.9), (0.6, 5.1)]]),
        '1' => (3.0, &[&[(0.5, 1.0), (1.5, 0.0), (1.5, 6.0)], &[(0.0, 6.0), (3.0, 6.0)]]),
        '2' => (4.0, &[&[(0.0, 1.0), (1.0, 0.0), (3.0, 0.0), (4.0, 1.0), (4.0, 2.0), (0.0, 6.0), (4.0, 6.0)]]),
        '3' => (4.0, &[&[(0.0, 1.0), (1.0, 0.0), (3.0, 0.0), (4.0, 1.0), (4.0, 2.0), (3.0, 3.0), (4.0, 4.0), (4.0, 5.0), (3.0, 6.0), (1.0, 6.0), (0.0, 5.0)], &[(1.5, 3.0), (3.0, 3.0)]]),
        '4' => (4.0, &[&[(3.0, 6.0), (3.0, 0.0), (0.0, 4.0), (4.0, 4.0)]]),
        '5' => (4.0, &[&[(4.0, 0.0), (0.0, 0.0), (0.0, 3.0), (3.0, 3.0), (4.0, 4.0), (4.0, 5.0), (3.0, 6.0), (0.0, 6.0)]]),
        '6' => (4.0, &[&[(3.5, 0.0), (1.0, 0.0), (0.0, 1.0), (0.0, 5.0), (1.0, 6.0), (3.0, 6.0), (4.0, 5.0), (4.0, 4.0), (3.0, 3.0), (0.0, 3.0)]]),
        '7' => (4.0, &[&[(0.0, 0.0), (4.0, 0.0), (1.5, 6.0)]]),
        '8' => (4.0, &[&[(1.0, 3.0), (0.0, 2.0), (0.0, 1.0), (1.0, 0.0), (3.0, 0.0), (4.0, 1.0), (4.0, 2.0), (3.0, 3.0), (1.0, 3.0), (0.0, 4.0), (0.0, 5.0), (1.0, 6.0), (3.0, 6.0), (4.0, 5.0), (4.0, 4.0), (3.0, 3.0)]]),
        '9' => (4.0, &[&[(4.0, 3.0), (1.0, 3.0), (0.0, 2.0), (0.0, 1.0), (1.0, 0.0), (3.0, 0.0), (4.0, 1.0), (4.0, 5.0), (3.0, 6.0), (0.5, 6.0)]]),
        '-' => (3.0, &[&[(0.0, 3.5), (3.0, 3.5)]]),
        '+' | '&' => (4.0, &[&[(2.0, 1.5), (2.0, 4.5)], &[(0.5, 3.0), (3.5, 3.0)]]),
        '.' => (0.0, &[&[(0.0, 6.0), (0.0, 6.0)]]),
        '\'' => (0.0, &[&[(0.0, 0.0), (0.0, 1.5)]]),
        '/' => (3.0, &[&[(0.0, 6.0), (3.0, 0.0)]]),
        _ => return None,
    })
}

const GAP: f32 = 1.6;
const SPACE: f32 = 3.2;
const STROKE: f32 = 0.9;
const LINE_GAP: f32 = 2.4;

/// The brand name as glyph text: upper case, common accents folded,
/// unknown characters dropped.
fn glyph_text(s: &str) -> String {
    let mut o = String::new();
    for c in s.chars().flat_map(|c| c.to_uppercase()) {
        let c = match c {
            'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' => 'A',
            'Ç' => 'C',
            'È' | 'É' | 'Ê' | 'Ë' => 'E',
            'Ì' | 'Í' | 'Î' | 'Ï' => 'I',
            'Ñ' => 'N',
            'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' | 'Ø' => 'O',
            'Ù' | 'Ú' | 'Û' | 'Ü' => 'U',
            'Ý' => 'Y',
            '_' => ' ',
            c => c,
        };
        if c.is_whitespace() {
            if !o.is_empty() && !o.ends_with(' ') {
                o.push(' ');
            }
        } else if glyph(c).is_some() {
            o.push(c);
        }
    }
    o.trim().to_string()
}

/// Width of a line in cell units (stroke included).
fn line_width(s: &str) -> f32 {
    let mut w = 0.0;
    let mut first = true;
    for c in s.chars() {
        if c == ' ' {
            w += SPACE;
            first = true;
            continue;
        }
        let (adv, _) = glyph(c).unwrap();
        if !first {
            w += GAP;
        }
        w += adv;
        first = false;
    }
    w + STROKE
}

/// The word groups (1..=3 lines) that let the text be largest in a square.
fn layout(text: &str) -> Vec<String> {
    let words: Vec<&str> = text.split(' ').collect();
    let mut best: (f32, Vec<String>) = (0.0, vec![text.to_string()]);
    let n = words.len();
    let mut try_split = |cuts: &[usize]| {
        let mut lines = Vec::new();
        let mut a = 0;
        for &c in cuts.iter().chain(std::iter::once(&n)) {
            lines.push(words[a..c].join(" "));
            a = c;
        }
        let w = lines.iter().map(|l| line_width(l)).fold(0.0f32, f32::max);
        let h = lines.len() as f32 * 6.0 + (lines.len() - 1) as f32 * LINE_GAP + STROKE;
        // the larger the scale that fits (w, h) in the unit square, the better
        let s = (1.0 / w).min(1.0 / h);
        if s > best.0 * 1.0001 {
            best = (s, lines);
        }
    };
    try_split(&[]);
    for i in 1..n {
        try_split(&[i]);
        for j in i + 1..n {
            try_split(&[i, j]);
        }
    }
    best.1
}

/// A text badge: the brand name in light strokes on transparent, centred in
/// a `size` square (letters at most 0.22 of the square high).
pub fn text_badge(name: &str, size: u32) -> Image {
    let mut text = glyph_text(name);
    if text.is_empty() {
        text = "AC".into();
    }
    let lines = layout(&text);
    let w = lines.iter().map(|l| line_width(l)).fold(0.0f32, f32::max);
    let h = lines.len() as f32 * 6.0 + (lines.len() - 1) as f32 * LINE_GAP + STROKE;
    let box_ = 0.86 * size as f32;
    let scale = (box_ / w).min(box_ / h).min(0.22 * size as f32 / 6.0);
    let mut cov = vec![0f32; (size * size) as usize];
    let r = STROKE * 0.5 * scale;
    let oy = (size as f32 - h * scale) * 0.5 + r;
    for (li, line) in lines.iter().enumerate() {
        let lw = line_width(line);
        let mut x = (size as f32 - lw * scale) * 0.5 + r;
        let y = oy + li as f32 * (6.0 + LINE_GAP) * scale;
        let mut first = true;
        for c in line.chars() {
            if c == ' ' {
                x += SPACE * scale;
                first = true;
                continue;
            }
            let (adv, strokes) = glyph(c).unwrap();
            if !first {
                x += GAP * scale;
            }
            first = false;
            for st in strokes {
                for seg in st.windows(2) {
                    let a = (x + seg[0].0 * scale, y + seg[0].1 * scale);
                    let b = (x + seg[1].0 * scale, y + seg[1].1 * scale);
                    stroke(&mut cov, size, a, b, r);
                }
            }
            x += adv * scale;
        }
    }
    let mut px = vec![0u8; (size * size * 4) as usize];
    for (i, c) in cov.iter().enumerate() {
        px[i * 4..i * 4 + 4].copy_from_slice(&[242, 242, 242, (c * 255.0).round() as u8]);
    }
    Image { w: size, h: size, px }
}

/// A round-capped segment, anti-aliased (1 px ramp), into a coverage map.
fn stroke(cov: &mut [f32], size: u32, a: (f32, f32), b: (f32, f32), r: f32) {
    let x0 = ((a.0.min(b.0) - r - 1.0).floor().max(0.0)) as u32;
    let y0 = ((a.1.min(b.1) - r - 1.0).floor().max(0.0)) as u32;
    let x1 = ((a.0.max(b.0) + r + 1.0).ceil() as u32).min(size - 1);
    let y1 = ((a.1.max(b.1) + r + 1.0).ceil() as u32).min(size - 1);
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let ll = dx * dx + dy * dy;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let (px, py) = (x as f32 + 0.5 - a.0, y as f32 + 0.5 - a.1);
            let t = if ll > 0.0 { ((px * dx + py * dy) / ll).clamp(0.0, 1.0) } else { 0.0 };
            let d = ((px - t * dx).powi(2) + (py - t * dy).powi(2)).sqrt();
            let c = (r - d + 0.5).clamp(0.0, 1.0);
            let i = (y * size + x) as usize;
            if c > cov[i] {
                cov[i] = c;
            }
        }
    }
}

/// Badge (1024) and logo (128) images of a car-local brand, and where they
/// came from.
pub fn art(ac1_dir: &std::path::Path, label: &str) -> ([Image; 2], &'static str) {
    match logo_source(ac1_dir) {
        Some((mark, what)) => ([fit_square(&mark, 1024, 0.92), fit_square(&mark, 128, 0.92)], what),
        None => ([text_badge(label, 1024), text_badge(label, 128)], "text"),
    }
}

/// Where a car-local brand's badge comes from: "logo.png", "ui\badge.png" or "text".
pub fn art_source(ac1_dir: &std::path::Path) -> &'static str {
    logo_source(ac1_dir).map(|s| s.1).unwrap_or("text")
}

fn logo_source(ac1_dir: &std::path::Path) -> Option<(Image, &'static str)> {
    use crate::ac1::car::find_ci;
    let sources = [(find_ci(ac1_dir, "logo.png"), "logo.png"), (find_ci(ac1_dir, "ui").and_then(|d| find_ci(&d, "badge.png")), "ui\\badge.png")];
    for (path, what) in sources {
        let Some(img) = path.filter(|p| p.is_file()).and_then(|p| std::fs::read(p).ok()).and_then(|b| crate::io::texture::decode_any(&b).ok()) else { continue };
        if let Some(mark) = usable_logo(&img) {
            return Some((mark, what));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stock() -> Brands {
        let b = |k: &str, n: &str, l: &str| StockBrand { key: k.into(), name: n.into(), label: l.into() };
        Brands { list: vec![b("mercedes_benz", "Mercedes-Benz", "Mercedes-AMG"), b("alfa_romeo", "Alfa Romeo", "Alfa Romeo"), b("bmw", "BMW", "BMW"), b("nissan", "Nissan", "Nissan"), b("porsche", "Porsche", "Porsche"), b("volkswagen", "Volkswagen", "Volkswagen"), b("mini", "Mini", "Mini")] }
    }

    #[test]
    fn matching() {
        let s = stock();
        let k = |a: &str| s.match_stock(a).map(|b| b.key.clone());
        assert_eq!(k("Mercedes-Benz").as_deref(), Some("mercedes_benz"));
        assert_eq!(k("MERCEDES BENZ").as_deref(), Some("mercedes_benz"));
        assert_eq!(k("Mercedes").as_deref(), Some("mercedes_benz"));
        assert_eq!(k("Mercedes-AMG").as_deref(), Some("mercedes_benz"));
        assert_eq!(k("alfa-romeo").as_deref(), Some("alfa_romeo"));
        assert_eq!(k("BMW M").as_deref(), Some("bmw"));
        assert_eq!(k("Porsche Motorsport").as_deref(), Some("porsche"));
        assert_eq!(k("Datsun").as_deref(), Some("nissan"));
        assert_eq!(k("VW").as_deref(), Some("volkswagen"));
        assert_eq!(k("MINI").as_deref(), Some("mini"));
        assert_eq!(k("Pagani"), None);
        assert_eq!(k("Minimax"), None);
        assert_eq!(k(""), None);
        let r = resolve(Some("Mercedes-AMG"), None, &s);
        assert_eq!((r.key.as_str(), r.name.as_str(), r.stock), ("mercedes_benz", "Mercedes-Benz", true));
        let r = resolve(Some("Aston Martin Racing"), None, &s);
        assert_eq!((r.key.as_str(), r.name.as_str(), r.label.as_str(), r.stock), ("aston_martin_racing", "aston_martin_racing", "Aston Martin Racing", false));
        assert_eq!(resolve(None, None, &s).key, "assetto_corsa");
        assert_eq!(resolve(None, Some("Pagani Zonda"), &s).key, "assetto_corsa");
        assert_eq!(resolve(None, Some("BMW M3 E30"), &s).name, "BMW");
        assert_eq!(resolve(Some("???"), None, &s).key, "assetto_corsa");
        assert_eq!(resolve(Some("日本"), None, &s).key, "ac1");
        let r = resolve(Some("\u{1d17a}Kia"), None, &s);
        assert_eq!((r.key.as_str(), r.label.as_str()), ("kia", "Kia"));
        assert_eq!(resolve(Some("\u{1d17a}BMW"), None, &s).key, "bmw");
    }

    #[test]
    fn ui_ids() {
        assert_eq!(ui_id("Mercedes-Benz"), "mercedes_benz");
        assert_eq!(ui_id("Alfa Romeo"), "alfa_romeo");
        // only the first space: why car-local names are their keys
        assert_eq!(ui_id("Aston Martin Racing"), "aston_martin racing");
        assert_eq!(ui_loc_key("Aston Martin Racing"), "brand_aston_martin_racing");
        assert_eq!(ui_id("aston_martin_racing"), "aston_martin_racing");
    }

    #[test]
    fn text_badges() {
        let img = text_badge("Pagani", 1024);
        let on = img.px.chunks_exact(4).filter(|p| p[3] > 128).count();
        assert!(on > 5000 && on < 1024 * 1024 / 4, "{on}");
        // centred: the mark's box is symmetric
        let xs: Vec<u32> = (0..1024 * 1024).filter(|i| img.px[*i as usize * 4 + 3] > 128).map(|i| i % 1024).collect();
        let (a, b) = (*xs.iter().min().unwrap(), *xs.iter().max().unwrap());
        assert!((a as i32 - (1023 - b) as i32).abs() <= 3, "{a} {b}");
        // long names wrap and still fit
        let img = text_badge("Automobili Turismo e Sport Racing", 128);
        assert!(img.px.chunks_exact(4).any(|p| p[3] > 200));
        assert_eq!(layout("A B").len(), 1);
        assert!(layout("AUTOMOBILI TURISMO SPORT").len() >= 2);
        // nothing renderable -> still a badge
        assert!(text_badge("日本", 128).px.chunks_exact(4).any(|p| p[3] > 200));
        // deterministic
        assert_eq!(text_badge("Pagani", 128).px, text_badge("Pagani", 128).px);
    }

    #[test]
    fn logos() {
        let blank = Image { w: 256, h: 256, px: vec![0; 256 * 256 * 4] };
        assert!(usable_logo(&blank).is_none());
        let tiny = Image { w: 16, h: 16, px: vec![255; 16 * 16 * 4] };
        assert!(usable_logo(&tiny).is_none());
        // a black square on transparent: cropped and lightened
        let mut px = vec![0u8; 200 * 100 * 4];
        for y in 20..60 {
            for x in 50..150 {
                px[(y * 200 + x) * 4 + 3] = 255;
            }
        }
        let m = usable_logo(&Image { w: 200, h: 100, px }).unwrap();
        assert_eq!((m.w, m.h), (100, 40));
        assert!(m.px[0] > 200);
        let f = fit_square(&m, 128, 0.92);
        assert_eq!((f.w, f.h), (128, 128));
        assert_eq!(f.px[(64 * 128 + 64) * 4 + 3], 255);
        assert_eq!(f.px[3], 0);
    }
}
