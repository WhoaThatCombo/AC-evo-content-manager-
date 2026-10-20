//! Surface classes: what kind of ground an AC1 physics surface is, and what
//! an AC1 material draws (drivable track, tarmac, grass / sand / dirt).
//!
//! # Surface kind (ours)
//!
//! From the AC1 KEY, WAV, DAMPING, FRICTION and IS_PITLANE, first match:
//!
//! | rule | kind |
//! |---|---|
//! | key names a wall / barrier / fence / guard rail / tyre wall | Wall |
//! | IS_PITLANE = 1 | Pit |
//! | `kerb.wav`, or key KERB / CURB / RUMBLE / SAUSAGE | Kerb (Flat kerb when SIN_HEIGHT = 0 and VIBRATION_GAIN < 0.3) |
//! | `extraturf.wav`, or key TURF / CARPET / ASTRO | Turf |
//! | `sand.wav` / `gravel.wav` / key SAND / GRAVEL / GHIAIA / SABBIA / TRAP with DAMPING > 0 | Sand (a real trap: damping slows the car) |
//! | `sand.wav` without damping | Verge (loose, road-like) when FRICTION >= 0.75, else Grass |
//! | `grass.wav`, or key GRASS / ERBA / GREEN-with-low-grip / DIRT / MUD | Grass |
//! | key CONCRETE / CONCR / CEMENT / CEMENTO | Concrete |
//! | not a valid track surface and FRICTION < 0.97 | Verge |
//! | anything else | Road |
//!
//! Only real traps become Sand: a loose verge with a sand sound but no
//! damping drives like a road edge with less grip, and off-road ground with
//! little grip is grass (the lesson from the rally converter).
//!
//! # Material classes (ours)
//!
//! A material is drivable / tarmac / grass-family when the physics surfaces
//! of the meshes that use it say so (most triangles win); a material only on
//! decor falls back to its name, then its diffuse map's name, then its other
//! maps' names (ASPH, TARMAC, ROAD -> tarmac; GRASS, ERBA, SAND, GRAVEL,
//! DIRT -> grass family).

use super::scan::{Role, Scan, Surface};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Road,
    Kerb,
    FlatKerb,
    Grass,
    Sand,
    Concrete,
    Pit,
    Verge,
    Turf,
    Wall,
}

impl Kind {
    /// cars drive on it (track, kerbs, verges, pits; grass / sand traps too:
    /// spawns may not sit there but the ground is real)
    pub fn drivable(self) -> bool {
        !matches!(self, Kind::Wall)
    }
    /// part of the race track proper (spawns, timing gates snap to these)
    pub fn track(self) -> bool {
        matches!(self, Kind::Road | Kind::Kerb | Kind::FlatKerb | Kind::Pit | Kind::Concrete)
    }
    pub fn name(self) -> &'static str {
        match self {
            Kind::Road => "road", Kind::Kerb => "kerb", Kind::FlatKerb => "flat kerb", Kind::Grass => "grass", Kind::Sand => "sand",
            Kind::Concrete => "concrete", Kind::Pit => "pit", Kind::Verge => "verge", Kind::Turf => "turf", Kind::Wall => "wall",
        }
    }
}

fn has(key: &str, words: &[&str]) -> bool {
    words.iter().any(|w| key.contains(w))
}

pub fn surface_kind(s: &Surface) -> Kind {
    let k = s.key.to_uppercase();
    let wav = s.wav.to_lowercase();
    if has(&k, &["WALL", "BARRIER", "FENCE", "GUARD", "RAIL", "ARMCO", "TYRES", "TIRES", "TECPRO", "MURO", "BARR"]) {
        return Kind::Wall;
    }
    if s.pitlane {
        return Kind::Pit;
    }
    if wav.contains("kerb") || has(&k, &["KERB", "CURB", "RUMBLE", "SAUSAGE", "CORDOL"]) {
        return if s.sin_height <= 0.0 && s.vibration_gain < 0.3 { Kind::FlatKerb } else { Kind::Kerb };
    }
    if wav.contains("extraturf") || has(&k, &["TURF", "CARPET", "ASTRO"]) {
        return Kind::Turf;
    }
    let sandy = wav.contains("sand") || wav.contains("gravel") || has(&k, &["SAND", "GRAVEL", "GHIAIA", "SABBIA", "TRAP"]);
    if sandy && s.damping > 0.0 {
        return Kind::Sand;
    }
    if sandy {
        return if s.friction >= 0.75 { Kind::Verge } else { Kind::Grass };
    }
    if wav.contains("grass") || has(&k, &["GRASS", "ERBA", "DIRT", "MUD", "TERRA", "SOIL"]) || (k.contains("GREEN") && s.friction < 0.8) {
        return Kind::Grass;
    }
    if has(&k, &["CONCRETE", "CONCR", "CEMENT", "CEMENTO", "CONC"]) {
        return Kind::Concrete;
    }
    if !s.valid_track && s.friction < 0.97 {
        return Kind::Verge;
    }
    Kind::Road
}

