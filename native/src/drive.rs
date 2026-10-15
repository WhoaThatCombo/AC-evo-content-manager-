//! Quick Drive - ported from acecm/drive.py (the job: start, join, capture).
//!
//! EVO will not take -car/-track and spawn. We write the GameModeSave
//! (track, time, mode), launch into that session and press the same buttons
//! a player would through the game's UI inspector (gameui.rs).
//!
//! Still answered by the backend for now: the Drive page's options and
//! public list (they merge the third-party directory) and the pasted-address
//! lookup. Everything that RUNS - the job and its state - is here.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use prost_reflect::{DynamicMessage, ReflectMessage, Value as PV};
use serde_json::{json, Map, Value};

use crate::{backend, carsmap, config, content, gamesettings, gameui, launch, lobby, logs, paths, procs, profiles, protos, serverproc, servers};

pub const DRIVE_ID: &str = "acecm-drive";

// ⚠ SP_MODES is what is ACCEPTED, OFFERED_MODES what the picker lists: a
// profile that saved a no-longer-offered mode must keep working.
pub const SP_MODES: [&str; 6] = ["PRACTICE", "INSTANT_RACE", "HOTLAP", "HOTSTINT", "RACE_WEEKEND", "TEST_DRIVE"];
pub const OFFERED_MODES: [&str; 3] = ["PRACTICE", "INSTANT_RACE", "RACE_WEEKEND"];
pub const AI_MODES: [&str; 2] = ["INSTANT_RACE", "RACE_WEEKEND"];
// exactly what the game's own slider offers
pub const AGGRO: [&str; 2] = ["Safe", "Preview"];
// straight out of the game's UI bundle; the sliders accept nothing else
const TIME_MULTIPLIERS: [i64; 7] = [1, 2, 4, 6, 12, 24, 48];
const MAX_OPPONENTS: i64 = 32;
const SKILL_MIN: i64 = 80;
const SKILL_MAX: i64 = 100;
const START_TIME_STEP_MIN: i64 = 5;

const TERMINAL: [&str; 4] = ["idle", "launched", "failed", "cancelled"];

fn defaults() -> Map<String, Value> {
    let v = json!({
        "via": "sp", "server_id": "", "local_id": "", "server_ip": "", "server_tcp_port": 0,
        "password": "", "car": "", "track_index": 18, "custom_track": "", "game_mode": "PRACTICE",
        "weather": "CLEAR", "grip": "OPTIMUM", "tod_hour": 13, "tod_minute": 0, "num_opponents": 10,
        "skill_min": 80, "skill_max": 95, "aggressiveness": "Safe", "single_make": true,
        "duration_min": 90, "practice_min": 10, "quali_min": 15, "warmup_min": 10, "race_laps": 10,
        "race_type": "LAPS", "race_minutes": 20, "time_mult": 1, "starting_position": 0,
    });
    match v { Value::Object(m) => m, _ => unreachable!() }
}

fn state_path() -> PathBuf {
    paths::data().join("drive.json")
}

fn now_f() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn now_i() -> i64 {
    now_f() as i64
}

// ------------------------------------------------------- python-ish bits --
fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

fn s_of(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(Value::Bool(b)) => if *b { "True".into() } else { "False".into() },
        Some(v) => v.to_string(),
    }
}

/// `x or ""` then str
fn s_or(v: Option<&Value>) -> String {
    if truthy(v) { s_of(v) } else { String::new() }
}

/// Python int(): numbers truncate, numeric strings parse, bools are 0/1
fn py_int(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f.trunc() as i64)),
        Value::String(s) => s.trim().parse().ok(),
        Value::Bool(b) => Some(*b as i64),
        _ => None,
    }
}

/// `int(x or 0)`
fn int0(v: Option<&Value>) -> i64 {
    if truthy(v) { v.and_then(py_int).unwrap_or(0) } else { 0 }
}

fn get<'a>(m: &'a Map<String, Value>, k: &str) -> Option<&'a Value> {
    m.get(k)
}

// --------------------------------------------------------------- the pick --
pub fn load_pick() -> Map<String, Value> {
    let got: Map<String, Value> = std::fs::read(state_path()).ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| if let Value::Object(m) = v { Some(m) } else { None })
        .unwrap_or_default();
    let mut out = defaults();
    for (k, v) in out.iter_mut() {
        if let Some(g) = got.get(k) {
            *v = g.clone();
        }
    }
    out
}

fn save_pick(pick: Map<String, Value>) -> Map<String, Value> {
    let _ = std::fs::create_dir_all(paths::data());
    let mut blob = load_pick();
    for (k, v) in pick {
        blob.insert(k, v);
    }
    let _ = profiles::write_json_indent2(&state_path(), &Value::Object(blob.clone()));
    blob
}

// ---------------------------------------------------------------- the job --
static JOB: Mutex<Option<Map<String, Value>>> = Mutex::new(None);
static CANCEL: AtomicBool = AtomicBool::new(false);
static JOB_THREAD: Mutex<Option<std::thread::ThreadId>> = Mutex::new(None);

fn with_job<T>(f: impl FnOnce(&mut Map<String, Value>) -> T) -> T {
    let mut g = JOB.lock().unwrap();
    let job = g.get_or_insert_with(|| match json!({"phase": "idle", "hint": "", "fault": "", "started": 0,
        "join": null, "launch": null, "wrote": null}) { Value::Object(m) => m, _ => unreachable!() });
    f(job)
}

fn job_get(k: &str) -> Value {
    with_job(|j| j.get(k).cloned().unwrap_or(Value::Null))
}

fn job_put(k: &str, v: Value) {
    with_job(|j| { j.insert(k.into(), v); });
}

fn phase() -> String {
    let p = s_or(Some(&job_get("phase")));
    if p.is_empty() { "idle".into() } else { p }
}

/// Update the job and keep a step history for the UI's checklist: a phase
/// change is a new step, a new hint for the SAME phase updates that step.
fn set(kw: Value) {
    let Value::Object(kw) = kw else { return };
    with_job(|job| {
        if kw.contains_key("started") {
            job.insert("steps".into(), json!([]));
            CANCEL.store(false, Ordering::SeqCst);
        }
        let ph = kw.get("phase").and_then(|v| v.as_str()).map(String::from);
        let hint = kw.get("hint").and_then(|v| v.as_str()).filter(|h| !h.is_empty()).map(String::from);
        if !job.get("steps").map(|s| s.is_array()).unwrap_or(false) {
            job.insert("steps".into(), json!([]));
        }
        let steps = job.get_mut("steps").and_then(|s| s.as_array_mut()).unwrap();
        if let Some(ph) = ph.as_deref().filter(|p| !p.is_empty()) {
            let same = steps.last().and_then(|s| s.get("phase")).and_then(|p| p.as_str()) == Some(ph);
            if !same {
                steps.push(json!({"phase": ph, "hint": hint.clone().unwrap_or_default(), "at": now_f()}));
                let n = steps.len();
                if n > 40 {
                    steps.drain(..n - 40);
                }
            } else if let Some(h) = &hint {
                steps.last_mut().unwrap()["hint"] = json!(h);
            }
        } else if let (Some(h), Some(last)) = (&hint, steps.last_mut()) {
            last["hint"] = json!(h);
        }
        if ph.as_deref().map(|p| TERMINAL.contains(&p)).unwrap_or(false) {
            CANCEL.store(false, Ordering::SeqCst);
        }
        for (k, v) in kw {
            job.insert(k, v);
        }
    });
}

struct Cancelled;

/// sleep for the job thread - the point where Stop takes effect
fn sleep(seconds: f64) -> Result<(), Cancelled> {
    let mine = *JOB_THREAD.lock().unwrap() == Some(std::thread::current().id());
    if mine && CANCEL.load(Ordering::SeqCst) {
        return Err(Cancelled);
    }
    std::thread::sleep(Duration::from_secs_f64(seconds));
    if mine && CANCEL.load(Ordering::SeqCst) {
        return Err(Cancelled);
    }
    Ok(())
}

pub fn cancel() -> Value {
    if TERMINAL.contains(&phase().as_str()) {
        return json!({"ok": false, "error": "nothing is running"});
    }
    CANCEL.store(true, Ordering::SeqCst);
    logs::info(format!("drive: stop requested during {}", phase()));
    json!({"ok": true})
}

fn game_running() -> bool {
    procs::game_running()
}

// --------------------------------------------------------------- the save --
fn mode_name(pick: &Map<String, Value>) -> String {
    let mut raw = { let g = s_or(get(pick, "game_mode")); if g.is_empty() { "PRACTICE".into() } else { g } }
        .trim().to_uppercase().replace("GAMEMODETYPE_", "");
    if !SP_MODES.contains(&raw.as_str()) {
        raw = "PRACTICE".into();
    }
    format!("GameModeType_{raw}")
}

fn mode_short(pick: &Map<String, Value>) -> String {
    mode_name(pick).replace("GameModeType_", "")
}

fn pint(pick: &Map<String, Value>, key: &str, lo: i64, hi: i64, fallback: i64) -> i64 {
    let n = match pick.get(key) {
        None | Some(Value::Null) => fallback,
        Some(v) => py_int(v).unwrap_or(fallback),
    };
    n.clamp(lo, hi)
}

fn aggro(pick: &Map<String, Value>) -> &'static str {
    let raw = { let a = s_or(get(pick, "aggressiveness")); if a.is_empty() { "Safe".into() } else { a } };
    AGGRO.iter().find(|n| raw.trim().eq_ignore_ascii_case(n)).copied().unwrap_or("Safe")
}

fn time_mult(pick: &Map<String, Value>) -> i64 {
    let want = match pick.get("time_mult") {
        None => 1.0,
        Some(v) if !truthy(Some(v)) => 1.0,
        Some(Value::Number(n)) => n.as_f64().unwrap_or(1.0),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(1.0),
        Some(Value::Bool(b)) => *b as i64 as f64,
        _ => 1.0,
    };
    *TIME_MULTIPLIERS.iter().min_by(|a, b| {
        let (da, db) = (((**a) as f64 - want).abs(), ((**b) as f64 - want).abs());
        da.partial_cmp(&db).unwrap().then(a.cmp(b))
    }).unwrap()
}

fn race_type(pick: &Map<String, Value>) -> &'static str {
    let r = { let r = s_or(get(pick, "race_type")); if r.is_empty() { "LAPS".into() } else { r } };
    if r.to_uppercase() == "TIME" { "TIME" } else { "LAPS" }
}

fn race_duration(pick: &Map<String, Value>) -> i64 {
    if race_type(pick) == "TIME" {
        pint(pick, "race_minutes", 1, 600, 20) * 60
    } else {
        pint(pick, "race_laps", 1, 200, 10)
    }
}

fn set_enum(msg: &mut DynamicMessage, field: &str, name: &str) -> Result<(), String> {
    let fd = msg.descriptor().get_field_by_name(field).ok_or_else(|| format!("no field {field}"))?;
    let en = fd.kind().as_enum().cloned().ok_or_else(|| format!("{field} is not an enum"))?;
    let v = en.get_value_by_name(name).ok_or_else(|| format!("unknown {field} value {name}"))?;
    msg.set_field(&fd, PV::EnumNumber(v.number()));
    Ok(())
}

fn str_field(msg: &DynamicMessage, f: &str) -> String {
    msg.get_field_by_name(f).and_then(|v| v.as_str().map(String::from)).unwrap_or_default()
}

