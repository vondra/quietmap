//! The five daily prayer times by the standard astronomical algorithm (PrayTimes.org, H.
//! Zarrabi-Zadeh; research 2026-10-04: within a few minutes of Diyanet's, JAKIM's and Kemenag's
//! published tables), and the share of the year each call falls in the day, evening and night
//! period on the clocks of its place. Fajr begins at a set solar depression before sunrise, Dhuhr
//! at solar noon, Asr when a shadow is its object's length (Hanafi practice: twice) plus its noon
//! shadow, Maghrib at sunset, Isha at a set depression after sunset or a set time after Maghrib.
//! Where twilight never gets that deep (summer above about 48 N), Fajr and Isha take the angle's
//! sixtieth part of the night from sunrise and sunset (the angle-based rule).

use crate::period::period_in;
use chrono_tz::Tz;

/// How a country's authority sets Fajr and Isha, its Asr shadow factor and the margins (minutes)
/// its tables add to the astronomical times (Fajr, Dhuhr, Asr, Maghrib, Isha).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Method {
    pub fajr_angle: f64,
    pub isha: Isha,
    pub asr_factor: f64,
    pub margins_min: [f64; 5],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Isha {
    Angle(f64),
    MinutesAfterMaghrib(f64),
}

/// The Muslim World League's 18 and 17 degrees, Asr at one shadow.
pub const WORLD_LEAGUE: Method = Method {
    fajr_angle: 18.0,
    isha: Isha::Angle(17.0),
    asr_factor: 1.0,
    margins_min: [0.0; 5],
};

/// A sun's declination (degrees) and equation of time (hours) at a Julian date.
fn sun(julian_date: f64) -> (f64, f64) {
    let d = julian_date - 2_451_545.0;
    let g = (357.529 + 0.985_600_28 * d).to_radians();
    let q = 280.459 + 0.985_647_36 * d;
    let l = (q + 1.915 * g.sin() + 0.020 * (2.0 * g).sin()).to_radians();
    let e = (23.439 - 0.000_000_36 * d).to_radians();
    let right_ascension = (e.cos() * l.sin()).atan2(l.cos()).to_degrees() / 15.0;
    let declination = (e.sin() * l.sin()).asin().to_degrees();
    let equation = q / 15.0 - right_ascension.rem_euclid(24.0);
    (declination, (equation + 12.0).rem_euclid(24.0) - 12.0)
}

/// Hours from noon until the sun stands `altitude` degrees high (negative below the horizon); NaN
/// when it never does.
fn hours_from_noon(altitude: f64, lat: f64, declination: f64) -> f64 {
    let (phi, delta) = (lat.to_radians(), declination.to_radians());
    let cosine =
        (altitude.to_radians().sin() - phi.sin() * delta.sin()) / (phi.cos() * delta.cos());
    if cosine.abs() > 1.0 {
        f64::NAN
    } else {
        cosine.acos().to_degrees() / 15.0
    }
}

/// The five times (UTC hours of the day; Fajr, Dhuhr, Asr, Maghrib, Isha) on the day beginning at
/// `day_unix` (00:00 UTC) at (`lat`, `lon`), each with the sun of its own hour (two passes from
/// solar-time guesses); `None` when the sun neither rises nor sets.
pub fn prayer_hours(day_unix: i64, lat: f64, lon: f64, method: &Method) -> Option<[f64; 5]> {
    let midnight_jd = day_unix as f64 / 86_400.0 + 2_440_587.5;
    let sun_at = |hours: f64| sun(midnight_jd + hours / 24.0);
    let noon_at = |hours: f64| 12.0 - lon / 15.0 - sun_at(hours).1;
    let span_at = |altitude: f64, hours: f64| hours_from_noon(altitude, lat, sun_at(hours).0);
    let asr_span_at = |hours: f64| {
        let declination = sun_at(hours).0;
        let shadow = method.asr_factor + (lat - declination).to_radians().abs().tan();
        hours_from_noon((1.0 / shadow).atan().to_degrees(), lat, declination)
    };
    // Fajr, sunrise, Dhuhr, Asr, sunset, Isha: guesses at local solar 05, 06, 12, 13, 18, 18 h.
    let mut times = [5.0, 6.0, 12.0, 13.0, 18.0, 18.0].map(|hour| hour - lon / 15.0);
    for _ in 0..2 {
        let [fajr, sunrise, noon, asr, sunset, isha] = times;
        let next = [
            noon_at(fajr) - span_at(-method.fajr_angle, fajr),
            noon_at(sunrise) - span_at(-0.833, sunrise),
            noon_at(noon),
            noon_at(asr) + asr_span_at(asr),
            noon_at(sunset) + span_at(-0.833, sunset),
            match method.isha {
                Isha::Angle(angle) => noon_at(isha) + span_at(-angle, isha),
                Isha::MinutesAfterMaghrib(_) => isha,
            },
        ];
        for (time, value) in times.iter_mut().zip(next) {
            if value.is_finite() {
                *time = value;
            }
        }
        if let Isha::MinutesAfterMaghrib(minutes) = method.isha {
            times[5] = times[4] + minutes / 60.0;
        }
    }
    let [fajr, sunrise, noon, asr, sunset, isha] = times;
    if span_at(-0.833, noon).is_nan() {
        return None;
    }
    // Twilight too shallow or too long: the angle's sixtieth part of the night.
    let night = 24.0 - (sunset - sunrise);
    let portion = |angle: f64| angle / 60.0 * night;
    let fajr = if span_at(-method.fajr_angle, fajr).is_nan()
        || sunrise - fajr > portion(method.fajr_angle)
    {
        sunrise - portion(method.fajr_angle)
    } else {
        fajr
    };
    let isha = match method.isha {
        Isha::Angle(angle) if span_at(-angle, isha).is_nan() || isha - sunset > portion(angle) => {
            sunset + portion(angle)
        }
        _ => isha,
    };
    let margins = method.margins_min.map(|minutes| minutes / 60.0);
    Some([
        fajr + margins[0],
        noon + margins[1],
        asr + margins[2],
        sunset + margins[3],
        isha + margins[4],
    ])
}

