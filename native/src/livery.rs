//! The player's garage and car liveries - ported from livery.py. Each saved car is a CarFinalStateWithConsumable record decoded
//! with the game's own schema; its multilayer_states map sits several levels
//! down, so it is found by walking rather than by a fixed path.

use std::path::Path;

use prost_reflect::{DynamicMessage, ReflectMessage, Value as PV};
use serde_json::{json, Map, Value};

use crate::{detect, protos};

const RECORD: &str = "CarFinalStateWithConsumable";
const EXT: &str = ".carfinalstatewithconsumable";

/// <Saved Games>\ACE\ProfileData\<profile>\OpenData\SavedCars, or "".
pub fn saved_cars_dir() -> String {
    // dev/test: point the garage at a copy
    if let Ok(t) = std::env::var("ACECM_SAVED_CARS") {
        return if Path::new(&t).is_dir() { t } else { String::new() };
    }
    let root = Path::new(&detect::saved_games()).join("ACE").join("ProfileData");
    let Ok(rd) = std::fs::read_dir(&root) else { return String::new() };
    let mut names: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    for n in names {
        let d = root.join(&n).join("OpenData").join("SavedCars");
        if d.is_dir() {
            return d.to_string_lossy().into_owned();
        }
    }
    String::new()
}

/// Every sub-message, depth-first (set fields only, like ListFields).
fn walk<'a>(m: &'a DynamicMessage, depth: usize, out: &mut Vec<DynamicMessage>) {
    if depth > 6 {
        return;
    }
    out.push(m.clone());
    for (_f, v) in m.fields() {
        match v {
            PV::Message(sub) => walk(sub, depth + 1, out),
            PV::List(items) => for it in items { if let PV::Message(s) = it { walk(s, depth + 1, out) } },
            PV::Map(mp) => for it in mp.values() { if let PV::Message(s) = it { walk(s, depth + 1, out) } },
            _ => {}
        }
    }
}

fn slots(rec: &DynamicMessage) -> Map<String, Value> {
    let mut all = vec![];
    walk(rec, 0, &mut all);
    for holder in all {
        let Some(fd) = holder.descriptor().get_field_by_name("multilayer_states") else { continue };
        if !holder.has_field(&fd) {
            continue;
        }
        if let PV::Map(states) = holder.get_field(&fd).into_owned() {
            if states.is_empty() {
                continue;
            }
            let mut out = Map::new();
            for (k, st) in states {
                let key = match k { prost_reflect::MapKey::String(s) => s, other => format!("{other:?}") };
                out.insert(key, json!(current_color(&st)));
            }
            return out;
        }
    }
    Map::new()
}

fn current_color(st: &PV) -> String {
    let PV::Message(m) = st else { return String::new() };
    let Some(ch) = m.get_field_by_name("primary_channel_state") else { return String::new() };
    let PV::Message(ch) = ch.into_owned() else { return String::new() };
    match ch.get_field_by_name("oem_color_path").map(|v| v.into_owned()) {
        Some(PV::String(s)) => s,
        _ => String::new(),
    }
}

/// <model>_<uuid> -> (model, uuid)
fn split_name(stem: &str) -> (String, String) {
    // the uuid is the last underscore-free token, shaped 8-4-4-4-12 hex
    if let Some((model, inst)) = stem.rsplit_once('_') {
        let g: Vec<&str> = inst.split('-').collect();
        let shape = [8, 4, 4, 4, 12];
        if g.len() == 5 && g.iter().zip(shape)
            .all(|(s, n)| s.len() == n && s.chars().all(|c| c.is_ascii_hexdigit()))
        {
            return (model.to_string(), inst.to_string());
        }
    }
    (stem.to_string(), String::new())
}

