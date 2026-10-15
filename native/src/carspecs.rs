//! Headline specs for a car, read out of the game's own physics files -
//! ported from carspecs.py. Every number comes from the files the game
//! drives with (CarData, DrivetrainData, GearBoxData, AiCarData, EngineData
//! and its power curve), plus the per-trim sheet in each mechanical preset.
//!
//! ⚠ POWER IS DERIVED: the curve is pre-boost torque, so peak power is
//! max(Nm x (1 + boost) x rpm / 7120.6), boost capped by the wastegate.
//! ⚠ TOP SPEED from AiCarData is the AI's hint, labelled an estimate.

use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::sync::Mutex;

use prost_reflect::{DynamicMessage, ReflectMessage, Value as PV};
use serde_json::{json, Map, Value};

use crate::kspkg::{self, Entry};

const CACHE_V: i64 = 2;
const NM_RPM_TO_HP: f64 = 9549.0 * 0.7457;

static MEM: Mutex<Option<(String, Value)>> = Mutex::new(None);

fn varint(b: &[u8], mut i: usize) -> Option<(u64, usize)> {
    let (mut r, mut s) = (0u64, 0u32);
    loop {
        let x = *b.get(i)?;
        i += 1;
        r |= ((x & 0x7f) as u64) << s;
        s += 7;
        if x & 0x80 == 0 {
            return Some((r, i));
        }
        if s > 63 {
            return None;
        }
    }
}

enum F<'a> {
    B(&'a [u8]),
    F(f32),
    V,
}

fn fields(b: &[u8]) -> Vec<(u64, F<'_>)> {
    let (mut i, mut out) = (0usize, Vec::new());
    while i < b.len() {
        let Some((k, ni)) = varint(b, i) else { break };
        i = ni;
        match k & 7 {
            2 => {
                let Some((ln, ni)) = varint(b, i) else { break };
                let end = (ni + ln as usize).min(b.len());
                out.push((k >> 3, F::B(&b[ni..end])));
                i = ni + ln as usize;
            }
            5 => {
                if i + 4 > b.len() { break; }
                out.push((k >> 3, F::F(f32::from_le_bytes(b[i..i + 4].try_into().unwrap()))));
                i += 4;
            }
            0 => { let Some((_, ni)) = varint(b, i) else { break }; i = ni; out.push((k >> 3, F::V)); }
            1 => i += 8,
            _ => break,
        }
    }
    out
}

/// (rpm, value) pairs from a .curve - by hand: the value sits in field 10,
/// which the schema's Vector2Data does not declare.
fn curve_points(blob: &[u8]) -> Vec<(f64, f64)> {
    let mut pts = Vec::new();
    for (n, v) in fields(blob) {
        let (1, F::B(sub)) = (n, v) else { continue };
        let mut d: HashMap<u64, f64> = HashMap::new();
        for (fnum, fv) in fields(sub) {
            match fv {
                F::F(x) => { d.insert(fnum, x as f64); }
                _ => { d.remove(&fnum); }
            }
        }
        if let (Some(x), Some(y)) = (d.get(&1), d.get(&10)) {
            pts.push((*x, *y));
        }
    }
    pts
}

fn round(v: f64) -> i64 {
    v.round_ties_even() as i64
}

fn round_n(v: f64, n: i32) -> f64 {
    let m = 10f64.powi(n);
    (v * m).round_ties_even() / m
}

fn num(m: &DynamicMessage, f: &str) -> f64 {
    match m.get_field_by_name(f).map(|v| v.into_owned()) {
        Some(PV::F32(x)) => x as f64, Some(PV::F64(x)) => x,
        Some(PV::I32(x)) => x as f64, Some(PV::U32(x)) => x as f64,
        Some(PV::I64(x)) => x as f64, Some(PV::U64(x)) => x as f64,
        Some(PV::EnumNumber(x)) => x as f64,
        _ => 0.0,
    }
}

