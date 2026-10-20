//! AC1 `data.acd`: the car's `data\` folder packed into one file.
//!
//! Layout (read off the AC1 install; docs/FORMATS.md, "AC1 data.acd"):
//! an optional 8-byte header whose first i32 is negative (-1111 on newer
//! packs), then entries until the end: `i32 name length, name (latin-1),
//! i32 n, n x i32` where the low byte of each i32 is one encrypted byte.
//!
//! Encryption: byte i of a file is `plain[i] + key[i % key.len()]` (mod 256),
//! where `key` is the ASCII text of eight numbers 0..255 joined by `-`,
//! each a function of the lower-cased car folder name ([`key`]).
//!
//! How this was found: twelve AC1 cars in the user's install ship both
//! `data.acd` and an unpacked `data\` folder. Subtracting the plain files
//! from the encrypted bytes gives a repeating ASCII key per car; the eight
//! numbers were then fitted against the folder names by a brute-force
//! search over small loop formulas, and the fit was checked on every
//! `data.acd` in the install (each must decrypt to printable text with a
//! `[HEADER]`/`[BASIC]` `car.ini`).

use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::path::Path;

/// C-style truncating division / remainder on i32 (the key loops wrap at 32 bits).
fn tdiv(a: i32, b: i32) -> i32 {
    if b == 0 { 0 } else { a.wrapping_div(b) }
}
fn trem(a: i32, b: i32) -> i32 {
    if b == 0 { 0 } else { a.wrapping_rem(b) }
}

/// The eight key numbers for a car folder name.
pub fn key_parts(folder: &str) -> [u8; 8] {
    let s = folder.to_lowercase();
    let b: Vec<i32> = s.bytes().map(|c| c as i32).collect();
    let n = b.len();
    // 0: byte sum
    let k0 = b.iter().fold(0i32, |a, &x| a.wrapping_add(x));
    // 1: a = a * b[i] - b[i + 1] over even i
    let mut k1 = 0i32;
    let mut i = 0;
    while i + 1 < n {
        k1 = k1.wrapping_mul(b[i]).wrapping_sub(b[i + 1]);
        i += 2;
    }
    let k2 = key2(&b);
    // 3: 0x1683 minus every byte after the first
    let k3 = b.iter().skip(1).fold(0x1683i32, |a, &x| a.wrapping_sub(x));
    let k4 = key4(&b);
    // 5: 101 minus the bytes at even i, while i + 2 < n
    let mut k5 = 101i32;
    let mut i = 0;
    while i + 2 < n {
        k5 = k5.wrapping_sub(b[i]);
        i += 2;
    }
    // 6: 171 reduced modulo the bytes at even i, while i + 2 < n
    let mut k6 = 171i32;
    let mut i = 0;
    while i + 2 < n {
        k6 = trem(k6, b[i]);
        i += 2;
    }
    // 7: a = a / b[i] + b[i + 1]
    let mut k7 = 0i32;
    for i in 0..n.saturating_sub(1) {
        k7 = tdiv(k7, b[i]).wrapping_add(b[i + 1]);
    }
    [k0, k1, k2, k3, k4, k5, k6, k7].map(|k| (k & 0xff) as u8)
}

/// 2: over i = 1, 4, 7, ... while i < n - 3: a = a * b[i] / (b[i + 1] + 27) - 27 - b[i - 1]
fn key2(b: &[i32]) -> i32 {
    let mut a = KEY2_INIT;
    let mut i = 1;
    while i + 3 < b.len() {
        a = tdiv(a.wrapping_mul(b[i]), b[i + 1] + 27).wrapping_sub(27).wrapping_sub(b[i - 1]);
        i += 3;
    }
    a
}
const KEY2_INIT: i32 = 0;

/// 4: over i = 1, 5, 9, ... while i < n - 4: a = (b[i] + 15) * a * (b[i - 1] + 15) + 22, from 66
fn key4(b: &[i32]) -> i32 {
    let mut a = 66i32;
    let mut i = 1;
    while i + 4 < b.len() {
        a = (b[i] + 15).wrapping_mul(a).wrapping_mul(b[i - 1] + 15).wrapping_add(22);
        i += 4;
    }
    a
}

