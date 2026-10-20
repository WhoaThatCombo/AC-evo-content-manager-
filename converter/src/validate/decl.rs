//! Declaration rows: simulate how a session finds its containers.
//!
//! Game facts (track_containers.table of content.kspkg, the session logs, the
//! UI scripts; see `track::declare`):
//! - a session names a track (tracks.table f1 = row f10 track_name), a
//!   layout (row f14 layout_name, the UI's `TrackLayouts.items[].layout`)
//!   and an event (row f1 event_name; gflag `ai_tester_track__eventname`:
//!   "track name + __ + event name (ie Brands Hatch__Indy Hotstint)");
//! - the mode rows: Race (f5 is_instant_race) for races, Time Attack (f6) for
//!   practice and time attack ("Fuji Speedway GP Short Time Attack
//!   Practice"), Hotstint (f7) for hot laps; "No Game Mode" (f15
//!   is_base_layout) is the layout's base, loaded in every mode - stock
//!   often keeps the layout container and spawns only there (24h Time
//!   Attack lists none of its own); academy rows carry f13;
//! - the containers loaded = base row + mode row; their "Start Pos"
//!   entities (f50.f104 StartingPositionData f6 packed types: 3 grid, 4
//!   pit, 5 box; stock hotlap starts are 3) are the spawn points the log
//!   counts per type. None of the type the mode starts on: "No starting
//!   position!" and the crash (ks_black_cat_county, a session naming layout
//!   "layout_short" / event "Black Cat County" that no row had).
//! - the UI derives image ids from (track, layout): `declare::ui_track_id`,
//!   `ui_map_id`.
//!
//! `strict` (our own output) also demands: unique layout names (by UI id),
//! names of letters / digits / spaces only, "<layout> <mode>" event names
//! unique in the track, all three mode rows plus one base row per layout.
//! Stock passes without `strict` (Watkins Glen's two "GP Hotstint" rows are
//! a warning).

use std::collections::{BTreeMap, BTreeSet};

use super::Report;
use crate::io::pb::{Msg, Val};
use crate::track::declare::{event_name, ui_map_id, ui_track_id};

#[derive(Clone, Debug, Default)]
pub struct Row {
    pub event: String,
    pub track: String,
    pub layout: String,
    pub race: bool,
    pub time_attack: bool,
    pub hotstint: bool,
    pub base: bool,
    pub academy: bool,
    pub max_drivers: u64,
    pub containers: Vec<String>,
}

/// One track_containers.table Line (f8 TrackContainersContentData).
pub fn row(line: &Msg) -> Option<Row> {
    let b = line.sub(8)?;
    Some(Row {
        event: b.str(1).unwrap_or_default(),
        track: b.str(10).unwrap_or_default(),
        layout: b.str(14).unwrap_or_default(),
        race: b.varint(5) == Some(1),
        time_attack: b.varint(6) == Some(1),
        hotstint: b.varint(7) == Some(1),
        base: b.varint(15) == Some(1),
        academy: b.varint(13) == Some(1),
        max_drivers: b.varint(9).unwrap_or(0),
        containers: b.fields.iter().filter(|f| f.num == 11).filter_map(|f| match &f.val { Val::Bytes(x) => Some(String::from_utf8_lossy(x).into_owned()), _ => None }).collect(),
    })
}

/// Every row of a track_containers.table blob.
pub fn table_rows(blob: &[u8]) -> Vec<Row> {
    let Ok(top) = Msg::parse(blob) else { return Vec::new() };
    top.sub(2).map(|t| t.subs(3).iter().filter_map(row).collect()).unwrap_or_default()
}

/// Spawn points per StartingPositionType in one container scene.
pub fn spawn_types(scene: &[u8]) -> BTreeMap<u64, usize> {
    let mut out = BTreeMap::new();
    for e in super::entities(scene) {
        if super::entity_kind(&e).as_deref() != Some("Start Pos") {
            continue;
        }
        let packed = e.sub(50).and_then(|d| d.sub(104)).and_then(|s| s.bytes(6).map(|b| b.to_vec())).unwrap_or_default();
        let types = crate::io::pb::varints(&packed);
        let any = !types.is_empty();
        for v in types {
            *out.entry(v).or_default() += 1;
        }
        if !any {
            *out.entry(0).or_default() += 1;
        }
    }
    out
}

pub const MODES: [(&str, &str); 3] = [("Race", "race"), ("Time Attack", "practice / time attack"), ("Hotstint", "hotstint")];

