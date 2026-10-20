//! AC1 kn5 visual geometry -> the EVO exterior and interior meshes.
//!
//! 1. `flatten`: every renderable mesh node of the kn5 with its world matrix
//!    and its place in the tree (`Inst`). Dropped: inactive subtrees (the
//!    low-res cockpit, damage glass; the rim-blur subtrees are kept and
//!    flagged), invisible / non-renderable meshes, meshes with non-finite
//!    positions or a broken node transform.
//! 2. `route`: which mesh (exterior / interior), which section and which bone
//!    each instance goes to (`Route`), from the tree position and the
//!    material kind (`materials::kind_of`). Wheel parts (rims, tyres, blur)
//!    go to the wheel stage instead.
//! 3. `Built`: the vertices of each section in EVO car space (frame datum),
//!    rigidly skinned to one bone; `lods` adds the simplified levels; `encode`
//!    writes the `.mesh` (layout: docs/FORMATS.md, ACECM protos/Mesh.proto).
//!
//! Double-sided modelling: AC1 models a double-sided surface as two
//! coincident triangles with opposite winding. Stock car materials never
//! disable culling (`cullMode` is 0 on every stock car material), so the pair
//! is kept and each copy is pushed 0.4 mm out along its own normal (ours):
//! no coincident depth, no self-shadowing between the copies.

use std::collections::{BTreeMap, HashMap};

use crate::ac1::kn5::{Kn5, Mesh, Node, NodeKind};
use crate::car::frame::{self, M4};
use crate::io::pb::{self, Msg, Val};

/// A kn5 mesh node placed in the car.
pub struct Inst<'k> {
    /// '/'-joined node names from the root (root excluded), the mesh node last
    pub path: String,
    pub node: String,
    pub mesh: &'k Mesh<'k>,
    /// AC1 world matrix (no datum)
    pub world: M4,
    pub material: usize,
    /// the wheel corner the node hangs under (WHEEL_xx / SUSP_xx / ...)
    pub corner: Option<&'static str>,
    pub under: Under,
    /// inside a RIM_BLUR_xx subtree (inactive in AC1 at rest)
    pub blur: bool,
    /// under COCKPIT_HR or STEER_HR
    pub cockpit: bool,
    /// under STEER_HR
    pub steer: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Under {
    Body,
    /// under WHEEL_xx: spins with the wheel
    Wheel,
    /// under SUSP_xx (or HUB_xx): steers, does not spin
    Susp,
    /// under a DISC_xx node (AC1's brake rotors sit beside WHEEL_xx at the
    /// root and turn with the wheel): the DISC_xx bone, as stock EXT_DISC
    Disc,
}

fn seg_eq(s: &str, a: &str) -> bool {
    s.eq_ignore_ascii_case(a)
}

