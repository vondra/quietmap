//! Regression cases for spill.

use super::*;

fn tags_of(pairs: &[(&str, &str)]) -> Tags {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn only_the_yard_tag_routes_to_the_yard_site_type() {
    assert_eq!(site_type_from_tags(&tags_of(&[("landuse", "railway")])), 0);
    assert_eq!(site_type_from_tags(&tags_of(&[("railway", "yard")])), 5);
    assert_eq!(
        site_type_from_tags(&tags_of(&[("landuse", "railway"), ("railway", "yard")])),
        5
    );
    assert_eq!(
        site_type_from_tags(&tags_of(&[("landuse", "industrial")])),
        0
    );
    assert_eq!(site_type_from_tags(&tags_of(&[("landuse", "quarry")])), 1);
    assert_eq!(site_type_from_tags(&tags_of(&[("landuse", "farmyard")])), 2);
    assert_eq!(site_type_from_tags(&tags_of(&[("man_made", "works")])), 3);
}

#[test]
fn facility_activity_survives_generic_industrial_landuse() {
    for (activity, expected) in [("wastewater_plant", 4), ("works", 3)] {
        assert_eq!(
            site_type_from_tags(&tags_of(&[
                ("landuse", "industrial"),
                ("man_made", activity)
            ])),
            expected
        );
    }
}
