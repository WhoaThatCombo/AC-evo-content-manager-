//! The digital dash (requirements car-lights-leds-display.md): AC1
//! `digital_instruments.ini` items -> an EVO display page in the AC1
//! screen's place.
//!
//! # What the game wants (stock cars and the SDK sample)
//!
//! - `CarActorData.displays` (f11): {f1 html page, f2 display material,
//!   f4 / f5 page size, f9 display enum, f10 custom `.cardisplaysystem`}.
//!   The page renders into a texture the display material shows over the
//!   section's UVs: u to the right, v down (the sample's INT_DISPLAY: u = 0
//!   on the driver's left edge, v = 0 on the top edge).
//! - Pages are HTML + CSS + JS (Coherent Gameface): the shared scripts in
//!   `content\cars\common_assets\displays\js`, values bound with
//!   `data-bind-value="{{ModelCurrentCar.<field>}}"` (`display_speed_kmh`,
//!   `gear`, `rpm`, `water_temperature_c`, ...), the body driven by
//!   `CarDisplay.process({{ModelCurrentCar}})` from `getNewDisplay` (dark
//!   while the ignition is off: `body[data-page=off]` in common.css). The
//!   game's own fonts are in `common_assets\displays\fonts`.
//!
//! # From AC1
//!
//! `[ITEM_n]` PARENT (a `DISPLAY_DATA*` dummy), POSITION (in the dummy's
//! frame), TYPE (GEAR, SPEED, RPM, RPM_GRAPH, CLOCK, WATER_TEMP, FUEL,
//! TURBO, LAPTIME, BEST_LAP, CURRENT_LAP, TC / ABS_LEVEL, PLACE_HOLDER +
//! TEXT...), SIZE (character height, m), COLOR, FONT, ALIGN. The dummy's
//! XY plane is the screen and its +Z points away from the driver (ks_porsche_
//! 911_gt3_rs DISPLAY_DATA_0: x = car left, z = forward, tilted 13 deg), so
//! the driver's left-to-right is the dummy's -x: the page's u runs along -x,
//! its v along -y.
//!
//! # Ours (the page design)
//!
//! One page per dummy laid out over all its items; the items at their AC1
//! places and sizes in the game's digital font (digital-7) or Roboto
//! Condensed on a dark background; RPM_GRAPH as a bar from its AC1 textures;
//! the rpm readout turns red from 95 % of the car's own limiter. The page is
//! shown on one quad per cluster of items (items closer than 0.8 character
//! heights), 1.5 mm in front of the AC1 screen, so the AC1 dials between
//! clusters stay visible.

use anyhow::Result;

use crate::ac1::car::Car;
use crate::car::frame::{self, Frame, M4};
use crate::car::fx;
use crate::files::Files;
use crate::io::pb::{Field, Msg, Val};

pub struct Item {
    pub kind: String,
    /// position in the dummy's frame (m)
    pub pos: [f64; 3],
    pub size: f64,
    pub colour: [f32; 3],
    pub font: String,
    pub align: String,
    pub text: String,
    /// RPM_GRAPH
    pub wh: [f64; 2],
    pub rpm_max: f64,
    pub textures: [Option<String>; 2],
}

pub struct Screen {
    pub parent: String,
    /// the dummy's world (EVO space, datum applied)
    pub world: M4,
    pub items: Vec<Item>,
}

pub struct DisplayPlan {
    pub screens: Vec<Screen>,
    pub template: Option<Msg>,
    pub limiter: f64,
    pub notes: Vec<String>,
}

const SUPPORTED: &[&str] = &["GEAR", "SPEED", "RPM", "RPM_GRAPH", "CLOCK", "WATER_TEMP", "FUEL", "TURBO", "PLACE_HOLDER", "AMBIENT_TEMP", "OIL_TEMP",
    "LAPTIME", "BEST_LAP", "CURRENT_LAP", "TC_LEVEL", "ABS_LEVEL"];

