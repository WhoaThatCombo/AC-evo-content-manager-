//! Geography of a converted track: nation, continent, time zone, coordinates.
//!
//! Game facts (stock `system\tracks.table` rows of the 18 Kunos circuits):
//! - nation: three-letter codes, ISO 3166-1 alpha-3 (GBR, ITA, USA, JPN,
//!   BEL, DEU, FRA, AUT) except South Africa, which the game writes RSA;
//! - continent: "Europe", "North America", "Asia", "Africa", and
//!   "Australia" for Oceania (Mount Panorama);
//! - coordinates: {f1 longitude, f2 latitude} as doubles;
//! - time zone: the `TimeZone` enum of the game's `DateTime` proto
//!   ([`TimeZone`]); stock uses the country's own value where one exists
//!   (Italy 2 ... Japan 16) and the US zones by region.
//!
//! The country comes from `ui_track.json` "country" (a name or a code),
//! matched against data/countries.tsv (ISO 3166-1 / UN M49) with a few
//! common aliases. Countries without an enum value of their own get the
//! value whose rules match (CET countries: Germany; WET: United Kingdom) or
//! else the nearest standard offset, estimated from the longitude (ours).

use super::{countries, Country};

/// The game's `TimeZone` enum (ACECM `protos\DateTime.proto.txt`), with the
/// standard UTC offset (hours) of each zone.
pub const TIME_ZONES: [(u32, &str, f32); 18] = [
    (0, "UTC", 0.0), (1, "LocalSystemTime", f32::NAN), (2, "Italy", 1.0), (3, "Germany", 1.0), (4, "Belgium", 1.0),
    (5, "Spain_Madrid", 1.0), (6, "Spain_Barcelona", 1.0), (7, "France", 1.0), (8, "Netherlands", 1.0), (9, "UnitedKingdom", 0.0),
    (10, "Hungary", 1.0), (11, "Austria", 1.0), (12, "Australia_Bathurst", 10.0), (13, "US_Texas", -6.0), (14, "South_Africa", 2.0),
    (15, "Pacific_North_America", -8.0), (16, "Japan", 9.0), (17, "EasternStandardTime", -5.0),
];

#[derive(Clone, Debug)]
pub struct Geo {
    pub country: Option<Country>,
    /// the game's nation code (alpha-3, RSA for South Africa)
    pub nation: String,
    pub continent: String,
    pub timezone: u32,
    pub latitude: f64,
    pub longitude: f64,
    /// false when the AC1 track gave no geotags (coordinates are then the
    /// country's time-zone meridian at the equator, a neutral sun)
    pub located: bool,
}

const ALIASES: [(&str, &str); 22] = [
    ("usa", "US"), ("united states", "US"), ("united states of america", "US"), ("america", "US"), ("uk", "GB"), ("united kingdom", "GB"),
    ("great britain", "GB"), ("england", "GB"), ("scotland", "GB"), ("wales", "GB"), ("northern ireland", "GB"), ("russia", "RU"),
    ("south korea", "KR"), ("korea", "KR"), ("czech republic", "CZ"), ("holland", "NL"), ("the netherlands", "NL"), ("uae", "AE"),
    ("macau", "MO"), ("vietnam", "VN"), ("turkey", "TR"), ("iran", "IR"),
];

/// Match an AC1 country text ("Italy", "USA", "GB", "Deutschland"...).
pub fn find_country(text: &str) -> Option<Country> {
    // "U.S.A." / "U.K." / "Italy." as their letters
    let t: String = text.trim().to_lowercase().chars().filter(|c| *c != '.').collect();
    let t = t.trim().to_string();
    if t.is_empty() {
        return None;
    }
    let all = countries();
    let code = ALIASES.iter().find(|(a, _)| *a == t).map(|(_, c)| c.to_string());
    if let Some(c) = code {
        return all.into_iter().find(|x| x.alpha2 == c);
    }
    let local = [("deutschland", "DE"), ("italia", "IT"), ("espana", "ES"), ("españa", "ES"), ("nippon", "JP"), ("osterreich", "AT"), ("österreich", "AT"), ("belgique", "BE"), ("schweiz", "CH"), ("suisse", "CH")];
    if let Some((_, c)) = local.iter().find(|(a, _)| *a == t) {
        return all.into_iter().find(|x| x.alpha2 == *c);
    }
    all.iter().find(|x| x.alpha2.to_lowercase() == t || x.alpha3.to_lowercase() == t || x.name.to_lowercase() == t).cloned()
        .or_else(|| all.iter().filter(|x| x.name.len() >= 4).find(|x| t.contains(&x.name.to_lowercase()) || x.name.to_lowercase().starts_with(&t)).cloned())
}

