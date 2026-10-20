//! The lamp-name corpus behind the lamp keyword lists (src/car/lights.rs):
//! every lamp mesh name the AC1 cars' lights.ini and Custom Shaders Patch
//! configs reference, labelled with its function, tokenised; per token the
//! number of lamp meshes of each function. Writes data/lamp_tokens.tsv.
//!
//! Labels (one per (car, mesh); the most specific wins):
//! - CSP-declared (`lights::csp_lamps`): indicator, high, reverse, brake,
//!   fog; a CSP headlight / rear light is head or tail by position;
//! - lights.ini `[BRAKE_n]`: brake;
//! - lights.ini `[LIGHT_n]`, where nothing declares the function: by colour
//!   and position: amber COLOR indicator, else head (front) or tail (rear),
//!   front / rear from the mesh's kn5 position (AC1 cars face +z: checked on
//!   the corpus, the brake meshes at z < 0 are counted), else from the
//!   colour (red-dominant: rear);
//! - head = a headlight lit with the lights whose beam nobody declares; on
//!   a car whose CSP config declares its high beams, its other headlights
//!   are declared low beams (low).
//! Dashboard names (`lights::dashy`) are left out.
//!
//! cargo run --release --example lampcorpus -- <AC1 content\cars dir> > data/lamp_tokens.tsv

use aceconv::ac1::kn5::{self, Node, NodeKind};
use aceconv::car::lights::{csp_lamps, dashy, tokens, Func};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

const LABELS: [&str; 8] = ["indicator", "reverse", "fog", "high", "low", "head", "brake", "tail"];
/// The functions a name can claim (brake / tail / head come from the
/// lights.ini section and the position).
const KEYWORD_LABELS: [&str; 5] = ["indicator", "reverse", "fog", "high", "low"];
/// OURS: a keyword predicts its function on at least 90 % of its labelled
/// lamps, and at least 5 cars have it with that function (fewer is one
/// author's naming).
const MIN_PRECISION: f64 = 0.9;
const MIN_CARS: usize = 5;
/// OURS: a keyword must mostly name lamps on the cars that use it: of the
/// mesh nodes carrying the token (lamp or not, across the corpus' kn5
/// models), at least this share are lamps. Words that name seats, speakers,
/// steering wheels, decals and other parts more often than lamps are not
/// lamp words, whatever a few lamp meshes are called.
const MIN_LAMP_SHARE: f64 = 0.5;

/// A two-letter position code (left / right / front / middle / centre
/// combinations such as `rr`, `lf`, `lm`, `mr`): it says where a part is,
/// never what a lamp does.
fn position_code(t: &str) -> bool {
    t.len() == 2 && t.chars().all(|c| "lrfmcb".contains(c))
}

fn red(c: [f32; 3]) -> bool {
    c[0] > 1.5 * c[1] && c[0] > 1.5 * c[2]
}

/// Amber / orange: red-dominant with a green share, little blue.
fn amber(c: [f32; 3]) -> bool {
    c[0] > 0.0 && c[1] >= 0.25 * c[0] && c[1] <= 0.8 * c[0] && c[2] <= 0.25 * c[0]
}

fn label_of(f: Func) -> &'static str {
    match f {
        Func::Indicator => "indicator",
        Func::Reverse => "reverse",
        Func::Fog => "fog",
        Func::High => "high",
        Func::Low => "low",
        Func::Drl => "head",
        Func::Brake => "brake",
        Func::Tail => "tail",
    }
}

/// Specificity: declared functions beat the generic headlight / rear light.
fn rank(l: &str) -> u8 {
    match l {
        "indicator" | "reverse" | "fog" | "high" => 3,
        "brake" | "low" => 2,
        _ => 1,
    }
}

