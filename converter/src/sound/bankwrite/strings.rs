//! Strings bank STDT radix tree (docs/FORMATS.md, "FMOD Studio banks").
//!
//! * radix tree of the case-sensitive paths; a node's key char is the lower-cased first
//!   char of its edge label; children ordered by label bytes;
//! * DFS pre-order: visiting a node allocates all its children contiguously (node array),
//!   each child's label goes to the blob (label + NUL) when the child is reached;
//! * a node without children carries the GUID index; a path ending at an inner node gets
//!   a terminal child (word0 0xffffff, word1 = GUID index) placed first;
//! * GUIDs sorted by FMOD's comparator (Data1, Data2, Data3 numeric, then Data4 bytes);
//!   tri a = GUID index -> node index, tri b = node index -> parent index.

use super::prj::Guid;
use super::w::W;

struct Node {
    label: Vec<u8>,
    kids: Vec<usize>,
    term: Option<Guid>,
}

pub fn fmod_guid_key(g: &Guid) -> (u32, u16, u16, [u8; 8]) {
    let b = &g.0;
    (
        u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        u16::from_le_bytes([b[4], b[5]]),
        u16::from_le_bytes([b[6], b[7]]),
        b[8..16].try_into().unwrap(),
    )
}

/// STDT body for (path, guid) entries.
pub fn stdt(entries: &[(String, Guid)]) -> Vec<u8> {
    let mut nodes: Vec<Node> = vec![Node { label: Vec::new(), kids: Vec::new(), term: None }];
    // insertion in byte order; the tree merges case-insensitively and keeps the first label
    let mut sorted: Vec<&(String, Guid)> = entries.iter().collect();
    sorted.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    for (p, g) in sorted {
        insert(&mut nodes, 0, p.as_bytes(), *g);
    }
    let mut gs: Vec<Guid> = entries.iter().map(|e| e.1).collect();
    gs.sort_by_key(fmod_guid_key);
    gs.dedup();
    let gidx = |g: &Guid| gs.iter().position(|x| x == g).unwrap() as u32;
    let mut words: Vec<(u32, u32)> = vec![(0, 0)];
    let mut pars: Vec<u32> = vec![0xffffff];
    let mut tria = vec![0u32; gs.len()];
    let mut blob: Vec<u8> = Vec::new();
    // iterative DFS: (node, index, blob offset)
    fn visit(n: usize, idx: usize, off: u32, nodes: &Vec<Node>, words: &mut Vec<(u32, u32)>, pars: &mut Vec<u32>, tria: &mut Vec<u32>,
        blob: &mut Vec<u8>, gidx: &dyn Fn(&Guid) -> u32) {
        let mut kids = nodes[n].kids.clone();
        kids.sort_by(|a, b| nodes[*a].label.cmp(&nodes[*b].label));
        let term = nodes[n].term;
        let nslots = kids.len() + usize::from(!kids.is_empty() && term.is_some());
        let first = words.len();
        for _ in 0..nslots {
            words.push((0, 0));
            pars.push(idx as u32);
        }
        let ch = if n == 0 { 0 } else { nodes[n].label[0].to_ascii_lowercase() as u32 };
        let w0 = if n == 0 { 0xffffff } else { (ch << 24) | off };
        if nslots == 0 {
            let gi = gidx(&term.expect("leaf without guid"));
            words[idx] = (w0, gi);
            tria[gi as usize] = idx as u32;
            return;
        }
        words[idx] = (w0, ((nslots as u32) << 24) | first as u32);
        let mut j = first;
        if term.is_some() && !kids.is_empty() {
            let gi = gidx(&term.unwrap());
            words[j] = (0xffffff, gi);
            tria[gi as usize] = j as u32;
            j += 1;
        }
        for k in kids {
            let o = blob.len() as u32;
            blob.extend_from_slice(&nodes[k].label);
            blob.push(0);
            visit(k, j, o, nodes, words, pars, tria, blob, gidx);
            j += 1;
        }
    }
    visit(0, 0, 0xffffff, &nodes, &mut words, &mut pars, &mut tria, &mut blob, &gidx);
    let mut w = W::new();
    w.u32(1);
    w.arr(&words, |w, x| {
        w.u32(x.0).u32(x.1);
    });
    w.guids(&gs);
    w.vcount(blob.len() as u32).bytes(&blob);
    w.vcount(tria.len() as u32);
    for t in &tria {
        w.bytes(&t.to_le_bytes()[..3]);
    }
    w.vcount(pars.len() as u32);
    for t in &pars {
        w.bytes(&t.to_le_bytes()[..3]);
    }
    w.b
}

fn insert(nodes: &mut Vec<Node>, n: usize, s: &[u8], g: Guid) {
    if s.is_empty() {
        nodes[n].term = Some(g);
        return;
    }
    let found = nodes[n].kids.iter().copied().find(|k| nodes[*k].label[0].eq_ignore_ascii_case(&s[0]));
    match found {
        None => {
            nodes.push(Node { label: s.to_vec(), kids: Vec::new(), term: Some(g) });
            let id = nodes.len() - 1;
            nodes[n].kids.push(id);
        }
        Some(k) => {
            let lab = nodes[k].label.clone();
            let j = lab.iter().zip(s.iter()).take_while(|(a, b)| a.eq_ignore_ascii_case(b)).count();
            if j < lab.len() {
                // split k into mid(lab[..j]) -> k(lab[j..])
                nodes.push(Node { label: lab[..j].to_vec(), kids: vec![k], term: None });
                let mid = nodes.len() - 1;
                nodes[k].label = lab[j..].to_vec();
                let pos = nodes[n].kids.iter().position(|x| *x == k).unwrap();
                nodes[n].kids[pos] = mid;
                insert(nodes, mid, &s[j..], g);
            } else {
                insert(nodes, k, &s[j..], g);
            }
        }
    }
}
