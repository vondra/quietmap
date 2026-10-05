//! People outside bars, pubs, nightclubs, beer gardens, restaurants, cafés and fast-food places,
//! and the crowds in the streets of nightlife clusters (research 2026-10-04, people). A person
//! present emits a talker's sound power with a third of them talking at once (Hodgson via Hayne
//! 2011; the ANSI S3.5 talker powers of Rindel & Christensen 2012): eating 63.6 dB(A) (a normal
//! voice), lively and seated 70.7 (a raised one), standing at night 73.0 (between raised and loud:
//! Ramón-Turner 2025 72-74, Flanagan 2023 72-74, Hayne 2011 72.5 at fifty people). The Austrian
//! design values (63 normal, 71 lively), Andalusia's 70 a seat and the measured crowds fall on
//! this ladder.
//!
//! Seats outside are taken 0.30 by day (a judgment: nothing measured), 0.75 in the evenings and
//! nights of weekends (ÖNORM's design occupancy; 31 people a patio at the evening rush, Kaplan
//! 2019), on other days 2.5 dB fewer in the evening and 6 dB fewer at night (Melbourne's counts
//! 1.5-2.0 and 2.9-6.5 times, Paris's busy and quiet nights 5-6 dB apart). Drinking places keep
//! 2.4 people at their door while open (Kaplan 2019, 72 bars), and the hour after they close
//! still carries their last hour's people 5 dB down (Paris's Halles profile). Seats and street
//! crowds count in the terrace season only.

use super::DAY_ONLY_OFFSET_DB;
use super::spectrum::SoundPower;
use crate::bands::{BANDS, PERIOD_HOURS};

pub const EATING_DBA: f64 = 63.6;
pub const LIVELY_DBA: f64 = 70.7;
pub const STANDING_DBA: f64 = 73.0;
/// A raised voice's bands, 63 Hz to 8 kHz, Z-weighted (Rindel & Christensen 2012 Table 1).
pub const VOICE_SPECTRUM: [f64; BANDS] = [-26.9, -15.9, -5.4, 0.0, -3.0, -11.1, -17.6, -26.5];
/// People at a drinking place's door while it is open (Kaplan et al. 2019).
pub const DOOR_PEOPLE: f64 = 2.4;

const DAY_OCCUPANCY: f64 = 0.30;
const PEAK_OCCUPANCY: f64 = 0.75;
const WEEKDAY_EVENING_DB: f64 = -2.5;
const WEEKDAY_NIGHT_DB: f64 = -6.0;
const DISPERSAL_DB: f64 = -5.0;
/// A cluster's street crowd covers 40 m of an 8 m street (Ballesteros 2014's segments).
const CROWD_AREA_M2: f64 = 40.0 * 8.0;

/// Hours of a week, `[weekday][hour]`, Monday first.
pub type WeekHours = [[bool; 24]; 7];

/// A place's people outside: its seats and the share of the year they are used, its open and
/// terrace hours, the hour its door crowd starts (`None`: none), what its seated guests do in the
/// evening and at night (dB(A) a person), whether it empties into the street when it closes, and
/// its share of a cluster's street crowd (dB(A) at the weekend's peak; `None` outside clusters).
pub struct Venue<'a> {
    pub seats: f64,
    pub season_share: f64,
    pub open: &'a WeekHours,
    pub terrace: &'a WeekHours,
    pub door_from_hour: Option<usize>,
    pub evening_dba: f64,
    pub night_dba: f64,
    pub drinking: bool,
    pub crowd_dba: Option<f64>,
}

fn period(hour: usize) -> usize {
    match hour {
        7..=18 => 0,
        19..=22 => 1,
        _ => 2,
    }
}

/// Friday's and Saturday's evenings and the nights after them.
fn weekend(day: usize, hour: usize) -> bool {
    if hour < 7 {
        day == 5 || day == 6
    } else {
        day == 4 || day == 5
    }
}

fn energy(level_dba: f64) -> f64 {
    10f64.powf(level_dba / 10.0)
}