/// Lower-case node name -> mean z of its meshes' vertices (world, sampled);
/// `cockpit` collects the nodes under COCKPIT_HR / STEER_HR (the converter's
/// cockpit, where lamps are dashboard backlights).
fn positions(n: &Node, parent: [f32; 16], in_cockpit: bool, out: &mut HashMap<String, (f64, f64)>, cockpit: &mut HashSet<String>) -> (f64, f64) {
    let in_cockpit = in_cockpit || n.name.eq_ignore_ascii_case("COCKPIT_HR") || n.name.eq_ignore_ascii_case("STEER_HR");
    if in_cockpit {
        cockpit.insert(n.name.to_lowercase());
    }
    let world = match &n.kind {
        NodeKind::Transform(m) => kn5::mul(m, &parent),
        _ => parent,
    };
    let mut acc = (0.0, 0.0);
    if let NodeKind::Mesh(m) = &n.kind {
        let step = (m.vertex_count / 200).max(1);
        let mut v = 0;
        while v < m.vertex_count {
            acc.0 += kn5::xform(&world, m.position(v))[2] as f64;
            acc.1 += 1.0;
            v += step;
        }
    }
    for c in &n.children {
        let a = positions(c, world, in_cockpit, out, cockpit);
        acc.0 += a.0;
        acc.1 += a.1;
    }
    if acc.1 > 0.0 {
        let e = out.entry(n.name.to_lowercase()).or_insert((0.0, 0.0));
        e.0 += acc.0;
        e.1 += acc.1;
    }
    acc
}

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let mut dirs: Vec<_> = std::fs::read_dir(&a[1])?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    // (car, mesh) -> label
    let mut lamps: BTreeMap<(String, String), &str> = BTreeMap::new();
    // (car, mesh node) for every node with geometry in the corpus' kn5 models
    let mut all_nodes: BTreeSet<(String, String)> = BTreeSet::new();
    let (mut brake_rear, mut brake_pos, mut by_colour, mut cars_used, mut with_model) = (0usize, 0usize, 0usize, 0usize, 0usize);
    for dir in &dirs {
        let folder = dir.file_name().unwrap().to_string_lossy().into_owned();
        let Ok(data) = aceconv::ac1::acd::read_car_data(dir) else { continue };
        let li = aceconv::ac1::ini::parse(data.get("lights.ini").map(|s| s.as_str()).unwrap_or(""));
        let csp = aceconv::car::fx::csp_config_at(dir, &folder);
        if li.sections.is_empty() && csp.is_none() {
            continue;
        }
        cars_used += 1;
        // kn5 positions (unless the model is protected or missing)
        let mut pos: HashMap<String, (f64, f64)> = HashMap::new();
        let mut cockpit: HashSet<String> = HashSet::new();
        match aceconv::ac1::car::Car::open(dir) {
            Ok(car) => match car.kn5() {
                Ok(k) => {
                    positions(&k.root, kn5::IDENTITY, false, &mut pos, &mut cockpit);
                    with_model += 1;
                }
                Err(_) => {}
            },
            Err(_) => {}
        }
        for n in pos.keys() {
            all_nodes.insert((folder.clone(), n.clone()));
        }
        let z = |n: &str| pos.get(n).filter(|p| p.1 > 0.0).map(|p| p.0 / p.1);
        // labels before the beam rule
        let mut found: Vec<(String, &'static str)> = Vec::new();
        for (sec, kv) in li.sections.iter() {
            let brake = sec.starts_with("BRAKE_");
            if !(brake || sec.starts_with("LIGHT_")) {
                continue;
            }
            let Some(names) = kv.get("NAME") else { continue };
            let col = aceconv::car::fx::rgb(kv.get("COLOR"));
            for n in aceconv::car::fx::name_list(names) {
                if brake {
                    if let Some(zz) = z(&n) {
                        brake_pos += 1;
                        brake_rear += (zz < 0.0) as usize;
                    }
                    found.push((n, "brake"));
                    continue;
                }
                // front / rear: the kn5 position, else the colour (red-dominant: rear)
                let front = match z(&n) {
                    Some(zz) => zz > 0.0,
                    None => {
                        by_colour += 1;
                        !red(col)
                    }
                };
                let l = if amber(col) { "indicator" } else if front { "head" } else { "tail" };
                found.push((n, l));
            }
        }
        let mut csp_high = false;
        if let Some((_, secs)) = &csp {
            for (names, k) in csp_lamps(secs) {
                csp_high |= k == Some(Func::High);
                for n in names {
                    let l = match k {
                        Some(f) => label_of(f),
                        None => match z(&n) {
                            Some(zz) if zz < 0.0 => "tail",
                            Some(_) => "head",
                            None => continue,
                        },
                    };
                    found.push((n, l));
                }
            }
        }
        for (n, l) in found {
            if dashy(&n) || cockpit.contains(&n) {
                continue;
            }
            // a car whose CSP config declares its high beams: its other
            // headlights are declared low beams
            let l = if l == "head" && csp_high { "low" } else { l };
            let e = lamps.entry((folder.clone(), n)).or_insert(l);
            if rank(l) > rank(e) {
                *e = l;
            }
        }
    }
    // token -> label -> lamp count, and the cars per token
    let mut counts: BTreeMap<String, BTreeMap<&str, usize>> = BTreeMap::new();
    let mut cars: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut label_cars: BTreeMap<(String, &str), BTreeSet<String>> = BTreeMap::new();
    for ((car, mesh), l) in &lamps {
        let toks: BTreeSet<String> = tokens(mesh).into_iter().collect();
        for t in toks {
            *counts.entry(t.clone()).or_default().entry(l).or_default() += 1;
            cars.entry(t.clone()).or_default().insert(car.clone());
            label_cars.entry((t, *l)).or_default().insert(car.clone());
        }
    }
    // token -> non-lamp mesh nodes carrying it
    let mut other: BTreeMap<String, usize> = BTreeMap::new();
    for (car, node) in &all_nodes {
        if lamps.contains_key(&(car.clone(), node.clone())) {
            continue;
        }
        let toks: BTreeSet<String> = tokens(node).into_iter().collect();
        for t in toks {
            if counts.contains_key(&t) {
                *other.entry(t).or_default() += 1;
            }
        }
    }
    let mut per_label: BTreeMap<&str, usize> = BTreeMap::new();
    for l in lamps.values() {
        *per_label.entry(l).or_default() += 1;
    }
    println!("# lamp-name corpus: {} lamp meshes of {cars_used} AC1 cars; labels {per_label:?}", lamps.len());
    println!("# kn5 models read: {with_model} cars (the rest protected or missing); LIGHT meshes placed by colour: {by_colour}");
    println!("# AC1 cars face +z: brake meshes with a kn5 position {brake_pos}, at z < 0 {brake_rear}");
    println!("# keyword = the best function when its precision >= {MIN_PRECISION} and >= {MIN_CARS} cars have the token with that function, for {KEYWORD_LABELS:?};");
    println!("# and the token mostly names lamps (lamp meshes / all mesh nodes carrying it >= {MIN_LAMP_SHARE}) and is not a two-letter position code (rr, lf, lm, ...);");
    println!("# 'head' (a headlight whose beam nobody declares) counts against every function but high / low");
    println!("token\tlamps\tcars\t{}\tbest\tprecision\tkeyword", LABELS.join("\t"));
    for (t, c) in &counts {
        let get = |l: &str| c.get(l).copied().unwrap_or(0);
        let total: usize = c.values().sum();
        let (best, n) = LABELS.iter().filter(|l| **l != "head").map(|l| (*l, get(l))).max_by_key(|x| x.1).unwrap();
        // only undeclared headlights: no function to name
        let best = if n == 0 { "-" } else { best };
        let denom = if best == "high" || best == "low" { total - get("head") } else { total };
        let prec = if denom > 0 { n as f64 / denom as f64 } else { 0.0 };
        let best_cars = label_cars.get(&(t.clone(), best)).map(|c| c.len()).unwrap_or(0);
        let others = other.get(t).copied().unwrap_or(0);
        let lamp_share = total as f64 / (total + others) as f64;
        let kw = KEYWORD_LABELS.contains(&best) && prec >= MIN_PRECISION && best_cars >= MIN_CARS
            && lamp_share >= MIN_LAMP_SHARE && !position_code(t);
        let cols: Vec<String> = LABELS.iter().map(|l| get(l).to_string()).collect();
        println!("{t}\t{total}\t{}\t{}\t{best}\t{prec:.4}\t{}", cars[t].len(), cols.join("\t"), if kw { best } else { "-" });
    }
    Ok(())
}
