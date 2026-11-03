//! Regression cases for ways.

use super::{
    ParkingKind, has_a_building, is_a_closed_ring, is_functional_area, is_leisure_area,
    parking_kind,
};

fn tag_of<'a>(tags: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<&'a str> + 'a {
    move |key: &str| tags.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

/// Praha, Na Špitálce (way 1342239310): a 2 m wide `parking=street_side`
/// strip stood in the served world as a 7 m wall, and the popup called the
/// pavement beside it indoors. Open ground is a SOURCE and never a building
/// row; a deck or a basement keeps the building lane; a ground with a
/// function still routes as one.
#[test]
fn a_car_park_routes_by_what_it_physically_is() {
    let kind = |tags: &[(&str, &str)]| parking_kind(tag_of(tags));
    let na_spitalce: &[(&str, &str)] = &[("amenity", "parking"), ("parking", "street_side")];
    assert_eq!(kind(na_spitalce), Some(ParkingKind::OpenStrip));
    assert_eq!(kind(&[("amenity", "parking")]), Some(ParkingKind::OpenLot));
    assert_eq!(
        kind(&[("amenity", "parking"), ("parking", "surface")]),
        Some(ParkingKind::OpenLot)
    );
    // A car park that IS a building stands, whatever value the tag carries.
    assert_eq!(
        kind(&[("amenity", "parking"), ("building", "yes")]),
        Some(ParkingKind::Structure)
    );
    assert_eq!(
        kind(&[("building", "garage")]),
        Some(ParkingKind::Structure)
    );
    for structure in [
        [("amenity", "parking"), ("parking", "multi-storey")],
        [("amenity", "parking"), ("parking", "carports")],
    ] {
        assert_eq!(
            kind(&structure),
            Some(ParkingKind::Structure),
            "{structure:?}"
        );
    }
    for below in [
        [("amenity", "parking"), ("parking", "underground")],
        [("amenity", "parking"), ("location", "underground")],
    ] {
        assert_eq!(kind(&below), Some(ParkingKind::Underground), "{below:?}");
    }
    // A single stall inside a lot, and cars on a roof: already counted.
    for inside in [
        [("amenity", "parking_space"), ("parking", "surface")],
        [("amenity", "parking"), ("parking", "rooftop")],
    ] {
        assert_eq!(
            kind(&inside),
            Some(ParkingKind::NotItsOwnSource),
            "{inside:?}"
        );
    }
    // Both routers ask `parking_kind` before any land use, so a lot inside a
    // retail or industrial zone is a lot — the zone never claims it.
    assert_eq!(
        kind(&[("amenity", "parking"), ("landuse", "retail")]),
        Some(ParkingKind::OpenLot)
    );
    assert_eq!(
        kind(&[
            ("amenity", "parking"),
            ("parking", "multi-storey"),
            ("landuse", "industrial")
        ]),
        Some(ParkingKind::Structure)
    );
    // The car park routes on its own CLOSED ring, not through the leisure
    // tag gate: that gate is shared with nodes, which have no area at all.
    assert!(!is_leisure_area(na_spitalce));
    assert!(is_a_closed_ring(&[7, 8, 9, 7]));
    assert!(
        !is_a_closed_ring(&[7, 8, 9, 10]),
        "a lane drawn as a line has no area"
    );
    assert!(!is_a_closed_ring(&[7, 7]));
    // `building=no` says there is no building: it must not route one by its
    // mere presence.
    assert!(!has_a_building(tag_of(&[("building", "no")])));
    assert!(has_a_building(tag_of(&[("building", "yes")])));
    // A ground with a function is still a building-lane source.
    assert!(is_functional_area(tag_of(&[("amenity", "school")])));
    assert!(is_functional_area(tag_of(&[("landuse", "retail")])));
    assert!(!is_functional_area(tag_of(&[("leisure", "pitch")])));
}

#[test]
fn road_keep_list_carries_old_ref_for_the_fallback() {
    assert!(keep_road_tag("ref"));
    assert!(keep_road_tag("old_ref"));
    assert!(!keep_road_tag("int_ref"));
    assert!(!keep_road_tag("tiger:reviewed"));
}

use crate::osm::classify::tags::keep_road_tag;
