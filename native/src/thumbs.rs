//! Cached car renders and track covers for the list rows - ported from the
//! acecm/thumbs.py: cars are rendered by evoview, track covers extracted or
//! decoded from BC7 out of the client archive.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use serde_json::{Map, Value};

use crate::{carsmap, livery, paths, trackimport};

fn dir(which: &str) -> PathBuf {
    let d = paths::data().join("thumbs").join(which);
    let _ = std::fs::create_dir_all(&d);
    d
}

fn safe(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || "_.-".contains(c) { c } else { '_' }).collect()
}

/// Keyed by the LOOK (trim + livery), not just the model: a car's trims
/// share one model folder.
pub fn car_path(car_id: &str, big: bool, visual: &str, paint: &str) -> PathBuf {
    let mut tag = String::new();
    if !visual.is_empty() || !paint.is_empty() {
        let raw = format!("{visual}|{paint}").to_lowercase();
        let h = sha1_smol::Sha1::from(raw.as_bytes()).digest().to_string();
        tag = format!("@{}", &h[..10]);
    }
    dir("cars").join(format!("{}{}{}", safe(car_id), tag, if big { "@big.png" } else { ".png" }))
}

pub fn track_path(folder: &str) -> PathBuf {
    dir("tracks").join(format!("{}.png", safe(folder)))
}

static GARAGE: Mutex<Option<(String, Map<String, Value>)>> = Mutex::new(None);

/// model -> the EXT SKIN paint on it. Memoised on the SavedCars folder's
/// mtime (one page of the car list asks once per row).
fn garage_paints() -> Map<String, Value> {
    let d = livery::saved_cars_dir();
    let mt = if !d.is_empty() && Path::new(&d).is_dir() {
        std::fs::metadata(&d).ok().and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|x| x.as_nanos()).unwrap_or(0)
    } else { 0 };
    let key = format!("{d}|{mt}");
    if let Some((k, m)) = GARAGE.lock().unwrap().as_ref() {
        if *k == key {
            return m.clone();
        }
    }
    let mut out = Map::new();
    for c in livery::garage() {
        let model = c.get("model").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let skin = c.get("slots").and_then(|s| s.get("EXT SKIN")).and_then(|v| v.as_str())
            .unwrap_or("").trim().to_string();
        if !model.is_empty() && !skin.is_empty() && !out.contains_key(&model) {
            let leaf = skin.replace('\\', "/");
            out.insert(model, Value::String(leaf.rsplit('/').next().unwrap_or("").to_string()));
        }
    }
    *GARAGE.lock().unwrap() = Some((key, out.clone()));
    out
}

/// (model, visual preset, EXT SKIN paint) for a mechanical preset id.
/// mods::car_models(), reused for a few seconds (it lists both mod folders:
/// ~3 ms, and preset_look runs once per car in a picture check).
fn mod_models() -> Map<String, Value> {
    static M: Mutex<Option<(std::time::Instant, Map<String, Value>)>> = Mutex::new(None);
    let mut g = M.lock().unwrap();
    if let Some((at, m)) = g.as_ref() {
        if at.elapsed().as_secs() < 3 {
            return m.clone();
        }
    }
    let m = crate::mods::car_models();
    *g = Some((std::time::Instant::now(), m.clone()));
    m
}

/// Everything preset_look() reads, as plain maps, rebuilt every few
/// seconds. ⚠ It used to clone the whole preset table, the mod map and the
/// garage paints on EVERY call - ~2 ms each, once per car in a picture check.
struct Looks {
    model: HashMap<String, String>,
    visual: HashMap<String, String>,
    paint: HashMap<String, String>,
}

static LOOKS: Mutex<Option<(std::time::Instant, std::sync::Arc<Looks>)>> = Mutex::new(None);

fn looks() -> std::sync::Arc<Looks> {
    if let Some((at, l)) = LOOKS.lock().unwrap().as_ref() {
        if at.elapsed().as_secs() < 3 {
            return l.clone();
        }
    }
    let t = carsmap::table(false);
    let s = |v: &Value| v.as_str().unwrap_or("").to_string();
    let mut model: HashMap<String, String> = t.get("presets").and_then(|p| p.as_object())
        .map(|m| m.iter().map(|(k, v)| (k.clone(), s(v))).collect()).unwrap_or_default();
    // ⚠ a MOD's presets are not in the base game's table: its own manifest
    // names the car. Without this the model was the preset id, no garage
    // paint matched, and the render fell back to the bare model
    for (k, v) in mod_models() {
        model.entry(k).or_insert_with(|| s(&v));
    }
    let visual = t.get("variants").and_then(|p| p.as_object())
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.get("visual").map(s).unwrap_or_default())).collect()).unwrap_or_default();
    let paint = garage_paints().iter().map(|(k, v)| (k.clone(), s(v))).collect();
    let l = std::sync::Arc::new(Looks { model, visual, paint });
    *LOOKS.lock().unwrap() = Some((std::time::Instant::now(), l.clone()));
    l
}