/// Every owned car: model, instance id, and the colour of each slot.
pub fn garage() -> Vec<Value> {
    let d = saved_cars_dir();
    let mut out = vec![];
    if d.is_empty() {
        return out;
    }
    let mut names: Vec<String> = std::fs::read_dir(&d).map(|rd| {
        rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()
    }).unwrap_or_default();
    names.sort();
    for name in names {
        let Some(stem) = name.strip_suffix(EXT) else { continue };
        let path = Path::new(&d).join(&name);
        let (model, inst) = split_name(stem);
        let mut entry = json!({"model": model, "instance": inst,
                               "file": path.to_string_lossy(), "slots": {}});
        match std::fs::read(&path).map_err(|e| e.to_string()).and_then(|raw| protos::decode(RECORD, &raw)) {
            Ok(rec) => entry["slots"] = Value::Object(slots(&rec)),
            Err(e) => entry["error"] = json!(e),
        }
        out.push(entry);
    }
    out
}

// ------------------------------------------------------------- colours --
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

fn entries_of(pkg: &str) -> Vec<crate::kspkg::Entry> {
    crate::kspkg::entries(Path::new(pkg)).unwrap_or_default()
}

fn norm_path(p: &str) -> String {
    p.to_lowercase().replace('/', "\\")
}

fn varint(buf: &[u8], mut i: usize) -> (Option<u64>, usize) {
    let (mut shift, mut val) = (0u32, 0u64);
    while i < buf.len() {
        let b = buf[i];
        i += 1;
        val |= ((b & 0x7f) as u64) << shift;
        if b & 0x80 == 0 {
            return (Some(val), i);
        }
        shift += 7;
        if shift > 63 {
            break;
        }
    }
    (None, i)
}

/// Name, rgb and material scalars out of a .oemmultilayercolor (walked by
/// field number: a missing value must stay missing).
pub fn color_from_blob(blob: &[u8], path: &str) -> Value {
    let mut out = json!({"path": path, "name": "", "rgb": null});
    let mut fields: HashMap<u64, f64> = HashMap::new();
    let mut i = 0;
    while i < blob.len() {
        let (Some(tag), ni) = varint(blob, i) else { break };
        i = ni;
        let (num, wire) = (tag >> 3, tag & 7);
        match wire {
            2 => {
                let (Some(ln), ni) = varint(blob, i) else { break };
                i = ni;
                let Some(end) = i.checked_add(ln as usize).filter(|e| *e <= blob.len()) else { break };
                let _ = end;
                let chunk = &blob[i..i + ln as usize];
                i += ln as usize;
                if num == 2 {
                    out["name"] = json!(String::from_utf8_lossy(chunk));
                } else if num == 3 {
                    let mut j = 0;
                    while j < chunk.len() {
                        let t = chunk[j];
                        if t & 7 != 5 || j + 5 > chunk.len() {
                            break;
                        }
                        fields.insert((t >> 3) as u64, f32::from_le_bytes(chunk[j + 1..j + 5].try_into().unwrap()) as f64);
                        j += 5;
                    }
                }
            }
            5 => {
                if i + 4 > blob.len() {
                    break;
                }
                // ⚠ material params live at TOP level, beside the colour
                fields.entry(num).or_insert(f32::from_le_bytes(blob[i..i + 4].try_into().unwrap()) as f64);
                i += 4;
            }
            1 => i += 8,
            0 => { let (_, ni) = varint(blob, i); i = ni; }
            _ => break,
        }
    }
    // ⚠ a zero component is OMITTED by proto3: missing means 0.0
    if [1, 2, 3].iter().any(|k| fields.contains_key(k)) {
        out["rgb"] = json!([fields.get(&1).copied().unwrap_or(0.0), fields.get(&2).copied().unwrap_or(0.0), fields.get(&3).copied().unwrap_or(0.0)]);
    }
    for (num, key) in [(4, "metalness"), (5, "roughness"), (6, "clear_coat"), (7, "normal_intensity")] {
        if let Some(v) = fields.get(&num) {
            out[key] = json!(v);
        }
    }
    out
}