pub fn prepare(car: &Car, f: &Frame, files: &Files, id: &str) -> DisplayPlan {
    let mut p = DisplayPlan {
        screens: Vec::new(),
        template: files.get(&format!("content\\cars\\{id}\\materials\\INT_DISPLAY.material")).and_then(|b| Msg::parse(b).ok()),
        limiter: car.ini("engine.ini").num("ENGINE_DATA", "LIMITER").unwrap_or(7000.0),
        notes: Vec::new(),
    };
    let di = car.ini("digital_instruments.ini");
    let mut skipped = 0;
    for (sec, kv) in di.sections.iter() {
        if !sec.starts_with("ITEM_") {
            continue;
        }
        let kind = kv.get("TYPE").map(|s| s.trim().to_uppercase()).unwrap_or_default();
        if !SUPPORTED.contains(&kind.as_str()) {
            skipped += 1;
            continue;
        }
        let Some(parent) = kv.get("PARENT").map(|s| s.trim().to_string()) else { continue };
        let Some(w) = f.node_world.iter().find(|(n, _)| n.eq_ignore_ascii_case(&parent)).map(|(_, m)| f.evo_m(m)) else {
            p.notes.push(format!("display: {sec} parent {parent} is not in the kn5"));
            continue;
        };
        let v = kv.get("POSITION").map(|s| crate::ac1::ini::nums_of(s)).unwrap_or_default();
        let num = |k: &str, d: f64| kv.get(k).and_then(|s| crate::ac1::ini::num_of(s)).unwrap_or(d);
        let col = fx::rgb(kv.get("COLOR"));
        let col = if col == [0.0; 3] { [1.0; 3] } else { fx::hue(col) };
        let tex = |k: &str| kv.get(k).and_then(|n| crate::ac1::car::find_ci(&car.dir.join("texture"), n.trim()).or_else(|| crate::ac1::car::find_ci(&car.dir, n.trim()))).map(|p| p.to_string_lossy().into_owned());
        let item = Item {
            kind: kind.clone(),
            pos: [v.first().copied().unwrap_or(0.0), v.get(1).copied().unwrap_or(0.0), v.get(2).copied().unwrap_or(0.0)],
            size: num("SIZE", 0.01).abs().max(0.002),
            colour: col,
            font: kv.get("FONT").cloned().unwrap_or_default().to_lowercase(),
            align: kv.get("ALIGN").map(|s| s.trim().to_uppercase()).unwrap_or_else(|| "LEFT".into()),
            text: kv.get("TEXT").cloned().unwrap_or_default(),
            wh: [num("WIDTH", 0.1).abs(), num("HEIGHT", 0.02).abs()],
            rpm_max: num("RPM_MAX", p.limiter),
            textures: [tex("TEXTURE_BASE"), tex("TEXTURE_TOP")],
        };
        match p.screens.iter_mut().find(|s| s.parent.eq_ignore_ascii_case(&parent)) {
            Some(s) => s.items.push(item),
            None => p.screens.push(Screen { parent, world: w, items: vec![item] }),
        }
    }
    p.screens.truncate(4);
    p.notes.push(format!("display: {} screen(s), {} item(s){}", p.screens.len(), p.screens.iter().map(|s| s.items.len()).sum::<usize>(),
        if skipped > 0 { format!(", {skipped} item(s) of other types left out") } else { String::new() }));
    p
}

/// Rough width (m) of an item: characters x 0.6 x height.
fn item_width(it: &Item) -> f64 {
    let chars = match it.kind.as_str() {
        "GEAR" => 1.0,
        "SPEED" | "WATER_TEMP" | "FUEL" | "AMBIENT_TEMP" | "OIL_TEMP" => 3.0,
        "TURBO" => 4.0,
        "RPM" | "CLOCK" => 5.0,
        "LAPTIME" | "BEST_LAP" => 8.0,
        "CURRENT_LAP" | "TC_LEVEL" | "ABS_LEVEL" => 2.0,
        "PLACE_HOLDER" => it.text.chars().count().max(1) as f64,
        _ => 3.0,
    };
    if it.kind == "RPM_GRAPH" { it.wh[0] } else { chars * 0.6 * it.size }
}

/// Item extent in page axes (u = -x, v = -y of the dummy): (u0, u1, v0, v1).
fn extent(it: &Item) -> (f64, f64, f64, f64) {
    let (u, v) = (-it.pos[0], -it.pos[1]);
    if it.kind == "RPM_GRAPH" {
        return (u, u + it.wh[0], v, v + it.wh[1]);
    }
    let w = item_width(it);
    let (a, b) = match it.align.as_str() {
        "CENTER" => (u - w / 2.0, u + w / 2.0),
        "RIGHT" => (u - w, u),
        _ => (u, u + w),
    };
    (a, b, v - it.size / 2.0, v + it.size / 2.0)
}

