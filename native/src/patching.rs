//! Binary patching, done safely enough to trust - ported from patching.py
//! and hooking.py. Patches are DATA: the build they were made for (MD5),
//! the bytes expected at each site and the bytes to write. Applying checks
//! every site first and writes nothing unless all match.
//!
//! ⚠ `.text` is r-x at runtime: scratch goes on the stack, tables in .data
//! page slack. ⚠ A patch is only valid for its build: the MD5 gate refuses.

use std::path::{Path, PathBuf};

use md5::Digest;
use serde_json::{json, Map, Value};

fn registry() -> PathBuf {
    crate::paths::data().join("patches.json")
}

fn hexs(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Result<Vec<u8>, String> {
    let s = s.trim();
    if s.len() % 2 != 0 || !s.is_ascii() {
        return Err("bad hex".into());
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string())).collect()
}

fn md5hex(d: &[u8]) -> String {
    hexs(&md5::Md5::digest(d))
}

// --------------------------------------------------------------- PE helpers --
pub struct Section {
    name: String,
    va: u64,
    vsize: u64,
    raw: u64,
    rawsize: u64,
    write: bool,
    exec: bool,
}

pub struct Pe {
    pub data: Vec<u8>,
    image_base: u64,
    sections: Vec<Section>,
}

impl Pe {
    pub fn open(path: &str) -> Result<Pe, String> {
        let d = std::fs::read(path).map_err(|e| e.to_string())?;
        let u16at = |o: usize| d.get(o..o + 2).map(|b| u16::from_le_bytes(b.try_into().unwrap()) as usize).ok_or("truncated PE");
        let u32at = |o: usize| d.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()) as u64).ok_or("truncated PE");
        let pe = u32at(0x3c)? as usize;
        let image_base = d.get(pe + 48..pe + 56).map(|b| u64::from_le_bytes(b.try_into().unwrap())).ok_or("truncated PE")?;
        let nsec = u16at(pe + 6)?;
        let optsz = u16at(pe + 20)?;
        let off = pe + 24 + optsz;
        let mut sections = Vec::new();
        for i in 0..nsec {
            let b = off + i * 40;
            let name_raw = d.get(b..b + 8).ok_or("truncated PE")?;
            let name = String::from_utf8_lossy(name_raw).trim_end_matches('\0').to_string();
            let (vsz, va, rsz, ptr) = (u32at(b + 8)?, u32at(b + 12)?, u32at(b + 16)?, u32at(b + 20)?);
            let chars = u32at(b + 36)?;
            sections.push(Section { name, va: image_base + va, vsize: vsz, raw: ptr, rawsize: rsz,
                                    write: chars & 0x8000_0000 != 0, exec: chars & 0x2000_0000 != 0 });
        }
        Ok(Pe { data: d, image_base, sections })
    }

    pub fn va_to_off(&self, va: u64) -> Option<usize> {
        for s in &self.sections {
            if s.va <= va && va < s.va + s.vsize {
                let o = s.raw + (va - s.va);
                return (o < s.raw + s.rawsize).then_some(o as usize);
            }
        }
        None
    }

    /// Runs of int3 padding in executable sections, biggest first.
    pub fn code_caves(&self, minimum: usize) -> Vec<(u64, usize, String)> {
        let mut out = Vec::new();
        for s in self.sections.iter().filter(|s| s.exec) {
            let end = (s.raw + s.rawsize.min(s.vsize)) as usize;
            let blob = &self.data[(s.raw as usize).min(self.data.len())..end.min(self.data.len())];
            let mut run = 0usize;
            for (i, b) in blob.iter().enumerate() {
                if *b == 0xCC {
                    run += 1;
                    continue;
                }
                if run >= minimum {
                    out.push((s.va + (i - run) as u64, run, s.name.clone()));
                }
                run = 0;
            }
            if run >= minimum {
                out.push((s.va + (blob.len() - run) as u64, run, s.name.clone()));
            }
        }
        out.sort_by(|a, b| b.1.cmp(&a.1));
        out
    }

    /// Writable space past a section's VIRTUAL size to the page boundary.
    /// ⚠ NOT the raw->virtual gap: that is the section's BSS, in use.
    fn bss_slack(&self) -> Vec<Value> {
        let mut out = Vec::new();
        for s in self.sections.iter().filter(|s| s.write) {
            let end_virt = s.va + s.vsize;
            let page_end = (end_virt + 0xfff) & !0xfff;
            if page_end - end_virt >= 64 {
                out.push(json!({"section": s.name, "va": format!("{end_virt:#x}"), "size": page_end - end_virt}));
            }
        }
        out
    }
}