fn i32_field(msg: &DynamicMessage, f: &str) -> i32 {
    msg.get_field_by_name(f).and_then(|v| v.as_i32()).unwrap_or(0)
}

fn set_i32(msg: &mut DynamicMessage, f: &str, v: i64) {
    msg.set_field_by_name(f, PV::I32(v as i32));
}

fn sessions_mut(obj: &mut DynamicMessage) -> &mut Vec<PV> {
    match obj.get_field_by_name_mut("sessions").unwrap() {
        PV::List(l) => l,
        _ => unreachable!(),
    }
}

fn new_session(obj: &DynamicMessage) -> PV {
    let fd = obj.descriptor().get_field_by_name("sessions").unwrap();
    PV::Message(DynamicMessage::new(fd.kind().as_message().unwrap().clone()))
}

fn sub_mut<'a>(m: &'a mut DynamicMessage, f: &str) -> &'a mut DynamicMessage {
    // ⚠ touching a singular message field sets its presence, like python's
    // `s.time_of_day.hour = ...`
    match m.get_field_by_name_mut(f).unwrap() {
        PV::Message(x) => x,
        _ => unreachable!(),
    }
}

fn set_race_duration(sess: &mut DynamicMessage, pick: &Map<String, Value>) {
    set_i32(sess, "duration", race_duration(pick));
    let _ = set_enum(sess, "duration_type", &format!("GameModeSelectionDuration_{}", race_type(pick)));
}

/// num_opponents / skill live on session 0; a race weekend keeps practice /
/// quali / warmup / race as sessions 0-3.
fn apply_mode_fields(obj: &mut DynamicMessage, pick: &Map<String, Value>, mode: &str) {
    let short = mode.replace("GameModeType_", "");
    if short == "RACE_WEEKEND" {
        let want = [
            ("Race Weekend", "TIME", pint(pick, "practice_min", 1, 240, 10) * 60),
            ("Qualifying", "TIME", pint(pick, "quali_min", 1, 120, 15) * 60),
            ("Warmup", "TIME", pint(pick, "warmup_min", 0, 60, 10) * 60),
            ("Race", race_type(pick), race_duration(pick)),
        ];
        while sessions_mut(obj).len() < 4 {
            let s = new_session(obj);
            sessions_mut(obj).push(s);
        }
        for (i, (name, dur_t, dur)) in want.iter().enumerate() {
            let PV::Message(s) = &mut sessions_mut(obj)[i] else { continue };
            if str_field(s, "name").is_empty() {
                s.set_field_by_name("name", PV::String(name.to_string()));
            }
            set_i32(s, "duration", *dur);
            let _ = set_enum(s, "duration_type", &format!("GameModeSelectionDuration_{dur_t}"));
            // ⚠ only session 0 carries the AI settings (the game's own
            // convention); stray values write skill 0, off the slider
            if i > 0 {
                set_i32(s, "num_opponents", 0);
                set_i32(s, "min_strength", 0);
                set_i32(s, "max_strength", 0);
                s.set_field_by_name("single_make", PV::Bool(false));
            }
        }
    } else if sessions_mut(obj).is_empty() {
        let s = new_session(obj);
        sessions_mut(obj).push(s);
    }
    let PV::Message(sess) = &mut sessions_mut(obj)[0] else { return };
    if ["PRACTICE", "HOTLAP", "HOTSTINT", "TEST_DRIVE"].contains(&short.as_str()) {
        set_i32(sess, "duration", pint(pick, "duration_min", 1, 600, 90) * 60);
        let _ = set_enum(sess, "duration_type", "GameModeSelectionDuration_TIME");
    } else if short == "INSTANT_RACE" {
        // the RACE TYPE toggle: laps, or SECONDS when timed
        set_race_duration(sess, pick);
        let pos = pint(pick, "starting_position", 0, 40, 0);
        if pos != 0 {
            set_i32(sess, "starting_position", pos);
        }
    }
    if AI_MODES.contains(&short.as_str()) {
        set_i32(sess, "num_opponents", pint(pick, "num_opponents", 1, MAX_OPPONENTS, 10));
        let mut lo = pint(pick, "skill_min", SKILL_MIN, SKILL_MAX, 80);
        let mut hi = pint(pick, "skill_max", SKILL_MIN, SKILL_MAX, 95);
        if hi < lo {
            std::mem::swap(&mut lo, &mut hi);
        }
        set_i32(sess, "min_strength", lo);
        set_i32(sess, "max_strength", hi);
        let single = match pick.get("single_make") { None => true, v => truthy(v) };
        sess.set_field_by_name("single_make", PV::Bool(single));
        let _ = set_enum(sess, "grid_type", "GameModeSelectionGridType_AUTO");
        let _ = set_enum(sess, "aggressivness", aggro(pick));
    }
}

fn event(pick: &Map<String, Value>) -> Option<Value> {
    let tracks = content::tracks().get("tracks").and_then(|t| t.as_array()).cloned().unwrap_or_default();
    let name = s_or(get(pick, "custom_track")).trim().to_string();
    if !name.is_empty() {
        if let Some(hit) = tracks.iter().find(|t| {
            let c = s_or(t.get("custom_track"));
            (if c.is_empty() { s_or(t.get("track")) } else { c }) == name
        }) {
            return Some(hit.clone());
        }
    }
    let idx = int0(get(pick, "track_index"));
    tracks.into_iter().find(|t| t.get("index").and_then(py_int) == Some(idx) && t.get("index").map(|v| v.is_number()).unwrap_or(false))
}

/// Set initial_grip on the weather preset this session uses (matched on
/// the enum SUFFIX: two grip enums exist).
fn set_grip(last: &mut DynamicMessage, wname: &str, grip: &str) -> bool {
    let want = { let g = grip.trim().to_uppercase(); if g.is_empty() { "OPTIMUM".into() } else { g } };
    let PV::List(list) = last.get_field_by_name_mut("weather_data").unwrap() else { return false };
    let Some(PV::Message(first)) = list.first() else { return false };
    let d = first.descriptor();
    let Some(gf) = d.get_field_by_name("initial_grip") else { return false };
    let Some(genum) = gf.kind().as_enum().cloned() else { return false };
    let Some(val) = genum.values().find(|v| v.name().rsplit('_').next().unwrap_or("").to_uppercase() == want) else {
        let names: Vec<String> = genum.values().map(|v| v.name().to_string()).collect();
        logs::warn(format!("drive unknown grip {grip} (have {})", names.join(", ")));
        return false;
    };
    let target = d.get_field_by_name("type").and_then(|t| t.kind().as_enum().cloned()).and_then(|e| e.get_value_by_name(wname)).map(|v| v.number());
    let mut hit = false;
    let any = !list.is_empty();
    for w in list.iter_mut() {
        let PV::Message(w) = w else { continue };
        if let Some(t) = target {
            if w.get_field_by_name("type").and_then(|v| v.as_enum_number()) != Some(t) {
                continue;
            }
        }
        w.set_field_by_name("initial_grip", PV::EnumNumber(val.number()));
        hit = true;
    }
    if !hit && any {
        logs::warn(format!("drive: no weather_data entry for {wname}; grip not applied"));
    }
    hit
}

fn car_model(preset_id: &str) -> String {
    if preset_id.is_empty() {
        return String::new();
    }
    let cars = content::cars().get("cars").and_then(|c| c.as_array()).cloned().unwrap_or_default();
    if let Some(m) = cars.iter().find(|c| c.get("id").and_then(|v| v.as_str()) == Some(preset_id)).map(|c| s_or(c.get("model"))).filter(|m| !m.is_empty()) {
        return m;
    }
    let t = carsmap::table(false);
    let presets = t.get("presets");
    let m = presets.and_then(|p| p.get(preset_id)).map(|v| s_or(Some(v))).filter(|s| !s.is_empty())
        .or_else(|| presets.and_then(|p| p.get(preset_id.to_lowercase())).map(|v| s_or(Some(v))).filter(|s| !s.is_empty()));
    m.unwrap_or_else(|| preset_id.to_string())
}

fn backup(path: &Path) {
    if !path.is_file() {
        return;
    }
    let bak = PathBuf::from(format!("{}.bak_acecm", path.display()));
    if !bak.is_file() {
        let _ = std::fs::copy(path, &bak);
    }
    let hist = format!("{}.{}.bak", path.display(), chrono::Local::now().format("%Y%m%d-%H%M%S"));
    let _ = std::fs::copy(path, hist);
}

fn write_synced(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::File::create(path)?;
    f.write_all(bytes)?;
    f.flush()?;
    f.sync_all()
}

