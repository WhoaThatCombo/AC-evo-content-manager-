//! Editing the two system tables that make a track exist to the server -
//! ported from acecm/tracktables.py.
//!
//!   tracks.table             f2 rows: f1 display name, f3 folder, f4 .scene,
//!                            f5 country, f8 .track, f13 region
//!   track_containers.table   f2 wrapper -> repeated f3 rows -> f8 body:
//!                            f1 session name, f10 display name,
//!                            f11 repeated container path, f14 layout ...
//!
//! ⚠ track_containers holds ONE ROW PER GAME MODE (Road Atlanta ships 12);
//! miss the mode the server starts in and it reports `containers: []`.
//! ⚠ Layout names match CASE-SENSITIVELY.
//! Every function re-emits fields exactly as it read them, so a row it does
//! not touch comes out byte-identical.
#![allow(dead_code)]

use std::collections::{BTreeSet, HashMap};

use crate::pb::{self, Field, Val};

pub fn varint(mut n: u64) -> Vec<u8> {
    let mut out = vec![];
    loop {
        let b = (n & 127) as u8;
        n >>= 7;
        out.push(b | if n != 0 { 128 } else { 0 });
        if n == 0 {
            return out;
        }
    }
}

fn tag(field: u64, wire: u8) -> Vec<u8> {
    varint((field << 3) | wire as u64)
}

pub fn emit_bytes(field: u64, value: &[u8]) -> Vec<u8> {
    let mut o = tag(field, 2);
    o.extend(varint(value.len() as u64));
    o.extend_from_slice(value);
    o
}

fn emit(f: &Field) -> Vec<u8> {
    match (&f.val, f.wire) {
        (Val::Bytes(b), 2) => emit_bytes(f.num, b),
        (Val::Int(n), _) => { let mut o = tag(f.num, 0); o.extend(varint(*n)); o }
        (Val::Bytes(b), w) => { let mut o = tag(f.num, w); o.extend_from_slice(b); o }
    }
}

fn walk(d: &[u8]) -> Result<Vec<Field<'_>>, String> {
    pb::walk(d).map_err(|_| "bad wire type".to_string())
}