// ------------------------------------------------------------------ patches --
fn site_off(pe: &Pe, site: &Value) -> Option<usize> {
    if let Some(o) = site.get("off").and_then(|v| v.as_u64()) {
        return Some(o as usize);
    }
    pe.va_to_off(site.get("va").and_then(|v| v.as_u64())?)
}

fn sv(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

/// clean | applied | mismatch | wrong-build | missing
pub fn status(p: &Value) -> Value {
    let target = sv(p, "target");
    if !Path::new(&target).is_file() {
        return json!({"state": "missing", "detail": "target not found"});
    }
    let Ok(pe) = Pe::open(&target) else { return json!({"state": "missing", "detail": "target not found"}) };
    let cur = md5hex(&pe.data);
    let sites = p.get("sites").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let (mut applied, mut clean) = (0, 0);
    for s in &sites {
        let Some(off) = site_off(&pe, s) else { return json!({"state": "mismatch", "detail": "address not in image"}) };
        let n = sv(s, "write").len() / 2;
        let have = hexs(&pe.data[off.min(pe.data.len())..(off + n).min(pe.data.len())]);
        if have == sv(s, "write").to_lowercase() {
            applied += 1;
        } else if have == sv(s, "expect").to_lowercase() {
            clean += 1;
        }
    }
    let state = if applied == sites.len() { "applied" } else if clean == sites.len() { "clean" } else { "mismatch" };
    let want = sv(p, "build_md5");
    let wrong = state == "clean" && !want.is_empty() && cur != want;
    json!({"state": if wrong { "wrong-build" } else { state }, "md5": cur, "expected_md5": want,
           "sites_applied": applied, "sites_clean": clean, "sites": sites.len()})
}

fn backup_path(p: &Value) -> String {
    format!("{}.bak_{}", sv(p, "target"), sv(p, "id"))
}

pub fn apply(p: &Value, force: bool) -> Value {
    let st = status(p);
    let state = sv(&st, "state");
    let with = |mut v: Value| { if let (Some(o), Some(s)) = (v.as_object_mut(), st.as_object()) { for (k, x) in s { o.insert(k.clone(), x.clone()); } } v };
    match state.as_str() {
        "applied" => return with(json!({"ok": true, "already": true})),
        "missing" => return json!({"ok": false, "error": "target not found"}),
        "mismatch" => return with(json!({"ok": false, "error": "bytes at one or more sites match neither the expected nor the patched value - refusing to write"})),
        "wrong-build" if !force => return with(json!({"ok": false, "error": format!("this patch was made for build {}, target is {} - offsets will not match", sv(p, "build_md5"), sv(&st, "md5"))})),
        _ => {}
    }
    let target = sv(p, "target");
    let Ok(pe) = Pe::open(&target) else { return json!({"ok": false, "error": "target not found"}) };
    let mut data = pe.data.clone();
    // verify EVERY site before writing ANY
    let mut writes = Vec::new();
    for s in p.get("sites").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
        let Some(off) = site_off(&pe, &s) else { return json!({"ok": false, "error": "address not in image"}) };
        let (Ok(exp), Ok(new)) = (unhex(&sv(&s, "expect")), unhex(&sv(&s, "write"))) else { return json!({"ok": false, "error": "bad hex in patch"}) };
        if data.get(off..off + exp.len()) != Some(&exp[..]) {
            return json!({"ok": false, "error": format!("verify failed at {off:#x}")});
        }
        writes.push((off, new));
    }
    let bak = backup_path(p);
    if writes.iter().any(|(off, new)| off + new.len() > data.len()) {
        return json!({"ok": false, "error": "a site writes past the end of the file"});
    }
    if let Err(e) = crate::safefs::backup_once(Path::new(&target), Path::new(&bak)) {
        return json!({"ok": false, "error": e.to_string()});
    }
    let n = writes.len();
    for (off, new) in writes {
        data[off..off + new.len()].copy_from_slice(&new);
    }
    if let Err(e) = crate::safefs::write_atomic(Path::new(&target), &data) {
        return json!({"ok": false, "error": e.to_string()});
    }
    json!({"ok": true, "backup": bak, "sites": n})
}

