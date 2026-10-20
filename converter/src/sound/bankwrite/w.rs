//! Byte-level writer for the FMOD Studio 2.03 bank serialisation (FMT 0x92).
//! docs/FORMATS.md, "FMOD Studio banks".

use super::prj::Guid;

#[derive(Default, Clone)]
pub struct W {
    pub b: Vec<u8>,
}

impl W {
    pub fn new() -> W {
        W { b: Vec::new() }
    }
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.b.push(v);
        self
    }
    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.b.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.b.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn i32(&mut self, v: i32) -> &mut Self {
        self.b.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn f32(&mut self, v: f32) -> &mut Self {
        self.b.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn guid(&mut self, g: &Guid) -> &mut Self {
        self.b.extend_from_slice(&g.0);
        self
    }
    pub fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.b.extend_from_slice(v);
        self
    }
    /// u16, high bit = a second u16 carries bits 15..30.
    pub fn vcount(&mut self, v: u32) -> &mut Self {
        if v < 0x8000 {
            self.u16(v as u16)
        } else {
            self.u16((v & 0x7fff) as u16 | 0x8000).u16((v >> 15) as u16)
        }
    }
    pub fn str(&mut self, s: &str) -> &mut Self {
        self.vcount(s.len() as u32).bytes(s.as_bytes())
    }
    /// u16-size-prefixed sub-block.
    pub fn sub(&mut self, f: impl FnOnce(&mut W)) -> &mut Self {
        let mut s = W::new();
        f(&mut s);
        self.u16(s.b.len() as u16).bytes(&s.b)
    }
    /// Fixed-size element array (same=1): vcount(n<<1|1), [u16 size], elements.
    pub fn arr<T>(&mut self, items: &[T], mut f: impl FnMut(&mut W, &T)) -> &mut Self {
        self.vcount((items.len() as u32) << 1 | 1);
        let mut first = true;
        for it in items {
            let mut s = W::new();
            f(&mut s, it);
            if first {
                self.u16(s.b.len() as u16);
                first = false;
            }
            self.bytes(&s.b);
        }
        self
    }
    /// Variable-size element array (same=0): each element has its own u16 size.
    pub fn arr_var<T>(&mut self, items: &[T], mut f: impl FnMut(&mut W, &T)) -> &mut Self {
        self.vcount((items.len() as u32) << 1);
        for it in items {
            let mut s = W::new();
            f(&mut s, it);
            self.u16(s.b.len() as u16).bytes(&s.b);
        }
        self
    }
    pub fn guids(&mut self, g: &[Guid]) -> &mut Self {
        self.arr(g, |w, x| {
            w.guid(x);
        })
    }
    /// RIFF chunk (odd sizes padded with a zero byte).
    pub fn chunk(&mut self, id: &[u8; 4], f: impl FnOnce(&mut W)) -> &mut Self {
        let mut s = W::new();
        f(&mut s);
        self.bytes(id).u32(s.b.len() as u32).bytes(&s.b);
        if s.b.len() & 1 == 1 {
            self.u8(0);
        }
        self
    }
    pub fn chunk_bytes(&mut self, id: &[u8; 4], body: &[u8]) -> &mut Self {
        self.bytes(id).u32(body.len() as u32).bytes(body);
        if body.len() & 1 == 1 {
            self.u8(0);
        }
        self
    }
    pub fn list(&mut self, typ: &[u8; 4], f: impl FnOnce(&mut W)) -> &mut Self {
        let mut s = W::new();
        s.bytes(typ);
        f(&mut s);
        self.bytes(b"LIST").u32(s.b.len() as u32).bytes(&s.b);
        if s.b.len() & 1 == 1 {
            self.u8(0);
        }
        self
    }
    /// `LIST <typ>` holding `LCNT` + one item per element (empty: no LCNT).
    pub fn items<T>(&mut self, typ: &[u8; 4], items: &[T], mut f: impl FnMut(&mut W, &T)) -> &mut Self {
        self.list(typ, |w| {
            if !items.is_empty() {
                w.chunk(b"LCNT", |c| {
                    c.u32(items.len() as u32);
                });
                for it in items {
                    f(w, it);
                }
            }
        })
    }
}

/// Trigger-condition / parameter code list: u32 byte length + entries.
#[derive(Default, Clone)]
pub struct Code {
    pub b: Vec<u8>,
}
impl Code {
    pub fn param(&mut self, g: &Guid) -> &mut Self {
        self.b.extend_from_slice(&0x11u32.to_le_bytes());
        self.b.extend_from_slice(&g.0);
        self
    }
    pub fn range(&mut self, tag: u32, lo: f32, hi: f32) -> &mut Self {
        self.b.extend_from_slice(&tag.to_le_bytes());
        self.b.extend_from_slice(&lo.to_le_bytes());
        self.b.extend_from_slice(&hi.to_le_bytes());
        self
    }
    pub fn raw(&mut self, tag: u32) -> &mut Self {
        self.b.extend_from_slice(&tag.to_le_bytes());
        self
    }
    pub fn write(&self, w: &mut W) {
        w.u32(self.b.len() as u32).bytes(&self.b);
    }
}
