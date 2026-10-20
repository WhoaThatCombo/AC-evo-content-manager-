//! The track side: the AC1 scan and ground queries every track stage reads
//! (`scan`, `ground`, `classes`), and the stages that write the EVO track.

pub mod dyntrack;

pub mod ao;
pub mod bakes;
pub mod ai;
pub mod audio;
pub mod build;
pub mod bvh;
pub mod classes;
pub mod csplights;
pub mod csptrees;
pub mod declare;
pub mod drs;
pub mod gameplay;
pub mod geo;
pub mod grass;
pub mod ground;
pub mod ids;
pub mod irradiance;
pub mod look;
pub mod materials;
pub mod meshes;
pub mod pigment;
pub mod pits;
pub mod scan;
pub mod scene;
pub mod spawns;
pub mod surfaces;
pub mod textures;
pub mod tvcams;
pub mod uiimages;
pub mod vegcat;
pub mod vegetation;

pub use ground::Ground;
pub use scan::{Scan, Source};

/// A surface hit at a point (ground queries, `ground::Ground`).
#[derive(Clone, Copy, Debug)]
pub struct Hit {
    /// world height of the surface
    pub y: f32,
    /// index into `Scan::surfaces` (the AC1 surfaces.ini key hit)
    pub surface: usize,
    /// drivable as the ground was built (`Ground::build`'s per-surface flags;
    /// `aceconv track` passes track surfaces: road, kerb, pit, concrete, verge)
    pub drivable: bool,
    /// unit normal of the triangle hit, pointing up
    pub normal: [f32; 3],
}

/// One row of data/countries.tsv.
#[derive(Clone, Debug)]
pub struct Country {
    pub alpha2: String,
    pub numeric: u16,
    pub name: String,
    /// UN M49 sub-region code
    pub subregion: String,
    pub continent: String,
    /// IANA zones (zone1970.tab) covering the country, most populous first
    pub timezones: Vec<String>,
    /// ISO 3166-1 alpha-3
    pub alpha3: String,
}

/// ISO 3166-1 countries with their UN M49 sub-region and continent.
pub fn countries() -> Vec<Country> {
    include_str!("../../data/countries.tsv")
        .lines()
        .filter(|l| !l.starts_with('#') && !l.starts_with("alpha2"))
        .filter_map(|l| {
            let c: Vec<&str> = l.split('\t').collect();
            (c.len() >= 5).then(|| Country { alpha2: c[0].into(), numeric: c[1].parse().unwrap_or(0), name: c[2].into(), subregion: c[3].into(), continent: c[4].into(),
                timezones: c.get(5).map(|z| z.split(',').filter(|s| !s.is_empty()).map(String::from).collect()).unwrap_or_default(),
                alpha3: c.get(6).map(|s| s.to_string()).unwrap_or_default() })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn countries_load() {
        let c = super::countries();
        assert!(c.len() >= 240);
        assert!(c.iter().any(|x| x.alpha2 == "IT" && x.continent == "Europe" && x.timezones == ["Europe/Rome"]));
        assert!(c.iter().any(|x| x.alpha2 == "IT" && x.alpha3 == "ITA"));
        assert!(c.iter().all(|x| x.alpha3.len() == 3));
    }
}