pub fn restore(p: &Value) -> Value {
    let bak = backup_path(p);
    let target = sv(p, "target");
    if Path::new(&bak).is_file() {
        return match crate::safefs::copy_atomic(Path::new(&bak), Path::new(&target)) {
            Ok(_) => json!({"ok": true, "restored_from": bak}),
            Err(e) => json!({"ok": false, "error": e.to_string()}),
        };
    }
    // no backup: put the expected bytes back, which is equally correct
    let Ok(pe) = Pe::open(&target) else { return json!({"ok": false, "error": "target not found"}) };
    let mut data = pe.data.clone();
    let mut n = 0;
    for s in p.get("sites").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
        let Some(off) = site_off(&pe, &s) else { continue };
        let Ok(exp) = unhex(&sv(&s, "expect")) else { continue };
        if off + exp.len() <= data.len() {
            data[off..off + exp.len()].copy_from_slice(&exp);
            n += 1;
        }
    }
    if let Err(e) = crate::safefs::write_atomic(Path::new(&target), &data) {
        return json!({"ok": false, "error": e.to_string()});
    }
    json!({"ok": true, "reverted_sites": n, "note": "no backup; wrote the expected bytes back"})
}

fn norm_patch(d: &Value) -> Value {
    let id = d.get("id").filter(|v| !v.is_null()).or_else(|| d.get("pid")).cloned().unwrap_or(Value::Null);
    json!({"id": id, "title": d.get("title").cloned().unwrap_or(json!("")), "target": d.get("target").cloned().unwrap_or(json!("")),
           "build_md5": d.get("build_md5").cloned().unwrap_or(json!("")), "sites": d.get("sites").cloned().unwrap_or(json!([])),
           "description": d.get("description").cloned().unwrap_or(json!(""))})
}

pub fn load() -> Vec<Value> {
    std::fs::read(registry()).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.as_array().cloned()).unwrap_or_default().iter().map(norm_patch).collect()
}

fn save(items: &[Value]) {
    let _ = crate::profiles::write_json_indent2(&registry(), &Value::Array(items.to_vec()));
}

pub fn upsert(d: &Value) -> Value {
    let mut items: Vec<Value> = load().into_iter().filter(|p| p.get("id") != d.get("id")).collect();
    items.push(norm_patch(d));
    save(&items);
    d.clone()
}

pub fn remove(id: &Value) -> Value {
    let items: Vec<Value> = load().into_iter().filter(|p| p.get("id") != Some(id)).collect();
    save(&items);
    json!({"ok": true})
}

pub fn find(id: &Value) -> Option<Value> {
    load().into_iter().find(|p| p.get("id") == Some(id))
}

pub fn overview() -> Value {
    let out: Vec<Value> = load().into_iter().map(|mut p| { p["status"] = status(&p); p }).collect();
    json!({"patches": out, "registry": registry().to_string_lossy()})
}