pub fn preset_look(preset_id: &str) -> (String, String, String) {
    let l = looks();
    let model = l.model.get(preset_id).filter(|m| !m.is_empty()).cloned().unwrap_or_else(|| preset_id.to_string());
    let visual = l.visual.get(preset_id).cloned().unwrap_or_default();
    let paint = l.paint.get(&model).cloned().unwrap_or_default();
    (model, visual, paint)
}

/// The cached render a list row shows, or None (no render is ever MADE here).
pub fn list_car(id: &str, preset: &str) -> Option<PathBuf> {
    let (mut cid, mut visual, mut paint) = (id.to_string(), String::new(), String::new());
    if !preset.is_empty() {
        let (m, v, p) = preset_look(preset);
        if cid.is_empty() {
            cid = m;
        }
        visual = v;
        paint = p;
    }
    if cid.is_empty() {
        return None;
    }
    let shot = car_path(&cid, false, &visual, &paint);
    if shot.is_file() {
        return Some(shot);
    }
    // the trim-and-livery render may not exist yet: fall back to the model
    if !visual.is_empty() || !paint.is_empty() {
        let plain = car_path(&cid, false, "", "");
        if plain.is_file() {
            return Some(plain);
        }
    }
    None
}

/// Folder id or the DISPLAY NAME some lists send ("Circuit Of The Americas").
pub fn resolve_folder(folder: &str) -> String {
    if folder.is_empty() {
        return String::new();
    }
    let m = trackimport::track_map(false);
    if m.values().any(|v| v.as_str() == Some(folder)) {
        return folder.to_string();
    }
    if let Some(hit) = m.get(folder).and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
        return hit.to_string();
    }
    let low = folder.trim().to_lowercase();
    for (name, f) in &m {
        if name.trim().to_lowercase() == low {
            return f.as_str().unwrap_or(folder).to_string();
        }
    }
    folder.to_string()
}

// ------------------------------------------------------------ making them --
// ⚠ Rendering is serialised: each car costs a GPU context and ~2 s, and the
// user may have the viewer open.
use serde_json::json;
use std::collections::{BTreeSet, HashMap, HashSet};

const SIZE: (u32, u32) = (900, 600);
const BIG: (u32, u32) = (1440, 960);
/// ⚠ Pictures are drawn at twice their size and halved. Many MOD cars ship
/// broken small mip levels: drawn small, the livery samples those and the car
/// came out plain white with grey noise (RX-8, RS6...), while the viewer -
/// car filling the window, large mips - looked right. Drawn big, the GPU
/// stays on the good levels; halving also anti-aliases every car.
const SUPERSAMPLE: u32 = 2;

/// Halve an RGBA image by 2x2 averaging, to RGB.
fn halve(px: &[u8], w: u32, h: u32) -> Vec<u8> {
    let (ow, oh) = (w / 2, h / 2);
    let mut out = vec![0u8; (ow * oh * 3) as usize];
    for y in 0..oh {
        for x in 0..ow {
            for c in 0..3 {
                let at = |dx: u32, dy: u32| px[(((y * 2 + dy) * w + x * 2 + dx) * 4 + c) as usize] as u32;
                out[((y * ow + x) * 3 + c) as usize] = ((at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1) + 2) / 4) as u8;
            }
        }
    }
    out
}

/// Read the supersampled render at `src`, write it halved to `dst`.
fn downsample(src: &Path, dst: &Path) -> Result<(), String> {
    let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(src).map_err(|e| e.to_string())?));
    let mut rd = dec.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; rd.output_buffer_size()];
    let info = rd.next_frame(&mut buf).map_err(|e| e.to_string())?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err(format!("unexpected render format {:?} {:?}", info.color_type, info.bit_depth));
    }
    let rgb = halve(&buf[..info.buffer_size()], info.width, info.height);
    let tmp = dst.with_extension("png.part");
    {
        let file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), info.width / 2, info.height / 2);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().map_err(|e| e.to_string())?;
        wr.write_image_data(&rgb).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp, dst).map_err(|e| e.to_string())
}

