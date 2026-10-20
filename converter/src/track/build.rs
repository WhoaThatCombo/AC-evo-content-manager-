//! `aceconv track`: an AC1 track folder -> a loadable EVO track folder
//! (`<out>\<id>\`, as it is installed under `content\tracks\<id>\`) and its
//! declaration rows (`ui\track_decl.json`, applied by ACECM).
//!
//! The track-base stage: scan, ground, surfaces, meshes with minimal
//! materials, the scene and its containers, spawns, timing gates, the
//! .track file and the rows. Later stages add the full look, AI lines and
//! pits, TV cameras, lighting bakes and vegetation.

use anyhow::{bail, Context, Result};
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use super::classes::{surface_kind, Kind};
use super::declare::{self, LayoutDecl};
use super::geo;
use super::ground::Ground;
use super::meshes::{self, GroupRole};
use super::scan::{Role, Scan, Source};
use super::scene::{transform, Entity, SceneBuilder};
use super::spawns::{self, Spawns};
use super::surfaces;
use crate::io::kspkg::Pkg;
use crate::io::pb::{Msg, Val};

#[derive(Clone, Debug, Default)]
pub struct Options {
    /// track id (default: `ids::track_id` of the AC1 folder name)
    pub id: Option<String>,
    /// the game's content.kspkg (stock templates and ids); None = default
    /// location, ignored when missing
    pub content: Option<PathBuf>,
}

pub struct Converted {
    pub id: String,
    pub dir: PathBuf,
    pub report: Vec<String>,
}

/// The container suffix of a layout: its AC1 folder name, slugged.
pub fn layout_stem(ac1: &str) -> String {
    let s = crate::ids::slug(ac1);
    if s.is_empty() { "main".into() } else { s }
}

/// Display names: the track (the common start of the layouts' ui names)
/// and each layout ("GP" from "Brands Hatch - GP"; "Full" for a lone layout).
fn names(scan: &Scan) -> (String, Vec<String>) {
    let ui: Vec<String> = scan.layouts.iter().map(|l| l.ui.name.trim().to_string()).collect();
    let title = |s: &str| s.split(|c: char| c == '_' || c == '-' || c == ' ').filter(|w| !w.is_empty()).map(|w| {
        let mut c = w.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
    }).collect::<Vec<_>>().join(" ");
    let first = ui.iter().find(|s| !s.is_empty()).cloned().unwrap_or_else(|| title(&scan.folder));
    if scan.layouts.len() == 1 {
        return (first, vec!["Full".into()]);
    }
    // common prefix of the ui names, cut at a separator
    let mut common = first.clone();
    for s in ui.iter().filter(|s| !s.is_empty()) {
        while !s.starts_with(&common) {
            common.pop();
        }
    }
    let track = common.trim_end_matches(|c: char| c == '-' || c == ' ' || c == ':' || c == '(').trim().to_string();
    let track = if track.len() >= 3 { track } else { first.clone() };
    let mut seen = HashSet::new();
    let lays = scan.layouts.iter().zip(&ui).map(|(l, u)| {
        let rest = u.strip_prefix(&track).unwrap_or("").trim_start_matches(|c: char| c == '-' || c == ' ' || c == ':' || c == '(').trim_end_matches(')').trim().to_string();
        // AC1 often names the main layout exactly like the track (Black Cat
        // County: "Black Cat County", "... - Long", "... - Short"): then the
        // layout folder names it
        let base = layout_word(&if rest.is_empty() { folder_layout_name(&l.name) } else { rest });
        let base = if base.is_empty() { "Full".to_string() } else { base };
        unique_name(base, &mut seen)
    }).collect();
    (track, lays)
}

/// A layout display name the game's UI ids survive unchanged: letters,
/// digits and single spaces (stock: "GP", "GP Short", "Layout 3A"; the UI's
/// two id formulas - `getTrackId` keeps hyphens, the map's `slug` does not -
/// agree only on such names). Known AC1 abbreviations are spelled out.
pub fn layout_word(s: &str) -> String {
    let words: Vec<String> = s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(|w| {
        let lw = w.to_lowercase();
        let full = match lw.as_str() {
            "int" | "intl" => "International",
            "nat" | "natl" => "National",
            "gp" => "GP",
            "rev" => "Reverse",
            _ => "",
        };
        if !full.is_empty() {
            return full.to_string();
        }
        let mut c = w.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
    }).collect();
    words.join(" ")
}

/// The name an AC1 layout folder gives ("layout_short" -> "short",
/// "layout_int" -> "int", "gp" -> "gp").
fn folder_layout_name(folder: &str) -> String {
    let f = folder.trim();
    let low = f.to_lowercase();
    for p in ["layout_", "layout-", "layout"] {
        if low.starts_with(p) && low.len() > p.len() {
            return f[p.len()..].to_string();
        }
    }
    f.to_string()
}

/// `base`, or `base 2`, `base 3`...: unique by the UI id (case and
/// punctuation ignored), so two layouts never share rows or images.
fn unique_name(base: String, seen: &mut HashSet<String>) -> String {
    let key = |s: &str| s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect::<String>();
    let mut n = base.clone();
    let mut k = 2;
    while !seen.insert(key(&n)) {
        n = format!("{base} {k}");
        k += 1;
    }
    n
}

