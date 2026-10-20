//! EVO `.animation` writer.
//!
//! Layout, read off the donor's stock clips with `examples/pbdump.rs`
//! (ks_abarth_695_biposto `abarth_695_biposto_ext_door_l.animation` and
//! `abarth_695_biposto_int_pedal_brake.animation`):
//! - f1 source {f1 source file name (stock: the authoring .fbx path; ours:
//!   `<clip>.ksanim`), f2 frame rate (f32 30)};
//! - f2 take {f1 take name ("Take 001"), f2 frame rate (f32 30), f3
//!   (repeated) track {f1 bone name, f2 (repeated) one 4x4 matrix per frame}};
//! - a matrix is the bone's LOCAL transform as f32 fields 1..16, row-major
//!   with row vectors: the translation is in 13..15 (door_l's DOOR_L hinge
//!   at 0.749 / 0.472 / 0.679), 16 is 1, and zero components are left out
//!   (f4 / f8 / f12 never appear);
//! - every track of a clip has the same frame count (22 on both clips), and
//!   a clip carries its moving bone's parent too, held still (CAR_BODY on
//!   the door, COCKPIT_HR on the pedal).

use crate::car::frame::M4;
use crate::io::pb::{Msg, Val};

pub const FPS: f32 = 30.0;

fn matrix(m: &M4) -> Msg {
    let mut o = Msg::default();
    for (k, v) in m.iter().enumerate() {
        let v = *v as f32;
        if v != 0.0 {
            o.push(k as u32 + 1, Val::F32(v.to_le_bytes()));
        }
    }
    o
}

/// Encode one clip: `tracks` = (bone name, local matrix per frame).
pub fn encode_clip(name: &str, tracks: &[(&str, Vec<M4>)]) -> Vec<u8> {
    let mut src = Msg::default();
    src.set_str(1, &format!("{name}.ksanim"));
    src.set_f32(2, FPS);
    let mut take = Msg::default();
    take.set_str(1, "Take 001");
    take.set_f32(2, FPS);
    for (bone, frames) in tracks {
        let mut t = Msg::default();
        t.set_str(1, bone);
        for f in frames {
            t.push(2, Val::Msg(matrix(f)));
        }
        take.push(3, Val::Msg(t));
    }
    let mut top = Msg::default();
    top.push(1, Val::Msg(src));
    top.push(2, Val::Msg(take));
    top.encode()
}

/// (bone, frame count) of every track of a `.animation`.
pub fn tracks(b: &[u8]) -> Vec<(String, usize)> {
    let Ok(m) = Msg::parse(b) else { return vec![] };
    m.subs(2).iter().flat_map(|take| take.subs(3)).map(|t| (t.str(1).unwrap_or_default(), t.all(2).count())).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn round_trip() {
        let id = crate::car::frame::I4;
        let mut moved = id;
        moved[12] = 0.5;
        let b = super::encode_clip("car_door_l", &[("AC1_DOOR_L", vec![id, moved])]);
        assert_eq!(super::tracks(&b), vec![("AC1_DOOR_L".to_string(), 2)]);
    }
}
