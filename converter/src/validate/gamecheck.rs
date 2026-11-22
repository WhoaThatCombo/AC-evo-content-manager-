//! `aceconv game-check`: predict, from the files alone, what makes a
//! conversion fail in the real game although it looks fine in evoview.
//!
//! Every rule names the game code (AssettoCorsaEVO.exe, build of
//! 2026-09-07/17, addresses are VAs) or the in-game incident it comes from,
//! and the way the game reacts. The rules are calibrated on stock content:
//! `aceconv game-check --stock` runs the content rules on every stock car
//! and track of content.kspkg and must report nothing.
//!
//! What the game does that evoview does not:
//! - it finds a packaged file by the record id, FNV-1a-64 of the lower-cased
//!   UTF-16 path (server FUN_14128b920 binary-searches content.kspkg; mod
//!   packages are found by id too but need not be sorted - 32 installed
//!   third-party packages have unsorted tables and load): a record whose id
//!   is not that hash is a file the game cannot find ("Failed to find file:
//!   {}", thrown at six loader sites), and two records with one id (paths that differ only in case)
//!   are one file to the game; evoview reads every record by its path;
//! - `.texturemips` are streamed straight to the GPU and must be stored
//!   plaintext (record kind 0): an encrypted one throws "Reading directly to
//!   GPU an encrypted file is not supported" (crash, seen in game);
//! - loose files and later archives fill ABSENCES only: a path content.kspkg
//!   already holds is read from content.kspkg (seen in game: a loose
//!   uicomponents.css changed nothing);
//! - a material's shader is looked up in the renderer's shader registry
//!   (0x141ccbaa0 "Shader {} NOT FOUND, RETURNING NULL"); the bindless
//!   material constructor (0x141e6dbb0) then throws "Bindless material type
//!   not found";
//! - the track collider loader (0x1414aa960) resolves each SMesh (kind 100)
//!   and ISMesh (kind 105) surface name against the loaded Surface Defs and
//!   throws "surface id not found" / "Mesh surface id doesn't exist"; a
//!   Surface Def collisionCategory outside 0..4 throws "Wrong/unmapped
//!   collider category" (0x1414da820, 0x140ba1f00);
//! - the remote game mode (0x141910990) counts the spawn points of every
//!   type in the session's containers and throws "No starting position!" on
//!   none (seen in game: ks_black_cat_county layout_short) - `decl::check`;
//! - a scene without a Track Info entity is run with "defaulting to
//!   Nurburgring settings" (weather / location / time zone of another track);
//! - the drivable-camera loader (0x141bcda40) wants exactly six drivable
//!   cameras ("Car: {} has an invalid number of Drivable cameras");
//! - the car's mechanical preset named by the mod's `<id>.json` is loaded by
//!   path (0x141781fe0 "mechanical car preset data not found {}");
//! - Saved Games\ACE\CarCameras / CarMirrors keep per-id settings the game
//!   re-applies over a reinstalled actor (`car::usersettings`).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Result;

use super::{entities, entity_kind, Game, Report};
use crate::io::kspkg::{entry_id, xor};
use crate::io::pb::{Msg, Val};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Sev {
    /// the game crashes, throws, or never shows the content
    Fail,
    /// it loads, but wrong or fragile
    Risk,
}

#[derive(Clone, Debug)]
pub struct Finding {
    pub sev: Sev,
    pub check: &'static str,
    pub file: String,
    pub reason: String,
    pub reaction: String,
}

#[derive(Default, Debug)]
pub struct Outcome {
    pub findings: Vec<Finding>,
    /// what was looked at (one line per car / track)
    pub checked: Vec<String>,
}

impl Outcome {
    fn add(&mut self, sev: Sev, check: &'static str, file: impl Into<String>, reason: impl Into<String>, reaction: impl Into<String>) {
        self.findings.push(Finding { sev, check, file: file.into(), reason: reason.into(), reaction: reaction.into() });
    }
    pub fn fails(&self) -> usize {
        self.findings.iter().filter(|f| f.sev == Sev::Fail).count()
    }
    pub fn print(&self) {
        for c in &self.checked {
            println!("checked {c}");
        }
        let mut f: Vec<&Finding> = self.findings.iter().collect();
        f.sort_by(|a, b| (a.sev, a.check, &a.file).cmp(&(b.sev, b.check, &b.file)));
        for x in f {
            println!("{} [{}] {}\n    why:  {}\n    game: {}", if x.sev == Sev::Fail { "FAIL" } else { "RISK" }, x.check, x.file, x.reason, x.reaction);
        }
        println!("{} predicted failure(s), {} risk(s)", self.fails(), self.findings.len() - self.fails());
    }
}

const MISSING: &str = "\"Failed to find file: {}\" is thrown while loading - crash to desktop";

/// The protobuf files the game parses (calibrated: every stock one parses).
const PB_EXTS: [&str; 16] = [
    ".actor", ".car", ".mesh", ".material", ".texture", ".scene", ".track", ".carsetup", ".mechanicalcarpreset", ".visualcarpreset", ".carfinalstate", ".moddedcarcontent",
    ".carengine", ".gearbox", ".suspension", ".tyre",
];

/// Physics files whose floats must all be finite.
/// (.aicardata is left out: stock ks_ferrari_488_challenge_evo's holds a NaN
/// at f5.f10 and the car runs - AI hints, not the physics step.)
const PHYS_EXTS: [&str; 14] = [
    ".car", ".carengine", ".gearbox", ".drivetrain", ".clutch", ".brakesystem", ".suspension", ".coilover", ".wing", ".turbo", ".tyre", ".carsetup", ".carsetuplimits",
    ".mechanicalcarpreset",
];

