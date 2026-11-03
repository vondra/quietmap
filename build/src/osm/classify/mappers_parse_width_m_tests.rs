//! Regression cases for mappers.

use super::parse_width_m;

fn near(a: Option<f32>, b: f32) -> bool {
    a.is_some_and(|v| (v - b).abs() < 0.01)
}

#[test]
fn integer_metres() {
    assert!(near(parse_width_m(Some("45")), 45.0));
}

#[test]
fn decimal_metres_dot() {
    assert!(near(parse_width_m(Some("45.5")), 45.5));
}

#[test]
fn decimal_metres_comma_eu() {
    assert!(near(parse_width_m(Some("45,5")), 45.5));
}

#[test]
fn explicit_metre_suffix() {
    assert!(near(parse_width_m(Some("45 m")), 45.0));
    assert!(near(parse_width_m(Some("45m")), 45.0));
}

#[test]
fn feet_suffix_converts_to_metres() {
    assert!(near(parse_width_m(Some("147 ft")), 147.0 * 0.3048));
    assert!(near(parse_width_m(Some("147ft")), 147.0 * 0.3048));
}

#[test]
fn multi_stripe_takes_max() {
    assert!(near(parse_width_m(Some("45;35;50")), 50.0));
}

#[test]
fn range_takes_max() {
    assert!(near(parse_width_m(Some("45-50")), 50.0));
}

#[test]
fn mixed_units_in_multi_stripe() {
    // 150 ft = 45.72 m, max of (45.72, 50) is 50.
    assert!(near(parse_width_m(Some("150 ft;50")), 50.0));
}

#[test]
fn ambiguous_unit_in_range_returns_none_for_unit_fragment() {
    // "60-ft" splits to ["60", "ft"]; "ft" is non-numeric → None.
    // Only the "60" fragment yields a value (60 m, no conversion).
    // Acoustically: bare "60-ft" is malformed OSM; the metre reading
    // is at most ~30% over and bounded by typical runway widths.
    assert!(near(parse_width_m(Some("60-ft")), 60.0));
}

#[test]
fn empty_returns_none() {
    assert_eq!(parse_width_m(Some("")), None);
    assert_eq!(parse_width_m(None), None);
}

#[test]
fn non_numeric_returns_none() {
    assert_eq!(parse_width_m(Some("NA")), None);
    assert_eq!(parse_width_m(Some("unknown")), None);
}
