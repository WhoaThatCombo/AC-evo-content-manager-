//! AC1 `.knh` node hierarchy (`driver_base_pos.knh`: the seated driver
//! pose), read off the AC1 install's own files: per node `i32 name length,
//! name, 16 f32 local matrix (row vectors, translation in 12..14, like the
//! kn5), i32 child count`, then the children, depth first. The root is
//! `SCENE_ROOT`; the driver's joints are `DRIVER:RIG_*` / `DRIVER:HAND_*`.

use anyhow::{bail, Result};

use crate::car::frame::M4;

#[derive(Clone, Debug)]
pub struct KNode {
    pub name: String,
    pub local: M4,
    pub children: Vec<KNode>,
}

pub fn parse(b: &[u8]) -> Result<KNode> {
    fn rd(b: &[u8], o: &mut usize, depth: usize) -> Result<KNode> {
        if depth > 256 {
            bail!("knh nesting too deep");
        }
        let i32_ = |o: &mut usize| -> Result<i32> {
            let Some(s) = b.get(*o..*o + 4) else { bail!("knh truncated at {o}") };
            *o += 4;
            Ok(i32::from_le_bytes(s.try_into().unwrap()))
        };
        let n = i32_(o)?;
        if !(0..=4096).contains(&n) {
            bail!("knh name length {n}");
        }
        let Some(s) = b.get(*o..*o + n as usize) else { bail!("knh truncated") };
        let name = String::from_utf8_lossy(s).into_owned();
        *o += n as usize;
        let mut local = [0f64; 16];
        for v in local.iter_mut() {
            let Some(s) = b.get(*o..*o + 4) else { bail!("knh truncated") };
            *v = f32::from_le_bytes(s.try_into().unwrap()) as f64;
            *o += 4;
        }
        let c = i32_(o)?;
        if !(0..=10_000).contains(&c) {
            bail!("knh child count {c}");
        }
        let mut children = Vec::with_capacity(c as usize);
        for _ in 0..c {
            children.push(rd(b, o, depth + 1)?);
        }
        Ok(KNode { name, local, children })
    }
    let mut o = 0;
    rd(b, &mut o, 0)
}

impl KNode {
    /// (name, local, parent name) of every node, parents first.
    pub fn flat(&self) -> Vec<(String, M4, Option<String>)> {
        let mut out = Vec::new();
        fn walk(n: &KNode, parent: Option<&str>, out: &mut Vec<(String, M4, Option<String>)>) {
            out.push((n.name.clone(), n.local, parent.map(|s| s.to_string())));
            for c in &n.children {
                walk(c, Some(&n.name), out);
            }
        }
        walk(self, None, &mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn two_nodes() {
        let mut b = Vec::new();
        let node = |b: &mut Vec<u8>, name: &str, t: f32, kids: i32| {
            b.extend((name.len() as i32).to_le_bytes());
            b.extend(name.as_bytes());
            for i in 0..16 {
                let v = if i % 5 == 0 { 1.0f32 } else if i == 13 { t } else { 0.0 };
                b.extend(v.to_le_bytes());
            }
            b.extend(kids.to_le_bytes());
        };
        node(&mut b, "SCENE_ROOT", 0.0, 1);
        node(&mut b, "DRIVER:RIG_Hips", 0.5, 0);
        let k = super::parse(&b).unwrap();
        let f = k.flat();
        assert_eq!(f.len(), 2);
        assert_eq!(f[1].0, "DRIVER:RIG_Hips");
        assert_eq!(f[1].1[13], 0.5);
        assert_eq!(f[1].2.as_deref(), Some("SCENE_ROOT"));
    }
}
