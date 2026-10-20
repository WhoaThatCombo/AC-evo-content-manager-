//! The actor's audio slots (`<id>.actor` f103.f9, AudioEventData: f1 event path, f2 slot type,
//! f5 transform, f6 bank) pointed at the converted events. Slots of roles that were not built
//! keep the donor's (the game's shared events: brakes, wheels, wind, the factory backfire ...).

use anyhow::{anyhow, Result};

use super::map::Role;
use crate::files::Files;
use crate::io::pb::{Msg, Val};

/// The car's own bank, as the actor names it.
pub fn bank_path(id: &str) -> String {
    format!("content\\cars\\{id}\\sfx\\master_bank.bank")
}

pub fn apply(files: &mut Files, id: &str, events: &[(Role, String)]) -> Result<Vec<String>> {
    let ap = format!("content\\cars\\{id}\\{id}.actor");
    let b = files.get(&ap).cloned().ok_or_else(|| anyhow!("no {ap}"))?;
    let mut a = Msg::parse(&b)?;
    let car = a.msg_mut(103)?;
    let bank = bank_path(id);
    let mut done = Vec::new();
    for i in car.idx(9) {
        let s = car.open(i)?;
        let ty = s.varint(2).unwrap_or(0);
        if let Some((r, path)) = events.iter().find(|(r, _)| r.slot() == ty) {
            s.set_str(1, path);
            s.set_str(6, &bank);
            done.push(*r);
        }
    }
    let mut notes = Vec::new();
    for (r, path) in events {
        if !done.contains(r) {
            // the donor has no slot of this type: add one at the car's origin
            let mut m = Msg::default();
            m.set_str(1, path);
            if r.slot() != 0 {
                m.set_varint(2, r.slot());
            }
            let mut tr = Msg::default();
            let mut sc = Msg::default();
            for f in 1..=3 {
                sc.set_f32(f, 1.0);
            }
            tr.push(1, Val::Msg(Msg::default()));
            tr.push(2, Val::Msg(Msg::default()));
            tr.push(3, Val::Msg(sc));
            m.push(5, Val::Msg(tr));
            m.set_str(6, &bank);
            car.push(9, Val::Msg(m));
            notes.push(format!("sound: actor had no {} slot - added", r.name()));
        }
    }
    files.put(&ap, a.encode());
    notes.push(format!("sound: actor slots -> {} ({})", bank, events.iter().map(|e| e.0.name()).collect::<Vec<_>>().join(", ")));
    Ok(notes)
}