fn ext_in(p: &str, exts: &[&str]) -> bool {
    let l = p.to_lowercase();
    exts.iter().any(|e| l.ends_with(e))
}

// ---------------------------------------------------------------- package table

/// One raw table record as the game sees it.
#[derive(Clone, Debug)]
pub struct Record {
    pub path: String,
    pub kind: u16,
    pub path_len: u16,
    pub id: u64,
    pub size: u64,
    pub off: u64,
}

const BUCKET: usize = 256;
const TABLE: usize = 64 * 1024 * 1024;

/// Every used record of a package, in table order, plus header problems
/// (records with data but no name).
pub fn records(pkg: &Path) -> Result<(Vec<Record>, usize, u64)> {
    let f = std::fs::File::open(pkg)?;
    let map = unsafe { memmap2::Mmap::map(&f)? };
    anyhow::ensure!(map.len() >= TABLE, "{} is smaller than its table", pkg.display());
    let mut table = map[map.len() - TABLE..].to_vec();
    xor(&mut table);
    let mut out = Vec::new();
    let mut nameless = 0;
    for r in table.chunks_exact(BUCKET) {
        let end = r[..228].iter().position(|&b| b == 0).unwrap_or(228);
        if end == 0 {
            if r[228..].iter().any(|&b| b != 0) {
                nameless += 1;
            }
            continue;
        }
        out.push(Record {
            path: String::from_utf8_lossy(&r[..end]).into_owned(),
            kind: u16::from_le_bytes([r[228], r[229]]),
            path_len: u16::from_le_bytes([r[230], r[231]]),
            id: u64::from_le_bytes(r[232..240].try_into().unwrap()),
            size: u32::from_le_bytes(r[240..244].try_into().unwrap()) as u64,
            off: u64::from_le_bytes(r[248..256].try_into().unwrap()),
        });
    }
    Ok((out, nameless, (map.len() - TABLE) as u64))
}

/// The table rules: ids, order, kinds, ranges, plaintext mips.
/// `archive`: content.kspkg (binary searched, so it must be sorted). Mod
/// packages are not required to be: 32 installed third-party car packages
/// have unsorted tables and load in game.
pub fn check_table(name: &str, recs: &[Record], nameless: usize, data_end: u64, archive: bool, o: &mut Outcome) {
    if nameless > 0 {
        o.add(Sev::Fail, "pkg-header", name, format!("{nameless} record(s) with a size / offset / id but no path (a zeroed header)"),
            "the loader skips them - their files are \"Failed to find file\" (seen in game: the alabama_r_4e insert)");
    }
    let mut bad_id = Vec::new();
    let mut prev: Option<u64> = None;
    let mut unsorted = 0;
    let mut seen: HashMap<u64, &str> = HashMap::new();
    let mut dups: Vec<(String, String)> = Vec::new();
    for r in recs {
        if r.id != entry_id(&r.path) {
            bad_id.push(r.path.as_str());
        }
        if prev.map(|p| r.id < p).unwrap_or(false) {
            unsorted += 1;
        }
        prev = Some(r.id);
        if let Some(o2) = seen.insert(r.id, &r.path) {
            dups.push((o2.to_string(), r.path.clone()));
        }
        if r.path_len as usize != r.path.chars().count() {
            o.add(Sev::Fail, "pkg-header", format!("{name}: {}", r.path), format!("record path length {} for a {}-char path", r.path_len, r.path.chars().count()), "the path the game compares is cut or overrun - file not found");
        }
        match r.kind {
            1 => {}
            0 if r.path.to_lowercase().ends_with(".texturemips") => {}
            // (stock ks_abarth_695_biposto ships an encrypted, stem-less
            // generated\thumbnails\.texturemips: no texture names it)
            0x100 if r.path.to_lowercase().ends_with(".texturemips") && !r.path.ends_with("\\.texturemips") => o.add(Sev::Fail, "mips-encrypted", format!("{name}: {}", r.path),
                "a .texturemips stored encrypted (record kind 0x100); stock stores all 38799 plaintext (kind 0)",
                "\"Reading directly to GPU an encrypted file is not supported\" - crash when the texture streams in (seen in game)"),
            0x100 => {}
            0 => o.add(Sev::Risk, "pkg-plain", format!("{name}: {}", r.path), "a non-mips file stored plaintext (kind 0); stock encrypts every one",
                "read raw while every other file is decrypted - only the mips path is known to work this way"),
            k => o.add(Sev::Fail, "pkg-header", format!("{name}: {}", r.path), format!("record kind {k:#x} (stock: 0 mips, 0x100 file, 1 folder)"), "unknown kind - the loader skips or misreads the file"),
        }
        if r.kind != 1 && r.off.saturating_add(r.size) > data_end {
            o.add(Sev::Fail, "pkg-range", format!("{name}: {}", r.path), format!("data {}..{} runs past the data region ({data_end} B)", r.off, r.off + r.size), "reads table bytes or past the file - garbage / crash");
        }
    }
    for (a, b) in dups {
        let how = if a.eq_ignore_ascii_case(&b) { "paths equal but for case (the id hashes the lower-cased path)" } else { "an FNV collision" };
        o.add(Sev::Fail, "pkg-dup", name, format!("two records share one id, {how}: {a} and {b}"),
            "the binary search lands on one of them; whatever references the other gets the first's bytes (or a mips / header mismatch)");
    }
    if !bad_id.is_empty() {
        let mixed = bad_id.iter().filter(|p| entry_id(&p.to_lowercase()) != entry_id(p) || p.chars().any(|c| c.is_uppercase())).count();
        o.add(Sev::Fail, "pkg-id", name, format!("{} record id(s) are not FNV-1a-64 of the lower-cased UTF-16 path ({mixed} mixed-case), e.g. {}", bad_id.len(), bad_id.iter().take(3).cloned().collect::<Vec<_>>().join(", ")),
            format!("{MISSING} (seen in game: a mixed-case id)"));
    }
    if unsorted > 0 && archive {
        o.add(Sev::Fail, "pkg-sort", name, format!("{unsorted} record(s) out of id order"), format!("the binary search misses entries past the break - {MISSING}"));
    }
}