fn binding(it: &Item, limiter: f64) -> (String, String) {
    let _ = limiter;
    let m = "ModelCurrentCar";
    match it.kind.as_str() {
        "GEAR" => (format!("{{{{{m}.gear}}}}"), "N".into()),
        "SPEED" => (format!("Math.round({{{{{m}.display_speed_kmh}}}})"), "0".into()),
        "RPM" => (format!("Math.round({{{{{m}.rpm}}}})"), "0".into()),
        "WATER_TEMP" => (format!("Math.round({{{{{m}.water_temperature_c}}}})"), "0".into()),
        "OIL_TEMP" => (format!("Math.round({{{{{m}.water_temperature_c}}}})"), "0".into()),
        "AMBIENT_TEMP" => (format!("{{{{{m}.air_temperature_c}}}}.toFixed(0)"), "0".into()),
        "FUEL" => (format!("Math.round({{{{{m}.fuel_liter_current_quantity}}}})"), "0".into()),
        "TURBO" => (format!("{{{{{m}.turbo_boost}}}}.toFixed(2)"), "0.00".into()),
        "CLOCK" => (format!("CarDisplay.clock({{{{{m}.time_of_day_hours}}}}, {{{{{m}.time_of_day_minutes}}}})"), "12:00".into()),
        "LAPTIME" => (format!("CarDisplay.laptime({{{{{m}.current_lap_time_ms}}}})"), "0:00.0".into()),
        "BEST_LAP" => (format!("CarDisplay.laptime({{{{{m}.low_frequency.best_laptime_ms}}}})"), "-:--.-".into()),
        "CURRENT_LAP" => (format!("{{{{{m}.low_frequency.total_lap_count}}}} + 1"), "1".into()),
        "TC_LEVEL" => (format!("{{{{{m}.low_frequency.electronics.tc_level}}}}"), "0".into()),
        "ABS_LEVEL" => (format!("{{{{{m}.low_frequency.electronics.abs_level}}}}"), "0".into()),
        _ => (String::new(), it.text.clone()),
    }
}

fn font_of(name: &str) -> &'static str {
    if ["digital", "led", "lcd", "german", "f2004", "audi", "s2000", "ttcup", "e92"].iter().any(|k| name.contains(k)) {
        "ac1digital"
    } else {
        "ac1text"
    }
}

fn hex(c: [f32; 3]) -> String {
    let b = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(c[0]), b(c[1]), b(c[2]))
}

pub struct Page {
    pub html: String,
    pub css: String,
    pub js: String,
    pub width: u32,
    pub height: u32,
    /// one quad per cluster of items (the page between clusters is not
    /// shown, so the AC1 dials around a digital inset stay visible):
    /// corners in EVO space (top-left, top-right, bottom-right, bottom-left)
    /// and the page rectangle they show (u0, v0, u1, v1 in 0..1)
    pub quads: Vec<([[f64; 3]; 4], [f32; 4])>,
    /// (file name in displays\, source path)
    pub images: Vec<(String, String)>,
}