static JOB: Mutex<Option<Map<String, Value>>> = Mutex::new(None);
static COVER_JOB: Mutex<Option<Map<String, Value>>> = Mutex::new(None);

fn idle_job() -> Map<String, Value> {
    match json!({"state": "idle", "done": 0, "total": 0, "current": "", "made": 0}) { Value::Object(m) => m, _ => unreachable!() }
}

fn job_set(j: &Mutex<Option<Map<String, Value>>>, kv: Value) {
    if let Value::Object(kv) = kv {
        let mut g = j.lock().unwrap();
        let m = g.get_or_insert_with(idle_job);
        for (k, v) in kv {
            m.insert(k, v);
        }
    }
}

fn job_get(j: &Mutex<Option<Map<String, Value>>>) -> Map<String, Value> {
    j.lock().unwrap().get_or_insert_with(idle_job).clone()
}

pub fn job() -> Value {
    Value::Object(job_get(&JOB))
}

pub fn cover_job() -> Value {
    Value::Object(job_get(&COVER_JOB))
}

/// One car rendered by evoview into the cache; `make=false` never launches
/// it (a list GET must not spawn a console per row).
/// Pictures that failed to render this session. ⚠ A car the viewer cannot
/// load (a mod preset not in the base archive) failed again on EVERY Drive
/// or Home visit - one viewer launch per such car, each time. Forced builds
/// retry; a restart forgets the list (a game update may fix them).
static FAILED: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);

fn failed(out: &Path) -> bool {
    FAILED.lock().unwrap().as_ref().map(|f| f.contains(out)).unwrap_or(false)
}

fn mark_failed(out: &Path, yes: bool) {
    let mut g = FAILED.lock().unwrap();
    let f = g.get_or_insert_with(HashSet::new);
    if yes { f.insert(out.to_path_buf()); } else { f.remove(out); }
}

pub fn render_car(car_id: &str, force: bool, make: bool, big: bool, visual: &str, paint: &str) -> Option<PathBuf> {
    if car_id.is_empty() {
        return None;
    }
    let out = car_path(car_id, big, visual, paint);
    if out.is_file() && !force {
        return Some(out);
    }
    if !make || (!force && failed(&out)) {
        return None;
    }
    let r = render_car_now(car_id, big, visual, paint, &out);
    mark_failed(&out, r.is_none());
    if r.is_some() {
        let src = source_fp(car_id, &mut HashMap::new());
        set_record(&out, Some(json!({"renderer": renderer_full_id(), "source": src})));
    }
    r
}

fn render_car_now(car_id: &str, big: bool, visual: &str, paint: &str, out: &Path) -> Option<PathBuf> {
    let out = out.to_path_buf();
    let exe = crate::viewer::viewer_exe()?;
    let pkg = crate::viewer::package_for(car_id)?;
    let (w, h) = if big { BIG } else { SIZE };
    let raw = out.with_extension("x2.png");
    let mut c = std::process::Command::new(&exe);
    c.arg(&pkg).args(["--car", car_id, "--shot"]).arg(&raw)
        .args(["--size", &format!("{}x{}", w * SUPERSAMPLE, h * SUPERSAMPLE), "--yaw", "2.35", "--pitch", "0.17", "--zoom", "1.35"]);
    // a mod package holds only its own car; the base game supplies tyres
    if let Some(base) = crate::viewer::package() {
        if std::path::absolute(&base).ok() != std::path::absolute(&pkg).ok() {
            c.args(["--base", &base]);
        }
    }
    // which trim and colour: otherwise every trim renders as the first one
    if !visual.is_empty() {
        c.env("EVOVIEW_VISUAL", visual);
    }
    if !paint.is_empty() {
        c.env("EVOVIEW_PAINT", paint);
    }
    c.current_dir(paths::data()).stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    crate::procs::hide(&mut c);
    let child = c.spawn().ok()?;
    let pid = child.id();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || { let _ = tx.send(child.wait_with_output()); });
    let o = match rx.recv_timeout(std::time::Duration::from_secs(180)) {
        Ok(Ok(o)) => o,
        _ => {
            crate::procs::kill(pid);
            crate::logs::warn(format!("thumb {car_id}: timed out"));
            return None;
        }
    };
    if !o.status.success() || !raw.is_file() {
        let _ = std::fs::remove_file(&raw);
        let t = if o.stdout.is_empty() { String::from_utf8_lossy(&o.stderr).into_owned() } else { String::from_utf8_lossy(&o.stdout).into_owned() };
        let tail: String = t.chars().rev().take(200).collect::<Vec<_>>().into_iter().rev().collect();
        crate::logs::warn(format!("thumb {car_id}: exit {:?} {tail}", o.status.code()));
        return None;
    }
    let r = downsample(&raw, &out);
    let _ = std::fs::remove_file(&raw);
    if let Err(e) = r {
        crate::logs::warn(format!("thumb {car_id}: {e}"));
        return None;
    }
    Some(out)
}

