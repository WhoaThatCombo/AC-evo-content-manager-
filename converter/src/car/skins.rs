//! AC1 skins -> EVO liveries, colours and localisation.
//!
//! Stock model (ks_porsche_935, a race car with painted liveries): a livery
//! is a `.design` whose customisable slot (the section name, e.g. EXT_SKIN)
//! replaces the section's material with a livery material under
//! `skins\<design>\`, a `.cardesign` naming the design and the compatible
//! rims, a `.visualcarpreset` (design + material states), and a
//! `.carfinalstate` for the mechanical preset + that visual preset; the
//! mechanical preset lists every visual preset.
//!
//! Here, in AC1 skin folder order (the default skin first):
//! - livery i: every section whose textures the skin resolves differently
//!   from the default skin gets its own material (the default skin's are the
//!   meshes' own materials); a slot per such section in design i;
//! - a representative colour per livery (ours): the mean linear colour of
//!   the livery's baked paint albedo (for the single-colour Kunos street
//!   cars that is the paint), as an `.oemmultilayercolor` the visual preset
//!   shows;
//! - "Single Colour": the donor's layered paint material on the main paint
//!   section, its paint layer masked to where the AC1 diffuse alpha shows
//!   the detail colour, OEM colours = the liveries' colours (stock cars
//!   offer a single-colour variant);
//! - the donor's own OEM paints (the Abarth's) are removed, with every
//!   reference to them (caliper and rim colours too);
//! - en.cars.loc names every preset, design, colour and slot.
//!
//! Localisation keys (read off stock en.cars.loc against the stock files):
//! presets `<car id>_<slug of the preset description>`, design slots
//! `<car id>_<slot>`, designs and colours `<slug of the description>`.

use anyhow::{anyhow, Result};

use std::path::PathBuf;

use crate::ac1::car::Car;
use crate::ac1::kn5::Kn5;
use crate::car::identity::Identity;
use crate::car::materials::{self, Kind, MatIn, Templates, Tex};
use crate::files::Files;
use crate::io::pb::{Msg, Val};

/// A metallic finish, from the skin's name.
pub fn is_metallic(skin: &str) -> bool {
    let s = skin.to_lowercase();
    ["metal", "mica", "pearl", "_met", " met"].iter().any(|k| s.contains(k)) || s.ends_with("met")
}

pub struct Skin {
    pub folder: String,
    pub dir: PathBuf,
    pub display: String,
}

pub fn list(car: &Car) -> Vec<Skin> {
    car.skins.iter().map(|f| {
        let dir = car.skin_dir(f);
        let display = crate::ac1::car::find_ci(&dir, "ui_skin.json").and_then(|p| std::fs::read(p).ok())
            .and_then(|b| crate::ac1::car::ui_field(&String::from_utf8_lossy(&b), "skinname"))
            .unwrap_or_else(|| pretty(f));
        Skin { folder: f.clone(), dir, display }
    }).collect()
}