/// Every protobuf file of a package parses after the decoding its record
/// kind implies.
fn check_decode(name: &str, files: &BTreeMap<String, Vec<u8>>, o: &mut Outcome) {
    for (p, b) in files {
        if ext_in(p, &PB_EXTS) && !b.is_empty() && Msg::parse(b).is_err() {
            o.add(Sev::Fail, "pkg-decode", format!("{name}: {p}"), "does not parse as protobuf once decoded (wrong key phase / double XOR / truncated)", "the loader reads garbage - exception or crash at load");
        }
    }
}

// ---------------------------------------------------------------- content rules

/// Shaders the game's registry is known to hold: every shader a stock
/// material names.
pub fn stock_shaders(game: &Game) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for e in &game.pkg.entries {
        if !e.path.to_lowercase().ends_with(".material") {
            continue;
        }
        if let Some(s) = game.pkg.read(e).ok().and_then(|b| Msg::parse(&b).ok()).and_then(|m| m.str(1)) {
            out.insert(s);
        }
    }
    out
}

/// Materials: a shader the game has, no editor stub.
fn check_materials(files: &BTreeMap<String, Vec<u8>>, shaders: &BTreeSet<String>, o: &mut Outcome) {
    let mut unknown: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for (p, b) in files {
        let l = p.to_lowercase();
        if l.ends_with(".material") {
            match Msg::parse(b).ok().and_then(|m| m.str(1)) {
                Some(s) if shaders.contains(&s) => {}
                Some(s) => unknown.entry(s).or_default().push(p),
                None => o.add(Sev::Fail, "shader", p.as_str(), "the material names no shader (f1)", "\"Bindless material type not found\" (0x141e6dbb0) - crash"),
            }
        } else if l.ends_with(".mesh") && !l.contains("collider") && memchr::memmem::find(b, b"editor/default.material").is_some() {
            // (every stock collider mesh carries the stub; no drawn mesh does)
            o.add(Sev::Fail, "stub-material", p.as_str(), "a section uses the editor stub editor/default.material", "the car renderer builds no batch for it: \"... missing car renderer cache\" - crash (seen: the RX-8 build)");
        }
    }
    for (s, ps) in unknown {
        o.add(Sev::Fail, "shader", ps[0], format!("shader {s:?} is used by no stock material ({} material(s))", ps.len()),
            "\"Shader {} NOT FOUND, RETURNING NULL\" (0x141ccbaa0), then \"Bindless material type not found\" (0x141e6dbb0) - crash");
    }
}

/// Every float a physics file holds is finite (a NaN / inf spreads through
/// the ODE step: the car explodes or the session crashes).
fn check_finite(files: &BTreeMap<String, Vec<u8>>, o: &mut Outcome) {
    fn walk(m: &Msg, path: &mut Vec<u32>, bad: &mut Vec<String>, depth: usize) {
        for f in &m.fields {
            match &f.val {
                Val::F32(v) => {
                    if !f32::from_le_bytes(*v).is_finite() {
                        bad.push(path.iter().chain([&f.num]).map(|n| n.to_string()).collect::<Vec<_>>().join("."));
                    }
                }
                Val::Bytes(b) if depth < 16 && !b.is_empty() && !b.iter().all(|c| (32..127).contains(c)) => {
                    if let Ok(s) = Msg::parse(b) {
                        path.push(f.num);
                        walk(&s, path, bad, depth + 1);
                        path.pop();
                    }
                }
                Val::Msg(s) if depth < 16 => {
                    path.push(f.num);
                    walk(s, path, bad, depth + 1);
                    path.pop();
                }
                _ => {}
            }
        }
    }
    for (p, b) in files {
        if !ext_in(p, &PHYS_EXTS) {
            continue;
        }
        let Ok(m) = Msg::parse(b) else { continue };
        let mut bad = Vec::new();
        walk(&m, &mut Vec::new(), &mut bad, 0);
        if !bad.is_empty() {
            o.add(Sev::Fail, "phys-nan", p.as_str(), format!("{} non-finite float(s) at field {}", bad.len(), bad.iter().take(4).cloned().collect::<Vec<_>>().join(", ")),
                "NaN / inf enters the physics step - the car vanishes or the session crashes (seen in game: the RX-8 steer-ratio NaN)");
        }
    }
}

/// The car's drivable cameras: exactly six.
fn check_cameras(files: &BTreeMap<String, Vec<u8>>, id: &str, o: &mut Outcome) {
    let get = |p: &str| files.iter().find(|(k, _)| k.eq_ignore_ascii_case(p)).map(|(_, v)| v);
    let ap = format!("content\\cars\\{id}\\{id}.actor");
    let Some(car) = get(&ap).and_then(|b| Msg::parse(b).ok()).and_then(|a| a.sub(103)) else { return };
    let n = car.subs(10).len();
    if n != 6 {
        o.add(Sev::Fail, "cameras", ap, format!("{n} drivable cameras (f103.f10); stock cars have 6"),
            "\"Car: {} has an invalid number of Drivable cameras\" (0x141bcda40) - camera cycling indexes past the list");
    }
}