/// Every renderable mesh instance of the kn5.
pub fn flatten<'k>(k: &'k Kn5<'k>, notes: &mut Vec<String>) -> Vec<Inst<'k>> {
    struct Ctx {
        corner: Option<&'static str>,
        under: Under,
        blur: bool,
        cockpit: bool,
        steer: bool,
    }
    fn walk<'k>(n: &'k Node<'k>, parent: &M4, path: &str, root: bool, c: &Ctx, out: &mut Vec<Inst<'k>>, dropped: &mut usize) {
        let up = n.name.to_ascii_uppercase();
        let is_blur = up.starts_with("RIM_BLUR");
        // inactive subtrees are not shown by AC1 (except the rim blur, which
        // AC1 switches on with speed)
        if !n.active && !is_blur && !root {
            return;
        }
        if seg_eq(&n.name, "COCKPIT_LR") || up.starts_with("DAMAGE_") {
            return;
        }
        let world = match &n.kind {
            NodeKind::Transform(m) => frame::mul(&frame::m64(m), parent),
            _ => *parent,
        };
        let p = if root { String::new() } else if path.is_empty() { n.name.clone() } else { format!("{path}/{}", n.name) };
        let mut cc = Ctx { corner: c.corner, under: c.under, blur: c.blur || is_blur, cockpit: c.cockpit, steer: c.steer };
        if let NodeKind::Transform(_) = &n.kind {
            if let Some(corner) = frame::corner_of(&n.name) {
                if up.starts_with("WHEEL_") {
                    cc.corner = Some(corner);
                    cc.under = Under::Wheel;
                } else if (up.starts_with("SUSP_") || up.starts_with("HUB_")) && cc.under != Under::Wheel {
                    cc.corner = Some(corner);
                    cc.under = Under::Susp;
                } else if up.starts_with("DISC_") && cc.under == Under::Body {
                    cc.corner = Some(corner);
                    cc.under = Under::Disc;
                }
            }
            if seg_eq(&n.name, "COCKPIT_HR") {
                cc.cockpit = true;
            }
            if seg_eq(&n.name, "STEER_HR") {
                cc.cockpit = true;
                cc.steer = true;
            }
        }
        if let NodeKind::Mesh(m) = &n.kind {
            let ok = m.renderable && m.visible && m.vertex_count > 0 && m.index_count() >= 3 && world.iter().all(|x| x.is_finite())
                && (0..m.vertex_count).all(|v| m.position(v).iter().all(|x| x.is_finite()));
            if ok {
                out.push(Inst {
                    path: p.clone(), node: n.name.clone(), mesh: m, world, material: m.material_id.max(0) as usize,
                    corner: cc.corner, under: cc.under, blur: cc.blur, cockpit: cc.cockpit, steer: cc.steer,
                });
            } else if m.vertex_count > 0 && m.visible && m.renderable {
                *dropped += 1;
            }
        }
        for ch in &n.children {
            walk(ch, &world, &p, false, &cc, out, dropped);
        }
    }
    let mut out = Vec::new();
    let mut dropped = 0;
    walk(&k.root, &frame::I4, "", true, &Ctx { corner: None, under: Under::Body, blur: false, cockpit: false, steer: false }, &mut out, &mut dropped);
    if dropped > 0 {
        notes.push(format!("geometry: {dropped} mesh(es) with non-finite positions or transforms dropped"));
    }
    out
}

// ------------------------------------------------------------- vertices --

#[derive(Clone, Copy, Default, PartialEq)]
pub struct Vert {
    pub pos: [f32; 3],
    pub nrm: [f32; 3],
    pub uv: [f32; 2],
    /// texcoords1 (zero except on mirrors: their render-target UVs)
    pub uv1: [f32; 2],
    pub tan: [f32; 4],
    pub bone: u16,
}

impl meshopt::DecodePosition for Vert {
    fn decode_position(&self) -> [f32; 3] {
        self.pos
    }
}

/// One section's geometry (its own vertex list, local indices).
#[derive(Clone, Default)]
pub struct Geo {
    pub verts: Vec<Vert>,
    pub idx: Vec<u32>,
}

impl Geo {
    pub fn tris(&self) -> usize {
        self.idx.len() / 3
    }
}

