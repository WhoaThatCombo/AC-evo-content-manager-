//! TV cameras: AC1 `data\cameras.ini` (set 1) and `data\cameras_1.ini`
//! (set 2) -> `containers\tv1_cameras_<L>.scene`, `tv2_cameras_<L>.scene`
//! and one FOV curve per camera in `cameras\`.
//!
//! # Format (stock, 57 TV containers on 17 tracks)
//!
//! Camera entities, kinds `Camera`, `CameraTV`, `TV_set1` | `TV_set2`;
//! transform = camera position and Euler rotation. A camera looks along
//! its local -Z (stock: the practice sequence's fixed-point camera GP_09 at
//! brands_hatch aims along -Z at its target_point, the car-relative
//! sequence cameras look back at the car along -Z, monza tv2 12 of 12 face
//! the track with -Z; AC1 FORWARD becomes -Z);
//! bounds carry only the active distance. f50.f102 CameraData:
//! - f12 {} (additional properties), f14 {f21 graphics override};
//! - f15 CamTVData {f1 start segment, f2 end segment, f3 use segments 1,
//!   f6 FOV curve, f9 spring mode 1, f10 `tvcam1.tvcamsettings`}; a
//!   segment is {f1 position on the track, f2 width, f3 height, f4 angle}:
//!   start 30 x 10, end 10 x 5 on 1,155 of 1,178 stock cameras; the angle
//!   is the track heading atan2(fx, fz) in degrees folded into -90..90
//!   (the gate line has no direction; checked on monza's tv1 / tv2).
//!   A camera's end segment is the next camera's start: the camera is
//!   live while the car is between them.
//! - f16 {f3 1, f4 {}}.
//! Overrides / curves stock uses most: set 1
//! `dof_tvcam_pcot_for300_back600` + `cameracurve_fov_TV1`, set 2
//! `dof_TV2_for_FOVTV2_700` + `cameracurve_fov_TV2_1.5-700m_25-40m`.
//! A `.curve` is repeated keys {f1 x (distance to the car, m), f3 / f4
//! tangents (empty = flat), f10 y (FOV, degrees)}.
//! The containers are listed only in the No Game Mode row.
//!
//! # Ours
//!
//! - IN_POINT / OUT_POINT (lap fractions of the AC1 fast lane, which our
//!   ideal line follows point for point) are mapped onto the ideal line.
//! - FOV curve: the camera keeps the car the same size on screen (FOV
//!   proportional to 1 / distance) from MAX_FOV at the nearest point of
//!   its stretch to MIN_FOV at the farthest, clamped to MIN..MAX, as 6 keys
//!   spaced geometrically, plus flat ends.

use super::ai::{self, Line};
use super::scene::{euler_of, v3, Entity, SceneBuilder};
use crate::ac1::ini::Ini;
use crate::io::pb::{Msg, Val};

pub const SET1_OVERRIDE: &str = "content\\tracks\\common_assets\\camera_settings\\dof_tvcam_pcot_for300_back600.graphicsoverride";
pub const SET2_OVERRIDE: &str = "content\\tracks\\common_assets\\camera_settings\\dof_TV2_for_FOVTV2_700.graphicsoverride";
pub const SETTINGS: &str = "content\\tracks\\common_assets\\camera_settings\\tvcam1.tvcamsettings";

#[derive(Clone, Debug)]
pub struct Cam {
    pub name: String,
    pub pos: [f32; 3],
    pub fwd: [f32; 3],
    pub up: [f32; 3],
    pub min_fov: f32,
    pub max_fov: f32,
    pub in_point: f32,
    pub out_point: f32,
}

pub fn read(ini: &Ini) -> Vec<Cam> {
    let mut out = Vec::new();
    for (sec, s) in ini.with_prefix("CAMERA_") {
        let v3 = |k: &str| -> Option<[f32; 3]> {
            let n = crate::ac1::ini::nums_of(s.get(k)?);
            (n.len() >= 3 && n.iter().all(|x| x.is_finite())).then(|| [n[0] as f32, n[1] as f32, n[2] as f32])
        };
        let f = |k: &str| s.get(k).and_then(|v| crate::ac1::ini::num_of(v)).map(|x| x as f32);
        let (Some(pos), Some(fwd)) = (v3("POSITION"), v3("FORWARD")) else { continue };
        let (Some(i), Some(o)) = (f("IN_POINT"), f("OUT_POINT")) else { continue };
        let a = f("MIN_FOV").unwrap_or(10.0).clamp(0.5, 120.0);
        let b = f("MAX_FOV").unwrap_or(a).clamp(0.5, 120.0);
        out.push(Cam {
            name: s.get("NAME").cloned().unwrap_or_else(|| sec.to_string()), pos, fwd, up: v3("UP").unwrap_or([0.0, 1.0, 0.0]),
            min_fov: a.min(b), max_fov: a.max(b), in_point: i.clamp(0.0, 1.0), out_point: o.clamp(0.0, 1.0),
        });
    }
    out
}

/// Heading folded into -90..90 degrees (the stock segment angle).
pub fn gate_angle(f: [f32; 3]) -> f32 {
    let mut h = f[0].atan2(f[2]).to_degrees();
    while h > 90.0 {
        h -= 180.0;
    }
    while h <= -90.0 {
        h += 180.0;
    }
    h
}

fn segment(s: &ai::Sample, w: f32, h: f32) -> Msg {
    let mut m = Msg::default();
    m.push(1, Val::Msg(v3(s.pos)));
    m.set_f32(2, w);
    m.set_f32(3, h);
    let a = gate_angle(s.fwd);
    if a != 0.0 {
        m.set_f32(4, a);
    }
    m
}

