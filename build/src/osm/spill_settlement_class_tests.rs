//! Regression cases for spill.

use super::*;
use crate::osm::ids as st;

#[test]
fn a_building_with_a_generator_keeps_its_settlement_source() {
    for (building, expected) in [
        ("house", st::SETTLEMENT_HOUSE),
        ("retail", st::SETTLEMENT_FOOD_RETAIL),
    ] {
        for source in ["solar", "wind", "gas"] {
            let tags = Tags::from([
                ("building".into(), building.into()),
                ("power".into(), "generator".into()),
                ("generator:source".into(), source.into()),
            ]);
            let get = |key: &str| tags.get(key).map(String::as_str);
            assert_eq!(building_type_from_tags(&tags), expected, "{tags:?}");
            assert_eq!(
                classify::feature_types(FeatureType::Building, get),
                vec![FeatureType::Building]
            );
        }
    }
    for pairs in [
        [("building", "yes"), ("power", "transformer")],
        [("building", "yes"), ("disused:power", "substation")],
    ] {
        let tags: Tags = pairs
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();
        assert!(
            classify::feature_types(FeatureType::Building, |key| tags
                .get(key)
                .map(String::as_str))
            .contains(&FeatureType::Industrial)
        );
    }
    for power in ["plant", "substation"] {
        let tags = Tags::from([
            ("building".into(), "yes".into()),
            ("power".into(), power.into()),
        ]);
        assert_eq!(building_type_from_tags(&tags), st::SETTLEMENT_SILENT);
    }
}

#[test]
fn an_indoor_karting_hall_keeps_its_building_emission() {
    // SPEC silences the roofed formula row BECAUSE its building emits: the
    // building must not be silent too, or the hall emits nothing at all.
    let tags = Tags::from([
        ("building".into(), "sports_hall".into()),
        ("leisure".into(), "sports_centre".into()),
        ("sport".into(), "karting".into()),
        ("indoor".into(), "yes".into()),
    ]);
    assert_eq!(building_type_from_tags(&tags), 9);
    // Power buildings stay silent (their Industrial row owns the emission).
    let power = Tags::from([
        ("building".into(), "yes".into()),
        ("power".into(), "substation".into()),
    ]);
    assert_eq!(building_type_from_tags(&power), st::SETTLEMENT_SILENT);
}

#[test]
fn building_type_splits_house_from_apartments() {
    assert_eq!(building_type("apartments"), 0);
    assert_eq!(building_type("residential"), 0);
    assert_eq!(building_type("yes"), 0);
    assert_eq!(building_type("house"), st::SETTLEMENT_HOUSE);
    assert_eq!(building_type("detached"), st::SETTLEMENT_HOUSE);
    assert_eq!(building_type("terrace"), st::SETTLEMENT_HOUSE);
}

#[test]
fn building_type_food_retail_and_hospitality() {
    assert_eq!(building_type("supermarket"), st::SETTLEMENT_FOOD_RETAIL);
    assert_eq!(building_type("retail"), st::SETTLEMENT_FOOD_RETAIL);
    assert_eq!(building_type("restaurant"), st::SETTLEMENT_HOSPITALITY);
    assert_eq!(building_type("pub"), st::SETTLEMENT_HOSPITALITY);
    assert_eq!(building_type("commercial"), 1);
    assert_eq!(building_type("office"), 1);
}

#[test]
fn silent_tail_routed_out_of_residential() {
    for v in [
        "shed",
        "roof",
        "hut",
        "greenhouse",
        "ruins",
        "construction",
        "carport_roof",
    ] {
        assert_eq!(
            building_type(v),
            st::SETTLEMENT_SILENT,
            "{v} must be SILENT"
        );
    }
    assert_eq!(building_type("some_unknown_value"), 0);
}