/// Stamp GameModeSave + lastgamemode under `root` so startup_gamemode has
/// something to load. (`root` is the game's settings folder.)
pub fn write_gamemode_into(root: &Path, pick: &Map<String, Value>, check_game: bool) -> Value {
    if check_game && game_running() {
        return json!({"ok": false, "error": "close the game first — it reads these saves at boot and overwrites them on exit"});
    }
    let Some(ev) = event(pick) else {
        return json!({"ok": false, "error": "that track index is not in the list"});
    };
    let mode = mode_name(pick);
    let gmdir = root.join("GameModes");
    let _ = std::fs::create_dir_all(&gmdir);
    let save_path = gmdir.join(format!("{mode}.gamemodesave"));
    let last_path = gmdir.join("gamemode.lastgamemode");
    let Some(desc) = protos::find("GameModeSave") else {
        return json!({"ok": false, "error": "GameModeSave schema is missing"});
    };
    let mut obj = match std::fs::read(&save_path) {
        Ok(b) => match DynamicMessage::decode(desc.clone(), b.as_slice()) {
            Ok(m) => m,
            Err(e) => return json!({"ok": false, "error": format!("DecodeError: {e}")}),
        },
        Err(_) => DynamicMessage::new(desc),
    };
    if let Err(e) = set_enum(&mut obj, "type", &mode) {
        return json!({"ok": false, "error": e});
    }
    apply_mode_fields(&mut obj, pick, &mode);
    let hour = pint(pick, "tod_hour", 0, 23, 13);
    // the start-time slider steps in 5 minutes
    let minute = (pint(pick, "tod_minute", 0, 59, 0) / START_TIME_STEP_MIN * START_TIME_STEP_MIN).min(55);
    let tm = time_mult(pick) as f32;
    {
        let PV::Message(sess) = &mut sessions_mut(&mut obj)[0] else { unreachable!() };
        if str_field(sess, "name").is_empty() {
            let g = { let g = s_or(get(pick, "game_mode")); if g.is_empty() { "PRACTICE".into() } else { g } };
            sess.set_field_by_name("name", PV::String(py_title(&g.replace('_', " "))));
        }
    }
    for s in sessions_mut(&mut obj).iter_mut() {
        let PV::Message(s) = s else { continue };
        {
            let tod = sub_mut(s, "time_of_day");
            if i32_field(tod, "year") == 0 {
                set_i32(tod, "year", 2014);
                set_i32(tod, "month", 8);
                set_i32(tod, "day", 15);
            }
            set_i32(tod, "hour", hour);
            set_i32(tod, "minute", minute);
            // the TIME MULTIPLIER slider
            tod.set_field_by_name("time_multiplier", PV::F32(tm));
        }
        let ti = sub_mut(s, "track_item");
        let pick_s = |k: &str| s_or(ev.get(k));
        let name = { let n = pick_s("track"); if n.is_empty() { str_field(ti, "name") } else { n } };
        ti.set_field_by_name("name", PV::String(name));
        let layout = { let n = pick_s("layout"); if n.is_empty() { str_field(ti, "layout") } else { n } };
        ti.set_field_by_name("layout", PV::String(layout));
        let evn = { let n = pick_s("name"); let n = if n.is_empty() { pick_s("event_name") } else { n }; if n.is_empty() { str_field(ti, "event_name") } else { n } };
        ti.set_field_by_name("event_name", PV::String(evn));
        if truthy(ev.get("length_m")) {
            set_i32(ti, "track_length", int0(ev.get("length_m")));
        }
        ti.set_field_by_name("nation", PV::String(String::new()));
        ti.set_field_by_name("continent", PV::String(String::new()));
        ti.set_field_by_name("is_enabled", PV::Bool(true));
    }
    let car = s_or(get(pick, "car"));
    let model = car_model(&car);
    if !model.is_empty() && !AI_MODES.contains(&mode_short(pick).as_str()) {
        let cars = content::cars().get("cars").and_then(|c| c.as_array()).cloned().unwrap_or_default();
        let label = cars.iter().find(|c| c.get("id").and_then(|v| v.as_str()) == Some(car.as_str()))
            .map(|c| s_or(c.get("label"))).filter(|l| !l.is_empty()).unwrap_or_else(|| model.clone());
        let PV::Message(sess) = &mut sessions_mut(&mut obj)[0] else { unreachable!() };
        let mfd = sess.descriptor().get_field_by_name("models").unwrap();
        let mut m = DynamicMessage::new(mfd.kind().as_message().unwrap().clone());
        m.set_field_by_name("name", PV::String(model.clone()));
        m.set_field_by_name("display_name", PV::String(label));
        m.set_field_by_name("is_enabled", PV::Bool(true));
        sess.set_field(&mfd, PV::List(vec![PV::Message(m)]));
    }
    backup(&save_path);
    // ⚠ explicit flush + fsync: the game we launch next reads it at once
    use prost::Message;
    if let Err(e) = write_synced(&save_path, &obj.encode_to_vec()) {
        return json!({"ok": false, "error": format!("{e}")});
    }
    let weather = { let w = s_or(get(pick, "weather")); if w.is_empty() { "CLEAR".into() } else { w } };
    let wname = format!("GameModeSelectionWeatherType_{}", weather.trim().to_uppercase());
    if let (Some(ld), true) = (protos::find("LastGameMode"), last_path.is_file()) {
        if let Ok(b) = std::fs::read(&last_path) {
            if let Ok(mut last) = DynamicMessage::decode(ld, b.as_slice()) {
                let _ = set_enum(&mut last, "type", &mode);
                // weather_type is the selected preset; weather_data[] is the library
                if set_enum(&mut last, "weather_type", &wname).is_err() {
                    logs::warn(format!("drive unknown weather {wname}"));
                }
                if let PV::List(list) = last.get_field_by_name_mut("weather_data").unwrap() {
                    if let Some(PV::Message(first)) = list.first_mut() {
                        let clear = first.descriptor().get_field_by_name("type").and_then(|t| t.kind().as_enum().cloned())
                            .and_then(|e| e.get_value_by_name("GameModeSelectionWeatherType_CLEAR")).map(|v| v.number());
                        if let Some(c) = clear {
                            if first.get_field_by_name("type").and_then(|v| v.as_enum_number()) != Some(c) {
                                first.set_field_by_name("type", PV::EnumNumber(c));
                            }
                        }
                    }
                }
                // grip is PER WEATHER PRESET: only the one this session uses
                let grip = { let g = s_or(get(pick, "grip")); if g.is_empty() { "OPTIMUM".into() } else { g } };
                set_grip(&mut last, &wname, &grip);
                backup(&last_path);
                let _ = write_synced(&last_path, &last.encode_to_vec());
            }
        }
    }
    let PV::Message(sess) = &sessions_mut(&mut obj)[0] else { unreachable!() };
    let sess = sess.clone();
    let ti = match sess.get_field_by_name("track_item").map(|c| c.into_owned()) { Some(PV::Message(m)) => m, _ => unreachable!() };
    let ai = AI_MODES.contains(&mode_short(pick).as_str());
    logs::info(format!("drive wrote {mode} track={}/{} car={model} weather={wname} tod={hour:02}:{minute:02} ai={} skill={}-{}",
        str_field(&ti, "name"), str_field(&ti, "layout"),
        if ai { i32_field(&sess, "num_opponents") } else { 0 },
        i32_field(&sess, "min_strength"), i32_field(&sess, "max_strength")));
    json!({"ok": true, "mode": mode, "save": save_path.to_string_lossy(),
           "track": str_field(&ti, "name"), "layout": str_field(&ti, "layout"), "car_model": model,
           "weather": wname, "tod_hour": hour, "tod_minute": minute,
           "num_opponents": i32_field(&sess, "num_opponents")})
}

/// str.title()
fn py_title(s: &str) -> String {
    let mut out = String::new();
    let mut prev_alpha = false;
    for c in s.chars() {
        if c.is_alphabetic() {
            if prev_alpha { out.extend(c.to_lowercase()) } else { out.extend(c.to_uppercase()) }
            prev_alpha = true;
        } else {
            out.push(c);
            prev_alpha = false;
        }
    }
    out
}

fn write_gamemode(pick: &Map<String, Value>) -> Value {
    write_gamemode_into(Path::new(&gamesettings::settings_dir()), pick, true)
}

// ------------------------------------------------------------ the proxy --
fn norm(p: &str) -> String {
    let a = std::path::absolute(p).unwrap_or_else(|_| PathBuf::from(p));
    let s = a.to_string_lossy().replace('/', "\\");
    if cfg!(windows) { s.to_lowercase() } else { s }
}

/// The client is rewritten to localhost:448; without the proxy the garage
/// cannot list/set cars. Replaces a proxy serving another data folder.
pub fn ensure_backend() -> Value {
    let st = backend::state();
    if truthy(st.get("listening")) {
        let theirs = s_or(backend::join_state().get("lobby"));
        let mine = lobby::path().to_string_lossy().into_owned();
        if !theirs.is_empty() && norm(&theirs) == norm(&mine) {
            return json!({"ok": true, "already": true});
        }
        logs::info(format!("backend on the port serves {}, not {mine:?}; replacing it",
            if theirs.is_empty() { "'(unknown)'".to_string() } else { format!("{theirs:?}") }));
        backend::stop();
        let port = config::load().get("backend_port").and_then(|v| v.as_i64()).unwrap_or(448);
        for p in [port as u16, 8093] {
            for pid in procs::tcp_listen_pids(p) {
                procs::kill(pid);
            }
        }
        let _ = sleep(1.0);
    }
    backend::start("proxy")
}

fn stop_leftover_server() {
    if let Some(prof) = profiles::load().into_iter().find(|p| p.get("id").and_then(|v| v.as_str()) == Some(DRIVE_ID)) {
        if truthy(serverproc::status(&prof).get("running")) {
            serverproc::stop(DRIVE_ID);
            logs::info("drive stopped leftover Quick Drive dedicated server");
        }
    }
}

fn car_ids_of(sv: &Value) -> Vec<String> {
    let mut raw = Vec::new();
    let list = |k: &str| sv.get(k).and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let mut all = list("allowed_cars_list");
    let full = list("allowed_cars_list_full");
    all.extend(if !full.is_empty() { full } else { list("cars") });
    for x in all {
        if x.is_object() {
            let a = s_or(x.get("car_name"));
            let a = if a.is_empty() { s_or(x.get("name")) } else { a };
            raw.push(if a.is_empty() { s_or(x.get("id")) } else { a });
        } else {
            raw.push(s_or(Some(&x)));
        }
    }
    raw.into_iter().filter(|c| !c.is_empty()).collect()
}

fn car_allowed(sv: &Value, car_id: &str) -> bool {
    let allow = car_ids_of(sv);
    if allow.is_empty() || allow.iter().any(|a| a == car_id) {
        return true;
    }
    let blob = format!("{car_id} {}", car_model(car_id));
    allow.iter().any(|a| blob.contains(a.as_str()))
}

fn find_local(pick: &Map<String, Value>) -> Option<Value> {
    let lid = { let l = s_or(get(pick, "local_id")); if l.is_empty() { s_or(get(pick, "server_id")) } else { l } }.trim().to_string();
    if lid.is_empty() {
        return None;
    }
    profiles::load().into_iter().find(|p| p.get("id").and_then(|v| v.as_str()) == Some(lid.as_str()))
}

/// Make this profile list in the in-game browser with truthful ads.
pub fn publish_local(profile_id: &str) -> Value {
    let Some(mut prof) = profiles::load().into_iter().find(|p| p.get("id").and_then(|v| v.as_str()) == Some(profile_id)) else {
        return json!({"ok": false, "error": "no such server profile"});
    };
    let mut changed = false;
    if truthy(prof.get("no_lobby")) {
        prof["no_lobby"] = json!(false);
        serverproc::upsert(&prof);
        changed = true;
    }
    let ad = lobby::write(&prof);
    let be = ensure_backend();
    let st = backend::state();
    json!({
        "ok": truthy(st.get("listening")) || truthy(be.get("ok")),
        "listed": true,
        "cleared_private": changed,
        "ad": {"name": ad.get("server_name"), "track": ad.get("track"), "layout": ad.get("layout"),
               "tcp_port": ad.get("tcp_port"), "cars": ad.get("cars").filter(|c| truthy(Some(c))).cloned().unwrap_or(json!([])),
               "lan_ip": ad.get("lan_ip")},
        "backend": truthy(st.get("listening")),
        "hint": "Open Multiplayer in-game — the row is tagged [ACECM]. Friends on the LAN see the same name at your LAN IP.",
    })
}

fn port_open(port: i64) -> bool {
    if !(1..=65535).contains(&port) {
        return false;
    }
    std::net::TcpStream::connect_timeout(&std::net::SocketAddr::from(([127, 0, 0, 1], port as u16)), Duration::from_millis(400)).is_ok()
}

fn tcp_of(prof: &Value) -> i64 {
    let t = int0(prof.get("tcp_port"));
    if t == 0 { 9700 } else { t }
}

/// HTTP answering is not enough - join uses the TCP port.
fn wait_server(prof: &Value, seconds: f64) -> Result<Value, Cancelled> {
    let tcp = tcp_of(prof);
    let until = Instant::now() + Duration::from_secs_f64(seconds);
    let mut last = json!({});
    while Instant::now() < until {
        last = serverproc::status(prof);
        last["tcp_open"] = json!(port_open(tcp));
        if truthy(last.get("tcp_open")) {
            return Ok(last);
        }
        sleep(0.8)?;
    }
    last["tcp_open"] = json!(port_open(tcp));
    Ok(last)
}

fn clients(prof: &Value) -> i64 {
    int0(serverproc::status(prof).get("clients"))
}

fn launch_summary(launched: &Value) -> Value {
    let mut m = Map::new();
    for k in ["ok", "error", "via", "backend", "inspector_patched", "needs_admin"] {
        if let Some(v) = launched.get(k) {
            m.insert(k.into(), v.clone());
        }
    }
    Value::Object(m)
}

fn pick_keys(v: &Value, keys: &[&str]) -> Value {
    let mut m = Map::new();
    for k in keys {
        if let Some(x) = v.get(*k) {
            m.insert((*k).into(), x.clone());
        }
    }
    Value::Object(m)
}