/// (preset id, label, model, visual, paint) for everything the UI shows,
/// one per distinct LOOK.
fn render_jobs() -> Vec<(String, String, String, String, String)> {
    let mut jobs = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for c in crate::content::cars().get("cars").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
        let pid = c.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if pid.is_empty() {
            continue;
        }
        let (model, visual, paint) = preset_look(&pid);
        if model.is_empty() || !seen.insert((model.clone(), visual.clone(), paint.clone())) {
            continue;
        }
        let label = c.get("label").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).unwrap_or(&pid).to_string();
        jobs.push((pid, label, model, visual, paint));
    }
    if !jobs.is_empty() {
        return jobs;
    }
    crate::viewer::index(false).get("cars").and_then(|v| v.as_array()).cloned().unwrap_or_default().iter()
        .filter_map(|c| c.get("id").and_then(|v| v.as_str()).map(|id| {
            let l = c.get("label").and_then(|v| v.as_str()).unwrap_or(id).to_string();
            (id.to_string(), l, id.to_string(), String::new(), String::new())
        })).collect()
}

/// Render every car in the background; poll job(). `force` redraws all of
/// them; otherwise only pictures that are missing or out of date.
pub fn build_all(force: bool) -> Value {
    build_pass(force)
}

// ------------------------------------------------------- picture records --
// ⚠ The render cache used to ask only "is there a file": pictures from an
// old viewer (flat paint, missing decals, cars that did not render at all)
// stayed for good while the viewer improved, and a game or mod update never
// redrew the car it changed. Every picture now carries what it was drawn
// FROM - the renderer and the car's package - and is redrawn by itself
// whenever either changes. Nobody has to delete a folder by hand.

/// Bump when render_car's framing (size, yaw, pitch, zoom) changes: every
/// picture is then redrawn on the next pass.
const RECIPE: &str = "v2 900x600 1440x960 yaw2.35 pitch0.17 zoom1.35 ss2";

fn records_path() -> PathBuf {
    dir("cars").join("sources.json")
}

/// One shared copy (a pass and on-demand renders both write it).
static RECORDS: Mutex<Option<Map<String, Value>>> = Mutex::new(None);

fn load_records() -> Map<String, Value> {
    let mut g = RECORDS.lock().unwrap();
    g.get_or_insert_with(|| std::fs::read(records_path()).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.as_object().cloned()).unwrap_or_default()).clone()
}

/// Goes up whenever a car picture is drawn; the UI puts it in picture URLs.
/// ⚠ The webview keeps an image for the whole page under the same URL
/// (max-age does not matter), so a car redrawn by a pass or a garage paint
/// change kept its OLD picture on the Drive card for as long as the app ran.
/// Starts at the launch time so a restarted app never repeats a value.
static PIC_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn pic_gen() -> u64 {
    use std::sync::atomic::Ordering::SeqCst;
    if PIC_GEN.load(SeqCst) == 0 {
        let t = std::time::SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(1);
        let _ = PIC_GEN.compare_exchange(0, t, SeqCst, SeqCst);
    }
    PIC_GEN.load(SeqCst)
}

/// Remember what `p` was drawn from (None: forget it), and save.
fn set_record(p: &Path, v: Option<Value>) {
    pic_gen();
    PIC_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    load_records();
    let mut g = RECORDS.lock().unwrap();
    let m = g.get_or_insert_with(Map::new);
    match v {
        Some(v) => { m.insert(pic_name(p), v); }
        None => { m.remove(&pic_name(p)); }
    }
    let _ = crate::safefs::write_atomic(&records_path(), serde_json::to_string(&*m).unwrap_or_default().as_bytes());
}

/// The package a model's picture is drawn from, as "size:mtime" - a game
/// update replaces content.kspkg, a mod update its own .kspkg.
fn source_fp(model: &str, cache: &mut HashMap<String, String>) -> String {
    let Some(pkg) = crate::viewer::package_for(model) else { return String::new() };
    if let Some(v) = cache.get(&pkg) {
        return v.clone();
    }
    let fp = std::fs::metadata(&pkg).map(|m| {
        let t = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
        format!("{}:{t}", m.len())
    }).unwrap_or_default();
    cache.insert(pkg, fp.clone());
    fp
}

