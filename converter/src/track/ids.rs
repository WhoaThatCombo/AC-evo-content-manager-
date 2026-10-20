//! Entity ids (the scene's fixed64 `guid`).
//!
//! Stock ids look like arbitrary 64-bit values and must be unique within a
//! track. Ours: FNV-1a-64 of `"<track id>/<entity kind>/<stable name>"`, so
//! converting the same input again gives the same ids. A clash (two keys,
//! one hash) is resolved by hashing `"<key>#1"`, `"<key>#2"`, ... in the
//! order the keys are asked for, which is deterministic for one input. 0 is
//! never handed out (proto3 would drop it).

use std::collections::HashMap;

pub struct Ids {
    track: String,
    /// hash -> the key that owns it
    used: HashMap<u64, String>,
    /// key -> hash already handed out
    given: HashMap<String, u64>,
}

impl Ids {
    pub fn new(track: &str) -> Ids {
        Ids { track: track.to_string(), used: HashMap::new(), given: HashMap::new() }
    }

    /// The id of entity `name` of kind `kind` (the same key always gets
    /// the same id).
    pub fn of(&mut self, kind: &str, name: &str) -> u64 {
        let key = format!("{}/{}/{}", self.track, kind, name);
        if let Some(&h) = self.given.get(&key) {
            return h;
        }
        let mut n = 0u32;
        loop {
            let k = if n == 0 { key.clone() } else { format!("{key}#{n}") };
            let h = crate::ids::fnv1a64(&k);
            if h != 0 && !self.used.contains_key(&h) {
                self.used.insert(h, key.clone());
                self.given.insert(key, h);
                return h;
            }
            n += 1;
        }
    }

    /// A fresh id for a name that may repeat (the n-th call for the same
    /// kind / name gets `name#n`).
    pub fn unique(&mut self, kind: &str, name: &str) -> u64 {
        let mut n = 0u32;
        loop {
            let nm = if n == 0 { name.to_string() } else { format!("{name}#{n}") };
            let key = format!("{}/{}/{}", self.track, kind, nm);
            if !self.given.contains_key(&key) {
                return self.of(kind, &nm);
            }
            n += 1;
        }
    }

    pub fn count(&self) -> usize {
        self.used.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_and_unique() {
        let mut a = Ids::new("t");
        let mut b = Ids::new("t");
        let x = a.of("SMesh", "road_1");
        assert_eq!(x, b.of("SMesh", "road_1"));
        assert_eq!(x, a.of("SMesh", "road_1"));
        assert_ne!(x, a.of("Zone", "road_1"));
        let u1 = a.unique("Start Pos", "grid");
        let u2 = a.unique("Start Pos", "grid");
        assert_ne!(u1, u2);
        assert_eq!(a.count(), 4);
    }
}