fn err_of(v: &Value, fallback: &str) -> String {
    let e = s_or(v.get("error"));
    if e.is_empty() { fallback.to_string() } else { e }
}

/// launch with -no_intro; Err(fault) when it did not take
fn launch_for_join() -> Result<(), String> {
    set(json!({"phase": "launching_game", "hint": "launching the game"}));
    let launched = launch::launch_game(&["-no_intro".to_string()]);
    job_put("launch", launch_summary(&launched));
    if !truthy(launched.get("ok")) {
        return Err(err_of(&launched, "could not launch"));
    }
    Ok(())
}

fn run_local(pick: &Map<String, Value>) -> Result<(), Cancelled> {
    let Some(mut prof) = find_local(pick) else {
        set(json!({"phase": "failed", "fault": "pick one of your ACECM servers first"}));
        return Ok(());
    };
    let car = s_or(get(pick, "car"));
    if !car_allowed(&json!({"cars": servers::allowed_car_ids(&prof)}), &car) {
        set(json!({"phase": "failed", "fault": "that car is not on this server's allowed list"}));
        return Ok(());
    }
    if truthy(prof.get("no_lobby")) {
        prof["no_lobby"] = json!(false);
        serverproc::upsert(&prof);
    }
    set(json!({"phase": "starting_backend", "hint": "starting the lobby so it lists"}));
    ensure_backend();
    lobby::write(&prof);
    let tcp = tcp_of(&prof);
    let pname = { let n = s_or(prof.get("name")); n };
    let mut st = serverproc::status(&prof);
    if truthy(st.get("running")) && !truthy(wait_server(&prof, 6.0)?.get("tcp_open")) {
        // ⚠ stale "running": the record outlived the socket - restart it
        logs::info("drive: server marked running but TCP closed; restarting");
        serverproc::stop(&s_or(prof.get("id")));
        sleep(1.5)?;
        st = serverproc::status(&prof);
    }
    if !truthy(st.get("running")) {
        set(json!({"phase": "starting_server", "hint": format!("starting {}", if pname.is_empty() { "the server" } else { &pname })}));
        let started = serverproc::start(&prof);
        if !truthy(started.get("ok")) {
            set(json!({"phase": "failed", "fault": err_of(&started, "could not start the server")}));
            return Ok(());
        }
        set(json!({"phase": "starting_server", "hint": "waiting for the dedicated server"}));
        st = wait_server(&prof, 50.0)?;
        if !truthy(st.get("tcp_open")) {
            set(json!({"phase": "failed", "fault": format!("the dedicated server process started but never opened TCP {tcp} — Join would fail with 'socket did not respond'. Stop the server and start it again from Servers; check Logs if it dies on season/track load")}));
            return Ok(());
        }
    } else {
        st = wait_server(&prof, 12.0)?;
        if !truthy(st.get("tcp_open")) {
            set(json!({"phase": "failed", "fault": format!("ACECM thinks that server is running, but nothing is listening on 127.0.0.1:{tcp} — Stop it and Start & Join again")}));
            return Ok(());
        }
    }
    let _ = st;
    lobby::write(&prof);
    // ⚠ name the server we are advertising, or refresh() corrects the blob
    // from whichever server is running
    lobby::refresh(int0(prof.get("tcp_port")));
    let sv = json!({
        "server_id": s_or(prof.get("id")),
        "server_name": ({ let n = s_or(prof.get("name")); if n.is_empty() { "ACECM".to_string() } else { n } }),
        "server_ip": "127.0.0.1",
        "server_tcp_port": tcp,
        "server_udp_port": tcp,
        "cars": servers::allowed_car_ids(&prof),
    });
    if !game_running() {
        if let Err(f) = launch_for_join() {
            set(json!({"phase": "failed", "fault": f}));
            return Ok(());
        }
    }
    // ⚠ never join a local server with the public pick's password/id
    let mut local_pick = pick.clone();
    local_pick.insert("password".into(), json!(s_or(prof.get("driver_password"))));
    local_pick.insert("server_id".into(), sv["server_id"].clone());
    let before = clients(&prof);
    let p2 = prof.clone();
    let joined = move || clients(&p2) > before;
    let poked = enter_and_join(&local_pick, &sv, Some(&joined))?;
    job_put("join", pick_keys(&poked, &["ok", "error", "note", "connected"]));
    if !truthy(poked.get("ok")) {
        set(json!({"phase": "failed", "fault": err_of(&poked, "could not join the local server")}));
        return Ok(());
    }
    let hint = if truthy(poked.get("connected")) { "connected — the server sees you, pit menu next".to_string() }
        else { let n = s_or(poked.get("note")); if n.is_empty() { "joining your server — pit menu next".into() } else { n } };
    set(json!({"phase": "launched", "hint": hint}));
    Ok(())
}

fn find_public(pick: &Map<String, Value>) -> Option<Value> {
    let sid = s_or(get(pick, "server_id")).trim().to_string();
    let ip = s_or(get(pick, "server_ip")).trim().to_string();
    let tcp = int0(get(pick, "server_tcp_port"));
    let list = backend::server_list();
    for s in list.get("servers").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
        if !sid.is_empty() && s_or(s.get("server_id")) == sid {
            return Some(s);
        }
        if !ip.is_empty() && int0(s.get("server_tcp_port")) == tcp && s_or(s.get("server_ip")) == ip {
            return Some(s);
        }
    }
    if !ip.is_empty() && tcp != 0 {
        let udp = get(pick, "server_udp_port").filter(|v| truthy(Some(v))).cloned().unwrap_or(json!(tcp));
        let cars = get(pick, "allowed_cars").filter(|v| truthy(Some(v))).cloned().unwrap_or(json!([]));
        return Some(json!({"server_name": ip, "server_ip": ip, "server_tcp_port": tcp, "server_udp_port": udp, "cars": cars}));
    }
    None
}

/// Wait until boot_state is this page (sp / mp / home). No extra goTo.
fn wait_page(want: &str, seconds: f64) -> Result<String, Cancelled> {
    let until = Instant::now() + Duration::from_secs_f64(seconds);
    let mut last = String::new();
    while Instant::now() < until {
        if !game_running() {
            return Ok(String::new());
        }
        let boot = gameui::boot_state();
        if !boot.is_empty() && boot != last {
            logs::info(format!("drive boot {boot}"));
            last = boot.clone();
        }
        if gameui::boot_page(&boot) == want && !gameui::paintshop_up(&boot) {
            return Ok(boot);
        }
        if gameui::session_loading(&boot) || gameui::in_pits(Some(&boot)) {
            return Ok(boot);
        }
        sleep(0.3)?;
    }
    Ok(last)
}

fn val_str(r: &Value) -> String {
    s_or(r.get("value"))
}

fn py_repr(v: &Value) -> String {
    // close enough to python's repr of a js_value dict for the log
    v.to_string()
}

/// Home screen, set an allowed car, then Connect once on the list.
fn enter_and_join(pick: &Map<String, Value>, sv: &Value, joined: Option<&dyn Fn() -> bool>) -> Result<Value, Cancelled> {
    let deadline = Instant::now() + Duration::from_secs(90);
    set(json!({"phase": "waiting_for_menu", "hint": "waiting for the menu"}));
    let state = wait_ready(deadline)?;
    match state.as_str() {
        "gone" => return Ok(json!({"ok": false, "error": "the game closed before join"})),
        "live" => return Ok(json!({"ok": true, "started": "already"})),
        "ready" => {}
        _ => return Ok(json!({"ok": false, "error": menu_not_ready_error("join")})),
    }
    let car = s_or(get(pick, "car"));
    let model = car_model(&car);
    let have = gameui::current_car_name();
    if !model.is_empty() && have != model {
        set(json!({"phase": "selecting_car", "hint": format!("setting {model}")}));
        match gameui::select_car(&model, &car, "", "") {
            Ok(chosen) => logs::info(format!("drive set car: {}", py_repr(&chosen))),
            Err(e) => return Ok(json!({"ok": false, "error": format!("could not set car: {e}")})),
        }
    }
    let host = s_or(sv.get("server_ip")).trim().to_string();
    let tcp = int0(sv.get("server_tcp_port"));
    // the proxy trims the game's list; pin the server we are joining
    let _ = std::fs::write(paths::data().join("join_pin.json"), json!({"ip": host, "tcp": tcp, "at": now_f()}).to_string());
    let pw = s_or(get(pick, "password"));
    let sid = { let s = s_or(sv.get("server_id")); if s.is_empty() { s_or(get(pick, "server_id")) } else { s } };
    if host.is_empty() || tcp == 0 {
        return Ok(json!({"ok": false, "error": "that server has no address"}));
    }
    ensure_backend();
    let sname = { let n = s_or(sv.get("server_name")); if n.is_empty() { host.clone() } else { n } };
    set(json!({"phase": "joining", "hint": format!("opening Multiplayer for {sname}")}));
    gameui::focus_game();
    // ⚠ only deep-link when the address is in the list the GAME sees: a
    // miss puts a modal on screen. Our own server is injected after the
    // snapshot, so a local join counts as listed.
    let mut deep = host == "127.0.0.1" || host == "localhost" || truthy(sv.get("local"));
    if !deep {
        for s in backend::server_list().get("servers").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
            if s_or(s.get("server_ip")) == host && int0(s.get("server_tcp_port")) == tcp {
                deep = true;
                break;
            }
        }
    }
    match gameui::enter_multiplayer(&host, tcp as u32, &pw, deep) {
        Ok(went) => logs::info(format!("drive ui multiplayer: {} (deep={})", py_repr(&went), if deep { "True" } else { "False" })),
        Err(e) => logs::warn(format!("drive multiplayer goto lost: {e}")),
    }
    // ⚠ ask again until the page is actually open: a busy UI drops gotos
    let mut on = wait_page("mp", 20.0)?;
    if gameui::boot_page(&on) != "mp" && !gameui::session_loading(&on) {
        let retry_until = Instant::now() + Duration::from_secs(40);
        let mut opened = false;
        while Instant::now() < retry_until {
            if !game_running() {
                return Ok(json!({"ok": false, "error": "the game closed before joining"}));
            }
            if gameui::server_list_page() {
                opened = true;
                break;
            }
            set(json!({"phase": "joining", "hint": "waiting for Multiplayer to open"}));
            match gameui::enter_multiplayer(&host, tcp as u32, &pw, deep) {
                Ok(r) => logs::info(format!("drive ui multiplayer retry: {}", py_repr(&r))),
                Err(e) => logs::warn(format!("drive multiplayer retry lost: {e}")),
            }
            sleep(2.0)?;
        }
        if !opened {
            on = wait_page("mp", 5.0)?;
            if gameui::boot_page(&on) != "mp" && !gameui::session_loading(&on) {
                return Ok(json!({"ok": false, "error": format!("Multiplayer never opened - the game stopped responding to ACECM (last {})", if on.is_empty() { "-" } else { &on })}));
            }
        }
    }
    // let the page fetch the list once; overlapping ServerList replies while
    // connecting crash the physics thread
    sleep(2.5)?;
    let mut picked = Value::Null;
    let mut sent = false;
    let mut first_ineligible: Option<Instant> = None;
    let until = Instant::now() + Duration::from_secs(50);
    while Instant::now() < until {
        if !game_running() {
            return Ok(json!({"ok": false, "error": "the game closed before join"}));
        }
        match gameui::join_public(&host, tcp as u32, &pw, &sid) {
            Ok(p) => picked = p,
            Err(e) => {
                logs::warn(format!("drive join_public lost: {e}"));
                sleep(0.8)?;
                continue;
            }
        }
        logs::info(format!("drive ui pick: {}", py_repr(&picked)));
        let val = val_str(&picked);
        if val.starts_with("connect:") || val == "already-sent" {
            sent = true;
            break;
        }
        if val.starts_with("not-in-list:") {
            set(json!({"phase": "joining", "hint": "list is up — that server is not on this page yet"}));
        } else if matches!(val.as_str(), "waiting-list" | "loading" | "no-page") {
            set(json!({"phase": "joining", "hint": "waiting for the in-game server list"}));
        } else if val == "no-car" {
            set(json!({"phase": "joining", "hint": "waiting for the current car"}));
        } else if let Some(rest) = val.strip_prefix("car-not-eligible:") {
            // ⚠ partly timing: #btnJoin stays disabled until the server's
            // details arrive. Give it 10 s, then believe it.
            let t0 = *first_ineligible.get_or_insert_with(Instant::now);
            if t0.elapsed() < Duration::from_secs(10) {
                set(json!({"phase": "joining", "hint": "waiting for the server's car list"}));
                sleep(1.0)?;
                continue;
            }
            let car = if rest.is_empty() { "your current car" } else { rest };
            gameui::focus_game();
            return Ok(json!({"ok": false, "error": format!("this server does not allow {car} - pick a car it permits and try again")}));
        } else if val.starts_with("select-mismatch:") {
            set(json!({"phase": "joining", "hint": "the list selected a different server — retrying"}));
            logs::info(format!("drive join: {val}"));
        }
        sleep(1.0)?;
    }
    if !sent {
        gameui::focus_game();
        let v = picked.get("value").map(|v| if v.is_string() { v.as_str().unwrap().to_string() } else if v.is_null() { "None".into() } else { v.to_string() }).unwrap_or_else(|| "None".into());
        return Ok(json!({"ok": false, "error": format!("the in-game list never selected that server ({v})")}));
    }
    set(json!({"phase": "joining", "hint": "Connect sent — waiting for the session"}));
    gameui::focus_game();
    // no more inspector traffic: a second Connect while physics loads
    // access-violates the physics thread. For our own server `joined` asks
    // the server itself (its client count) - a real answer.
    let until = Instant::now() + Duration::from_secs(if joined.is_some() { 90 } else { 40 });
    while Instant::now() < until {
        if !game_running() {
            return Ok(json!({"ok": false, "error": "the game closed while joining (likely a crash — try Join again)"}));
        }
        if let Some(j) = joined {
            if j() {
                return Ok(json!({"ok": true, "join": picked, "connected": true}));
            }
        }
        let txt = this_boot_log();
        if (txt.contains("Established connection to server") && !txt.contains(":0")) || txt.contains("Game Started!") {
            return Ok(json!({"ok": true, "join": picked, "connected": true}));
        }
        sleep(0.8)?;
    }
    Ok(json!({"ok": true, "join": picked,
              "note": "Join was pressed once — if you are still on the list, the server refused (full, wrong car, password)"}))
}