/// A Spline entity (linear nodes, empty anchors, as stock centre / limit
/// lines). `half_width` = SplineData f11 recorded_half_car_width (stock
/// sets it, 1.055, on the ideal-line splines only).
fn spline_entity(name: &str, kinds: &[&str], points: &[[f32; 3]], closed: bool, half_width: Option<f32>) -> Entity {
    let mut sd = Msg::default();
    let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
    for p in points {
        let mut n = Msg::default();
        n.push(1, Val::Msg(transform(*p, [0.0; 3], [1.0; 3])));
        n.push(10, Val::Msg(Msg::default()));
        n.push(11, Val::Msg(Msg::default()));
        sd.push(3, Val::Msg(n));
        lo = [lo[0].min(p[0]), lo[1].min(p[2])];
        hi = [hi[0].max(p[0]), hi[1].max(p[2])];
    }
    if closed {
        sd.set_varint(10, 1);
    }
    if let Some(w) = half_width {
        sd.set_f32(11, w);
    }
    let mut a = Msg::default();
    a.push(1, Val::Msg(sd));
    let mut col = Msg::default();
    col.set_f32(2, 1.0);
    col.set_f32(4, 1.0);
    a.push(21, Val::Msg(col));
    let mut dbg = Msg::default();
    dbg.set_f32(1, 0.2);
    dbg.set_varint(2, 10);
    dbg.set_varint(3, 3);
    a.push(30, Val::Msg(dbg));
    let mut e = Entity::new(name, "Spline", 108, a);
    e.kinds.extend(kinds.iter().map(|k| k.to_string()));
    e.bounds = Some((lo, hi));
    e
}

/// The layout container's splines (stock: every race layout of the 17
/// stock tracks has them):
/// - `<stem>_track_center_spline` (tag `center_spline`), ~5 m nodes;
/// - `<stem>_track_limits_left` / `_right` (tags `track_limits_left` /
///   `_right`): the white lines, `sides` off the ideal line, ~4 m nodes
///   (stock 986 nodes on 3.9 km). The game finds them by NAME (stock cota /
///   brands carry no tag) and takes the layout path from the container
///   holding `track_limits_left` (its scene path minus ".scene": the
///   AISplineData f5 layout_path of `layouts\<stem>.ideal_line.aisplinedata`,
///   the file it then loads); without it: "scene_actors collection
///   doesn't contain track_limits_left splines", no AI spline is loaded and
///   the session has "no ideal line" (no simulated reference, and the pit
///   menu's car-relative camera sequence sits at the world origin);
/// - `<stem>_ideal_line` (tag `ideal_line`; stock monza / oulton tag theirs
///   `center_spline`, so the name is what counts): the ideal line's own
///   points, f11 1.055 as stock;
/// - `<stem>_pitlane_spline` (tag `pitlane_spline`) when there is a pit
///   line (stock has one on 30 of 36 layouts).
fn layout_splines(stem: &str, line: &[[f32; 3]], closed: bool, ideal: Option<&super::ai::Line>, pit: Option<&super::ai::Line>) -> Vec<Entity> {
    let mut out = Vec::new();
    if line.len() > 2 {
        out.push(spline_entity(&format!("{stem}_track_center_spline"), &["center_spline"], &resample(line, 5.0), closed, None));
    }
    if let Some(il) = ideal.filter(|l| l.samples.len() > 2) {
        let edge = |sign: f32, k: usize| -> Vec<[f32; 3]> {
            il.samples.iter().map(|s| {
                let l = super::ai::left_of(s.fwd);
                let d = sign * s.sides[k];
                [s.pos[0] + l[0] * d, s.pos[1], s.pos[2] + l[2] * d]
            }).collect()
        };
        for (side, sign, k) in [("left", 1.0f32, 0usize), ("right", -1.0, 1)] {
            let pts = super::ai::resample(&edge(sign, k), il.closed, 4.0);
            let tag = format!("track_limits_{side}");
            out.push(spline_entity(&format!("{stem}_{tag}"), &[tag.as_str()], &pts, il.closed, None));
        }
        let pts: Vec<[f32; 3]> = il.samples.iter().map(|s| s.pos).collect();
        out.push(spline_entity(&format!("{stem}_ideal_line"), &["ideal_line"], &pts, il.closed, Some(super::ai::HALF_CAR_WIDTH)));
    }
    if let Some(pl) = pit.filter(|l| l.samples.len() > 1) {
        let pts: Vec<[f32; 3]> = pl.samples.iter().map(|s| s.pos).collect();
        out.push(spline_entity(&format!("{stem}_pitlane_spline"), &["pitlane_spline"], &pts, pl.closed, None));
    }
    out
}

