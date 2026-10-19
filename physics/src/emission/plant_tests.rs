//! Shops', offices' and restaurants' plant: the hours, the cooling's size and running share, the
//! kitchen extract's size.

use super::*;
use crate::emission::DAY_ONLY_OFFSET_DB;

fn hours(spans: &[(usize, usize)]) -> WeekHours {
    std::array::from_fn(|_| {
        std::array::from_fn(|hour| spans.iter().any(|&(from, to)| (from..to).contains(&hour)))
    })
}

/// Business hours fill 60 of the week's 84 day hours, 6 of its 28 evening hours and no night.
#[test]
fn business_hours_run_six_days_into_the_evening() {
    let [day, evening, night] = period_shares(&BUSINESS_HOURS);
    assert!((day - 60.0 / 84.0).abs() < 1e-12, "{day}");
    assert!((evening - 6.0 / 28.0).abs() < 1e-12, "{evening}");
    assert_eq!(night, 0.0);
}

/// A kitchen opens an hour before its place and closes an hour after it, across midnight and the
/// week's end: Sunday's 23rd hour runs on into Monday's first.
#[test]
fn a_kitchen_runs_an_hour_either_side_of_its_open_hours() {
    let kitchen = kitchen_hours(&hours(&[(12, 15), (19, 23)]));
    let running: Vec<usize> = (0..24).filter(|&hour| kitchen[2][hour]).collect();
    assert_eq!(running, [11, 12, 13, 14, 15, 18, 19, 20, 21, 22, 23]);
    let mut sunday_late = [[false; 24]; 7];
    sunday_late[6][23] = true;
    let kitchen = kitchen_hours(&sunday_late);
    assert!(kitchen[6][22] && kitchen[6][23] && kitchen[0][0]);
    assert_eq!(kitchen.iter().flatten().filter(|&&on| on).count(), 3);
}

/// A 102 m2 shop where it never gets warm (no cooling degree days), in the 30 % of buildings that
/// cool: 16.2 kW of units at 74.3 dB(A), 252 full-load hours of its 3,432 open ones, so heard 1.6 %
/// of the day, its day mean 56.24 dB(A), less in the evening, silent at night.
#[test]
fn a_cold_climates_shop_cools_a_few_open_hours() {
    let plant = cooling_plant(102.0, 0.0, 0.30, &BUSINESS_HOURS).unwrap();
    let (sound, duty) = (plant.sound_power().unwrap(), plant.duty);
    assert!((plant.running_dba - (56.6 + 14.6 * (0.159f64 * 102.0).log10())).abs() < 1e-9);
    let share = 0.30 * (252.2 / 3_432.0) * (60.0 / 84.0);
    assert!((duty[0] - share).abs() < 1e-4, "{duty:?}");
    assert!((duty[1] - duty[0] * (6.0 / 28.0) / (60.0 / 84.0)).abs() < 1e-12);
    assert_eq!(duty[2], 0.0);
    assert!((sound.day_dba - plant.running_dba - 10.0 * duty[0].log10()).abs() < 1e-9);
    assert!((sound.day_dba - 56.24).abs() < 0.01, "{}", sound.day_dba);
    assert_eq!(sound.night_offset_db, DAY_ONLY_OFFSET_DB);
}

/// The units grow with the climate's size a square metre and split at the EU15's mean chiller: a
/// 1,000 m2 office at 40 degree days (164.7 kW) runs at 88.6 dB(A); a hot climate's units run
/// longer, never more than the open hours; a place never open never runs.
#[test]
fn larger_plants_and_hotter_climates_run_louder_and_longer() {
    let office = cooling_plant(1_000.0, 40.0, 0.30, &BUSINESS_HOURS).unwrap();
    assert!((office.running_dba - 88.58).abs() < 0.01, "{office:?}");
    let hot = cooling_plant(1_000.0, 2_100.0, 0.847, &BUSINESS_HOURS).unwrap();
    assert!(hot.duty[0] > 5.0 * office.duty[0]);
    let always = cooling_plant(1_000.0, 30_000.0, 1.0, &BUSINESS_HOURS).unwrap();
    assert!((always.duty[0] - 60.0 / 84.0).abs() < 1e-12, "{always:?}");
    assert_eq!(cooling_plant(1_000.0, 40.0, 0.30, &[[false; 24]; 7]), None);
    assert_eq!(cooling_plant(1_000.0, 40.0, 0.0, &BUSINESS_HOURS), None);
}

/// The Aurich fast-food restaurant's 316 m2 give its measured 72 dB(A); a footprint past the
/// largest prototype restaurant counts that restaurant's kitchen; the extract sounds in its hours
/// only.
#[test]
fn a_kitchen_extract_follows_its_footprint_in_its_hours() {
    let open_daily = hours(&[(11, 23)]);
    let extract = kitchen_extract(316.0, &kitchen_hours(&open_daily)).unwrap();
    // The kitchen runs 10-24: 9 of 12 day hours, the whole evening, one of 8 night hours.
    assert_eq!(extract.duty, [0.75, 1.0, 0.125]);
    assert!((extract.running_dba - 72.0).abs() < 0.01, "{extract:?}");
    let sound = extract.sound_power().unwrap();
    assert!((sound.day_dba - extract.running_dba - 10.0 * 0.75f64.log10()).abs() < 1e-9);
    assert_eq!(
        kitchen_extract(50_000.0, &open_daily),
        kitchen_extract(511.0, &open_daily)
    );
    assert_eq!(kitchen_extract(316.0, &[[false; 24]; 7]), None);
}
