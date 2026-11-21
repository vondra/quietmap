//! Parse one US DOT FRA Form 6180.71 grade-crossing row into flow and sounding counts.
use super::{column, flat_end_periods, non_neg_num, num};
use crate::railways::types::Crossing;
use std::path::Path;

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

pub(super) fn fra_end_periods(day: f64, night: f64) -> [f64; 3] {
    [
        day * 11.0 / 12.0 + night / 12.0,
        night * 4.0 / 12.0,
        day / 12.0 + night * 7.0 / 12.0,
    ]
}

pub(super) struct FraColumns {
    pub rev: usize,
    pub id: usize,
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
    pub(super) fn from_headers(headers: &[String], path: &Path) -> Result<Self, String> {
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
    complete_dn: Option<(f64, f64)>,
    weekly_daily: Option<f64>,
    switching: f64,
    transit: f64,
    corrupt_dn: bool,
}

impl FraCounts {
    fn parse(record: &csv::StringRecord, cols: &FraColumns) -> Self {
        let day_raw = num(record, cols.day);
        let night_raw = num(record, cols.night);
        let corrupt_dn = day_raw.is_some_and(|v| v < 0.0)
            || night_raw.is_some_and(|v| v < 0.0)
            || (day_raw.is_some() != night_raw.is_some());
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
    passenger_lt1: bool,
    measured_periods: bool,
}

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
    if !services.is_empty() && services.iter().all(|code| *code == "14") {
        return None;
    }
    let passenger_only_svc = !services.is_empty() && !has_freight_svc && has_heavy_pax_svc;
    let freight_only_svc = has_freight_svc && !has_heavy_pax_svc;

    let dn = counts.complete_dn.map_or(0.0, |(d, n)| d + n);
    let raw_pax_opt = non_neg_num(record, cols.passenger);
    let lt1_pax = record.get(cols.lt1_passenger) == Some("Yes");
    let reason = record.get(cols.reason).unwrap_or("");

    // Mixed freight+transit crossing (no heavy-rail passenger service) whose through-train count
    // includes urban transit (e.g. 661798L, 661802Y, 661803F, 661929M, 662048E) cannot separate
    // heavy-rail through trains from light rail; reject flow while retaining the independent horn.
    if counts.transit > 0.0
        && has_transit_svc
        && !has_heavy_pax_svc
        && raw_pax_opt.is_some_and(|p| p > 0.0 && dn >= 0.9 * counts.transit.min(p))
    {
        return None;
    }

    // Reconcile urban transit counts before any freight/passenger service classification.
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

    let make = |total_periods, passenger_daily, passenger_lt1| {
        Some(FraFlow {
            total_periods,
            passenger_daily,
            passenger_lt1,
            measured_periods,
        })
    };
    if raw_daily == 0.0 {
        return make([0.0; 3], Some(0.0), false);
    }
    if pax_irreconcilable {
        return make(raw_periods, None, false);
    }
    if passenger_only_svc && !lt1_pax && pax_opt != Some(0.0) {
        let total = raw_daily.max(pax_opt.unwrap_or(0.0));
        return make(scale_periods(raw_periods, total), Some(total), false);
    }
    if let Some(pax) = pax_opt.filter(|value| *value > 0.0) {
        let total = raw_daily.max(pax);
        return make(scale_periods(raw_periods, total), Some(pax), false);
    }
    if freight_only_svc {
        return make(raw_periods, Some(0.0), false);
    }
    // `Less Than One Passenger Train Per Day` bounds passengers below 1/day rather than fixing a split.
    let passenger_daily = (pax_opt == Some(0.0) && !lt1_pax).then_some(0.0);
    make(
        raw_periods,
        passenger_daily,
        passenger_daily.is_none() && lt1_pax,
    )
}

pub(super) fn parse_fra_row(
    record: &csv::StringRecord,
    cols: &FraColumns,
    row: usize,
    source_id: u16,
) -> Option<Crossing> {
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
    let flow = parse_fra_flow(record, cols, &counts);
    let is_public_highway =
        record.get(cols.kind) == Some("3") && record.get(cols.purpose) == Some("1");
    // Whistleban: 1 = 24 hr and 3 = Chicago excused are silent all day;
    // 2 = partial sounds 07-22 (evening 19-22 is 3/4 of END 19-23); 0/empty sounds all day.
    let horn_base = if !is_public_highway || counts.corrupt_dn {
        [0.0; 3]
    } else if let Some(flow) = flow
        .as_ref()
        .filter(|flow| flow.total_periods.iter().sum::<f64>() > 0.0)
    {
        flow.total_periods
    } else if let Some((day, night)) = counts.complete_dn.filter(|&(d, n)| d + n > 0.0) {
        fra_end_periods(day, night)
    } else if let Some(daily) = counts.weekly_daily {
        flat_end_periods(daily)
    } else {
        [0.0; 3]
    };
    let soundings = match record.get(cols.ban) {
        Some("1") | Some("3") => [0.0; 3],
        Some("2") => [horn_base[0], horn_base[1] * 0.75, 0.0],
        _ => horn_base,
    };
    if soundings.iter().all(|&s| s <= 0.0) && flow.is_none() {
        return None;
    }
    let mph = non_neg_num(record, cols.typical)
        .filter(|v| *v > 0.0)
        .or_else(|| non_neg_num(record, cols.timetable).filter(|v| *v > 0.0));
    let (total_periods, passenger_daily, passenger_lt1, measured_periods) = match flow {
        Some(flow) => (
            Some(flow.total_periods),
            flow.passenger_daily,
            flow.passenger_lt1,
            flow.measured_periods,
        ),
        None => (None, None, false, false),
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
        passenger_lt1,
        measured_periods,
    })
}