/// The people of a place hour by hour of the week as sound power (energy), before the dispersal.
fn hour_energy(venue: &Venue, day: usize, hour: usize) -> f64 {
    let p = period(hour);
    let weekday_db = match (p, weekend(day, hour)) {
        (0, _) | (_, true) => 0.0,
        (1, false) => WEEKDAY_EVENING_DB,
        _ => WEEKDAY_NIGHT_DB,
    };
    let busy = energy(weekday_db);
    let seated = if venue.terrace[day][hour] {
        let occupancy = if p == 0 {
            DAY_OCCUPANCY
        } else {
            PEAK_OCCUPANCY
        };
        let per_person = [EATING_DBA, venue.evening_dba, venue.night_dba][p];
        venue.seats * venue.season_share * occupancy * busy * energy(per_person)
    } else {
        0.0
    };
    let at_door = venue.open[day][hour]
        && venue
            .door_from_hour
            .is_some_and(|from| hour >= from || hour < 7);
    let door = if at_door {
        DOOR_PEOPLE * busy * energy(STANDING_DBA)
    } else {
        0.0
    };
    let crowd = match venue.crowd_dba {
        Some(crowd) if venue.open[day][hour] && p == 2 => venue.season_share * busy * energy(crowd),
        _ => 0.0,
    };
    seated + door + crowd
}

/// The place's people as a source: the mean sound power of each period over the week (closed hours
/// silent), a raised voice's spectrum; `None` when nobody is ever outside.
pub fn venue_sound_power(venue: &Venue) -> Option<SoundPower> {
    let mut sums = [0.0; 3];
    for day in 0..7 {
        for hour in 0..24 {
            let mut hour_sum = hour_energy(venue, day, hour);
            let (before_day, before_hour) = if hour == 0 {
                ((day + 6) % 7, 23)
            } else {
                (day, hour - 1)
            };
            let just_closed = venue.open[before_day][before_hour] && !venue.open[day][hour];
            if venue.drinking && just_closed && period(hour) == 2 {
                hour_sum += hour_energy(venue, before_day, before_hour) * energy(DISPERSAL_DB);
            }
            sums[period(hour)] += hour_sum;
        }
    }
    let levels: [f64; 3] = std::array::from_fn(|p| {
        let mean = sums[p] / (7.0 * PERIOD_HOURS[p]);
        if mean > 0.0 {
            10.0 * mean.log10()
        } else {
            f64::NEG_INFINITY
        }
    });
    let loudest = levels.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if !loudest.is_finite() {
        return None;
    }
    // A silent day keeps the near-silence other day-less sources carry.
    let day = if levels[0].is_finite() {
        levels[0]
    } else {
        loudest + DAY_ONLY_OFFSET_DB
    };
    let offset = |level: f64| {
        if level.is_finite() {
            level - day
        } else {
            DAY_ONLY_OFFSET_DB
        }
    };
    Some(SoundPower {
        day_dba: day,
        spectrum_db: VOICE_SPECTRUM,
        evening_offset_db: offset(levels[1]),
        night_offset_db: offset(levels[2]),
    })
}

/// The street crowd of a cluster at the weekend's peak (dB(A) over its 40 m), from the weighted
/// count `n` of drinking places along 40 m of street (bars 0.8, pubs 1, nightclubs 2, restaurants
/// 0.6): Ballesteros 2014's soundwalk level LAeq = 62.8 + 7.4 ln n (Madrid, Cuenca, Saturdays
/// 01:30-03:00) read as a density DP = ((7.4 ln n - 0.9) / 24.5)^2 people a m2 through the same
/// thesis's density law LAeq = 62.2 + 24.5 sqrt(DP) at 4 m (the middle of the research's bounds),
/// each person at 71.7 + 7.41 DP dB(A) (Ramón-Turner 2025). `None` under three places.
pub fn street_crowd_dba(n: f64) -> Option<f64> {
    if n < 3.0 {
        return None;
    }
    let root = (7.4 * n.ln() - 0.9) / 24.5;
    let density = root * root;
    Some(71.7 + 7.41 * density + 10.0 * (density * CROWD_AREA_M2).log10())
}

/// Seats and street crowds count in the terrace season, the share of the year a day's mean
/// temperature reaches 12.5 C (a judgment reproducing Paris's half year of terraces, Chauvineau
/// 2025's -3 dB, and Spain's -1 to -1.5 dB; colder places less), at least a tenth of it.
pub const TERRACE_SEASON_C: f64 = 12.5;
pub const LEAST_SEASON_SHARE: f64 = 0.1;

#[cfg(test)]
#[path = "people_tests.rs"]
mod tests;