/// Distances from the camera to its stretch of the line (min, max).
fn reach(c: &Cam, line: &Line) -> (f32, f32) {
    let (a, b) = (c.in_point * line.length, c.out_point * line.length);
    let mut near = f32::INFINITY;
    let mut far = 0.0f32;
    for s in &line.samples {
        let inside = if a <= b { s.dist >= a && s.dist <= b } else { s.dist >= a || s.dist <= b };
        if inside {
            let d = ai::dist3(s.pos, c.pos);
            near = near.min(d);
            far = far.max(d);
        }
    }
    if !near.is_finite() {
        let d = ai::dist3(line.at(a).pos, c.pos);
        return (d.max(5.0), d.max(5.0) * 2.0);
    }
    (near.max(2.0), far.max(near.max(2.0) + 1.0))
}

/// The FOV curve (ours, see the module header).
pub fn curve(min_fov: f32, max_fov: f32, near: f32, far: f32) -> Vec<u8> {
    let key = |x: f32, y: f32| {
        let mut k = Msg::default();
        if x != 0.0 {
            k.set_f32(1, x);
        }
        k.push(3, Val::Msg(Msg::default()));
        k.push(4, Val::Msg(Msg::default()));
        k.set_f32(10, y);
        k
    };
    let mut m = Msg::default();
    m.push(1, Val::Msg(key(0.0, max_fov)));
    // FOV at distance d keeps the car's angular size: tan(fov/2) * d = const
    let k = (max_fov.to_radians() * 0.5).tan() * near;
    let fov_at = |d: f32| (2.0 * (k / d).atan()).to_degrees().clamp(min_fov, max_fov);
    let n = 6;
    for i in 0..n {
        let d = near * (far / near).powf(i as f32 / (n - 1) as f32);
        m.push(1, Val::Msg(key(d, fov_at(d))));
    }
    m.push(1, Val::Msg(key(far * 2.0 + 10.0, fov_at(far))));
    m.encode()
}

pub struct Written {
    pub stems: Vec<String>,
    /// (path relative to the track folder, bytes)
    pub files: Vec<(String, Vec<u8>)>,
    pub cameras: usize,
}

/// Write the TV containers of layout `stem` (AC1 `data` inis of that layout).
pub fn write(b: &mut SceneBuilder, track: &str, stem: &str, data: &std::collections::HashMap<String, Ini>, line: &Line) -> Written {
    let mut w = Written { stems: Vec::new(), files: Vec::new(), cameras: 0 };
    if line.samples.len() < 2 {
        return w;
    }
    for (set, file) in [(1, "cameras.ini"), (2, "cameras_1.ini")] {
        let Some(ini) = data.get(file) else { continue };
        let cams = read(ini);
        if cams.is_empty() {
            continue;
        }
        let c = format!("tv{set}_cameras_{stem}");
        for (i, cam) in cams.iter().enumerate() {
            let s0 = line.at(cam.in_point * line.length);
            let s1 = line.at(cam.out_point * line.length);
            let (near, far) = reach(cam, line);
            let curve_rel = format!("cameras\\tv{set}_{stem}_{i:02}.curve");
            w.files.push((curve_rel.clone(), curve(cam.min_fov, cam.max_fov, near, far)));
            let mut base = Msg::default();
            base.set_str(21, if set == 1 { SET1_OVERRIDE } else { SET2_OVERRIDE });
            let mut tv = Msg::default();
            tv.push(1, Val::Msg(segment(s0, 30.0, 10.0)));
            tv.push(2, Val::Msg(segment(s1, 10.0, 5.0)));
            tv.set_varint(3, 1);
            tv.set_str(6, &format!("content\\tracks\\{track}\\{curve_rel}"));
            tv.set_varint(9, 1);
            tv.set_str(10, SETTINGS);
            let mut seq = Msg::default();
            seq.set_varint(3, 1);
            seq.push(4, Val::Msg(Msg::default()));
            let mut cd = Msg::default();
            cd.push(12, Val::Msg(Msg::default()));
            cd.push(14, Val::Msg(base));
            cd.push(15, Val::Msg(tv));
            cd.push(16, Val::Msg(seq));
            let mut e = Entity::new(&format!("tv{set}_camera_{:02}", i + 1), "Camera", 102, cd);
            e.kinds.push("CameraTV".into());
            e.kinds.push(format!("TV_set{set}"));
            e.position = cam.pos;
            e.rotation = euler_of([-cam.fwd[0], -cam.fwd[1], -cam.fwd[2]], cam.up);
            b.add(Some(&c), e);
            w.cameras += 1;
        }
        w.stems.push(c);
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angle_and_curve() {
        // stock monza tv1_camera_01: heading 174.4 -> -5.6
        let h = 174.4f32.to_radians();
        assert!((gate_angle([h.sin(), 0.0, h.cos()]) + 5.6).abs() < 0.01);
        let c = Msg::parse(&curve(2.0, 8.0, 20.0, 400.0)).unwrap();
        let ys: Vec<f32> = c.subs(1).iter().map(|k| k.f32(10).unwrap()).collect();
        assert_eq!(ys[0], 8.0);
        assert!(ys.windows(2).all(|w| w[1] <= w[0]) && *ys.last().unwrap() >= 2.0, "{ys:?}");
        let ini = crate::ac1::ini::parse("[CAMERA_0]\nNAME=1\nPOSITION=58.9 ,-0.98 ,-467.7\nFORWARD=-0.91 ,-0.01 ,0.40\nUP=0,1,0\nMIN_FOV=2\nMAX_FOV=8\nIN_POINT=0.95\nOUT_POINT=0.1\n");
        let cams = read(&ini);
        assert_eq!(cams.len(), 1);
        assert_eq!((cams[0].min_fov, cams[0].max_fov, cams[0].in_point), (2.0, 8.0, 0.95));
    }
}
