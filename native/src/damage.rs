//! Car damage for hosted servers - ported from acecm/damage.py.
//!
//! No damage setting exists in the server config or the season blob. The
//! server builds sessions from TEMPLATE season definitions in its own
//! content.kspkg; SeasonDefinition field 50 (assists_preset, a
//! RacingSettingsAssistPreset) is the one place assists are imposed on every
//! car. Its damage_rate (field 14) is OFFSET: shown % = (rate + 1) x 100.
//! We write a NEUTRAL block (every on/off assist FREE, stability open 0..1,
//! only damage imposed); at 100 % no block at all - the factory template
//! byte for byte.
//! ⚠ Global to the install (every server reads the same templates).
//! ⚠ Never overwrite an assists block we did not write.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::{kspkg, kspkg_write, paths, trackdeploy, tracktables::varint};

pub const TEMPLATES: &[&str] = &[
    r"content\data\practice.seasondefinition",
    r"content\data\race_weekend.seasondefinition",
];
const FIELD_ASSISTS: u64 = 50;
const FREE: u64 = 2;
const TYPE_CUSTOM: u64 = 4;
const ENUM_FIELDS: [u64; 10] = [1, 2, 3, 4, 6, 7, 8, 11, 12, 13];
const F_TYPE: u64 = 10;
const F_STAB_MIN: u64 = 16;
const F_STAB_MAX: u64 = 17;
const F_DAMAGE: u64 = 14;

fn factory_dir() -> PathBuf {
    paths::data().join("damage_factory")
}

enum V { Int(u64), Raw(Vec<u8>) }

/// (field, raw bytes of the whole field, value) or an error.
fn fields(d: &[u8]) -> Result<Vec<(u64, Vec<u8>, V)>, String> {
    let rd = |d: &[u8], p: &mut usize| -> Result<u64, String> {
        let (mut r, mut s) = (0u64, 0u32);
        loop {
            let b = *d.get(*p).ok_or("truncated")?;
            *p += 1;
            if s < 64 { r |= ((b & 0x7f) as u64) << s; }
            if b & 0x80 == 0 { return Ok(r); }
            s += 7;
        }
    };
    let mut out = vec![];
    let mut p = 0;
    while p < d.len() {
        let start = p;
        let tag = rd(d, &mut p)?;
        let (f, wt) = (tag >> 3, tag & 7);
        if f == 0 {
            return Err("bad tag".into());
        }
        let v = match wt {
            0 => V::Int(rd(d, &mut p)?),
            1 => { let e = p + 8; let v = d.get(p..e.min(d.len())).unwrap_or(&[]).to_vec(); p = e; V::Raw(v) }
            5 => { let e = p + 4; let v = d.get(p..e.min(d.len())).unwrap_or(&[]).to_vec(); p = e; V::Raw(v) }
            2 => { let n = rd(d, &mut p)? as usize; let e = p.saturating_add(n); let v = d.get(p..e.min(d.len())).unwrap_or(&[]).to_vec(); p = e; V::Raw(v) }
            _ => return Err(format!("unsupported wire type {wt}")),
        };
        if p > d.len() {
            return Err("truncated".into());
        }
        out.push((f, d[start..p].to_vec(), v));
    }
    Ok(out)
}

fn block(pct: i64) -> Vec<u8> {
    let rate = (pct as f64 / 100.0).clamp(0.0, 1.0) - 1.0;
    let mut b = vec![];
    b.extend(varint(F_TYPE << 3));
    b.extend(varint(TYPE_CUSTOM));
    for f in ENUM_FIELDS {
        b.extend(varint(f << 3));
        b.extend(varint(FREE));
    }
    b.extend(varint((F_STAB_MIN << 3) | 5));
    b.extend(0.0f32.to_le_bytes());
    b.extend(varint((F_STAB_MAX << 3) | 5));
    b.extend(1.0f32.to_le_bytes());
    b.extend(varint((F_DAMAGE << 3) | 5));
    b.extend((rate as f32).to_le_bytes());
    b
}

/// Our block -> damage %, a foreign block -> None.
fn parse_block(raw: &[u8]) -> Option<i64> {
    let fs = fields(raw).ok()?;
    let mut m: BTreeMap<u64, V> = BTreeMap::new();
    for (f, _r, v) in fs {
        m.insert(f, v);
    }
    let int = |f: u64| match m.get(&f) { Some(V::Int(n)) => Some(*n), _ => None };
    if int(F_TYPE) != Some(TYPE_CUSTOM) {
        return None;
    }
    if ENUM_FIELDS.iter().any(|f| int(*f) != Some(FREE)) {
        return None;
    }
    let allowed: Vec<u64> = ENUM_FIELDS.iter().copied().chain([F_TYPE, F_STAB_MIN, F_STAB_MAX, F_DAMAGE]).collect();
    if m.keys().any(|k| !allowed.contains(k)) {
        return None;
    }
    let rate = match m.get(&F_DAMAGE) {
        Some(V::Raw(b)) if b.len() == 4 => f32::from_le_bytes(b[..4].try_into().unwrap()) as f64,
        Some(_) => return None,
        None => 0.0,
    };
    Some(((rate + 1.0) * 100.0).round_ties_even() as i64)
}

/// (template without field 50, the field-50 payload or None).
fn split(t: &[u8]) -> Result<(Vec<u8>, Option<Vec<u8>>), String> {
    let mut kept = vec![];
    let mut blk = None;
    for (f, raw, v) in fields(t)? {
        if f == FIELD_ASSISTS {
            if let V::Raw(b) = v { blk = Some(b) }
        } else {
            kept.extend(raw);
        }
    }
    Ok((kept, blk))
}

