//! aceconv: the command line.

use anyhow::{anyhow, bail, Context, Result};
use std::path::{Path, PathBuf};

const USAGE: &str = "aceconv <command> ...

  unpack <pkg.kspkg> <out dir> [filter]     extract (filter: path substring)
  pack <dir> <out.kspkg>                    pack a folder (holding content\\...) into a package
  check-car <car.kspkg> [content.kspkg]     validate a converted car package
  check-car-stock [content.kspkg] [id...]   the car batch checks on stock cars (their calibration)
  check-track <track dir> [content.kspkg]   validate a converted track folder
  check-track-decl [content.kspkg] [--skip a,b]  simulate every stock track's (layout, mode) lookup
                                            (+ its track-limit / ideal-line splines and spawn orientation)
  check-lighting <track dir>...             only the static-mesh lighting checks
  make-donor [content.kspkg] [out dir]      build the donor car + template materials
  make-fmod-template [content.kspkg] [dir]  decompile the game banks into the sound template
  fmod-decompile <content.kspkg> <bank> <dir> [event filter]
  fmodbank <project dir> <out dir>          build master_bank(.strings).bank in-house
  sound <ac1 car dir> <out dir> [id]        AC1 car sounds -> <out>\\master_bank(.strings).bank (as the car stage)
  ac1sound-dump <ac1 car dir> <out dir>     decompile an AC1 car bank (FMOD 1.08) into a Studio project
  stockstats [content.kspkg] [out dir]      measure stock content -> <out>\\stockstats.json
  stockstats compare <car.kspkg | track dir> <stockstats.json>
  albedo <track dir | content.kspkg> [track id...]   effective albedo per material class
  measure <phys | frames | pigment> [content.kspkg]   stock track measurements behind track-stage constants
  slim <in.kspkg | dir> <out.kspkg | dir>   shrink a package (base refs, dedupe, unreferenced)
  pack-share <kspkg | track dir> <out.acemod>
  unpack-share <in.acemod> <out.kspkg | dir>
  acd <ac1 car dir> [file...]               print a car's data files (data.acd or data\\)
  car <ac1 car dir> <out dir> [--id <id>] [--donor <zip>] [--refmats <dir>] [--content <content.kspkg>]
                                            convert an AC1 car -> <out>\\<id>.kspkg + <id>.json
  car-frame <ac1 car dir>                   the car's EVO frame (datum, wheel centres, CoG) as json
  reset-car-settings <id> [--dry-run] [--dir <Saved Games\\ACE>]
                                            forget the game's saved cameras / mirrors of a car id
                                            (the game re-applies them over a reinstalled actor;
                                            run with the game closed)
  car-phys <car.kspkg | stock car id> [content.kspkg]   the physics model, gearing, AI hints, PI as json
  car-phys --ac1 <ac1 car dir>...           physics stage only (no package): drivetrain read-back, one json line per car
  track <ac1 track dir> <out dir> [--id <id>]  convert an AC1 track -> <out>\\<id>\\ (+ ui\\track_decl.json)
  track-scan <ac1 track dir>...             scan AC1 tracks: layouts, meshes, spawns, AI lines
  veg-catalogue [content.kspkg]             the factory plants the vegetation stage picks from
  car-id <ac1 folder>... / track-id <ac1 folder>...
  brands <ac1 cars dir> [content.kspkg]     the stock brands, and the brand each AC1 car gets
  legacy-id <mods dir> <cars|tracks> <ac1 folder>   earlier conversions of the same source

Environment: ACECONV_GAME_CONTENT (content.kspkg), ACECONV_THREADS (default 2),
ACECONV_BAKES (track lighting bakes: comma list of ao, probes, pigment, or none; unset = all),
ACECONV_BANK_VORBIS (car sound samples: <quality 0.6..1> Vorbis, 0 PCM16; unset = as the AC1 bank),
ACECONV_VEGETATION (track plants: comma list of trees, grass, or none; unset = both).";

/// Below-normal work by default: the converter shares the PC with the game.
fn low_priority() {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetCurrentProcess() -> isize;
            fn SetPriorityClass(h: isize, class: u32) -> i32;
        }
        // IDLE_PRIORITY_CLASS
        unsafe { SetPriorityClass(GetCurrentProcess(), 0x40) };
    }
}