fn pic_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Is this picture missing, or drawn by another renderer / from another
/// version of its car?
fn out_of_date(p: &Path, rid: &str, src: &str, rec: &Map<String, Value>) -> bool {
    if !p.is_file() {
        return true;
    }
    match rec.get(&pic_name(p)) {
        // no record: drawn before records existed - by an older viewer
        None => true,
        Some(r) => r.get("renderer").and_then(|v| v.as_str()) != Some(rid)
            || r.get("source").and_then(|v| v.as_str()) != Some(src),
    }
}

fn renderer_full_id() -> String {
    let exe = renderer_id();
    if exe.is_empty() { String::new() } else { format!("{exe} {RECIPE}") }
}

fn build_pass(force: bool) -> Value {
    {
        let mut g = JOB.lock().unwrap();
        let m = g.get_or_insert_with(idle_job);
        if m.get("state").and_then(|v| v.as_str()) == Some("running") {
            return json!({"ok": false, "error": "already building"});
        }
        *m = idle_job();
        m.insert("state".into(), json!("running"));
    }
    if force {
        *FAILED.lock().unwrap() = None;
    }
    let rid = renderer_full_id();
    std::thread::spawn(move || {
        let jobs = render_jobs();
        job_set(&JOB, json!({"total": jobs.len()}));
        let t0 = std::time::Instant::now();
        let mut fps = HashMap::new();
        let mut made = 0;
        for (i, (pid, label, model, visual, paint)) in jobs.iter().enumerate() {
            job_set(&JOB, json!({"done": i + 1, "current": if label.is_empty() { pid } else { label }}));
            let p = car_path(model, false, visual, paint);
            let src = source_fp(model, &mut fps);
            let stale = force || out_of_date(&p, &rid, &src, &load_records());
            if !stale {
                continue;
            }
            // redraw over an existing picture (it stays up until replaced)
            if render_car(model, p.is_file(), true, false, visual, paint).is_none() {
                continue;
            }
            made += 1;
            job_set(&JOB, json!({"made": made}));
            // the model's other pictures from the old renderer go: the large
            // detail render is redrawn on demand, and the plain fallback is
            // superseded by this trim/paint picture
            for other in [car_path(model, true, visual, paint), car_path(model, false, "", "")] {
                if other != p && other.is_file() && out_of_date(&other, &rid, &src, &load_records()) {
                    let _ = std::fs::remove_file(&other);
                    set_record(&other, None);
                }
            }
        }
        job_set(&JOB, json!({"state": "done", "current": "", "seconds": (t0.elapsed().as_secs_f64() * 10.0).round_ties_even() / 10.0}));
    });
    json!({"ok": true})
}

/// Which viewer build draws the pictures: a hash of its exe, cached
/// against the file's size and mtime (hashing 8 MB once per change).
fn renderer_id() -> String {
    static C: Mutex<Option<(String, u64, u128, String)>> = Mutex::new(None);
    let Some(exe) = crate::viewer::viewer_exe() else { return String::new() };
    let Ok(m) = std::fs::metadata(&exe) else { return String::new() };
    let t = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0);
    if let Some((p, l, mt, id)) = C.lock().unwrap().as_ref() {
        if *p == exe && *l == m.len() && *mt == t {
            return id.clone();
        }
    }
    use sha2::Digest;
    let id = std::fs::read(&exe).map(|b| sha2::Sha256::digest(&b).iter().take(8).map(|x| format!("{x:02x}")).collect::<String>()).unwrap_or_default();
    *C.lock().unwrap() = Some((exe, m.len(), t, id.clone()));
    id
}

/// Start a background pass when any picture is missing or out of date: a
/// new viewer (ACECM update), a new render recipe, a game update or an
/// updated mod. Called when the Home and Play pages load; cheap when
/// everything is current (one stat per package).
pub fn ensure_renders() -> bool {
    if job_get(&JOB).get("state").and_then(|v| v.as_str()) == Some("running") {
        return false;
    }
    // ⚠ at most every 15 s: Home and Play both ask on every visit, and the
    // answer only changes when a viewer, game or mod update lands
    static LAST: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    {
        let mut g = LAST.lock().unwrap();
        if g.map(|t| t.elapsed().as_secs() < 15).unwrap_or(false) {
            return false;
        }
        *g = Some(std::time::Instant::now());
    }
    let rid = renderer_full_id();
    if rid.is_empty() {
        return false;
    }
    let rec = load_records();
    let mut fps = HashMap::new();
    let due = render_jobs().iter().any(|j| {
        let p = car_path(&j.2, false, &j.3, &j.4);
        !failed(&p) && out_of_date(&p, &rid, &source_fp(&j.2, &mut fps), &rec)
    });
    if due {
        crate::logs::info("thumbs: some car pictures are missing or out of date - redrawing them in the background");
        build_pass(false);
    }
    due
}