fn archive() -> Option<PathBuf> {
    let k = trackdeploy::server_kspkg();
    k.is_file().then_some(k)
}

fn read(archive: &Path, path: &str) -> Option<Vec<u8>> {
    let e = kspkg::find_entry(archive, path).ok().flatten()?;
    let mut f = std::fs::File::open(archive).ok()?;
    use std::io::{Read, Seek, SeekFrom};
    f.seek(SeekFrom::Start(e.offset)).ok()?;
    let mut b = vec![0u8; e.size as usize];
    f.read_exact(&mut b).ok()?;
    kspkg::decrypt(&mut b, 0);
    Some(b)
}

/// The untouched template, kept outside the archive (refreshed whenever the
/// archive holds one with NO assists block - including after an update).
fn factory(path: &str, current: &[u8]) -> Option<Vec<u8>> {
    let _ = std::fs::create_dir_all(factory_dir());
    let fp = factory_dir().join(Path::new(path.rsplit('\\').next().unwrap_or(path)));
    let (rest, blk) = split(current).ok()?;
    match blk {
        None => {
            if std::fs::read(&fp).ok().as_deref() != Some(current) {
                let _ = std::fs::write(&fp, current);
            }
            Some(current.to_vec())
        }
        Some(b) if parse_block(&b).is_some() => Some(rest),
        Some(_) => None,
    }
}

pub fn status() -> Value {
    let Some(a) = archive() else { return json!({"ok": false, "error": "server content.kspkg not found"}) };
    let mut t = Map::new();
    let mut pcts = std::collections::BTreeSet::new();
    for path in TEMPLATES {
        let st = match read(&a, path) {
            None => json!({"state": "missing"}),
            Some(cur) => match split(&cur) {
                Err(e) => json!({"state": "unreadable", "error": e}),
                Ok((_r, None)) => json!({"state": "factory", "damage": 100}),
                Ok((_r, Some(b))) => match parse_block(&b) {
                    Some(p) => json!({"state": "ours", "damage": p}),
                    None => json!({"state": "foreign"}),
                },
            },
        };
        if let Some(d) = st.get("damage").and_then(|v| v.as_i64()) {
            pcts.insert(d);
        }
        t.insert(path.to_string(), st);
    }
    let foreign = t.values().any(|s| s["state"] == "foreign");
    let damage = if pcts.len() == 1 { json!(pcts.iter().next().unwrap()) } else { Value::Null };
    json!({"ok": true, "path": a.to_string_lossy(), "templates": t, "foreign": foreign, "damage": damage})
}

/// pct 0..100; 100 restores the factory templates.
pub fn set_damage(pct: f64) -> Value {
    let pct = pct.round_ties_even().clamp(0.0, 100.0) as i64;
    let Some(a) = archive() else { return json!({"ok": false, "error": "server content.kspkg not found"}) };
    if trackdeploy::server_running() {
        return json!({"ok": false, "error": "stop every server first - the server holds content.kspkg open"});
    }
    let _lock = match crate::archlock::lock() { Ok(g) => g, Err(e) => return json!({"ok": false, "error": e}) };
    let mut changes: Vec<(String, Vec<u8>)> = vec![];
    let mut notes = vec![];
    for path in TEMPLATES {
        let Some(cur) = read(&a, path) else {
            notes.push(format!("{path} not in this archive - skipped"));
            continue;
        };
        let blk = match split(&cur) {
            Ok((_r, b)) => b,
            Err(e) => return json!({"ok": false, "error": format!("{path} does not parse ({e}); refusing to write")}),
        };
        if let Some(b) = &blk {
            if parse_block(b).is_none() {
                let base = path.rsplit('\\').next().unwrap_or(path);
                return json!({"ok": false, "foreign": true,
                              "error": format!("{base} already carries an assists block ACECM did not write - leaving it alone")});
            }
        }
        let Some(base) = factory(path, &cur) else {
            return json!({"ok": false, "error": format!("cannot rebuild {path}; verify the server files in Steam first")});
        };
        let want = if pct == 100 { base } else {
            let b = block(pct);
            let mut w = base;
            w.extend(varint((FIELD_ASSISTS << 3) | 2));
            w.extend(varint(b.len() as u64));
            w.extend(b);
            w
        };
        if want != cur {
            changes.push((path.to_string(), want));
        }
    }
    if changes.is_empty() {
        let mut s = status();
        s["unchanged"] = json!(true);
        s["notes"] = json!(notes);
        return s;
    }
    let res = match kspkg_write::write_inplace(&a, &changes) {
        Ok(r) => r,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let ver = kspkg_write::verify(&a);
    if ver["ok"] != true {
        let undone = kspkg_write::undo_inplace(&a, &res);
        return json!({"ok": false, "error": format!("archive failed verification: {ver}"), "rolled_back": undone.is_ok()});
    }
    for (path, blob) in &changes {
        if read(&a, path).as_deref() != Some(blob.as_slice()) {
            let undone = kspkg_write::undo_inplace(&a, &res);
            return json!({"ok": false, "error": format!("readback of {path} does not match"), "rolled_back": undone.is_ok()});
        }
    }
    let mut s = status();
    s["written"] = json!(changes.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>());
    s["notes"] = json!(notes);
    s["records"] = res.get("records").cloned().unwrap_or(Value::Null);
    s
}

/// Called by start: make the archive match THIS profile.
pub fn apply_for(profile: &Value) -> Value {
    let want = profile.get("damage_pct").and_then(|v| v.as_f64()).map(|v| v as i64).unwrap_or(100);
    let cur = status();
    if cur["ok"] == true && cur["damage"].as_i64() == Some(want) {
        return json!({"ok": true, "unchanged": true, "damage": want});
    }
    set_damage(want as f64)
}