#[test]
fn poi_class_priorities() {
    assert_eq!(poi_class(Some("hospital"), None, None, None), Some(4));
    assert_eq!(poi_class(Some("school"), None, None, None), Some(3));
    assert_eq!(
        poi_class(Some("cafe"), None, None, None),
        Some(st::SETTLEMENT_HOSPITALITY)
    );
    assert_eq!(
        poi_class(None, Some("supermarket"), None, None),
        Some(st::SETTLEMENT_FOOD_RETAIL)
    );
    assert_eq!(
        poi_class(None, Some("convenience"), None, None),
        Some(st::SETTLEMENT_FOOD_RETAIL)
    );
    assert_eq!(poi_class(None, Some("clothes"), None, None), Some(1));
    assert_eq!(poi_class(None, None, Some("hospital"), None), Some(4));
    assert_eq!(poi_class(None, None, None, Some("hotel")), Some(6));
    assert_eq!(poi_class(Some("bench"), None, None, None), None);
    assert_eq!(poi_class(None, None, None, None), None);
}

#[test]
fn farm_auxiliary_silent_unless_livestock() {
    let mut shed = Tags::new();
    shed.insert("building".into(), "farm_auxiliary".into());
    assert_eq!(building_type_from_tags(&shed), st::SETTLEMENT_SILENT);
    let mut barn = Tags::new();
    barn.insert("building".into(), "farm_auxiliary".into());
    barn.insert("animal".into(), "cattle".into());
    assert_eq!(building_type_from_tags(&barn), 8);
}

#[test]
fn a_retail_or_commercial_zone_is_commercial_plant_not_dwellings() {
    for landuse in ["retail", "commercial"] {
        let mut t = Tags::new();
        t.insert("landuse".into(), landuse.into());
        assert_eq!(building_type_from_tags(&t), 1, "landuse={landuse}");
        assert_eq!(
            ground_state(&t),
            GROUND_IS_A_FUNCTIONAL_AREA,
            "landuse={landuse}"
        );
    }
    // A named function inside the zone still wins.
    let mut shop = Tags::new();
    shop.insert("landuse".into(), "retail".into());
    shop.insert("shop".into(), "supermarket".into());
    assert_eq!(building_type_from_tags(&shop), st::SETTLEMENT_FOOD_RETAIL);
}

/// A garage below the ground emits its vent fans and screens nothing, and is
/// not a wrapper: the buildings above it are not inside it. A building that
/// merely HAS a basement garage still stands.
#[test]
fn what_stands_on_a_row_decides_whether_it_screens() {
    let tags = |pairs: &[(&str, &str)]| {
        let mut t = Tags::new();
        for (key, value) in pairs {
            t.insert((*key).into(), (*value).into());
        }
        t
    };
    let garage = tags(&[("building", "yes"), ("amenity", "parking")]);
    assert_eq!(
        building_type_from_tags(&garage),
        st::SETTLEMENT_PARKING_STRUCTURE
    );
    assert_eq!(
        ground_state(&garage),
        GROUND_HOLDS_A_BUILDING,
        "a garage on the surface screens"
    );
    for below in [
        &[("building", "yes"), ("location", "underground")] as &[(&str, &str)],
        &[("building", "parking"), ("parking", "underground")],
        &[("amenity", "parking"), ("parking", "underground")],
    ] {
        assert_eq!(
            ground_state(&tags(below)),
            GROUND_IS_BELOW_A_BUILDING,
            "{below:?}"
        );
    }
    // An `amenity=parking` building IS a car park (class 7 above), so
    // `parking=underground` puts THAT building below the ground. A house or a
    // block of flats with a basement garage carries no `amenity=parking`: it
    // never reaches the car park rule, and it keeps its wall.
    assert_eq!(
        ground_state(&tags(&[
            ("building", "yes"),
            ("amenity", "parking"),
            ("parking", "underground")
        ])),
        GROUND_IS_BELOW_A_BUILDING
    );
    for stands in [
        &[("building", "house"), ("parking", "underground")] as &[(&str, &str)],
        &[("building", "apartments"), ("parking", "underground")],
        // A deck with no `building` tag stands too.
        &[("amenity", "parking"), ("parking", "multi-storey")],
    ] {
        assert_eq!(
            ground_state(&tags(stands)),
            GROUND_HOLDS_A_BUILDING,
            "{stands:?}"
        );
    }
    // `building=no` says nothing stands here, and nothing overrides that.
    for denied in [
        &[("building", "no"), ("amenity", "school")] as &[(&str, &str)],
        &[
            ("building", "no"),
            ("amenity", "parking"),
            ("parking", "multi-storey"),
        ],
    ] {
        assert_eq!(
            ground_state(&tags(denied)),
            GROUND_IS_A_FUNCTIONAL_AREA,
            "{denied:?}"
        );
    }
}

