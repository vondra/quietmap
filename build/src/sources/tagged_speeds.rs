//! Median mapped speed by country, class and surroundings, read from the year tables.
use crate::source_tables::SourceTables;

/// The median tagged speed of a class in a country at a built-up code (1 rural, 2 urban), if
/// enough rows were tagged.
pub fn tagged_speed_kmh(
    tables: &SourceTables,
    class: usize,
    country_iso: u16,
    built_up: u8,
) -> Option<f64> {
    let column = match built_up {
        1 => 0,
        2 => 1,
        _ => return None,
    };
    let row = class.checked_sub(2).filter(|row| *row < 8)?;
    let iso = country_iso.to_le_bytes();
    let speed = tables.tagged_speeds.get(iso)?[row][column];
    (speed > 0).then(|| f64::from(speed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rural_minor_roads_are_signed_below_the_open_road_limit() {
        let tables = SourceTables::test_fixture();
        let iso = |code: &[u8; 2]| u16::from_le_bytes(*code);
        assert_eq!(tagged_speed_kmh(tables, 9, iso(b"CZ"), 1), Some(50.0));
        assert_eq!(tagged_speed_kmh(tables, 9, iso(b"TH"), 1), Some(50.0));
        assert_eq!(tagged_speed_kmh(tables, 9, iso(b"TH"), 0), None);
        assert_eq!(tagged_speed_kmh(tables, 0, iso(b"TH"), 1), None);
    }
}