/// One entry from whichever package actually holds it (a modded car's
/// paints live in ITS package).
fn read_from_packages(want: &str, pkg: Option<&str>) -> Option<Vec<u8>> {
    let mut order: Vec<String> = Vec::new();
    if let Some(p) = pkg {
        order.push(p.to_string());
    }
    let parts: Vec<&str> = want.split('\\').collect();
    if parts.len() > 2 && parts[0] == "content" && parts[1] == "cars" {
        if let Some(own) = crate::viewer::package_for(parts[2]) {
            order.push(own);
        }
    }
    if let Some(b) = crate::viewer::package() {
        order.push(b);
    }
    order.extend(crate::viewer::packages().into_iter().map(|(_, p)| p));
    let mut tried: Vec<String> = Vec::new();
    for pk in order {
        if pk.is_empty() || tried.contains(&pk) || !Path::new(&pk).is_file() {
            continue;
        }
        tried.push(pk.clone());
        if let Some(e) = entries_of(&pk).into_iter().find(|e| norm_path(&e.path) == want) {
            if let Ok(b) = std::fs::File::open(&pk).and_then(|mut f| crate::kspkg::read_entry(&mut f, &e)) {
                return Some(b);
            }
        }
    }
    None
}

pub fn color_info(path: &str) -> Value {
    match read_from_packages(&norm_path(path), None) {
        Some(b) => color_from_blob(&b, path),
        None => json!({}),
    }
}

static NAMES: Mutex<Option<Map<String, Value>>> = Mutex::new(None);

fn names_path() -> PathBuf {
    crate::paths::data().join("livery_names.json")
}

fn names_key(model: &str) -> String {
    let mut bits: Vec<String> = Vec::new();
    for pk in [crate::viewer::package_for(model), crate::viewer::package()].into_iter().flatten() {
        if let Ok(m) = std::fs::metadata(&pk) {
            let ns = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0);
            let base = Path::new(&pk).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            bits.push(format!("{base}:{ns}:{}", m.len()));
        }
    }
    bits.sort();
    bits.dedup();
    bits.join("|")
}

/// path -> display name, cached on disk per model (walking the base index
/// costs seconds; this runs whenever a car is selected).
pub fn color_names(model: &str, paths: &[String]) -> Value {
    let want: HashMap<String, String> = paths.iter().map(|p| (p.to_lowercase().replace('/', "\\"), p.clone())).collect();
    let mut out = Map::new();
    if want.is_empty() {
        return Value::Object(out);
    }
    let ck = names_key(model);
    {
        let mut g = NAMES.lock().unwrap();
        let names = g.get_or_insert_with(|| std::fs::read(names_path()).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| if let Value::Object(m) = v { Some(m) } else { None }).unwrap_or_default());
        if let Some(c) = names.get(model) {
            if c.get("key").and_then(|v| v.as_str()) == Some(ck.as_str()) {
                if let Some(got) = c.get("names").and_then(|v| v.as_object()) {
                    if want.values().all(|p| got.contains_key(p)) {
                        return Value::Object(got.clone());
                    }
                }
            }
        }
    }
    let mut order: Vec<String> = Vec::new();
    order.extend(crate::viewer::package_for(model));
    order.extend(crate::viewer::package());
    order.extend(crate::viewer::packages().into_iter().map(|(_, p)| p));
    let mut seen: Vec<String> = Vec::new();
    for pk in order {
        if pk.is_empty() || seen.contains(&pk) || !Path::new(&pk).is_file() || out.len() == want.len() {
            continue;
        }
        seen.push(pk.clone());
        let Ok(mut f) = std::fs::File::open(&pk) else { continue };
        for e in entries_of(&pk) {
            let low = norm_path(&e.path);
            let Some(orig) = want.get(&low) else { continue };
            if out.contains_key(orig) {
                continue;
            }
            if let Ok(b) = crate::kspkg::read_entry(&mut f, &e) {
                let info = color_from_blob(&b, "");
                let n = info.get("name").and_then(|v| v.as_str()).unwrap_or("");
                if !n.is_empty() {
                    out.insert(orig.clone(), json!(n));
                }
            }
        }
    }
    if !out.is_empty() {
        let mut g = NAMES.lock().unwrap();
        let names = g.get_or_insert_with(Map::new);
        names.insert(model.to_string(), json!({"key": ck, "names": out.clone()}));
        let _ = std::fs::write(names_path(), crate::profiles::py_dumps(&Value::Object(names.clone())));
    }
    Value::Object(out)
}

