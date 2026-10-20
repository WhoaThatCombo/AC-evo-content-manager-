//! AC1 `.kn5` model reader.
//!
//! Layout: "sc6969", version (i32; >5 carries one extra i32), textures
//! (active i32, name, size i32, DDS/PNG bytes), materials (name, shader,
//! blend u8, alphaTested u8, depthMode i32, props: name + f32 + 36 bytes of
//! v2/v3/v4, samplers: slot name + i32 + texture name), then ONE node tree.
//! Node: type i32 (1 transform, 2 mesh, 3 skinned mesh), name, child count,
//! active u8; transform = 16 f32; mesh = castShadows/visible/transparent u8,
//! [skinned: bones (name + 16 f32 bind)], vertex count, vertices (11 f32:
//! pos3 nrm3 uv2 tan3; skinned 19: + weights4 + bone indices4 as f32),
//! u16 indices, material id, layer, lodIn, lodOut, [mesh: bounding sphere
//! 4 f32 + renderable u8].
//!
//! ⚠ Parsed ONCE per conversion and shared by every stage. Texture and
//! vertex data stay slices into the memory map - nothing is copied.

use anyhow::{bail, Context, Result};

pub struct Texture<'a> {
    pub active: bool,
    pub name: String,
    pub data: &'a [u8],
}

#[derive(Clone, Debug)]
pub struct Prop {
    pub value: f32,
    pub v2: [f32; 2],
    pub v3: [f32; 3],
    pub v4: [f32; 4],
}

#[derive(Clone)]
pub struct Material {
    pub name: String,
    pub shader: String,
    pub blend_mode: u8,
    pub alpha_tested: bool,
    pub depth_mode: i32,
    pub props: Vec<(String, Prop)>,
    /// (slot, texture name)
    pub textures: Vec<(String, String)>,
}

impl Material {
    pub fn prop(&self, name: &str) -> Option<&Prop> {
        self.props.iter().find(|(n, _)| n == name).map(|(_, p)| p)
    }
    pub fn texture(&self, slot: &str) -> Option<&str> {
        self.textures.iter().find(|(n, _)| n == slot).map(|(_, t)| t.as_str())
    }
}

pub struct Bone {
    pub name: String,
    pub bind: [f32; 16],
}

pub struct Mesh<'a> {
    pub cast_shadows: bool,
    pub visible: bool,
    pub transparent: bool,
    pub skin_bones: Vec<Bone>,
    pub vertex_count: usize,
    /// 11 (or 19 when skinned) f32 per vertex, little-endian, unaligned
    pub stride: usize,
    pub vertex_bytes: &'a [u8],
    pub index_bytes: &'a [u8],
    pub material_id: i32,
    pub layer: i32,
    pub lod_in: f32,
    pub lod_out: f32,
    pub renderable: bool,
}

impl Mesh<'_> {
    #[inline]
    pub fn f(&self, vertex: usize, k: usize) -> f32 {
        let o = (vertex * self.stride + k) * 4;
        f32::from_le_bytes(self.vertex_bytes[o..o + 4].try_into().unwrap())
    }
    pub fn position(&self, v: usize) -> [f32; 3] { [self.f(v, 0), self.f(v, 1), self.f(v, 2)] }
    pub fn normal(&self, v: usize) -> [f32; 3] { [self.f(v, 3), self.f(v, 4), self.f(v, 5)] }
    pub fn uv(&self, v: usize) -> [f32; 2] { [self.f(v, 6), self.f(v, 7)] }
    pub fn tangent(&self, v: usize) -> [f32; 3] { [self.f(v, 8), self.f(v, 9), self.f(v, 10)] }
    pub fn index_count(&self) -> usize { self.index_bytes.len() / 2 }
    #[inline]
    pub fn index(&self, i: usize) -> u16 {
        u16::from_le_bytes([self.index_bytes[i * 2], self.index_bytes[i * 2 + 1]])
    }
}

pub enum NodeKind<'a> {
    Transform([f32; 16]),
    Mesh(Mesh<'a>),
    Other,
}

pub struct Node<'a> {
    pub kind: NodeKind<'a>,
    pub name: String,
    pub active: bool,
    pub children: Vec<Node<'a>>,
}

pub struct Kn5<'a> {
    pub version: i32,
    pub textures: Vec<Texture<'a>>,
    pub materials: Vec<Material>,
    pub root: Node<'a>,
}

struct R<'a> {
    b: &'a [u8],
    o: usize,
}

impl<'a> R<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.o + n > self.b.len() {
            bail!("kn5 truncated at {} (+{n})", self.o);
        }
        let s = &self.b[self.o..self.o + n];
        self.o += n;
        Ok(s)
    }
    fn i32(&mut self) -> Result<i32> { Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap())) }
    fn u8(&mut self) -> Result<u8> { Ok(self.take(1)?[0]) }
    fn f32(&mut self) -> Result<f32> { Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap())) }
    fn str(&mut self) -> Result<String> {
        let n = self.i32()?;
        if !(0..=1_000_000).contains(&n) {
            bail!("bad kn5 string length {n} at {}", self.o - 4);
        }
        Ok(String::from_utf8_lossy(self.take(n as usize)?).into_owned())
    }
    fn mat16(&mut self) -> Result<[f32; 16]> {
        let mut m = [0f32; 16];
        for v in &mut m {
            *v = self.f32()?;
        }
        Ok(m)
    }
}

