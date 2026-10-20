//! Scene files: the main `<id>.scene` and its `containers\*.scene`, and the
//! builder every track stage adds entities through.
//!
//! Layout (ACECM `protos\Scene.proto.txt`, checked on stock scenes):
//! - SceneData: f1 tags (every kind string its entities carry), f2 entities
//!   (SceneActorData), f3 graphics settings;
//! - SceneActorData: f1 name, f2 enabled, f3 kind strings (the entity's own
//!   kind first; a Container lists the kinds its file holds after it), f4
//!   transform {f1 position, f2 rotation (Euler degrees, see
//!   [`euler_of`]), f3 scale}, f12 guid (fixed64), f13 bounds {f1 xz min,
//!   f2 xz max, f3 active distance}, f14 active-distance override, f50
//!   actor data (one field per kind: f100 SMesh, f104 Start Pos, f109
//!   Surface Def, f111 Track Info, f112 Zone, f108 Spline, f121 Container).
//! - Stock writes the transform's position / rotation even when zero (empty
//!   messages) and scale (1, 1, 1); bounds carry active distance 250 on
//!   almost every entity. We do the same.
//!
//! The game only loads the entity kinds a Container entity declares
//! (`Container` + kinds); `finish` declares exactly what each file holds.

use indexmap::IndexMap;

use super::ids::Ids;
use crate::io::pb::{Msg, Val};

/// Stock's active distance on nearly every entity.
pub const ACTIVE_DISTANCE: f32 = 250.0;

pub fn v3(v: [f32; 3]) -> Msg {
    let mut m = Msg::default();
    for (i, x) in v.iter().enumerate() {
        if *x != 0.0 {
            m.set_f32(i as u32 + 1, *x);
        }
    }
    m
}

pub fn v2(v: [f32; 2]) -> Msg {
    let mut m = Msg::default();
    for (i, x) in v.iter().enumerate() {
        if *x != 0.0 {
            m.set_f32(i as u32 + 1, *x);
        }
    }
    m
}

/// TransformData {f1 position, f2 rotation, f3 scale}.
pub fn transform(pos: [f32; 3], rot: [f32; 3], scale: [f32; 3]) -> Msg {
    let mut t = Msg::default();
    t.push(1, Val::Msg(v3(pos)));
    t.push(2, Val::Msg(v3(rot)));
    t.push(3, Val::Msg(v3(scale)));
    t
}

/// Euler angles (degrees) of a rotation, in the convention stock scenes
/// use: R = Rz(z) * Ry(y) * Rx(x) acting on column vectors (X applied
/// first), Y-up, right-handed; a car's forward is local +Z. Found on stock
/// spawn points: a pit box facing 102.8 degrees is stored (-176.8, 77.2,
/// -177.75), the pit slot in front of it (0.65, 11.95, -1.4), and the
/// pitch / roll this convention gives both match the pit-lane slope.
/// `fwd` / `up` are world vectors (they are orthonormalised here).
pub fn euler_of(fwd: [f32; 3], up: [f32; 3]) -> [f32; 3] {
    let f = super::scan::norm3(fwd);
    let mut u = super::scan::norm3(up);
    // up orthogonal to forward
    let d = f[0] * u[0] + f[1] * u[1] + f[2] * u[2];
    u = super::scan::norm3([u[0] - d * f[0], u[1] - d * f[1], u[2] - d * f[2]]);
    // local +X = up x forward (right-handed: X, Y, Z = left, up, forward)
    let x = [u[1] * f[2] - u[2] * f[1], u[2] * f[0] - u[0] * f[2], u[0] * f[1] - u[1] * f[0]];
    // columns of R are the world images of the local axes
    let r = |row: usize, col: usize| -> f32 { [x, u, f][col][row] };
    let sy = (-r(2, 0)).clamp(-1.0, 1.0);
    let ry = sy.asin();
    let (rx, rz) = if sy.abs() < 0.99999 {
        (r(2, 1).atan2(r(2, 2)), r(1, 0).atan2(r(0, 0)))
    } else {
        (0.0, (-r(0, 1)).atan2(r(1, 1)))
    };
    let deg = |a: f32| {
        let d = a.to_degrees();
        if d.abs() < 1e-4 { 0.0 } else { d }
    };
    [deg(rx), deg(ry), deg(rz)]
}

