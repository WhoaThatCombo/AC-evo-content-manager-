//! Schema-less protobuf walking - ported from tracktables.walk.
//! (field, wire, value): length-delimited values come back as bytes.

pub enum Val<'a> {
    Int(u64),
    Bytes(&'a [u8]),
}

pub struct Field<'a> {
    pub num: u64,
    pub wire: u8,
    pub val: Val<'a>,
}

fn varint(d: &[u8], i: &mut usize) -> Option<u64> {
    let (mut v, mut s) = (0u64, 0u32);
    loop {
        let b = *d.get(*i)?;
        *i += 1;
        if s < 64 {
            v |= ((b & 127) as u64) << s;
        }
        s += 7;
        if b & 128 == 0 {
            return Some(v);
        }
    }
}

/// Every field at this level. ⚠ Like the Python walker: a length running
/// past the end just stops the walk; a truncated varint or an unknown wire
/// type is an error (Err), which callers treat as "unreadable".
pub fn walk(d: &[u8]) -> Result<Vec<Field<'_>>, ()> {
    let mut out = vec![];
    let mut i = 0;
    while i < d.len() {
        let key = varint(d, &mut i).ok_or(())?;
        let (num, wire) = (key >> 3, (key & 7) as u8);
        match wire {
            2 => {
                let n = varint(d, &mut i).ok_or(())? as usize;
                if i.checked_add(n).map(|e| e > d.len()).unwrap_or(true) {
                    return Ok(out);
                }
                out.push(Field { num, wire, val: Val::Bytes(&d[i..i + n]) });
                i += n;
            }
            0 => out.push(Field { num, wire, val: Val::Int(varint(d, &mut i).ok_or(())?) }),
            5 => {
                let e = (i + 4).min(d.len());
                out.push(Field { num, wire, val: Val::Bytes(&d[i..e]) });
                i += 4;
            }
            1 => {
                let e = (i + 8).min(d.len());
                out.push(Field { num, wire, val: Val::Bytes(&d[i..e]) });
                i += 8;
            }
            _ => return Err(()),
        }
    }
    Ok(out)
}

impl<'a> Val<'a> {
    pub fn bytes(&self) -> &'a [u8] {
        match self {
            Val::Bytes(b) => b,
            Val::Int(_) => &[],
        }
    }
}