/// What an AC1 material draws.
#[derive(Clone, Copy, Debug, Default)]
pub struct MatClass {
    /// drawn on a drivable physics surface (road, kerb, verge, pit)
    pub drivable: bool,
    pub tarmac: bool,
    /// grass / sand / dirt family
    pub grassy: bool,
    /// the surface kind most of its physics triangles have
    pub kind: Option<Kind>,
}

fn name_class(text: &str) -> (bool, bool) {
    let t = text.to_uppercase();
    let tarmac = has(&t, &["ASPH", "TARMAC", "ROAD", "ASFALT", "STRADA", "PISTA"]);
    let grassy = has(&t, &["GRASS", "ERBA", "SAND", "GRAVEL", "DIRT", "MUD", "SABBIA", "GHIAIA", "TERRAIN"]);
    (tarmac, grassy && !tarmac)
}

/// Height within which a drawn ground triangle counts as lying on a physics
/// surface (ours: AC1 visual ground sits 0..10 cm over its invisible
/// collision mesh; Brands Hatch's pit-box floor `concrete_box` 0..2 cm over
/// PITS).
const LYING_ON_M: f32 = 0.25;

/// AC1 ground multilayers that are only drawn (their collision is a hidden
/// physics mesh) take the kind of the physics surface they lie on, when it
/// covers at least half of their upward-facing area: Brands Hatch's pit-box
/// floor `concrete_box` (a ksMultilayer named neither asphalt nor grass)
/// lies on PITS, and stock draws it with the road shader (DynamicTrack, its
/// albedo in the road range), not as a building. Votes are triangle counts
/// scaled to the material's own (so they compare with drawn-physics votes).
fn lying_on(scan: &Scan, kinds: &[Kind], votes: &mut [std::collections::HashMap<Kind, usize>]) {
    let multilayer = |g: usize| {
        let m = scan.material(g);
        m.shader.to_ascii_lowercase().starts_with("ksmultilayer") && m.blend_mode == 0 && !m.alpha_tested
    };
    let want: Vec<bool> = (0..scan.materials.len()).map(|g| votes[g].is_empty() && multilayer(g)).collect();
    if !want.iter().any(|w| *w) {
        return;
    }
    // every physics triangle of every layout, drivable or not
    let mut tris = Vec::new();
    for m in &scan.meshes {
        let Role::Physics { surface, .. } = m.role else { continue };
        let p = scan.positions(m);
        for t in scan.triangles(m) {
            tris.push(super::ground::Tri { v: [p[t[0] as usize], p[t[1] as usize], p[t[2] as usize]], surface: surface as u16 });
        }
    }
    let ground = super::ground::Ground::from_triangles(tris, vec![true; scan.surfaces.len()]);
    if ground.is_empty() {
        return;
    }
    let mut area: Vec<std::collections::HashMap<Kind, f64>> = vec![Default::default(); scan.materials.len()];
    let mut up_area = vec![0f64; scan.materials.len()];
    let mut ntri = vec![0usize; scan.materials.len()];
    for m in scan.meshes.iter().filter(|m| matches!(m.role, Role::Visual) && want[m.material]) {
        let p = scan.positions(m);
        for t in scan.triangles(m) {
            let (a, b, c) = (p[t[0] as usize], p[t[1] as usize], p[t[2] as usize]);
            let (u, v) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
            let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            ntri[m.material] += 1;
            if len < 1e-9 || n[1].abs() / len < 0.8 {
                continue;
            }
            let w = len as f64 / 2.0;
            up_area[m.material] += w;
            let q = [(a[0] + b[0] + c[0]) / 3.0, (a[1] + b[1] + c[1]) / 3.0, (a[2] + b[2] + c[2]) / 3.0];
            if let Some(h) = ground.nearest(q[0], q[1], q[2]).filter(|h| (h.y - q[1]).abs() <= LYING_ON_M) {
                *area[m.material].entry(kinds[h.surface]).or_default() += w;
            }
        }
    }
    for g in (0..scan.materials.len()).filter(|&g| want[g] && up_area[g] > 0.0) {
        let total: f64 = area[g].values().sum();
        if total < 0.5 * up_area[g] {
            continue;
        }
        for (k, a) in &area[g] {
            votes[g].insert(*k, ((a / up_area[g]) * ntri[g] as f64).round().max(1.0) as usize);
        }
    }
}