/// A kn5 protected by its author (Custom Shaders Patch encryption): the
/// marker sits at the START of some files and at the END of others (135 cars
/// and 3 tracks of a 460-mod library had it only at the end, past a 64 KB head
/// check, and "converted" into noise).
pub fn protected(b: &[u8]) -> bool {
    const MARK: &[u8] = b"__AC_SHADERS_PATCH_KN5ENC";
    const SPAN: usize = 65536;
    let head = &b[..b.len().min(SPAN)];
    let tail = &b[b.len().saturating_sub(SPAN)..];
    memchr::memmem::find(head, MARK).is_some() || memchr::memmem::find(tail, MARK).is_some()
}

pub fn parse(b: &[u8]) -> Result<Kn5<'_>> {
    let mut r = R { b, o: 0 };
    if r.take(6)? != b"sc6969" {
        bail!("not a kn5 (bad magic) - CSP-protected models are not supported yet");
    }
    let version = r.i32()?;
    if version > 5 {
        r.i32()?;
    }
    let mut textures = Vec::new();
    for _ in 0..r.i32()?.max(0) {
        let active = r.i32()?;
        if active == 0 {
            textures.push(Texture { active: false, name: String::new(), data: &[] });
            continue;
        }
        let name = r.str()?;
        let size = r.i32()?.max(0) as usize;
        textures.push(Texture { active: true, name, data: r.take(size)? });
    }
    let mut materials = Vec::new();
    for _ in 0..r.i32()?.max(0) {
        let name = r.str()?;
        let shader = r.str()?;
        let blend_mode = r.u8()?;
        let alpha_tested = r.u8()? != 0;
        let depth_mode = r.i32()?;
        let mut props = Vec::new();
        for _ in 0..r.i32()?.max(0) {
            let pname = r.str()?;
            let value = r.f32()?;
            let mut f = [0f32; 9];
            for v in &mut f {
                *v = r.f32()?;
            }
            props.push((pname, Prop { value, v2: [f[0], f[1]], v3: [f[2], f[3], f[4]], v4: [f[5], f[6], f[7], f[8]] }));
        }
        let mut tex = Vec::new();
        for _ in 0..r.i32()?.max(0) {
            let slot = r.str()?;
            r.i32()?;
            tex.push((slot, r.str()?));
        }
        materials.push(Material { name, shader, blend_mode, alpha_tested, depth_mode, props, textures: tex });
    }
    let root = node(&mut r).context("kn5 node tree")?;
    Ok(Kn5 { version, textures, materials, root })
}

fn node<'a>(r: &mut R<'a>) -> Result<Node<'a>> {
    let ty = r.i32()?;
    let name = r.str()?;
    let children = r.i32()?.max(0) as usize;
    let active = r.u8()? != 0;
    let kind = match ty {
        1 => NodeKind::Transform(r.mat16()?),
        2 | 3 => {
            let cast_shadows = r.u8()? != 0;
            let visible = r.u8()? != 0;
            let transparent = r.u8()? != 0;
            let mut skin_bones = Vec::new();
            if ty == 3 {
                for _ in 0..r.i32()?.max(0) {
                    let bname = r.str()?;
                    skin_bones.push(Bone { name: bname, bind: r.mat16()? });
                }
            }
            let vertex_count = r.i32()?.max(0) as usize;
            let stride = if ty == 3 { 19 } else { 11 };
            let vertex_bytes = r.take(vertex_count * stride * 4)?;
            let ic = r.i32()?.max(0) as usize;
            let index_bytes = r.take(ic * 2)?;
            let material_id = r.i32()?;
            let layer = r.i32()?;
            let lod_in = r.f32()?;
            let lod_out = r.f32()?;
            let mut renderable = true;
            if ty == 2 {
                r.take(16)?; // bounding sphere
                renderable = r.u8()? != 0;
            }
            NodeKind::Mesh(Mesh {
                cast_shadows, visible, transparent, skin_bones, vertex_count, stride,
                vertex_bytes, index_bytes, material_id, layer, lod_in, lod_out, renderable,
            })
        }
        _ => NodeKind::Other,
    };
    let mut kids = Vec::with_capacity(children);
    for _ in 0..children {
        kids.push(node(r)?);
    }
    Ok(Node { kind, name, active, children: kids })
}

impl<'a> Node<'a> {
    /// Every node with its world matrix (row-major as stored; AC1 matrices
    /// are row-vector style: world = local * parent).
    pub fn walk<'s>(&'s self, parent: [f32; 16], out: &mut Vec<(&'s Node<'a>, [f32; 16])>) {
        let world = match &self.kind {
            NodeKind::Transform(m) => mul(m, &parent),
            _ => parent,
        };
        out.push((self, world));
        for c in &self.children {
            c.walk(world, out);
        }
    }
}

pub const IDENTITY: [f32; 16] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

/// a * b for row-major 4x4.
pub fn mul(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut o = [0f32; 16];
    for r in 0..4 {
        for c in 0..4 {
            o[r * 4 + c] = (0..4).map(|k| a[r * 4 + k] * b[k * 4 + c]).sum();
        }
    }
    o
}

/// Point through a row-major, row-vector matrix.
pub fn xform(m: &[f32; 16], p: [f32; 3]) -> [f32; 3] {
    [
        p[0] * m[0] + p[1] * m[4] + p[2] * m[8] + m[12],
        p[0] * m[1] + p[1] * m[5] + p[2] * m[9] + m[13],
        p[0] * m[2] + p[1] * m[6] + p[2] * m[10] + m[14],
    ]
}