fn sub(m: &DynamicMessage, f: &str) -> Option<DynamicMessage> {
    match m.get_field_by_name(f).map(|v| v.into_owned()) { Some(PV::Message(s)) => Some(s), _ => None }
}

fn decode(name: &str, b: &[u8]) -> Option<DynamicMessage> {
    crate::protos::decode_lenient(name, b).ok()
}

fn enum_name(m: &DynamicMessage, f: &str) -> Option<String> {
    let fd = m.descriptor().get_field_by_name(f)?;
    let en = fd.kind().as_enum()?.clone();
    let n = num(m, f) as i32;
    en.get_value(n).map(|v| v.name().to_string())
}

const SPEC_KEYS: [(&str, &str); 10] = [("POWER", "power_hp"), ("TORQUE", "torque_nm"), ("WEIGHT", "mass_kg"),
    ("MAX_SPEED", "top_speed_kmh"), ("ACCELERATION", "accel_s"), ("GEARS", "gears"), ("DISPLACEMENT", "displacement_l"),
    ("CYLINDERS", "cylinders"), ("FUEL_TANK_CAPACITY", "fuel_l"), ("MAX_RPM", "redline_rpm")];

fn unit_fix(name: &str, unit: &str) -> f64 {
    match (name, unit) {
        ("power_hp", "POWER_KILOWATT") => 1.34102,
        ("power_hp", "POWER_HORSEPOWER_METRIC") => 0.98632,
        ("mass_kg", "WEIGHT_POUNDS") => 0.453592,
        ("top_speed_kmh", "SPEED_MILES_PER_HOUR") => 1.60934,
        _ => 1.0,
    }
}

/// CarSpecsData -> our field names, as the GAME states them.
fn spec_sheet(cs: &DynamicMessage) -> Map<String, Value> {
    let mut out = Map::new();
    let Some(PV::List(items)) = cs.get_field_by_name("items").map(|v| v.into_owned()) else { return out };
    for it in items {
        let PV::Message(it) = it else { continue };
        let Some(key) = enum_name(&it, "key") else { continue };
        let key = key.replace("CarSpecs_KeyEnum_", "");
        let Some((_, name)) = SPEC_KEYS.iter().find(|(k, _)| *k == key) else { continue };
        let unit = enum_name(&it, "units").map(|u| u.replace("CarSpecs_UnitsEnum_", "")).unwrap_or_default();
        let val = num(&it, "value") * unit_fix(name, &unit);
        out.insert(name.to_string(), if *name == "displacement_l" || *name == "accel_s" { json!(round_n(val, 1)) } else { json!(round(val)) });
    }
    if !out.is_empty() {
        out.insert("exact".into(), json!(true));
    }
    out
}

fn curve_index(pkg: &str) -> HashMap<String, Entry> {
    let mut out = HashMap::new();
    for e in kspkg::entries(Path::new(pkg)).unwrap_or_default() {
        let low = e.path.to_lowercase().replace('/', "\\");
        if low.ends_with(".curve") {
            out.entry(low.rsplit('\\').next().unwrap_or("").to_string()).or_insert(e);
        }
    }
    out
}

struct Ctx<'a> {
    fh: File,
    base_pkg: Option<&'a str>,
    base_curves: &'a HashMap<String, Entry>,
    base_fh: Option<File>,
}