/// The game's time-zone value for a country (alpha-2) at a longitude.
pub fn timezone(alpha2: &str, lon: f64, lat: f64) -> u32 {
    match alpha2 {
        "IT" | "SM" | "VA" => 2,
        "DE" => 3,
        "BE" => 4,
        // Catalonia (Barcelona) has its own value; the rest of Spain Madrid's
        "ES" => if lon > 0.2 && lat > 40.5 { 6 } else { 5 },
        "FR" | "MC" | "AD" => 7,
        "NL" => 8,
        "GB" | "IE" | "PT" | "IM" | "JE" | "GG" | "IS" | "FO" => 9,
        "HU" => 10,
        "AT" => 11,
        "AU" | "NZ" => 12,
        "ZA" | "LS" | "SZ" | "BW" | "NA" | "ZW" => 14,
        "JP" => 16,
        // continental US and Canada by region
        "US" | "CA" => if lon > -87.5 { 17 } else if lon > -104.0 { 13 } else if lon > -115.0 { 13 } else { 15 },
        "MX" => 13,
        // the rest of central Europe keeps CET / CEST like Germany
        "CH" | "LI" | "LU" | "DK" | "NO" | "SE" | "PL" | "CZ" | "SK" | "SI" | "HR" | "BA" | "RS" | "ME" | "MK" | "AL" | "MT" => 3,
        _ => nearest_offset(lon),
    }
}

/// The zone whose standard offset is nearest the longitude's (lon / 15 h).
fn nearest_offset(lon: f64) -> u32 {
    let off = (lon / 15.0) as f32;
    // a country without its own value: prefer the generic zones
    let candidates = [0u32, 3, 12, 13, 14, 15, 16, 17];
    *candidates.iter().min_by(|&&a, &&b| {
        let da = (TIME_ZONES[a as usize].2 - off).abs();
        let db = (TIME_ZONES[b as usize].2 - off).abs();
        da.partial_cmp(&db).unwrap()
    }).unwrap()
}

/// The game's continent word for a countries.tsv continent.
pub fn continent_word(c: &str) -> String {
    match c {
        "Oceania" => "Australia".into(),
        x => x.into(),
    }
}

/// Geography from the AC1 ui fields.
pub fn of(country_text: &str, geo: Option<(f64, f64)>) -> Geo {
    let c = find_country(country_text);
    let (lat, lon, located) = match geo {
        Some((la, lo)) => (la, lo, true),
        None => {
            // no geotags: the country's zone meridian, at the equator
            let tz = c.as_ref().map(|c| timezone(&c.alpha2, 0.0, 0.0)).unwrap_or(0);
            let off = TIME_ZONES[tz as usize].2;
            (0.0, if off.is_finite() { off as f64 * 15.0 } else { 0.0 }, false)
        }
    };
    let nation = c.as_ref().map(|c| if c.alpha2 == "ZA" { "RSA".to_string() } else { c.alpha3.clone() }).unwrap_or_default();
    let continent = c.as_ref().map(|c| continent_word(&c.continent)).unwrap_or_default();
    let timezone = match &c {
        Some(c) => timezone(&c.alpha2, lon, lat),
        None => nearest_offset(lon),
    };
    Geo { country: c, nation, continent, timezone, latitude: lat, longitude: lon, located }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stock_like() {
        // the stock rows: Oulton Park GBR / Europe / 9, Kyalami RSA / Africa / 14, Laguna Seca USA / 15, COTA 13, Road Atlanta 17
        let g = of("UK", Some((53.18, -2.61)));
        assert_eq!((g.nation.as_str(), g.continent.as_str(), g.timezone), ("GBR", "Europe", 9));
        let g = of("South Africa", Some((-25.99, 28.07)));
        assert_eq!((g.nation.as_str(), g.continent.as_str(), g.timezone), ("RSA", "Africa", 14));
        assert_eq!(of("USA", Some((36.58, -121.75))).timezone, 15);
        assert_eq!(of("United States", Some((30.13, -97.64))).timezone, 13);
        assert_eq!(of("USA", Some((34.15, -83.81))).timezone, 17);
        assert_eq!(of("Australia", Some((-33.44, 149.55))).continent, "Australia");
        assert_eq!(of("Italy", None).timezone, 2);
        assert_eq!(of("Japan", Some((35.37, 138.93))).timezone, 16);
        assert_eq!(of("Spain", Some((41.57, 2.26))).timezone, 6);
        assert_eq!(of("Poland", Some((52.0, 17.0))).timezone, 3);
        assert_eq!(of("Malaysia", Some((2.76, 101.74))).timezone, 16);
        assert!(!of("Nowhere", None).located);
    }
}