/// Car-content rules on a files map (`content\cars\<id>\...`), shared by
/// converted packages and the stock calibration.
pub fn car_content(files: &BTreeMap<String, Vec<u8>>, id: &str, shaders: &BTreeSet<String>, o: &mut Outcome) {
    check_materials(files, shaders, o);
    check_finite(files, o);
    check_cameras(files, id, o);
}

// ---------------------------------------------------------------- tracks

/// Track-content rules on a files map (`content\tracks\<id>\...` plus any
/// shared scene the track references).
pub fn track_content(files: &BTreeMap<String, Vec<u8>>, id: &str, read: &dyn Fn(&str) -> Option<Vec<u8>>, o: &mut Outcome) {
    let prefix = format!("content\\tracks\\{id}\\").to_lowercase();
    let main_path = format!("{prefix}{id}.scene");
    let get = |p: &str| files.iter().find(|(k, _)| k.to_lowercase() == p.to_lowercase()).map(|(_, v)| v.clone()).or_else(|| read(p));
    let Some(main) = get(&main_path) else { return };
    // scenes of the track: the main one and every container it lists,
    // containers of containers too (the Nordschleife nests them)
    let mut scenes: Vec<(String, Vec<u8>)> = vec![(main_path.clone(), main.clone())];
    let mut seen: HashSet<String> = [main_path.clone()].into();
    let mut i = 0;
    while i < scenes.len() {
        let mut next = Vec::new();
        for e in entities(&scenes[i].1) {
            if entity_kind(&e).as_deref() == Some("Container") {
                if let Some(p) = e.sub(50).and_then(|d| d.sub(121)).and_then(|c| c.str(1)) {
                    if seen.insert(p.to_lowercase()) {
                        if let Some(b) = get(&p) {
                            next.push((p, b));
                        }
                    }
                }
            }
        }
        scenes.extend(next);
        i += 1;
    }
    let mut surfaces: HashSet<String> = HashSet::new();
    let mut uses: Vec<(String, String, String)> = Vec::new(); // (scene, entity, surface)
    let mut track_info = false;
    for (sp, b) in &scenes {
        for e in entities(b) {
            let kind = entity_kind(&e).unwrap_or_default();
            let Some(d) = e.sub(50) else { continue };
            if let Some(sd) = d.sub(109) {
                for s in sd.subs(1) {
                    if let Some(sid) = s.str(1) {
                        // the lookup ignores case (stock: Mount Panorama's "WALL"
                        // mesh on its "wall" def, the Red Bull Ring's "grass" on "GRASS")
                        surfaces.insert(sid.to_lowercase());
                        let cat = s.varint(2).unwrap_or(0);
                        if cat > 4 {
                            o.add(Sev::Fail, "surface-category", sp.as_str(), format!("Surface Def {sid:?} has collisionCategory {cat} (the game maps 0..4)"),
                                "\"Wrong/unmapped collider category\" thrown (0x1414da820) - crash while loading the track");
                        }
                    }
                }
            }
            if d.get(111).is_some() {
                track_info = true;
            }
            for k in [100u32, 105] {
                if let Some(m) = d.sub(k) {
                    if let Some(s) = m.str(2).filter(|s| !s.is_empty()) {
                        uses.push((sp.clone(), format!("{kind} {:?}", e.str(1).unwrap_or_default()), s));
                    }
                }
            }
        }
    }
    let mut missing: BTreeMap<String, (String, String, usize)> = BTreeMap::new();
    for (sp, ent, s) in uses {
        if !surfaces.contains(&s.to_lowercase()) {
            missing.entry(s).or_insert((sp, ent, 0)).2 += 1;
        }
    }
    for (s, (sp, ent, n)) in missing {
        o.add(Sev::Fail, "surface-id", sp, format!("{ent} (and {} more) names surface {s:?}, which no Surface Def entity of the track defines", n - 1),
            "\"surface id not found\" / \"Mesh surface id doesn't exist\" thrown by the collider loader (0x1414aa960) - crash while loading the track");
    }
    if !track_info {
        o.add(Sev::Risk, "track-info", main_path, "no Track Info entity (f50.f111) in the main scene or its containers",
            "\"{}.scene has no SceneTrackInfo ... defaulting to Nurburgring settings\" - weather, location and time of another track");
    }
}

// ---------------------------------------------------------------- the validator's own rules

