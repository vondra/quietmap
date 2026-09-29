//! Open-air leisure areas (courts, pitches, playgrounds, terraces, car parks) by the settlement
//! area law over their polygon, and outdoor shooting ranges by shots per year. Levels are annual:
//! an active sound power minus a stated season and duty cut (dev4 settlement v2; END does not
//! model sport). Motorsport as dev4 built it is dropped (PLAN-z13 DROP) and emits nothing.

use super::DAY_ONLY_OFFSET_DB;
use super::settlement::area_law_lw_dba;
use super::spectrum::SoundPower;
use crate::bands::BANDS;

/// Leisure class ids of `leisure.arrow` (`leisure_v5`).
pub const PITCH: u8 = 0;
pub const PADEL: u8 = 1;
pub const TENNIS: u8 = 2;
pub const BASKETBALL: u8 = 3;
pub const PLAYGROUND: u8 = 4;
pub const POOL: u8 = 5;
pub const OUTDOOR_SEATING: u8 = 6;
pub const STADIUM: u8 = 7;
pub const CAR_PARK: u8 = 8;
pub const CAR_PARK_STREET: u8 = 9;
pub const MOTORSPORT: u8 = 10;
pub const SHOOTING: u8 = 11;
/// Floodlit artificial-turf pitch.
pub const ARTIFICIAL_TURF_PITCH: u8 = 12;

/// The plant floor all leisure areas share; the per-m2 term carries the level.
const LEISURE_FLOOR_DBA: f64 = 40.0;
/// Pitches count at most 10 ha (PLAN-z13 FIX: 8,739 pitches over 10 ha, 413 over 100 ha, are
/// ski runs and trail networks; one read 40 dB inside a desert singletrack).
pub const PITCH_AREA_CAP_M2: f64 = 100_000.0;

/// A class's area law with its reference footprint (the area of a node without a polygon).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LeisureProfile {
    pub lw_per_m2_dba: f64,
    pub reference_area_m2: f64,
    pub spectrum_db: [f64; BANDS],
    pub evening_offset_db: f64,
    pub night_offset_db: f64,
    /// Lot area per parking space: car parks add the parking study's searching traffic.
    pub m2_per_parking_space: Option<f64>,
    /// The largest area the law counts.
    pub area_cap_m2: f64,
}

const fn profile(
    per_m2: f64,
    reference_m2: f64,
    spectrum: [f64; BANDS],
    evening: f64,
    night: f64,
) -> LeisureProfile {
    LeisureProfile {
        lw_per_m2_dba: per_m2,
        reference_area_m2: reference_m2,
        spectrum_db: spectrum,
        evening_offset_db: evening,
        night_offset_db: night,
        m2_per_parking_space: None,
        area_cap_m2: f64::INFINITY,
    }
}

const PLAYERS: [f64; BANDS] = [-2.0, -1.0, 0.0, 1.0, 1.0, 0.0, -2.0, -4.0];
/// Parkplatzlaermstudie Tab. 25: A-weighted bands of parking movements, turned Z-weighted.
const CAR_PARK_SPECTRUM: [f64; BANDS] = [8.5, -1.0, -4.2, -5.5, -5.3, -5.8, -10.4, -18.5];