fn pngs(which: &str) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir(which)).into_iter().flatten().flatten()
        .filter_map(|e| { let n = e.file_name().to_string_lossy().into_owned(); n.strip_suffix(".png").map(String::from) }).collect();
    v.sort();
    v
}

pub fn have() -> Vec<String> {
    pngs("cars")
}

pub fn covers_have() -> Vec<String> {
    pngs("tracks")
}

// ----------------------------------------------------------------- covers --
static COVERS: Mutex<Option<HashMap<String, crate::kspkg::Entry>>> = Mutex::new(None);
static TEXTURES: Mutex<Option<HashMap<String, (crate::kspkg::Entry, crate::kspkg::Entry)>>> = Mutex::new(None);

fn track_folders() -> Vec<String> {
    let s: BTreeSet<String> = trackimport::track_map(false).values().filter_map(|v| v.as_str().map(String::from)).collect();
    s.into_iter().collect()
}

/// Track folder -> archive entry of a shipped PNG cover (file names only
/// START with the folder: brands_hatch_indy.png).
fn cover_index() -> HashMap<String, crate::kspkg::Entry> {
    if let Some(c) = COVERS.lock().unwrap().as_ref() {
        return c.clone();
    }
    let mut out = HashMap::new();
    if let Some(pkg) = crate::viewer::package() {
        let mut folders = track_folders();
        folders.sort_by(|a, b| b.len().cmp(&a.len()));
        for e in crate::kspkg::entries(Path::new(&pkg)).unwrap_or_default() {
            let low = e.path.to_lowercase();
            if e.size == 0 || !low.contains(r"\images\tracks\") || !low.ends_with(".png") {
                continue;
            }
            let stem = low.rsplit('\\').next().unwrap_or("").trim_end_matches(".png").to_string();
            if let Some(f) = folders.iter().find(|f| stem.starts_with(&f.to_lowercase())) {
                out.entry(f.clone()).or_insert(e);
            }
        }
    }
    *COVERS.lock().unwrap() = Some(out.clone());
    out
}

/// (name a file might use, folder), longest first: covers are named after
/// the DISPLAY name (circuit_de_spa_francorchamps-gp for `spa`).
fn aliases() -> Vec<(String, String)> {
    let mut out: BTreeSet<(String, String)> = BTreeSet::new();
    for (name, folder) in trackimport::track_map(false) {
        let Some(folder) = folder.as_str().map(String::from) else { continue };
        out.insert((folder.to_lowercase(), folder.clone()));
        let mut flat = String::new();
        let mut last_us = false;
        for ch in name.to_lowercase().chars() {
            if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
                flat.push(ch);
                last_us = false;
            } else if !last_us {
                flat.push('_');
                last_us = true;
            }
        }
        let flat = flat.trim_matches('_').to_string();
        if !flat.is_empty() {
            out.insert((flat.clone(), folder.clone()));
            out.insert((flat.split('_').next().unwrap_or("").to_string(), folder.clone()));
        }
    }
    let mut v: Vec<(String, String)> = out.into_iter().collect();
    // longest first; ties in python's set order are arbitrary, keep sorted
    v.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
    v
}

/// Track folder -> (.texture, .texturemips) of its cover photo.
fn texture_index() -> HashMap<String, (crate::kspkg::Entry, crate::kspkg::Entry)> {
    if let Some(t) = TEXTURES.lock().unwrap().as_ref() {
        return t.clone();
    }
    let mut out = HashMap::new();
    if let Some(pkg) = crate::viewer::package() {
        let al = aliases();
        let mut stems: std::collections::BTreeMap<String, (Option<crate::kspkg::Entry>, Option<crate::kspkg::Entry>)> = Default::default();
        let mut order: Vec<String> = Vec::new();
        for e in crate::kspkg::entries(Path::new(&pkg)).unwrap_or_default() {
            let low = e.path.to_lowercase();
            if e.size == 0 || !low.contains(r"\images\tracks\") {
                continue;
            }
            let (stem, is_h) = if let Some(s) = low.strip_suffix(".texture") { (s.to_string(), true) }
                else if let Some(s) = low.strip_suffix(".texturemips") { (s.to_string(), false) } else { continue };
            if !stems.contains_key(&stem) {
                order.push(stem.clone());
            }
            let slot = stems.entry(stem).or_default();
            if is_h { slot.0.get_or_insert(e); } else { slot.1.get_or_insert(e); }
        }
        for stem in order {
            let (Some(h), Some(m)) = stems[&stem].clone() else { continue };
            let name = stem.rsplit('\\').next().unwrap_or("").to_string();
            if let Some((_, folder)) = al.iter().find(|(a, _)| name.starts_with(a.as_str())) {
                out.entry(folder.clone()).or_insert((h, m));
            }
        }
    }
    *TEXTURES.lock().unwrap() = Some(out.clone());
    out
}