fn norm3(v: [f64; 3]) -> [f64; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 1e-12 && l.is_finite() { [v[0] / l, v[1] / l, v[2] / l] } else { [0.0, 1.0, 0.0] }
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn det3(m: &M4) -> f64 {
    m[0] * (m[5] * m[10] - m[6] * m[9]) - m[1] * (m[4] * m[10] - m[6] * m[8]) + m[2] * (m[4] * m[9] - m[5] * m[8])
}

/// The normal matrix (inverse transpose of the 3x3 part) for row vectors.
fn normal_matrix(m: &M4) -> M4 {
    let mut r = *m;
    r[12] = 0.0;
    r[13] = 0.0;
    r[14] = 0.0;
    match frame::inv(&r) {
        Some(i) => {
            // transpose of the inverse
            let mut t = frame::I4;
            for a in 0..3 {
                for b in 0..3 {
                    t[a * 4 + b] = i[b * 4 + a];
                }
            }
            t
        }
        None => r,
    }
}

/// Append one instance to a section, transformed by `m` (AC1 world -> the
/// target space, e.g. with the datum), skinned to `bone`.
pub fn append(g: &mut Geo, mesh: &Mesh, m: &M4, bone: u16) {
    let base = g.verts.len() as u32;
    let nm = normal_matrix(m);
    let flip = det3(m) < 0.0;
    let n = mesh.vertex_count;
    for v in 0..n {
        let p = mesh.position(v);
        let q = frame::xform(m, [p[0] as f64, p[1] as f64, p[2] as f64]);
        let nr = mesh.normal(v);
        let nn = norm3(frame::xdir(&nm, [nr[0] as f64, nr[1] as f64, nr[2] as f64]));
        let t = mesh.tangent(v);
        let tt = frame::xdir(m, [t[0] as f64, t[1] as f64, t[2] as f64]);
        // Gram-Schmidt against the normal
        let tt = norm3([tt[0] - nn[0] * dot(tt, nn), tt[1] - nn[1] * dot(tt, nn), tt[2] - nn[2] * dot(tt, nn)]);
        let uv = mesh.uv(v);
        g.verts.push(Vert {
            pos: [q[0] as f32, q[1] as f32, q[2] as f32],
            nrm: [nn[0] as f32, nn[1] as f32, nn[2] as f32],
            uv,
            uv1: [0.0; 2],
            tan: [tt[0] as f32, tt[1] as f32, tt[2] as f32, 1.0],
            bone,
        });
    }
    let ic = mesh.index_count() / 3 * 3;
    for t in (0..ic).step_by(3) {
        let (a, b, c) = (mesh.index(t) as usize, mesh.index(t + 1) as usize, mesh.index(t + 2) as usize);
        if a >= n || b >= n || c >= n || a == b || b == c || a == c {
            continue;
        }
        if flip {
            g.idx.extend_from_slice(&[base + a as u32, base + c as u32, base + b as u32]);
        } else {
            g.idx.extend_from_slice(&[base + a as u32, base + b as u32, base + c as u32]);
        }
    }
    fix_handedness(g, base as usize);
}

/// Tangent w (bitangent sign) from the triangles' UV orientation, for the
/// vertices from `from` on.
fn fix_handedness(g: &mut Geo, from: usize) {
    let n = g.verts.len() - from;
    if n == 0 {
        return;
    }
    let mut acc = vec![0f64; n];
    let start = g.idx.iter().position(|&i| i as usize >= from).unwrap_or(g.idx.len()) / 3 * 3;
    for t in g.idx[start..].chunks_exact(3) {
        let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
        let (va, vb, vc) = (g.verts[a], g.verts[b], g.verts[c]);
        let e1 = [(vb.pos[0] - va.pos[0]) as f64, (vb.pos[1] - va.pos[1]) as f64, (vb.pos[2] - va.pos[2]) as f64];
        let e2 = [(vc.pos[0] - va.pos[0]) as f64, (vc.pos[1] - va.pos[1]) as f64, (vc.pos[2] - va.pos[2]) as f64];
        let (du1, dv1) = ((vb.uv[0] - va.uv[0]) as f64, (vb.uv[1] - va.uv[1]) as f64);
        let (du2, dv2) = ((vc.uv[0] - va.uv[0]) as f64, (vc.uv[1] - va.uv[1]) as f64);
        let r = du1 * dv2 - du2 * dv1;
        if r.abs() < 1e-20 {
            continue;
        }
        // bitangent direction (dP/dv)
        let bt = [(e2[0] * du1 - e1[0] * du2) / r, (e2[1] * du1 - e1[1] * du2) / r, (e2[2] * du1 - e1[2] * du2) / r];
        for &i in &[a, b, c] {
            if i < from {
                continue;
            }
            let v = g.verts[i];
            let nn = [v.nrm[0] as f64, v.nrm[1] as f64, v.nrm[2] as f64];
            let tt = [v.tan[0] as f64, v.tan[1] as f64, v.tan[2] as f64];
            acc[i - from] += dot(cross(nn, tt), bt).signum();
        }
    }
    for (k, s) in acc.iter().enumerate() {
        g.verts[from + k].tan[3] = if *s < 0.0 { -1.0 } else { 1.0 };
    }
}

/// Split AC1 double-sided pairs (coincident triangles of opposite winding):
/// each copy gets its own vertices, moved `eps` along its face normal.
/// Returns the number of pairs.
pub fn split_double_sided(g: &mut Geo, eps: f32) -> usize {
    let q = |p: [f32; 3]| [(p[0] * 2000.0).round() as i64, (p[1] * 2000.0).round() as i64, (p[2] * 2000.0).round() as i64];
    let nt = g.tris();
    let mut by_set: HashMap<[[i64; 3]; 3], Vec<usize>> = HashMap::new();
    let key = |g: &Geo, t: usize| {
        let mut k = [q(g.verts[g.idx[t * 3] as usize].pos), q(g.verts[g.idx[t * 3 + 1] as usize].pos), q(g.verts[g.idx[t * 3 + 2] as usize].pos)];
        k.sort();
        k
    };
    for t in 0..nt {
        by_set.entry(key(g, t)).or_default().push(t);
    }
    let face_n = |g: &Geo, t: usize| -> [f64; 3] {
        let p = |i: usize| { let v = g.verts[g.idx[t * 3 + i] as usize].pos; [v[0] as f64, v[1] as f64, v[2] as f64] };
        let (a, b, c) = (p(0), p(1), p(2));
        norm3(cross([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]))
    };
    let mut paired = vec![false; nt];
    let mut pairs = 0;
    for list in by_set.values() {
        if list.len() < 2 {
            continue;
        }
        for i in 0..list.len() {
            for j in i + 1..list.len() {
                let (a, b) = (list[i], list[j]);
                if paired[a] || paired[b] {
                    continue;
                }
                if dot(face_n(g, a), face_n(g, b)) < -0.9 {
                    paired[a] = true;
                    paired[b] = true;
                    pairs += 1;
                }
            }
        }
    }
    if pairs == 0 {
        return 0;
    }
    for t in 0..nt {
        if !paired[t] {
            continue;
        }
        let n = face_n(g, t);
        for k in 0..3 {
            let mut v = g.verts[g.idx[t * 3 + k] as usize];
            v.pos = [v.pos[0] + (n[0] as f32) * eps, v.pos[1] + (n[1] as f32) * eps, v.pos[2] + (n[2] as f32) * eps];
            g.idx[t * 3 + k] = g.verts.len() as u32;
            g.verts.push(v);
        }
    }
    compact(g);
    pairs
}

/// Append `b` to `a` (one section from two).
pub fn merge(a: &mut Geo, b: &Geo) {
    let base = a.verts.len() as u32;
    a.verts.extend_from_slice(&b.verts);
    a.idx.extend(b.idx.iter().map(|i| i + base));
}

/// Drop unreferenced vertices, merge exact duplicates.
pub fn compact(g: &mut Geo) {
    if g.verts.is_empty() {
        return;
    }
    let (count, remap) = meshopt::generate_vertex_remap(&g.verts, Some(&g.idx));
    let mut idx: Vec<u32> = g.idx.iter().map(|&i| remap[i as usize]).collect();
    let mut verts = vec![Vert::default(); count];
    for (i, &r) in remap.iter().enumerate() {
        if r != u32::MAX {
            verts[r as usize] = g.verts[i];
        }
    }
    let v = meshopt::optimize_vertex_fetch(&mut idx, &verts);
    g.verts = v;
    g.idx = idx;
}

/// A simplified copy with about `ratio` of the triangles (never empty: at
/// least one triangle stays). Borders are not locked; a section that will
/// not simplify far enough falls back to the sloppy simplifier.
pub fn simplify(g: &Geo, ratio: f32) -> Geo {
    if ratio >= 0.999 || g.tris() <= 4 {
        return g.clone();
    }
    let target = ((g.idx.len() as f32 * ratio) as usize / 3 * 3).max(3);
    // weld by position for the simplifier (UV seams split vertices)
    let pos: Vec<[f32; 3]> = g.verts.iter().map(|v| v.pos).collect();
    let (_, remap) = meshopt::generate_vertex_remap(&pos, Some(&g.idx));
    let mut first = vec![u32::MAX; g.verts.len()];
    for (i, &r) in remap.iter().enumerate() {
        if r != u32::MAX && first[r as usize] == u32::MAX {
            first[r as usize] = i as u32;
        }
    }
    let welded: Vec<u32> = g.idx.iter().map(|&i| first[remap[i as usize] as usize]).collect();
    let mut err = 0f32;
    let mut out = meshopt::simplify_decoder(&welded, &g.verts, target, 0.05, meshopt::SimplifyOptions::None, Some(&mut err));
    if out.len() > target * 3 / 2 {
        out = meshopt::simplify_sloppy_decoder(&welded, &g.verts, target, 0.2, Some(&mut err));
    }
    if out.len() < 3 {
        out = g.idx[..3].to_vec();
    }
    // back to a vertex of the right UV island: the welded index picked the
    // first vertex at that position; prefer the original corner when the
    // triangle survives unchanged is not tracked, so keep the welded one
    let mut r = Geo { verts: g.verts.clone(), idx: out };
    compact(&mut r);
    r
}

// --------------------------------------------------------------- skeleton --

#[derive(Clone, Debug)]
pub struct Bone {
    pub name: String,
    pub parent: String,
    /// local to the parent (row vectors)
    pub local: M4,
}

pub fn bone_msg(b: &Bone) -> Msg {
    let mut m = Msg::default();
    m.set_str(1, &b.name);
    if !b.parent.is_empty() {
        m.set_str(2, &b.parent);
    }
    let mut mm = Msg::default();
    for (k, v) in b.local.iter().enumerate() {
        let v = *v as f32;
        if v != 0.0 {
            mm.push(k as u32 + 1, Val::F32(v.to_le_bytes()));
        }
    }
    m.push(3, Val::Msg(mm));
    m
}

fn bone_local(m: &Msg) -> [f32; 16] {
    let t = m.sub(3).unwrap_or_default();
    let mut o = [0f32; 16];
    for (i, v) in o.iter_mut().enumerate() {
        *v = t.f32(i as u32 + 1).unwrap_or(0.0);
    }
    o
}

/// World (rest) matrix of every bone message, by parent chain.
pub fn bone_worlds(bones: &[Msg]) -> Vec<[f32; 16]> {
    let names: Vec<String> = bones.iter().map(|b| b.str(1).unwrap_or_default()).collect();
    let parents: Vec<Option<usize>> = bones.iter().enumerate().map(|(i, b)| {
        let p = b.str(2).unwrap_or_default();
        names.iter().position(|n| *n == p).filter(|&j| j != i)
    }).collect();
    let locals: Vec<[f32; 16]> = bones.iter().map(bone_local).collect();
    let mut w: Vec<Option<[f32; 16]>> = vec![None; bones.len()];
    fn get(i: usize, l: &[[f32; 16]], p: &[Option<usize>], w: &mut Vec<Option<[f32; 16]>>, d: usize) -> [f32; 16] {
        if let Some(m) = w[i] {
            return m;
        }
        let m = match p[i] {
            Some(j) if d < 64 => crate::ac1::kn5::mul(&l[i], &get(j, l, p, w, d + 1)),
            _ => l[i],
        };
        w[i] = Some(m);
        m
    }
    (0..bones.len()).map(|i| get(i, &locals, &parents, &mut w, 0)).collect()
}

/// World matrices of a bone list (by parent names).
pub fn worlds_of(bones: &[Bone]) -> BTreeMap<String, M4> {
    let by: HashMap<&str, &Bone> = bones.iter().map(|b| (b.name.as_str(), b)).collect();
    let mut out = BTreeMap::new();
    for b in bones {
        let mut m = b.local;
        let mut p = b.parent.as_str();
        let mut d = 0;
        while let Some(pb) = by.get(p) {
            m = frame::mul(&m, &pb.local);
            p = pb.parent.as_str();
            d += 1;
            if d > 64 {
                break;
            }
        }
        out.insert(b.name.clone(), m);
    }
    out
}

// ------------------------------------------------------------- the mesh --

/// One section of a built mesh.
#[derive(Clone)]
pub struct Section {
    pub name: String,
    pub material: String,
    pub geo: Geo,
}

/// A whole mesh: sections per LOD, one skeleton.
pub struct Built {
    pub bones: Vec<Bone>,
    /// LOD -> sections
    pub lods: Vec<Vec<Section>>,
    /// lodIn per LOD (0 for LOD0)
    pub lod_in: Vec<f32>,
    /// write bone weights / indices (car body meshes; rims and tyres have none)
    pub skinned: bool,
}

fn f32s(v: impl IntoIterator<Item = f32>) -> Vec<u8> {
    let mut o = Vec::new();
    for x in v {
        o.extend_from_slice(&x.to_le_bytes());
    }
    o
}

fn vec3(p: [f32; 3]) -> Msg {
    let mut m = Msg::default();
    for (i, x) in p.iter().enumerate() {
        if *x != 0.0 {
            m.set_f32(i as u32 + 1, *x);
        }
    }
    m
}

impl Built {
    pub fn tris(&self, lod: usize) -> usize {
        self.lods.get(lod).map(|s| s.iter().map(|x| x.geo.tris()).sum()).unwrap_or(0)
    }

    /// Bounds of LOD0 (min, max).
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for s in self.lods.first().into_iter().flatten() {
            for v in &s.geo.verts {
                for k in 0..3 {
                    lo[k] = lo[k].min(v.pos[k]);
                    hi[k] = hi[k].max(v.pos[k]);
                }
            }
        }
        if !lo[0].is_finite() {
            return ([0.0; 3], [0.0; 3]);
        }
        (lo, hi)
    }

    /// The `.mesh` bytes, from a template mesh (its top-level fields other than
    /// the LODs and bounds are kept: visibility, flags, culling).
    pub fn encode(&self, template: Option<&[u8]>) -> Vec<u8> {
        let mut top = template.and_then(|t| Msg::parse(t).ok()).unwrap_or_default();
        top.remove(5);
        top.remove(6);
        top.remove(7);
        if top.get(2).is_none() {
            top.set_varint(2, 1);
        }
        if top.get(3).is_none() {
            top.set_varint(3, 1);
        }
        let (lo, hi) = self.bounds();
        // field order like stock: f2 f3 f5.. f6 f7 then the rest
        let rest: Vec<pb::Field> = top.fields.drain(..).collect();
        let mut out = Msg::default();
        for f in rest.iter().filter(|f| f.num < 5) {
            out.fields.push(f.clone());
        }
        let bones: Vec<Msg> = self.bones.iter().map(bone_msg).collect();
        for (li, secs) in self.lods.iter().enumerate() {
            out.push(5, Val::Msg(self.lod_msg(li, secs, &bones)));
        }
        out.push(6, Val::Msg(vec3(lo)));
        out.push(7, Val::Msg(vec3(hi)));
        for f in rest.into_iter().filter(|f| f.num > 7) {
            out.fields.push(f);
        }
        out.encode()
    }

    fn lod_msg(&self, li: usize, secs: &[Section], bones: &[Msg]) -> Msg {
        let mut m = Msg::default();
        m.set_varint(1, 1);
        let lin = self.lod_in.get(li).copied().unwrap_or(0.0);
        if lin != 0.0 {
            m.set_f32(3, lin);
        }
        let mut verts: Vec<Vert> = Vec::new();
        let mut idx: Vec<u32> = Vec::new();
        for s in secs {
            let base = verts.len() as u32;
            let start = idx.len();
            verts.extend_from_slice(&s.geo.verts);
            idx.extend(s.geo.idx.iter().map(|i| i + base));
            let mut b = Msg::default();
            b.set_str(1, &s.name);
            if start > 0 {
                b.set_varint(2, start as u64);
            }
            b.set_varint(3, s.geo.idx.len() as u64);
            b.set_str(4, &s.material);
            m.push(4, Val::Msg(b));
        }
        let n = verts.len();
        m.push(5, Val::Bytes(f32s(verts.iter().flat_map(|v| v.pos))));
        m.push(6, Val::Bytes(f32s(verts.iter().flat_map(|v| v.nrm))));
        m.push(7, Val::Bytes(f32s(verts.iter().flat_map(|v| v.uv))));
        m.push(8, Val::Bytes(f32s(verts.iter().flat_map(|v| v.tan))));
        if self.skinned {
            m.push(9, Val::Bytes(f32s(verts.iter().flat_map(|_| [1.0f32, 0.0, 0.0, 0.0]))));
            m.push(10, Val::Bytes(f32s(verts.iter().flat_map(|v| [v.bone as f32, 0.0, 0.0, 0.0]))));
        }
        m.push(11, Val::Bytes(pb::varint_bytes(&idx)));
        for b in bones {
            m.push(12, Val::Msg(b.clone()));
        }
        m.push(13, Val::Bytes(vec![0u8; n * 16]));
        m.push(14, Val::Bytes(f32s(verts.iter().flat_map(|v| v.uv1))));
        m.push(15, Val::Bytes(vec![0u8; n * 8]));
        m.push(16, Val::Bytes(vec![0u8; n * 8]));
        let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for v in &verts {
            for k in 0..3 {
                lo[k] = lo[k].min(v.pos[k]);
                hi[k] = hi[k].max(v.pos[k]);
            }
        }
        if !lo[0].is_finite() {
            lo = [0.0; 3];
            hi = [0.0; 3];
        }
        m.push(17, Val::Msg(vec3(lo)));
        m.push(18, Val::Msg(vec3(hi)));
        m
    }
}

