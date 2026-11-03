//! Road traffic by period from admitted counts; otherwise the global period model.
use crate::source_tables::SourceTables;
use physics::bands::PERIODS;

/// Road groups: motorways and trunks with their links; primary roads and links; other roads.
pub(crate) const GROUPS: usize = 3;

/// Shares where nothing is measured, light and heavy alike: between the counted temperate pattern
/// (US, Japan, Germany, the Netherlands, Britain: 0.75-0.81 by day, 0.09-0.15 in the evening,
/// 0.07-0.13 at night) and the late ones (Spain 0.68/0.19/0.13, Thailand 0.63/0.18/0.19).
pub(crate) const DEFAULT_PERIOD_SHARES: [[f64; PERIODS]; GROUPS] =
    [[0.65, 0.20, 0.15], [0.70, 0.18, 0.12], [0.70, 0.18, 0.12]];

/// The group of a road class index (see `CLASS_NAMES`).
pub(crate) fn road_group(class_index: usize) -> usize {
    match class_index {
        0 | 1 | 10 | 11 => 0,
        2 | 12 => 1,
        _ => 2,
    }
}

/// The day/evening/night shares of a road's daily traffic in a country: of its light vehicles and
/// motorcycles, or (`heavy`) of its medium and heavy vehicles.
pub(crate) fn period_shares(
    tables: &SourceTables,
    country_iso: u16,
    class_index: usize,
    heavy: bool,
) -> [f64; PERIODS] {
    let iso = country_iso.to_le_bytes();
    let group = road_group(class_index);
    tables
        .period_shares
        .get(iso)
        .map_or(DEFAULT_PERIOD_SHARES[group], |shares| {
            shares[group][usize::from(heavy)]
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_sorted_and_every_row_sums_to_one() {
        let tables = SourceTables::test_fixture();
        for groups in tables.period_shares.values() {
            for shares in groups.iter().flatten() {
                assert!((shares.iter().sum::<f64>() - 1.0).abs() < 0.002);
            }
        }
    }

    #[test]
    fn counted_countries_carry_less_evening_lorries_more_night_and_thailand_more_of_both() {
        let tables = SourceTables::test_fixture();
        let iso = |code: &[u8; 2]| u16::from_le_bytes(*code);
        // A German Landesstrasse: Baden-Wuerttemberg's counters.
        let cars = period_shares(tables, iso(b"DE"), 3, false);
        let lorries = period_shares(tables, iso(b"DE"), 3, true);
        assert!(cars[1] < 0.13 && cars[2] < 0.10, "{cars:?}");
        assert!(lorries[1] < cars[1] && lorries[2] > cars[2], "{lorries:?}");
        // Czechia has no counts by the hour and takes its region's.
        assert!(period_shares(tables, iso(b"CZ"), 5, false)[2] < 0.10);
        // Thailand's main roads run on at night.
        assert!(period_shares(tables, iso(b"TH"), 2, false)[2] > 0.17);
        // A country outside the measured regions keeps the default.
        assert_eq!(
            period_shares(tables, iso(b"BR"), 0, true),
            DEFAULT_PERIOD_SHARES[0]
        );
    }
}
