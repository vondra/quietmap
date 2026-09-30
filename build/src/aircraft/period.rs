//! END 2002/49/EC period (day 07-19, evening 19-23, night 23-07) from local wall-clock time at a
//! coordinate. The time zone is looked up at every coordinate itself (dev4 build-robust 373d81fa):
//! dev4's thread-local cache keyed by 0.1 degree cells answered a whole cell with its first
//! visitor's zone, so border periods depended on thread scheduling (24,564 of 72.8 M rows).

use chrono::{DateTime, NaiveDate, Timelike};
use chrono_tz::Tz;
use std::sync::OnceLock;
use tzf_rs::DefaultFinder;

pub const DAY: u8 = 0;
pub const EVENING: u8 = 1;
pub const NIGHT: u8 = 2;

fn finder() -> &'static DefaultFinder {
    static FINDER: OnceLock<DefaultFinder> = OnceLock::new();
    FINDER.get_or_init(DefaultFinder::new)
}

fn zone(lat: f64, lon: f64) -> Option<Tz> {
    let name = finder().get_tz_name(lon, lat);
    (!name.is_empty()).then(|| name.parse().ok()).flatten()
}

/// The zone at a coordinate; open polar water takes the first zone found stepping towards the
/// equator (Tromso, Kiruna and their neighbours lie in well-defined zones), else UTC.
pub fn time_zone(lat: f64, lon: f64) -> Tz {
    if let Some(tz) = zone(lat, lon) {
        return tz;
    }
    if lat.abs() > 60.0 {
        let towards_equator = -lat.signum();
        for step in [1.0, 2.0, 4.0, 8.0, 16.0] {
            if let Some(tz) = zone((lat + towards_equator * step).clamp(-89.9, 89.9), lon) {
                return tz;
            }
        }
    }
    chrono_tz::UTC
}

/// The period of a UTC instant at a coordinate; a non-finite instant counts as night.
pub fn period(timestamp: f64, lat: f64, lon: f64) -> u8 {
    if !timestamp.is_finite() {
        return NIGHT;
    }
    let utc = DateTime::from_timestamp(timestamp as i64, 0).unwrap_or(DateTime::UNIX_EPOCH);
    match utc.with_timezone(&time_zone(lat, lon)).hour() {
        7..=18 => DAY,
        19..=22 => EVENING,
        _ => NIGHT,
    }
}

/// Days since 2020-01-01 of a `YYYY-MM-DD` day (the segment `date_id` column).
pub fn date_id(day: &str) -> Result<i16, String> {
    let date =
        NaiveDate::parse_from_str(day, "%Y-%m-%d").map_err(|error| format!("{day}: {error}"))?;
    if date.format("%Y-%m-%d").to_string() != day {
        return Err(format!("{day}: not YYYY-MM-DD"));
    }
    let epoch = NaiveDate::from_ymd_opt(2020, 1, 1).expect("valid date");
    i16::try_from((date - epoch).num_days()).map_err(|_| format!("{day}: out of range"))
}

/// Unix time of a day's 00:00 UTC.
pub fn day_start(day: &str) -> Result<i64, String> {
    Ok(i64::from(date_id(day)?) * 86_400 + 1_577_836_800)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2023-01-02 00:00 UTC: 01:00 in Prague, 19:00 in New York, 09:00 in Tokyo.
    #[test]
    fn periods_follow_local_time() {
        let midnight_utc = 1_672_617_600.0;
        assert_eq!(period(midnight_utc, 50.08, 14.43), NIGHT);
        assert_eq!(period(midnight_utc, 40.71, -74.00), EVENING);
        assert_eq!(period(midnight_utc, 35.68, 139.69), DAY);
        assert_eq!(period(f64::NAN, 50.0, 14.0), NIGHT);
    }

    /// dev4's cache answered a whole 0.1 degree cell with its first visitor's zone: these two
    /// points share cell 49.5/22.6 but lie in Poland and in Ukraine, an hour apart.
    #[test]
    fn a_zone_belongs_to_the_coordinate_not_to_its_cell() {
        assert_eq!(time_zone(49.500, 22.660).name(), "Europe/Warsaw");
        assert_eq!(time_zone(49.505, 22.665).name(), "Europe/Kyiv");
        let half_past_four_utc = 1_756_830_600.0;
        assert_eq!(period(half_past_four_utc, 49.500, 22.660), DAY);
        assert_eq!(period(half_past_four_utc, 49.505, 22.665), EVENING);
        assert_ne!(
            time_zone(80.0, -100.0),
            chrono_tz::UTC,
            "polar water takes a neighbour"
        );
    }

    #[test]
    fn day_ids_count_from_2020_and_refuse_malformed_days() {
        assert_eq!(date_id("2020-01-01"), Ok(0));
        assert_eq!(date_id("2021-03-01"), Ok(366 + 31 + 28));
        assert_eq!(day_start("2025-09-02"), Ok(1_756_771_200));
        for bad in [
            "2025-02-30",
            "2025-13-01",
            "2025-1-1",
            "../2025-01-01",
            "9999-01-01",
        ] {
            assert!(date_id(bad).is_err(), "{bad}");
        }
    }
}
