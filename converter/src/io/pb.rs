//! Lossless, schema-less protobuf editing.
//!
//! A message is a list of fields in file order. Length-delimited values stay
//! raw bytes until a caller asks to edit them as a sub-message (`msg_mut`),
//! so decode -> encode is byte-identical for everything not touched (checked
//! over every donor file by `aceconv pbcheck`). Unknown fields survive.

use anyhow::{bail, Result};

#[derive(Clone, Debug)]
pub enum Val {
    Varint(u64),
    F64([u8; 8]),
    F32([u8; 4]),
    Bytes(Vec<u8>),
    /// a length-delimited value opened for editing
    Msg(Msg),
}

#[derive(Clone, Debug)]
pub struct Field {
    pub num: u32,
    pub val: Val,
}

#[derive(Clone, Debug, Default)]
pub struct Msg {
    pub fields: Vec<Field>,
}

pub fn read_varint(d: &[u8], i: &mut usize) -> Result<u64> {
    let (mut v, mut s) = (0u64, 0u32);
    loop {
        let Some(&b) = d.get(*i) else { bail!("truncated varint") };
        *i += 1;
        if s < 64 {
            v |= ((b & 0x7f) as u64) << s;
        }
        s += 7;
        if b & 0x80 == 0 {
            return Ok(v);
        }
        if s > 70 {
            bail!("varint too long");
        }
    }
}

pub fn write_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

impl Msg {
    pub fn parse(d: &[u8]) -> Result<Msg> {
        let mut fields = Vec::new();
        let mut i = 0;
        while i < d.len() {
            let key = read_varint(d, &mut i)?;
            let num = (key >> 3) as u32;
            if num == 0 {
                bail!("field number 0");
            }
            let val = match key & 7 {
                0 => Val::Varint(read_varint(d, &mut i)?),
                1 => {
                    let Some(s) = d.get(i..i + 8) else { bail!("truncated fixed64") };
                    i += 8;
                    Val::F64(s.try_into().unwrap())
                }
                2 => {
                    let n = read_varint(d, &mut i)? as usize;
                    let Some(s) = d.get(i..i + n) else { bail!("length past end") };
                    i += n;
                    Val::Bytes(s.to_vec())
                }
                5 => {
                    let Some(s) = d.get(i..i + 4) else { bail!("truncated fixed32") };
                    i += 4;
                    Val::F32(s.try_into().unwrap())
                }
                w => bail!("wire type {w}"),
            };
            fields.push(Field { num, val });
        }
        Ok(Msg { fields })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode_into(&mut out);
        out
    }

    pub fn encode_into(&self, out: &mut Vec<u8>) {
        for f in &self.fields {
            let n = f.num as u64;
            match &f.val {
                Val::Varint(v) => {
                    write_varint(out, n << 3);
                    write_varint(out, *v);
                }
                Val::F64(b) => {
                    write_varint(out, n << 3 | 1);
                    out.extend_from_slice(b);
                }
                Val::F32(b) => {
                    write_varint(out, n << 3 | 5);
                    out.extend_from_slice(b);
                }
                Val::Bytes(b) => {
                    write_varint(out, n << 3 | 2);
                    write_varint(out, b.len() as u64);
                    out.extend_from_slice(b);
                }
                Val::Msg(m) => {
                    let inner = m.encode();
                    write_varint(out, n << 3 | 2);
                    write_varint(out, inner.len() as u64);
                    out.extend_from_slice(&inner);
                }
            }
        }
    }

    // ---- reading ---------------------------------------------------------

    pub fn get(&self, num: u32) -> Option<&Val> {
        self.fields.iter().find(|f| f.num == num).map(|f| &f.val)
    }

    pub fn all(&self, num: u32) -> impl Iterator<Item = &Val> {
        self.fields.iter().filter(move |f| f.num == num).map(|f| &f.val)
    }

    pub fn varint(&self, num: u32) -> Option<u64> {
        match self.get(num)? {
            Val::Varint(v) => Some(*v),
            _ => None,
        }
    }

    pub fn f32(&self, num: u32) -> Option<f32> {
        match self.get(num)? {
            Val::F32(b) => Some(f32::from_le_bytes(*b)),
            _ => None,
        }
    }

    pub fn bytes(&self, num: u32) -> Option<&[u8]> {
        match self.get(num)? {
            Val::Bytes(b) => Some(b),
            _ => None,
        }
    }

    pub fn str(&self, num: u32) -> Option<String> {
        self.bytes(num).map(|b| String::from_utf8_lossy(b).into_owned())
    }

    /// A sub-message, parsed fresh (read-only).
    pub fn sub(&self, num: u32) -> Option<Msg> {
        match self.get(num)? {
            Val::Bytes(b) => Msg::parse(b).ok(),
            Val::Msg(m) => Some(m.clone()),
            _ => None,
        }
    }

    pub fn subs(&self, num: u32) -> Vec<Msg> {
        self.all(num)
            .filter_map(|v| match v {
                Val::Bytes(b) => Msg::parse(b).ok(),
                Val::Msg(m) => Some(m.clone()),
                _ => None,
            })
            .collect()
    }

    // ---- editing ---------------------------------------------------------

    /// Open field `idx` (an index into `fields`) for editing as a message.
    pub fn open(&mut self, idx: usize) -> Result<&mut Msg> {
        let f = &mut self.fields[idx];
        if let Val::Bytes(b) = &f.val {
            f.val = Val::Msg(Msg::parse(b)?);
        }
        match &mut f.val {
            Val::Msg(m) => Ok(m),
            _ => bail!("field {} is not length-delimited", f.num),
        }
    }

    /// The first field `num`, opened as a message (created when absent).
    pub fn msg_mut(&mut self, num: u32) -> Result<&mut Msg> {
        let idx = match self.fields.iter().position(|f| f.num == num) {
            Some(i) => i,
            None => {
                self.fields.push(Field { num, val: Val::Msg(Msg::default()) });
                self.fields.len() - 1
            }
        };
        self.open(idx)
    }

    /// Indices of every field `num`.
    pub fn idx(&self, num: u32) -> Vec<usize> {
        self.fields.iter().enumerate().filter(|(_, f)| f.num == num).map(|(i, _)| i).collect()
    }

    /// Replace the first field `num` (append when absent).
    pub fn set(&mut self, num: u32, val: Val) {
        match self.fields.iter_mut().find(|f| f.num == num) {
            Some(f) => f.val = val,
            None => self.fields.push(Field { num, val }),
        }
    }

    pub fn set_varint(&mut self, num: u32, v: u64) {
        self.set(num, Val::Varint(v));
    }

    pub fn set_f32(&mut self, num: u32, v: f32) {
        self.set(num, Val::F32(v.to_le_bytes()));
    }

    pub fn set_str(&mut self, num: u32, s: &str) {
        self.set(num, Val::Bytes(s.as_bytes().to_vec()));
    }

    pub fn remove(&mut self, num: u32) {
        self.fields.retain(|f| f.num != num);
    }

    pub fn push(&mut self, num: u32, val: Val) {
        self.fields.push(Field { num, val });
    }
}

/// f32 slice -> little-endian bytes (packed repeated / vertex buffers).
pub fn f32_bytes(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

/// u32 slice -> packed varints (index buffers).
pub fn varint_bytes(v: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 2);
    for &x in v {
        write_varint(&mut out, x as u64);
    }
    out
}

/// packed varints -> values (stops at the first malformed one)
pub fn varints(b: &[u8]) -> Vec<u64> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match read_varint(b, &mut i) {
            Ok(v) => out.push(v),
            Err(_) => break,
        }
    }
    out
}
