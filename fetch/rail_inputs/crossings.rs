//! Normalize FRA and Transport Canada publisher fields to the shared crossing movement.
#[path = "fra_crossings.rs"]
mod fra;

use crate::{
    railways::{config::FraConversion, types::Crossing},
    year::Input,
};
use chrono::NaiveDate;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{self, Read},
    path::Path,
};

#[cfg(test)]
use fra::fra_end_periods;

const TC_OSM_OFFSET: i64 = 1_000_000_000;

struct HashingReader<R> {
    inner: R,
    digest: Sha256,
}

impl<R: Read> HashingReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            digest: Sha256::new(),
        }
    }

    fn finish_hex(mut self) -> io::Result<String> {
        io::copy(&mut self, &mut io::sink())?;
        Ok(format!("{:x}", self.digest.finalize()))
    }
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let count = self.inner.read(buf)?;
        self.digest.update(&buf[..count]);
        Ok(count)
    }
}

pub(super) fn column(headers: &[String], name: &str, path: &Path) -> Result<usize, String> {
    headers
        .iter()
        .position(|h| h == name)
        .ok_or_else(|| format!("{} lacks a {name} column", path.display()))
}

pub(super) fn num(record: &csv::StringRecord, idx: usize) -> Option<f64> {
    let text = record.get(idx)?.trim();
    if text.is_empty() || text == "-" {
        return None;
    }
    text.parse::<f64>().ok().filter(|value| value.is_finite())
}

pub(super) fn non_neg_num(record: &csv::StringRecord, idx: usize) -> Option<f64> {
    num(record, idx).filter(|value| *value >= 0.0)
}

pub(super) fn flat_end_periods(daily: f64) -> [f64; 3] {
    [daily * 12.0 / 24.0, daily * 4.0 / 24.0, daily * 8.0 / 24.0]
}

fn parse_fra_date(text: &str) -> Result<NaiveDate, String> {
    let text = text.trim();
    if let Ok(day) = NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        return Ok(day);
    }
    if text.len() >= 10
        && let Ok(day) = NaiveDate::parse_from_str(&text[..10], "%Y-%m-%d")
    {
        return Ok(day);
    }
    NaiveDate::parse_from_str(text, "%m/%d/%Y")
        .map_err(|err| format!("invalid FRA revision date {text:?}: {err}"))
}

fn stream_fra(
    path: &Path,
    source_id: u16,
    expected: Option<(&str, &str, NaiveDate)>,
) -> Result<Vec<Crossing>, String> {
    let file = File::open(path).map_err(|err| format!("opens {}: {err}", path.display()))?;
    let mut reader = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .from_reader(HashingReader::new(file));
    let headers: Vec<String> = reader
        .headers()
        .map_err(|err| format!("reads {} headers: {err}", path.display()))?
        .iter()
        .map(|h| h.trim().to_string())
        .collect();
    let cols = fra::FraColumns::from_headers(&headers, path)?;
    // Deduplicate repeated Crossing IDs (keeping the latest Revision Date, then the later row).
    let mut latest: BTreeMap<String, (Option<NaiveDate>, usize, Option<Crossing>)> =
        BTreeMap::new();
    for (row, record) in reader.records().enumerate() {
        let record = record.map_err(|err| format!("reads {}: {err}", path.display()))?;
        let rev_text = record.get(cols.rev).unwrap_or("").trim();
        let rev_date = if rev_text.is_empty() {
            None
        } else {
            let rev = parse_fra_date(rev_text)?;
            if let Some((_, _, cutoff)) = expected
                && rev > cutoff
            {
                return Err(format!(
                    "{}: row {} revision {rev} exceeds manifest cutoff {cutoff}",
                    path.display(),
                    row + 2
                ));
            }
            Some(rev)
        };
        let id = record.get(cols.id).unwrap_or("").trim();
        if id.is_empty() {
            continue;
        }
        let parsed = fra::parse_fra_row(&record, &cols, row, source_id);
        let replace = match latest.get(id) {
            None => true,
            Some((prev_rev, prev_row, _)) => (rev_date, row) >= (*prev_rev, *prev_row),
        };
        if replace {
            latest.insert(id.to_string(), (rev_date, row, parsed));
        }
    }
    let actual = reader
        .into_inner()
        .finish_hex()
        .map_err(|err| format!("reads {}: {err}", path.display()))?;
    if let Some((spec_id, expected_sha, _)) = expected
        && actual != expected_sha
    {
        return Err(format!(
            "{spec_id}: FRA crossing inventory checksum {actual} differs from manifest {expected_sha}"
        ));
    }
    Ok(latest
        .into_values()
        .filter_map(|(_, _, crossing)| crossing)
        .collect())
}