fn abs(p: &str) -> String {
    std::path::absolute(p).map(|a| a.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// Only the configured game / dedicated-server binaries (and their folders).
fn allowed_target(target: &str) -> bool {
    let t = abs(target);
    if target.is_empty() || !Path::new(target).is_file() {
        return false;
    }
    let cfg = crate::config::load();
    let mut allowed: Vec<String> = Vec::new();
    for p in [crate::detect::find("server_exe", false), crate::config::get_str(&cfg, "game_exe"), crate::config::get_str(&cfg, "server_exe"), crate::detect::find("game_exe", false)] {
        if !p.is_empty() {
            allowed.push(abs(&p));
        }
    }
    if allowed.contains(&t) {
        return true;
    }
    allowed.iter().filter_map(|p| Path::new(p).parent().map(|d| d.to_string_lossy().to_string()))
        .any(|r| !r.is_empty() && Path::new(&t).starts_with(&r))
}

/// What a binary offers a patch author: caves, slack, identity.
pub fn inspect(target: &str) -> Value {
    if !allowed_target(target) {
        return json!({"ok": false, "error": "that file is not the game or server"});
    }
    let pe = match Pe::open(target) { Ok(p) => p, Err(_) => return json!({"ok": false, "error": "not found"}) };
    let sections: Vec<Value> = pe.sections.iter().map(|s| json!({"name": s.name, "va": format!("{:#x}", s.va), "vsize": s.vsize,
        "raw": s.raw, "rawsize": s.rawsize, "write": s.write, "exec": s.exec})).collect();
    let caves: Vec<Value> = pe.code_caves(64).into_iter().take(5).map(|(va, size, sec)| json!({"va": format!("{va:#x}"), "size": size, "section": sec})).collect();
    json!({"ok": true, "target": target, "md5": md5hex(&pe.data), "size": pe.data.len(), "image_base": format!("{:#x}", pe.image_base),
           "sections": sections, "code_caves": caves, "bss_slack": pe.bss_slack()})
}

// ----------------------------------------------------------------- hooking --
/// Plan a trampoline hook: whole instructions covering >= `minimum` bytes
/// move to the cave (rip-relative and rel32 call/jmp relocated, short
/// branches refused), payload first, then a jump back. Returns sites ready
/// to register as a patch.
pub fn hook_plan(pe: &Pe, site_va: u64, cave_va: u64, payload: &[u8], minimum: usize) -> Result<Value, String> {
    use iced_x86::{Decoder, DecoderOptions, Formatter, FormatterOutput, FormatterTextKind, IntelFormatter, MemorySizeOptions};
    let off = pe.va_to_off(site_va).ok_or(format!("{site_va:#x} is not in the image"))?;
    let code = &pe.data[off..(off + 64).min(pe.data.len())];
    let mut dec = Decoder::with_ip(64, code, site_va, DecoderOptions::NONE);
    // ⚠ constant offsets describe the LAST decoded instruction: take them now
    let (mut ins, mut taken) = (Vec::new(), 0usize);
    while dec.can_decode() {
        let i = dec.decode();
        if i.is_invalid() {
            break;
        }
        taken += i.len();
        ins.push((i, dec.get_constant_offsets(&i)));
        if taken >= minimum {
            break;
        }
    }
    if taken < minimum {
        return Err("could not decode enough instructions".into());
    }
    let return_va = site_va + taken as u64;
    let mut cave: Vec<u8> = payload.to_vec();
    let mut listing = Vec::new();
    // the listing reads like capstone's (the Python version's) Intel syntax
    let mut fmt = IntelFormatter::new();
    let o = fmt.options_mut();
    o.set_hex_prefix("0x");
    o.set_hex_suffix("");
    o.set_uppercase_hex(false);
    o.set_space_after_operand_separator(true);
    o.set_space_between_memory_add_operators(true);
    o.set_memory_size_options(MemorySizeOptions::Always);
    o.set_rip_relative_addresses(true);
    o.set_branch_leading_zeros(false);
    o.set_show_branch_size(false);
    struct Out(String);
    impl FormatterOutput for Out {
        fn write(&mut self, t: &str, _k: FormatterTextKind) {
            self.0.push_str(t);
        }
    }
    for (i, co) in &ins {
        let i = i.clone();
        let (mut m, mut ops) = (Out(String::new()), Out(String::new()));
        fmt.format_mnemonic(&i, &mut m);
        fmt.format_all_operands(&i, &mut ops);
        // capstone calls the imm64 and moffs forms of mov "movabs"
        if m.0 == "mov" && ((0..i.op_count()).any(|k| i.op_kind(k) == iced_x86::OpKind::Immediate64) || (i.memory_displ_size() == 8 && i.memory_base() == iced_x86::Register::None && i.memory_index() == iced_x86::Register::None)) {
            m.0 = "movabs".into();
        }
        let text = format!("{} {}", m.0, ops.0);
        listing.push(format!("{:012x}  {text}", i.ip()));
        let i = &i;
        let start = (i.ip() - site_va) as usize;
        let mut raw = code[start..start + i.len()].to_vec();
        let new_addr = cave_va + cave.len() as u64;
        // relative call/jmp (E8/E9 rel32)
        if i.len() == 5 && (raw[0] == 0xE8 || raw[0] == 0xE9) {
            let target = i.ip() as i64 + 5 + i32::from_le_bytes(raw[1..5].try_into().unwrap()) as i64;
            let rel = target - (new_addr as i64 + 5);
            let rel = i32::try_from(rel).map_err(|_| "relocated call/jmp out of rel32 range".to_string())?;
            raw[1..5].copy_from_slice(&rel.to_le_bytes());
            cave.extend_from_slice(&raw);
            continue;
        }
        // a rip-relative memory operand: the displacement is relative to
        // the END of the instruction, so moving it changes what it reads
        if i.is_ip_rel_memory_operand() {
            let target = i.ip_rel_memory_address() as i64;
            let rel = target - (new_addr as i64 + i.len() as i64);
            let rel = i32::try_from(rel).map_err(|_| "rip-relative target out of range".to_string())?;
            let pos = co.displacement_offset();
            raw[pos..pos + 4].copy_from_slice(&rel.to_le_bytes());
            cave.extend_from_slice(&raw);
            continue;
        }
        let m = m.0.as_str();
        if m.starts_with('j') || matches!(m, "loop" | "loope" | "loopne" | "call") {
            return Err(format!("cannot safely relocate '{text}' at {:#x} - pick another hook site", i.ip()));
        }
        cave.extend_from_slice(&raw);
    }
    let back_from = cave_va + cave.len() as u64;
    let rel = i32::try_from(return_va as i64 - (back_from as i64 + 5)).map_err(|_| "return jump out of rel32 range".to_string())?;
    cave.push(0xE9);
    cave.extend_from_slice(&rel.to_le_bytes());
    let rel_in = i32::try_from(cave_va as i64 - (site_va as i64 + 5)).map_err(|_| "cave too far from the hook site for rel32".to_string())?;
    let mut site_bytes = vec![0xE9];
    site_bytes.extend_from_slice(&rel_in.to_le_bytes());
    site_bytes.extend(std::iter::repeat(0x90).take(taken.saturating_sub(5)));
    let original = &pe.data[off..off + taken];
    let cave_off = pe.va_to_off(cave_va).ok_or("cave address is not in the image")?;
    let cave_now = &pe.data[cave_off..(cave_off + cave.len()).min(pe.data.len())];
    if cave_now.iter().any(|b| *b != 0xCC) {
        return Err(format!("cave at {cave_va:#x} is not free ({} bytes needed)", cave.len()));
    }
    let mut out = Map::new();
    out.insert("sites".into(), json!([
        {"va": site_va, "expect": hexs(original), "write": hexs(&site_bytes)},
        {"va": cave_va, "expect": hexs(cave_now), "write": hexs(&cave)},
    ]));
    out.insert("displaced".into(), json!(listing));
    out.insert("bytes_taken".into(), json!(taken));
    out.insert("cave_used".into(), json!(cave.len()));
    out.insert("return_to".into(), json!(format!("{return_va:#x}")));
    Ok(Value::Object(out))
}

/// Python's int(x) for a JSON value, with its error text.
fn py_int(v: &Value) -> Result<i64, String> {
    match v {
        Value::Number(n) => n.as_i64().or(n.as_f64().map(|f| f.trunc() as i64)).ok_or("ValueError: out of range".into()),
        Value::Bool(b) => Ok(*b as i64),
        Value::String(s) => s.trim().replace('_', "").parse::<i64>()
            .map_err(|_| format!("ValueError: invalid literal for int() with base 10: '{s}'")),
        Value::Null => Err("TypeError: int() argument must be a string, a bytes-like object or a real number, not 'NoneType'".into()),
        _ => Err("TypeError: int() argument must be a string, a bytes-like object or a real number".into()),
    }
}

/// bytes.fromhex: whitespace between bytes is allowed.
fn py_fromhex(s: &str) -> Result<Vec<u8>, String> {
    let c: Vec<char> = s.chars().collect();
    let (mut out, mut i) = (Vec::new(), 0);
    while i < c.len() {
        if c[i].is_whitespace() {
            i += 1;
            continue;
        }
        let hi = c[i].to_digit(16).ok_or(format!("ValueError: non-hexadecimal number found in fromhex() arg at position {i}"))?;
        let lo = c.get(i + 1).and_then(|x| x.to_digit(16)).ok_or(format!("ValueError: non-hexadecimal number found in fromhex() arg at position {}", i + 1))?;
        out.push((hi * 16 + lo) as u8);
        i += 2;
    }
    Ok(out)
}

/// POST /api/hook/plan
pub fn hook_route(body: &Value) -> Value {
    let target = body.get("target").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(String::from)
        .unwrap_or_else(|| crate::detect::find("server_exe", false));
    let pe = match Pe::open(&target) { Ok(p) => p, Err(e) => return json!({"ok": false, "error": format!("OSError: {e}")}) };
    let truthy = |v: &Value| !matches!(v, Value::Null | Value::Bool(false)) && v != &json!(0) && v != &json!("");
    let cave_v = match body.get("cave").filter(|v| truthy(v)) {
        Some(c) => c.clone(),
        None => match pe.code_caves(64).first() { Some(c) => json!(c.0), None => return json!({"ok": false, "error": "no code cave"}) },
    };
    let parsed = (|| -> Result<(i64, i64, Vec<u8>, i64), String> {
        let site = py_int(body.get("site").ok_or("KeyError: 'site'")?)?;
        let cave = py_int(&cave_v)?;
        let payload = py_fromhex(body.get("payload").and_then(|v| v.as_str()).unwrap_or(""))?;
        let minimum = match body.get("minimum") { Some(v) => py_int(v)?, None => 5 };
        Ok((site, cave, payload, minimum))
    })();
    let (site, cave, payload, minimum) = match parsed { Ok(p) => p, Err(e) => return json!({"ok": false, "error": e}) };
    match hook_plan(&pe, site as u64, cave as u64, &payload, minimum.max(0) as usize) {
        Ok(plan) => {
            let mut out = Map::new();
            out.insert("ok".into(), json!(true));
            out.insert("target".into(), json!(target));
            out.insert("cave".into(), json!(format!("{cave:#x}")));
            if let Value::Object(p) = plan {
                out.extend(p);
            }
            Value::Object(out)
        }
        Err(e) => json!({"ok": false, "error": format!("HookError: {e}")}),
    }
}
