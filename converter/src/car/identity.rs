//! Car identity: the mod id, the donor clone and what makes the clone a new,
//! independent car.
//!
//! - The id is `ids::car_id` of the AC1 folder (docs/FORMATS.md, "Ids").
//! - The clone renames the donor through protobuf (`donor::rename_content`:
//!   length prefixes re-encoded, so the id may have any length): the folder
//!   id `ks_modded_car` -> the mod id, and the donor's token `modded_car`
//!   (file names, preset names such as `preset_modded_car_mech_1`, the
//!   localisation keys) -> the mod id without `ks_` ("stem"). Paths are
//!   renamed case-insensitively (the donor's generated thumbnail is
//!   capitalised).
//! - The brand (`brand.rs`: what the game needs, the matching rules): the
//!   AC1 ui_car.json brand matched to a stock brand (written as the stock
//!   name, e.g. "Mercedes-Benz"; branding from the game); otherwise a
//!   complete brand of our own: `uiresources\branding\oem\<key>\{badge,logo}`
//!   textures at the package ROOT (where the UI fetches them; car-local
//!   copies are never served) from the AC1 logo (or a text badge), and its
//!   `brand_<key>` label in the car-local `en.cars.loc` (the exe merges
//!   every `content\cars\*\uiresources\localization\<lang>.cars.loc`).
//! - Descriptions: `uiresources\localization\cars\<id>\en.loc` and
//!   `<mech preset>.en.loc` at the package root (`description_locs`).
//! - `.carfinalstate` identifiers: every stock final state carries one
//!   number N, unique across the game (mx5_na 242..244, ae86 355..359, 935
//!   305..310), as the GUID string `00000000-0000-0000-0000-<N in 12 hex>`,
//!   the PGUID {b: N} of each part and the livery linker {b: N}. Ours:
//!   N = 0x7000_0000_0000 | (FNV-1a-64 of "<id>/<final state file>" & 0x0fff_ffff_ffff),
//!   far above the stock numbers, stable per car and file.

use anyhow::{anyhow, Result};
use std::collections::HashSet;

use crate::ac1::car::Car;
use crate::files::Files;
use crate::io::pb::{Msg, Val};

/// The donor car's id (`content\cars\ks_modded_car\...`, built by `donor`).
pub const DONOR_ID: &str = "ks_modded_car";
/// The donor's per-car token in file and preset names.
pub const DONOR_STEM: &str = "modded_car";
/// The donor's own (generated) brand.
pub const DONOR_BRAND: &str = "modded_brand";

#[derive(Clone, Debug)]
pub struct Identity {
    pub id: String,
    /// id without "ks_": the token in file and preset names
    pub stem: String,
    pub display: String,
    /// brand key: branding folder and loc key
    pub brand: String,
    /// the brand string written into the car (stock name, or the key)
    pub brand_name: String,
    pub brand_display: String,
    /// the brand is a stock one (branding comes from the game)
    pub stock_brand: bool,
    pub year: u32,
    pub folder: String,
}

impl Identity {
    pub fn base(&self) -> String {
        format!("content\\cars\\{}", self.id)
    }
    /// The mechanical preset name (the `<id>.json` "name").
    pub fn mech_preset(&self) -> String {
        format!("preset_{}_mech_1", self.stem)
    }
}

/// The brands the game ships (`uiresources\branding\oem\<key>\...`).
pub fn stock_brands(game: &crate::validate::Game) -> crate::car::brand::Brands {
    crate::car::brand::Brands::from_game(game)
}

pub fn identity(car: &Car, id: Option<&str>, taken: &HashSet<String>, brands: &crate::car::brand::Brands) -> Identity {
    let id = id.map(|s| s.to_string()).unwrap_or_else(|| crate::ids::car_id(&car.folder, taken));
    let stem = id.strip_prefix("ks_").unwrap_or(&id).to_string();
    let display = car.ui("name").unwrap_or_else(|| car.folder.clone());
    let b = crate::car::brand::resolve(car.ui("brand").as_deref(), car.ui("name").as_deref(), brands);
    let (brand, brand_name, brand_display, stock_brand) = (b.key, b.name, b.label, b.stock);
    let year = car.ui("year").and_then(|y| y.parse::<f64>().ok()).map(|y| y as u32).filter(|y| (1880..=2100).contains(y)).unwrap_or(2000);
    Identity { id, stem, display, brand, brand_name, brand_display, stock_brand, year, folder: car.folder.clone() }
}