/// The area-law profile of a class; `None` for motorsport, shooting and unknown ids.
///
/// Active anchors minus about 9 dB of season and duty: padel 90 (padelcreations, Higgins),
/// tennis 84 (TU Muenchen), basketball tennis - 6, playground and pool estimates, outdoor
/// seating 71 dB(A) per guest (Laermfibel, VDI 3770), stadium match days only (-12). Pitches:
/// 58 dB LAeq,1h at 10 m (Sport England AGP) over 100 x 64 m is 59.8 dB(A)/m2 active; grass is
/// played 4 h by day and 1 h in summer evenings a week for 40 weeks, a booked artificial pitch
/// 40 h a week all year (26 day, 8 evening of 34 peak hours); both are dark at night. Car parks
/// (Parkplatzlaermstudie, LfU Bayern 2007): 63 dB(A) per space and movement, 0.40 movements per
/// space and hour by day (06-22) and 0.05 by night, 23.8 m2 per space on a lot and 13.3 on a
/// street strip (medians of OSM polygons with `capacity`), re-averaged onto 19-23 and 23-07.
pub fn leisure_profile(class: u8) -> Option<LeisureProfile> {
    let car_park = |per_m2: f64, reference_m2: f64, m2_per_space: f64| LeisureProfile {
        m2_per_parking_space: Some(m2_per_space),
        ..profile(per_m2, reference_m2, CAR_PARK_SPECTRUM, -1.1, -6.3)
    };
    let pitch = |per_m2: f64, evening: f64| LeisureProfile {
        area_cap_m2: PITCH_AREA_CAP_M2,
        ..profile(per_m2, 7_000.0, PLAYERS, evening, -25.0)
    };
    Some(match class {
        PITCH => pitch(45.4, -1.2),
        ARTIFICIAL_TURF_PITCH => pitch(55.4, -0.3),
        PADEL => profile(
            58.0,
            200.0,
            [-6.0, -4.0, -2.0, -1.0, 0.0, 1.0, 2.0, 1.0],
            0.0,
            -15.0,
        ),
        TENNIS => profile(
            50.0,
            260.0,
            [-5.0, -4.0, -2.0, -1.0, 0.0, 1.0, 1.0, 0.0],
            -3.0,
            -20.0,
        ),
        BASKETBALL => profile(
            42.0,
            420.0,
            [-4.0, -3.0, -1.0, 0.0, 0.0, 0.0, 0.0, -1.0],
            -3.0,
            -20.0,
        ),
        PLAYGROUND => profile(
            48.0,
            200.0,
            [-3.0, -1.0, 1.0, 2.0, 1.0, 0.0, -2.0, -5.0],
            -5.0,
            -25.0,
        ),
        POOL => profile(
            50.0,
            400.0,
            [-3.0, -2.0, 0.0, 1.0, 1.0, 0.0, -2.0, -5.0],
            -5.0,
            -25.0,
        ),
        OUTDOOR_SEATING => profile(
            55.0,
            12.0,
            [-2.0, -1.0, 1.0, 2.0, 1.0, 0.0, -3.0, -6.0],
            0.0,
            -15.0,
        ),
        STADIUM => profile(40.0, 7_000.0, PLAYERS, -3.0, -12.0),
        CAR_PARK => car_park(45.3, 1_000.0, 23.8),
        CAR_PARK_STREET => car_park(47.8, 100.0, 13.3),
        _ => return None,
    })
}

/// A leisure area of `class` over `area_m2`; `None` for classes without an area law.
pub fn leisure_sound_power(class: u8, area_m2: f64) -> Option<SoundPower> {
    let profile = leisure_profile(class)?;
    let counted_m2 = area_m2.min(profile.area_cap_m2);
    // Parkplatzlaermstudie Formula 3: K_D = 2.5 lg(B - 9) above ten spaces.
    let searching_traffic_db = match profile.m2_per_parking_space {
        Some(per_space) if area_m2 / per_space > 10.0 => 2.5 * (area_m2 / per_space - 9.0).log10(),
        _ => 0.0,
    };
    Some(SoundPower {
        day_dba: area_law_lw_dba(LEISURE_FLOOR_DBA, profile.lw_per_m2_dba, counted_m2)
            + searching_traffic_db,
        spectrum_db: profile.spectrum_db,
        evening_offset_db: profile.evening_offset_db,
        night_offset_db: profile.night_offset_db,
    })
}

/// A shooting range's loudest discipline; `Silent` for archery, paintball and air guns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShootingSubtype {
    Rifle,
    Pistol,
    Shotgun,
    Silent,
}

fn tokens(value: &str) -> impl Iterator<Item = String> + '_ {
    value
        .split(';')
        .map(|token| token.trim().to_ascii_lowercase().replace(['-', ' '], "_"))
}