/// Days of 2026 sampled for the year's shares (every fifth day).
const SAMPLE_DAYS: std::ops::Range<i64> = 0..73;
const YEAR_2026_UNIX: i64 = 1_767_225_600;

/// Each call's share of the year in the day, evening and night period on the clocks of `zone`
/// (Fajr, Dhuhr, Asr, Maghrib, Isha); a day the sun neither rises nor sets has no calls.
pub fn call_periods(lat: f64, lon: f64, zone: &Tz, method: &Method) -> [[f64; 3]; 5] {
    let mut counts = [[0.0; 3]; 5];
    for sample in SAMPLE_DAYS {
        let day_unix = YEAR_2026_UNIX + sample * 5 * 86_400;
        let Some(hours) = prayer_hours(day_unix, lat, lon, method) else {
            continue;
        };
        for (call, hour) in hours.iter().enumerate() {
            let instant = day_unix as f64 + hour * 3_600.0;
            counts[call][usize::from(period_in(zone, instant))] += 1.0;
        }
    }
    counts.map(|call| call.map(|days| days / SAMPLE_DAYS.end as f64))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minutes past local midnight of `hour_utc` at UTC+`offset`.
    fn local_minutes(hour_utc: f64, offset: f64) -> f64 {
        (hour_utc + offset).rem_euclid(24.0) * 60.0
    }

    /// Istanbul (Diyanet 18/17 with its margins: Dhuhr +5, Asr +4, Maghrib +7 minutes on the
    /// year's average, research 2026-10-04) on 2026-10-04 at UTC+3 against Diyanet's own table:
    /// imsak 05:32, Dhuhr 12:58, Asr 16:13, Maghrib 18:50, Isha 20:09.
    #[test]
    fn istanbul_matches_the_official_table() {
        let turkey = Method {
            margins_min: [0.0, 5.0, 4.0, 7.0, 0.0],
            ..WORLD_LEAGUE
        };
        let day = 1_791_072_000; // 2026-10-04 00:00 UTC
        let hours = prayer_hours(day, 41.0082, 28.9784, &turkey).unwrap();
        let official = [
            5.0 * 60.0 + 32.0,
            12.0 * 60.0 + 58.0,
            16.0 * 60.0 + 13.0,
            18.0 * 60.0 + 50.0,
            20.0 * 60.0 + 9.0,
        ];
        for (hour, official) in hours.iter().zip(official) {
            let error = local_minutes(*hour, 3.0) - official;
            assert!(error.abs() <= 2.5, "{hours:?}: {error} min");
        }
    }

    /// Jakarta's calls (Kemenag 20/18): Fajr always at night, Maghrib always by day, Isha mostly
    /// in the evening (the research's official 2026 table: 12 % by day); Cologne's summer Isha
    /// comes after 23:00 (the research's 30 % at night) and its Fajr is always before 07:00.
    #[test]
    fn calls_fall_in_their_periods_over_the_year() {
        let jakarta: Tz = "Asia/Jakarta".parse().unwrap();
        let kemenag = Method {
            fajr_angle: 20.0,
            isha: Isha::Angle(18.0),
            asr_factor: 1.0,
            margins_min: [2.5, 3.5, 2.4, 3.1, 2.4],
        };
        let shares = call_periods(-6.2, 106.85, &jakarta, &kemenag);
        assert_eq!(shares[0], [0.0, 0.0, 1.0]);
        assert_eq!(shares[3], [1.0, 0.0, 0.0]);
        assert!(
            shares[4][0] > 0.03 && shares[4][0] < 0.25,
            "{:?}",
            shares[4]
        );
        let berlin: Tz = "Europe/Berlin".parse().unwrap();
        let cologne = call_periods(50.94, 6.96, &berlin, &WORLD_LEAGUE);
        assert_eq!(cologne[0], [0.0, 0.0, 1.0]);
        assert!(
            cologne[4][2] > 0.2 && cologne[4][2] < 0.4,
            "{:?}",
            cologne[4]
        );
        assert_eq!(cologne[1], [1.0, 0.0, 0.0]);
    }
}