/// The rotation matrix (column vectors) of stock Euler angles, for tests
/// and for stages that read stock transforms.
pub fn matrix_of(e: [f32; 3]) -> [[f32; 3]; 3] {
    let (x, y, z) = (e[0].to_radians(), e[1].to_radians(), e[2].to_radians());
    let rx = [[1.0, 0.0, 0.0], [0.0, x.cos(), -x.sin()], [0.0, x.sin(), x.cos()]];
    let ry = [[y.cos(), 0.0, y.sin()], [0.0, 1.0, 0.0], [-y.sin(), 0.0, y.cos()]];
    let rz = [[z.cos(), -z.sin(), 0.0], [z.sin(), z.cos(), 0.0], [0.0, 0.0, 1.0]];
    let m = |a: [[f32; 3]; 3], b: [[f32; 3]; 3]| {
        let mut o = [[0f32; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                o[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
            }
        }
        o
    };
    m(m(rz, ry), rx)
}

#[derive(Clone, Debug)]
pub struct Entity {
    pub name: String,
    /// own kind first, then sub-tags (`center_spline`, `timelines`, ...)
    pub kinds: Vec<String>,
    pub position: [f32; 3],
    pub rotation: [f32; 3],
    pub scale: [f32; 3],
    /// xz bounds (min, max); None writes only the active distance
    pub bounds: Option<([f32; 2], [f32; 2])>,
    pub active_distance: f32,
    pub distance_override: Option<f32>,
    /// f50 field number and its data
    pub actor: (u32, Msg),
}

impl Entity {
    pub fn new(name: &str, kind: &str, field: u32, data: Msg) -> Entity {
        Entity {
            name: name.to_string(), kinds: vec![kind.to_string()], position: [0.0; 3], rotation: [0.0; 3], scale: [1.0; 3],
            bounds: None, active_distance: ACTIVE_DISTANCE, distance_override: None, actor: (field, data),
        }
    }
    pub fn kind(&self) -> &str {
        &self.kinds[0]
    }
    fn encode(&self, id: u64) -> Msg {
        let mut e = Msg::default();
        e.set_str(1, &self.name);
        e.set_varint(2, 1);
        for k in &self.kinds {
            e.push(3, Val::Bytes(k.as_bytes().to_vec()));
        }
        e.push(4, Val::Msg(transform(self.position, self.rotation, self.scale)));
        e.push(12, Val::F64(id.to_le_bytes()));
        let mut b = Msg::default();
        if let Some((lo, hi)) = self.bounds {
            b.push(1, Val::Msg(v2(lo)));
            b.push(2, Val::Msg(v2(hi)));
        }
        b.set_f32(3, self.active_distance);
        e.push(13, Val::Msg(b));
        if let Some(d) = self.distance_override {
            e.set_f32(14, d);
        }
        let mut a = Msg::default();
        a.push(self.actor.0, Val::Msg(self.actor.1.clone()));
        e.push(50, Val::Msg(a));
        e
    }
}

#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub entities: Vec<(u64, Entity)>,
    pub graphics: Option<Msg>,
}

impl Scene {
    /// Every kind string, in first-seen order.
    pub fn tags(&self) -> Vec<String> {
        let mut t: Vec<String> = Vec::new();
        for (_, e) in &self.entities {
            for k in &e.kinds {
                if !t.contains(k) {
                    t.push(k.clone());
                }
            }
        }
        t
    }
    pub fn encode(&self) -> Vec<u8> {
        let mut s = Msg::default();
        for t in self.tags() {
            s.push(1, Val::Bytes(t.into_bytes()));
        }
        for (id, e) in &self.entities {
            s.push(2, Val::Msg(e.encode(*id)));
        }
        if let Some(g) = &self.graphics {
            s.push(3, Val::Msg(g.clone()));
        }
        s.encode()
    }
}

pub struct ContainerScene {
    pub scene: Scene,
    /// tags the Container entity declares beyond its entities' own kinds
    /// (zone instance types: `timelines`, `pitlane`, ...; stock declares them
    /// on the Container while the Zone entities carry only `Zone`)
    pub extra: Vec<String>,
    /// loaded through the declaration rows (stock `loaded_on_demand` = 1);
    /// false = always loaded with the main scene
    pub on_demand: bool,
}

/// Collects the entities of the main scene and its containers.
pub struct SceneBuilder {
    pub track: String,
    pub ids: Ids,
    pub main: Scene,
    /// container stem -> scene (in creation order)
    pub containers: IndexMap<String, ContainerScene>,
}

impl SceneBuilder {
    pub fn new(track: &str) -> SceneBuilder {
        SceneBuilder { track: track.to_string(), ids: Ids::new(track), main: Scene::default(), containers: IndexMap::new() }
    }

    pub fn container_path(&self, stem: &str) -> String {
        format!("content\\tracks\\{}\\containers\\{stem}.scene", self.track)
    }

