//! Stock statistics: what the game's own content looks like, measured, so
//! every package can check its output against the stock distribution
//! instead of against any earlier tool's output.
//!
//! `aceconv stockstats <content.kspkg> <out dir>` writes `stockstats.json`
//! ([`Stock`]); `aceconv stockstats compare <converted> <stockstats.json>`
//! measures one converted car package / track folder the same way and
//! prints every metric outside the stock range.

pub mod albedo;
pub mod trackmeasure;
pub mod stockstats;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Named numeric metrics of one item (a car, a track).
pub type Metrics = BTreeMap<String, f64>;

/// The spread of one metric over the stock items.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Range {
    pub n: usize,
    pub min: f64,
    pub p10: f64,
    pub p50: f64,
    pub p90: f64,
    pub max: f64,
}

impl Range {
    pub fn of(v: &[f64]) -> Option<Range> {
        let mut s: Vec<f64> = v.iter().copied().filter(|x| x.is_finite()).collect();
        if s.is_empty() {
            return None;
        }
        s.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let q = |p: f64| s[((s.len() - 1) as f64 * p).round() as usize];
        Some(Range { n: s.len(), min: s[0], p10: q(0.1), p50: q(0.5), p90: q(0.9), max: s[s.len() - 1] })
    }
}

/// Metric name -> range over the stock items of one kind.
pub type Ranges = BTreeMap<String, Range>;

/// Summaries of per-item metrics.
pub fn ranges(items: &BTreeMap<String, Metrics>) -> Ranges {
    let mut by: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for m in items.values() {
        for (k, v) in m {
            by.entry(k.clone()).or_default().push(*v);
        }
    }
    by.into_iter().filter_map(|(k, v)| Range::of(&v).map(|r| (k, r))).collect()
}

/// How one value sits against a stock range.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Fit {
    /// inside p10..p90
    Typical,
    /// inside min..max, outside p10..p90
    Unusual,
    /// outside min..max
    Outside,
}

pub fn fit(r: &Range, v: f64) -> Fit {
    // serde_json's float parsing can be one ulp off: an f32 value equal to
    // the stock maximum must not read as outside it
    let e = 1e-9 * (1.0 + v.abs());
    if v < r.min - e || v > r.max + e {
        Fit::Outside
    } else if v < r.p10 - e || v > r.p90 + e {
        Fit::Unusual
    } else {
        Fit::Typical
    }
}

/// The whole stockstats.json.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Stock {
    pub version: u32,
    /// content.kspkg size and modification time (seconds)
    pub content: (u64, u64),
    pub entries: usize,
    /// global facts (id rules, texture header table, limits, ...)
    pub facts: serde_json::Value,
    pub cars: BTreeMap<String, Metrics>,
    pub tracks: BTreeMap<String, Metrics>,
    pub car_ranges: Ranges,
    pub track_ranges: Ranges,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_and_fit() {
        let r = Range::of(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0]).unwrap();
        assert_eq!((r.min, r.p10, r.p50, r.p90, r.max), (1.0, 2.0, 6.0, 10.0, 11.0));
        assert_eq!(fit(&r, 5.0), Fit::Typical);
        assert_eq!(fit(&r, 1.5), Fit::Unusual);
        assert_eq!(fit(&r, 12.0), Fit::Outside);
    }
}