fn run_server(pick: &Map<String, Value>) -> Result<(), Cancelled> {
    let Some(sv) = find_public(pick) else {
        set(json!({"phase": "failed", "fault": "that public server is not in the captured list — open Multiplayer in-game once to refresh it"}));
        return Ok(());
    };
    if !car_allowed(&sv, &s_or(get(pick, "car"))) {
        set(json!({"phase": "failed", "fault": "that car is not on this server's allowed list"}));
        return Ok(());
    }
    ensure_backend();
    if !game_running() {
        if let Err(f) = launch_for_join() {
            set(json!({"phase": "failed", "fault": f}));
            return Ok(());
        }
    }
    let poked = enter_and_join(pick, &sv, None)?;
    job_put("join", pick_keys(&poked, &["ok", "error", "note", "connected"]));
    if !truthy(poked.get("ok")) {
        set(json!({"phase": "failed", "fault": err_of(&poked, "could not join the server")}));
        return Ok(());
    }
    let n = s_or(poked.get("note"));
    set(json!({"phase": "launched", "hint": if n.is_empty() { "joining the server — pit menu next".to_string() } else { n }}));
    Ok(())
}

fn run_sp(pick: &Map<String, Value>) -> Result<(), Cancelled> {
    stop_leftover_server();
    if game_running() {
        ensure_backend();
        let poked = enter_and_start(pick)?;
        if !truthy(poked.get("ok")) {
            if ui_failed(&poked, "join") {
                return Ok(());
            }
            set(json!({"phase": "failed", "fault": err_of(&poked, "could not press Start")}));
            return Ok(());
        }
        set(json!({"phase": "launched", "hint": "in the pit menu — change setup, then Drive in-game"}));
        return Ok(());
    }
    set(json!({"phase": "writing", "hint": "writing the single-player session", "fault": "", "join": null, "wrote": null}));
    let wrote = write_gamemode(pick);
    let mut w = Map::new();
    for k in ["ok", "mode", "track", "layout", "car_model", "error"] {
        w.insert(k.into(), wrote.get(k).cloned().unwrap_or(Value::Null));
    }
    job_put("wrote", Value::Object(w));
    if !truthy(wrote.get("ok")) {
        set(json!({"phase": "failed", "fault": err_of(&wrote, "could not write save")}));
        return Ok(());
    }
    let mode = s_of(wrote.get("mode"));
    let mut extra = vec!["-no_intro".to_string(), format!("-startup_gamemode={mode}"), format!("--startup_gamemode={mode}")];
    let cm = s_or(wrote.get("car_model"));
    if !cm.is_empty() {
        extra.push(format!("-load_single_car={cm}"));
        extra.push(format!("--load_single_car={cm}"));
    }
    std::thread::spawn(|| { ensure_backend(); });
    set(json!({"phase": "launching_game", "hint": format!("launching {mode} at {}", s_of(wrote.get("track")))}));
    job_put("t_launch", json!(now_f()));
    let launched = launch::launch_game(&extra);
    job_put("launch", launch_summary(&launched));
    if !truthy(launched.get("ok")) {
        set(json!({"phase": "failed", "fault": err_of(&launched, "could not launch")}));
        return Ok(());
    }
    let poked = enter_and_start(pick)?;
    if !truthy(poked.get("ok")) {
        if ui_failed(&poked, "single player") {
            return Ok(());
        }
        set(json!({"phase": "failed", "fault": err_of(&poked, "could not press Start in the UI")}));
        return Ok(());
    }
    set(json!({"phase": "launched", "hint": "in the pit menu — change setup, then Drive in-game"}));
    Ok(())
}

fn run(pick: Map<String, Value>) {
    *JOB_THREAD.lock().unwrap() = Some(std::thread::current().id());
    let via = s_or(get(&pick, "via"));
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match via.as_str() {
        "server" => run_server(&pick),
        "local" => run_local(&pick),
        _ => run_sp(&pick),
    }));
    match r {
        Ok(Ok(())) => {}
        Ok(Err(Cancelled)) => set(json!({"phase": "cancelled", "hint": "stopped", "fault": ""})),
        Err(p) => {
            let msg = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_else(|| "panic".into());
            logs::warn(format!("drive: {msg}"));
            set(json!({"phase": "failed", "fault": format!("RuntimeError: {msg}")}));
        }
    }
}

// --------------------------------------------------------- the game log --
/// Newest log for THIS Drive. Steam may write a tiny stub then a real one.
fn this_boot_log() -> String {
    let since = int0(Some(&job_get("started")));
    let folder = Path::new(&gamesettings::settings_dir()).join("Logs");
    let Ok(rd) = std::fs::read_dir(&folder) else { return String::new() };
    let mut cands: Vec<(u64, PathBuf)> = Vec::new();
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().to_lowercase();
        if !(n.starts_with("log-") && n.ends_with(".txt")) {
            continue;
        }
        let Ok(md) = e.metadata() else { continue };
        let mt = md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs_f64()).unwrap_or(0.0);
        if since != 0 && mt < (since - 5) as f64 {
            continue;
        }
        cands.push((md.len(), e.path()));
    }
    cands.sort_by(|a, b| b.cmp(a));
    for (size, path) in cands.into_iter().take(2) {
        let Ok(bytes) = std::fs::read(&path) else { continue };
        let start = if size > 250_000 { (size - 250_000) as usize } else { 0 };
        let txt = String::from_utf8_lossy(&bytes[start.min(bytes.len())..]).into_owned();
        if size <= 250_000 && since != 0 {
            let head: String = txt.chars().take(4000).collect();
            if let Some(t0) = first_log_stamp(&head) {
                if t0 + 3 < since {
                    continue;
                }
            }
        }
        return txt;
    }
    String::new()
}

/// `[YYYY-MM-DD HH:MM:SS` as local epoch seconds
fn first_log_stamp(txt: &str) -> Option<i64> {
    let b = txt.as_bytes();
    let mut i = 0;
    while let Some(off) = txt[i..].find('[') {
        let s = i + off + 1;
        if s + 19 <= b.len() {
            let cand = &txt[s..s + 19];
            if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(cand, "%Y-%m-%d %H:%M:%S") {
                use chrono::TimeZone;
                return chrono::Local.from_local_datetime(&dt).earliest().map(|d| d.timestamp());
            }
        }
        i = s;
    }
    None
}

fn session_live() -> bool {
    this_boot_log().contains("Game Started!")
}

fn menu_not_ready_error(action: &str) -> String {
    if !gameui::listening() {
        let port = std::env::var("INSPECTOR_PORT").unwrap_or_else(|_| "9444".into());
        return format!("the game launched but its menu inspector never came up on :{port}. Drive cannot press {action} without it. Close EVO, then Drive again — ACECM will enable the inspector. If that write is refused, run ACECM as administrator once");
    }
    format!("the menu never became ready — open the game and press {action} once")
}

/// Last live gate: menu.html + CurrentCar (or already on SP / pits).
fn wait_ready(deadline: Instant) -> Result<String, Cancelled> {
    let mut saw_game = false;
    let mut last_beat: Option<Instant> = None;
    let mut last_boot = String::new();
    let mut last_home: Option<Instant> = None;
    while Instant::now() < deadline {
        let running = game_running();
        if running {
            saw_game = true;
        } else if saw_game {
            logs::info("drive: game closed while waiting for the menu");
            return Ok("gone".into());
        }
        if session_live() {
            return Ok("live".into());
        }
        let boot = gameui::boot_state();
        if !boot.is_empty() && boot != last_boot {
            logs::info(format!("drive boot {boot}"));
            last_boot = boot.clone();
        }
        if gameui::session_loading(&boot) || gameui::in_pits(Some(&boot)) {
            return Ok("live".into());
        }
        if gameui::home_ready(&boot) {
            return Ok("ready".into());
        }
        // ⚠ parked on Multiplayer: home never comes back on its own
        let page = gameui::boot_page(&boot);
        if !page.is_empty() && !["home", "sp", "intro", "menu-nocar"].contains(&page.as_str())
            && boot.contains("|car|") && last_home.map(|t| t.elapsed() >= Duration::from_secs(4)).unwrap_or(true) {
            logs::info(format!("drive: on {page:?}, sending the menu home: {}", py_repr(&gameui::go_home())));
            last_home = Some(Instant::now());
        }
        if last_beat.map(|t| t.elapsed() >= Duration::from_secs(5)).unwrap_or(true) {
            logs::info(format!("drive waiting for home (game={} inspector={} boot={})",
                if running { "True" } else { "False" }, if gameui::listening() { "True" } else { "False" },
                if boot.is_empty() { "-" } else { &boot }));
            last_beat = Some(Instant::now());
        }
        sleep(0.3)?;
    }
    Ok(String::new())
}