pub fn covers_available() -> Vec<String> {
    let mut s: BTreeSet<String> = cover_index().into_keys().collect();
    s.extend(texture_index().into_keys());
    s.into_iter().collect()
}

fn varint(d: &[u8], mut i: usize) -> Option<(u64, usize)> {
    let (mut r, mut s) = (0u64, 0u32);
    loop {
        let b = *d.get(i)?;
        i += 1;
        r |= ((b & 0x7f) as u64) << s;
        if b & 0x80 == 0 {
            return Some((r, i));
        }
        s += 7;
        if s > 63 {
            return None;
        }
    }
}

/// Top-level varint fields of a .texture header.
fn header_ints(d: &[u8]) -> HashMap<u64, u64> {
    let mut out = HashMap::new();
    let mut i = 0;
    while i < d.len() {
        let Some((k, ni)) = varint(d, i) else { break };
        i = ni;
        match k & 7 {
            0 => { let Some((v, ni)) = varint(d, i) else { break }; i = ni; out.entry(k >> 3).or_insert(v); }
            2 => { let Some((l, ni)) = varint(d, i) else { break }; i = ni + l as usize; }
            5 => i += 4,
            1 => i += 8,
            _ => break,
        }
    }
    out
}

const PAGE: usize = 64 * 1024;
const TILE: usize = 256;

/// Decode a track's BC7 cover to PNG, picking a mip near `want` wide.
/// ⚠ Only four tracks ship a .png; the rest ship the photo as BC7.
fn decode_cover(folder: &str, want: u64) -> Option<PathBuf> {
    let (he, me) = texture_index().get(folder).cloned()?;
    let pkg = crate::viewer::package()?;
    let r = (|| -> Result<PathBuf, String> {
        let mut f = std::fs::File::open(&pkg).map_err(|e| e.to_string())?;
        let hdr = crate::kspkg::read_entry(&mut f, &he).map_err(|e| e.to_string())?;
        // ⚠ .texturemips are stored UNENCRYPTED - read_entry knows
        let raw = crate::kspkg::read_entry(&mut f, &me).map_err(|e| e.to_string())?;
        let ints = header_ints(&hdr);
        let (w, h, nmips, fmt) = (*ints.get(&1).unwrap_or(&0), *ints.get(&2).unwrap_or(&0), *ints.get(&3).unwrap_or(&0), *ints.get(&4).unwrap_or(&0));
        if !(33..=35).contains(&fmt) || w == 0 {
            return Err("not BC7".into());
        }
        let mut off = 0usize;
        let mut pick = None;
        for level in 0..nmips.max(1) {
            let (mw, mh) = ((w >> level).max(1), (h >> level).max(1));
            let cols = (mw as usize).div_ceil(TILE).max(1);
            let rows = (mh as usize).div_ceil(TILE).max(1);
            let size = cols * rows * PAGE;
            if off + size > raw.len() {
                return Err("short mip chain".into());
            }
            if mw <= want || level == nmips.saturating_sub(1) {
                pick = Some((level, mw as usize, mh as usize, cols, rows, off));
                break;
            }
            off += size;
        }
        let (level, mw, mh, cols, rows, off) = pick.ok_or("no mip")?;
        // one tile per page, padded to the full tile in BOTH directions
        let (iw, _ih) = (cols * TILE, rows * TILE);
        let mut rgb = vec![0u8; mw * mh * 3];
        let mut px = vec![0u32; TILE * TILE];
        for i in 0..cols * rows {
            let blob = &raw[off + i * PAGE..off + (i + 1) * PAGE];
            texture2ddecoder::decode_bc7(blob, TILE, TILE, &mut px).map_err(|e| e.to_string())?;
            let (tx, ty) = ((i % cols) * TILE, (i / cols) * TILE);
            for y in 0..TILE {
                let gy = ty + y;
                if gy >= mh {
                    break;
                }
                for x in 0..TILE {
                    let gx = tx + x;
                    if gx >= mw {
                        break;
                    }
                    let p = px[y * TILE + x];
                    let o = (gy * mw + gx) * 3;
                    rgb[o] = (p >> 16) as u8;
                    rgb[o + 1] = (p >> 8) as u8;
                    rgb[o + 2] = p as u8;
                }
            }
        }
        let _ = iw;
        let dest = track_path(folder);
        let tmp = dest.with_extension("png.part");
        {
            let file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
            let mut enc = png::Encoder::new(std::io::BufWriter::new(file), mw as u32, mh as u32);
            enc.set_color(png::ColorType::Rgb);
            enc.set_depth(png::BitDepth::Eight);
            enc.set_compression(png::Compression::Best);
            let mut wr = enc.write_header().map_err(|e| e.to_string())?;
            wr.write_image_data(&rgb).map_err(|e| e.to_string())?;
        }
        std::fs::rename(&tmp, &dest).map_err(|e| e.to_string())?;
        crate::logs::info(format!("decoded cover for {folder} ({mw}x{mh} from mip {level})"));
        Ok(dest)
    })();
    match r {
        Ok(p) => Some(p),
        Err(e) => {
            if e != "not BC7" {
                crate::logs::warn(format!("decode cover {folder}: {e}"));
            }
            None
        }
    }
}

