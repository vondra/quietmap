//! What a posted speed means: one rule for the signed-speed fit, the graded transitions and the sources.
use crate::source_tables::CountryTable;

/// The `speed_limit` code of `maxspeed=none`, a derestricted road.
pub(crate) const DERESTRICTED_CODE: u8 = 255;

/// Walking pace (OpenStreetMap's `maxspeed=walk`): no main road is signed slower.
const WALKING_PACE_KMH: u8 = 10;

/// The fastest legal default of any country in the year's legal-speed table (Bulgaria's 140 on
/// motorways in 2026): no main road is signed faster.
pub(crate) fn fastest_legal_kmh(legal: &CountryTable<[u8; 4]>) -> u8 {
    legal
        .values()
        .flat_map(|speeds| speeds.iter().copied())
        .max()
        .unwrap_or(u8::MAX)
}

/// A road's posted speed as every reader takes it: the sign, [`DERESTRICTED_CODE`], or `None` where
/// nothing is posted or the value is no sign ([`is_sign`]), so the road counts as untagged.
pub(crate) fn signed_speed(class: usize, posted: u8, fastest_legal_kmh: u8) -> Option<u8> {
    match posted {
        0 => None,
        DERESTRICTED_CODE => Some(DERESTRICTED_CODE),
        posted => is_sign(class, posted, fastest_legal_kmh).then_some(posted),
    }
}

/// Whether a posted speed on a road of dev4 `class` is a sign: on a main road, link or unclassified
/// road from walking pace to `fastest_legal_kmh`. Barcelona's Passeig de Maragall, a secondary road,
/// was posted 4 (40 typed short; 79 dB at 15 m where 40-50 km/h give 67), Riyadh's secondary roads 2;
/// 254 is the extractor's clamp of a longer number. Local streets may post walking pace or less. A
/// value that is no sign leaves the road untagged: in the signed-speed medians (Comoros' rural
/// tertiary roads took 6 km/h from them in 2024), as a transition's anchor and in the sources.
fn is_sign(class: usize, posted: u8, fastest_legal_kmh: u8) -> bool {
    !matches!(class, 0..=4 | 9..=12) || (WALKING_PACE_KMH..=fastest_legal_kmh).contains(&posted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_roads_are_signed_from_walking_pace_to_the_fastest_legal_limit() {
        let fastest =
            fastest_legal_kmh(&crate::source_tables::SourceTables::test_fixture().legal_speeds);
        assert_eq!(fastest, 140);
        assert_eq!(
            signed_speed(3, 4, fastest),
            None,
            "a secondary road at 4 km/h"
        );
        assert_eq!(signed_speed(2, 254, fastest), None, "the clamp");
        assert_eq!(signed_speed(0, 140, fastest), Some(140));
        assert_eq!(signed_speed(0, 141, fastest), None);
        assert_eq!(signed_speed(4, 10, fastest), Some(10));
        assert_eq!(
            signed_speed(6, 5, fastest),
            Some(5),
            "a living street at walking pace"
        );
        assert_eq!(
            signed_speed(0, DERESTRICTED_CODE, fastest),
            Some(DERESTRICTED_CODE)
        );
        assert_eq!(signed_speed(2, 0, fastest), None, "nothing posted");
    }
}
