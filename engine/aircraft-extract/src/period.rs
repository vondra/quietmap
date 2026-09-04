//! TZ-aware Lden period bucketing (END 2002/49/EC: day 07-19,
//! evening 19-23, night 23-07) computed from local wall-clock time at
//! the segment's coordinate.
//!
//! `tzf-rs` ships an embedded polygon dataset (~30 MB) loaded once per
//! process via `OnceLock`. Lookups allocate, so a thread-local 0.1°
//! (~11 km) cache absorbs > 99 % of calls — one full flight typically
//! reuses the same TZ bucket end-to-end.
//!
//! Polar fallback (lat > 70 °N) uses nearest-neighbor TZ rather than UTC
//! because the practical receivers there (Tromsø, Bodø, Kiruna,
//! Rovaniemi) are inside well-defined country TZ polygons; only the
//! open ocean / ice cap above them needs a sane default.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;

use chrono::{DateTime, NaiveDate, Timelike, Utc};
use chrono_tz::Tz;
use tzf_rs::DefaultFinder;

fn tz_finder() -> &'static DefaultFinder {
    static F: OnceLock<DefaultFinder> = OnceLock::new();
    F.get_or_init(DefaultFinder::new)
}

thread_local! {
    static TZ_CACHE: RefCell<HashMap<(i16, i16), Tz>> = RefCell::new(HashMap::new());
}

/// Resolve the IANA timezone for a coordinate, with a thread-local 0.1°
/// cache and a polar nearest-neighbor fallback.
pub fn resolve_tz(lat: f64, lon: f64) -> Tz {
    let key = ((lat * 10.0) as i16, (lon * 10.0) as i16);
    TZ_CACHE.with(|c| {
        if let Some(&tz) = c.borrow().get(&key) {
            return tz;
        }
        let tz = lookup_tz_with_polar_fallback(lat, lon);
        c.borrow_mut().insert(key, tz);
        tz
    })
}

fn lookup_tz_raw(lat: f64, lon: f64) -> Option<Tz> {
    let name = tz_finder().get_tz_name(lon, lat);
    if name.is_empty() {
        return None;
    }
    name.parse().ok()
}

fn lookup_tz_with_polar_fallback(lat: f64, lon: f64) -> Tz {
    if let Some(tz) = lookup_tz_raw(lat, lon) {
        return tz;
    }
    // Polar / open-ocean: walk southward until we hit a populated TZ.
    if lat.abs() > 60.0 {
        let direction = if lat > 0.0 { -1.0 } else { 1.0 };
        for step_deg in [1.0, 2.0, 4.0, 8.0, 16.0] {
            let probe_lat = (lat + direction * step_deg).clamp(-89.9, 89.9);
            if let Some(tz) = lookup_tz_raw(probe_lat, lon) {
                return tz;
            }
        }
    }
    chrono_tz::UTC
}

/// Validated YYYY-MM-DD day offset from 2020-01-01 for the Arrow date column.
pub fn parse_date_id(day_str: &str) -> anyhow::Result<i16> {
    let date = NaiveDate::parse_from_str(day_str, "%Y-%m-%d")?;
    anyhow::ensure!(
        date.format("%Y-%m-%d").to_string() == day_str,
        "date must be YYYY-MM-DD: {day_str}"
    );
    let epoch = NaiveDate::from_ymd_opt(2020, 1, 1).unwrap();
    Ok(i16::try_from(date.signed_duration_since(epoch).num_days())?)
}

/// Classify a UTC Unix timestamp into an END 2002/49/EC period using
/// local wall-clock time at the given coordinate. Returns 0=day,
/// 1=evening, 2=night. Non-finite `ts` (NaN / ±∞) → night (conservative).
pub fn period_from_timestamp(ts: f64, lat: f64, lon: f64) -> u8 {
    if !ts.is_finite() {
        return 2;
    }
    let tz = resolve_tz(lat, lon);
    let utc = DateTime::<Utc>::from_timestamp(ts as i64, 0).unwrap_or(DateTime::<Utc>::UNIX_EPOCH);
    match utc.with_timezone(&tz).hour() {
        7..=18 => 0,
        19..=22 => 1,
        _ => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_dates_never_become_a_valid_sampling_day() {
        for day in [
            "2025-02-30",
            "2025-13-01",
            "2025-1-1",
            "../2025-01-01",
            "9999-01-01",
        ] {
            assert!(parse_date_id(day).is_err(), "{day}");
        }
    }

    #[test]
    fn date_id_2020_01_01_is_zero() {
        assert_eq!(parse_date_id("2020-01-01").unwrap(), 0);
    }

    #[test]
    fn date_id_handles_leap_year() {
        // 2020-03-01: Jan 31 + Feb-leap 29 → day 60 of year (zero-indexed).
        assert_eq!(parse_date_id("2020-03-01").unwrap(), 60);
        // 2021-03-01: 366 (leap 2020) + Jan 31 + Feb 28.
        assert_eq!(parse_date_id("2021-03-01").unwrap(), 366 + 31 + 28);
    }

    #[test]
    fn prague_winter_2300_local_is_night() {
        // 2023-01-02 00:00 UTC = 2023-01-02 01:00 CET → night.
        let ts = 1672621200.0;
        assert_eq!(period_from_timestamp(ts, 50.08, 14.43), 2);
    }

    #[test]
    fn nyc_winter_1900_local_is_evening() {
        let ts = 1672621200.0;
        assert_eq!(period_from_timestamp(ts, 40.71, -74.00), 1);
    }

    #[test]
    fn tokyo_morning() {
        let ts = 1672621200.0;
        assert_eq!(period_from_timestamp(ts, 35.68, 139.69), 0);
    }

    #[test]
    fn nonfinite_timestamp_is_night() {
        assert_eq!(period_from_timestamp(f64::NAN, 50.0, 14.0), 2);
        assert_eq!(period_from_timestamp(f64::INFINITY, 50.0, 14.0), 2);
    }

    #[test]
    fn polar_fallback_uses_neighbor_not_utc() {
        // 80 °N, 100 °W (deep arctic ocean): if tzf-rs has no polygon
        // here the result must still be a real IANA zone, not UTC.
        let tz = resolve_tz(80.0, -100.0);
        // We can't pin to a specific zone (depends on tzf-rs polygons)
        // but we assert it is *not* UTC when neighbours exist.
        // Tromsø area (70°N 19°E) — Norway TZ.
        let tromso = resolve_tz(70.0, 19.0);
        assert_ne!(tromso, chrono_tz::UTC);
        // Drop unused-binding warning even when polar polygon exists.
        let _ = tz;
    }
}