/// The key text for a car folder name: `"n0-n1-...-n7"`.
pub fn key(folder: &str) -> String {
    key_parts(folder).iter().map(|k| k.to_string()).collect::<Vec<_>>().join("-")
}

/// Every entry of a data.acd, decrypted: (file name as stored, bytes).
pub fn decrypt(acd: &[u8], folder: &str) -> Result<Vec<(String, Vec<u8>)>> {
    let k = key(folder).into_bytes();
    let rd = |o: usize| -> Result<i32> {
        acd.get(o..o + 4).map(|x| i32::from_le_bytes(x.try_into().unwrap())).context("data.acd truncated")
    };
    let mut o = 0;
    if rd(0)? < 0 {
        o = 8;
    }
    let mut out = Vec::new();
    while o < acd.len() {
        let nl = rd(o)?;
        if !(1..=4096).contains(&nl) {
            bail!("data.acd: bad name length {nl} at {o:#x}");
        }
        o += 4;
        let name: String = acd.get(o..o + nl as usize).context("data.acd truncated")?.iter().map(|&c| c as char).collect();
        o += nl as usize;
        let n = rd(o)?;
        if n < 0 || o + 4 + n as usize * 4 > acd.len() {
            bail!("data.acd: {name}: bad size {n}");
        }
        o += 4;
        let data: Vec<u8> = (0..n as usize).map(|i| acd[o + 4 * i].wrapping_sub(k[i % k.len()])).collect();
        o += 4 * n as usize;
        out.push((name, data));
    }
    Ok(out)
}

fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

/// The car's data files: lower-case file name -> latin-1 text. `data.acd`
/// wins when present; otherwise every `data\*.ini|lut|rto`.
pub fn read_car_data(car_dir: &Path) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    let acd = car_dir.join("data.acd");
    if acd.is_file() {
        let folder = car_dir.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        let b = std::fs::read(&acd).with_context(|| format!("read {}", acd.display()))?;
        for (name, data) in decrypt(&b, &folder)? {
            out.insert(name.to_lowercase(), latin1(&data));
        }
        return Ok(out);
    }
    let dir = car_dir.join("data");
    for e in std::fs::read_dir(&dir).with_context(|| format!("{} has neither data.acd nor data\\", car_dir.display()))?.flatten() {
        let p = e.path();
        let ext = p.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
        if p.is_file() && matches!(ext.as_str(), "ini" | "lut" | "rto") {
            out.insert(e.file_name().to_string_lossy().to_lowercase(), latin1(&std::fs::read(&p)?));
        }
    }
    Ok(out)
}

/// Plausible decrypted text (the library scan's check).
pub fn looks_plain(t: &str) -> bool {
    let bad = t.chars().filter(|&c| (c as u32) < 32 && !matches!(c, '\t' | '\r' | '\n')).count();
    bad * 200 <= t.len().max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Keys recovered from cars that ship data.acd and data\ side by side.
    #[test]
    fn recovered_keys() {
        for (f, k) in [
            ("chevrolet_impala", "159-179-151-71-10-114-72-98"),
            ("jvs_mitsubishi_evo6_tme", "119-191-93-118-218-226-11-102"),
            ("matt_field_c6_vette", "152-208-194-88-54-209-8-102"),
            ("tommykaira_zzs_pub", "170-63-54-77-226-0-55-99"),
            ("rtp_honda_integra_dc2_spoon2", "32-206-30-213-10-47-7-51"),
            ("chevrolet_corvette_1969_6.2l_ohv_v8_engine", "190-235-96-40-62-58-15-102"),
        ] {
            assert_eq!(key(f), k, "{f}");
        }
    }

    #[test]
    fn round_trip() {
        let k = key("some_car");
        let plain = b"[HEADER]\r\nVERSION=1\r\n";
        let mut acd = Vec::new();
        acd.extend_from_slice(&7i32.to_le_bytes());
        acd.extend_from_slice(b"car.ini");
        acd.extend_from_slice(&(plain.len() as i32).to_le_bytes());
        for (i, &c) in plain.iter().enumerate() {
            acd.extend_from_slice(&(c.wrapping_add(k.as_bytes()[i % k.len()]) as i32).to_le_bytes());
        }
        let d = decrypt(&acd, "Some_Car").unwrap();
        assert_eq!(d[0].0, "car.ini");
        assert_eq!(d[0].1, plain);
    }
}