/// AI-line points every ~5 m (stock centre splines: 862 nodes over 4.3 km).
fn resample(points: &[[f32; 3]], step: f32) -> Vec<[f32; 3]> {
    let mut out = Vec::new();
    let mut acc = f32::INFINITY;
    for w in points.windows(2) {
        acc += super::spawns::dist3(w[0], w[1]);
        if acc >= step {
            out.push(w[0]);
            acc = 0.0;
        }
    }
    if let Some(l) = points.last() {
        out.push(*l);
    }
    out
}

fn clean_out(dir: &Path) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    // only ever replace a folder this converter wrote
    if !dir.join("ui").join("track_decl.json").is_file() {
        if std::fs::read_dir(dir)?.next().is_some() {
            bail!("{} exists and was not written by aceconv (no ui\\track_decl.json) - not replacing it", dir.display());
        }
        return Ok(());
    }
    std::fs::remove_dir_all(dir).with_context(|| format!("clear {}", dir.display()))
}

/// Run the lighting bakes on the written meshes (`ACECONV_BAKES` picks them).
#[allow(clippy::too_many_arguments)]
fn bake(scan: &Scan, id: &str, out: &Path, mo: &meshes::Output, mats: &super::materials::Materials, looks: &super::look::Looks,
    track_flags: &[bool], bounds: &[([f32; 2], [f32; 2])], canopies: (super::vegetation::Occ, super::vegetation::Occ), b: &mut SceneBuilder) -> Result<Vec<String>> {
    use super::bakes::{self, MatInfo, MeshRef};
    let prefix = format!("content\\tracks\\{id}\\");
    let albedo: std::collections::HashMap<&str, [f32; 3]> = mats.albedo.iter().map(|(n, _, _, a)| (n.as_str(), *a)).collect();
    let mut materials = std::collections::HashMap::new();
    for (&(g, _, v), w) in &mats.by_key {
        let stem = w.path.rsplit('\\').next().unwrap_or("").trim_end_matches(".material");
        let var = looks.variants.get(v as usize);
        let m = scan.material(g);
        materials.insert(w.path.to_lowercase(), MatInfo {
            albedo: albedo.get(stem).copied().unwrap_or([0.2; 3]),
            opaque: !super::materials::transparent(scan, g) && m.depth_mode == 0,
            foliage: m.alpha_tested && m.blend_mode == 0,
            emissive: var.map(|x| x.emissive.is_some() || x.blink.is_some() || x.start_light.is_some()).unwrap_or(false),
            shader: bakes::shader_tag(&w.shader),
        });
    }
    let refs: Vec<MeshRef> = mo.meshes.iter().map(|w| {
        let rel = w.path.strip_prefix(&prefix).unwrap_or(&w.path).replace('\\', "/");
        let (drawn, ground, drivable) = match w.role {
            GroupRole::Visual => (true, false, false),
            GroupRole::Drawn(s) => (true, true, track_flags.get(s).copied().unwrap_or(false)),
            GroupRole::Collider(_) => (false, false, false),
        };
        MeshRef { file: out.join(rel), lo: w.lo, hi: w.hi, drawn, ground, drivable }
    }).collect();
    let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
    for (a, c) in bounds {
        lo = [lo[0].min(a[0]), lo[1].min(a[1])];
        hi = [hi[0].max(c[0]), hi[1].max(c[1])];
    }
    let inp = bakes::Inputs { out, track: id, meshes: refs, materials, drivable: (lo, hi), extra_occluders: canopies.0, extra_foliage: canopies.1 };
    let mut gs = b.main.graphics.take().unwrap_or_default();
    let r = bakes::run(&inp, bakes::Switches::from_env(), b, &mut gs);
    b.main.graphics = Some(gs);
    Ok(r?.lines)
}