#[test]
fn building_type_from_tags_poi_overrides_yes() {
    let mut t = Tags::new();
    t.insert("building".into(), "yes".into());
    t.insert("amenity".into(), "hospital".into());
    assert_eq!(building_type_from_tags(&t), 4);
}

#[test]
fn parse_height_strips_one_metre_suffix() {
    assert_eq!(parse_height("4 m"), Some(4.0));
    assert_eq!(parse_height("4m"), Some(4.0));
    assert_eq!(parse_height("4.5"), Some(4.5));
    assert_eq!(parse_height("tall"), None);
    assert_eq!(parse_height("2000mm"), None);
    assert_eq!(parse_height("6 ft"), None);
    assert_eq!(parse_height("10 m "), Some(10.0));
}

#[test]
fn spill_key_roundtrip() {
    let sq = Square { x: 276, y: 173 };
    assert_eq!(spill_key(sq), 173 * 512 + 276);
    assert_eq!(square_from_spill_key(173 * 512 + 276), Some(sq));
    assert_eq!(square_from_spill_key(512 * 512), None);
}

#[test]
fn ring_text_roundtrip() {
    let ring = vec![(100, 200), (300, 400), (500, 600)];
    let text = encode_ring_text(&ring);
    assert_eq!(parse_ring_text(&text), Some(ring));
    assert_eq!(parse_ring_text(""), None);
    assert_eq!(parse_ring_text("1,2"), None);
    assert_eq!(parse_ring_text("a,b;c,d;e,f"), None);
}

#[test]
fn tsv_text_cannot_create_columns_or_records() {
    assert_eq!(TsvText("A\tB\r\nŽluťoučký").to_string(), "A B  Žluťoučký");
}

#[test]
fn road_ref_falls_back_to_old_ref_only_when_empty() {
    let mut renumbered = Tags::new();
    renumbered.insert("highway".into(), "trunk".into());
    renumbered.insert("int_ref".into(), "E 55;E 65".into());
    renumbered.insert("old_ref".into(), "D1".into());
    assert_eq!(tsv_road_ref(&renumbered).to_string(), "D1");
    let mut current = Tags::new();
    current.insert("ref".into(), "D1".into());
    current.insert("old_ref".into(), "D0".into());
    assert_eq!(tsv_road_ref(&current).to_string(), "D1");
    let mut unmarked = Tags::new();
    unmarked.insert("highway".into(), "residential".into());
    assert_eq!(tsv_road_ref(&unmarked).to_string(), "");
    let mut empty_ref = Tags::new();
    empty_ref.insert("ref".into(), "".into());
    empty_ref.insert("old_ref".into(), "D1".into());
    assert_eq!(tsv_road_ref(&empty_ref).to_string(), "D1");
}

#[test]
fn disk_full_stops_every_spill_row_writer() {
    let dir = std::env::temp_dir().join(format!("osm-spill-full-{}", std::process::id()));
    let square = crate::osm::geometry::square_of(50.0, 14.0);
    for feature in [FeatureType::Road, FeatureType::Building, FeatureType::Poi] {
        let mut spiller = Spiller::new(&dir, 1, Default::default(), &[]).unwrap();
        // Zero capacity makes the actual ENOSPC visible during the row write.
        spiller.writers.insert(
            (feature.name().into(), 0),
            BufWriter::with_capacity(
                0,
                std::fs::OpenOptions::new()
                    .write(true)
                    .open("/dev/full")
                    .unwrap(),
            ),
        );
        let tags = Tags::new();
        let result = match feature {
            FeatureType::Road => spiller.emit_segment(
                &feature,
                square,
                1,
                0,
                &([50.0, 14.0], [50.001, 14.0], 111.0),
                &tags,
                None,
            ),
            FeatureType::Building => {
                spiller.emit_polygon(&feature, square, 2, "way", 50.0, 14.0, &tags, None)
            }
            _ => spiller.emit_poi(square, 50.0, 14.0, 1),
        };
        assert_eq!(
            result
                .unwrap_err()
                .downcast_ref::<std::io::Error>()
                .unwrap()
                .raw_os_error(),
            Some(28)
        );
    }
    fs::remove_dir_all(dir).unwrap();
}