fn s_field(m: &DynamicMessage, f: &str) -> String {
    match m.get_field_by_name(f).map(|v| v.into_owned()) { Some(PV::String(s)) => s, _ => String::new() }
}

/// Every design this car ships, decoded as DesignData (never scraped: a
/// half-working scraper would offer colours the car forbids, which crashes
/// the game). From the car's OWN package.
pub fn designs(model: &str) -> Vec<Value> {
    let Some(pkg) = crate::viewer::package_for(model).or_else(crate::viewer::package) else { return Vec::new() };
    let prefix = format!("content\\cars\\{model}\\skins\\").to_lowercase();
    let Ok(mut f) = std::fs::File::open(&pkg) else { return Vec::new() };
    let mut out = Vec::new();
    for e in entries_of(&pkg) {
        let low = e.path.to_lowercase();
        if !norm_path(&e.path).starts_with(&prefix) || !low.ends_with(".design") {
            continue;
        }
        let Ok(blob) = crate::kspkg::read_entry(&mut f, &e) else { continue };
        let Some(desc) = protos::find("DesignData") else { break };
        let Ok(msg) = DynamicMessage::decode(desc, blob.as_slice()) else {
            crate::logs::info(format!("livery: {} did not decode", e.path));
            continue;
        };
        let mut slots = Map::new();
        if let Some(PV::List(cms)) = msg.get_field_by_name("customizable_materials").map(|v| v.into_owned()) {
            for cm in cms {
                let PV::Message(cm) = cm else { continue };
                let colours: Vec<String> = match cm.get_field_by_name("multilayer").map(|v| v.into_owned()) {
                    Some(PV::Message(ml)) => match ml.get_field_by_name("oem_color_paths").map(|v| v.into_owned()) {
                        Some(PV::List(l)) => l.into_iter().filter_map(|x| if let PV::String(s) = x { Some(s) } else { None }).collect(),
                        _ => Vec::new(),
                    },
                    _ => Vec::new(),
                };
                let key = { let d = s_field(&cm, "description"); if d.is_empty() { s_field(&cm, "slot") } else { d } };
                slots.insert(key, json!({"slot": s_field(&cm, "slot"), "material": s_field(&cm, "material_replacement_path"), "colors": colours}));
            }
        }
        let name = { let d = s_field(&msg, "description"); if d.is_empty() { e.path.rsplit('\\').next().unwrap_or("").to_string() } else { d } };
        out.push(json!({"path": e.path, "name": name, "slots": slots}));
    }
    out
}

/// Slot -> [colour paths] this CAR permits, across all its designs.
pub fn allowed(model: &str) -> Value {
    let mut out = Map::new();
    for d in designs(model) {
        for (slot, info) in d["slots"].as_object().cloned().unwrap_or_default() {
            let have = out.entry(slot).or_insert(json!([]));
            for c in info["colors"].as_array().cloned().unwrap_or_default() {
                if !have.as_array().unwrap().contains(&c) {
                    have.as_array_mut().unwrap().push(c);
                }
            }
        }
    }
    Value::Object(out)
}

// --------------------------------------------------------------- writing --
#[derive(Clone, Debug)]
enum Step {
    Field(u32),
    Idx(u32, usize),
    Key(u32, prost_reflect::MapKey),
}