fn threads() {
    let n = std::env::var("ACECONV_THREADS").ok().and_then(|v| v.parse().ok()).unwrap_or(2usize);
    let _ = rayon::ThreadPoolBuilder::new().num_threads(n.max(1)).build_global();
}

fn content(a: Option<&String>) -> PathBuf {
    a.map(PathBuf::from).unwrap_or_else(aceconv::io::gamedir::evo_content)
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let Some(cmd) = a.get(1).map(|s| s.as_str()) else {
        println!("{USAGE}");
        return Ok(());
    };
    low_priority();
    threads();
    let arg = |i: usize| a.get(i).ok_or_else(|| anyhow!("missing argument {i}\n\n{USAGE}"));
    match cmd {
        "unpack" => {
            let out = PathBuf::from(arg(3)?);
            let n = aceconv::io::kspkg::read_each(Path::new(arg(2)?), a.get(4).map(|s| s.as_str()).unwrap_or(""), |p, d| {
                let dst = out.join(p.replace('\\', "/"));
                if let Some(d) = dst.parent() {
                    std::fs::create_dir_all(d)?;
                }
                std::fs::write(&dst, d).with_context(|| format!("write {}", dst.display()))
            })?;
            println!("{n} files -> {}", out.display());
        }
        "pack" => {
            let files = aceconv::slim::read_dir(Path::new(arg(2)?))?;
            let mut list: Vec<(String, Vec<u8>)> = files.into_iter().collect();
            aceconv::io::kspkg::write(Path::new(arg(3)?), &mut list)?;
            println!("{} files -> {}", list.len(), arg(3)?);
        }
        "check-car" => {
            let game = aceconv::validate::Game::open(&content(a.get(3)))?;
            let r = aceconv::validate::car(Path::new(arg(2)?), &game)?;
            aceconv::validate::print(&r, arg(2)?);
            if !r.ok() {
                std::process::exit(2);
            }
        }
        "check-car-stock" => {
            // the batch checks on stock cars (their calibration): all, or the ids given
            let game = aceconv::validate::Game::open(&content(a.get(2)))?;
            let mut ids: Vec<String> = if a.len() > 3 { a[3..].to_vec() } else { game.ids("cars").into_iter().filter(|i| game.has(&format!("content\\cars\\{i}\\{i}.actor"))).collect() };
            ids.sort();
            let (mut pass, mut warned) = (0, 0);
            for id in &ids {
                let r = aceconv::validate::stock_car(id, &game);
                println!("{id}: {} errors, {} warnings", r.errors.len(), r.warnings.len());
                for e in &r.errors {
                    println!("  ERROR {e}");
                }
                for w in &r.warnings {
                    println!("  warn  {w}");
                }
                pass += r.ok() as usize;
                warned += (!r.warnings.is_empty()) as usize;
            }
            println!("{pass} of {} stock cars pass ({warned} with warnings)", ids.len());
            if pass != ids.len() {
                std::process::exit(2);
            }
        }
        "check-track" => {
            let dir = PathBuf::from(arg(2)?);
            let id = dir.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            let game = aceconv::validate::Game::open(&content(a.get(3)))?;
            let r = aceconv::validate::track(&dir, &id, &game)?;
            aceconv::validate::print(&r, arg(2)?);
            if !r.ok() {
                std::process::exit(2);
            }
        }
        "check-track-decl" => {
            // the game's own rows (content.kspkg system tables): every track
            // whose scene the package holds, minus --skip names
            let mut skip: Vec<String> = Vec::new();
            let mut pkg_arg: Option<&String> = None;
            let mut i = 2;
            while i < a.len() {
                if a[i] == "--skip" {
                    skip.extend(a.get(i + 1).map(|s| s.split(',').map(|x| x.trim().to_string()).collect::<Vec<_>>()).unwrap_or_default());
                    i += 2;
                } else {
                    pkg_arg = Some(&a[i]);
                    i += 1;
                }
            }
            let game = aceconv::validate::Game::open(&content(pkg_arg))?;
            let tracks = game.read("system\\tracks.table").context("no system\\tracks.table")?;
            let rows = aceconv::validate::decl::table_rows(&game.read("system\\track_containers.table").context("no system\\track_containers.table")?);
            let top = aceconv::io::pb::Msg::parse(&tracks)?;
            let mut names: Vec<String> = Vec::new();
            let mut scenes: std::collections::HashMap<String, String> = Default::default();
            for line in top.sub(2).map(|t| t.subs(3)).unwrap_or_default() {
                let Some(d) = line.sub(2) else { continue };
                let (Some(n), Some(scene)) = (d.str(1), d.str(4)) else { continue };
                if game.has(&scene) && d.str(8).is_some() && !skip.iter().any(|s| s.eq_ignore_ascii_case(&n)) && !names.contains(&n) {
                    scenes.insert(n.clone(), scene);
                    names.push(n);
                }
            }
            let (mut pass, mut all) = (0, 0);
            for n in &names {
                let mut r = aceconv::validate::Report::default();
                let read = |p: &str| game.read(p);
                let mut s = aceconv::validate::decl::check(n, &rows, &read, false, &mut r);
                let main = scenes.get(n).and_then(|p| game.read(p)).unwrap_or_default();
                s.extend(aceconv::validate::lines::check(n, &rows, &main, &read, false, &mut r));
                all += 1;
                pass += r.ok() as usize;
                println!("{n}: {} errors, {} warnings", r.errors.len(), r.warnings.len());
                for l in &s {
                    println!("  {l}");
                }
                for e in &r.errors {
                    println!("  ERROR {e}");
                }
                for w in &r.warnings {
                    println!("  warn  {w}");
                }
            }
            println!("{pass} of {all} tracks pass");
            if pass != all {
                std::process::exit(2);
            }
        }
        "check-lighting" => {
            let mut bad = false;
            for d in &a[2..] {
                let r = aceconv::validate::track_lighting(Path::new(d))?;
                aceconv::validate::print(&r, d);
                bad |= !r.ok();
            }
            if bad {
                std::process::exit(2);
            }
        }
        "make-donor" => {
            let c = content(a.get(2));
            match a.get(3) {
                Some(out) => {
                    aceconv::donor::make(&c, Path::new(out))?;
                    println!("donor -> {out}");
                }
                None => {
                    let (zip, refmats) = aceconv::donor::ensure(&c)?;
                    println!("donor {}\nrefmats {}", zip.display(), refmats.display());
                }
            }
        }
        "make-fmod-template" => {
            let c = content(a.get(2));
            match a.get(3) {
                Some(dir) => {
                    let out = aceconv::sound::mkfmod::make(&c, Path::new(dir))?;
                    for n in &out.notes {
                        println!("note: {n}");
                    }
                    println!("{} events -> {dir}", out.events.len());
                }
                None => println!("{}", aceconv::sound::mkfmod::ensure(&c)?.display()),
            }
        }
        "fmod-decompile" => {
            let out = aceconv::sound::mkfmod::decompile_bank(Path::new(arg(2)?), arg(3)?, Path::new(arg(4)?), a.get(5).map(|s| s.as_str()).unwrap_or(""))?;
            for n in &out.notes {
                println!("note: {n}");
            }
            println!("{} events -> {}", out.events.len(), arg(4)?);
        }
        "fmodbank" => {
            let b = aceconv::sound::bankwrite::build_dir(Path::new(arg(2)?))?;
            let out = PathBuf::from(arg(3)?);
            std::fs::create_dir_all(&out)?;
            std::fs::write(out.join(format!("{}.bank", b.name)), &b.bank)?;
            std::fs::write(out.join(format!("{}.strings.bank", b.name)), &b.strings)?;
            println!("{} -> {}", b.name, out.display());
        }
        "sound" => {
            // AC1 car sound -> <out>\master_bank(.strings).bank (the car stage does the same inside the package)
            let car = PathBuf::from(arg(2)?);
            let out = PathBuf::from(arg(3)?);
            // the event paths carry the id: a stock car's id would resolve to the game's own events
            let id = a.get(4).cloned().unwrap_or_else(|| format!("ac1_{}", car.file_name().map(|f| f.to_string_lossy().to_lowercase()).unwrap_or_default()));
            let o = aceconv::sound::map::Opts { car_id: id, content: content(None), limiter: aceconv::sound::map::limiter_rpm(&car), work: out.join("project"),
                vorbis: aceconv::sound::map::vorbis_env() };
            let b = aceconv::sound::map::build(&car, &o)?;
            std::fs::write(out.join("master_bank.bank"), &b.bank)?;
            std::fs::write(out.join("master_bank.strings.bank"), &b.strings)?;
            for n in &b.notes {
                println!("note: {n}");
            }
            for (r, p) in &b.events {
                println!("{r:?}	{p}");
            }
        }
        "ac1sound-dump" => {
            let notes = aceconv::sound::map::dump(Path::new(arg(2)?), &content(a.get(4)), Path::new(arg(3)?))?;
            for n in &notes {
                println!("note: {n}");
            }
        }
        "stockstats" if a.get(2).map(|s| s == "compare").unwrap_or(false) => {
            let n = aceconv::stats::stockstats::compare(Path::new(arg(3)?), Path::new(arg(4)?))?;
            if n > 0 {
                std::process::exit(2);
            }
        }
        "stockstats" => {
            let c = content(a.get(2));
            let out = a.get(3).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("stockstats"));
            let st = aceconv::stats::stockstats::measure(&c)?;
            let p = aceconv::stats::stockstats::write(&st, &out)?;
            println!("{} cars, {} tracks -> {}", st.cars.len(), st.tracks.len(), p.display());
        }
        "albedo" => albedo_cmd(Path::new(arg(2)?), &a[3..])?,
        "measure" => {
            use aceconv::stats::trackmeasure as tm;
            let pkg = aceconv::io::kspkg::Pkg::open(&content(a.get(3)))?;
            let out = match arg(2)?.as_str() {
                "phys" => tm::phys(&pkg)?,
                "frames" => tm::frames(&pkg)?,
                "pigment" => tm::pigment(&pkg)?,
                w => bail!("measure: unknown '{w}' (phys, frames, pigment)"),
            };
            print!("{out}");
        }
        "slim" => {
            let o = aceconv::slim::Opts::from_env();
            let game = content(None);
            let st = aceconv::slim::slim_path(Path::new(arg(2)?), Path::new(arg(3)?), game.is_file().then_some(game.as_path()), &o)?;
            for l in &st.lines {
                println!("{l}");
            }
            println!("saved {:.1} MB", st.saved() as f64 / 1048576.0);
        }
        "pack-share" => {
            let game = content(None);
            let r = aceconv::share::pack(Path::new(arg(2)?), Path::new(arg(3)?), game.is_file().then_some(game.as_path()))?;
            println!("{} bytes in {:.1} s", r.out_bytes, r.secs);
        }
        "unpack-share" => {
            let game = content(None);
            let r = aceconv::share::unpack(Path::new(arg(2)?), Path::new(arg(3)?), game.is_file().then_some(game.as_path()))?;
            println!("rebuilt in {:.1} s, identical: {}", r.secs, r.identical);
        }
        "acd" => {
            let d = aceconv::ac1::acd::read_car_data(Path::new(arg(2)?))?;
            if a.len() <= 3 {
                for (k, v) in &d {
                    println!("{k}\t{} bytes", v.len());
                }
            }
            for n in &a[3..] {
                println!("===== {n}");
                println!("{}", d.get(&n.to_lowercase()).map(|s| s.as_str()).unwrap_or("<missing>"));
            }
        }
        "car" => {
            let src = PathBuf::from(arg(2)?);
            let out = PathBuf::from(arg(3)?);
            let mut o = aceconv::car::default_opts(&src, &out);
            let mut i = 4;
            while i < a.len() {
                let v = a.get(i + 1).cloned().ok_or_else(|| anyhow!("{} needs a value", a[i]))?;
                match a[i].as_str() {
                    "--id" => o.id = Some(v),
                    "--donor" => o.donor_zip = PathBuf::from(v),
                    "--refmats" => o.refmats = PathBuf::from(v),
                    "--content" => o.content = Some(PathBuf::from(v)),
                    x => bail!("unknown option {x}"),
                }
                i += 2;
            }
            let r = aceconv::car::convert(&o)?;
            for n in &r.notes {
                println!("note: {n}");
            }
            println!("{}\t{}", r.id, r.package.display());
        }
        "reset-car-settings" => {
            // the game's saved per-car cameras / mirrors (car::usersettings);
            // run with the game closed
            let id = arg(2)?.to_string();
            let dry = a.iter().any(|x| x == "--dry-run");
            let dir = match a.iter().position(|x| x == "--dir") {
                Some(i) => PathBuf::from(a.get(i + 1).ok_or_else(|| anyhow!("--dir needs a value"))?),
                None => aceconv::car::usersettings::saved_dir().ok_or_else(|| anyhow!("no Saved Games\\ACE folder (pass --dir)"))?,
            };
            let done = aceconv::car::usersettings::reset(&dir, &id, dry)?;
            if done.is_empty() {
                println!("{}: nothing saved for {id}", dir.display());
            }
            for d in done {
                println!("{}{d}", if dry { "would remove " } else { "removed " });
            }
        }
        "car-frame" => {
            let car = aceconv::ac1::car::Car::open(Path::new(arg(2)?))?;
            let k = car.kn5()?;
            let f = aceconv::car::frame::Frame::build(&car, &k);
            println!("{}", serde_json::to_string_pretty(&f.json())?);
        }
        "car-phys" if a.get(2).map(|s| s == "--ac1").unwrap_or(false) => {
            // physics stage only, on AC1 car folders: one json line per car
            let (donor, _) = aceconv::batch::donor_paths()?;
            let pd = aceconv::car::physics_donor(&donor)?;
            let c = content(None);
            for dir in &a[3..] {
                let v = aceconv::car::physics_dry_run(Path::new(dir), &pd, c.is_file().then_some(c.as_path()))
                    .unwrap_or_else(|e| serde_json::json!({"car": Path::new(dir).file_name().map(|f| f.to_string_lossy().into_owned()), "error": format!("{e:#}")}));
                println!("{}", serde_json::to_string(&v)?);
            }
        }
        "car-phys" => {
            // the physics model of a converted package or a stock car id
            let src = arg(2)?;
            let pkg = aceconv::io::kspkg::Pkg::open(&content(a.get(3)))?;
            let files: std::collections::BTreeMap<String, Vec<u8>> = if src.to_lowercase().ends_with(".kspkg") {
                aceconv::io::kspkg::read(Path::new(src), "")?.into_iter().collect()
            } else {
                let pre = format!("content\\cars\\{}\\", src.to_lowercase());
                pkg.entries.iter().filter(|e| e.path.to_lowercase().starts_with(&pre) && !e.path.to_lowercase().ends_with("mips") && !e.path.to_lowercase().ends_with(".mesh") && !e.path.to_lowercase().ends_with(".texture")).filter_map(|e| pkg.read(e).ok().map(|d| (e.path.clone(), d))).collect()
            };
            let lower: std::collections::BTreeMap<String, &Vec<u8>> = files.iter().map(|(k, v)| (k.to_lowercase(), v)).collect();
            let get = |p: &str| -> Option<Vec<u8>> { lower.get(&p.to_lowercase()).map(|v| (*v).clone()).or_else(|| pkg.find(p).and_then(|e| pkg.read(e).ok())) };
            let cdp = aceconv::car::physics::stats::cardata_of(files.keys().map(|s| s.as_str())).ok_or_else(|| anyhow!("no CarData"))?;
            let cd = aceconv::io::pb::Msg::parse(&files[&cdp])?;
            let d = aceconv::car::physics::stats::derive(&cd, &get);
            let (pi, h, run) = aceconv::car::physics::ai::perf_indicator(&d);
            let g = aceconv::car::physics::check::gearing(&d);
            println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                "cardata": cdp, "pi": pi, "hints": h, "gearing": g, "metrics": aceconv::car::physics::stats::metrics(&d),
                "chassis": aceconv::car::physics::check::chassis(&d), "derived": d, "t_0_100": run.t_0_100,
            }))?);
        }
        "car-id" | "track-id" => {
            let kind = if cmd == "car-id" { "cars" } else { "tracks" };
            let c = content(None);
            let taken = if c.is_file() { aceconv::validate::Game::open(&c)?.ids(kind) } else { Default::default() };
            for f in &a[2..] {
                let name = Path::new(f).file_name().map(|x| x.to_string_lossy().into_owned()).unwrap_or_else(|| f.clone());
                println!("{name}\t{}", aceconv::ids::id_for(kind, &name, &taken));
            }
        }
        "brands" => {
            // the stock brands, and what every AC1 car's ui_car.json brand becomes
            let game = aceconv::validate::Game::open(&content(a.get(3)))?;
            let stock = aceconv::car::brand::Brands::from_game(&game);
            println!("stock brands ({}): key | name in the cars | en label", stock.list.len());
            for b in &stock.list {
                println!("  {} | {} | {}", b.key, b.name, b.label);
            }
            let mut dirs: Vec<PathBuf> = std::fs::read_dir(arg(2)?)?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
            dirs.sort();
            let (mut n_stock, mut local, mut art) = (0, Vec::new(), std::collections::BTreeMap::<&str, usize>::new());
            let mut by_stock = std::collections::BTreeMap::<String, usize>::new();
            let mut skipped = Vec::new();
            for d in &dirs {
                let ui = aceconv::ac1::car::find_ci(d, "ui").and_then(|u| aceconv::ac1::car::find_ci(&u, "ui_car.json"));
                let raw = ui.and_then(|p| std::fs::read(p).ok()).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
                let folder = d.file_name().unwrap().to_string_lossy().into_owned();
                if raw.is_empty() {
                    skipped.push(folder);
                    continue;
                }
                let ac1 = aceconv::ac1::car::ui_field(&raw, "brand");
                let b = aceconv::car::brand::resolve(ac1.as_deref(), aceconv::ac1::car::ui_field(&raw, "name").as_deref(), &stock);
                if b.stock {
                    n_stock += 1;
                    *by_stock.entry(format!("{} <- {}", b.key, ac1.as_deref().unwrap_or("-"))).or_default() += 1;
                } else {
                    let src = aceconv::car::brand::art_source(d);
                    *art.entry(src).or_default() += 1;
                    local.push(format!("  {folder}: \"{}\" -> {} \"{}\" ({src})", ac1.as_deref().unwrap_or("-"), b.key, b.label));
                }
            }
            println!("\nstock matches (key <- AC1 brand: cars):");
            for (k, n) in &by_stock {
                println!("  {k}: {n}");
            }
            println!("\ncar-local brands:");
            for l in &local {
                println!("{l}");
            }
            println!("\nno ui\\ui_car.json (skipped): {}", skipped.join(", "));
            println!("\n{} cars: {} stock brand, {} car-local (badge from {:?})", dirs.len() - skipped.len(), n_stock, local.len(), art);
        }
        "legacy-id" => {
            let kind = arg(3)?.as_str();
            if kind != "cars" && kind != "tracks" {
                bail!("kind must be cars or tracks");
            }
            let found = aceconv::ids::legacy(Path::new(arg(2)?), kind, Path::new(arg(4)?))?;
            if found.is_empty() {
                println!("no earlier conversion found");
            }
            for e in found {
                println!("{}\t{}\t{}\t{}", e.id, e.strength, e.path.display(), e.why.join("; "));
            }
        }
        "track" => {
            let id = a.iter().position(|x| x == "--id").and_then(|i| a.get(i + 1)).cloned();
            let opt = aceconv::track::build::Options { id, content: None };
            let r = aceconv::track::build::convert(Path::new(arg(2)?), Path::new(arg(3)?), &opt)?;
            for l in &r.report {
                println!("{l}");
            }
            println!("-> {}", r.dir.display());
        }
        "veg-catalogue" => {
            let pkg = aceconv::io::kspkg::Pkg::open(&content(a.get(2)))?;
            let c = aceconv::track::vegcat::load(&pkg);
            for s in &c.species {
                println!("{:?}	{}	h {:.2}	w {:.2}	cull {:.0}	colour {:.3} {:.3} {:.3}	conifer {} palm {} dead {}	climate {:?}	{}",
                    s.family, s.name, s.height, s.width, s.lod_out, s.colour[0], s.colour[1], s.colour[2], s.conifer, s.palm, s.dead, s.climate, s.path);
            }
            for n in &c.notes {
                println!("note: {n}");
            }
        }
        "track-scan" => {
            let mut bad = 0;
            for d in &a[2..] {
                match aceconv::track::Source::open(Path::new(d)).and_then(|s| aceconv::track::Scan::new(&s).map(|sc| sc.report())) {
                    Ok(lines) => lines.iter().for_each(|l| println!("{l}")),
                    Err(e) => {
                        bad += 1;
                        println!("{d}: ERROR {e:#}");
                    }
                }
            }
            if bad > 0 {
                std::process::exit(2);
            }
        }
        "help" | "-h" | "--help" => println!("{USAGE}"),
        c => bail!("unknown command {c}\n\n{USAGE}"),
    }
    Ok(())
}