fn one(cx: &mut Ctx, blobs: &HashMap<&'static str, Entry>, curves: &HashMap<String, Entry>) -> Map<String, Value> {
    let mut spec = Map::new();
    let read = |cx: &mut Ctx, kind: &str| blobs.get(kind).and_then(|e| kspkg::read_entry(&mut cx.fh, e).ok());
    let mut engine: Option<DynamicMessage> = None;
    if let Some(car) = read(cx, ".car").and_then(|b| decode("CarData", &b)) {
        if let Some(g) = sub(&car, "general") {
            let (tm, fuel) = (num(&g, "totalMass"), num(&g, "fuel"));
            if tm != 0.0 { spec.insert("mass_kg".into(), json!(round(tm))); }
            if fuel != 0.0 { spec.insert("fuel_l".into(), json!(round(fuel))); }
            if let Some(PV::String(n)) = g.get_field_by_name("screenName").map(|v| v.into_owned()) {
                if !n.is_empty() { spec.insert("name".into(), json!(n)); }
            }
        } else {
            // python: m.general on an unset field is an empty message
        }
        engine = sub(&car, "engine").or_else(|| car.descriptor().get_field_by_name("engine").and_then(|fd| fd.kind().as_message().map(|m| DynamicMessage::new(m.clone()))));
    }
    if let Some(d) = read(cx, ".drivetrain").and_then(|b| decode("DrivetrainData", &b)) {
        if let Some(v) = enum_name(&d, "tractionType") {
            spec.insert("drive".into(), json!(match v.as_str() { "RWD" => "RWD", "FWD" => "FWD", _ => "AWD" }));
        }
    }
    if let Some(g) = read(cx, ".gearbox").and_then(|b| decode("GearBoxData", &b)) {
        let n = num(&g, "gear_count");
        if n != 0.0 { spec.insert("gears".into(), json!(n as i64)); }
    }
    if let Some(a) = read(cx, ".aicardata").and_then(|b| decode("AiCarData", &b)) {
        let rl = sub(&a, "gears").map(|g| num(&g, "redline_rpm")).unwrap_or(0.0);
        if rl != 0.0 { spec.insert("redline_rpm".into(), json!(round(rl))); }
        let ts = sub(&a, "physics_hints").map(|h| num(&h, "topspeed_kmh")).unwrap_or(0.0);
        if ts != 0.0 { spec.insert("top_speed_kmh_est".into(), json!(round(ts))); }
    }
    if engine.is_none() {
        engine = read(cx, ".carengine").and_then(|b| decode("EngineData", &b));
    }
    if let Some(eng) = engine {
        let pc = match eng.get_field_by_name("powerCurve").map(|v| v.into_owned()) { Some(PV::String(s)) => s, _ => String::new() };
        let name = pc.replace('\\', "/").rsplit('/').next().unwrap_or("").to_lowercase();
        let blob = match curves.get(&name) {
            Some(e) => kspkg::read_entry(&mut cx.fh, e).ok(),
            None => match (cx.base_curves.get(&name), cx.base_pkg) {
                (Some(e), Some(bp)) => {
                    if cx.base_fh.is_none() { cx.base_fh = File::open(bp).ok(); }
                    cx.base_fh.as_mut().and_then(|f| kspkg::read_entry(f, e).ok())
                }
                _ => None,
            },
        };
        if let Some(blob) = blob {
            let pts = curve_points(&blob);
            // ⚠ the WASTEGATE is the real ceiling, not max_boost
            let mut boost: f64 = 0.0;
            if let Some(PV::List(ts)) = eng.get_field_by_name("turbos").map(|v| v.into_owned()) {
                for t in ts {
                    let PV::Message(t) = t else { continue };
                    let mut b = num(&t, "max_boost");
                    let wg = if t.descriptor().get_field_by_name("wastegate").is_some() { num(&t, "wastegate") } else { 0.0 };
                    if wg > 0.0 { b = b.min(wg); }
                    boost = boost.max(b);
                }
            }
            if !pts.is_empty() {
                let mult = 1.0 + boost;
                let hp = pts.iter().map(|(x, y)| y * mult * x / NM_RPM_TO_HP).fold(f64::NEG_INFINITY, f64::max);
                let nm = pts.iter().map(|(_, y)| y * mult).fold(f64::NEG_INFINITY, f64::max);
                if hp > 0.0 {
                    spec.insert("power_hp".into(), json!(round(hp)));
                    spec.insert("torque_nm".into(), json!(round(nm)));
                    spec.insert("boost_bar".into(), if boost != 0.0 { json!(round_n(boost, 2)) } else { json!(0) });
                }
            }
        }
        let lim = num(&eng, "limiter");
        if lim != 0.0 && !spec.get("redline_rpm").map(|v| v.as_i64().unwrap_or(0) != 0).unwrap_or(false) {
            spec.insert("redline_rpm".into(), json!(round(lim)));
        }
    }
    if let (Some(hp), Some(kg)) = (spec.get("power_hp").and_then(|v| v.as_f64()), spec.get("mass_kg").and_then(|v| v.as_f64())) {
        if hp != 0.0 && kg != 0.0 {
            spec.insert("hp_per_tonne".into(), json!(round(hp * 1000.0 / kg)));
        }
    }
    spec
}