fn mode_of(r: &Row, mode: &str) -> bool {
    match mode {
        "Race" => r.race,
        "Time Attack" => r.time_attack,
        _ => r.hotstint,
    }
}

/// The spawn types a mode starts on: race on the grid, practice / time
/// attack in the pits (pit or box), hotstint on a hotlap start (stock
/// type 3; the Nordschleife's 4 + 5).
fn needs(mode: &str) -> &'static [u64] {
    match mode {
        "Race" => &[3],
        "Time Attack" => &[4, 5],
        // stock hotlap starts are grid-type (3), the Nordschleife's pit / box (4, 5)
        _ => &[3, 4, 5],
    }
}

fn clean_name(s: &str) -> bool {
    !s.is_empty() && !s.contains("  ") && s.trim() == s && s.chars().all(|c| c.is_ascii_alphanumeric() || c == ' ')
}

/// Check one track's rows. `read` returns a container's bytes (None =
/// missing). Returns one summary line per (layout, mode).
pub fn check(track: &str, rows: &[Row], read: &dyn Fn(&str) -> Option<Vec<u8>>, strict: bool, r: &mut Report) -> Vec<String> {
    let mut summary = Vec::new();
    let mine: Vec<&Row> = rows.iter().filter(|x| x.track == track && !x.academy).collect();
    if mine.is_empty() {
        r.err(format!("{track}: no container rows name this track (f10)"));
        return summary;
    }
    let layouts: BTreeSet<&str> = mine.iter().map(|x| x.layout.as_str()).collect();
    // layout names: unique by the UI's image ids, readable
    let mut ids: BTreeMap<String, &str> = BTreeMap::new();
    for l in &layouts {
        if l.trim().is_empty() {
            r.err(format!("{track}: a row has no layout name (f14)"));
        }
        if strict && !clean_name(l) {
            r.err(format!("{track}: layout name {l:?} - use letters, digits and single spaces (the UI's two image ids differ otherwise)"));
        }
        for id in [ui_track_id(track, l), ui_map_id(track, l)] {
            if let Some(o) = ids.insert(id.clone(), l) {
                if o != *l {
                    let m = format!("{track}: layouts {o:?} and {l:?} share the UI id {id}");
                    if strict { r.err(m) } else { r.warn(m) }
                }
            }
        }
    }
    // event names: one session id per mode row
    let mut events: BTreeMap<&str, Vec<&Row>> = BTreeMap::new();
    for x in mine.iter().filter(|x| x.race || x.time_attack || x.hotstint) {
        events.entry(x.event.as_str()).or_default().push(x);
    }
    for (e, xs) in &events {
        let lays: BTreeSet<&str> = xs.iter().map(|x| x.layout.as_str()).collect();
        if xs.len() > 1 {
            let m = format!("{track}: event name {e:?} on {} rows (layouts {lays:?}) - the session id track__event is ambiguous", xs.len());
            if strict { r.err(m) } else { r.warn(m) }
        }
    }
    let mut cache: BTreeMap<String, Option<BTreeMap<u64, usize>>> = BTreeMap::new();
    let mut spawns_of = |p: &str| -> Option<BTreeMap<u64, usize>> { cache.entry(p.to_lowercase()).or_insert_with(|| read(p).map(|b| spawn_types(&b))).clone() };
    for l in &layouts {
        let base: Vec<&&Row> = mine.iter().filter(|x| x.layout == *l && x.base).collect();
        match base.len() {
            1 => {}
            0 => {
                let m = format!("{track} / {l}: no No Game Mode (f15 is_base_layout) row");
                if strict { r.err(m) } else { r.warn(m) }
            }
            n => r.warn(format!("{track} / {l}: {n} base rows")),
        }
        let playable = mine.iter().any(|x| x.layout == *l && (x.race || x.time_attack || x.hotstint));
        if !playable {
            summary.push(format!("{track} / {l}: no mode rows (not offered)"));
            if strict {
                r.err(format!("{track} / {l}: no Race / Time Attack / Hotstint row"));
            }
            continue;
        }
        for (mode, what) in MODES {
            let rows_m: Vec<&&Row> = mine.iter().filter(|x| x.layout == *l && mode_of(x, mode)).collect();
            let Some(row) = rows_m.first() else {
                let m = format!("{track} / {l}: no {mode} row - {what} cannot start on this layout");
                if strict { r.err(m) } else { summary.push(format!("{track} / {l} / {mode}: not offered")) }
                continue;
            };
            if strict && row.event != event_name(l, mode) {
                r.err(format!("{track} / {l}: {mode} row is named {:?}, not {:?}", row.event, event_name(l, mode)));
            }
            // the session's lookup by track + event must land on this layout
            let by_event: Vec<&&Row> = mine.iter().filter(|x| x.event == row.event && mode_of(x, mode)).collect();
            if by_event.first().map(|x| x.layout != row.layout).unwrap_or(true) {
                let m = format!("{track} / {l}: session id {track}__{} resolves to layout {:?}", row.event, by_event.first().map(|x| x.layout.as_str()).unwrap_or(""));
                if strict { r.err(m) } else { r.warn(m) }
            }
            let mut files: Vec<&String> = base.iter().flat_map(|b| b.containers.iter()).collect();
            files.extend(row.containers.iter());
            let mut have: BTreeMap<u64, usize> = BTreeMap::new();
            for f in &files {
                if !f.to_lowercase().ends_with(".scene") {
                    continue;
                }
                match spawns_of(f) {
                    Some(t) => {
                        for (k, n) in t {
                            *have.entry(k).or_default() += n;
                        }
                    }
                    None => r.err(format!("{track} / {l} / {mode}: container {f} does not exist")),
                }
            }
            let n: usize = needs(mode).iter().map(|t| have.get(t).copied().unwrap_or(0)).sum();
            summary.push(format!("{track} / {l} / {mode}: row {:?}, {} containers, spawns {:?}", row.event, files.len(), have));
            if n == 0 {
                r.err(format!("{track} / {l} / {mode}: the loaded containers hold no spawn point of type {:?} (have {have:?}) - \"No starting position!\"", needs(mode)));
            }
            if mode == "Race" && row.max_drivers > 0 && (have.get(&3).copied().unwrap_or(0) as u64) < row.max_drivers.min(2) {
                r.warn(format!("{track} / {l}: Race row allows {} drivers on {} grid slots", row.max_drivers, have.get(&3).copied().unwrap_or(0)));
            }
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(event: &str, layout: &str, mode: u32, c: &[&str]) -> Row {
        Row {
            event: event.into(), track: "T".into(), layout: layout.into(), race: mode == 5, time_attack: mode == 6, hotstint: mode == 7, base: mode == 15,
            academy: false, max_drivers: 2, containers: c.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn lookup_needs_spawns_of_the_mode() {
        let rows = vec![
            r("GP Race", "GP", 5, &["grid.scene"]), r("GP Time Attack", "GP", 6, &[]), r("GP Hotstint", "GP", 7, &["hot.scene"]), r("No Game Mode", "GP", 15, &["pit.scene"]),
        ];
        let start = |t: u64| {
            let mut d = Msg::default();
            let mut s = Msg::default();
            s.push(6, Val::Bytes(vec![t as u8]));
            d.push(104, Val::Msg(s));
            let mut e = Msg::default();
            e.set_str(1, "x");
            e.push(3, Val::Bytes(b"Start Pos".to_vec()));
            e.push(50, Val::Msg(d));
            let mut sc = Msg::default();
            sc.push(2, Val::Msg(e));
            sc.encode()
        };
        let read = |p: &str| -> Option<Vec<u8>> { match p { "grid.scene" | "hot.scene" => Some(start(3)), "pit.scene" => Some(start(4)), _ => None } };
        let mut rep = Report::default();
        let s = check("T", &rows, &read, true, &mut rep);
        assert!(rep.errors.is_empty(), "{:?}", rep.errors);
        assert_eq!(s.len(), 3);
        // a layout whose Time Attack row and base row hold no pit spawn
        let rows2 = vec![r("GP Race", "GP", 5, &["grid.scene"]), r("GP Time Attack", "GP", 6, &[]), r("GP Hotstint", "GP", 7, &["hot.scene"]), r("No Game Mode", "GP", 15, &[])];
        let mut rep = Report::default();
        check("T", &rows2, &read, true, &mut rep);
        assert!(rep.errors.iter().any(|e| e.contains("Time Attack") && e.contains("No starting position")), "{:?}", rep.errors);
        // two layouts with one UI id
        let rows3 = vec![r("A B Race", "A B", 5, &["grid.scene"]), r("A-B Race", "A-B", 5, &["grid.scene"])];
        let mut rep = Report::default();
        check("T", &rows3, &read, true, &mut rep);
        assert!(rep.errors.iter().any(|e| e.contains("share the UI id")), "{:?}", rep.errors);
    }
}