/// One degenerate triangle (a mesh that must exist but has nothing to show).
pub fn placeholder(material: &str, name: &str) -> Section {
    let v = Vert { nrm: [0.0, 1.0, 0.0], tan: [1.0, 0.0, 0.0, 1.0], ..Default::default() };
    Section { name: name.to_string(), material: material.to_string(), geo: Geo { verts: vec![v, v, v], idx: vec![0, 1, 2] } }
}

/// The LOD levels of a mesh (ours, calibrated on stock cars: the exterior /
/// interior `.mesh` of every stock car has 5 levels; mx5_na, 992_gt3_rs and
/// the donor switch the exterior at 10 / 20 / 50 / 100 m and the interior at
/// 5 / 10 / 20 / 50 m; their triangle shares per level are about
/// 1 / 0.6-0.85 / 0.2-0.25 / 0.11-0.13 / 0.04-0.05 outside and
/// 1 / 0.38-0.42 / 0.14-0.17 / 0.1-0.12 / 0.06-0.07 inside).
pub struct LodPlan {
    pub lod_in: [f32; 5],
    pub ratio: [f32; 5],
}

pub const EXT_LODS: LodPlan = LodPlan { lod_in: [0.0, 10.0, 20.0, 50.0, 100.0], ratio: [1.0, 0.7, 0.24, 0.12, 0.045] };
pub const INT_LODS: LodPlan = LodPlan { lod_in: [0.0, 5.0, 10.0, 20.0, 50.0], ratio: [1.0, 0.4, 0.16, 0.11, 0.065] };