/// Path to a cached cover PNG, extracting or decoding it if the game ships
/// one; None when it ships none.
pub fn track_cover(folder: &str) -> Option<PathBuf> {
    let folder = resolve_folder(folder);
    let out = track_path(&folder);
    if out.is_file() {
        return Some(out);
    }
    if let Some(e) = cover_index().get(&folder).cloned() {
        if let Some(pkg) = crate::viewer::package() {
            match std::fs::File::open(&pkg).and_then(|mut f| crate::kspkg::read_entry(&mut f, &e)) {
                Ok(b) => {
                    if std::fs::write(&out, b).is_ok() {
                        return Some(out);
                    }
                }
                Err(ex) => crate::logs::warn(format!("cover {folder}: {ex}")),
            }
        }
    }
    decode_cover(&folder, 640)
}

/// Decode every track cover in the background; poll cover_job().
pub fn build_covers(force: bool) -> Value {
    {
        let mut g = COVER_JOB.lock().unwrap();
        let m = g.get_or_insert_with(idle_job);
        if m.get("state").and_then(|v| v.as_str()) == Some("running") {
            return json!({"ok": false, "error": "already decoding"});
        }
        *m = idle_job();
        m.insert("state".into(), json!("running"));
    }
    std::thread::spawn(move || {
        let folders = track_folders();
        job_set(&COVER_JOB, json!({"total": folders.len()}));
        let t0 = std::time::Instant::now();
        let mut made = 0;
        for (i, f) in folders.iter().enumerate() {
            job_set(&COVER_JOB, json!({"done": i + 1, "current": f}));
            if force {
                let _ = std::fs::remove_file(track_path(f));
            }
            if track_cover(f).is_some() {
                made += 1;
                job_set(&COVER_JOB, json!({"made": made}));
            }
        }
        job_set(&COVER_JOB, json!({"state": "done", "current": "", "seconds": (t0.elapsed().as_secs_f64() * 10.0).round_ties_even() / 10.0}));
    });
    json!({"ok": true})
}

/// /api/thumbs/status
pub fn status() -> Value {
    let mut out = job_get(&JOB);
    out.insert("have".into(), json!(have()));
    out.insert("covers".into(), json!(covers_available()));
    out.insert("covers_have".into(), json!(covers_have()));
    out.insert("cover_job".into(), cover_job());
    Value::Object(out)
}

/// What the car thumb route serves: list rows read the cache; `big` (the
/// one car the user opened) and `make` (the Drive card) DO render.
pub fn car_for_request(id: &str, preset: &str, big: bool, make: bool, force: bool) -> Option<PathBuf> {
    let (mut cid, mut visual, mut paint) = (id.to_string(), String::new(), String::new());
    if !preset.is_empty() {
        let (m, v, p) = preset_look(preset);
        if cid.is_empty() {
            cid = m;
        }
        visual = v;
        paint = p;
    }
    if big {
        return render_car(&cid, force, true, true, &visual, &paint)
            .or_else(|| render_car(&cid, false, false, false, &visual, &paint))
            .or_else(|| render_car(&cid, false, false, false, "", ""));
    }
    let shot = render_car(&cid, force, make, false, &visual, &paint);
    if shot.is_none() && (!visual.is_empty() || !paint.is_empty()) {
        return render_car(&cid, false, false, false, "", "");
    }
    shot
}