/// Paths to every message reachable through SET fields (ListFields order).
fn paths(m: &DynamicMessage, depth: usize, cur: &mut Vec<Step>, out: &mut Vec<Vec<Step>>) {
    if depth > 6 {
        return;
    }
    out.push(cur.clone());
    for (fd, v) in m.fields() {
        let n = fd.number();
        match v {
            PV::Message(sub) => { cur.push(Step::Field(n)); paths(sub, depth + 1, cur, out); cur.pop(); }
            PV::List(items) => for (i, it) in items.iter().enumerate() {
                if let PV::Message(s) = it { cur.push(Step::Idx(n, i)); paths(s, depth + 1, cur, out); cur.pop(); }
            },
            PV::Map(mp) => {
                let mut keys: Vec<&prost_reflect::MapKey> = mp.keys().collect();
                keys.sort();
                for k in keys {
                    if let PV::Message(s) = &mp[k] { cur.push(Step::Key(n, k.clone())); paths(s, depth + 1, cur, out); cur.pop(); }
                }
            }
            _ => {}
        }
    }
}

fn at<'a>(m: &'a mut DynamicMessage, path: &[Step]) -> Option<&'a mut DynamicMessage> {
    let Some((first, rest)) = path.split_first() else { return Some(m) };
    let next = match first {
        Step::Field(n) => match m.get_field_by_number_mut(*n)? { PV::Message(s) => s, _ => return None },
        Step::Idx(n, i) => match m.get_field_by_number_mut(*n)? { PV::List(l) => match l.get_mut(*i)? { PV::Message(s) => s, _ => return None }, _ => return None },
        Step::Key(n, k) => match m.get_field_by_number_mut(*n)? { PV::Map(mp) => match mp.get_mut(k)? { PV::Message(s) => s, _ => return None }, _ => return None },
    };
    at(next, rest)
}