/// A failed UI step is not a failed launch, as long as the game is up: the
/// session is already decided, so say what could not be pressed and hand
/// over.
fn ui_failed(poked: &Value, where_: &str) -> bool {
    if game_running() {
        let why = err_of(poked, "the menu did not respond");
        set(json!({"phase": "launched",
            "hint": format!("the game is up and your session is set — press Start in the menu to go (ACECM could not: {why})"),
            "fault": ""}));
        logs::info(format!("drive: handing over to the player at {where_}: {why}"));
        return true;
    }
    false
}

fn boot_took(what: &str) {
    let t0 = job_get("t_launch").as_f64().unwrap_or(0.0);
    if t0 == 0.0 {
        return;
    }
    let took = now_f() - t0;
    let flags = if truthy(config::load().get("fast_boot")) { "fast_boot" } else { "default" };
    logs::info(format!("drive TIMING: {what} after {took:.1}s ({flags})"));
    with_job(|j| {
        if !j.get("timing").map(|t| t.is_object()).unwrap_or(false) {
            j.insert("timing".into(), json!({}));
        }
        j["timing"][what] = json!((took * 10.0).round_ties_even() / 10.0);
    });
}

/// Set car / conditions once, open SP once, press Start once.
fn enter_and_start(pick: &Map<String, Value>) -> Result<Value, Cancelled> {
    let deadline = Instant::now() + Duration::from_secs(90);
    set(json!({"phase": "waiting_for_menu", "hint": "waiting for the menu"}));
    let state = wait_ready(deadline)?;
    match state.as_str() {
        "gone" => return Ok(json!({"ok": false, "error": "the game closed before Start"})),
        "live" => {
            logs::info("drive session already live for this boot");
            gameui::focus_game();
            return Ok(json!({"ok": true, "started": "already"}));
        }
        "ready" => {}
        _ => return Ok(json!({"ok": false, "error": menu_not_ready_error("Start")})),
    }
    boot_took("menu ready");
    let car = s_or(get(pick, "car"));
    let model = car_model(&car);
    if !game_running() {
        return Ok(json!({"ok": false, "error": "the game closed before Start"}));
    }
    if session_live() {
        gameui::focus_game();
        return Ok(json!({"ok": true, "started": "already"}));
    }
    gameui::focus_game();
    let have = gameui::current_car_name();
    if !model.is_empty() && have != model {
        set(json!({"phase": "selecting_car", "hint": format!("setting {model}")}));
        match gameui::select_car(&model, &car, "", "") {
            Ok(chosen) => logs::info(format!("drive set car: {}", py_repr(&chosen))),
            Err(e) => return Ok(json!({"ok": false, "error": format!("could not set car: {e}")})),
        }
    } else if !model.is_empty() {
        logs::info(format!("drive car already {have}"));
    }
    set(json!({"phase": "setting_conditions", "hint": "setting weather, time and mode"}));
    match gameui::apply_conditions(&Value::Object(pick.clone()), "CLEAR", 13, 0) {
        Ok(c) => logs::info(format!("drive conditions: {}", py_repr(&c))),
        Err(e) => return Ok(json!({"ok": false, "error": format!("could not set conditions: {e}")})),
    }
    set(json!({"phase": "opening_sp", "hint": "opening Single Player"}));
    match gameui::enter_singleplayer() {
        Ok(w) => logs::info(format!("drive ui goto: {}", py_repr(&w))),
        Err(e) => return Ok(json!({"ok": false, "error": format!("could not open Single Player: {e}")})),
    }
    let on = wait_page("sp", 20.0)?;
    if gameui::boot_page(&on) != "sp" {
        return Ok(json!({"ok": false, "error": format!("Single Player never stayed open — the game kept returning to the home menu (last {})", if on.is_empty() { "-" } else { &on })}));
    }
    // ⚠ set the track HERE, like the car, instead of trusting
    // -startup_gamemode to have carried it in
    if let Some(ev) = event(pick) {
        let tname = { let t = s_or(ev.get("track")); if t.is_empty() { "the track".into() } else { t } };
        set(json!({"phase": "selecting_track", "hint": format!("setting {tname}")}));
        let evn = { let n = s_or(ev.get("name")); if n.is_empty() { s_or(ev.get("event_name")) } else { n } };
        match gameui::select_track(&s_or(ev.get("track")), &s_or(ev.get("layout")), &evn, int0(ev.get("length_m"))) {
            Ok(p) => logs::info(format!("drive set track: {}", py_repr(&p))),
            Err(e) => logs::warn(format!("drive set track failed: {e}")),
        }
    }
    // ⚠ never press Start with nothing selected: no spawn points, opponents
    // -1, a crash in a render worker
    let track_now = gameui::selected_track(10.0);
    if track_now.is_empty() {
        return Ok(json!({"ok": false, "error": "Single Player still has no track selected after asking the game to set it, so Start was not pressed (it would crash the game). Pick the track in-game and press Drive again."}));
    }
    logs::info(format!("drive track confirmed: {track_now}"));
    set(json!({"phase": "starting_session", "hint": "starting the session"}));
    let started = match gameui::press_start() {
        Ok(s) => s,
        Err(e) => return Ok(json!({"ok": false, "error": format!("Start failed: {e}")})),
    };
    logs::info(format!("drive ui start: {}", py_repr(&started)));
    boot_took("session started");
    gameui::focus_game();
    let val = val_str(&started);
    if !truthy(started.get("ok")) {
        let e = s_or(started.get("error"));
        return Ok(json!({"ok": false, "error": format!("Start failed: {}", if e.is_empty() { "no reply".into() } else { e })}));
    }
    if val.starts_with("fail:") || val.starts_with("no-") || val == "not-on-sp" || val == "paintshop" {
        return Ok(json!({"ok": false, "error": format!("Start failed: {val}")}));
    }
    let until = Instant::now() + Duration::from_secs(25);
    let mut last_boot = on;
    while Instant::now() < until {
        if !game_running() {
            return Ok(json!({"ok": false, "error": "the game closed after Start"}));
        }
        let boot = gameui::boot_state();
        if !boot.is_empty() && boot != last_boot {
            logs::info(format!("drive boot {boot}"));
            last_boot = boot.clone();
        }
        if gameui::session_loading(&boot) || gameui::in_pits(Some(&boot)) || session_live() {
            logs::info(format!("drive session loading ({boot})"));
            gameui::focus_game();
            return Ok(json!({"ok": true, "start": started}));
        }
        sleep(0.4)?;
    }
    Ok(json!({"ok": false, "error": format!("Start was pressed once but the session never loaded (still {})", if last_boot.is_empty() { "on the menu" } else { &last_boot })}))
}

// -------------------------------------------------------------- start() --
fn body_int(body: &Map<String, Value>, k: &str, fallback: i64) -> i64 {
    match body.get(k) {
        Some(v) if !v.is_null() => py_int(v).unwrap_or(fallback),
        _ => fallback,
    }
}

fn prev_int(prev: &Map<String, Value>, k: &str, d: i64) -> i64 {
    prev.get(k).and_then(py_int).unwrap_or(d)
}

fn strip_or(body: &Map<String, Value>, prev: &Map<String, Value>, k: &str) -> String {
    let b = s_or(body.get(k));
    if !b.is_empty() { b.trim().to_string() } else { s_or(prev.get(k)).trim().to_string() }
}

/// Write the SP save and launch. Returns once the job is queued.
pub fn start(body: &Value) -> Value {
    let empty = Map::new();
    let body = body.as_object().unwrap_or(&empty);
    let prev = load_pick();
    let mut via = { let v = s_or(body.get("via")); let v = if v.is_empty() { s_or(prev.get("via")) } else { v }; if v.is_empty() { "sp".into() } else { v } }.trim().to_lowercase();
    if !["sp", "server", "local"].contains(&via.as_str()) {
        via = "sp".into();
    }
    let up = |k: &str, d: &str| { let v = s_or(body.get(k)); (if v.is_empty() { d.to_string() } else { v }).trim().to_uppercase() };
    let mut m = Map::new();
    m.insert("via".into(), json!(via));
    m.insert("local_id".into(), json!(strip_or(body, &prev, "local_id")));
    m.insert("server_id".into(), json!(strip_or(body, &prev, "server_id")));
    m.insert("server_ip".into(), json!(strip_or(body, &prev, "server_ip")));
    m.insert("server_tcp_port".into(), json!(body_int(body, "server_tcp_port", int0(prev.get("server_tcp_port")))));
    m.insert("server_udp_port".into(), json!(body_int(body, "server_udp_port", int0(prev.get("server_udp_port")))));
    m.insert("password".into(), if body.contains_key("password") { body["password"].clone() } else { json!(s_or(prev.get("password"))) });
    m.insert("car".into(), json!(s_or(body.get("car")).trim()));
    m.insert("track_index".into(), json!(body_int(body, "track_index", prev_int(&prev, "track_index", 18))));
    let ct = if body.contains_key("custom_track") { s_of(body.get("custom_track")) } else { s_or(prev.get("custom_track")) };
    m.insert("custom_track".into(), json!(ct.trim()));
    m.insert("game_mode".into(), json!(up("game_mode", "PRACTICE")));
    m.insert("weather".into(), json!(up("weather", "CLEAR")));
    m.insert("grip".into(), json!(up("grip", "OPTIMUM")));
    m.insert("race_type".into(), json!(up("race_type", "LAPS")));
    m.insert("race_minutes".into(), json!(body_int(body, "race_minutes", 20)));
    m.insert("time_mult".into(), json!(body_int(body, "time_mult", 1)));
    m.insert("tod_hour".into(), json!(body_int(body, "tod_hour", 13)));
    m.insert("tod_minute".into(), json!(body_int(body, "tod_minute", 0)));
    for (k, d) in [("num_opponents", 10), ("skill_min", 80), ("skill_max", 95)] {
        m.insert(k.into(), json!(body_int(body, k, prev_int(&prev, k, d))));
    }
    let ag = { let a = s_or(body.get("aggressiveness")); let a = if a.is_empty() { s_or(prev.get("aggressiveness")) } else { a }; if a.is_empty() { "Safe".into() } else { a } };
    m.insert("aggressiveness".into(), json!(ag));
    let sm = if body.contains_key("single_make") { truthy(body.get("single_make")) } else { match prev.get("single_make") { None => true, v => truthy(v) } };
    m.insert("single_make".into(), json!(sm));
    for (k, d) in [("duration_min", 90), ("practice_min", 10), ("quali_min", 15), ("warmup_min", 10), ("race_laps", 10), ("starting_position", 0)] {
        m.insert(k.into(), json!(body_int(body, k, prev_int(&prev, k, d))));
    }
    let pick = save_pick(m);
    if s_or(pick.get("car")).is_empty() {
        return json!({"ok": false, "error": "pick a car first"});
    }
    let car = s_or(pick.get("car"));
    match s_or(pick.get("via")).as_str() {
        "server" => {
            let Some(sv) = find_public(&pick) else { return json!({"ok": false, "error": "pick a public server first"}) };
            if !car_allowed(&sv, &car) {
                return json!({"ok": false, "error": "that car is not allowed on this server"});
            }
        }
        "local" => {
            let Some(prof) = find_local(&pick) else { return json!({"ok": false, "error": "pick one of your ACECM servers"}) };
            if !car_allowed(&json!({"cars": servers::allowed_car_ids(&prof)}), &car) {
                return json!({"ok": false, "error": "that car is not allowed on this server"});
            }
        }
        _ => {
            let gm = s_of(pick.get("game_mode"));
            if !SP_MODES.contains(&gm.as_str()) {
                return json!({"ok": false, "error": format!("{gm} is not a single-player mode")});
            }
        }
    }
    // ⚠ "not finished", not a list of busy phases
    let ph = phase();
    if !TERMINAL.contains(&ph.as_str()) {
        if ph == "waiting_for_menu" && !game_running() {
            set(json!({"phase": "failed", "fault": "the game closed before Start"}));
        } else {
            return json!({"ok": false, "error": "Drive is already running — wait for it"});
        }
    }
    if !truthy(content::tracks().get("tracks")) {
        return json!({"ok": false, "error": "no track list — set the dedicated server folder in Settings"});
    }
    let running = game_running();
    if running && !gameui::listening() {
        return json!({"ok": false, "error": menu_not_ready_error("Start")});
    }
    let via = s_or(pick.get("via"));
    if via == "server" || via == "local" {
        // the FIRST real step, since the checklist shows it
        let first = if running { "entering" } else if via == "local" { "starting_backend" } else { "launching_game" };
        let hint = if running { "joining…" } else if first == "starting_backend" { "starting the lobby" } else { "launching the game…" };
        set(json!({"phase": first, "hint": hint, "fault": "", "started": now_i(), "kind": via,
                   "join": null, "launch": null, "wrote": null}));
    } else {
        set(json!({"phase": if running { "entering" } else { "writing" }, "hint": if running { "pressing Start…" } else { "writing session…" },
                   "fault": "", "started": now_i(), "kind": "sp", "join": null, "launch": null, "wrote": null}));
    }
    let p2 = pick.clone();
    std::thread::spawn(move || run(p2));
    json!({"ok": true, "phase": phase(), "pick": pick, "hint": "writing the session, launching, then pressing Start"})
}