fn pretty(f: &str) -> String {
    let s: String = f.trim_start_matches(|c: char| c.is_ascii_digit() || c == '_').replace('_', " ");
    let s = if s.trim().is_empty() { f.replace('_', " ") } else { s };
    s.split_whitespace().map(|w| {
        let mut c = w.chars();
        c.next().map(|x| x.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
    }).collect::<Vec<_>>().join(" ")
}

/// Which texture sources a material resolves to under a skin.
/// The content of every texture a material resolves to under a skin (the
/// skin's file or the kn5's): many mods copy every texture into every skin,
/// so a skin's own file does not mean a different look.
fn signature(m: &crate::ac1::kn5::Material, skin: &Skin, tex: &Tex, cache: &mut std::collections::HashMap<String, u64>) -> Vec<u64> {
    m.textures.iter().map(|(_, t)| {
        let key = match crate::ac1::car::find_ci(&skin.dir, t) {
            Some(p) => p.to_string_lossy().to_lowercase(),
            None => format!("kn5:{}", t.to_lowercase()),
        };
        if let Some(h) = cache.get(&key) {
            return *h;
        }
        let h = tex.source(t, Some(&skin.dir)).map(|(b, _)| materials::h64(&b)).unwrap_or(0);
        cache.insert(key, h);
        h
    }).collect()
}

/// `img` scaled to fit a `size` square, centred on transparent.
fn letterbox(img: &crate::io::texture::Image, size: u32) -> crate::io::texture::Image {
    let s = size as f32 / img.w.max(img.h) as f32;
    let (w, h) = (((img.w as f32 * s) as u32).clamp(1, size), ((img.h as f32 * s) as u32).clamp(1, size));
    let mut px = vec![0u8; (size * size * 4) as usize];
    let (ox, oy) = ((size - w) / 2, (size - h) / 2);
    for y in 0..h {
        for x in 0..w {
            let sx = ((x as f32 / s) as u32).min(img.w - 1);
            let sy = ((y as f32 / s) as u32).min(img.h - 1);
            let si = ((sy * img.w + sx) * 4) as usize;
            let di = (((y + oy) * size + x + ox) * 4) as usize;
            px[di..di + 4].copy_from_slice(&img.px[si..si + 4]);
        }
    }
    crate::io::texture::Image { w: size, h: size, px }
}

fn vec3(c: [f32; 3]) -> Msg {
    let mut m = Msg::default();
    for (i, x) in c.iter().enumerate() {
        if *x != 0.0 {
            m.set_f32(i as u32 + 1, *x);
        }
    }
    m
}

fn bytes(s: &str) -> Val {
    Val::Bytes(s.as_bytes().to_vec())
}

/// Slot description as stock writes it ("EXT_SKIN" -> "EXT SKIN").
fn slot_desc(s: &str) -> String {
    s.replace('_', " ")
}

fn read(files: &Files, p: &str) -> Result<Msg> {
    let k = files.find_ci(p).ok_or_else(|| anyhow!("the donor has no {p}"))?;
    Msg::parse(files.get(&k).unwrap())
}

/// A multilayer material state {material, primary {color, oem, emissive}, secondary, tertiary}.
fn ml_state(material: &str, oem: Option<&str>) -> Msg {
    let mut st = Msg::default();
    st.set_str(1, material);
    // channel state: f1 colour, f10 OEM colour, f20 emissive (stock order)
    let mut prim = Msg::default();
    prim.push(1, Val::Msg(Msg::default()));
    if let Some(o) = oem {
        prim.set_str(10, o);
    }
    prim.push(20, Val::Msg(Msg::default()));
    st.push(2, Val::Msg(prim));
    let mut sec = Msg::default();
    sec.push(1, Val::Msg(Msg::default()));
    sec.push(20, Val::Msg(Msg::default()));
    st.push(3, Val::Msg(sec.clone()));
    st.push(4, Val::Msg(sec));
    st
}

/// A design slot {slot, description, material, multilayer {primary {mode, pickers}, secondary, tertiary, oem paths}}.
fn design_slot(slot: &str, material: &str, pickers: bool, oems: &[String]) -> Msg {
    let mut s = Msg::default();
    s.set_str(1, slot);
    s.set_str(2, &slot_desc(slot));
    s.set_str(3, material);
    let mut ml = Msg::default();
    let mut prim = Msg::default();
    if pickers {
        // ChannelCustomization_OemAndPickers (2), with the stock picker set
        prim.set_varint(1, 2);
        for f in [11u32, 12, 13, 14, 15, 16] {
            prim.set_varint(f, 1);
        }
    } else {
        // ChannelCustomization_OemOnly (1)
        prim.set_varint(1, 1);
    }
    ml.push(1, Val::Msg(prim));
    ml.push(2, Val::Msg(Msg::default()));
    ml.push(3, Val::Msg(Msg::default()));
    for o in oems {
        ml.push(4, bytes(o));
    }
    s.push(6, Val::Msg(ml));
    s
}

pub struct Section<'a> {
    pub name: &'a str,
    pub kind: Kind,
    pub material: usize,
    pub interior: bool,
    pub path: &'a str,
}

pub struct Out {
    pub notes: Vec<String>,
    pub liveries: usize,
}