/// One class per global material of the scan.
pub fn material_classes(scan: &Scan) -> Vec<MatClass> {
    let kinds: Vec<Kind> = scan.surfaces.iter().map(surface_kind).collect();
    // triangles per (material, kind)
    let mut votes: Vec<std::collections::HashMap<Kind, usize>> = vec![Default::default(); scan.materials.len()];
    for m in &scan.meshes {
        if let Role::Physics { surface, drawn: true } = m.role {
            *votes[m.material].entry(kinds[surface]).or_default() += scan.mesh(m).index_count() / 3;
        }
    }
    lying_on(scan, &kinds, &mut votes);
    (0..scan.materials.len())
        .map(|g| {
            let mat = scan.material(g);
            if let Some((&k, _)) = votes[g].iter().max_by_key(|(k, n)| (**n, k.name())) {
                return MatClass {
                    drivable: k.drivable() && k != Kind::Grass && k != Kind::Sand,
                    tarmac: matches!(k, Kind::Road | Kind::Pit),
                    grassy: matches!(k, Kind::Grass | Kind::Sand),
                    kind: Some(k),
                };
            }
            // the material's own name decides first, then its diffuse map,
            // then the other maps (a grass multilayer may carry a
            // `tarmac-detail` layer: Brands Hatch `grass_ext`)
            let diffuse = mat.texture("txDiffuse").unwrap_or("").to_string();
            let others: String = mat.textures.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>().join(" ");
            let (tarmac, grassy) = [scan.materials[g].name.clone(), diffuse, others].iter().map(|s| name_class(s)).find(|c| c.0 || c.1).unwrap_or((false, false));
            MatClass { drivable: false, tarmac, grassy, kind: None }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(key: &str, wav: &str, friction: f32, damping: f32, valid: bool) -> Surface {
        Surface { key: key.into(), wav: wav.into(), friction, damping, valid_track: valid, ..Default::default() }
    }

    #[test]
    fn kinds() {
        assert_eq!(surface_kind(&s("ROAD", "", 1.0, 0.0, true)), Kind::Road);
        assert_eq!(surface_kind(&s("GRASS", "grass.wav", 0.6, 0.0, false)), Kind::Grass);
        assert_eq!(surface_kind(&s("SAND", "sand.wav", 0.8, 0.1, false)), Kind::Sand);
        // a loose verge with a sand sound but no damping is not a trap
        assert_eq!(surface_kind(&s("DIRTROAD", "sand.wav", 0.85, 0.0, false)), Kind::Verge);
        assert_eq!(surface_kind(&s("OFFROAD", "sand.wav", 0.6, 0.0, false)), Kind::Grass);
        let mut k = s("KERB", "kerb.wav", 0.95, 0.0, true);
        k.sin_height = 0.03;
        assert_eq!(surface_kind(&k), Kind::Kerb);
        assert_eq!(surface_kind(&s("FLATKERB", "kerb.wav", 0.95, 0.0, true)), Kind::FlatKerb);
        let mut p = s("PITLANE", "", 0.98, 0.0, true);
        p.pitlane = true;
        assert_eq!(surface_kind(&p), Kind::Pit);
        assert_eq!(surface_kind(&s("OUTLAY", "", 0.9, 0.0, false)), Kind::Verge);
        assert_eq!(surface_kind(&s("WALL", "", 1.0, 0.0, false)), Kind::Wall);
        assert_eq!(surface_kind(&s("CONCR", "", 0.95, 0.0, true)), Kind::Concrete);
        assert_eq!(surface_kind(&s("ERBA", "grass.wav", 0.85, 0.0, false)), Kind::Grass);
    }
}