// -------------------------------------------------------------- capture --
fn game_still_up() -> bool {
    game_running() || gameui::window_open()
}

/// Ask the menu to quit, close the window, then force-kill the tree.
fn close_game() -> bool {
    logs::info(format!("drive quit: {}", py_repr(&gameui::quit_game())));
    if gameui::close_window() {
        logs::info("drive quit: posted WM_CLOSE");
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if !game_still_up() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    let mut pids = procs::pids_named(&["AssettoCorsaEVO"]);
    pids.extend(procs::pids_named_prefix(&["assettocorsaevo"]));
    pids.sort();
    pids.dedup();
    for pid in pids {
        procs::kill(pid);
        logs::info(format!("drive killed leftover EVO pid {pid}"));
    }
    std::thread::sleep(Duration::from_millis(500));
    let up = game_still_up();
    if up {
        logs::warn("drive quit: game still running after force-kill");
    }
    !up
}

fn fresh_list(since: i64) -> (usize, i64, bool) {
    let lst = backend::server_list();
    let n = lst.get("servers").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
    let at = int0(lst.get("captured_at"));
    (n, at, n > 0 && at >= since - 3)
}

fn run_capture_inner(launched_us: &mut bool) -> Result<(), Cancelled> {
    let since = { let s = int0(Some(&job_get("started"))); if s == 0 { now_i() } else { s } };
    // fast path: ask the lobby ourselves (~2.5 s, no game)
    if !game_running() {
        set(json!({"phase": "capturing_list", "hint": "asking the lobby directly"}));
        let r = backend::lobby_fetch(true, 30);
        if truthy(r.get("ok")) && !truthy(r.get("busy")) {
            let n = int0(r.get("count"));
            job_put("captured", json!(n));
            let ms = { let s = r.get("spawn_ms").and_then(|v| v.as_f64()).filter(|x| *x != 0.0); s.or_else(|| r.get("ms").and_then(|v| v.as_f64())).unwrap_or(0.0) };
            set(json!({"phase": "launched", "hint": format!("got {n} public servers in {:.1} s", ms / 1000.0)}));
            return Ok(());
        }
        logs::info(format!("direct list fetch failed, launching the game: {}", s_of(r.get("error"))));
    }
    ensure_backend();
    if !truthy(backend::state().get("listening")) {
        set(json!({"phase": "capturing_list", "hint": "starting the lobby proxy"}));
        let r = backend::start("proxy");
        if !truthy(r.get("ok")) {
            set(json!({"phase": "failed", "fault": err_of(&r, "could not start the proxy")}));
            return Ok(());
        }
    }
    if game_running() && !gameui::listening() {
        set(json!({"phase": "failed", "fault": menu_not_ready_error("capture")}));
        return Ok(());
    }
    if !game_running() {
        set(json!({"phase": "launching_game", "hint": "launching the game to pull the public list"}));
        let launched = launch::launch_game(&["-no_intro".to_string()]);
        job_put("launch", launch_summary(&launched));
        if !truthy(launched.get("ok")) {
            set(json!({"phase": "failed", "fault": err_of(&launched, "could not launch")}));
            return Ok(());
        }
        *launched_us = true;
    }
    set(json!({"phase": "waiting_for_menu", "hint": "waiting for the home screen"}));
    let state = wait_ready(Instant::now() + Duration::from_secs(90))?;
    if state == "gone" {
        set(json!({"phase": "failed", "fault": "the game closed before the list arrived"}));
        return Ok(());
    }
    if state != "ready" && state != "live" {
        if *launched_us {
            close_game();
        }
        set(json!({"phase": "failed", "fault": menu_not_ready_error("capture")}));
        return Ok(());
    }
    set(json!({"phase": "capturing_list", "hint": "opening the in-game server list"}));
    gameui::focus_game();
    // ⚠ ask repeatedly and CHECK the page changed: a busy UI drops gotos
    let goto_deadline = Instant::now() + Duration::from_secs(45);
    let mut landed = false;
    while Instant::now() < goto_deadline {
        if !game_running() {
            set(json!({"phase": "failed", "fault": "the game closed before the list arrived"}));
            return Ok(());
        }
        match gameui::enter_multiplayer("", 0, "", true) {
            Ok(r) => logs::info(format!("drive capture goto: {}", py_repr(&r))),
            Err(e) => logs::warn(format!("drive capture goto: {e}")),
        }
        sleep(1.5)?;
        if gameui::server_list_page() {
            landed = true;
            break;
        }
        set(json!({"phase": "capturing_list", "hint": "waiting for the in-game server list to open"}));
    }
    if !landed {
        logs::warn("drive capture: multiplayer never opened");
    }
    match gameui::refresh_server_list() {
        Ok(r) => logs::info(format!("drive capture refresh: {}", py_repr(&r))),
        Err(e) => logs::warn(format!("drive capture refresh: {e}")),
    }
    let deadline = Instant::now() + Duration::from_secs(75);
    let mut last_n = 0;
    let mut done = false;
    while Instant::now() < deadline {
        if !game_running() {
            set(json!({"phase": "failed", "fault": "the game closed before the list arrived"}));
            return Ok(());
        }
        let (n, _at, fresh) = fresh_list(since);
        last_n = n;
        if fresh && n >= 10 {
            done = true;
            break;
        }
        set(json!({"phase": "capturing_list", "hint": format!("waiting for the public list… {n} so far")}));
        if deadline.saturating_duration_since(Instant::now()) < Duration::from_secs(12) {
            let _ = gameui::refresh_server_list();
        }
        sleep(0.6)?;
    }
    if !done {
        let (n, _at, fresh) = fresh_list(since);
        last_n = n;
        if !(fresh && n >= 1) {
            if *launched_us {
                close_game();
            }
            set(json!({"phase": "failed", "fault": "the public list did not come through — is the lobby URL rewritten? Open Multiplayer once and watch the Backend tab."}));
            return Ok(());
        }
    }
    set(json!({"phase": "quitting_game", "hint": format!("got {last_n} servers — closing the game")}));
    close_game();
    job_put("captured", json!(last_n));
    set(json!({"phase": "launched", "hint": format!("captured {last_n} public servers — game closed, Drive is ready")}));
    Ok(())
}

fn run_capture() {
    *JOB_THREAD.lock().unwrap() = Some(std::thread::current().id());
    let mut launched_us = false;
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_capture_inner(&mut launched_us)));
    match r {
        Ok(Ok(())) => {}
        Ok(Err(Cancelled)) => {
            if launched_us {
                close_game();
            }
            set(json!({"phase": "cancelled", "hint": "stopped", "fault": ""}));
        }
        Err(p) => {
            let msg = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_else(|| "panic".into());
            logs::warn(format!("drive capture: {msg}"));
            if launched_us {
                close_game();
            }
            set(json!({"phase": "failed", "fault": format!("RuntimeError: {msg}")}));
        }
    }
}

/// Refresh server_list.json: straight from the lobby, else via the game.
pub fn capture_list() -> Value {
    if !TERMINAL.contains(&phase().as_str()) {
        return json!({"ok": false, "error": "Drive is already running — wait for it"});
    }
    if game_running() && !gameui::listening() {
        return json!({"ok": false, "error": menu_not_ready_error("capture")});
    }
    set(json!({"phase": "capturing_list", "hint": "preparing to pull the public list", "fault": "",
               "started": now_i(), "kind": "capture", "join": null, "launch": null, "wrote": null, "captured": 0}));
    std::thread::spawn(run_capture);
    json!({"ok": true, "phase": phase(), "hint": "asking the lobby for the public list"})
}

pub fn status() -> Value {
    let pick = load_pick();
    let exe = backend::game_exe();
    let job = with_job(|j| j.clone());
    let g = |k: &str| job.get(k).cloned().unwrap_or(Value::Null);
    let ph = phase();
    json!({
        "ok": true,
        "pick": pick,
        "phase": ph,
        "hint": s_or(job.get("hint")),
        "fault": s_or(job.get("fault")),
        "started": if truthy(job.get("started")) { g("started") } else { json!(0) },
        "wrote": g("wrote"),
        "launch": g("launch"),
        "game_running": !exe.is_empty() && game_running(),
        "single_player": true,
        "captured": if truthy(job.get("captured")) { g("captured") } else { json!(0) },
        "kind": s_or(job.get("kind")),
        "steps": job.get("steps").filter(|s| s.is_array()).cloned().unwrap_or(json!([])),
        "stopping": CANCEL.load(Ordering::SeqCst) && !TERMINAL.contains(&ph.as_str()),
        "now": now_f(),
    })
}

// ------------------------------------------------------------ favourites --
fn save_favourites(items: &[Value]) {
    if let Err(e) = profiles::write_json_indent2(&paths::data().join("favourites.json"), &Value::Array(items.to_vec())) {
        logs::warn(format!("could not write favourites: {e}"));
    }
}

