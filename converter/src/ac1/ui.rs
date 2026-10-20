//! AC1 `ui_track.json` (and other AC1 ui json): real files are often not
//! strict JSON - a UTF-8 BOM, raw newlines and tabs inside strings, `//`
//! and `/* */` comments, trailing commas. [`lenient`] repairs those before
//! parsing; [`TrackUi`] holds the fields the track stages use.

use serde_json::Value;

/// Parse AC1-style JSON: BOM, comments, raw control characters in strings
/// and trailing commas are tolerated. `None` when it still is not JSON.
pub fn lenient(text: &str) -> Option<Value> {
    let t = text.trim_start_matches('\u{feff}');
    if let Ok(v) = serde_json::from_str(t) {
        return Some(v);
    }
    let c: Vec<char> = t.chars().collect();
    let mut out = String::with_capacity(t.len());
    let (mut i, mut in_str) = (0, false);
    while i < c.len() {
        let ch = c[i];
        if in_str {
            match ch {
                '\\' => {
                    out.push(ch);
                    if let Some(&n) = c.get(i + 1) {
                        // a lone backslash before a character JSON does not escape: keep it literal
                        if "\"\\/bfnrtu".contains(n) {
                            out.push(n);
                        } else {
                            out.push('\\');
                            out.push(n);
                        }
                        i += 1;
                    }
                }
                '"' => {
                    in_str = false;
                    out.push(ch);
                }
                '\n' => out.push_str("\\n"),
                '\r' => {}
                '\t' => out.push_str("\\t"),
                x if (x as u32) < 0x20 => {}
                x => out.push(x),
            }
            i += 1;
            continue;
        }
        if ch == '"' {
            in_str = true;
            out.push(ch);
        } else if ch == '/' && c.get(i + 1) == Some(&'/') {
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
            continue;
        } else if ch == '/' && c.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < c.len() && !(c[i] == '*' && c[i + 1] == '/') {
                i += 1;
            }
            i += 2;
            continue;
        } else if ch == ',' {
            // drop a comma that only whitespace separates from } or ]
            let mut j = i + 1;
            while j < c.len() && c[j].is_whitespace() {
                j += 1;
            }
            if j < c.len() && (c[j] == '}' || c[j] == ']') {
                i += 1;
                continue;
            }
            out.push(ch);
        } else {
            out.push(ch);
        }
        i += 1;
    }
    serde_json::from_str(&out).ok()
}

/// What a track stage needs from `ui_track.json`.
#[derive(Clone, Debug, Default)]
pub struct TrackUi {
    pub name: String,
    pub country: String,
    pub city: String,
    /// metres, when the file gives a length
    pub length_m: Option<f32>,
    pub pitboxes: Option<u32>,
    /// (latitude, longitude) in degrees, north / east positive
    pub geo: Option<(f64, f64)>,
    /// metres above sea level, when the geotags carry it
    pub altitude: Option<f64>,
    pub tags: Vec<String>,
}

fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

/// "3.7 km", "3700m", "3700", "2.3 miles" -> metres.
pub fn length_m(s: &str) -> Option<f32> {
    let l = s.trim().to_lowercase().replace(',', ".");
    let num: String = l.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let v: f32 = num.parse().ok()?;
    let rest = l[num.len()..].trim();
    let m = if rest.starts_with("km") {
        v * 1000.0
    } else if rest.starts_with("mi") {
        v * 1609.344
    } else if rest.starts_with('m') || rest.is_empty() {
        // a bare small number is kilometres ("3.7")
        if rest.is_empty() && v < 100.0 { v * 1000.0 } else { v }
    } else {
        return None;
    };
    (m > 10.0 && m < 1_000_000.0).then_some(m)
}

