//! The declaration rows that make a converted track exist in the game:
//! one `system\tracks.table` row and the `system\track_containers.table`
//! rows of every layout and game mode. The converter never writes
//! content.kspkg: it returns the rows in `ui\track_decl.json` and ACECM
//! applies them (with its backup / verify / undo).
//!
//! Table layout (ACECM `protos\CustomTable.proto.txt`, `LogicScene.proto.txt`):
//! TableData f2 PresetTableData, f3 (repeated) Line; a tracks.table Line
//! carries f2 TrackContentData {f1 name, f3 folder_path, f4 file_path (the
//! .scene), f5 nation, f6 coordinates {f1 longitude, f2 latitude} (doubles),
//! f7 timezone, f8 track_data_path (the .track), f10 is_release, f13
//! continent}; a track_containers.table Line carries f8
//! TrackContainersContentData {f1 event_name, f5 instant race, f6 time
//! attack, f7 hot stint, f8 track_length (m), f9 max_drivers, f10
//! track_name, f11 container paths, f14 layout_name, f15 is_base_layout,
//! f16 is_default, f17 test_stage, f18 layout_image}.
//!
//! What stock does (the 18 Kunos circuits; road_atlanta's rows are not
//! used, a local install replaced them): four rows per layout - "<Layout>
//! Race" (f5, f9 = grid size; layout, timelines, grid and pit spawns),
//! "<Layout> Time Attack" (f6; layout, timelines, pit spawns), "<Layout>
//! Hotstint" (f7, f9 = 1; layout, timelines, hotlap spawns), and "No Game
//! Mode" (f15; TV cameras and the shared camera sequence). Every row has
//! f16 = 1 and f17 = 10. f18 = {f1 margin 10, f2 / f3 the track's minimum
//! x / z, f4 its larger extent, f5 or f6 the slack on the shorter axis
//! (f4 - extent: the map texture centres the track, half of it each side;
//! measured on brands_hatch-gp, x right, z down)}. The default layout's No
//! Game Mode row has f20 ui_priority 1. f21 (backend id) is left out: it
//! names Kunos's server-side records.
//!
//! How a session finds its rows (stock evidence, `aceconv check-track-decl`):
//! - track_name (f10) = the tracks.table row's name (f1);
//! - layout_name (f14) = the layout the UI lists (`TrackLayouts.items[].layout`)
//!   and the one a saved session names (`GameModeType_*.gamemodesave`);
//! - event_name (f1) = the mode row: Race (f5) for races, Time Attack (f6)
//!   for practice and time attack ("Fuji Speedway GP Short Time Attack
//!   Practice"), Hotstint (f7) for hot laps; "No Game Mode" (f15
//!   is_base_layout) is the layout's base row, loaded under every mode
//!   (stock puts the layout container and often the spawns there: 24h Time
//!   Attack lists no spawn file of its own).
//! A session naming a layout / event no row has loads no container at all:
//! "0 spawn points" of every type, "No starting position!", crash. Stock
//! row names are not always "<layout> <mode>" (Watkins Glen's GP Inner Loop
//! has a "GP Hotstint"); ours always are, and every layout name is unique.
//!
//! track_decl.json: `tracks_table_row` (hex of one tracks.table Line),
//! `container_rows` (hex of each track_containers.table Line, a Race row
//! first), `layouts` (per layout: names, event names, UI image ids,
//! containers, rows), `ui_files` (UI images to place at the mods root),
//! `geo`, `aceconv_source` (the AC1 folder; `aceconv legacy-id` finds
//! earlier conversions by it).

use serde_json::{json, Value};

use super::geo::Geo;
use crate::io::pb::{Msg, Val};