const KINDS: [&str; 5] = [".car", ".drivetrain", ".gearbox", ".aicardata", ".carengine"];

fn scan_pkg(pkg: &str, base_pkg: Option<&str>, base_curves: &HashMap<String, Entry>) -> (Vec<(String, Map<String, Value>)>, Vec<(String, Map<String, Value>)>) {
    let mut got: Vec<(String, HashMap<&'static str, Entry>)> = Vec::new();
    let mut curves: HashMap<String, HashMap<String, Entry>> = HashMap::new();
    let mut presets: Vec<(String, Entry)> = Vec::new();
    for e in kspkg::entries(Path::new(pkg)).unwrap_or_default() {
        let low = e.path.to_lowercase().replace('/', "\\");
        let parts: Vec<&str> = low.split('\\').collect();
        if parts.len() < 3 || parts[0] != "content" || parts[1] != "cars" {
            continue;
        }
        let model = parts[2].to_string();
        let leaf = parts[parts.len() - 1].to_string();
        if low.ends_with(".curve") {
            curves.entry(model).or_default().insert(leaf, e);
            continue;
        }
        if low.ends_with(".mechanicalcarpreset") {
            let pid = leaf.rsplit_once('.').map(|(a, _)| a.to_string()).unwrap_or(leaf);
            match presets.iter_mut().find(|(p, _)| *p == pid) { Some(x) => x.1 = e, None => presets.push((pid, e)) }
            continue;
        }
        if let Some(kind) = KINDS.iter().find(|k| low.ends_with(*k)) {
            // first of each kind wins (alternative engines live under parts/)
            let slot = match got.iter_mut().position(|(m, _)| *m == model) {
                Some(i) => i,
                None => { got.push((model.clone(), HashMap::new())); got.len() - 1 }
            };
            got[slot].1.entry(kind).or_insert(e);
        }
    }
    let Ok(fh) = File::open(pkg) else { return (Vec::new(), Vec::new()) };
    let mut cx = Ctx { fh, base_pkg, base_curves, base_fh: None };
    let empty = HashMap::new();
    let mut out = Vec::new();
    for (model, blobs) in &got {
        let spec = one(&mut cx, blobs, curves.get(model).unwrap_or(&empty));
        out.push((model.clone(), spec));
    }
    // ⚠ the game's OWN per-trim spec sheet beats anything derived
    let mut pre = Vec::new();
    for (pid, e) in presets {
        if let Some(m) = kspkg::read_entry(&mut cx.fh, &e).ok().and_then(|b| decode("MechanicalCarPresetData", &b)) {
            if let Some(cs) = sub(&m, "car_specs") {
                let s = spec_sheet(&cs);
                if !s.is_empty() {
                    pre.push((pid, s));
                }
            }
        }
    }
    (out, pre)
}

fn build() -> Value {
    let mut cars = Map::new();
    let mut pre = Map::new();
    let base = crate::viewer::package();
    let base_curves = base.as_deref().filter(|b| Path::new(b).is_file()).map(curve_index).unwrap_or_default();
    let mut seen: Vec<String> = Vec::new();
    for (_, pk) in crate::viewer::packages() {
        if pk.is_empty() || seen.contains(&pk) || !Path::new(&pk).is_file() {
            continue;
        }
        seen.push(pk.clone());
        let (c, p) = scan_pkg(&pk, base.as_deref(), &base_curves);
        for (m, s) in c {
            if !s.is_empty() && !cars.contains_key(&m) {
                cars.insert(m, Value::Object(s));
            }
        }
        for (id, s) in p {
            pre.entry(id).or_insert(Value::Object(s));
        }
    }
    let built = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    json!({"cars": cars, "presets": pre, "built": built, "packages": seen.len(), "v": CACHE_V})
}

fn key() -> String {
    let pkg = crate::viewer::package().unwrap_or_else(|| "None".into());
    match std::fs::metadata(&pkg) {
        Ok(m) => {
            let ns = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0);
            format!("{pkg}|{ns}|{}|{CACHE_V}", m.len())
        }
        Err(_) => format!("{pkg}|{CACHE_V}"),
    }
}