/// The reaction of a validator message, by its wording. Only the wordings
/// of load failures are FAIL; the rest (looks, UI, AI) are RISK.
fn reaction_of(msg: &str) -> (&'static str, Sev, String) {
    let m = msg.to_lowercase();
    let looks = ["brighter", "white roads", "never samples", "flicker", "below their class", "culls", "shadows pop", "albedo"];
    if looks.iter().any(|k| m.contains(k)) {
        return ("looks", Sev::Risk, "loads; renders wrong (white / flat / flickering surfaces)".into());
    }
    let table: [(&str, &'static str, &str); 14] = [
        ("no starting position", "spawns", "\"No starting position!\" thrown by the remote game mode (0x141910990) - crash on session start (seen: ks_black_cat_county)"),
        ("missing", "missing-file", MISSING),
        ("does not exist", "missing-file", MISSING),
        ("renderer cache", "render-cache", "\"... missing car renderer cache\" thrown by the car renderer - crash when the car is shown"),
        ("ext_rim", "wheel-batches", "a wheel batch slot is dereferenced without a null check - crash when the car is shown (seen: EXT_RIM_BLUR_STATIC)"),
        ("ext_tyre", "wheel-batches", "a wheel batch slot is dereferenced without a null check - crash when the car is shown"),
        ("audio slot", "audio", "the FMOD event is not found - a silent car"),
        ("brand:", "brand", "UI only: wrong / missing brand tile or description"),
        ("cameras:", "cameras", "the game uses other cameras than the package's"),
        ("physics", "physics", "the car drives wrong or the physics step misbehaves"),
        ("container", "containers", "the game loads no / the wrong containers for the session (empty track, no road, no limits)"),
        ("spline", "splines", "\"scene_actors collection doesn't contain track_limits_left splines\" - no track limits / AI line"),
        ("spawn", "spawns", "the car is placed wrong or the session cannot start"),
        ("row", "decl", "the session's (layout, mode) lookup finds the wrong / no row"),
    ];
    let fail = ["spawns", "missing-file", "render-cache", "wheel-batches"];
    for (k, c, r) in table {
        if m.contains(k) {
            return (c, if fail.contains(&c) { Sev::Fail } else { Sev::Risk }, r.to_string());
        }
    }
    let crash = ["does not parse", "not a protobuf", "non-finite", "no lod", "no vertices", "index ", "no spawn points", "no pit spawn", "no collision"];
    if crash.iter().any(|k| m.contains(k)) {
        return ("validator", Sev::Fail, "the loader reads invalid data - exception or crash at load".into());
    }
    ("validator", Sev::Risk, "converter rule: loads, but not like stock (see the reason)".to_string())
}

fn relay(r: &Report, file: &str, o: &mut Outcome) {
    for e in &r.errors {
        let (c, s, x) = reaction_of(e);
        o.add(s, c, file, e.clone(), x);
    }
}

// ---------------------------------------------------------------- entry points

/// Context the install-level rules need.
pub struct Ctx<'a> {
    pub game: &'a Game,
    pub shaders: BTreeSet<String>,
    /// Saved Games\ACE (per-car camera / mirror settings), if any
    pub saved: Option<PathBuf>,
    /// run the converter's own validator too (`validate::car` / `track`)
    pub full: bool,
}

/// A car package (`<id>.kspkg`, its `<id>.json` beside it).
pub fn car_package(kspkg: &Path, ctx: &Ctx, o: &mut Outcome) -> Result<()> {
    let name = kspkg.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    let (recs, nameless, data_end) = records(kspkg)?;
    check_table(&name, &recs, nameless, data_end, false, o);
    // the files the rules read (mips / banks / images stay on disk: a mods
    // folder holds many GB of them; meshes too under --rules-only, which
    // then skips the stub-material rule); every other path is listed, empty
    let pkg = crate::io::kspkg::Pkg::open(kspkg)?;
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for e in &pkg.entries {
        let mesh = e.path.to_lowercase().ends_with(".mesh");
        let b = if (ext_in(&e.path, &PB_EXTS) || ext_in(&e.path, &PHYS_EXTS)) && (ctx.full || !mesh) { pkg.read(e)? } else { Vec::new() };
        files.insert(e.path.clone(), b);
    }
    drop(pkg);
    check_decode(&name, &files, o);
    let ids: BTreeSet<String> = files.keys().filter_map(|k| k.strip_prefix("content\\cars\\").or_else(|| k.strip_prefix("CONTENT\\CARS\\")).and_then(|x| x.split('\\').next()).map(|s| s.to_lowercase())).collect();
    let stem = kspkg.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    let id = ids.iter().next().cloned().unwrap_or_default();
    o.checked.push(format!("car {name}: {} files, id {id}", files.len()));
    if ids.len() != 1 {
        o.add(Sev::Fail, "car-id", name.as_str(), format!("the package holds {} car folders {:?}", ids.len(), ids), "the game takes the car id from the package name; other folders are not cars");
    }
    if !id.is_empty() && stem != id {
        o.add(Sev::Fail, "car-id", name.as_str(), format!("the package is named {stem}.kspkg but holds content\\cars\\{id}\\"), "the game looks the car up by the package name: car missing from the dealership");
    }
    let stock_ids = ctx.game.ids("cars");
    if stock_ids.contains(&id) {
        o.add(Sev::Fail, "shadow", name.as_str(), format!("{id} is a stock car id"), "content.kspkg holds the same paths and wins every lookup - the stock car shows, not the conversion");
    } else {
        let shadow: Vec<&String> = files.keys().filter(|k| ctx.game.has(k)).collect();
        if !shadow.is_empty() {
            o.add(Sev::Risk, "shadow", name.as_str(), format!("{} path(s) also in content.kspkg, e.g. {}", shadow.len(), shadow.iter().take(3).map(|s| s.as_str()).collect::<Vec<_>>().join(", ")),
                "content.kspkg wins the lookup: the package's copies are never read (seen: a loose uicomponents.css)");
        }
    }
    // the sidecar json
    let json = kspkg.with_extension("json");
    match std::fs::read(&json).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()) {
        None => o.add(Sev::Fail, "car-json", json.display().to_string(), "missing or not JSON", "the game never registers the car (it reads the preset list from <id>.json)"),
        Some(v) => {
            let cars = v["cars"].as_array().cloned().unwrap_or_default();
            if cars.is_empty() {
                o.add(Sev::Fail, "car-json", json.display().to_string(), "no \"cars\" entries", "no preset is registered - car missing");
            }
            for c in cars {
                let Some(n) = c["name"].as_str() else {
                    o.add(Sev::Fail, "car-json", json.display().to_string(), "a car entry without \"name\"", "no mechanical preset to load");
                    continue;
                };
                let want = format!("content\\cars\\{id}\\presets\\{n}.mechanicalcarpreset").to_lowercase();
                if !files.keys().any(|k| k.to_lowercase() == want) && !ctx.game.has(&want) {
                    o.add(Sev::Fail, "car-json", json.display().to_string(), format!("\"name\": {n:?} but the package has no {want}"),
                        "\"mechanical car preset data not found {}\" (0x141781fe0) - the car cannot be loaded");
                }
            }
        }
    }
    car_content(&files, &id, &ctx.shaders, o);
    // per-car settings the game re-applies over the actor
    if let Some(dir) = &ctx.saved {
        let ap = files.iter().find(|(k, _)| k.eq_ignore_ascii_case(&format!("content\\cars\\{id}\\{id}.actor"))).map(|(_, v)| v);
        if let (Ok(b), Some(actor)) = (std::fs::read(dir.join(crate::car::usersettings::CAMERAS)), ap) {
            let stale = crate::car::usersettings::stale_cameras(&b, &id, actor);
            if !stale.is_empty() {
                o.add(Sev::Risk, "saved-cameras", dir.join(crate::car::usersettings::CAMERAS).display().to_string(), format!("older cameras saved for {id}: {}", stale.join("; ")),
                    format!("the game copies these over the actor's cameras (car::usersettings) - `aceconv reset-car-settings {id}` with the game closed"));
            }
        }
    }
    if ctx.full {
        let r = super::car(kspkg, ctx.game)?;
        relay(&r, &name, o);
    }
    Ok(())
}