pub fn convert(src_dir: &Path, out_root: &Path, opt: &Options) -> Result<Converted> {
    let t0 = std::time::Instant::now();
    let source = Source::open(src_dir)?;
    let scan = Scan::new(&source)?;
    let content = opt.content.clone().unwrap_or_else(crate::io::gamedir::evo_content);
    let pkg = Pkg::open(&content).ok();
    let mut report = scan.report();
    if pkg.is_none() {
        report.push(format!("note: {} not readable: built-in templates used", content.display()));
    }
    let taken: HashSet<String> = pkg.as_ref().map(|p| p.entries.iter().filter_map(|e| {
        let l = e.path.to_lowercase();
        l.strip_prefix("content\\tracks\\").and_then(|r| r.split_once('\\')).map(|(id, _)| id.to_string())
    }).collect()).unwrap_or_default();
    let id = match &opt.id {
        Some(i) => i.clone(),
        None => crate::ids::track_id(&scan.folder, &taken),
    };
    if !crate::ids::valid("tracks", &id) {
        bail!("track id {id:?} is not a valid id (lower-case a-z 0-9 _, 3..16 characters)");
    }
    let out = out_root.join(&id);
    clean_out(&out)?;
    std::fs::create_dir_all(out.join("containers"))?;
    std::fs::create_dir_all(out.join("ui"))?;
    let (display, layout_names) = names(&scan);
    // the name keys the container rows (f10): never one another track has
    // (an AC1 Kunos track is named like its EVO twin, "Brands Hatch")
    let display = {
        let others: HashSet<String> = pkg.as_ref().and_then(|p| p.find("system\\tracks.table").and_then(|e| p.read(e).ok()))
            .and_then(|b| Msg::parse(&b).ok()).map(|t| t.sub(2).map(|x| x.subs(3)).unwrap_or_default().iter().filter_map(|l| l.sub(2))
                .filter(|d| d.str(3).map(|f| !f.eq_ignore_ascii_case(&format!("content\\tracks\\{id}"))).unwrap_or(true))
                .filter_map(|d| d.str(1)).map(|n| n.to_lowercase()).collect()).unwrap_or_default();
        let mut n = display.clone();
        let mut k = 1;
        while others.contains(&n.to_lowercase()) {
            n = if k == 1 { format!("{display} AC") } else { format!("{display} AC {k}") };
            k += 1;
        }
        if n != display {
            report.push(format!("note: track name {display:?} is taken by another installed track: named {n:?}"));
        }
        n
    };
    let kinds: Vec<Kind> = scan.surfaces.iter().map(surface_kind).collect();
    let track_flags: Vec<bool> = kinds.iter().map(|k| k.track() || *k == Kind::Verge).collect();
    let mut notes: Vec<String> = Vec::new();

    // ---- per layout: ground, spawns, gates, extent
    struct LayoutWork {
        stem: String,
        spawns: Spawns,
        bounds: ([f32; 2], [f32; 2]),
        length: f32,
        ground_min: f32,
        line: Vec<[f32; 3]>,
        closed: bool,
        play: super::gameplay::Play,
    }
    let mut work: Vec<LayoutWork> = Vec::new();
    // the dynamic-track frame, shared by every layout (one per .track)
    let frame = super::gameplay::frame(&scan, &kinds);
    let mut presets = 0usize;
    // the first layout's ground, kept for the vegetation stage
    let mut ground0: Option<Ground> = None;
    for (li, l) in scan.layouts.iter().enumerate() {
        let g = Ground::build(&scan, li, track_flags.clone());
        if g.is_empty() {
            bail!("layout {:?} has no physics meshes: nothing to drive on", l.name);
        }
        let sp = spawns::build(l, &g);
        for n in &sp.notes {
            notes.push(format!("layout {:?}: {n}", l.name));
        }
        let line: Vec<[f32; 3]> = l.fast_lane.as_ref().map(|a| a.points.iter().map(|p| p.pos).collect()).unwrap_or_default();
        let bounds = if line.len() > 2 {
            let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
            for p in &line {
                lo = [lo[0].min(p[0]), lo[1].min(p[2])];
                hi = [hi[0].max(p[0]), hi[1].max(p[2])];
            }
            (lo, hi)
        } else {
            g.drivable_bounds()
        };
        let length = l.fast_lane.as_ref().map(|a| a.length()).filter(|v| *v > 100.0).or(l.ui.length_m).unwrap_or(0.0);
        let closed = l.fast_lane.as_ref().map(|a| a.points.len() > 2 && super::spawns::dist3(a.points[0].pos, a.points[a.points.len() - 1].pos) < 50.0).unwrap_or(false);
        let ground_min = g.bounds().0[1];
        // the AI line should lie on the ground index: |dy| < 0.3 m for 99 %,
        // dy measured from the line's own median height above the ground
        // (Kunos lines ride a constant 0.30 m above it: Brands GP p1..p99
        // 0.26..0.31 m; Barber's sits on it)
        if let Some(ai) = &l.fast_lane {
            let mut dys: Vec<f32> = ai.points.iter().filter_map(|p| g.below(p.pos[0], p.pos[1] + 2.0, p.pos[2]).map(|h| p.pos[1] - h.y)).collect();
            dys.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let med = dys.get(dys.len() / 2).copied().unwrap_or(0.0);
            let on = dys.iter().filter(|d| (*d - med).abs() < 0.3).count();
            let share = on as f32 / ai.points.len().max(1) as f32;
            report.push(format!("  layout {:?}: AI line {:.2} m above the ground (median); {:.1} % within 0.3 m of that", l.name, med, share * 100.0));
            if share < 0.99 {
                notes.push(format!("layout {:?}: only {:.1} % of the AI line follows the physics ground", l.name, share * 100.0));
            }
        }
        // ---- gameplay: AI splines, pit lane, dynamic track
        let start = sp.grid.first().map(|s| (s.pos, s.fwd));
        let mut play = super::gameplay::lines(&scan, li, &g, &kinds, start);
        for n in &play.notes {
            notes.push(format!("layout {:?}: {n}", l.name));
        }
        report.extend(play.report.drain(..));
        let pit_spots: Vec<[f32; 3]> = sp.pits.iter().map(|s| s.pos).collect();
        if let Some(r) = super::gameplay::pit_slot_report(&play, &pit_spots) {
            report.push(format!("  layout {:?}: {r}", l.name));
        }
        if let Some(st) = match &frame { Some(fr) => super::gameplay::write_preset(&out, &super::gameplay::preset_name(&layout_names[li]), &g, &kinds, &play, fr)?, None => None } {
            presets += 1;
            report.push(format!("  layout {:?}: dynamic track {} tiles, {} road + {} verge cells, rubber-free {:.1} %, rubber max {}, road grip p10/p50/p90 {:?}",
                l.name, st.tiles, st.road_cells, st.verge_cells, st.rubber_zero * 100.0, st.rubber_max, st.grip_road));
        }
        // the centre spline follows our ideal line (on the ground)
        let line = play.ideal.as_ref().map(|i| i.samples.iter().map(|s| s.pos).collect()).unwrap_or(line);
        let closed = play.ideal.as_ref().map(|i| i.closed).unwrap_or(closed);
        work.push(LayoutWork { stem: layout_stem(&l.name), spawns: sp, bounds, length, ground_min, line, closed, play });
        report.push(format!("  layout {:?}: ground {} tris, grid {}, pits {}, hotlap {}, gates {}", l.name, g.triangle_count(),
            work.last().unwrap().spawns.grid.len(), work.last().unwrap().spawns.pits.len(), work.last().unwrap().spawns.hotlap.len(), work.last().unwrap().spawns.gates.len()));
        if li == 0 {
            ground0 = Some(g);
        }
    }
    // two AC1 layout folders may slug to the same stem
    let mut stems = HashSet::new();
    for w in &mut work {
        while !stems.insert(w.stem.clone()) {
            w.stem.push('x');
        }
    }

    // ---- look, meshes and materials
    let looks = super::look::assign(&scan);
    notes.extend(looks.notes.iter().cloned());
    // ---- vegetation: AC1 cards and CSP lists -> factory plants (the
    // replaced cards are not drawn), the grass carpet
    let vopt = super::vegetation::Options::from_env();
    let geo0 = geo::of(&scan.layouts.iter().map(|l| l.ui.country.clone()).find(|c| !c.is_empty()).unwrap_or_default(),
        scan.layouts.iter().find_map(|l| l.ui.geo));
    let lines: Vec<Vec<[f32; 3]>> = work.iter().map(|w| w.line.clone()).collect();
    let line_index = super::vegetation::LineIndex::new(&lines);
    let cat = match (&pkg, vopt.trees || vopt.grass) {
        (Some(p), true) => {
            let t = std::time::Instant::now();
            let c = super::vegcat::load(p);
            report.push(format!("  vegetation catalogue: {} trees, {} bushes, {} impostors, {} grass tufts ({:.1} s)",
                c.count(super::vegcat::Family::Tree), c.count(super::vegcat::Family::Bush), c.count(super::vegcat::Family::Impostor), c.count(super::vegcat::Family::Grass), t.elapsed().as_secs_f64()));
            notes.extend(c.notes.iter().cloned());
            Some(c)
        }
        (None, true) => {
            notes.push("vegetation: no game package to take the factory plants from - AC1 trees kept".into());
            None
        }
        _ => None,
    };
    let veg = match (&cat, &ground0, vopt.trees) {
        (Some(c), Some(g0), true) => {
            let v = super::vegetation::plan(&super::vegetation::Inputs {
                scan: &scan, cat: c, ground: g0, kinds: &kinds, lines: &line_index, climate: super::vegcat::climate_of(geo0.latitude, geo0.located),
                track_id: &id, grass_on: vopt.grass,
            });
            report.extend(v.report.iter().cloned());
            notes.extend(v.notes.iter().cloned());
            v
        }
        _ => super::vegetation::Plan { drop: vec![false; scan.meshes.len()], ..Default::default() },
    };
    let plan = meshes::plan_except(&scan, &looks.of_mesh, &veg.drop);
    let tpl = super::materials::Templates::load(pkg.as_ref());
    let mats = super::materials::write(&scan, &id, &out, &plan.used_materials, &tpl, &looks)?;
    notes.extend(mats.notes.iter().cloned());
    let mpath = |g: usize, two: bool, v: u16| mats.by_key.get(&(g, two, v)).map(|w| w.path.clone());
    // start lights carry their lamp class in the vertex colour (stock
    // start-light meshes: 1/255 in R for the red lamps, G green, B the rest)
    let colour_of = |v: u16| looks.variants.get(v as usize).and_then(|x| x.start_light).map(|t| {
        let k = 1.0 / 255.0;
        match t { 1 => [k, 0.0, 0.0, 0.0], 2 => [0.0, k, 0.0, 0.0], _ => [0.0, 0.0, k, 0.0] }
    });
    let mo = meshes::write(&scan, &plan, &id, &out, &mpath, &colour_of)?;
    let mut b = SceneBuilder::new(&id);
    let all = scan.all_layouts();
    let mut used_surfaces: BTreeSet<usize> = BTreeSet::new();
    for w in &mo.meshes {
        let mut e = meshes::entity(w, &scan);
        if looks.variants.get(w.variant as usize).and_then(|x| x.start_light).is_some() {
            e.kinds.push("starting_lights".into());
        }
        let physics = !matches!(w.role, GroupRole::Visual);
        if let GroupRole::Drawn(s) | GroupRole::Collider(s) = w.role {
            used_surfaces.insert(s);
        }
        if !physics && w.layouts == all {
            b.add(None, e);
        } else {
            for (li, lw) in work.iter().enumerate() {
                if w.layouts & (1 << li) != 0 {
                    b.add(Some(&format!("layout_{}", lw.stem)), e.clone());
                }
            }
        }
    }

    // ---- CSP lights
    let (lamp_groups, lamp_skipped) = super::csplights::read(&scan);
    let lights = super::csplights::add(&mut b, &lamp_groups);
    if !lamp_skipped.is_empty() || !lights.skipped.is_empty() {
        notes.push(format!("CSP lights skipped (flashing / seasonal / over the cap): {}", lamp_skipped.iter().chain(&lights.skipped).take(10).cloned().collect::<Vec<_>>().join(", ")));
    }

    // ---- vegetation entities: plants in the main scene, the carpet in a
    // container loaded with the scene (as stock)
    if let Some(c) = &cat {
        for e in super::vegetation::entities(&veg, c) {
            b.add(None, e);
        }
        if let (true, Some(g0)) = (vopt.grass, &ground0) {
            let t = super::grass::build(&super::grass::Inputs { scan: &scan, cat: c, ground: g0, kinds: &kinds, lines: &lines, track_id: &id, dropped: &veg.drop });
            report.extend(t.report.iter().cloned());
            notes.extend(t.notes.iter().cloned());
            for (k, group) in super::grass::containers(t.entities).into_iter().enumerate() {
                let stem = format!("{}_{}", super::grass::CONTAINER, k + 1);
                b.container(&stem, false);
                for e in group {
                    b.add(Some(&stem), e);
                }
            }
        }
    }
    drop(ground0);

    // ---- main scene: track info, surfaces, graphics settings
    let g = geo::of(&scan.layouts.iter().map(|l| l.ui.country.clone()).find(|c| !c.is_empty()).unwrap_or_default(),
        scan.layouts.iter().find_map(|l| l.ui.geo));
    if !g.located {
        notes.push("no geotags in ui_track.json: coordinates are the time zone's meridian at the equator".into());
    }
    if g.country.is_none() {
        notes.push("country not recognised: nation / continent left empty".into());
    }
    let mut info = Msg::default();
    let mut ti = Msg::default();
    ti.set_varint(1, 1);
    ti.set_str(2, &display);
    let mut c = Msg::default();
    c.push(1, Val::F64(g.longitude.to_le_bytes()));
    c.push(2, Val::F64(g.latitude.to_le_bytes()));
    ti.push(4, Val::Msg(c));
    if g.timezone != 0 {
        ti.set_varint(5, g.timezone as u64);
    }
    info.push(1, Val::Msg(ti));
    let mut e = Entity::new("track_info", "Track Info", 111, info);
    e.distance_override = Some(100000.0);
    b.add(None, e);
    let fac = surfaces::factory(pkg.as_ref());
    let conv: Vec<surfaces::Converted> = used_surfaces.iter().map(|&s| surfaces::convert(&scan.surfaces[s], &fac)).collect();
    b.add(None, Entity::new("surfaces", "Surface Def", 109, surfaces::actor_data(&conv)));
    let ground_min = work.iter().map(|w| w.ground_min).fold(f32::INFINITY, f32::min);
    let mut gs = Msg::default();
    gs.set_f32(1, (ground_min - 50.0).floor());
    gs.set_f32(50, 1.0);
    gs.set_f32(51, 0.02);
    gs.set_varint(60, 1);
    gs.set_str(61, "content\\tracks\\common_assets\\curves\\omni_haze_density_by_sun_elevation.curve");
    b.main.graphics = Some(gs);

    // ---- per layout containers: centre spline, spawns, timelines, pit
    // zones, DRS, TV cameras; the track's audio zones
    let mut decls = Vec::new();
    let mut ui_files: Vec<(String, String)> = Vec::new();
    let mut side_files: Vec<(String, Vec<u8>)> = Vec::new();
    {
        let (rev, missing) = (0..scan.layouts.len()).fold((Vec::new(), Vec::new()), |mut a, li| {
            let (r, m) = super::audio::reverbs(&scan, li);
            for x in r {
                if !a.0.iter().any(|y: &super::audio::Reverb| y.name == x.name) {
                    a.0.push(x);
                }
            }
            a.1.extend(m);
            a
        });
        if !missing.is_empty() {
            notes.push(format!("audio_sources.ini reverbs without their node: {}", missing.join(", ")));
        }
        let lo = work.iter().map(|w| w.bounds.0).fold([f32::INFINITY; 2], |a, b| [a[0].min(b[0]), a[1].min(b[1])]);
        let hi = work.iter().map(|w| w.bounds.1).fold([f32::NEG_INFINITY; 2], |a, b| [a[0].max(b[0]), a[1].max(b[1])]);
        let y = work.iter().map(|w| w.ground_min).fold(f32::INFINITY, f32::min);
        let c = super::audio::write(&mut b, &id, ([lo[0], y, lo[1]], [hi[0], y, hi[1]]), &rev, work.first().and_then(|w| w.play.ideal.as_ref()));
        report.push(format!("  audio: {c} with the ambience and {} reverb / underpass zone(s)", rev.len()));
    }
    for (li, lw) in work.iter().enumerate() {
        let lstem = format!("layout_{}", lw.stem);
        b.container(&lstem, true);
        for e in layout_splines(&lw.stem, &lw.line, lw.closed, lw.play.ideal.as_ref(), lw.play.pit.as_ref()) {
            b.add(Some(&lstem), e);
        }
        let made = spawns::write(&mut b, &lw.stem, &lw.spawns);
        let pick = |p: &str| made.iter().find(|m| m.starts_with(p)).cloned();
        let gp = super::gameplay::layout_scene(&mut b, &scan, li, &lw.stem, &lw.play);
        report.extend(gp.report.iter().cloned());
        side_files.extend(gp.files);
        decls.push(LayoutDecl {
            name: layout_names[li].clone(), ac1: scan.layouts[li].name.clone(), layout_container: lstem,
            timelines: pick("timelines_"), grid: pick("spawnpoints_grid_"), pits: pick("spawnpoints_pitlane_"), hotlap: pick("spawnpoints_hotlap_"),
            extra: gp.extra, tv: gp.tv, length_m: lw.length.round() as u32, grid_slots: lw.spawns.grid.len(), pit_slots: lw.spawns.pits.len(), bounds: lw.bounds,
        });
        // the menus' and HUD's pictures / maps of this layout (uiimages.rs)
        let ui_dir = scan.src.layouts.get(li).and_then(|d| d.ui_json.as_ref()).and_then(|p| p.parent().map(|d| d.to_path_buf()));
        ui_files.extend(super::uiimages::write(&out, &display, &layout_names[li], ui_dir.as_deref(), &lw.line, lw.closed, lw.bounds, &mut notes)?);
    }
    // ---- lighting bakes: vertex AO, irradiance volumes, pigment (bakes.rs)
    let canopies = cat.as_ref().map(|c| super::vegetation::canopies(&veg, c)).unwrap_or_default();
    let bake_lines = bake(&scan, &id, &out, &mo, &mats, &looks, &track_flags, &work.iter().map(|w| w.bounds).collect::<Vec<_>>(), canopies, &mut b)?;
    report.extend(bake_lines);
    let tinfo = super::gameplay::track_info(&display, &g);
    for (lw, l) in work.iter().zip(&scan.layouts) {
        let n = super::gameplay::write_splines(&out, &id, &lw.stem, &lw.play, &tinfo)?;
        if n == 0 {
            report.push(format!("  layout {:?}: no AI splines", l.name));
        }
    }
    for (rel, bytes) in &side_files {
        let p = out.join(rel.replace('\\', "/"));
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(&p, bytes)?;
    }
    let entity_ids = b.ids.count();
    for (rel, bytes) in b.finish() {
        let p = out.join(rel.replace('\\', "/"));
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(&p, bytes).with_context(|| format!("write {}", p.display()))?;
    }

    // ---- .track: dynamic track on at the stock base grip; the frame and
    // presets come with the dynamic-track stage
    let mut dt = Msg::default();
    dt.set_varint(2, 1);
    dt.set_f32(4, 0.98);
    match (&frame, presets > 0) {
        (Some(f), true) => {
            dt.push(8, Val::Msg(super::scene::v3(f.centre)));
            dt.set_f32(9, f.extent);
        }
        _ => dt.set_f32(9, super::dyntrack::STOCK_EXTENT),
    }
    let mut tr = Msg::default();
    tr.push(1, Val::Msg(dt));
    tr.push(2, Val::Msg(Msg::default()));
    if let Some(a) = scan.layouts.iter().find_map(|l| l.ui.altitude).filter(|a| a.is_finite() && a.abs() < 9000.0) {
        tr.set_f32(3, a as f32);
    }
    std::fs::write(out.join(format!("{id}.track")), tr.encode())?;

    // ---- declaration rows
    notes.extend(scan.notes.iter().cloned());
    let decl = declare::json(&id, &display, &scan.folder, &g, &decls, &ui_files, &notes);
    std::fs::write(out.join("ui").join("track_decl.json"), serde_json::to_string_pretty(&decl)?)?;

    // ---- report
    let tris: usize = mo.meshes.iter().map(|m| m.tris).sum();
    report.push(format!("{id} \"{display}\": {} meshes ({} collision-only), {} triangles, {} materials, {} textures ({:.0} MB), {} entity ids",
        mo.meshes.len(), mo.meshes.iter().filter(|m| matches!(m.role, GroupRole::Collider(_))).count(), tris, mats.by_key.len(), mats.textures, mats.texture_mb, entity_ids));
    report.push(format!("  back-to-back sheets: {} merged on two-sided materials, {} pairs pushed apart ({} at a second distance); {} far-LOD copies dropped; {} slivers dropped; {} backdrops without shadows; {} normals repaired; {} far-shadow meshes; {} LOD levels",
        plan.back_to_back_merged, plan.back_to_back_pushed, plan.push_retries, plan.far_lod_dropped, plan.slivers, plan.backdrops, mo.fixed_normals, mo.meshes.iter().filter(|m| m.far_shadow).count(), mo.lod_levels));
    let mut sh: Vec<_> = mats.shaders.iter().collect();
    sh.sort();
    report.push(format!("  look: AC1 reference light (sun {} deg, look.rs); materials by shader {:?}; {} variants; lights {} single + {} instanced ({} instances)",
        super::look::SUN_ELEVATION_DEG, sh, looks.variants.len() - 1, lights.singles, lights.instanced, lights.instances));
    for cv in &conv {
        report.push(format!("  surface {:<16} {:<9} grip {:.3}", cv.key, cv.kind.name(), cv.grip));
    }
    report.push(format!("  geo: {} / {} / tz {} / {:.4}, {:.4}", g.nation, g.continent, g.timezone, g.latitude, g.longitude));
    for n in &notes {
        report.push(format!("  note: {n}"));
    }
    let unused: Vec<&str> = scan.meshes.iter().filter_map(|m| if let Role::Physics { surface, .. } = m.role { (!used_surfaces.contains(&surface)).then(|| scan.surfaces[surface].key.as_str()) } else { None }).collect();
    if !unused.is_empty() {
        report.push(format!("  warn: physics surfaces with no mesh written: {unused:?}"));
    }
    // brightness: every material's estimated mean linear albedo (look_albedo.tsv)
    let mut rows = vec!["material\tshader\tclass\talbedo_r\talbedo_g\talbedo_b\tluma".to_string()];
    for (name, sh, class, a) in &mats.albedo {
        let y = 0.2126 * a[0] + 0.7152 * a[1] + 0.0722 * a[2];
        rows.push(format!("{name}\t{sh}\t{class}\t{:.3}\t{:.3}\t{:.3}\t{y:.3}", a[0], a[1], a[2]));
    }
    std::fs::write(out.join("ui").join("look_albedo.tsv"), rows.join("\n"))?;
    // the summary is what the brightness fit aims at: `aceconv albedo`'s
    // measurement of the written folder (stats::albedo: its own material
    // classes and skips, the written textures' means), against the stock
    // p25..p75. (It printed the per-material estimate above before: a
    // different model and class split - Brands road 0.077 / grass 0.131
    // while the fitted, measured medians were 0.131 / 0.175.)
    let med: Vec<String> = match crate::stats::albedo::measure_dir(&out, pkg.as_ref()) {
        Ok(m) => m.by_class.iter().map(|(k, v)| {
            let (_, b, _) = crate::stats::albedo::quart(v);
            let stock = super::materials::STOCK_CLASS.iter().find(|x| x.0 == *k).map(|x| format!(", stock {:.3}..{:.3}", x.1, x.2)).unwrap_or_default();
            format!("{k} {b:.3} (n {}{stock})", v.len())
        }).collect(),
        Err(e) => vec![format!("not measured: {e}")],
    };
    report.push(format!("  albedo (as `aceconv albedo` measures it: median linear luma by class): {}", med.join(", ")));
    report.push(format!("  {:.1} s", t0.elapsed().as_secs_f64()));
    std::fs::write(out.join("ui").join("convert_report.txt"), report.join("\n"))?;
    Ok(Converted { id, dir: out, report })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::ai::{Line, Sample};

    #[test]
    fn layout_splines_named_as_the_game_looks_them_up() {
        // a 100 m straight along +z: left (up x forward) = +x
        let samples: Vec<Sample> = (0..51).map(|i| Sample { pos: [0.0, 1.0, i as f32 * 2.0], dist: i as f32 * 2.0, fwd: [0.0, 0.0, 1.0], sides: [3.0, 6.0], ..Default::default() }).collect();
        let il = Line { samples, closed: false, length: 100.0, ..Default::default() };
        let line: Vec<[f32; 3]> = il.samples.iter().map(|s| s.pos).collect();
        let es = layout_splines("gp", &line, false, Some(&il), None);
        let names: Vec<&str> = es.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["gp_track_center_spline", "gp_track_limits_left", "gp_track_limits_right", "gp_ideal_line"]);
        assert_eq!(es[1].kinds, ["Spline", "track_limits_left"]);
        assert_eq!(es[3].kinds, ["Spline", "ideal_line"]);
        // the limits sit on the white lines: 3 m left (+x), 6 m right (-x)
        let node_x = |e: &Entity| -> f32 {
            let sd = e.actor.1.sub(1).unwrap();
            sd.subs(3)[0].sub(1).unwrap().sub(1).unwrap().f32(1).unwrap()
        };
        assert!((node_x(&es[1]) - 3.0).abs() < 1e-4 && (node_x(&es[2]) + 6.0).abs() < 1e-4);
        // the ideal line keeps every point and the stock half car width
        assert_eq!(es[3].actor.1.sub(1).unwrap().subs(3).len(), 51);
        assert_eq!(es[3].actor.1.sub(1).unwrap().f32(11), Some(crate::track::ai::HALF_CAR_WIDTH));
    }
}
