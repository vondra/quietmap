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
    text.parse::<f64>().ok().filter(|value| value.is_finite())
}

fn non_neg_num(record: &csv::StringRecord, idx: usize) -> Option<f64> {
    num(record, idx).filter(|value| *value >= 0.0)
}

fn flat_end_periods(daily: f64) -> [f64; 3] {
    [daily * 12.0 / 24.0, daily * 4.0 / 24.0, daily * 8.0 / 24.0]
}

fn scale_periods(periods: [f64; 3], target_daily: f64) -> [f64; 3] {
    let sum: f64 = periods.iter().sum();
    if sum > 0.0 {
        let factor = target_daily / sum;
        [
            periods[0] * factor,
            periods[1] * factor,
            periods[2] * factor,
        ]
    } else {
        flat_end_periods(target_daily)
    }
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

struct FraColumns {
    id: usize,
    kind: usize,
    purpose: usize,
    position: usize,
    ban: usize,
    lat: usize,
    lon: usize,
    day: usize,
    night: usize,
    switching: usize,
    transit: usize,
    weekly: usize,
    passenger: usize,
    lt1_passenger: usize,
    main_tracks: usize,
    service_ids: usize,
    reason: usize,
    timetable: usize,
    typical: usize,
    closed: usize,
}

impl FraColumns {
    fn from_headers(headers: &[String], path: &Path) -> Result<Self, String> {
        Ok(Self {
            id: column(headers, "Crossing ID", path)?,
            kind: column(headers, "Crossing Type Code", path)?,
            purpose: column(headers, "Crossing Purpose Code", path)?,
            position: column(headers, "Crossing Position Code", path)?,
            ban: column(headers, "Whistleban Code", path)?,
            lat: column(headers, "Latitude", path)?,
            lon: column(headers, "Longitude", path)?,
            day: column(headers, "Total Daylight Thru Trains", path)?,
            night: column(headers, "Total Nighttime Thru Trains", path)?,
            switching: column(headers, "Total Switching Trains", path)?,
            transit: column(headers, "Total Transit Trains", path)?,
            weekly: column(headers, "Trains Per Week", path)?,
            passenger: column(headers, "Number Passenger Train Per Day", path)?,
            lt1_passenger: column(headers, "Less Than One Passenger Train Per Day", path)?,
            main_tracks: column(headers, "Number Of Main Tracks", path)?,
            service_ids: column(headers, "Type Of Train Service IDs", path)?,
            reason: column(headers, "Reason Description", path)?,
            timetable: column(headers, "Maximum Timetable Speed", path)?,
            typical: column(headers, "Typical Maximum Speed Over Crossing", path)?,
            closed: column(headers, "Crossing Closed", path)?,
        })
    }
}

struct FraFlow {
    total_periods: [f64; 3],
    passenger_daily: Option<f64>,
    main_tracks: u8,
    freight_only: bool,
}

/// Extract heavy-rail through-train counts from an open at-grade FRA row.
/// Returns `None` when the row carries no usable main-track through count
/// (missing/blank, zero main tracks, transit-only, or switching/transit only).
fn parse_fra_flow(record: &csv::StringRecord, cols: &FraColumns) -> Option<FraFlow> {
    let mt_raw = non_neg_num(record, cols.main_tracks)?;
    let mt_rounded = mt_raw.round();
    if !(1.0..=255.0).contains(&mt_rounded) || (mt_raw - mt_rounded).abs() > 1e-6 {
        return None;
    }
    let main_tracks = mt_rounded as u8;

    // FRA Form 6180.71 Part II Item 1D service codes:
    // 11 = Freight, 12 = Intercity, 13 = Commuter, 14 = Transit,
    // 15 = Shared Use Transit, 16 = Tourist/Other.
    let services: Vec<&str> = record
        .get(cols.service_ids)
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .collect();
    let has_freight_svc = services.contains(&"11");
    let has_pax_svc = services
        .iter()
        .any(|code| matches!(*code, "12" | "13" | "15" | "16"));
    let transit_only = !services.is_empty() && services.iter().all(|code| *code == "14");
    if transit_only {
        return None;
    }
    let passenger_only_svc = !services.is_empty() && !has_freight_svc && has_pax_svc;
    let freight_only_svc = has_freight_svc && !has_pax_svc;

    let day_opt = non_neg_num(record, cols.day);
    let night_opt = non_neg_num(record, cols.night);
    let dn = day_opt.unwrap_or(0.0) + night_opt.unwrap_or(0.0);
    let weekly = non_neg_num(record, cols.weekly).unwrap_or(0.0);
    let pax_opt = non_neg_num(record, cols.passenger);
    let switching = non_neg_num(record, cols.switching).unwrap_or(0.0);
    let transit = non_neg_num(record, cols.transit).unwrap_or(0.0);
    let lt1_pax = record.get(cols.lt1_passenger) == Some("Yes");
    let reason = record.get(cols.reason).unwrap_or("");

    let (raw_periods, raw_daily) = if dn > 0.0 {
        (
            fra_end_periods(day_opt.unwrap_or(0.0), night_opt.unwrap_or(0.0)),
            dn,
        )
    } else if weekly > 0.0 {
        let daily = weekly / 7.0;
        (flat_end_periods(daily), daily)
    } else if let Some(pax) = pax_opt.filter(|value| *value > 0.0) {
        (flat_end_periods(pax), pax)
    } else if day_opt == Some(0.0)
        && night_opt == Some(0.0)
        && switching == 0.0
        && transit == 0.0
        && reason == "No Train Traffic"
    {
        ([0.0; 3], 0.0)
    } else {
        return None;
    };

    if raw_daily == 0.0 {
        return Some(FraFlow {
            total_periods: [0.0; 3],
            passenger_daily: Some(0.0),
            main_tracks,
            freight_only: freight_only_svc,
        });
    }
    if passenger_only_svc {
        let total = raw_daily.max(pax_opt.unwrap_or(0.0));
        return Some(FraFlow {
            total_periods: scale_periods(raw_periods, total),
            passenger_daily: Some(total),
            main_tracks,
            freight_only: false,
        });
    }
    if let Some(pax) = pax_opt.filter(|value| *value > 0.0) {
        let total = raw_daily.max(pax);
        return Some(FraFlow {
            total_periods: scale_periods(raw_periods, total),
            passenger_daily: Some(pax),
            main_tracks,
            freight_only: false,
        });
    }
    if lt1_pax && has_pax_svc {
        return Some(FraFlow {
            total_periods: raw_periods,
            passenger_daily: Some(0.5_f64.min(raw_daily)),
            main_tracks,
            freight_only: false,
        });
    }
    if freight_only_svc {
        return Some(FraFlow {
            total_periods: raw_periods,
            passenger_daily: Some(0.0),
            main_tracks,
            freight_only: true,
        });
    }
    let passenger_daily = (pax_opt == Some(0.0) && !lt1_pax).then_some(0.0);
    Some(FraFlow {
        total_periods: raw_periods,
        passenger_daily,
        main_tracks,
        freight_only: false,
    })
}

pub fn parse_fra(path: &Path, source_id: u16) -> Result<Vec<Crossing>, String> {
    let (headers, records) = csv_records(path)?;
    let cols = FraColumns::from_headers(&headers, path)?;
    let mut out = Vec::new();
    for (row, record) in records.iter().enumerate() {
        // Keep only open at-grade crossings; grade-separated crossings carry no
        // required FRA train counts and never sound horns.
        if record.get(cols.position) != Some("1") || record.get(cols.closed) != Some("No") {
            continue;
        }
        let (Some(lat), Some(lon)) = (num(record, cols.lat), num(record, cols.lon)) else {
            continue;
        };
        if !(-90.0..=90.0).contains(&lat)
            || !(-180.0..=180.0).contains(&lon)
            || (lat == 0.0 && lon == 0.0)
        {
            continue;
        }
        let day_horn = non_neg_num(record, cols.day).unwrap_or(0.0);
        let night_horn = non_neg_num(record, cols.night).unwrap_or(0.0);
        let is_public_highway =
            record.get(cols.kind) == Some("3") && record.get(cols.purpose) == Some("1");
        // Whistleban: 1 = 24 hr and 3 = Chicago excused are silent all day;
        // 2 = partial sounds 07–22; 0/empty sounds around the clock.
        let soundings = if is_public_highway && day_horn + night_horn > 0.0 {
            match record.get(cols.ban) {
                Some("1") | Some("3") => [0.0; 3],
                Some("2") => fra_partial_periods(day_horn, night_horn),
                _ => fra_end_periods(day_horn, night_horn),
            }
        } else {
            [0.0; 3]
        };
        let flow = parse_fra_flow(record, &cols);
        if soundings.iter().all(|&s| s <= 0.0) && flow.is_none() {
            continue;
        }
        let mph = non_neg_num(record, cols.typical)
            .filter(|v| *v > 0.0)
            .or_else(|| non_neg_num(record, cols.timetable).filter(|v| *v > 0.0));
        let (total_periods, passenger_daily, main_tracks, freight_only) = match flow {
            Some(flow) => (
                Some(flow.total_periods),
                flow.passenger_daily,
                Some(flow.main_tracks),
                flow.freight_only,
            ),
            None => (None, None, None, false),
        };
        out.push(Crossing {
            id: record.get(cols.id).unwrap_or("").to_string(),
            lat,
            lon,
            soundings,
            speed_kmh: mph.map(|v| v * 1.609344),
            source_id,
            synth_osm: -(row as i64 + 1),
            total_periods,
            passenger_daily,
            main_tracks,
            freight_only,
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
        if !(-90.0..=90.0).contains(&lat)
            || !(-180.0..=180.0).contains(&lon)
            || (lat == 0.0 && lon == 0.0)
        {
            continue;
        }
        // Daily total, no day/night split: flat 12/4/8 across END periods.
        // Canada remains horns-only until a reliable passenger split is available.
        let daily = non_neg_num(record, c_trains).unwrap_or(0.0);
        if daily < 1.0 {
            continue;
        }
        let mph = non_neg_num(record, c_speed).filter(|v| *v > 0.0);
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
            main_tracks: None,
            freight_only: false,
        });
    }
    Ok(out)
}

#[cfg(test)]
#[path = "crossings_tests.rs"]
mod tests;