/// One geotag: decimal ("33.53", "-86.62", "33.53 N") or degrees /
/// minutes / seconds ("33°32'08\" N", "86 37 12 W"). `lat` says which axis
/// a bare number is.
pub fn coordinate(s: &str) -> Option<(f64, bool)> {
    let u = s.trim().to_uppercase();
    let mut neg = u.starts_with('-');
    let mut axis_lat: Option<bool> = None;
    for (c, lat, n) in [('N', true, false), ('S', true, true), ('E', false, false), ('W', false, true)] {
        if u.contains(c) && !u.contains("LAT") && !u.contains("LON") {
            axis_lat = Some(lat);
            neg ^= n;
        }
    }
    let nums: Vec<f64> = u.split(|c: char| !(c.is_ascii_digit() || c == '.')).filter(|x| !x.is_empty()).filter_map(|x| x.parse().ok()).collect();
    let v = match nums.len() {
        0 => return None,
        1 => nums[0],
        2 => nums[0] + nums[1] / 60.0,
        _ => nums[0] + nums[1] / 60.0 + nums[2] / 3600.0,
    };
    let v = if neg { -v } else { v };
    Some((v, axis_lat.unwrap_or(true)))
}

impl TrackUi {
    pub fn from_json(v: &Value) -> TrackUi {
        let mut u = TrackUi { name: text_of(&v["name"]), country: text_of(&v["country"]), city: text_of(&v["city"]), ..Default::default() };
        u.length_m = length_m(&text_of(&v["length"]));
        u.pitboxes = text_of(&v["pitboxes"]).trim().parse::<f32>().ok().filter(|n| *n >= 1.0 && *n < 1000.0).map(|n| n as u32);
        u.tags = v["tags"].as_array().map(|a| a.iter().map(text_of).collect()).unwrap_or_default();
        if let Some(g) = v["geotags"].as_array() {
            let g: Vec<String> = g.iter().map(text_of).collect();
            let c: Vec<Option<(f64, bool)>> = g.iter().map(|s| coordinate(s)).collect();
            if let (Some(Some(a)), Some(Some(b))) = (c.first(), c.get(1)) {
                // the axis letters win; otherwise AC1 order is latitude, longitude
                let (lat, lon) = if !a.1 && b.1 { (b.0, a.0) } else { (a.0, b.0) };
                if lat.abs() <= 90.0 && lon.abs() <= 180.0 && (lat != 0.0 || lon != 0.0) {
                    u.geo = Some((lat, lon));
                }
            }
            if let Some(Some(alt)) = g.get(2).map(|s| s.trim().trim_end_matches('m').trim().parse::<f64>().ok()) {
                u.altitude = Some(alt);
            }
        }
        u
    }

    pub fn read(path: &std::path::Path) -> Option<TrackUi> {
        let b = std::fs::read(path).ok()?;
        lenient(&String::from_utf8_lossy(&b)).map(|v| TrackUi::from_json(&v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repairs_ac1_json() {
        let t = "\u{feff}{\n  \"name\": \"Barber\",\n  // comment\n  \"description\": \"two\nlines\",\n  \"geotags\": [\"33.53 N\", \"86°37'12\\\" W\", \"180\"],\n  \"length\": \"3.7 km\",\n  \"pitboxes\": \"30\",\n}";
        let v = lenient(t).unwrap();
        let u = TrackUi::from_json(&v);
        assert_eq!(u.name, "Barber");
        assert_eq!(u.pitboxes, Some(30));
        assert_eq!(u.length_m, Some(3700.0));
        let (lat, lon) = u.geo.unwrap();
        assert!((lat - 33.53).abs() < 1e-9 && (lon + 86.62).abs() < 1e-6, "{lat} {lon}");
        assert_eq!(u.altitude, Some(180.0));
    }

    #[test]
    fn coordinates() {
        assert_eq!(coordinate("-86.5").unwrap().0, -86.5);
        assert!((coordinate("50 26 0 N").unwrap().0 - 50.4333).abs() < 1e-3);
        assert!(!coordinate("5.97 E").unwrap().1);
        assert_eq!(length_m("2.3 miles").map(|m| m.round()), Some(3701.0));
    }
}
