//! The FMOD Studio project object graph (metadata\**\*.xml), the bank writer's input.

use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::studioxml::parse as vp;

/// A GUID in its 16-byte Windows (bytes_le) form; Ord = memcmp of those bytes,
/// which is the order every bank list is sorted in.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Guid(pub [u8; 16]);

impl Guid {
    pub const ZERO: Guid = Guid([0; 16]);
    /// `{xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx}` (braces optional).
    pub fn parse(s: &str) -> Option<Guid> {
        let h: String = s.trim().trim_matches(|c| c == '{' || c == '}').chars().filter(|c| *c != '-').collect();
        if h.len() != 32 {
            return None;
        }
        let mut be = [0u8; 16];
        for i in 0..16 {
            be[i] = u8::from_str_radix(&h[2 * i..2 * i + 2], 16).ok()?;
        }
        let mut b = be;
        b[0..4].reverse();
        b[4..6].reverse();
        b[6..8].reverse();
        Some(Guid(b))
    }
    pub fn xor(&self, o: &Guid) -> Guid {
        let mut b = [0u8; 16];
        for i in 0..16 {
            b[i] = self.0[i] ^ o.0[i];
        }
        Guid(b)
    }
    /// FMOD's GUID hash: xor of the four little-endian u32 words.
    pub fn fold(&self) -> u32 {
        (0..4).fold(0, |a, i| a ^ u32::from_le_bytes(self.0[4 * i..4 * i + 4].try_into().unwrap()))
    }
    pub fn is_zero(&self) -> bool {
        self.0 == [0; 16]
    }
}

impl std::fmt::Display for Guid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let b = &self.0;
        write!(f, "{{{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-", b[3], b[2], b[1], b[0], b[5], b[4], b[7], b[6], b[8], b[9])?;
        for x in &b[10..] {
            write!(f, "{x:02x}")?;
        }
        write!(f, "}}")
    }
}
impl std::fmt::Debug for Guid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self}")
    }
}

pub struct Obj {
    pub id: Guid,
    pub class: String,
    pub props: HashMap<String, Vec<String>>,
    pub rels: HashMap<String, Vec<Guid>>,
    /// metadata file (for diagnostics)
    pub file: PathBuf,
}

impl Obj {
    pub fn p(&self, k: &str) -> Option<&str> {
        self.props.get(k).and_then(|v| v.first()).map(|s| s.as_str())
    }
    pub fn pv(&self, k: &str) -> &[String] {
        self.props.get(k).map(|v| v.as_slice()).unwrap_or(&[])
    }
    pub fn f(&self, k: &str, d: f64) -> f64 {
        self.p(k).and_then(|s| s.trim().parse::<f64>().ok()).unwrap_or(d)
    }
    pub fn b(&self, k: &str, d: bool) -> bool {
        match self.p(k) {
            Some("true") => true,
            Some("false") => false,
            _ => d,
        }
    }
    pub fn i(&self, k: &str, d: i64) -> i64 {
        self.p(k).and_then(|s| s.trim().parse::<i64>().ok()).unwrap_or(d)
    }
    pub fn r(&self, k: &str) -> &[Guid] {
        self.rels.get(k).map(|v| v.as_slice()).unwrap_or(&[])
    }
    pub fn r1(&self, k: &str) -> Option<Guid> {
        self.r(k).first().copied()
    }
    pub fn name(&self) -> &str {
        self.p("name").unwrap_or("")
    }
}

pub struct Project {
    pub dir: PathBuf,
    pub objs: HashMap<Guid, Obj>,
}

impl Project {
    pub fn load(dir: &Path) -> Result<Project> {
        let mut objs = HashMap::new();
        let meta = dir.join("metadata");
        let mut files = Vec::new();
        walk(&meta, &mut files)?;
        files.sort();
        for f in files {
            let text = std::fs::read_to_string(&f).with_context(|| format!("read {}", f.display()))?;
            for o in vp(&text) {
                let id = Guid::parse(&o.id).ok_or_else(|| anyhow!("bad id {} in {}", o.id, f.display()))?;
                let rels = o.rels.iter().map(|(k, v)| (k.clone(), v.iter().filter_map(|s| Guid::parse(s)).collect())).collect();
                let props = o.props.into_iter().map(|(k, v)| (k, v.into_iter().map(|s| unescape(&s)).collect())).collect();
                objs.insert(id, Obj { id, class: o.class, props, rels, file: f.clone() });
            }
        }
        Ok(Project { dir: dir.to_path_buf(), objs })
    }
    pub fn get(&self, g: &Guid) -> Option<&Obj> {
        self.objs.get(g)
    }
    pub fn of(&self, g: Option<Guid>) -> Option<&Obj> {
        g.and_then(|g| self.objs.get(&g))
    }
    pub fn class(&self, cls: &str) -> Vec<&Obj> {
        let mut v: Vec<&Obj> = self.objs.values().filter(|o| o.class == cls).collect();
        v.sort_by_key(|o| o.id);
        v
    }
}

fn walk(d: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for e in std::fs::read_dir(d).with_context(|| format!("read_dir {}", d.display()))? {
        let p = e?.path();
        if p.is_dir() {
            walk(&p, out)?;
        } else if p.extension().map(|x| x == "xml").unwrap_or(false) {
            out.push(p);
        }
    }
    Ok(())
}

fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}