/// Read a track folder as `content\tracks\<id>\...`: only the `.scene`
/// files (installed tracks hold GBs of textures); other paths map to empty
/// bytes.
pub fn read_track_dir(dir: &Path, id: &str) -> Result<BTreeMap<String, Vec<u8>>> {
    fn walk(base: &Path, d: &Path, prefix: &str, out: &mut BTreeMap<String, Vec<u8>>) -> std::io::Result<()> {
        for e in std::fs::read_dir(d)?.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(base, &p, prefix, out)?;
            } else {
                let rel = p.strip_prefix(base).unwrap().to_string_lossy().replace('/', "\\");
                let scene = p.extension().map(|x| x.eq_ignore_ascii_case("scene")).unwrap_or(false);
                out.insert(format!("{prefix}{rel}"), if scene { std::fs::read(&p)? } else { Vec::new() });
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &format!("content\\tracks\\{id}\\"), &mut out)?;
    Ok(out)
}

/// A track folder (converted output, or an installed loose track).
/// `installed`: the folder sits under mods\content\tracks, so the game's
/// tables must list it.
pub fn track_folder(dir: &Path, ctx: &Ctx, installed: bool, o: &mut Outcome) -> Result<()> {
    let id = dir.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    let files = read_track_dir(dir, &id)?;
    let decl = dir.join("ui").join("track_decl.json").is_file();
    o.checked.push(format!("track {id}: {} files{}", files.len(), if decl { "" } else { " (no ui\\track_decl.json: not an aceconv track, game rules only)" }));
    if ctx.game.ids("tracks").contains(&id.to_lowercase()) {
        o.add(Sev::Fail, "shadow", id.as_str(), format!("{id} is a stock track folder"), "content.kspkg holds the same paths and wins every lookup - the conversion's files are never read");
    } else {
        let shadow: Vec<&String> = files.keys().filter(|k| ctx.game.has(k)).collect();
        if !shadow.is_empty() {
            o.add(Sev::Risk, "shadow", id.as_str(), format!("{} file(s) also in content.kspkg, e.g. {}", shadow.len(), shadow[0]), "content.kspkg wins the lookup: these copies are never read");
        }
    }
    let read = |p: &str| ctx.game.read(p);
    track_content(&files, &id, &read, o);
    if installed {
        // the game lists a track only through its system tables
        let folder = format!("content\\tracks\\{id}").to_lowercase();
        let listed = ctx.game.read("system\\tracks.table").and_then(|b| Msg::parse(&b).ok()).map(|top| {
            top.sub(2).map(|t| t.subs(3)).unwrap_or_default().iter().filter_map(|l| l.sub(2)).any(|d| d.str(3).map(|f| f.to_lowercase().trim_end_matches('\\') == folder).unwrap_or(false))
        });
        if listed == Some(false) {
            o.add(Sev::Risk, "not-listed", id.as_str(), "no system\\tracks.table row of content.kspkg points at this folder", "the track is not offered in single player (a host that lists it still works for joiners)");
        }
    }
    if ctx.full && decl {
        let r = super::track(dir, &id, ctx.game)?;
        relay(&r, &id, o);
    }
    Ok(())
}

