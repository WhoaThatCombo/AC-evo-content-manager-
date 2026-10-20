//! The gameplay stage of `aceconv track`: per layout the AI splines and
//! control points (`ai`), the pit lane and its zones (`pits`), the
//! dynamic-track preset (`dyntrack`), the TV cameras (`tvcams`) and DRS
//! zones (`drs`); per track the audio zones (`audio`). `build.rs` calls
//! these in order; each writes its own files.

use anyhow::{Context, Result};
use std::path::Path;

use super::ai::{self, Line};
use super::classes::Kind;
use super::dyntrack;
use super::ground::Ground;
use super::pits;
use super::scan::{Role, Scan};
use super::scene::SceneBuilder;
use crate::io::pb::{Msg, Val};

#[derive(Default)]
pub struct Play {
    pub ideal: Option<Line>,
    pub pit: Option<Line>,
    pub zones: Option<pits::Zones>,
    pub cps: Vec<ai::ControlPoint>,
    pub report: Vec<String>,
    pub notes: Vec<String>,
}

/// The dynamic-track frame of the whole track: the box of every drivable
/// physics mesh (all layouts).
pub fn frame(scan: &Scan, kinds: &[Kind]) -> Option<dyntrack::Frame> {
    let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for m in &scan.meshes {
        let Role::Physics { surface, .. } = m.role else { continue };
        let k = kinds.get(surface).copied().unwrap_or(Kind::Road);
        if !(k.track() || k == Kind::Verge) {
            continue;
        }
        for p in scan.positions(m) {
            for c in 0..3 {
                lo[c] = lo[c].min(p[c]);
                hi[c] = hi[c].max(p[c]);
            }
        }
    }
    lo[0].is_finite().then(|| dyntrack::frame_for(lo, hi))
}

fn closed_of(a: &crate::ac1::ai::AiLine) -> bool {
    a.points.len() > 2 && ai::dist3(a.points[0].pos, a.points[a.points.len() - 1].pos) < 50.0
}

/// Lines, control points and pit zones of layout `li`. `start` (a grid
/// slot: position, forward) seeds a traced centre line when the layout has
/// no AC1 fast lane.
pub fn lines(scan: &Scan, li: usize, g: &Ground, kinds: &[Kind], start: Option<([f32; 3], [f32; 3])>) -> Play {
    let l = &scan.layouts[li];
    let mut p = Play::default();
    let fast = l.fast_lane.as_ref().filter(|a| a.points.len() > 8);
    let (pts, closed): (Vec<[f32; 3]>, bool) = match fast {
        Some(f) => (f.points.iter().map(|q| q.pos).collect(), closed_of(f)),
        None => match start.and_then(|(s, f)| ai::trace_centre(g, kinds, s, f)) {
            Some((pts, closed)) => {
                p.notes.push(format!("no AC1 fast_lane.ai: the ideal line is the road centre traced from grid slot 0 ({}, {} points)", if closed { "closed" } else { "open, the road was lost" }, pts.len()));
                // the traced points sit on the road; ai::build expects AC1 heights
                (pts, closed)
            }
            None => {
                p.notes.push("no AC1 fast_lane.ai and no road centre could be traced: no AI splines, pit zones, TV cameras, DRS or dynamic track".into());
                return p;
            }
        },
    };
    let ideal = ai::build(&pts, closed, ai::IDEAL_STEP, g, kinds, false, fast);
    let cps = ai::control_points(&ideal);
    let sides: Vec<f32> = ideal.samples.iter().map(|s| s.sides[0] + s.sides[1]).collect();
    let mut sw = sides.clone();
    sw.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let walls_open = ideal.samples.iter().filter(|s| s.walls[0] >= ai::WALL_CAP || s.walls[1] >= ai::WALL_CAP).count();
    p.report.push(format!(
        "  layout {:?}: ideal line {:.0} m ({}{}), {} points ({:.2} m each), {:.0} % on the ground ({:.2} m above it in AC1), width p10/p50/p90 {:.1}/{:.1}/{:.1} m, {} % of points see no wall within 80 m, {} control points, lap {:.1} s ({} speeds)",
        l.name, ideal.length, if closed { "closed" } else { "open" }, if fast.is_none() { ", traced" } else { "" }, ideal.samples.len(), ideal.length / ideal.samples.len().max(1) as f32, ideal.snapped * 100.0, ideal.offset,
        sw[sw.len() / 10], sw[sw.len() / 2], sw[sw.len() * 9 / 10], walls_open * 100 / ideal.samples.len().max(1), cps.len(), ideal.lap_time(), if ideal.ac1_speeds { "AC1" } else { "model" }));
    if ideal.snapped < 0.95 {
        p.notes.push(format!("only {:.0} % of the ideal line found the ground within 1.5 m", ideal.snapped * 100.0));
    }
    if let Some(pl) = l.pit_lane.as_ref().filter(|a| a.points.len() > 8) {
        let raw: Vec<[f32; 3]> = pl.points.iter().map(|q| q.pos).collect();
        let cut = pits::trim(&raw, &ideal);
        let mut pit = ai::build(&cut, false, ai::PIT_STEP, g, kinds, true, None);
        let (d0, d1) = (ideal.nearest(pit.samples[0].pos).1, ideal.nearest(pit.samples[pit.samples.len() - 1].pos).1);
        // stock pit splines start and end on the ideal line (0..6 m)
        if d0 > 20.0 || d1 > 20.0 {
            p.notes.push(format!("the AC1 pit lane does not join the ideal line (ends {d0:.0} / {d1:.0} m away): no pit spline or pit zones"));
            p.cps = cps;
            p.ideal = Some(ideal);
            return p;
        }
        let z = pits::zones(&mut pit, &ideal, kinds);
        p.report.push(format!("  layout {:?}: pit lane {:.0} m (AC1 {:.0} m), limiter zone {:.0} m ({}), start / end {:.1} / {:.1} m off the ideal line",
            l.name, pit.length, pl.length(), z.main_length, if z.main_from_surfaces { "IS_PITLANE surfaces" } else { "where the lane leaves the track" },
            ideal.nearest(pit.samples[0].pos).1, ideal.nearest(pit.samples[pit.samples.len() - 1].pos).1));
        p.zones = Some(z);
        p.pit = Some(pit);
    } else {
        p.notes.push("no AC1 pit_lane.ai: no pit spline or pit zones (AI cannot pit)".into());
    }
    p.cps = cps;
    p.ideal = Some(ideal);
    p
}