/// Pin an address. ⚠ ip+port is the identity, not the name (names change).
pub fn favourite_add(target: &str, name: &str) -> Value {
    let info = direct_lookup(target);
    if info.get("ok") != Some(&json!(true)) {
        return info;
    }
    let ip = s_of(info.get("ip"));
    let tcp = int0(info.get("tcp"));
    let mut items: Vec<Value> = crate::misc::favourites()["favourites"].as_array().cloned().unwrap_or_default()
        .into_iter().filter(|f| !(s_of(f.get("ip")) == ip && int0(f.get("tcp")) == tcp)).collect();
    let nm = [name.to_string(), s_or(info.get("name")), format!("{ip}:{tcp}")].into_iter().find(|s| !s.is_empty()).unwrap_or_default();
    let udp = if truthy(info.get("udp")) { info["udp"].clone() } else { json!(tcp) };
    items.insert(0, json!({"id": format!("{ip}:{tcp}"), "name": nm.trim(), "ip": ip, "tcp": tcp, "udp": udp, "added": now_i()}));
    save_favourites(&items);
    json!({"ok": true, "favourites": items, "added": format!("{ip}:{tcp}")})
}

pub fn favourite_remove(fid: &str) -> Value {
    let items: Vec<Value> = crate::misc::favourites()["favourites"].as_array().cloned().unwrap_or_default()
        .into_iter().filter(|f| s_of(f.get("id")) != fid).collect();
    save_favourites(&items);
    json!({"ok": true, "favourites": items})
}

/// What is at a pasted address: the captured list's entry, else the host's
/// own ACECM registry, else honestly unknown (still joinable).
/// ⚠ The pasted port is the GAME's; ACECM is probed on its own two ports.
pub fn direct_lookup(raw: &str) -> Value {
    let (host, port, _base) = crate::contentsync::parse_target(raw);
    if host.is_empty() {
        return json!({"ok": false, "error": "type an address, like 1.2.3.4:9700"});
    }
    let tcp = port.map(|p| p as i64).unwrap_or(9700);
    for s in backend::server_list().get("servers").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
        if s_of(s.get("server_ip")) == host && int0(s.get("server_tcp_port")) == tcp {
            let udp = { let u = int0(s.get("server_udp_port")); if u == 0 { tcp } else { u } };
            let name = { let n = s_or(s.get("server_name")); if n.is_empty() { host.clone() } else { n } };
            return json!({"ok": true, "source": "list", "ip": host, "tcp": tcp, "udp": udp, "name": name,
                "track": s_or(s.get("track")), "layout": s_or(s.get("layout")),
                "players": if truthy(s.get("players")) { s["players"].clone() } else { json!(0) },
                "max_players": if truthy(s.get("max_players")) { s["max_players"].clone() } else { json!(0) },
                "locked": truthy(s.get("driver_password")), "cars": car_ids_of(&s)});
        }
    }
    let got = crate::contentsync::discover(&host, None, &[8092, 8093], 2.0);
    if got.get("ok") == Some(&json!(true)) {
        let entries = got.get("servers").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        // ⚠ a port match only when UNAMBIGUOUS: entries default to 9700
        let onport: Vec<&Value> = entries.iter().filter(|e| int0(e.get("port")) == tcp).collect();
        let hit = if onport.len() == 1 { Some(onport[0]) } else if entries.len() == 1 { Some(&entries[0]) } else { None };
        let base = s_or(got.get("base"));
        if let Some(hit) = hit {
            let rt = hit.get("required_tracks").and_then(|v| v.as_array()).cloned().unwrap_or_default();
            let tracks: Vec<String> = rt.iter().map(|t| s_of(Some(t))).collect();
            let hp = int0(hit.get("port"));
            return json!({"ok": true, "source": "acecm", "ip": host, "tcp": if hp != 0 { hp } else { tcp }, "udp": tcp,
                "name": ({ let n = s_or(hit.get("name")); if n.is_empty() { host.clone() } else { n } }),
                "description": s_or(hit.get("description")), "track": tracks.join(", "),
                "needs_mods": hit.get("required_mods").filter(|v| truthy(Some(v))).cloned().unwrap_or(json!([])),
                "needs_tracks": if rt.is_empty() { json!([]) } else { json!(rt) },
                "content_bytes": if truthy(hit.get("content_bytes")) { hit["content_bytes"].clone() } else { json!(0) },
                "base": base, "sid": s_or(hit.get("id")), "cars": []});
        }
        let ambiguous = onport.len() > 1;
        let note = if ambiguous {
            format!("{} of this host's {} servers claim port {tcp}, so which one you mean is not clear - Get content will fetch what they all need", onport.len(), entries.len())
        } else {
            format!("{} servers shared here and none lists port {tcp} - Get content will fetch what they all need", entries.len())
        };
        let sids: Vec<Value> = entries.iter().filter(|e| truthy(e.get("id"))).map(|e| e["id"].clone()).collect();
        let names: Vec<Value> = entries.iter().map(|e| if truthy(e.get("name")) { e["name"].clone() } else { e.get("id").cloned().unwrap_or(Value::Null) }).collect();
        return json!({"ok": true, "source": "acecm", "ip": host, "tcp": tcp, "udp": tcp, "name": host, "base": base,
            "sids": sids, "names": names, "note": note, "cars": []});
    }
    json!({"ok": true, "source": "unknown", "ip": host, "tcp": tcp, "udp": tcp, "name": format!("{host}:{tcp}"), "cars": [],
        "note": "not in your captured list and the host is not sharing through ACECM, so its track and cars are unknown - joining will still work if the address is right"})
}

// --------------------------------------------------------- the Drive page --
/// ACECM profiles as Drive can start / join them.
fn local_briefs() -> Vec<Value> {
    let mut out = Vec::new();
    for p in profiles::load() {
        let st = serverproc::status(&p);
        let ev = lobby::event(&p);
        let evs = |k: &str| ev.get(k).map(|v| s_or(Some(v))).unwrap_or_default();
        let tcp = { let t = int0(p.get("tcp_port")); if t == 0 { 9700 } else { t } };
        let http = { let t = int0(p.get("http_port")); if t == 0 { 8080 } else { t } };
        let maxp = { let t = int0(p.get("max_players")); if t == 0 { 90 } else { t } };
        let custom = s_or(p.get("custom_track")).trim().to_string();
        let track = { let t = evs("track"); if t.is_empty() { s_or(p.get("custom_track")) } else { t } };
        out.push(json!({
            "id": p.get("id"),
            "name": ({ let n = s_or(p.get("name")); if n.is_empty() { "(unnamed)".to_string() } else { n } }),
            "running": truthy(st.get("running")),
            "clients": st.get("clients").cloned().unwrap_or(Value::Null),
            "track": track, "layout": evs("layout"), "custom_track": custom,
            "tcp_port": tcp, "udp_port": tcp, "http_port": http, "max_players": maxp,
            "cars": servers::allowed_car_ids(&p),
            "no_lobby": truthy(p.get("no_lobby")), "listed": !truthy(p.get("no_lobby")),
            "locked": truthy(p.get("driver_password")),
            "game_mode": ({ let g = s_or(p.get("game_mode")); if g.is_empty() { "PRACTICE".to_string() } else { g } }),
        }));
    }
    out
}

/// GET /api/drive: everything the page needs to draw - but NOT the public
/// list, which is the slow half and arrives separately.
pub fn options() -> Value {
    let tracks = content::tracks();
    let cars = content::cars();
    let be = backend::state();
    let pick = load_pick();
    let mut modes: Vec<String> = OFFERED_MODES.iter().map(|s| s.to_string()).collect();
    let cur = s_or(pick.get("game_mode")).to_uppercase();
    // ⚠ plus whatever is selected, even when no longer offered
    if !cur.is_empty() && SP_MODES.contains(&cur.as_str()) && !OFFERED_MODES.contains(&cur.as_str()) {
        modes.push(cur);
    }
    json!({
        "ok": true, "pick": pick, "game_modes": modes,
        "weather": serverproc::options().get("weather").cloned().unwrap_or(json!([])),
        "aggressiveness": AGGRO, "ai_modes": AI_MODES,
        "tracks": tracks.get("tracks").filter(|v| truthy(Some(v))).cloned().unwrap_or(json!([])),
        "tracks_error": tracks.get("error").cloned().unwrap_or(Value::Null),
        "cars": cars.get("cars").filter(|v| truthy(Some(v))).cloned().unwrap_or(json!([])),
        "cars_error": cars.get("error").cloned().unwrap_or(Value::Null),
        "servers": [], "car_pool": [], "servers_pending": true, "servers_meta": {},
        "local_servers": local_briefs(),
        "backend": {"listening": truthy(be.get("listening")), "client_patched": truthy(be.get("client_patched"))},
    })
}

/// The public list for the page: the lobby's, ENRICHED from the directory
/// (address match copies source/share_url/track_origin; unseen ones are
/// appended), with car ids interned into a pool.
pub fn public_servers() -> Value {
    backend::lobby_prefetch_default();
    let lst = backend::server_list();
    let mut raw: Vec<Value> = lst.get("servers").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let mut by_addr: std::collections::HashMap<(String, i64), usize> = Default::default();
    for (i, s) in raw.iter().enumerate() {
        by_addr.insert((s_or(s.get("server_ip")), int0(s.get("server_tcp_port"))), i);
    }
    // ⚠ block=false: a third-party host being slow must cost a stale row,
    // never the whole list
    for s in crate::evoreg::fetch(false, false).get("servers").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
        let key = (s_of(s.get("server_ip")), int0(s.get("server_tcp_port")));
        match by_addr.get(&key) {
            None => raw.push(s),
            Some(&i) => {
                for f in ["source", "share_url", "track_origin"] {
                    if truthy(s.get(f)) {
                        raw[i][f] = s[f].clone();
                    }
                }
            }
        }
    }
    let mut pool: Vec<String> = Vec::new();
    let mut seen: std::collections::HashMap<String, usize> = Default::default();
    let mut out = Vec::new();
    for s in &raw {
        let idx: Vec<usize> = car_ids_of(s).into_iter().map(|cid| {
            *seen.entry(cid.clone()).or_insert_with(|| { pool.push(cid); pool.len() - 1 })
        }).collect();
        let or0 = |k: &str| if truthy(s.get(k)) { s[k].clone() } else { json!(0) };
        let gm = [s.get("game_mode_type"), s.get("current_session")].into_iter().flatten().find(|v| truthy(Some(v))).cloned().unwrap_or(json!(""));
        out.push(json!({
            "id": s_or(s.get("server_id")),
            "name": ({ let n = s_or(s.get("server_name")); if n.is_empty() { "(unnamed)".to_string() } else { n } }),
            "server_ip": s_or(s.get("server_ip")), "server_tcp_port": or0("server_tcp_port"), "server_udp_port": or0("server_udp_port"),
            "track": s_or(s.get("track")), "layout": s_or(s.get("layout")), "game_mode": gm,
            "players": or0("players"), "max_players": or0("max_players"), "ping": or0("ping"),
            "cars": idx, "locked": truthy(s.get("driver_password")),
            "source": ({ let v = s_or(s.get("source")); if v.is_empty() { "lobby".to_string() } else { v } }),
            "share_url": s_or(s.get("share_url")), "track_origin": s_or(s.get("track_origin")),
        }));
    }
    let count = if truthy(lst.get("count")) { lst["count"].clone() } else { json!(out.len()) };
    json!({"ok": true, "servers": out, "car_pool": pool,
           "servers_meta": {"count": count, "captured_at": lst.get("captured_at").cloned().unwrap_or(Value::Null),
                            "cached": truthy(lst.get("cached")), "error": lst.get("error").cloned().unwrap_or(Value::Null),
                            "hint": lst.get("hint").cloned().unwrap_or(Value::Null)}})
}