/// A mods folder (Saved Games\ACE\mods or a converter out dir): every car
/// package, every track folder, and what they do to each other.
pub fn mods_dir(dir: &Path, ctx: &Ctx, o: &mut Outcome) -> Result<()> {
    let mut pkgs: Vec<PathBuf> = std::fs::read_dir(dir)?.flatten().map(|e| e.path()).filter(|p| p.extension().map(|e| e.eq_ignore_ascii_case("kspkg")).unwrap_or(false)).collect();
    pkgs.sort();
    // which package provides which path
    let mut owner: HashMap<String, (String, PathBuf)> = HashMap::new();
    // the bytes of one path in one package
    let bytes_of = |pkg: &Path, path: &str| -> Option<Vec<u8>> {
        let k = crate::io::kspkg::Pkg::open(pkg).ok()?;
        let e = k.find(path)?.clone();
        k.read(&e).ok()
    };
    for p in &pkgs {
        if let Err(e) = car_package(p, ctx, o) {
            o.add(Sev::Fail, "pkg-read", p.display().to_string(), format!("{e:#}"), "the game cannot mount it either");
            continue;
        }
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        if let Ok((recs, _, _)) = records(p) {
            let mut clash: Vec<(String, String)> = Vec::new();
            for r in recs.iter().filter(|r| r.kind != 1) {
                if let Some((prev, prev_path)) = owner.insert(r.path.to_lowercase(), (name.clone(), p.clone())) {
                    // identical copies (a shared brand badge) are harmless
                    if prev != name && bytes_of(&prev_path, &r.path) != bytes_of(p, &r.path) {
                        clash.push((prev, r.path.clone()));
                    }
                }
            }
            let mut by: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for (prev, path) in clash {
                by.entry(prev).or_default().push(path);
            }
            for (prev, paths) in by {
                o.add(Sev::Risk, "mod-clash", name.as_str(), format!("{} path(s) also in {prev} with other bytes, e.g. {}", paths.len(), paths[0]), "only one package's copy is read (mount order); the other mod gets the wrong file");
            }
        }
    }
    // loose content: tracks, and files that shadow the archive
    let tracks = dir.join("content").join("tracks");
    if let Ok(rd) = std::fs::read_dir(&tracks) {
        let mut ts: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        ts.sort();
        for t in ts {
            if t.file_name().map(|f| f.eq_ignore_ascii_case("common_assets")).unwrap_or(false) {
                continue;
            }
            if let Err(e) = track_folder(&t, ctx, true, o) {
                o.add(Sev::Fail, "track-read", t.display().to_string(), format!("{e:#}"), "unreadable folder");
            }
        }
    } else {
        // a converter out dir: track folders sit at the top
        let mut ts: Vec<PathBuf> = std::fs::read_dir(dir)?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).filter(|p| {
            let id = p.file_name().unwrap().to_string_lossy().into_owned();
            p.join(format!("{id}.scene")).is_file()
        }).collect();
        ts.sort();
        for t in ts {
            track_folder(&t, ctx, false, o)?;
        }
    }
    // loose files that shadow content.kspkg or a package
    let mut loose: Vec<String> = Vec::new();
    fn walk(base: &Path, d: &Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(base, &p, out);
            } else if let Ok(rel) = p.strip_prefix(base) {
                out.push(rel.to_string_lossy().replace('/', "\\"));
            }
        }
    }
    for top in ["content", "uiresources", "system"] {
        let d = dir.join(top);
        if d.is_dir() {
            walk(dir, &d, &mut loose);
        }
    }
    let shadow: Vec<&String> = loose.iter().filter(|p| ctx.game.has(p)).collect();
    if !shadow.is_empty() {
        let mut by_top: BTreeMap<String, Vec<&String>> = BTreeMap::new();
        for p in &shadow {
            let key = p.split('\\').take(3).collect::<Vec<_>>().join("\\");
            by_top.entry(key).or_default().push(p);
        }
        for (k, ps) in by_top {
            o.add(Sev::Risk, "loose-shadow", format!("{}\\{k}\\...", dir.display()), format!("{} loose file(s) whose path content.kspkg already holds, e.g. {}", ps.len(), ps[0]),
                "ignored: the archive entry wins the lookup, loose files only fill absences (seen in game: uicomponents.css)");
        }
    }
    let in_pkg: Vec<&String> = loose.iter().filter(|p| owner.contains_key(&p.to_lowercase()) && !p.to_lowercase().ends_with(".texturemips")).collect();
    if !in_pkg.is_empty() {
        o.add(Sev::Risk, "loose-shadow", dir.display().to_string(), format!("{} loose file(s) also inside a package, e.g. {} ({})", in_pkg.len(), in_pkg[0], owner[&in_pkg[0].to_lowercase()].0),
            "only one copy is read - edits to the loose file may do nothing");
    }
    // packages without their json and the other way round
    for e in std::fs::read_dir(dir)?.flatten() {
        let p = e.path();
        if p.extension().map(|x| x.eq_ignore_ascii_case("json")).unwrap_or(false) && !p.with_extension("kspkg").is_file() {
            let v = std::fs::read(&p).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
            if v.map(|v| v["cars"].is_array()).unwrap_or(false) {
                o.add(Sev::Risk, "car-json", p.display().to_string(), "a car json without its .kspkg", "the game lists presets of a car it cannot load");
            }
        }
    }
    Ok(())
}