pub fn page(s: &Screen, idx: usize, limiter: f64) -> Page {
    let (mut u0, mut u1, mut v0, mut v1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for it in &s.items {
        let e = extent(it);
        u0 = u0.min(e.0);
        u1 = u1.max(e.1);
        v0 = v0.min(e.2);
        v1 = v1.max(e.3);
    }
    let pad = ((u1 - u0).max(v1 - v0) * 0.12).max(0.004);
    let (u0, u1, v0, v1) = (u0 - pad, u1 + pad, v0 - pad, v1 + pad);
    let (wm, hm) = ((u1 - u0).max(0.01), (v1 - v0).max(0.005));
    let width = 1024u32;
    let height = (((width as f64 * hm / wm) / 32.0).round() as u32 * 32).clamp(64, 1024);
    let sx = width as f64 / wm;
    let sy = height as f64 / hm;
    // quads: page (u, v) -> dummy local (-u, -v, -1.5 mm towards the driver);
    // items closer than about one character height share a quad
    let corner = |u: f64, v: f64| frame::xform(&s.world, [-u, -v, -0.0015]);
    let ext: Vec<(f64, f64, f64, f64)> = s.items.iter().map(extent).collect();
    let mut group: Vec<usize> = (0..ext.len()).collect();
    fn root(g: &mut Vec<usize>, i: usize) -> usize {
        let mut r = i;
        while g[r] != r {
            r = g[r];
        }
        g[i] = r;
        r
    }
    for a in 0..ext.len() {
        for b in a + 1..ext.len() {
            let gap = s.items[a].size.max(s.items[b].size) * 0.8;
            let (ea, eb) = (ext[a], ext[b]);
            if ea.0 - gap < eb.1 && eb.0 - gap < ea.1 && ea.2 - gap < eb.3 && eb.2 - gap < ea.3 {
                let (ra, rb) = (root(&mut group, a), root(&mut group, b));
                group[ra] = rb;
            }
        }
    }
    let mut quads = Vec::new();
    let mut seen = Vec::new();
    for i in 0..ext.len() {
        let r = root(&mut group, i);
        if seen.contains(&r) {
            continue;
        }
        seen.push(r);
        let (mut a0, mut a1, mut b0, mut b1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
        let mut sz: f64 = 0.0;
        for k in 0..ext.len() {
            if root(&mut group, k) == r {
                a0 = a0.min(ext[k].0);
                a1 = a1.max(ext[k].1);
                b0 = b0.min(ext[k].2);
                b1 = b1.max(ext[k].3);
                sz = sz.max(s.items[k].size);
            }
        }
        let m = sz * 0.3;
        let (a0, a1, b0, b1) = ((a0 - m).max(u0), (a1 + m).min(u1), (b0 - m).max(v0), (b1 + m).min(v1));
        let uv = [((a0 - u0) / wm) as f32, ((b0 - v0) / hm) as f32, ((a1 - u0) / wm) as f32, ((b1 - v0) / hm) as f32];
        quads.push(([corner(a0, b0), corner(a1, b0), corner(a1, b1), corner(a0, b1)], uv));
    }
    let mut css = String::new();
    css.push_str("@font-face { font-family: ac1digital; src: url('../../common_assets/displays/fonts/digital-7_mono.ttf'); }\n");
    css.push_str("@font-face { font-family: ac1text; src: url('../../common_assets/displays/fonts/robotocondensed-bold.ttf'); }\n");
    css.push_str(&format!("body {{ width: {width}px; height: {height}px; margin: 0; overflow: hidden; background-color: #050607; }}\n"));
    css.push_str(".item { position: absolute; white-space: nowrap; line-height: 1; }\n");
    css.push_str("body.ac1-redline .rpm { color: #ff2010 !important; }\n");
    css.push_str(".bar { position: absolute; overflow: hidden; background-size: 100% 100%; }\n");
    let mut html = String::new();
    html.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n\n<head>\n");
    html.push_str("    <script src='../../common_assets/displays/js/cohtml.js'></script>\n");
    html.push_str("    <script src='../../common_assets/displays/js/displays.js'></script>\n\n");
    html.push_str("    <link rel='stylesheet' type='text/css' href='../../common_assets/displays/css/common.css'>\n");
    html.push_str(&format!("    <link rel='stylesheet' type='text/css' href='display_ac1_{idx}.css'>\n\n"));
    html.push_str(&format!("    <script src='display_ac1_{idx}.js'></script>\n</head>\n\n"));
    html.push_str("<body class=\"cardisplay\" data-mode=\"off\"\n    data-bind-car=\"CarDisplay.process({{ModelCurrentCar}})\">\n\n");
    let mut images = Vec::new();
    let mut bars = Vec::new();
    for (k, it) in s.items.iter().enumerate() {
        let e = extent(it);
        let (x, y) = ((e.0 - u0) * sx, (e.2 - v0) * sy);
        if it.kind == "RPM_GRAPH" {
            let (w, h) = (it.wh[0] * sx, it.wh[1] * sy);
            let mut style = format!("left: {x:.0}px; top: {y:.0}px; width: {w:.0}px; height: {h:.0}px;");
            if let Some(b) = &it.textures[0] {
                let n = format!("ac1_{idx}_{k}_base.png");
                style.push_str(&format!(" background-image: url('{n}');"));
                images.push((n, b.clone()));
            } else {
                style.push_str(" background-color: #202020;");
            }
            let top = match &it.textures[1] {
                Some(t) => {
                    let n = format!("ac1_{idx}_{k}_top.png");
                    images.push((n.clone(), t.clone()));
                    format!("background-image: url('{n}'); background-size: {w:.0}px {h:.0}px;")
                }
                None => format!("background-color: {};", hex(it.colour)),
            };
            css.push_str(&format!("#i{k} {{ {style} }}\n#i{k}b {{ position: absolute; left: 0; top: 0; height: 100%; width: 0px; overflow: hidden; {top} }}\n"));
            html.push_str(&format!("    <div id=\"i{k}\" class=\"bar\"><div id=\"i{k}b\"></div></div>\n"));
            bars.push((k, w, it.rpm_max.max(1000.0)));
            continue;
        }
        let fs = it.size * sy;
        let w = (e.1 - e.0) * sx;
        let align = match it.align.as_str() { "CENTER" => "center", "RIGHT" => "right", _ => "left" };
        let cls = if it.kind == "RPM" { "item rpm" } else { "item" };
        css.push_str(&format!("#i{k} {{ left: {x:.0}px; top: {y:.0}px; width: {w:.0}px; font-size: {fs:.0}px; font-family: {}; color: {}; text-align: {align}; }}\n",
            font_of(&it.font), hex(it.colour)));
        let (b, text) = binding(it, limiter);
        if b.is_empty() {
            html.push_str(&format!("    <div id=\"i{k}\" class=\"{cls}\">{}</div>\n", esc(&text)));
        } else {
            html.push_str(&format!("    <div id=\"i{k}\" class=\"{cls}\" data-bind-value=\"{b}\">{}</div>\n", esc(&text)));
        }
    }
    html.push_str("\n</body>\n\n</html>\n");
    let mut js = String::new();
    js.push_str("/// <reference path=\"../../common_assets/displays/js/displays.js\" />\n");
    js.push_str("// AC1 digital instruments as an EVO page (aceconv)\n\n");
    js.push_str(&format!("const AC1_LIMITER = {:.0};\n", limiter));
    js.push_str(&format!("const AC1_BARS = [{}];\n\n", bars.iter().map(|(k, w, m)| format!("[\"i{k}b\", {w:.0}, {m:.0}]")).collect::<Vec<_>>().join(", ")));
    js.push_str("const CarDisplay = {\n\n    ...window.getNewDisplay(\"race\", 800),\n\n    bars: [],\n\n");
    js.push_str("    init: () => {\n        CarDisplay.bars = AC1_BARS.map((b) => [$.q(\"#\" + b[0]), b[1], b[2]]);\n    },\n\n");
    js.push_str("    laptime: (ms) => {\n        if (!(ms > 0)) {\n            return \"-:--.-\";\n        }\n        const t = Math.floor(ms / 100);\n        const s = Math.floor(t / 10) % 60;\n        return Math.floor(t / 600) + \":\" + (s < 10 ? \"0\" : \"\") + s + \".\" + (t % 10);\n    },\n\n");
    js.push_str("    clock: (h, m) => {\n        const p = (x) => (x < 10 ? \"0\" : \"\") + x;\n        return p(h) + \":\" + p(m);\n    },\n\n");
    js.push_str("    process: (cardata) => {\n        if (!COMMON.process(cardata)) {\n            return;\n        }\n");
    js.push_str("        document.body.classList.toggle(\"ac1-redline\", cardata.rpm >= AC1_LIMITER * 0.95);\n");
    js.push_str("        for (const b of CarDisplay.bars) {\n            if (b[0]) {\n                const f = Math.max(0, Math.min(1, cardata.rpm / b[2]));\n                b[0].setAttribute(\"style\", \"width: \" + Math.round(f * b[1]) + \"px\");\n            }\n        }\n    }\n}\n");
    Page { html, css, js, width, height, quads, images }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Append a one-section quad to every LOD of an encoded mesh (skinned to
/// the mesh's root bone). Corners: top-left, top-right, bottom-right,
/// bottom-left; the front face looks towards `eye`.
pub fn append_quad(mesh: &[u8], name: &str, material: &str, quads: &[([[f64; 3]; 4], [f32; 4])], eye: [f64; 3]) -> Result<Vec<u8>> {
    let mut b = mesh.to_vec();
    for (k, (q, uv)) in quads.iter().enumerate() {
        b = append_one(&b, name, material, q, *uv, eye, k == 0)?;
    }
    Ok(b)
}

/// One quad; `new_section`: start the section, else extend the last one.
fn append_one(mesh: &[u8], name: &str, material: &str, q: &[[f64; 3]; 4], r: [f32; 4], eye: [f64; 3], new_section: bool) -> Result<Vec<u8>> {
    let mut top = Msg::parse(mesh)?;
    let sub = |a: [f64; 3], b: [f64; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let cross = |a: [f64; 3], b: [f64; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let norm = |a: [f64; 3]| { let l = dot(a, a).sqrt().max(1e-9); [a[0] / l, a[1] / l, a[2] / l] };
    let c = [(q[0][0] + q[2][0]) / 2.0, (q[0][1] + q[2][1]) / 2.0, (q[0][2] + q[2][2]) / 2.0];
    let mut n = norm(cross(sub(q[1], q[0]), sub(q[3], q[0])));
    // triangles (0, 3, 2), (0, 2, 1): cross(b - a, c - a) along -n of (1-0)x(3-0)
    let mut tris = [[0u32, 3, 2], [0, 2, 1]];
    let tn = norm(cross(sub(q[3], q[0]), sub(q[2], q[0])));
    let to_eye = sub(eye, c);
    if dot(tn, to_eye) < 0.0 {
        tris = [[0, 2, 3], [0, 1, 2]];
    }
    if dot(n, to_eye) < 0.0 {
        n = [-n[0], -n[1], -n[2]];
    }
    let t = norm(sub(q[1], q[0]));
    let uv = [[r[0], r[1]], [r[2], r[1]], [r[2], r[3]], [r[0], r[3]]];
    for li in top.idx(5) {
        let lod = top.open(li)?;
        let mut pos = fx::stream(lod, 5);
        let base = (pos.len() / 3) as u32;
        let mut nrm = fx::stream(lod, 6);
        let mut uvs = fx::stream(lod, 7);
        let mut tan = fx::stream(lod, 8);
        let mut w = fx::stream(lod, 9);
        let mut bi = fx::stream(lod, 10);
        let skinned = !w.is_empty();
        for k in 0..4 {
            pos.extend(q[k].iter().map(|x| *x as f32));
            nrm.extend(n.iter().map(|x| *x as f32));
            uvs.extend(uv[k]);
            tan.extend([t[0] as f32, t[1] as f32, t[2] as f32, 1.0]);
            if skinned {
                w.extend([1.0, 0.0, 0.0, 0.0]);
                bi.extend([0.0, 0.0, 0.0, 0.0]);
            }
        }
        let mut idx: Vec<u32> = lod.bytes(11).map(|b| crate::io::pb::varints(b).into_iter().map(|x| x as u32).collect()).unwrap_or_default();
        let first = idx.len();
        for tr in tris {
            idx.extend(tr.iter().map(|i| base + i));
        }
        fx::set_stream(lod, 5, &pos);
        fx::set_stream(lod, 6, &nrm);
        fx::set_stream(lod, 7, &uvs);
        fx::set_stream(lod, 8, &tan);
        if skinned {
            fx::set_stream(lod, 9, &w);
            fx::set_stream(lod, 10, &bi);
        }
        lod.set(11, Val::Bytes(crate::io::pb::varint_bytes(&idx)));
        for (f, per) in [(13u32, 4usize), (14, 2), (15, 2), (16, 2)] {
            if lod.get(f).is_some() {
                let mut s = fx::stream(lod, f);
                s.extend(std::iter::repeat(0.0).take(per * 4));
                fx::set_stream(lod, f, &s);
            }
        }
        if new_section {
            let mut sec = Msg::default();
            sec.set_str(1, name);
            sec.set_varint(2, first as u64);
            sec.set_varint(3, 6);
            sec.set_str(4, material);
            let at = lod.fields.iter().rposition(|x| x.num == 4).map(|i| i + 1).unwrap_or(0);
            lod.fields.insert(at, Field { num: 4, val: Val::Msg(sec) });
        } else if let Some(j) = lod.fields.iter().rposition(|x| x.num == 4) {
            // our section is the last one and its indices the last ones
            let sec = lod.open(j)?;
            let c = sec.varint(3).unwrap_or(0);
            sec.set_varint(3, c + 6);
        }
    }
    Ok(top.encode())
}

/// Write the pages, the quads, the display materials and the actor's
/// displays; without AC1 items the donor's displays are removed.
pub fn write(files: &mut Files, id: &str, stem: &str, p: &DisplayPlan, eye: [f64; 3]) -> Result<Vec<String>> {
    let base = format!("content\\cars\\{id}");
    let mut notes = p.notes.clone();
    let ap = format!("{base}\\{id}.actor");
    let Some(mut actor) = files.get(&ap).and_then(|b| Msg::parse(b).ok()) else { return Ok(notes) };
    // the donor's pages, scripts and display systems go
    for d in files.under(&format!("{base}\\displays")) {
        files.remove(&d);
    }
    for d in files.under(&format!("{base}\\instruments")) {
        if d.to_lowercase().ends_with(".cardisplaysystem") {
            files.remove(&d);
        }
    }
    let car = actor.msg_mut(103)?;
    car.remove(11);
    let ip = format!("{base}\\meshes\\{stem}_interior.mesh");
    let mut displays = Vec::new();
    for (i, s) in p.screens.iter().enumerate() {
        let pg = page(s, i, p.limiter);
        let html = format!("{base}\\displays\\display_ac1_{i}.html");
        files.put(&html, pg.html.clone().into_bytes());
        files.put(&format!("{base}\\displays\\display_ac1_{i}.css"), pg.css.clone().into_bytes());
        files.put(&format!("{base}\\displays\\display_ac1_{i}.js"), pg.js.clone().into_bytes());
        for (n, src) in &pg.images {
            if let Ok(b) = std::fs::read(src) {
                let png = match crate::io::texture::decode_any(&b) {
                    Ok(img) => {
                        let mut out = Vec::new();
                        let ok = image::RgbaImage::from_raw(img.w, img.h, img.px).map(|im| im.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).is_ok()).unwrap_or(false);
                        if ok { out } else { continue }
                    }
                    Err(_) => continue,
                };
                files.put(&format!("{base}\\displays\\{n}"), png);
            }
        }
        let sec = format!("INT_AC1_DISPLAY_{i}");
        let mat = format!("{base}\\materials\\{sec}.material");
        let m = p.template.clone().unwrap_or_default();
        files.put(&mat, m.encode());
        if let Some(b) = files.get(&ip).cloned() {
            files.put(&ip, append_quad(&b, &sec, &mat, &pg.quads, eye)?);
        }
        let mut sys = Msg::default();
        sys.push(2, Val::Msg(Msg::default()));
        let sp = format!("{base}\\instruments\\{id}_display_{i}.cardisplaysystem");
        files.put(&sp, sys.encode());
        let mut d = Msg::default();
        d.set_str(1, &html);
        d.set_str(2, &mat);
        d.set_varint(4, pg.width as u64);
        d.set_varint(5, pg.height as u64);
        d.set_str(10, &sp);
        displays.push(d);
        notes.push(format!("display {i}: {} ({} item(s)) -> {}x{} page on {} quad(s) [{}]", s.parent, s.items.len(), pg.width, pg.height, pg.quads.len(),
            pg.quads.iter().map(|(q, _)| format!("{:.0} x {:.0} mm", dist(q[0], q[1]) * 1000.0, dist(q[0], q[3]) * 1000.0)).collect::<Vec<_>>().join(", ")));
    }
    if displays.is_empty() {
        notes.push("display: no AC1 digital items - the donor's displays removed".into());
    }
    let at = car.fields.iter().position(|x| x.num > 11 && x.num != 12).unwrap_or(car.fields.len());
    for (k, d) in displays.into_iter().enumerate() {
        car.fields.insert(at + k, Field { num: 11, val: Val::Msg(d) });
    }
    files.put(&ap, actor.encode());
    Ok(notes)
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_binds_speed_gear_rpm() {
        let it = |k: &str, x: f64| Item { kind: k.into(), pos: [x, 0.0, 0.0], size: 0.01, colour: [1.0; 3], font: "digital_big".into(), align: "CENTER".into(), text: String::new(), wh: [0.1, 0.02], rpm_max: 8000.0, textures: [None, None] };
        let s = Screen { parent: "DISPLAY_DATA".into(), world: frame::I4, items: vec![it("GEAR", 0.0), it("SPEED", 0.03), it("RPM", -0.03)] };
        let p = page(&s, 0, 7000.0);
        assert!(p.html.contains("ModelCurrentCar.gear") && p.html.contains("display_speed_kmh") && p.html.contains("ModelCurrentCar.rpm"));
        assert!(p.js.contains("AC1_LIMITER = 7000"));
        // u runs along -x: the item at +x is at the page's left
        assert!(p.quads[0].0[0][0] > p.quads[0].0[1][0]);
    }
}