/// The rename pairs of a clone.
pub fn pairs(ident: &Identity) -> Vec<(String, String)> {
    vec![
        (DONOR_ID.to_string(), ident.id.clone()),
        (DONOR_STEM.to_string(), ident.stem.clone()),
        (DONOR_BRAND.to_string(), ident.brand.clone()),
    ]
}

/// The donor, renamed to the mod id.
pub fn clone_donor(donor: Files, ident: &Identity) -> Files {
    let p = pairs(ident);
    let pr: Vec<(&str, &str)> = p.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let mut out = Files::default();
    for (path, data) in donor.map {
        let np = crate::donor::rename_path(&path, &pr);
        let nd = crate::donor::rename_content(&path, &data, &pr);
        out.put(&np, nd);
    }
    out
}

/// Where a brand's badge / logo live in the mod package: the archive ROOT
/// path the UI requests (`coui://uiresources/branding/oem/<key>/...`), as
/// stock ships them in content.kspkg.
pub fn brand_root(key: &str) -> String {
    format!("uiresources\\branding\\oem\\{key}")
}

/// A JSON string value out of lenient AC1 json: raw newlines allowed,
/// escapes decoded. `None` when absent or empty.
pub fn json_string(text: &str, key: &str) -> Option<String> {
    let re = regex::Regex::new(&format!(r#""{}"\s*:\s*""#, regex::escape(key))).ok()?;
    let m = re.find(text)?;
    let mut out = String::new();
    let mut it = text[m.end()..].chars();
    while let Some(c) = it.next() {
        match c {
            '"' => break,
            '\\' => match it.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => {}
                'u' => {
                    let h: String = it.by_ref().take(4).collect();
                    if let Some(ch) = u32::from_str_radix(&h, 16).ok().and_then(char::from_u32) {
                        out.push(ch);
                    }
                }
                o => out.push(o),
            },
            o => out.push(o),
        }
    }
    let t = out.trim().to_string();
    (!t.is_empty()).then_some(t)
}

/// AC1 description html -> paragraphs of plain text (`<br>` breaks lines,
/// other tags dropped, a few entities decoded, whitespace collapsed).
pub fn description_paragraphs(html: &str) -> Vec<String> {
    let br = regex::Regex::new(r"(?i)<\s*br\s*/?\s*>|</?\s*p\s*>").unwrap();
    let tag = regex::Regex::new(r"<[^>]*>").unwrap();
    let t = br.replace_all(html, "\n");
    let t = tag.replace_all(&t, "");
    let t = t.replace("&nbsp;", " ").replace("&amp;", "&").replace("&quot;", "\"").replace("&#39;", "'").replace("&lt;", "<").replace("&gt;", ">");
    let mut paras = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    for line in t.lines() {
        let l = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if l.is_empty() {
            if !cur.is_empty() {
                paras.push(cur.join(" "));
                cur.clear();
            }
        } else {
            cur.push(l);
        }
    }
    if !cur.is_empty() {
        paras.push(cur.join(" "));
    }
    paras
}

/// The per-car description files the UI requests:
/// `uiresources\localization\cars\<id>\en.loc` (the car) and
/// `<mech preset>.en.loc` (the preset). Stock format: plain text with the
/// UI's markdown (`**_Name_**`, `- **Label:** value`), CRLF, opening with
/// one blank line pair ("\r\n\r\n"), paragraphs split by a blank line.
/// Ours: the car text is the AC1 ui_car.json description (else the bold
/// name); the preset text is the bold name and the AC1 spec lines.
pub fn description_locs(car: &Car, ident: &Identity) -> (String, String) {
    let join = |p: &[String]| format!("\r\n\r\n{}\r\n", p.join("\r\n\r\n"));
    let bold = format!("**_{}_**", ident.display.replace('*', ""));
    let mut desc = json_string(&car.ui_text, "description").map(|d| description_paragraphs(&d)).unwrap_or_default();
    if desc.is_empty() {
        desc.push(bold.clone());
    }
    let mut specs = Vec::new();
    for (k, label) in [("bhp", "Power"), ("torque", "Torque"), ("weight", "Weight"), ("pwratio", "Power to weight"), ("topspeed", "Top speed"), ("acceleration", "Acceleration")] {
        if let Some(v) = json_string(&car.ui_text, k).filter(|v| v.starts_with(|c: char| c.is_ascii_digit())) {
            specs.push(format!("- **{label}:** {v}"));
        }
    }
    let mut preset = vec![bold];
    if !specs.is_empty() {
        preset.push(specs.join("\r\n"));
    }
    (join(&desc), join(&preset))
}

/// The final-state number of one `.carfinalstate` file.
pub fn final_state_number(id: &str, file: &str) -> u64 {
    0x7000_0000_0000 | (crate::ids::fnv1a64(&format!("{id}/{}", file.to_lowercase())) & 0x0fff_ffff_ffff)
}

pub fn guid_string(n: u64) -> String {
    format!("00000000-0000-0000-{:04x}-{:012x}", (n >> 48) & 0xffff, n & 0xffff_ffff_ffff)
}

fn pguid(n: u64) -> Msg {
    let mut m = Msg::default();
    m.set_varint(2, n);
    m
}

/// Give a final state its own identifiers.
pub fn set_final_state_ids(data: &[u8], n: u64) -> Result<Vec<u8>> {
    let mut m = Msg::parse(data)?;
    for num in [10u32, 20, 30] {
        for i in m.idx(num) {
            let part = m.open(i)?;
            if part.get(100).is_some() {
                part.set_str(100, &guid_string(n));
            }
            if part.get(101).is_some() {
                part.set(101, Val::Msg(pguid(n)));
            }
        }
    }
    if m.get(52).is_some() {
        m.set(52, Val::Msg(pguid(n)));
    }
    Ok(m.encode())
}

/// Display name, year, brand, final-state ids, localisation of the brand.
pub fn apply(files: &mut Files, ident: &Identity, car: &Car) -> Result<Vec<String>> {
    let base = ident.base();
    let mut notes = Vec::new();
    // .moddedcarcontent
    let mc = format!("{base}\\{}.moddedcarcontent", ident.id);
    let b = files.get(&mc).cloned().ok_or_else(|| anyhow!("the donor has no .moddedcarcontent"))?;
    let mut m = Msg::parse(&b)?;
    m.set_str(4, &ident.display);
    m.set_varint(5, ident.year as u64);
    m.set_str(10, &ident.brand_name);
    files.put(&mc, m.encode());
    // the brand string of the presets, designs and rims (the clone wrote the key)
    for p in files.under(&base) {
        let low = p.to_lowercase();
        let num = if low.ends_with(".mechanicalcarpreset") || low.ends_with(".visualcarpreset") {
            1
        } else if low.ends_with(".design") || low.ends_with(".rimmesh") {
            102
        } else {
            continue;
        };
        let Some(mut m) = files.get(&p).and_then(|b| Msg::parse(b).ok()) else { continue };
        if m.str(num).is_some_and(|s| s != ident.brand_name) {
            m.set_str(num, &ident.brand_name);
            files.put(&p, m.encode());
        }
    }
    // final states
    for p in files.under(&format!("{base}\\presets")) {
        if !p.to_lowercase().ends_with(".carfinalstate") {
            continue;
        }
        let file = p.rsplit('\\').next().unwrap_or(&p).to_string();
        let n = final_state_number(&ident.id, &file);
        let nb = set_final_state_ids(files.get(&p).unwrap(), n)?;
        files.put(&p, nb);
    }
    // CarData screen names (the physics stage rewrites the rest of CarData)
    let cd = format!("{base}\\data\\cardata.car");
    if let Some(key) = files.find_ci(&cd) {
        let mut m = Msg::parse(files.get(&key).unwrap())?;
        for i in m.idx(1) {
            m.open(i)?.set_str(2, &ident.display);
        }
        if m.get(202).is_some() {
            m.set_str(202, &ident.display);
        }
        files.put(&key, m.encode());
    }
    // brand: localisation and branding
    let loc = format!("{base}\\uiresources\\localization\\en.cars.loc");
    let key = files.find_ci(&loc).unwrap_or(loc);
    let t = files.get(&key).map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
    let bk = format!("brand_{}", ident.brand);
    let mut lines: Vec<String> = t.lines().filter(|l| !l.trim().is_empty())
        .filter(|l| !l.split_once('=').is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case(&bk)))
        .map(|l| l.to_string()).collect();
    // a stock brand is named by the game's own localisation
    if !ident.stock_brand {
        lines.insert(0, format!("{bk}={}", ident.brand_display));
    }
    files.put(&key, lines.iter().map(|l| format!("{l}\r\n")).collect::<String>().into_bytes());
    // car-local branding is never served to the UI (docs/FORMATS.md, "Car UI
    // resources"): drop any (the donor's), the brand goes to the root path
    for p in files.under(&format!("{base}\\uiresources\\branding")) {
        files.remove(&p);
    }
    let brand_dir = brand_root(&ident.brand);
    for p in files.under(&brand_dir) {
        files.remove(&p);
    }
    // the car's descriptions, at the global path the UI requests
    let (desc, preset) = description_locs(car, ident);
    let loc_dir = format!("uiresources\\localization\\cars\\{}", ident.id);
    for p in files.under(&loc_dir) {
        files.remove(&p);
    }
    files.put(&format!("{loc_dir}\\en.loc"), desc.clone().into_bytes());
    files.put(&format!("{loc_dir}\\{}.en.loc", ident.mech_preset()), preset.into_bytes());
    if let Some(k) = files.find_ci(&format!("{base}\\uiresources\\localization\\en.loc")) {
        files.put(&k, desc.into_bytes());
    }
    if ident.stock_brand {
        notes.push(format!("identity: brand {} = stock \"{}\"", ident.brand, ident.brand_name));
    } else {
        // a pure function of (key, label, logo): two mods of one brand write the same bytes
        let (imgs, src) = crate::car::brand::art(&car.dir, &ident.brand_display);
        for (name, img) in ["badge", "logo"].iter().zip(imgs.iter()) {
            let e = crate::io::texture::encode(img, crate::io::texture::Fmt::Ui, true, &format!("{name}.png"));
            files.put(&format!("{brand_dir}\\{name}.texture"), e.header);
            files.put(&format!("{brand_dir}\\{name}.texturemips"), e.mips);
        }
        notes.push(format!("identity: brand {} \"{}\" (own, badge from {src})", ident.brand, ident.brand_display));
    }
    Ok(notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptions() {
        let t = "{\n\t\"name\": \"X\",\n\t\"description\": \"First \\\"one\\\" line\n<br><br>\n\tSecond &amp; last. \n\t\",\n\t\"specs\": {\"bhp\": \"740bhp\", \"topspeed\": \"--km/h\"}\n}";
        let d = json_string(t, "description").unwrap();
        assert_eq!(description_paragraphs(&d), vec!["First \"one\" line".to_string(), "Second & last.".to_string()]);
        assert_eq!(json_string(t, "bhp").as_deref(), Some("740bhp"));
        assert!(json_string(t, "class").is_none());
    }

    #[test]
    fn final_state_ids() {
        let n = final_state_number("ks_test_car_abcde", "a.carfinalstate");
        assert_eq!(n >> 44, 7);
        assert_eq!(guid_string(0xf2), "00000000-0000-0000-0000-0000000000f2");
        let mut part = Msg::default();
        part.set_str(1, "x");
        part.set_str(100, &guid_string(1));
        part.set(101, Val::Msg(pguid(1)));
        let mut top = Msg::default();
        top.push(10, Val::Msg(part));
        top.push(52, Val::Msg(pguid(1)));
        let out = Msg::parse(&set_final_state_ids(&top.encode(), n).unwrap()).unwrap();
        assert_eq!(out.sub(10).unwrap().str(100).unwrap(), guid_string(n));
        assert_eq!(out.sub(52).unwrap().varint(2), Some(n));
    }

    #[test]
    fn clone_renames() {
        let ident = Identity { id: "ks_bmw_m3_e30_x1y2z".into(), stem: "bmw_m3_e30_x1y2z".into(), display: "BMW".into(), brand: "bmw".into(), brand_name: "BMW".into(), brand_display: "BMW".into(), stock_brand: true, year: 1988, folder: "bmw".into() };
        let mut m = Msg::default();
        m.set_str(1, "content\\cars\\ks_modded_car\\presets\\preset_modded_car_mech_1.mechanicalcarpreset");
        m.set_str(2, "modded_brand");
        let mut f = Files::default();
        f.put("content\\cars\\ks_modded_car\\generated\\thumbnails\\Ks_Modded_Car-preset_Modded_Car_Mech_1.texture", m.encode());
        let out = clone_donor(f, &ident);
        let (p, d) = out.map.iter().next().unwrap();
        assert_eq!(p, "content\\cars\\ks_bmw_m3_e30_x1y2z\\generated\\thumbnails\\ks_bmw_m3_e30_x1y2z-preset_bmw_m3_e30_x1y2z_Mech_1.texture");
        let r = Msg::parse(d).unwrap();
        assert_eq!(r.str(1).unwrap(), "content\\cars\\ks_bmw_m3_e30_x1y2z\\presets\\preset_bmw_m3_e30_x1y2z_mech_1.mechanicalcarpreset");
        assert_eq!(r.str(2).unwrap(), "bmw");
    }
}