/// Pit slots against the pit spline: lateral offsets (stock monza: 8 m).
pub fn pit_slot_report(play: &Play, slots: &[[f32; 3]]) -> Option<String> {
    let pit = play.pit.as_ref()?;
    if slots.is_empty() {
        return None;
    }
    let mut d: Vec<f32> = slots.iter().map(|s| pit.nearest(*s).1).collect();
    d.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Some(format!("pit slots {:.1} / {:.1} / {:.1} m off the pit spline (min / median / max)", d[0], d[d.len() / 2], d[d.len() - 1]))
}

/// SceneTrackInfo for the ideal line's f8.
pub fn track_info(display: &str, geo: &super::geo::Geo) -> Msg {
    let mut ti = Msg::default();
    ti.set_varint(1, 1);
    ti.set_str(2, display);
    let mut c = Msg::default();
    c.push(1, Val::F64(geo.longitude.to_le_bytes()));
    c.push(2, Val::F64(geo.latitude.to_le_bytes()));
    ti.push(4, Val::Msg(c));
    if geo.timezone != 0 {
        ti.set_varint(5, geo.timezone as u64);
    }
    ti
}

/// Write `layouts\layout_<stem>.{ideal_line,pitlane}.aisplinedata` and
/// `.trackcontrolpoints`.
pub fn write_splines(out: &Path, id: &str, stem: &str, play: &Play, info: &Msg) -> Result<usize> {
    let Some(ideal) = &play.ideal else { return Ok(0) };
    let dir = out.join("layouts");
    std::fs::create_dir_all(&dir)?;
    let cpm = ai::control_points_msg(ideal, &play.cps);
    let lp = format!("content\\tracks\\{id}\\containers\\layout_{stem}");
    let b = ai::encode(ideal, &ai::Extra { layout_path: Some(lp), control_points: Some(&cpm), track_info: Some(info), is_pitlane: false });
    std::fs::write(dir.join(format!("layout_{stem}.ideal_line.aisplinedata")), &b).context("write ideal line")?;
    std::fs::write(dir.join(format!("layout_{stem}.trackcontrolpoints")), cpm.encode())?;
    let mut n = 2;
    if let Some(pit) = &play.pit {
        let b = ai::encode(pit, &ai::Extra { layout_path: None, control_points: None, track_info: None, is_pitlane: true });
        std::fs::write(dir.join(format!("layout_{stem}.pitlane.aisplinedata")), &b)?;
        n += 1;
    }
    Ok(n)
}

/// The preset file name of a layout: its display name, lower case (stock:
/// `gp`, `gp short`, `layout 1a-v2`), without characters Windows refuses.
pub fn preset_name(display: &str) -> String {
    let s: String = display.to_lowercase().chars().map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { '_' } else { c }).collect();
    let s = s.trim().trim_end_matches('.').to_string();
    if s.is_empty() { "full".into() } else { s }
}

/// Build and write `dynamic_track\<name>.dynamictrackpresetcompressed`.
pub fn write_preset(out: &Path, name: &str, g: &Ground, kinds: &[Kind], play: &Play, frame: &dyntrack::Frame) -> Result<Option<dyntrack::Stats>> {
    let Some(ideal) = &play.ideal else { return Ok(None) };
    let mut lines = vec![ideal];
    lines.extend(play.pit.as_ref());
    let (p, st) = dyntrack::build(g, kinds, &lines, frame);
    let dir = out.join("dynamic_track");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(format!("{name}.dynamictrackpresetcompressed")), dyntrack::encode(&p)?)?;
    Ok(Some(st))
}

pub struct SceneOut {
    pub extra: Vec<String>,
    pub tv: Vec<String>,
    pub files: Vec<(String, Vec<u8>)>,
    pub report: Vec<String>,
}

/// Pit zones, DRS and TV cameras of one layout into the scene builder.
pub fn layout_scene(b: &mut SceneBuilder, scan: &Scan, li: usize, stem: &str, play: &Play) -> SceneOut {
    let mut o = SceneOut { extra: Vec::new(), tv: Vec::new(), files: Vec::new(), report: Vec::new() };
    let Some(ideal) = &play.ideal else { return o };
    let l = &scan.layouts[li];
    if let Some(z) = &play.zones {
        if let Some(c) = pits::write(b, stem, z) {
            o.extra.push(c);
        }
    }
    if let Some(ini) = l.data.get("drs_zones.ini") {
        let zones = super::drs::read(ini);
        if let Some(c) = super::drs::write(b, stem, &zones, ideal) {
            o.report.push(format!("  layout {:?}: {} DRS zone(s)", l.name, zones.len()));
            o.extra.push(c);
        }
    }
    let track = b.track.clone();
    let tv = super::tvcams::write(b, &track, stem, &l.data, ideal);
    if tv.cameras > 0 {
        o.report.push(format!("  layout {:?}: {} TV cameras in {} set(s)", l.name, tv.cameras, tv.stems.len()));
    }
    o.tv = tv.stems;
    o.files = tv.files;
    o
}