    /// Make sure container `stem` exists.
    pub fn container(&mut self, stem: &str, on_demand: bool) {
        self.containers.entry(stem.to_string()).or_insert(ContainerScene { scene: Scene::default(), extra: Vec::new(), on_demand });
    }

    /// Declare an extra tag on container `stem` (created when missing).
    pub fn declare(&mut self, stem: &str, tag: &str) {
        self.container(stem, true);
        let c = &mut self.containers[stem];
        if !c.extra.iter().any(|t| t == tag) {
            c.extra.push(tag.to_string());
        }
    }

    /// Add an entity to the main scene (`None`) or a container; returns its id.
    pub fn add(&mut self, target: Option<&str>, e: Entity) -> u64 {
        let id = self.ids.unique(e.kind(), &format!("{}/{}", target.unwrap_or(""), e.name));
        match target {
            None => self.main.entities.push((id, e)),
            Some(c) => {
                self.container(c, true);
                self.containers[c].scene.entities.push((id, e));
            }
        }
        id
    }

    /// Encode everything: (archive path relative to the track folder, bytes).
    /// The main scene gets one Container entity per container, declaring the
    /// kinds that container holds.
    pub fn finish(mut self) -> Vec<(String, Vec<u8>)> {
        let mut out = Vec::new();
        let stems: Vec<String> = self.containers.keys().cloned().collect();
        for stem in &stems {
            let c = &self.containers[stem];
            let mut kinds = vec!["Container".to_string()];
            kinds.extend(c.scene.tags());
            for t in &c.extra {
                if !kinds.contains(t) {
                    kinds.push(t.clone());
                }
            }
            let mut d = Msg::default();
            d.set_str(1, &self.container_path(stem));
            if c.on_demand {
                d.set_varint(3, 1);
            }
            let mut e = Entity::new(stem, "Container", 121, d);
            e.kinds = kinds;
            let id = self.ids.of("Container", stem);
            self.main.entities.push((id, e));
            out.push((format!("containers\\{stem}.scene"), c.scene.encode()));
        }
        out.insert(0, (format!("{}.scene", self.track), self.main.encode()));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(m: [[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
        [0, 1, 2].map(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
    }

    #[test]
    fn euler_round_trip() {
        for (fwd, up) in [([0.0, 0.0, 1.0], [0.0, 1.0, 0.0]), ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]), ([-0.3, 0.05, -0.95], [0.02, 1.0, 0.03]), ([0.97, -0.02, -0.22], [0.0, 1.0, 0.01])] {
            let e = euler_of(fwd, up);
            let m = matrix_of(e);
            let f = apply(m, [0.0, 0.0, 1.0]);
            let n = super::super::scan::norm3(fwd);
            assert!((0..3).all(|k| (f[k] - n[k]).abs() < 1e-3), "{fwd:?} -> {e:?} -> {f:?}");
        }
        // stock pit box (-176.8, 77.2, -177.75): forward is about 102.8 degrees of heading
        let f = apply(matrix_of([-176.8, 77.2, -177.75]), [0.0, 0.0, 1.0]);
        let heading = f[0].atan2(f[2]).to_degrees();
        assert!((heading - 102.6).abs() < 1.0, "{heading}");
        // and a plain heading encodes like stock: yaw 102.8 -> (180, 77.2, 180)
        let h = 102.8f32.to_radians();
        let e = euler_of([h.sin(), 0.0, h.cos()], [0.0, 1.0, 0.0]);
        assert!((e[0].abs() - 180.0).abs() < 0.01 && (e[1] - 77.2).abs() < 0.01 && (e[2].abs() - 180.0).abs() < 0.01, "{e:?}");
    }

    #[test]
    fn containers_declare_their_kinds() {
        let mut b = SceneBuilder::new("t");
        b.add(Some("layout_x"), Entity::new("road", "SMesh", 100, Msg::default()));
        let mut sp = Entity::new("center", "Spline", 108, Msg::default());
        sp.kinds.push("center_spline".into());
        b.add(Some("layout_x"), sp);
        let files = b.finish();
        let main = Msg::parse(&files[0].1).unwrap();
        let c = main.subs(2).into_iter().find(|e| e.str(1).as_deref() == Some("layout_x")).unwrap();
        let kinds: Vec<String> = c.all(3).filter_map(|v| if let Val::Bytes(b) = v { Some(String::from_utf8_lossy(b).into_owned()) } else { None }).collect();
        assert_eq!(kinds, ["Container", "SMesh", "Spline", "center_spline"]);
        assert_eq!(files[1].0, "containers\\layout_x.scene");
    }
}