/// `aceconv albedo`: effective albedo per material class of a converted
/// track folder or of stock tracks (stats/albedo.rs).
fn albedo_cmd(src: &Path, ids: &[String]) -> Result<()> {
    use aceconv::stats::albedo;
    let game = aceconv::io::kspkg::Pkg::open(&content(None)).ok();
    let from_pkg = |p: &str| -> Option<Vec<u8>> { game.as_ref().and_then(|g| g.find(p).and_then(|e| g.read(e).ok())) };
    let page_pkg = |p: &str, i: usize| -> Option<Vec<u8>> {
        let g = game.as_ref()?;
        let e = g.find(p)?;
        g.raw(e).and_then(|r| r.get(i * 65536..(i + 1) * 65536).map(|s| s.to_vec()))
    };
    let print = |label: &str, r: &albedo::Report| {
        let parts: Vec<String> = r.by_class.iter().map(|(k, v)| {
            let (a, b, c) = albedo::quart(v);
            format!("{k} {b:.3} [{a:.3}..{c:.3}] n{}", v.len())
        }).collect();
        println!("{label}: {}", parts.join(", "));
    };
    if src.is_dir() {
        let id = src.file_name().unwrap().to_string_lossy().to_string();
        let r = albedo::measure_dir(src, game.as_ref())?;
        print(&id, &r);
        if std::env::var("ACECONV_ALBEDO_ROWS").is_ok() {
            for (n, s, c, y) in &r.rows {
                println!("  {n}\t{s}\t{c}\t{y:.3}");
            }
        }
        return Ok(());
    }
    let g = game.as_ref().ok_or_else(|| anyhow!("content.kspkg not readable"))?;
    let pkg = aceconv::io::kspkg::Pkg::open(src)?;
    let mut tracks: Vec<String> = ids.to_vec();
    if tracks.is_empty() {
        let mut s = std::collections::BTreeSet::new();
        for e in &pkg.entries {
            let l = e.path.to_lowercase();
            if let Some(r) = l.strip_prefix("content\\tracks\\") {
                if let Some((t, _)) = r.split_once('\\') {
                    if t != "common_assets" && t != "interns" && t != "road_atlanta" {
                        s.insert(t.to_string());
                    }
                }
            }
        }
        tracks = s.into_iter().collect();
    }
    let _ = g;
    let mut all: std::collections::BTreeMap<&'static str, Vec<f32>> = Default::default();
    let mut pooled: std::collections::BTreeMap<&'static str, Vec<f32>> = Default::default();
    for t in &tracks {
        let pre = format!("content\\tracks\\{t}\\");
        let mats: Vec<(String, Vec<u8>)> = pkg.entries.iter().filter(|e| e.path.to_lowercase().starts_with(&pre) && e.path.to_lowercase().ends_with(".material"))
            .filter_map(|e| pkg.read(e).ok().map(|b| (e.path.clone(), b))).collect();
        let r = albedo::measure(&mats, &|p| from_pkg(p), &|p, i| page_pkg(p, i));
        print(t, &r);
        if std::env::var("ACECONV_ALBEDO_ROWS").is_ok() {
            for (n, s, c, y) in &r.rows {
                println!("  {n}\t{s}\t{c}\t{y:.3}");
            }
        }
        for (k, v) in &r.by_class {
            let (_, med, _) = albedo::quart(v);
            all.entry(k).or_default().push(med);
            pooled.entry(k).or_default().extend(v.iter().copied());
        }
    }
    for v in pooled.values_mut() {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    }
    let q = |v: &[f32], p: f32| v[((v.len() - 1) as f32 * p).round() as usize];
    let parts: Vec<String> = pooled.iter().map(|(k, v)| format!("{k} p05 {:.3} p25 {:.3} p50 {:.3} p75 {:.3} p95 {:.3} n{}", q(v, 0.05), q(v, 0.25), q(v, 0.5), q(v, 0.75), q(v, 0.95), v.len())).collect();
    println!("STOCK materials pooled, per class: {}", parts.join(", "));
    for v in all.values_mut() {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    }
    let parts: Vec<String> = all.iter().map(|(k, v)| {
        let (a, b, c) = albedo::quart(v);
        format!("{k} {b:.3} [{a:.3}..{c:.3}] n{}", v.len())
    }).collect();
    println!("STOCK track medians, per class (median [p25..p75] over tracks): {}", parts.join(", "));
    Ok(())
}
