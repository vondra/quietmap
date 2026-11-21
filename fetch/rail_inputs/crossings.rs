//! Normalize FRA and Transport Canada publisher fields to the shared crossing movement.
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
    rev: usize,
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
    movement_code: usize,
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
            rev: column(headers, "Revision Date", path)?,
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
            movement_code: column(headers, "Movements Per Day Code", path)?,
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

/// Shared movement fields parsed once per open at-grade FRA row for both flows and horns.
struct FraCounts {
    /// Non-negative `(day, night)` thru trains when both half-days are explicitly present.
    complete_dn: Option<(f64, f64)>,
    /// Daily rate from `Trains Per Week` when valid (`< 7` trains/week, no daily/switching/transit
    /// movements, and not marked "1 or more per day").
    weekly_daily: Option<f64>,
    switching: f64,
    transit: f64,
    corrupt_dn: bool,
}

impl FraCounts {
    fn parse(record: &csv::StringRecord, cols: &FraColumns) -> Self {
        let day_raw = num(record, cols.day);
        let night_raw = num(record, cols.night);
        let negative_dn = day_raw.is_some_and(|v| v < 0.0) || night_raw.is_some_and(|v| v < 0.0);
        let one_half_blank = day_raw.is_some() != night_raw.is_some();
        let corrupt_dn = negative_dn || one_half_blank;
        let complete_dn = match (day_raw, night_raw) {
            (Some(d), Some(n)) if d >= 0.0 && n >= 0.0 => Some((d, n)),
            _ => None,
        };
        let switching = non_neg_num(record, cols.switching).unwrap_or(0.0);
        let transit = non_neg_num(record, cols.transit).unwrap_or(0.0);
        let movement_code = record.get(cols.movement_code).map(str::trim).unwrap_or("");
        let dn = complete_dn.map_or(0.0, |(d, n)| d + n);
        // FRA Part II Item 1.E: `Trains Per Week` applies only when Movements Per Day Code is
        // "Less Than One Movement Per Day" (strictly `< 7` trains/week, no daily/switching/transit).
        let weekly_daily = if !corrupt_dn
            && dn == 0.0
            && switching == 0.0
            && transit == 0.0
            && movement_code != "2"
        {
            non_neg_num(record, cols.weekly)
                .filter(|&w| w > 0.0 && w < 7.0)
                .map(|w| w / 7.0)
        } else {
            None
        };
        Self {
            complete_dn,
            weekly_daily,
            switching,
            transit,
            corrupt_dn,
        }
    }
}

struct FraFlow {
    total_periods: [f64; 3],
    passenger_daily: Option<f64>,
    main_tracks: u8,
    measured_periods: bool,
}

/// Extract heavy-rail through-train counts from an open at-grade FRA row.
/// Returns `None` when the row carries no usable main-track through count
/// (missing/blank, half-blank or negative day/night, zero main tracks,
/// transit-only, or switching/transit only).
fn parse_fra_flow(
    record: &csv::StringRecord,
    cols: &FraColumns,
    counts: &FraCounts,
) -> Option<FraFlow> {
    if counts.corrupt_dn {
        return None;
    }
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
    let has_heavy_pax_svc = services
        .iter()
        .any(|code| matches!(*code, "12" | "13" | "16"));
    let has_transit_svc = services.iter().any(|code| matches!(*code, "14" | "15"));
    let has_pax_svc = has_heavy_pax_svc || services.contains(&"15");
    let transit_only = !services.is_empty() && services.iter().all(|code| *code == "14");
    if transit_only {
        return None;
    }
    let passenger_only_svc = !services.is_empty() && !has_freight_svc && has_pax_svc;
    let freight_only_svc = has_freight_svc && !has_pax_svc;

    let dn = counts.complete_dn.map_or(0.0, |(d, n)| d + n);
    let raw_pax_opt = non_neg_num(record, cols.passenger);
    let lt1_pax = record.get(cols.lt1_passenger) == Some("Yes");
    let reason = record.get(cols.reason).unwrap_or("");

    // When a mixed freight+transit crossing (no heavy-rail passenger service) duplicates its
    // transit count into both through and passenger fields (e.g. San Diego Trolley 661798L),
    // the row supplies no heavy-rail train count.
    if counts.transit > 0.0
        && has_transit_svc
        && !has_heavy_pax_svc
        && dn > 0.0
        && (dn - counts.transit).abs() <= 1e-6
        && raw_pax_opt.is_some_and(|p| (p - counts.transit).abs() <= 1e-6)
    {
        return None;
    }

    // FRA Part I Item 22 passenger counts include urban transit trains, while
    // daylight/nighttime through counts exclude separately reported transit.
    // Reconcile any positive transit count regardless of whether service 14/15 was ticked.
    let (pax_opt, pax_irreconcilable) =
        if counts.transit > 0.0 && raw_pax_opt.is_some_and(|p| p > 0.0) {
            let p = raw_pax_opt.unwrap();
            if has_transit_svc && counts.transit > dn {
                let non_transit = p - counts.transit;
                if non_transit >= -1e-6 && non_transit <= dn + 1e-6 {
                    let clean = non_transit.max(0.0);
                    if dn > 0.0 {
                        (Some(clean), false)
                    } else {
                        ((clean > 0.0).then_some(clean), false)
                    }
                } else {
                    (None, true)
                }
            } else {
                (None, true)
            }
        } else {
            (raw_pax_opt, false)
        };

    let (raw_periods, raw_daily, measured_periods) =
        if let Some((day, night)) = counts.complete_dn.filter(|&(d, n)| d + n > 0.0) {
            (fra_end_periods(day, night), day + night, true)
        } else if let Some(daily) = counts.weekly_daily {
            (flat_end_periods(daily), daily, false)
        } else if let Some(pax) = pax_opt.filter(|value| *value > 0.0) {
            (flat_end_periods(pax), pax, false)
        } else if counts.complete_dn == Some((0.0, 0.0))
            && counts.switching == 0.0
            && counts.transit == 0.0
            && reason == "No Train Traffic"
        {
            ([0.0; 3], 0.0, true)
        } else {
            return None;
        };

    if raw_daily == 0.0 {
        return Some(FraFlow {
            total_periods: [0.0; 3],
            passenger_daily: Some(0.0),
            main_tracks,
            measured_periods,
        });
    }
    if pax_irreconcilable {
        return Some(FraFlow {
            total_periods: raw_periods,
            passenger_daily: None,
            main_tracks,
            measured_periods,
        });
    }
    if passenger_only_svc {
        let total = raw_daily.max(pax_opt.unwrap_or(0.0));
        return Some(FraFlow {
            total_periods: scale_periods(raw_periods, total),
            passenger_daily: Some(total),
            main_tracks,
            measured_periods,
        });
    }
    if let Some(pax) = pax_opt.filter(|value| *value > 0.0) {
        let total = raw_daily.max(pax);
        return Some(FraFlow {
            total_periods: scale_periods(raw_periods, total),
            passenger_daily: Some(pax),
            main_tracks,
            measured_periods,
        });
    }
    if freight_only_svc {
        return Some(FraFlow {
            total_periods: raw_periods,
            passenger_daily: Some(0.0),
            main_tracks,
            measured_periods,
        });
    }
    // `Less Than One Passenger Train Per Day` is an upper bound, not a measured split: leave unknown.
    let passenger_daily = (pax_opt == Some(0.0) && !lt1_pax).then_some(0.0);
    Some(FraFlow {
        total_periods: raw_periods,
        passenger_daily,
        main_tracks,
        measured_periods,
    })
}