/// The specs table, cached on the base archive in <data>/car_specs.json
/// (the same file and key the backend uses).
pub fn table(refresh: bool) -> Value {
    let k = key();
    if !refresh {
        if let Some((mk, v)) = MEM.lock().unwrap().as_ref() {
            if *mk == k {
                return v.clone();
            }
        }
        let path = crate::paths::data().join("car_specs.json");
        if let Some(c) = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) {
            if c.get("key").and_then(|v| v.as_str()) == Some(k.as_str()) {
                *MEM.lock().unwrap() = Some((k, c.clone()));
                return c;
            }
        }
    }
    let mut data = build();
    data["key"] = json!(k);
    let _ = std::fs::write(crate::paths::data().join("car_specs.json"), crate::profiles::py_dumps(&data));
    *MEM.lock().unwrap() = Some((k, data.clone()));
    data
}

pub fn for_model(model: &str) -> Value {
    if model.is_empty() {
        return json!({});
    }
    let t = table(false);
    let cars = t.get("cars");
    cars.and_then(|c| c.get(model)).filter(|v| v.as_object().map(|o| !o.is_empty()).unwrap_or(false))
        .or_else(|| cars.and_then(|c| c.get(model.to_lowercase())))
        .cloned().unwrap_or(json!({}))
}

/// Specs for a preset: the model's numbers with the TRIM's own sheet on top.
pub fn for_preset(preset_id: &str) -> Value {
    if preset_id.is_empty() {
        return json!({});
    }
    let mut model = crate::carsmap::table(false).get("presets").and_then(|p| p.get(preset_id)).and_then(|v| v.as_str()).unwrap_or("").to_string();
    if model.is_empty() {
        for c in crate::content::cars().get("cars").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
            if c.get("id").and_then(|v| v.as_str()) == Some(preset_id) {
                model = c.get("model").and_then(|v| v.as_str()).unwrap_or("").to_string();
                break;
            }
        }
    }
    let mut base = for_model(if model.is_empty() { preset_id } else { &model }).as_object().cloned().unwrap_or_default();
    let sheet = table(false).get("presets").and_then(|p| p.get(preset_id)).and_then(|v| v.as_object()).cloned().unwrap_or_default();
    if !sheet.is_empty() {
        if sheet.get("top_speed_kmh").map(|v| v.as_f64().unwrap_or(0.0) != 0.0).unwrap_or(false) {
            base.remove("top_speed_kmh_est");
        }
        for (k, v) in sheet {
            base.insert(k, v);
        }
    }
    if let (Some(hp), Some(kg)) = (base.get("power_hp").and_then(|v| v.as_f64()), base.get("mass_kg").and_then(|v| v.as_f64())) {
        if hp != 0.0 && kg != 0.0 {
            base.insert("hp_per_tonne".into(), json!(round(hp * 1000.0 / kg)));
        }
    }
    Value::Object(base)
}
