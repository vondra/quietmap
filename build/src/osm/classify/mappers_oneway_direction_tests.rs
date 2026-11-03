//! Regression cases for mappers.

use super::oneway_direction;

#[test]
fn explicit_values_win_over_everything() {
    for value in ["yes", "true", "1"] {
        assert_eq!(
            oneway_direction("residential", Some(value), Some("roundabout")),
            1
        );
        assert_eq!(oneway_direction("motorway", Some(value), None), 1);
    }
    for value in ["-1", "reverse"] {
        assert_eq!(oneway_direction("residential", Some(value), None), 2);
        // Explicit reverse beats the implicit motorway forward.
        assert_eq!(oneway_direction("motorway", Some(value), None), 2);
    }
    for value in ["no", "false", "0"] {
        assert_eq!(
            oneway_direction("motorway", Some(value), Some("roundabout")),
            0
        );
    }
}

#[test]
fn implicit_roundabout_and_motorway_are_single_direction() {
    assert_eq!(oneway_direction("trunk", None, Some("roundabout")), 3);
    assert_eq!(
        oneway_direction("residential", None, Some("mini_roundabout")),
        3
    );
    assert_eq!(oneway_direction("motorway", None, None), 4);
    assert_eq!(oneway_direction("motorway_link", None, None), 4);
    // Nothing implies two-way carriageways become single-direction.
    assert_eq!(oneway_direction("trunk", None, None), 0);
    assert_eq!(oneway_direction("primary", None, Some("circular")), 0);
    assert_eq!(oneway_direction("residential", None, None), 0);
}

#[test]
fn unrecognized_values_are_not_single_direction_evidence() {
    // Alternating/tidal flow and malformed values must not become a
    // one-way carriageway, and they suppress the implicit cases.
    assert_eq!(oneway_direction("motorway", Some("alternating"), None), 0);
    assert_eq!(oneway_direction("trunk", Some(""), Some("roundabout")), 3);
    assert_eq!(oneway_direction("residential", Some("maybe"), None), 0);
}