fn new_uuid() -> [u8; 16] {
    // dev/test: a fixed id so a write can be byte-compared
    if let Ok(t) = std::env::var("ACECM_TEST_UUID") {
        let h: String = t.chars().filter(|c| c.is_ascii_hexdigit()).collect();
        if h.len() == 32 {
            let mut b = [0u8; 16];
            for i in 0..16 {
                b[i] = u8::from_str_radix(&h[i * 2..i * 2 + 2], 16).unwrap_or(0);
            }
            return b;
        }
    }
    let mut b = [0u8; 16];
    let _ = getrandom::fill(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    b
}

fn uuid_str(b: &[u8; 16]) -> String {
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

/// Write a uuid everywhere the record keeps one: the guid string and the
/// same 128 bits as two big-endian u64s, which must not disagree.
fn set_guid(rec: &mut DynamicMessage, val: &[u8; 16]) {
    let hi = u64::from_be_bytes(val[..8].try_into().unwrap());
    let lo = u64::from_be_bytes(val[8..].try_into().unwrap());
    let mut all = Vec::new();
    paths(rec, 0, &mut Vec::new(), &mut all);
    for p in all {
        let Some(holder) = at(rec, &p) else { continue };
        let d = holder.descriptor();
        if let Some(fd) = d.get_field_by_name("guid") {
            if let PV::String(s) = holder.get_field(&fd).into_owned() {
                if !s.is_empty() {
                    holder.set_field(&fd, PV::String(uuid_str(val)));
                }
            }
        }
        for name in ["id", "uid", "hash"] {
            let Some(fd) = d.get_field_by_name(name) else { continue };
            let Some(md) = fd.kind().as_message().cloned() else { continue };
            let (Some(fa), Some(fb)) = (md.get_field_by_name("a"), md.get_field_by_name("b")) else { continue };
            // ⚠ like python: touching it creates it when absent
            if let PV::Message(sub) = holder.get_field_mut(&fd) {
                sub.set_field(&fa, PV::U64(hi));
                sub.set_field(&fb, PV::U64(lo));
            }
        }
    }
}

fn load(path: &str) -> Result<DynamicMessage, String> {
    let raw = std::fs::read(path).map_err(|e| e.to_string())?;
    protos::decode(RECORD, &raw)
}

/// Set one slot's colour, writing a channel state that AGREES with it.
/// Is `file` a saved-car record inside the garage folder? (the route takes
/// the path from the request)
fn in_garage(file: &str) -> bool {
    let d = saved_cars_dir();
    if d.is_empty() || !file.to_lowercase().ends_with(&EXT.to_lowercase()) {
        return false;
    }
    match (std::fs::canonicalize(&d), std::fs::canonicalize(file)) {
        (Ok(root), Ok(f)) => f.parent() == Some(root.as_path()),
        _ => false,
    }
}

pub fn apply_color(car_file: &str, slot: &str, color_path: &str, dry_run: bool) -> Value {
    use prost::Message;
    if !in_garage(car_file) {
        return json!({"ok": false, "error": "that is not a car in your garage"});
    }
    let info = color_info(color_path);
    let Some(rgb) = info.get("rgb").and_then(|v| v.as_array()).cloned() else {
        return json!({"ok": false, "error": format!("could not read {color_path}")});
    };
    let mut rec = match load(car_file) { Ok(r) => r, Err(e) => return json!({"ok": false, "error": e}) };
    let mut all = Vec::new();
    paths(&rec, 0, &mut Vec::new(), &mut all);
    let mut holder_path = None;
    for p in &all {
        let h = at(&mut rec, p).unwrap();
        if let Some(fd) = h.descriptor().get_field_by_name("multilayer_states") {
            if h.has_field(&fd) {
                if let PV::Map(m) = h.get_field(&fd).into_owned() {
                    if !m.is_empty() {
                        holder_path = Some((p.clone(), fd.number()));
                        break;
                    }
                }
            }
        }
    }
    let Some((hp, num)) = holder_path else {
        return json!({"ok": false, "error": format!("this car has no slot called {slot:?}"), "slots": []});
    };
    {
        let h = at(&mut rec, &hp).unwrap();
        let PV::Map(states) = h.get_field_by_number_mut(num).unwrap() else { unreachable!() };
        let key = prost_reflect::MapKey::String(slot.to_string());
        if !states.contains_key(&key) {
            let mut names: Vec<String> = states.keys().map(|k| match k { prost_reflect::MapKey::String(s) => s.clone(), o => format!("{o:?}") }).collect();
            names.sort();
            let py = format!("'{}'", slot.replace('\'', "\\'"));
            return json!({"ok": false, "error": format!("this car has no slot called {py}"), "slots": names});
        }
        let PV::Message(st) = states.get_mut(&key).unwrap() else { return json!({"ok": false, "error": "bad slot"}) };
        let PV::Message(ch) = st.get_field_by_name_mut("primary_channel_state").unwrap() else { return json!({"ok": false, "error": "bad slot"}) };
        ch.set_field_by_name("oem_color_path", PV::String(color_path.to_string()));
        if let Some(PV::Message(c)) = ch.get_field_by_name_mut("color") {
            for (i, k) in ["x", "y", "z"].iter().enumerate() {
                let v = rgb.get(i).and_then(|v| v.as_f64()).unwrap_or(0.0);
                if let Some(fd) = c.descriptor().get_field_by_name(k) {
                    let pv = if fd.kind() == prost_reflect::Kind::Double { PV::F64(v) } else { PV::F32(v as f32) };
                    c.set_field(&fd, pv);
                }
            }
        }
        for key in ["metalness", "roughness", "clear_coat", "normal_intensity"] {
            let Some(fd) = ch.descriptor().get_field_by_name(key) else { continue };
            match info.get(key).and_then(|v| v.as_f64()) {
                Some(v) => {
                    let pv = if fd.kind() == prost_reflect::Kind::Double { PV::F64(v) } else { PV::F32(v as f32) };
                    ch.set_field(&fd, pv);
                }
                None => {
                    if fd.supports_presence() && ch.has_field(&fd) {
                        ch.clear_field(&fd);
                    }
                }
            }
        }
    }
    let new = new_uuid();
    set_guid(&mut rec, &new);
    let blob = rec.encode_to_vec();
    if dry_run {
        return json!({"ok": true, "dry_run": true, "bytes": blob.len(), "color": info, "guid": uuid_str(&new)});
    }
    let bak = format!("{car_file}.bak_livery");
    if let Err(e) = crate::safefs::backup_once(Path::new(car_file), Path::new(&bak)) {
        return json!({"ok": false, "error": format!("could not back the car up first ({e}) - nothing was changed")});
    }
    if let Err(e) = crate::safefs::write_atomic(Path::new(car_file), &blob) {
        return json!({"ok": false, "error": e.to_string()});
    }
    let base = |p: &str| Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    crate::logs::info(format!("livery: {} -> {}", base(car_file), base(color_path)));
    json!({"ok": true, "bytes": blob.len(), "color": info, "guid": uuid_str(&new), "backup": bak})
}

/// model -> the .carfinalstate files the game ships for it.
fn states_in_package(pkg: &str) -> std::collections::BTreeMap<String, Vec<crate::kspkg::Entry>> {
    let mut out: std::collections::BTreeMap<String, Vec<crate::kspkg::Entry>> = Default::default();
    for e in entries_of(pkg) {
        let low = norm_path(&e.path);
        if !low.ends_with(".carfinalstate") {
            continue;
        }
        let parts: Vec<&str> = low.split('\\').collect();
        if parts.len() > 2 && parts[0] == "content" && parts[1] == "cars" {
            out.entry(parts[2].to_string()).or_default().push(e);
        }
    }
    for v in out.values_mut() {
        v.sort_by(|a, b| a.path.cmp(&b.path));
    }
    out
}

/// Give every car in the game a saved car (from the game's own shipped
/// state), so its liveries can be picked. Existing files are never touched.
pub fn populate(dry_run: bool) -> Value {
    use prost::Message;
    let d = saved_cars_dir();
    if d.is_empty() {
        return json!({"ok": false, "error": "no SavedCars folder - launch the game once so it creates your profile"});
    }
    let mut pkgs: Vec<String> = Vec::new();
    for (_, p) in crate::viewer::packages() {
        if !p.is_empty() && !pkgs.contains(&p) && Path::new(&p).is_file() {
            pkgs.push(p);
        }
    }
    if pkgs.is_empty() {
        return json!({"ok": false, "error": "content.kspkg not found"});
    }
    let have: std::collections::BTreeSet<String> = garage().iter().filter_map(|e| e.get("model").and_then(|v| v.as_str()).map(String::from)).collect();
    let mut src_of: std::collections::BTreeMap<String, (String, crate::kspkg::Entry)> = Default::default();
    for pk in &pkgs {
        for (model, list) in states_in_package(pk) {
            if !have.contains(&model) && !src_of.contains_key(&model) {
                if let Some(e) = list.into_iter().next() {
                    src_of.insert(model, (pk.clone(), e));
                }
            }
        }
    }
    let todo: Vec<String> = src_of.keys().cloned().collect();
    if dry_run {
        return json!({"ok": true, "dry_run": true, "would_add": todo, "already": have, "count": todo.len()});
    }
    let (mut made, mut failed) = (Vec::new(), Vec::new());
    for model in &todo {
        let (pk, e) = &src_of[model];
        let r = (|| -> Result<(), String> {
            let blob = std::fs::File::open(pk).and_then(|mut f| crate::kspkg::read_entry(&mut f, e)).map_err(|e| e.to_string())?;
            let data_desc = protos::find("CarFinalStateData").ok_or("CarFinalStateData schema missing")?;
            let data = DynamicMessage::decode(data_desc, blob.as_slice()).map_err(|_| "shipped state did not decode cleanly".to_string())?;
            let mut rec = DynamicMessage::new(protos::find(RECORD).ok_or("schema missing")?);
            rec.set_field_by_name("final_state", PV::Message(data));
            let new = new_uuid();
            set_guid(&mut rec, &new);
            let out = rec.encode_to_vec();
            // never write something we cannot read back
            protos::decode(RECORD, &out).map_err(|_| "record did not round-trip".to_string())?;
            let name = format!("{model}_{}{EXT}", uuid_str(&new));
            crate::safefs::write_atomic(&Path::new(&d).join(&name), &out).map_err(|e| e.to_string())?;
            remember_populated(&name, true);
            Ok(())
        })();
        match r {
            Ok(()) => made.push(model.clone()),
            Err(e) => {
                crate::logs::warn(format!("livery: could not add {model}: {e}"));
                failed.push(json!({"model": model, "error": e}));
            }
        }
    }
    crate::logs::info(format!("livery: added {} saved car(s)", made.len()));
    json!({"ok": true, "added": made, "failed": failed, "count": made.len(), "already": have.len()})
}

fn populated_path() -> std::path::PathBuf {
    crate::paths::data().join("livery_populated.json")
}

/// The garage files populate() wrote (names only), or None for a garage
/// populated before the list existed.
fn populated() -> Option<Vec<String>> {
    std::fs::read(populated_path()).ok().and_then(|b| serde_json::from_slice::<Vec<String>>(&b).ok())
}

fn remember_populated(name: &str, add: bool) {
    let mut v = populated().unwrap_or_default();
    v.retain(|n| n != name);
    if add {
        v.push(name.to_string());
    }
    let _ = crate::safefs::write_atomic(&populated_path(), &serde_json::to_vec_pretty(&v).unwrap_or_default());
}

/// Remove ONLY records populate() created that still have no mileage (a
/// car you have driven survives).
/// ⚠ Stricter than the Python version, which judged by mileage alone: a
/// car you OWN but never drove was deleted too, and a record whose fields
/// it could not find (a game update renaming them) counted as undriven and
/// was deleted. Now: only files on populate()'s list (and still undriven);
/// with no list at all, nothing is removed.
pub fn depopulate() -> Value {
    if saved_cars_dir().is_empty() {
        return json!({"ok": false, "error": "no SavedCars folder"});
    }
    let (mut gone, mut kept) = (Vec::new(), Vec::new());
    // ⚠ No list = an older version did the populating, and there is no
    // telling its records from cars the player owns but never drove: the
    // mileage rule alone deleted ~100 of those from a test garage. Refuse.
    let Some(listed) = populated() else {
        return json!({"ok": false, "error": "ACECM has no record of which cars it added (an older version added them), so nothing was removed - your garage is unchanged"});
    };
    let listed = Some(listed);
    for e in garage() {
        let file = e.get("file").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let model = e.get("model").cloned().unwrap_or(Value::Null);
        let name = Path::new(&file).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let ours = match &listed {
            Some(l) => l.contains(&name),
            None => !Path::new(&format!("{file}.bak_livery")).exists(),
        };
        if !ours {
            kept.push(model);
            continue;
        }
        let driven = match load(&file) {
            Ok(rec) => match rec.get_field_by_name("car_consumable_status").map(|v| v.into_owned()) {
                Some(PV::Message(cs)) => match cs.get_field_by_name("body").map(|v| v.into_owned()) {
                    Some(PV::Message(b)) => match b.get_field_by_name("hundred_meters").map(|v| v.into_owned()) {
                        Some(PV::U32(n)) => n != 0, Some(PV::I32(n)) => n != 0, Some(PV::U64(n)) => n != 0, Some(PV::I64(n)) => n != 0,
                        Some(PV::F32(n)) => n != 0.0, Some(PV::F64(n)) => n != 0.0, _ => true,
                    },
                    _ => true,
                },
                _ => true,
            },
            Err(_) => true,
        };
        if driven {
            kept.push(model);
            continue;
        }
        match std::fs::remove_file(&file) {
            Ok(()) => {
                remember_populated(&name, false);
                gone.push(model)
            }
            Err(ex) => crate::logs::warn(format!("livery: removing {model}: {ex}")),
        }
    }
    json!({"ok": true, "removed": gone, "kept": kept})
}