/// Write the liveries. `sections`: what the body / wheel stages wrote;
/// `paint_tmpl`: the donor's layered paint material (before the body stage
/// replaced it).
#[allow(clippy::too_many_arguments)]
pub fn write(files: &mut Files, ident: &Identity, car: &Car, k: &Kn5, sections: &[Section], tex: &mut Tex, t: &Templates, paint_tmpl: Option<Msg>) -> Result<Out> {
    let base = ident.base();
    let (id, stem) = (ident.id.as_str(), ident.stem.as_str());
    let mut notes = Vec::new();
    let skins = list(car);
    let mut loc: Vec<(String, String)> = Vec::new();
    // donor files (renamed)
    let design1 = format!("{base}\\skins\\design_{stem}_1.design");
    let cardesign1 = format!("{base}\\skins\\design_{stem}_1.cardesign");
    let visual1 = format!("{base}\\presets\\preset_{stem}_visual_1.visualcarpreset");
    let mech = format!("{base}\\presets\\{}.mechanicalcarpreset", ident.mech_preset());
    let fs1 = format!("{base}\\presets\\{id}_preset_{stem}_mech_1_preset_{stem}_visual_1.carfinalstate");
    let rimdesign = format!("{base}\\skins\\rimdesign_{stem}_1.design");
    let compat_rim = files.under(&format!("{base}\\skins")).into_iter().find(|p| p.to_lowercase().ends_with(".compatiblerim")).unwrap_or_default();
    let vis_tmpl = read(files, &visual1)?;
    let fs_tmpl = read(files, &fs1)?;
    let design_tmpl = read(files, &design1)?;
    let oem_dir = format!("{base}\\customization\\oem_mod");
    let oem_tmpl = files.under(&oem_dir).into_iter().find(|p| p.contains("_paint_")).and_then(|p| files.get(&p).and_then(|b| Msg::parse(b).ok())).unwrap_or_default();
    // the donor's own colours go
    for p in files.under(&oem_dir) {
        files.remove(&p);
    }
    let paint_secs: Vec<&Section> = sections.iter().filter(|s| s.kind == Kind::Paint).collect();
    let skinnable: Vec<&Section> = sections.iter().filter(|s| s.kind != Kind::Mirror && !s.path.contains("common_assets")).collect();
    let default = skins.first();
    let mut visuals: Vec<String> = Vec::new();
    let mut colours: Vec<String> = Vec::new();
    let n_liv = skins.len().max(1);
    let mut sig_cache = std::collections::HashMap::new();
    for li in 0..n_liv {
        let i = li + 1;
        let skin = skins.get(li);
        let skin_name = skin.map(|s| s.display.clone()).unwrap_or_else(|| "Standard".into());
        // sections this livery changes
        // (slot, material, paint?)
        let mut slots: Vec<(String, String, bool)> = Vec::new();
        for s in &skinnable {
            let m = &k.materials[s.material];
            let differs = match (skin, default) {
                (Some(sk), Some(d)) if li > 0 => signature(m, sk, tex, &mut sig_cache) != signature(m, d, tex, &mut sig_cache),
                _ => false,
            };
            if li == 0 || !differs {
                if s.kind == Kind::Paint {
                    slots.push((s.name.to_string(), s.path.to_string(), true));
                }
                continue;
            }
            let sk = skin.unwrap();
            let inp = MatIn { kind: s.kind, m, interior: s.interior, skin: Some((sk.folder.as_str(), sk.dir.clone())), metallic: is_metallic(&sk.folder) || is_metallic(&sk.display) };
            let mm = materials::build(files, tex, t, &inp);
            let p = format!("{base}\\skins\\{}\\{}.material", materials::slug(&sk.folder), s.name);
            files.put(&p, mm.encode());
            slots.push((s.name.to_string(), p, s.kind == Kind::Paint));
        }
        // paint first: the visual preset's first state carries the colour
        slots.sort_by_key(|x| !x.2);
        // the livery's colour
        let colour = paint_secs.first().and_then(|ps| {
            let m = &k.materials[ps.material];
            let sk = skin.map(|s| (s.folder.as_str(), s.dir.as_path()));
            tex.albedo(files, m, sk, true).map(|(_, c)| c)
        }).unwrap_or([0.3, 0.3, 0.3]);
        let oem_path = format!("{oem_dir}\\{stem}_paint_{i}.oemmultilayercolor");
        let mut oem = oem_tmpl.clone();
        oem.set_str(2, &format!("{stem} paint {i}"));
        oem.set(3, Val::Msg(vec3(colour)));
        let metal = skin.map(|s| is_metallic(&s.folder) || is_metallic(&s.display)).unwrap_or(false);
        oem.set_f32(4, if metal { 0.85 } else { 0.1 });
        files.put(&oem_path, oem.encode());
        colours.push(oem_path.clone());
        loc.push((format!("{stem}_paint_{i}"), skin_name.clone()));
        // design / cardesign / visual / final state
        let design = format!("{base}\\skins\\design_{stem}_{i}.design");
        let cardesign = format!("{base}\\skins\\design_{stem}_{i}.cardesign");
        let visual = format!("{base}\\presets\\preset_{stem}_visual_{i}.visualcarpreset");
        let fs = format!("{base}\\presets\\{id}_preset_{stem}_mech_1_preset_{stem}_visual_{i}.carfinalstate");
        let mut d = design_tmpl.clone();
        d.set_str(2, &format!("{stem} design {i}"));
        d.remove(3);
        for (slot, mat, paint) in &slots {
            let oems = if *paint { std::slice::from_ref(&oem_path) } else { &[] };
            d.push(3, Val::Msg(design_slot(slot, mat, false, oems)));
        }
        files.put(&design, d.encode());
        let mut cd = Msg::default();
        cd.set_str(1, &design);
        if !compat_rim.is_empty() {
            cd.set_str(2, &compat_rim);
        }
        files.put(&cardesign, cd.encode());
        let mut v = vis_tmpl.clone();
        v.set_str(2, &format!("{stem} visual {i}"));
        v.set_str(3, &skin_name);
        v.set_str(5, &cardesign);
        v.remove(8);
        let mut states = Vec::new();
        for (_, mat, paint) in &slots {
            states.push(ml_state(mat, if *paint { Some(&oem_path) } else { None }));
        }
        for st in states {
            v.push(8, Val::Msg(st));
        }
        files.put(&visual, v.encode());
        let mut f = fs_tmpl.clone();
        {
            let body = f.msg_mut(10)?;
            body.set_str(2, &visual);
            body.remove(3);
            for (slot, mat, paint) in &slots {
                let mut e = Msg::default();
                e.set_str(1, &slot_desc(slot));
                e.push(2, Val::Msg(ml_state(mat, if *paint { Some(&oem_path) } else { None })));
                body.push(3, Val::Msg(e));
            }
            // keep field order: 1, 2, 3.., 100, 101
            body.fields.sort_by_key(|x| x.num);
        }
        files.put(&fs, f.encode());
        visuals.push(visual);
        loc.push((format!("{id}_{stem}_visual_{i}"), skin_name.clone()));
        loc.push((format!("{stem}_design_{i}"), skin_name.clone()));
    }
    // Single Colour
    if let (Some(ps), Some(mut pm)) = (paint_secs.first(), paint_tmpl) {
        let i = n_liv + 1;
        let m = &k.materials[ps.material];
        let dname = m.texture("txDiffuse").unwrap_or("").to_string();
        let sk = default.map(|s| (s.folder.as_str(), s.dir.as_path()));
        // base layer: the diffuse as AC1 shows it where alpha is 1; paint
        // layer (Red) where the alpha lets the detail colour through
        if let Some((img, _, _)) = tex.image(&dname, sk.map(|s| s.1)) {
            let img = crate::io::texture::resize_to(&img, 2048, true);
            let px: Vec<u8> = img.px.chunks_exact(4).flat_map(|c| [255 - c[3], 0, 0, 255]).collect();
            let mask = crate::io::texture::Image { w: img.w, h: img.h, px };
            let mp = format!("{base}\\texture\\{}_paintmask.texture", materials::slug(&dname));
            let e = crate::io::texture::encode(&mask, crate::io::texture::Fmt::Bc1D, true, "paintmask.png");
            files.put(&mp, e.header);
            files.put(&format!("{mp}mips"), e.mips);
            crate::io::material::set_slot(&mut pm, "SpatMap", &mp);
            if let Some(p) = tex.get(files, &dname, materials::Role::Color, sk) {
                crate::io::material::set_slot(&mut pm, "Base_BaseColorMap", &p);
                crate::io::material::set_prop(&mut pm, "Base_HasBaseColorMap", &[1.0]);
            }
        }
        for (n, v) in [("Splatmap_UseConstant", &[0.0][..]), ("Splatmap_Constant", &[0.0, 0.0, 0.0, 0.0]), ("Green_Enable", &[0.0]), ("Blue_Enable", &[0.0]),
            ("Base_HasAmbientOcclusionMap", &[0.0]), ("Base_Basecolor", &[1.0, 1.0, 1.0, 0.0])] {
            crate::io::material::set_prop(&mut pm, n, v);
        }
        crate::io::material::set_slot(&mut pm, "Base_AmbientOcclusionMap", "");
        let single_mat = format!("{base}\\materials\\{}_SINGLE.material", ps.name);
        files.put(&single_mat, pm.encode());
        let design = format!("{base}\\skins\\design_{stem}_{i}.design");
        let cardesign = format!("{base}\\skins\\design_{stem}_{i}.cardesign");
        let visual = format!("{base}\\presets\\preset_{stem}_visual_{i}.visualcarpreset");
        let fs = format!("{base}\\presets\\{id}_preset_{stem}_mech_1_preset_{stem}_visual_{i}.carfinalstate");
        let mut d = design_tmpl.clone();
        d.set_str(2, &format!("{stem} design {i}"));
        d.remove(3);
        d.push(3, Val::Msg(design_slot(ps.name, &single_mat, true, &colours)));
        files.put(&design, d.encode());
        let mut cd = Msg::default();
        cd.set_str(1, &design);
        if !compat_rim.is_empty() {
            cd.set_str(2, &compat_rim);
        }
        files.put(&cardesign, cd.encode());
        let mut v = vis_tmpl.clone();
        v.set_str(2, &format!("{stem} visual {i}"));
        v.set_str(3, "Single Colour");
        v.set_str(5, &cardesign);
        v.remove(8);
        v.push(8, Val::Msg(ml_state(&single_mat, colours.first().map(|s| s.as_str()))));
        files.put(&visual, v.encode());
        let mut f = fs_tmpl.clone();
        {
            let body = f.msg_mut(10)?;
            body.set_str(2, &visual);
            body.remove(3);
            let mut e = Msg::default();
            e.set_str(1, &slot_desc(ps.name));
            e.push(2, Val::Msg(ml_state(&single_mat, colours.first().map(|s| s.as_str()))));
            body.push(3, Val::Msg(e));
            body.fields.sort_by_key(|x| x.num);
        }
        files.put(&fs, f.encode());
        visuals.push(visual);
        loc.push((format!("{id}_{stem}_visual_{i}"), "Single Colour".into()));
        loc.push((format!("{stem}_design_{i}"), "Single Colour".into()));
    }
    let _ = cardesign1;
    // menu thumbnails: stock cars carry one per final state,
    // generated\thumbnails\<id>-<mech preset>-<visual preset>.texture,
    // 1024 x 1024 BC7; ours from the AC1 skin's preview.jpg, centred
    let tdir = format!("{base}\\generated\\thumbnails");
    for p in files.under(&tdir) {
        if !p.ends_with("\\.texture") && !p.ends_with("\\.texturemips") {
            files.remove(&p);
        }
    }
    for (vi, v) in visuals.iter().enumerate() {
        let skin = skins.get(vi).or(default);
        let Some(img) = skin.and_then(|s| crate::ac1::car::find_ci(&s.dir, "preview.jpg")).and_then(|p| std::fs::read(p).ok()).and_then(|b| crate::io::texture::decode_any(&b).ok()) else { continue };
        let vstem = v.rsplit('\\').next().unwrap_or(v).trim_end_matches(".visualcarpreset");
        let tp = format!("{tdir}\\{id}-{}-{vstem}.texture", ident.mech_preset());
        let e = crate::io::texture::encode(&letterbox(&img, 1024), crate::io::texture::Fmt::Bc7, true, "thumbnail.png");
        files.put(&tp, e.header);
        files.put(&format!("{tp}mips"), e.mips);
    }
    // the mechanical preset lists every visual
    let mut mp = read(files, &mech)?;
    mp.remove(8);
    let rest: Vec<_> = mp.fields.drain(..).collect();
    let mut placed = false;
    for f in rest {
        if !placed && f.num > 8 {
            for v in &visuals {
                mp.push(8, bytes(v));
            }
            placed = true;
        }
        mp.fields.push(f);
    }
    if !placed {
        for v in &visuals {
            mp.push(8, bytes(v));
        }
    }
    files.put(&mech, mp.encode());
    // rim design / final states: no donor rim colour
    if let Some(key) = files.find_ci(&rimdesign) {
        let mut d = Msg::parse(files.get(&key).unwrap())?;
        for idx in d.idx(3) {
            let s = d.open(idx)?;
            let slot = s.str(1).unwrap_or_default();
            if let Some(mi) = s.idx(6).first().copied() {
                let ml = s.open(mi)?;
                ml.remove(4);
                // the converted rim wears the AC1 rim's own maps on its base
                // layer: no channel recolours it (mode 0, ChannelCustomization_None;
                // the donor's EXT_RIM offered OEM colours + pickers, mode 2).
                // The blur slots keep inheriting from EXT_RIM (mode 3).
                if slot == "EXT_RIM" || slot == "EXT_RIM_DECAL" {
                    for ch in [1u32, 2, 3] {
                        if let Some(ci) = ml.idx(ch).first().copied() {
                            ml.open(ci)?.remove(1);
                        }
                    }
                }
            }
        }
        files.put(&key, d.encode());
    }
    for p in files.under(&format!("{base}\\presets")) {
        if !p.to_lowercase().ends_with(".carfinalstate") {
            continue;
        }
        let mut f = Msg::parse(files.get(&p).unwrap())?;
        if let Some(ri) = f.idx(20).first().copied() {
            let rims = f.open(ri)?;
            for e in rims.idx(3) {
                let ent = rims.open(e)?;
                if let Some(vi) = ent.idx(2).first().copied() {
                    let st = ent.open(vi)?;
                    for ch in [2u32, 3, 4] {
                        for c in st.idx(ch) {
                            st.open(c)?.remove(10);
                        }
                    }
                }
            }
        }
        files.put(&p, f.encode());
    }
    // localisation
    let lp = format!("{base}\\uiresources\\localization\\en.cars.loc");
    if let Some(key) = files.find_ci(&lp) {
        let t = String::from_utf8_lossy(files.get(&key).unwrap()).into_owned();
        let mut rows: Vec<(String, String)> = t.lines().filter_map(|l| l.split_once('=')).map(|(a, b)| (a.trim().to_string(), b.trim().to_string()))
            .filter(|(k, _)| {
                let kl = k.to_lowercase();
                // donor colours, caliper and its stock-keyed rim go
                !(kl.starts_with(&format!("{stem}_paint_")) || kl.starts_with(&format!("{stem}_caliper")) || kl.starts_with(&format!("{stem}_rim_")) || kl.starts_with("ultraleggera") || kl == "design_1"
                    || kl.starts_with(&format!("{id}_{stem}_visual_")) || kl == format!("{id}_ext_caliper"))
            }).collect();
        for s in sections.iter().filter(|s| s.kind == Kind::Paint) {
            let n = s.name.strip_prefix("EXT_SKIN").map(|r| r.trim_start_matches('_').to_string()).unwrap_or_default();
            rows.push((format!("{id}_{}", s.name), if n.is_empty() { "Body Colour".into() } else { format!("Body Colour {n}") }));
        }
        rows.extend(loc);
        rows.push((crate::ids::slug(&format!("{} Wheel", ident.display)), format!("{} Wheel", ident.display)));
        rows.sort_by_key(|(k, _)| k.to_lowercase());
        rows.dedup_by(|a, b| a.0.eq_ignore_ascii_case(&b.0));
        files.put(&key, rows.iter().map(|(k, v)| format!("{k}={v}\r\n")).collect::<String>().into_bytes());
    }
    notes.push(format!("skins: {} liveries{}", n_liv, if paint_secs.is_empty() { " (no paint section: no single-colour variant)" } else { " + single colour" }));
    Ok(Out { notes, liveries: visuals.len() })
}