/// The shared camera sequence the newer stock circuits' No Game Mode rows
/// list (oulton_park, paul_ricard, sebring, watkins_glen; kyalami the
/// `_right` one; the older ones list their own world-placed
/// `camera_sequence_practice` in the Race / Time Attack rows, which nests
/// this one). It is what the pit menu (camera mode 10) plays: Camera
/// entities tagged `camera_sequence` following Splines tagged
/// `camera_spline`, with CamSeqData f5 spline_local, i.e. relative to the
/// focused car (the exe's CameraSequence update multiplies the spline
/// point and the camera matrix by the target's matrix). The game loads it
/// only through a main-scene Container entity with its path (see
/// `SceneBuilder::external`).
pub const CAMERA_SEQUENCE: &str = "content\\tracks\\common_assets\\containers\\common_camera_sequences\\common_car_cam_sequence_left.scene";
/// The kinds that file holds (stock brands_hatch declares the same on its
/// `camera_sequence_car` Container entity).
pub const CAMERA_SEQUENCE_KINDS: [&str; 4] = ["Camera", "Spline", "camera_sequence", "camera_spline"];

#[derive(Clone, Debug)]
pub struct LayoutDecl {
    /// display name ("GP", "Indy", "Full")
    pub name: String,
    /// AC1 layout folder ("" = root)
    pub ac1: String,
    pub layout_container: String,
    pub timelines: Option<String>,
    pub grid: Option<String>,
    pub pits: Option<String>,
    pub hotlap: Option<String>,
    /// containers listed in every gameplay row besides the above
    pub extra: Vec<String>,
    pub tv: Vec<String>,
    pub length_m: u32,
    pub grid_slots: usize,
    pub pit_slots: usize,
    /// xz bounds of the track line (min, max)
    pub bounds: ([f32; 2], [f32; 2]),
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn double(v: f64) -> Val {
    Val::F64(v.to_le_bytes())
}

/// The tracks.table Line.
pub fn track_row(id: &str, display: &str, geo: &Geo) -> Msg {
    let mut d = Msg::default();
    d.set_str(1, display);
    d.set_str(3, &format!("content\\tracks\\{id}"));
    d.set_str(4, &format!("content\\tracks\\{id}\\{id}.scene"));
    if !geo.nation.is_empty() {
        d.set_str(5, &geo.nation);
    }
    let mut c = Msg::default();
    c.push(1, double(geo.longitude));
    c.push(2, double(geo.latitude));
    d.push(6, Val::Msg(c));
    if geo.timezone != 0 {
        d.set_varint(7, geo.timezone as u64);
    }
    d.set_str(8, &format!("content\\tracks\\{id}\\{id}.track"));
    d.set_varint(10, 1);
    if !geo.continent.is_empty() {
        d.set_str(13, &geo.continent);
    }
    let mut line = Msg::default();
    line.push(2, Val::Msg(d));
    line
}

fn image(b: ([f32; 2], [f32; 2])) -> Msg {
    let (lo, hi) = b;
    let (ex, ez) = (hi[0] - lo[0], hi[1] - lo[1]);
    let k = ex.max(ez).max(1.0);
    let mut m = Msg::default();
    m.set_varint(1, 10);
    m.set_f32(2, lo[0]);
    m.set_f32(3, lo[1]);
    m.set_f32(4, k);
    // stock: f5 / f6 = the whole slack on the shorter axis; the map image
    // centres the track in it (brands_hatch-gp: x extent 822 m in a 949 m
    // square, f5 126.8, drawn 63.4 m in)
    if ex < ez {
        m.set_f32(5, k - ex);
    } else if ez < ex {
        m.set_f32(6, k - ez);
    }
    m
}

/// The four track_containers.table Lines of one layout: Race, Time
/// Attack, Hotstint, No Game Mode (in that order).
pub fn container_rows(id: &str, display: &str, l: &LayoutDecl, first: bool) -> Vec<(String, Msg)> {
    let path = |stem: &str| format!("content\\tracks\\{id}\\containers\\{stem}.scene");
    let mut out = Vec::new();
    let max = l.grid_slots.min(l.pit_slots).max(1);
    for (mode, flag) in [("Race", 5u32), ("Time Attack", 6), ("Hotstint", 7), ("No Game Mode", 15)] {
        let mut b = Msg::default();
        b.set_str(1, &event_name(&l.name, mode));
        if flag != 15 {
            b.set_varint(flag, 1);
        }
        if l.length_m > 0 {
            b.set_varint(8, l.length_m as u64);
        }
        match flag {
            5 | 6 => b.set_varint(9, max as u64),
            7 => b.set_varint(9, 1),
            _ => {}
        }
        b.set_str(10, display);
        let mut list: Vec<String> = Vec::new();
        if flag == 15 {
            list.extend(l.tv.iter().map(|s| path(s)));
            list.push(CAMERA_SEQUENCE.to_string());
        } else {
            list.push(path(&l.layout_container));
            if let Some(t) = &l.timelines {
                list.push(path(t));
            }
            let spawns: Vec<&Option<String>> = match flag {
                5 => vec![&l.grid, &l.pits],
                6 => vec![&l.pits],
                _ => vec![&l.hotlap],
            };
            for s in spawns.into_iter().flatten() {
                list.push(path(s));
            }
            list.extend(l.extra.iter().map(|s| path(s)));
        }
        for p in list {
            b.push(11, Val::Bytes(p.into_bytes()));
        }
        b.set_str(14, &l.name);
        if flag == 15 {
            b.set_varint(15, 1);
        }
        b.set_varint(16, 1);
        b.set_varint(17, 10);
        // stock: the default layout's No Game Mode row has ui_priority 1
        if flag == 15 && first {
            b.set_varint(20, 1);
        }
        if flag == 15 {
            b.push(18, Val::Bytes(Vec::new()));
        } else {
            b.push(18, Val::Msg(image(l.bounds)));
        }
        let mut line = Msg::default();
        line.push(8, Val::Msg(b));
        out.push((event_name(&l.name, mode), line));
    }
    out
}

/// The row name (event_name, f1) of a layout's mode: "<layout> <mode>",
/// "No Game Mode" for the base row (stock). The game names a session by the
/// track name and this (gflag `ai_tester_track__eventname`: "track name +
/// __ + event name (ie Brands Hatch__Indy Hotstint)"), and its "Game
/// Started!" log line prints them ("Fuji Speedway GP Short Time Attack").
pub fn event_name(layout: &str, mode: &str) -> String {
    if mode == "No Game Mode" { mode.to_string() } else { format!("{layout} {mode}") }
}

/// The UI's image id of a (track, layout) (`ksUI.getTrackId` in
/// `uiresources\js\components.js`): name trimmed with its FIRST "-" made
/// "_", "-", layout trimmed, every space "_", lower case. Used by
/// `images\tracks\<id>.texture` and `images\trackmaps\<id>.svg`.
pub fn ui_track_id(track: &str, layout: &str) -> String {
    format!("{}-{}", track.trim().replacen('-', "_", 1), layout.trim()).replace(' ', "_").to_lowercase()
}

/// The in-session map's id (`selectTrackMap`: `slug(name)-slug(layout)`,
/// slug = lower case with every run of non `[a-z0-9]` as "_"), used by
/// `images\trackmaps\<id>.texture`.
pub fn ui_map_id(track: &str, layout: &str) -> String {
    let slug = |s: &str| {
        let mut o = String::new();
        let mut gap = false;
        for c in s.trim().to_lowercase().chars() {
            if c.is_ascii_alphanumeric() {
                o.push(c);
                gap = false;
            } else if !gap {
                o.push('_');
                gap = true;
            }
        }
        o
    };
    format!("{}-{}", slug(track), slug(layout))
}

/// The whole ui\track_decl.json.
pub fn json(id: &str, display: &str, source: &str, geo: &Geo, layouts: &[LayoutDecl], ui_files: &[(String, String)], notes: &[String]) -> Value {
    let row = track_row(id, display, geo);
    let mut rows: Vec<Value> = Vec::new();
    let mut lay: Vec<Value> = Vec::new();
    // every layout's Race row first (the validator and ACECM read row 0 as a Race row)
    let per: Vec<Vec<(String, Msg)>> = layouts.iter().enumerate().map(|(i, l)| container_rows(id, display, l, i == 0)).collect();
    let mut order: Vec<(usize, usize)> = Vec::new();
    for (li, r) in per.iter().enumerate() {
        order.push((li, 0));
        let _ = r;
    }
    for (li, r) in per.iter().enumerate() {
        for k in 1..r.len() {
            order.push((li, k));
        }
    }
    let mut index_of: Vec<Vec<usize>> = vec![Vec::new(); layouts.len()];
    for (li, k) in order {
        index_of[li].push(rows.len());
        rows.push(json!(hex(&per[li][k].1.encode())));
    }
    for (li, l) in layouts.iter().enumerate() {
        lay.push(json!({
            "name": l.name, "layout_name": l.name, "ac1_layout": l.ac1,
            // what a session must name to load this layout: track name + layout_name + one of these
            "event_names": {
                "race": event_name(&l.name, "Race"), "time_attack": event_name(&l.name, "Time Attack"),
                "practice": event_name(&l.name, "Time Attack"), "hotstint": event_name(&l.name, "Hotstint"),
            },
            "ui_track_id": ui_track_id(display, &l.name), "ui_map_id": ui_map_id(display, &l.name), "length_m": l.length_m, "grid_slots": l.grid_slots, "pit_slots": l.pit_slots,
            "containers": {
                "layout": l.layout_container, "timelines": l.timelines, "spawnpoints_grid": l.grid, "spawnpoints_pitlane": l.pits,
                "spawnpoints_hotlap": l.hotlap, "tv": l.tv, "extra": l.extra,
            },
            "rows": per[li].iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(),
            "row_indices": index_of[li],
        }));
    }
    json!({
        "format": "aceconv-track-decl-1",
        "track_id": id,
        "display_name": display,
        "aceconv_source": source,
        "tracks_table_row": hex(&row.encode()),
        "container_rows": rows,
        "layouts": lay,
        "geo": {
            "nation": geo.nation, "continent": geo.continent, "timezone": geo.timezone,
            "latitude": geo.latitude, "longitude": geo.longitude, "located": geo.located,
            "country": geo.country.as_ref().map(|c| c.name.clone()),
        },
        // UI images: copy each `from` (relative to the track folder) to `to`
        // under the mods root (Saved Games\ACE\mods\), or pack them at
        // the root of a mod kspkg: coui serves uiresources\<path> from the
        // file system root only
        "ui_files": ui_files.iter().map(|(f, t)| json!({"from": f, "to": t})).collect::<Vec<_>>(),
        "notes": notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_like_stock() {
        let l = LayoutDecl {
            name: "GP".into(), ac1: "gp".into(), layout_container: "layout_gp".into(), timelines: Some("timelines_gp".into()),
            grid: Some("spawnpoints_grid_gp".into()), pits: Some("spawnpoints_pitlane_gp".into()), hotlap: Some("spawnpoints_hotlap_gp".into()),
            extra: vec![], tv: vec![], length_m: 3900, grid_slots: 24, pit_slots: 30, bounds: ([-400.0, -500.0], [500.0, 300.0]),
        };
        let rows = container_rows("t", "Track", &l, true);
        assert_eq!(rows.len(), 4);
        let race = rows[0].1.sub(8).unwrap();
        assert_eq!(race.str(1).as_deref(), Some("GP Race"));
        assert_eq!((race.varint(5), race.varint(9), race.varint(8)), (Some(1), Some(24), Some(3900)));
        assert_eq!(race.all(11).count(), 4);
        let img = race.sub(18).unwrap();
        assert_eq!((img.f32(2), img.f32(4), img.f32(6)), (Some(-400.0), Some(900.0), Some(100.0)));
        let hot = rows[2].1.sub(8).unwrap();
        assert_eq!((hot.varint(7), hot.varint(9)), (Some(1), Some(1)));
        let ngm = rows[3].1.sub(8).unwrap();
        assert_eq!((ngm.varint(15), ngm.varint(20)), (Some(1), Some(1)));
        assert_eq!(ui_track_id("Paul Ricard", "Layout 1A-V2"), "paul_ricard-layout_1a-v2");
        assert_eq!(ui_map_id("Rally Alsace - Munster", "Rally Munster"), "rally_alsace_munster-rally_munster");
        assert_eq!(ui_track_id("Rally Alsace - Munster", "Rally Munster"), "rally_alsace___munster-rally_munster");
        let g = super::super::geo::of("UK", Some((52.07, -1.02)));
        let t = track_row("t", "Track", &g).sub(2).unwrap();
        assert_eq!(t.str(5).as_deref(), Some("GBR"));
        assert_eq!(t.str(8).as_deref(), Some("content\\tracks\\t\\t.track"));
    }
}
