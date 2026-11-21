//! Normalize FRA and Transport Canada publisher fields to the shared crossing movement.
use crate::{
    railways::{config::FraConversion, types::Crossing},
    year::Input,
};
use sha2::{Digest, Sha256};
use std::{fs::File, io::Read, path::Path};
const TC_OSM_OFFSET: i64 = 1_000_000_000;

fn parse_fra_date(text: &str) -> Result<chrono::NaiveDate, String> {
    let text = text.trim();
    if let Ok(day) = chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        return Ok(day);
    }
    if text.len() >= 10
        && let Ok(day) = chrono::NaiveDate::parse_from_str(&text[..10], "%Y-%m-%d")
    {
        return Ok(day);
    }
    chrono::NaiveDate::parse_from_str(text, "%m/%d/%Y")
        .map_err(|err| format!("invalid FRA revision date {text:?}: {err}"))
}

pub fn verify_fra_input(path: &Path, spec: &Input) -> Result<(), String> {
    let conversion: FraConversion = serde_json::from_value(
        spec.conversion
            .clone()
            .ok_or_else(|| format!("{}: FRA crossings must pin sha256 and cutoff", spec.id))?,
    )
    .map_err(|err| format!("{}: invalid FRA conversion: {err}", spec.id))?;
    let mut file = File::open(path).map_err(|err| format!("opens {}: {err}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|err| format!("reads {}: {err}", path.display()))?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    let actual = format!("{:x}", digest.finalize());
    if actual != conversion.sha256 {
        return Err(format!(
            "{}: FRA crossing inventory checksum {actual} differs from manifest {}",
            spec.id, conversion.sha256
        ));
    }
    let cutoff = chrono::NaiveDate::parse_from_str(&conversion.cutoff, "%Y-%m-%d")
        .map_err(|err| format!("{}: invalid FRA cutoff: {err}", spec.id))?;
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
    let c_rev = column(&headers, "Revision Date", path)?;
    for (row, record) in reader.records().enumerate() {
        let record = record.map_err(|err| format!("reads {}: {err}", path.display()))?;
        let text = record.get(c_rev).unwrap_or("").trim();
        if text.is_empty() {
            continue;
        }
        let rev = parse_fra_date(text)?;
        if rev > cutoff {
            return Err(format!(
                "{}: row {} revision {rev} exceeds manifest cutoff {cutoff}",
                path.display(),
                row + 2
            ));
        }
    }
    Ok(())
}
fn csv_records(path: &Path) -> Result<(Vec<String>, Vec<csv::StringRecord>), String> {
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
    let mut records = Vec::new();
    for record in reader.records() {
        records.push(record.map_err(|err| format!("reads {}: {err}", path.display()))?);
    }
    Ok((headers, records))
}

fn column(headers: &[String], name: &str, path: &Path) -> Result<usize, String> {
    headers
        .iter()
        .position(|h| h == name)
        .ok_or_else(|| format!("{} lacks a {name} column", path.display()))
}

fn num(record: &csv::StringRecord, idx: usize) -> Option<f64> {
    let text = record.get(idx)?.trim();
    if text.is_empty() || text == "-" {
        return None;
    }
    text.parse::<f64>().ok()
}

fn fra_end_periods(day: f64, night: f64) -> [f64; 3] {
    [
        day * 11.0 / 12.0 + night / 12.0,
        night * 4.0 / 12.0,
        day / 12.0 + night * 7.0 / 12.0,
    ]
}

fn fra_partial_periods(day: f64, night: f64) -> [f64; 3] {
    [day * 11.0 / 12.0 + night / 12.0, night * 3.0 / 12.0, 0.0]
}

pub fn parse_fra(path: &Path, source_id: u16) -> Result<Vec<Crossing>, String> {
    let (headers, records) = csv_records(path)?;
    let c_id = column(&headers, "Crossing ID", path)?;
    let c_type = column(&headers, "Crossing Type Code", path)?;
    let c_purpose = column(&headers, "Crossing Purpose Code", path)?;
    let c_position = column(&headers, "Crossing Position Code", path)?;
    let c_ban = column(&headers, "Whistleban Code", path)?;
    let c_lat = column(&headers, "Latitude", path)?;
    let c_lon = column(&headers, "Longitude", path)?;
    let c_day = column(&headers, "Total Daylight Thru Trains", path)?;
    let c_night = column(&headers, "Total Nighttime Thru Trains", path)?;
    let c_timetable = column(&headers, "Maximum Timetable Speed", path)?;
    let c_typical = column(&headers, "Typical Maximum Speed Over Crossing", path)?;
    let c_closed = column(&headers, "Crossing Closed", path)?;
    let mut out = Vec::new();
    for (row, record) in records.iter().enumerate() {
        if record.get(c_type) != Some("3")
            || record.get(c_purpose) != Some("1")
            || record.get(c_position) != Some("1")
            || record.get(c_closed) != Some("No")
        {
            continue;
        }
        let (Some(lat), Some(lon)) = (num(record, c_lat), num(record, c_lon)) else {
            continue;
        };
        if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
            continue;
        }
        let day = num(record, c_day).unwrap_or(0.0).max(0.0);
        let night = num(record, c_night).unwrap_or(0.0).max(0.0);
        if day + night <= 0.0 {
            continue;
        }
        // Whistleban: 1 = 24 hr and 3 = Chicago excused are silent all day;
        // 2 = partial sounds 07–22; 0/empty sounds around the clock.
        let soundings = match record.get(c_ban) {
            Some("1") | Some("3") => {
                continue;
            }
            Some("2") => fra_partial_periods(day, night),
            _ => fra_end_periods(day, night),
        };
        let mph = num(record, c_typical)
            .filter(|v| *v > 0.0)
            .or_else(|| num(record, c_timetable).filter(|v| *v > 0.0));
        out.push(Crossing {
            id: record.get(c_id).unwrap_or("").to_string(),
            lat,
            lon,
            soundings,
            speed_kmh: mph.map(|v| v * 1.609344),
            source_id,
            synth_osm: -(row as i64 + 1),
        });
    }
    Ok(out)
}

pub fn parse_tc(path: &Path, source_id: u16) -> Result<Vec<Crossing>, String> {
    let (headers, records) = csv_records(path)?;
    let c_id = column(&headers, "TC Number", path)?;
    let c_access = column(&headers, "Access", path)?;
    let c_lat = column(&headers, "Latitude", path)?;
    let c_lon = column(&headers, "Longitude", path)?;
    let c_trains = column(&headers, "Trains Daily", path)?;
    let c_speed = column(&headers, "Train Max Speed (mph)", path)?;
    let mut out = Vec::new();
    for (row, record) in records.iter().enumerate() {
        if record.get(c_access) != Some("Public") {
            continue;
        }
        let (Some(lat), Some(lon)) = (num(record, c_lat), num(record, c_lon)) else {
            continue;
        };
        if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
            continue;
        }
        // Daily total, no day/night split: flat 12/4/8 across END periods.
        let daily = num(record, c_trains).unwrap_or(0.0);
        if daily < 1.0 {
            continue;
        }
        let mph = num(record, c_speed).filter(|v| *v > 0.0);
        out.push(Crossing {
            id: record.get(c_id).unwrap_or("").to_string(),
            lat,
            lon,
            soundings: [daily * 12.0 / 24.0, daily * 4.0 / 24.0, daily * 8.0 / 24.0],
            speed_kmh: mph.map(|v| v * 1.609344),
            source_id,
            synth_osm: -(TC_OSM_OFFSET + row as i64 + 1),
        });
    }
    Ok(out)
}

#[cfg(test)]
#[path = "crossings_tests.rs"]
mod tests;
