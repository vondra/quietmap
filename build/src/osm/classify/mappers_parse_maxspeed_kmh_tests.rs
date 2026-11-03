//! Regression cases for mappers.

use super::{MAXSPEED_NONE, parse_maxspeed_kmh};

#[test]
fn kmh_bare_int() {
    assert_eq!(parse_maxspeed_kmh("300"), 300);
    assert_eq!(parse_maxspeed_kmh("50 km/h"), 50);
    assert_eq!(parse_maxspeed_kmh(" 50 "), 50);
}

#[test]
fn mph_converts() {
    assert_eq!(parse_maxspeed_kmh("70 mph"), 113);
    assert_eq!(parse_maxspeed_kmh("30 mph"), 48);
    assert_eq!(parse_maxspeed_kmh("30mph"), 48);
    assert_eq!(parse_maxspeed_kmh("125 mph"), 201);
}

#[test]
fn knots_converts() {
    assert_eq!(parse_maxspeed_kmh("10 knots"), 19);
}

#[test]
fn none_returns_sentinel() {
    assert_eq!(parse_maxspeed_kmh("none"), MAXSPEED_NONE);
}

#[test]
fn walk_is_10() {
    assert_eq!(parse_maxspeed_kmh("walk"), 10);
}

#[test]
fn non_numeric_states_are_unknown() {
    assert_eq!(parse_maxspeed_kmh("signals"), 0);
    assert_eq!(parse_maxspeed_kmh("variable"), 0);
    assert_eq!(parse_maxspeed_kmh(""), 0);
    assert_eq!(parse_maxspeed_kmh("XX:urban"), 0);
    assert_eq!(parse_maxspeed_kmh("50 apples"), 0);
}

#[test]
fn multi_value_takes_first_token() {
    assert_eq!(parse_maxspeed_kmh("50;30"), 50);
}

#[test]
fn clamps_at_400() {
    assert_eq!(parse_maxspeed_kmh("999"), 400);
}
