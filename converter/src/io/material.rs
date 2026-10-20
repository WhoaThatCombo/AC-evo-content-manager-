//! `.material` editing (stock layout, read off content.kspkg):
//!
//! - f1: shader name (`UberVehicleMaterial`, ...);
//! - f4 (repeated): property {f1 name, f2 value};
//!   value = {f1: f32} for a scalar, {f2|f3|f4: {f1..fN: f32}} for a vector
//!   of 2..4 components. Proto3: zero components are left out, so a zero
//!   scalar or an all-zero vector is an empty value message;
//! - f5 (repeated): texture slot {f1 name, f2 {f2: texture path}}, an empty
//!   f2 for an unused slot.

use super::pb::{Msg, Val};

fn named(m: &Msg, field: u32, name: &str) -> Option<usize> {
    m.fields.iter().position(|f| {
        f.num == field
            && match &f.val {
                Val::Bytes(b) => Msg::parse(b).ok().and_then(|s| s.str(1)).as_deref() == Some(name),
                Val::Msg(s) => s.str(1).as_deref() == Some(name),
                _ => false,
            }
    })
}

/// The value message for `v` (1..=4 components).
pub fn value(v: &[f32]) -> Msg {
    let mut out = Msg::default();
    match v.len() {
        0 => {}
        1 => {
            if v[0] != 0.0 {
                out.set_f32(1, v[0]);
            }
        }
        n => {
            let mut c = Msg::default();
            for (i, x) in v.iter().enumerate().take(4) {
                if *x != 0.0 {
                    c.set_f32(i as u32 + 1, *x);
                }
            }
            out.set(n.min(4) as u32, Val::Msg(c));
        }
    }
    out
}

/// A property's components (`n` = expected count; missing ones are 0).
pub fn get_prop(m: &Msg, name: &str, n: usize) -> Option<Vec<f32>> {
    let i = named(m, 4, name)?;
    let p = match &m.fields[i].val {
        Val::Bytes(b) => Msg::parse(b).ok()?,
        Val::Msg(s) => s.clone(),
        _ => return None,
    };
    let v = p.sub(2).unwrap_or_default();
    if n <= 1 {
        return Some(vec![v.f32(1).unwrap_or(0.0)]);
    }
    let c = v.sub(n.min(4) as u32).unwrap_or_default();
    Some((1..=n as u32).map(|k| c.f32(k).unwrap_or(0.0)).collect())
}

/// Set (or add) property `name` to `v`.
pub fn set_prop(m: &mut Msg, name: &str, v: &[f32]) {
    let mut p = Msg::default();
    p.set_str(1, name);
    p.set(2, Val::Msg(value(v)));
    match named(m, 4, name) {
        Some(i) => m.fields[i].val = Val::Msg(p),
        None => m.push(4, Val::Msg(p)),
    }
}

/// Point texture slot `name` at `path` ("" = unused).
pub fn set_slot(m: &mut Msg, name: &str, path: &str) {
    let mut s = Msg::default();
    s.set_str(1, name);
    let mut t = Msg::default();
    if !path.is_empty() {
        t.set_str(2, path);
    }
    s.set(2, Val::Msg(t));
    match named(m, 5, name) {
        Some(i) => m.fields[i].val = Val::Msg(s),
        None => m.push(5, Val::Msg(s)),
    }
}

/// The texture a slot points at, if any.
pub fn get_slot(m: &Msg, name: &str) -> Option<String> {
    let i = named(m, 5, name)?;
    let s = match &m.fields[i].val {
        Val::Bytes(b) => Msg::parse(b).ok()?,
        Val::Msg(s) => s.clone(),
        _ => return None,
    };
    s.sub(2).and_then(|t| t.str(2)).filter(|p| !p.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stock_encodings() {
        // Splatmap_Constant = (1, 0, 0, 0) as stored by a stock material
        assert_eq!(value(&[1.0, 0.0, 0.0, 0.0]).encode(), b"\x22\x05\x0d\x00\x00\x80\x3f");
        assert!(value(&[0.0]).encode().is_empty());
        let mut m = Msg::default();
        set_prop(&mut m, "Base_Basecolor", &[0.5, 0.25, 0.0, 1.0]);
        set_prop(&mut m, "Reflectance", &[0.5]);
        set_slot(&mut m, "txDiffuse", "content\\cars\\x\\a.texture");
        let m = Msg::parse(&m.encode()).unwrap();
        assert_eq!(get_prop(&m, "Base_Basecolor", 4).unwrap(), vec![0.5, 0.25, 0.0, 1.0]);
        assert_eq!(get_prop(&m, "Reflectance", 1).unwrap(), vec![0.5]);
        assert_eq!(get_slot(&m, "txDiffuse").as_deref(), Some("content\\cars\\x\\a.texture"));
    }
}