fn parse_fra_row(
    record: &csv::StringRecord,
    cols: &FraColumns,
    row: usize,
    source_id: u16,
) -> Option<Crossing> {
    // Keep only open at-grade crossings; grade-separated crossings carry no
    // required FRA train counts and never sound horns.
    if record.get(cols.position) != Some("1") || record.get(cols.closed) != Some("No") {
        return None;
    }
    let (lat, lon) = (num(record, cols.lat)?, num(record, cols.lon)?);
    if !(-90.0..=90.0).contains(&lat)
        || !(-180.0..=180.0).contains(&lon)
        || (lat == 0.0 && lon == 0.0)
    {
        return None;
    }
    let counts = FraCounts::parse(record, cols);
    let is_public_highway =
        record.get(cols.kind) == Some("3") && record.get(cols.purpose) == Some("1");
    // Whistleban: 1 = 24 hr and 3 = Chicago excused are silent all day;
    // 2 = partial sounds 07–22; 0/empty sounds around the clock.
    let soundings = if !is_public_highway || counts.corrupt_dn {
        [0.0; 3]
    } else if let Some((day, night)) = counts.complete_dn.filter(|&(d, n)| d + n > 0.0) {
        match record.get(cols.ban) {
            Some("1") | Some("3") => [0.0; 3],
            Some("2") => fra_partial_periods(day, night),
            _ => fra_end_periods(day, night),
        }
    } else if let Some(daily) = counts.weekly_daily {
        match record.get(cols.ban) {
            Some("1") | Some("3") => [0.0; 3],
            Some("2") => [daily * 12.0 / 24.0, daily * 3.0 / 24.0, 0.0],
            _ => flat_end_periods(daily),
        }
    } else {
        [0.0; 3]
    };
    let flow = parse_fra_flow(record, cols, &counts);
    if soundings.iter().all(|&s| s <= 0.0) && flow.is_none() {
        return None;
    }
    let mph = non_neg_num(record, cols.typical)
        .filter(|v| *v > 0.0)
        .or_else(|| non_neg_num(record, cols.timetable).filter(|v| *v > 0.0));
    let (total_periods, passenger_daily, main_tracks, measured_periods) = match flow {
        Some(flow) => (
            Some(flow.total_periods),
            flow.passenger_daily,
            Some(flow.main_tracks),
            flow.measured_periods,
        ),
        None => (None, None, None, false),
    };
    Some(Crossing {
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
        measured_periods,
    })
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
    let cols = FraColumns::from_headers(&headers, path)?;
    // Deduplicate repeated Crossing IDs (keeping the latest Revision Date, then
    // the later CSV row) so every crossing votes once even if an upstream snapshot
    // carries duplicate rows.
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
        let parsed = parse_fra_row(&record, &cols, row, source_id);
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
            main_tracks: None,
            measured_periods: false,
        });
    }
    Ok(out)
}

#[cfg(test)]
#[path = "crossings_tests.rs"]
mod tests;