/// Stream, SHA-256 verify, cutoff-check, deduplicate, and parse an FRA crossing CSV in one pass.
pub fn parse_verified_fra(
    path: &Path,
    spec: &Input,
    source_id: u16,
) -> Result<Vec<Crossing>, String> {
    let conversion: FraConversion = serde_json::from_value(
        spec.conversion
            .clone()
            .ok_or_else(|| format!("{}: FRA crossings must pin sha256 and cutoff", spec.id))?,
    )
    .map_err(|err| format!("{}: invalid FRA conversion: {err}", spec.id))?;
    let cutoff = NaiveDate::parse_from_str(&conversion.cutoff, "%Y-%m-%d")
        .map_err(|err| format!("{}: invalid FRA cutoff: {err}", spec.id))?;
    stream_fra(
        path,
        source_id,
        Some((&spec.id, &conversion.sha256, cutoff)),
    )
}

#[cfg(test)]
pub fn parse_fra(path: &Path, source_id: u16) -> Result<Vec<Crossing>, String> {
    stream_fra(path, source_id, None)
}

pub fn parse_tc(path: &Path, source_id: u16) -> Result<Vec<Crossing>, String> {
    let mut reader = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .from_path(path)
        .map_err(|err| format!("opens {}: {err}", path.display()))?;
    let headers: Vec<String> = reader
        .headers()
        .map_err(|err| format!("reads {} headers: {err}", path.display()))?
        .iter()
        .map(|h| h.trim().to_string())
        .collect();
    let c_id = column(&headers, "TC Number", path)?;
    let c_access = column(&headers, "Access", path)?;
    let c_lat = column(&headers, "Latitude", path)?;
    let c_lon = column(&headers, "Longitude", path)?;
    let c_trains = column(&headers, "Trains Daily", path)?;
    let c_speed = column(&headers, "Train Max Speed (mph)", path)?;
    let mut out = Vec::new();
    for (row, record) in reader.records().enumerate() {
        let record = record.map_err(|err| format!("reads {}: {err}", path.display()))?;
        if record.get(c_access) != Some("Public") {
            continue;
        }
        let (Some(lat), Some(lon)) = (num(&record, c_lat), num(&record, c_lon)) else {
            continue;
        };
        if !(-90.0..=90.0).contains(&lat)
            || !(-180.0..=180.0).contains(&lon)
            || (lat == 0.0 && lon == 0.0)
        {
            continue;
        }
        // Daily total, no day/night split: flat 12/4/8 across END periods.
        // Canada remains horns-only until a reliable passenger split is available.
        let daily = non_neg_num(&record, c_trains).unwrap_or(0.0);
        if daily < 1.0 {
            continue;
        }
        let mph = non_neg_num(&record, c_speed).filter(|v| *v > 0.0);
        out.push(Crossing {
            id: record.get(c_id).unwrap_or("").to_string(),
            lat,
            lon,
            soundings: flat_end_periods(daily),
            speed_kmh: mph.map(|v| v * 1.609344),
            source_id,
            synth_osm: -(TC_OSM_OFFSET + row as i64 + 1),
            total_periods: None,
            passenger_daily: None,
            passenger_lt1: false,
            measured_periods: false,
        });
    }
    Ok(out)
}

#[cfg(test)]
#[path = "crossings_tests.rs"]
mod tests;