/// Rims and tyres: two levels, the second at 20 m with about 0.3 of the
/// triangles (the donor's stock rim and tyre meshes).
pub const WHEEL_LODS: LodPlan = LodPlan { lod_in: [0.0, 20.0, 0.0, 0.0, 0.0], ratio: [1.0, 0.3, 0.0, 0.0, 0.0] };

/// Fill LOD1.. of a built mesh by simplifying LOD0 (per section; a section
/// never goes empty). Levels with a zero ratio are not written.
pub fn make_lods(b: &mut Built, plan: &LodPlan) {
    use rayon::prelude::*;
    let lod0 = b.lods[0].clone();
    b.lods.truncate(1);
    b.lod_in = vec![0.0];
    for l in 1..5 {
        if plan.ratio[l] <= 0.0 {
            break;
        }
        let r = plan.ratio[l];
        let secs: Vec<Section> = lod0.par_iter().map(|s| Section { name: s.name.clone(), material: s.material.clone(), geo: simplify(&s.geo, r) }).collect();
        b.lods.push(secs);
        b.lod_in.push(plan.lod_in[l]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad() -> Geo {
        let v = |x: f32, z: f32| Vert { pos: [x, 0.0, z], nrm: [0.0, 1.0, 0.0], uv: [x, z], uv1: [0.0; 2], tan: [1.0, 0.0, 0.0, 1.0], bone: 0 };
        Geo { verts: vec![v(0., 0.), v(1., 0.), v(1., 1.), v(0., 1.)], idx: vec![0, 2, 1, 0, 3, 2] }
    }

    #[test]
    fn double_sided_pairs_split() {
        let mut g = quad();
        let back: Vec<u32> = g.idx.chunks(3).flat_map(|t| [t[0], t[2], t[1]]).collect();
        g.idx.extend(back);
        assert_eq!(split_double_sided(&mut g, 0.0004), 2);
        // copies no longer coincide
        let ys: Vec<f32> = g.verts.iter().map(|v| v.pos[1]).collect();
        assert!(ys.iter().any(|y| *y > 0.0) && ys.iter().any(|y| *y < 0.0));
        assert_eq!(g.tris(), 4);
    }

    #[test]
    fn simplify_never_empty() {
        let g = quad();
        assert!(simplify(&g, 0.01).tris() >= 1);
    }
}