/// Calibration: the content rules on every stock car and track. Prints
/// what fires; returns the findings.
pub fn stock(ctx: &Ctx, content: &Path) -> Outcome {
    let mut o = Outcome::default();
    let game = ctx.game;
    // the archive's own table (122k records: ids, order, kinds)
    match records(content) {
        Ok((recs, nameless, end)) => {
            check_table("content.kspkg", &recs, nameless, end, true, &mut o);
            o.checked.push(format!("content.kspkg table: {} records", recs.len()));
        }
        Err(e) => o.add(Sev::Fail, "pkg-read", content.display().to_string(), format!("{e:#}"), "unreadable"),
    }
    let mut cars: Vec<String> = game.ids("cars").into_iter().filter(|i| game.has(&format!("content\\cars\\{i}\\{i}.actor"))).collect();
    cars.sort();
    for id in &cars {
        let mut files = BTreeMap::new();
        for p in game.paths_with_prefix(&format!("content\\cars\\{id}\\")) {
            if ext_in(&p, &[".material", ".mesh", ".actor"]) || ext_in(&p, &PHYS_EXTS) {
                if let Some(b) = game.read(&p) {
                    files.insert(p, b);
                }
            }
        }
        check_decode(id, &files, &mut o);
        car_content(&files, id, &ctx.shaders, &mut o);
    }
    let mut tracks: Vec<String> = game.ids("tracks").into_iter().filter(|i| game.has(&format!("content\\tracks\\{i}\\{i}.scene"))).collect();
    tracks.sort();
    for id in &tracks {
        let files = BTreeMap::new();
        let read = |p: &str| game.read(p);
        track_content(&files, id, &read, &mut o);
    }
    o.checked.push(format!("{} stock cars, {} stock tracks", cars.len(), tracks.len()));
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(path: &str, kind: u16) -> Record {
        Record { path: path.into(), kind, path_len: path.chars().count() as u16, id: entry_id(path), size: 4, off: 0 }
    }

    #[test]
    fn table_rules() {
        let mut ok = vec![rec("content\\cars\\x\\a.mesh", 0x100), rec("content\\cars\\x\\t.texturemips", 0), rec("content\\cars\\x", 1)];
        ok.sort_by_key(|r| r.id);
        let mut o = Outcome::default();
        check_table("ok", &ok, 0, 100, true, &mut o);
        assert!(o.findings.is_empty(), "{:?}", o.findings);
        // encrypted mips, mixed-case id, unsorted, past the end
        let mut bad = ok.clone();
        bad.iter_mut().find(|r| r.path.ends_with(".texturemips")).unwrap().kind = 0x100;
        let mut m = rec("content\\cars\\x\\B.material", 0x100);
        m.id = {
            let mut h: u64 = 0xcbf29ce484222325;
            for u in m.path.encode_utf16() {
                for c in u.to_le_bytes() {
                    h = (h ^ c as u64).wrapping_mul(0x100000001b3);
                }
            }
            h
        };
        bad.push(m);
        bad.reverse();
        bad[0].off = 99;
        let mut o = Outcome::default();
        check_table("bad", &bad, 1, 100, true, &mut o);
        let checks: BTreeSet<&str> = o.findings.iter().map(|f| f.check).collect();
        for c in ["mips-encrypted", "pkg-id", "pkg-sort", "pkg-header"] {
            assert!(checks.contains(c), "{c} not caught: {:?}", o.findings);
        }
    }

    fn scene(ents: Vec<Msg>) -> Vec<u8> {
        let mut s = Msg::default();
        for e in ents {
            s.push(2, Val::Msg(e));
        }
        s.encode()
    }
    fn ent(kind: &str, field: u32, data: Msg) -> Msg {
        let mut e = Msg::default();
        e.set_str(1, "e");
        e.push(3, Val::Bytes(kind.as_bytes().to_vec()));
        let mut d = Msg::default();
        d.push(field, Val::Msg(data));
        e.push(50, Val::Msg(d));
        e
    }

    #[test]
    fn surfaces_and_categories() {
        let mut sd = Msg::default();
        let mut s = Msg::default();
        s.set_str(1, "ROAD");
        s.set_varint(2, 1);
        sd.push(1, Val::Msg(s));
        let mut sm = Msg::default();
        sm.set_str(1, "content\\tracks\\t\\m.mesh");
        sm.set_str(2, "ROAD");
        let info = ent("Track Info", 111, Msg::default());
        let good = scene(vec![ent("Surface Def", 109, sd.clone()), ent("SMesh", 100, sm.clone()), info.clone()]);
        let mut files = BTreeMap::new();
        files.insert("content\\tracks\\t\\t.scene".to_string(), good);
        let mut o = Outcome::default();
        track_content(&files, "t", &|_| None, &mut o);
        assert!(o.findings.is_empty(), "{:?}", o.findings);
        // a mesh on an undefined surface, a category 7, no track info
        let mut sm2 = sm.clone();
        sm2.set_str(2, "GRASS");
        let mut sd2 = Msg::default();
        let mut s2 = Msg::default();
        s2.set_str(1, "ROAD");
        s2.set_varint(2, 7);
        sd2.push(1, Val::Msg(s2));
        files.insert("content\\tracks\\t\\t.scene".to_string(), scene(vec![ent("Surface Def", 109, sd2), ent("SMesh", 100, sm2)]));
        let mut o = Outcome::default();
        track_content(&files, "t", &|_| None, &mut o);
        let checks: BTreeSet<&str> = o.findings.iter().map(|f| f.check).collect();
        assert_eq!(checks, ["surface-category", "surface-id", "track-info"].into_iter().collect(), "{:?}", o.findings);
    }

    #[test]
    fn materials_and_nan() {
        let shaders: BTreeSet<String> = ["UberVehicleMaterial".to_string()].into();
        let mut files = BTreeMap::new();
        let mut m = Msg::default();
        m.set_str(1, "MadeUpShader");
        files.insert("content\\cars\\x\\a.material".to_string(), m.encode());
        let mut c = Msg::default();
        c.set_f32(3, f32::NAN);
        let mut car = Msg::default();
        car.push(8, Val::Msg(c));
        files.insert("content\\cars\\x\\data\\x.car".to_string(), car.encode());
        let mut o = Outcome::default();
        car_content(&files, "x", &shaders, &mut o);
        let checks: BTreeSet<&str> = o.findings.iter().map(|f| f.check).collect();
        assert!(checks.contains("shader") && checks.contains("phys-nan"), "{:?}", o.findings);
    }
}
