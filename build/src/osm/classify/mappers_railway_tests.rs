//! Regression cases for mappers.

use super::{
    rail_traffic_mode, rail_type, rail_usage_type, railway_carries_trains, railway_is_underground,
};

#[test]
fn traffic_mode_maps_three_values_and_nothing_else() {
    assert_eq!(
        (
            rail_traffic_mode(Some("passenger")),
            rail_traffic_mode(Some("freight")),
            rail_traffic_mode(Some("mixed")),
        ),
        (1, 2, 3)
    );
    for other in [
        None,
        Some(""),
        Some("both"),
        Some("military"),
        Some("Mixed"),
    ] {
        assert_eq!(rail_traffic_mode(other), 0);
    }
}

#[test]
fn subway_is_light_rail_family_and_underground_without_a_tunnel_tag() {
    assert_eq!(rail_type("subway"), rail_type("light_rail"));
    assert_eq!(
        (rail_usage_type(Some("main")), rail_usage_type(None)),
        (0, 3)
    );
    assert!(railway_carries_trains(Some("subway"), None, None));
    assert!(!railway_carries_trains(Some("rail"), Some("yes"), None));
    assert!(!railway_carries_trains(Some("rail"), None, Some("yes")));
    assert!(!railway_carries_trains(Some("platform"), None, None));
    assert!(railway_is_underground(
        "subway",
        None,
        Some("-2"),
        None,
        None
    ));
    assert!(railway_is_underground(
        "subway",
        None,
        None,
        Some("underground"),
        None
    ));
    assert!(!railway_is_underground(
        "subway",
        None,
        Some("1"),
        None,
        None
    ));
    assert!(!railway_is_underground(
        "rail",
        None,
        Some("-1"),
        None,
        None
    ));
    assert!(railway_is_underground(
        "rail",
        Some("yes"),
        None,
        None,
        None
    ));
}
