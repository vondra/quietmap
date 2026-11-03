//! Regression cases for mappers.

use super::*;
use crate::osm::classify::ways::is_leisure_area;
use crate::osm::ids as lz;

fn tags(pairs: &[(&str, &str)]) -> Tags {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn car_park_kind_decides_how_many_spaces_its_area_holds() {
    let parking = |kind: Option<&str>| {
        let mut t = tags(&[("amenity", "parking")]);
        if let Some(kind) = kind {
            t.insert("parking".into(), kind.into());
        }
        leisure_sport_class(&t)
    };
    assert_eq!(parking(None), ids::LEISURE_CAR_PARK);
    assert_eq!(parking(Some("surface")), ids::LEISURE_CAR_PARK);
    assert_eq!(parking(Some("street_side")), ids::LEISURE_CAR_PARK_STREET);
    assert_eq!(parking(Some("lane")), ids::LEISURE_CAR_PARK_STREET);
}

#[test]
fn leisure_sport_class_resolution() {
    assert_eq!(
        leisure_sport_class(&tags(&[("sport", "padel")])),
        lz::LEISURE_PADEL
    );
    assert_eq!(
        leisure_sport_class(&tags(&[("leisure", "playground")])),
        lz::LEISURE_PLAYGROUND
    );
    assert_eq!(
        leisure_sport_class(&tags(&[("amenity", "biergarten")])),
        lz::LEISURE_OUTDOOR_SEATING
    );
    assert_eq!(
        leisure_sport_class(&tags(&[("leisure", "pitch")])),
        lz::LEISURE_PITCH
    );
    // Multi-value sport → loudest (padel > tennis).
    assert_eq!(
        leisure_sport_class(&tags(&[("sport", "tennis;padel")])),
        lz::LEISURE_PADEL
    );
    // Artificial turf remaps only pitch classes to the booked duty.
    assert_eq!(
        leisure_sport_class(&tags(&[
            ("leisure", "pitch"),
            ("surface", "artificial_turf")
        ])),
        lz::LEISURE_AGP
    );
    assert_eq!(
        leisure_sport_class(&tags(&[
            ("sport", "soccer"),
            ("surface", "artificial_turf")
        ])),
        lz::LEISURE_AGP
    );
    assert_eq!(
        leisure_sport_class(&tags(&[("sport", "soccer"), ("surface", "grass")])),
        lz::LEISURE_PITCH
    );
    assert_eq!(
        leisure_sport_class(&tags(&[
            ("sport", "tennis"),
            ("surface", "artificial_turf")
        ])),
        lz::LEISURE_TENNIS
    );
}

#[test]
fn leisure_area_gating() {
    assert!(is_leisure_area(&[("leisure", "playground")]));
    assert!(is_leisure_area(&[("leisure", "pitch")]));
    assert!(is_leisure_area(&[("amenity", "biergarten")]));
    assert!(is_leisure_area(&[("outdoor_seating", "yes")]));
    // Private back-yard pool (no access/public) is NOT a source.
    assert!(!is_leisure_area(&[("leisure", "swimming_pool")]));
    // Public pool is.
    assert!(is_leisure_area(&[
        ("leisure", "swimming_pool"),
        ("access", "public")
    ]));
    // Parks/gardens are not sources.
    assert!(!is_leisure_area(&[("leisure", "garden")]));
}

#[test]
fn opening_hours_day_fraction() {
    assert_eq!(opening_hours_fraction(Some("24/7")), 1);
    assert_eq!(opening_hours_fraction(Some("Mo-Fr 08:00-18:00")), 2);
    assert_eq!(opening_hours_fraction(Some("Mo-Su 11:00-23:00")), 3); // late close
    assert_eq!(opening_hours_fraction(Some("Tu-Su 18:00-02:00")), 3); // after-midnight
    assert_eq!(opening_hours_fraction(None), 0);
    assert_eq!(opening_hours_fraction(Some("")), 0);
}