fn s(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// Both tables wrap their rows in a single field 2.
fn unwrap(blob: &[u8]) -> Result<&[u8], String> {
    let top = walk(blob)?;
    if top.len() != 1 || top[0].num != 2 {
        return Err("unexpected table wrapper".into());
    }
    Ok(top[0].val.bytes())
}

pub fn track_paths(folder: &str) -> (String, String, String) {
    let base = format!("content\\tracks\\{folder}");
    (base.clone(), format!("{base}\\{folder}.scene"), format!("{base}\\{folder}.track"))
}

/// The display name of one tracks.table row (f2 -> f1).
fn row_name(v: &[u8]) -> Result<Option<String>, String> {
    let mut name = None;
    for g in walk(v)? {
        if g.num == 2 && g.wire == 2 {
            for h in walk(g.val.bytes())? {
                if h.num == 1 && h.wire == 2 {
                    name = Some(s(h.val.bytes()));
                }
            }
        }
    }
    Ok(name)
}

/// The base folder path (f2 -> f3) of one tracks.table row.
fn row_base(v: &[u8]) -> Result<Option<String>, String> {
    let mut base = None;
    for g in walk(v)? {
        if g.num == 2 && g.wire == 2 {
            for h in walk(g.val.bytes())? {
                if h.num == 3 && h.wire == 2 {
                    base = Some(s(h.val.bytes()));
                }
            }
        }
    }
    Ok(base)
}

pub fn registered_names(blob: &[u8]) -> Result<BTreeSet<String>, String> {
    let mut out = BTreeSet::new();
    for f in walk(unwrap(blob)?)? {
        if f.wire != 2 {
            continue;
        }
        for g in walk(f.val.bytes())? {
            if g.num != 2 || g.wire != 2 {
                continue;
            }
            for h in walk(g.val.bytes())? {
                if h.num == 1 && h.wire == 2 {
                    out.insert(s(h.val.bytes()));
                }
            }
        }
    }
    Ok(out)
}

/// Display names of every tracks.table row pointing at `folder`.
pub fn rows_for_folder(blob: &[u8], folder: &str) -> Result<Vec<Option<String>>, String> {
    let want = format!("content\\tracks\\{folder}").to_lowercase();
    let mut out = vec![];
    for f in walk(unwrap(blob)?)? {
        if f.wire != 2 {
            continue;
        }
        let (mut name, mut base) = (None, None);
        for g in walk(f.val.bytes())? {
            if g.num == 2 && g.wire == 2 {
                for h in walk(g.val.bytes())? {
                    if h.num == 1 && h.wire == 2 {
                        name = Some(s(h.val.bytes()));
                    } else if h.num == 3 && h.wire == 2 {
                        base = Some(s(h.val.bytes()));
                    }
                }
            }
        }
        if base.map(|b| b.to_lowercase() == want).unwrap_or(false) {
            out.push(name);
        }
    }
    Ok(out)
}

pub fn remove_track_row(blob: &[u8], display_name: &str) -> Result<Vec<u8>, String> {
    let mut out = vec![];
    for f in walk(unwrap(blob)?)? {
        if f.wire != 2 {
            out.extend(emit(&f));
            continue;
        }
        if row_name(f.val.bytes())?.as_deref() == Some(display_name) {
            continue;
        }
        out.extend(emit(&f));
    }
    Ok(emit_bytes(2, &out))
}

/// The display name of a track_containers row (f8 -> f10).
fn container_name(v: &[u8]) -> Result<Option<String>, String> {
    let mut name = None;
    for g in walk(v)? {
        if g.num != 8 || g.wire != 2 {
            continue;
        }
        for h in walk(g.val.bytes())? {
            if h.num == 10 && h.wire == 2 {
                name = Some(s(h.val.bytes()));
            }
        }
    }
    Ok(name)
}

pub fn remove_container_rows(blob: &[u8], display_name: &str) -> Result<Vec<u8>, String> {
    let mut out = vec![];
    for f in walk(unwrap(blob)?)? {
        if f.wire != 2 {
            out.extend(emit(&f));
            continue;
        }
        if container_name(f.val.bytes())?.as_deref() == Some(display_name) {
            continue;
        }
        out.extend(emit(&f));
    }
    Ok(emit_bytes(2, &out))
}

fn rewrite_track_row(row: &[u8], display_name: &str, base: &str, scene: &str, trackf: &str) -> Result<Vec<u8>, String> {
    let mut inner = vec![];
    for f in walk(row)? {
        if f.num == 2 && f.wire == 2 {
            let mut body = vec![];
            for g in walk(f.val.bytes())? {
                let repl: Option<&str> = match (g.wire, g.num) {
                    (2, 1) => Some(display_name),
                    (2, 3) => Some(base),
                    (2, 4) => Some(scene),
                    (2, 8) => Some(trackf),
                    _ => None,
                };
                match repl {
                    Some(r) => body.extend(emit_bytes(g.num, r.as_bytes())),
                    None => body.extend(emit(&g)),
                }
            }
            inner.extend(emit_bytes(f.num, &body));
        } else {
            inner.extend(emit(&f));
        }
    }
    Ok(inner)
}

/// Point `display_name` at `folder`'s own paths, cloning `template_name`'s
/// row (any stock track) when the name is new. Never duplicates a row.
pub fn upsert_track_row(blob: &[u8], display_name: &str, folder: &str, template_name: Option<&str>) -> Result<Vec<u8>, String> {
    let (base, scene, trackf) = track_paths(folder);
    let mut out = vec![];
    let mut found = false;
    let mut template: Option<(u64, Vec<u8>)> = None;
    for f in walk(unwrap(blob)?)? {
        if f.wire != 2 {
            out.extend(emit(&f));
            continue;
        }
        let v = f.val.bytes();
        let name = row_name(v)?;
        if template.is_none() {
            if let Some(n) = &name {
                if !n.is_empty() && (template_name.is_none() || Some(n.as_str()) == template_name) {
                    template = Some((f.num, v.to_vec()));
                }
            }
        }
        if name.as_deref() != Some(display_name) {
            out.extend(emit(&f));
            continue;
        }
        // ⚠ the name is taken by ANOTHER folder (a stock track, say "Road
        // Atlanta"): repointing it would replace that track for good
        if let Some(b) = row_base(v)? {
            if !b.is_empty() && !b.eq_ignore_ascii_case(&base) {
                return Err(format!("\"{display_name}\" is already the name of another track ({b}) - choose a different name"));
            }
        }
        out.extend(emit_bytes(f.num, &rewrite_track_row(v, display_name, &base, &scene, &trackf)?));
        found = true;
    }
    if !found {
        let (tf, tv) = template.ok_or("no template row to clone")?;
        out.extend(emit_bytes(tf, &rewrite_track_row(&tv, display_name, &base, &scene, &trackf)?));
    }
    Ok(emit_bytes(2, &out))
}

/// Every (outer field, f8 body) row registered for a display name.
pub fn container_rows(blob: &[u8], display_name: &str) -> Result<Vec<(u64, Vec<u8>)>, String> {
    let mut out = vec![];
    for f in walk(unwrap(blob)?)? {
        if f.wire != 2 {
            continue;
        }
        for g in walk(f.val.bytes())? {
            if g.num != 8 || g.wire != 2 {
                continue;
            }
            for h in walk(g.val.bytes())? {
                if h.num == 10 && h.wire == 2 && s(h.val.bytes()) == display_name {
                    out.push((f.num, g.val.bytes().to_vec()));
                }
            }
        }
    }
    Ok(out)
}

const SUFFIXES: &[&str] = &["_gp", "_layout", "_drift", "_national", "_club", "_short"];

pub fn role(name: &str) -> String {
    for suf in SUFFIXES {
        if name.ends_with(suf) && name.len() > suf.len() {
            return name[..name.len() - suf.len()].to_string();
        }
    }
    name.to_string()
}

/// Register `containers` (role -> this track's file name) for every game
/// mode `template_name` supports. Returns (new blob, rows made).
pub fn upsert_container_rows(blob: &[u8], display_name: &str, folder: &str, layout: &str,
                             containers: &HashMap<String, String>, template_name: &str)
                             -> Result<(Vec<u8>, usize), String> {
    let template = container_rows(blob, template_name)?;
    if template.is_empty() {
        return Err(format!("no container rows for template '{template_name}'"));
    }
    let mut kept = vec![];
    for f in walk(unwrap(blob)?)? {
        let mut drop = false;
        if f.wire == 2 {
            for g in walk(f.val.bytes())? {
                if g.num != 8 || g.wire != 2 {
                    continue;
                }
                for h in walk(g.val.bytes())? {
                    if h.num == 10 && h.wire == 2 && s(h.val.bytes()) == display_name {
                        drop = true; // replace, never duplicate
                    }
                }
            }
        }
        if !drop {
            kept.extend(emit(&f));
        }
    }
    let mut made = 0;
    for (outer_f, body) in &template {
        // ⚠ the SESSION NAME (f1, "<layout> <mode>") is part of the match
        let mut src_layout: Option<String> = None;
        for f in walk(body)? {
            if f.num == 14 && f.wire == 2 {
                src_layout = Some(s(f.val.bytes()));
            }
        }
        let mut buf = vec![];
        for f in walk(body)? {
            if f.num == 11 && f.wire == 2 {
                let path = s(f.val.bytes());
                let mut name = path.rsplit('\\').next().unwrap_or("").to_string();
                if let Some(n) = name.strip_suffix(".scene") {
                    name = n.to_string();
                }
                if path.contains("common_assets") {
                    buf.extend(emit(&f)); // shared camera sequences
                    continue;
                }
                if let Some(mine) = containers.get(&role(&name)) {
                    buf.extend(emit_bytes(11, format!("content\\tracks\\{folder}\\containers\\{mine}.scene").as_bytes()));
                }
                continue;
            }
            if f.num == 1 && f.wire == 2 && src_layout.as_deref().map(|l| !l.is_empty()).unwrap_or(false) {
                let n = s(f.val.bytes());
                let sl = src_layout.as_deref().unwrap();
                if let Some(rest) = n.strip_prefix(&format!("{sl} ")) {
                    buf.extend(emit_bytes(1, format!("{layout} {rest}").as_bytes()));
                    continue;
                }
                buf.extend(emit(&f));
            } else if f.num == 10 && f.wire == 2 {
                buf.extend(emit_bytes(10, display_name.as_bytes()));
            } else if f.num == 14 && f.wire == 2 {
                buf.extend(emit_bytes(14, layout.as_bytes()));
            } else {
                buf.extend(emit(&f));
            }
        }
        kept.extend(emit_bytes(*outer_f, &emit_bytes(8, &buf)));
        made += 1;
    }
    Ok((emit_bytes(2, &kept), made))
}