/// The discipline from the `shooting` tag, the `shooting:*` values, then the name: the loudest
/// firearm named wins (rifle over shotgun over pistol), a quiet discipline alone is silent and
/// an untyped range fires rifles.
pub fn shooting_subtype(shooting: Option<&str>, details: &[&str], name: &str) -> ShootingSubtype {
    let tags: Vec<String> = shooting
        .into_iter()
        .chain(details.iter().copied())
        .flat_map(tokens)
        .collect();
    let tagged = |words: &[&str]| tags.iter().any(|tag| words.contains(&tag.as_str()));
    let named = |words: &[&str]| {
        let name = name.to_ascii_lowercase();
        words.iter().any(|word| name.contains(word))
    };
    let shotgun = [
        "shotgun",
        "clay",
        "clay_pigeon",
        "claypigeon",
        "skeet",
        "trap",
    ];
    let pistol = ["pistol", "handgun", "ipsc"];
    let quiet = [
        "archery",
        "crossbow",
        "paintball",
        "airsoft",
        "laser",
        "laser_tag",
        "lasertag",
        "air_gun",
        "airgun",
        "air_rifle",
        "air_pistol",
        "blowgun",
        "indoor",
        "indoor_range",
        "virtual",
    ];
    if tagged(&["rifle"]) {
        ShootingSubtype::Rifle
    } else if tagged(&shotgun) {
        ShootingSubtype::Shotgun
    } else if tagged(&pistol) {
        ShootingSubtype::Pistol
    } else if tagged(&quiet) {
        ShootingSubtype::Silent
    } else if named(&["rifle"]) {
        ShootingSubtype::Rifle
    } else if named(&["clay", "skeet", "trap", "shotgun"]) {
        ShootingSubtype::Shotgun
    } else if named(&pistol) {
        ShootingSubtype::Pistol
    } else if named(&["archery", "crossbow", "paintball", "airsoft", "laser"]) {
        ShootingSubtype::Silent
    } else {
        ShootingSubtype::Rifle
    }
}

/// Shots a year of a range without register data (dev4 default, the Zurich register 64-ZH
/// carries real counts), spread over the 12 h day of 365 days.
const SHOTS_PER_YEAR: f64 = 20_000.0;
const DAY_SECONDS_PER_YEAR: f64 = 12.0 * 3_600.0 * 365.0;

/// A range's annual day sound power: the single-shot energy level L_E (RIVM Defensie emission
/// table 2024-10-10, sphere sums of 9 mm Glock 133.6, .308 Accuracy AW 139.0 and 12 ga clay
/// shot 134.8; 8 kHz continues the 2-4 kHz slope) spread over the day's seconds. Shots are
/// directional (9 mm +4.8 forward, -7 rear); the model has no direction. `None` when silent.
pub fn shooting_sound_power(subtype: ShootingSubtype) -> Option<SoundPower> {
    let (single_shot_level_db, spectrum_db) = match subtype {
        ShootingSubtype::Rifle => (139.0, [-15.0, -7.3, -0.2, 2.4, 0.0, -4.4, -5.7, -7.0]),
        ShootingSubtype::Pistol => (133.6, [-25.4, -16.4, -7.5, -1.2, 0.0, -5.8, -9.7, -13.6]),
        ShootingSubtype::Shotgun => (134.8, [-15.6, -7.5, -0.5, 1.4, 0.0, -3.4, -6.1, -8.8]),
        ShootingSubtype::Silent => return None,
    };
    Some(SoundPower {
        day_dba: single_shot_level_db + 10.0 * (SHOTS_PER_YEAR / DAY_SECONDS_PER_YEAR).log10(),
        spectrum_db,
        evening_offset_db: DAY_ONLY_OFFSET_DB,
        night_offset_db: DAY_ONLY_OFFSET_DB,
    })
}

#[cfg(test)]
#[path = "leisure_tests.rs"]
mod tests;
