//! Facility evidence: tag values, substation classes and the joins across facility parts.

use super::*;

fn tags(pairs: &[(&str, &str)]) -> Tags {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

#[test]
fn power_values_parse_units_lists_and_bare_numbers() {
    let output = |value: &str| plant_output_mw(&tags(&[("plant:output:electricity", value)]));
    assert_eq!(
        (output("4 MW"), output("2.112MW"), output("900 kW")),
        (Some(4.0), Some(2.112), Some(0.9))
    );
    assert_eq!(
        (output("1.2 GW"), output("5000000 W"), output("2,5 MW")),
        (Some(1200.0), Some(5.0), Some(2.5))
    );
    assert_eq!(
        (output("4"), output("4 MW;4000 kW"), output("2 MW;900 kW")),
        (Some(4.0), Some(4.0), Some(2.0))
    );
    for unknown in ["", "yes", "unknown", "4 TW", "4MMW", "-3 MW", "nan MW"] {
        assert_eq!(output(unknown), None, "{unknown:?}");
    }
    let rating = |value: &str| transformer_rating_mva(&tags(&[("rating", value)]));
    assert_eq!(
        (rating("450 MVA"), rating("50 kVA"), rating("630kVA")),
        (Some(450.0), Some(0.05), Some(0.63))
    );
    assert_eq!(
        (rating("25/33 MVA"), rating("16 MVA; 25 MVA")),
        (Some(33.0), Some(25.0))
    );
    assert_eq!(
        (rating("2x630kVA"), rating("2 X 1 MVA")),
        (Some(1.26), Some(2.0))
    );
    assert_eq!((rating("630"), rating("yes")), (None, None));
    let voltage = |value: &str| maximum_voltage_kv(&tags(&[("voltage", value)]));
    assert_eq!(
        (
            voltage("400000"),
            voltage("400000;110000"),
            voltage("400 kV")
        ),
        (Some(400.0), Some(400.0), Some(400.0))
    );
    assert_eq!((voltage("22kV"), voltage("high")), (Some(22.0), None));
}

#[test]
fn shares_gas_stations_plants_and_autotransformers_read_their_tags() {
    assert_eq!(facility_share(&tags(&[])), 1.0);
    assert_eq!(facility_share(&tags(&[("qm:facility_share", "0.5")])), 0.5);
    for garbage in ["0", "-0.5", "1.5", "abc", "", "NaN", "inf"] {
        assert_eq!(
            facility_share(&tags(&[("qm:facility_share", garbage)])),
            1.0,
            "{garbage:?}"
        );
    }
    assert!(is_gas_substation(&tags(&[("substation", "gas")])));
    assert!(is_gas_substation(&tags(&[("substation", "compression")])));
    assert!(!is_gas_substation(&tags(&[("substation", "transmission")])));
    assert!(is_solar_plant(&parse_tags(
        r#"{"power":"plant","plant:source":"solar"}"#
    )));
    assert!(!is_solar_plant(&parse_tags(r#"{"power":"generator"}"#)));
    assert!(parse_tags("not json").is_empty());
    assert!(is_autotransformer(&tags(&[(
        "transformer",
        "Auto-transformer"
    )])));
    assert!(!is_autotransformer(&tags(&[(
        "transformer",
        "distribution"
    )])));
}

#[test]
fn substation_power_prefers_the_join_then_its_own_rating_then_the_class() {
    let feed = |sum: f64, count: usize| SubstationFeed {
        rated_mva_sum: sum,
        rated_count: count,
        ..Default::default()
    };
    let own = tags(&[("voltage", "400000"), ("rating", "126 MVA")]);
    assert_eq!(substation_power(&own, &feed(65.0, 2)), (Some(65.0), 1));
    assert_eq!(
        substation_power(&tags(&[("rating", "450 MVA")]), &feed(0.0, 0)),
        (Some(450.0), 3)
    );
    assert_eq!(
        substation_power(&tags(&[("voltage", "220000")]), &feed(0.0, 0)),
        (None, 1)
    );
    let auto = SubstationFeed {
        has_autotransformer: true,
        ..Default::default()
    };
    assert_eq!(
        substation_power(&tags(&[("voltage", "400000")]), &auto),
        (None, 2)
    );
    assert_eq!(substation_power(&tags(&[]), &feed(0.0, 0)), (None, 3));
    // 45,534 of 86,804 canary substations are minor_distribution kiosks; a 115 kV one is not.
    let class = |pairs: &[(&str, &str)]| substation_power(&tags(pairs), &feed(0.0, 0)).1;
    assert_eq!(
        class(&[
            ("substation", "minor_distribution"),
            ("voltage", "20000;400")
        ]),
        4
    );
    assert_eq!(
        class(&[("substation", "minor_distribution"), ("voltage", "115000")]),
        3
    );
    assert_eq!(
        class(&[("substation", "transmission"), ("voltage", "110000")]),
        1
    );
    assert_eq!(class(&[("substation", "yes;minor_distribution")]), 4);
    assert_eq!(
        class(&[("substation", "transmission"), ("transformer", "auto")]),
        2
    );
}

fn square(x0: i32, y0: i32, side: i32) -> Vec<(i32, i32)> {
    vec![
        (x0, y0),
        (x0 + side, y0),
        (x0 + side, y0 + side),
        (x0, y0 + side),
    ]
}

/// Two parts of relation 7 share the 100 MVA unit standing in their overlap, counted once; an
/// unrelated way reads its own ring, an unindexed row its own polygon.
#[test]
fn a_facility_feed_counts_each_unit_once_across_its_parts() {
    let mut joins = FacilityJoins::default();
    let rating = tags(&[("rating", "100 MVA")]);
    joins.add_row(14, ("relation", 7), (50, 50), &square(0, 0, 100), &rating);
    joins.add_row(
        14,
        ("relation", 7),
        (100, 100),
        &square(50, 50, 100),
        &rating,
    );
    joins.add_row(
        14,
        ("way", 9),
        (5050, 5050),
        &square(5000, 5000, 100),
        &tags(&[]),
    );
    joins.add_row(15, ("node", 11), (70, 60), &[], &rating);
    let part_feed = joins.substation_feed(("relation", 7), &[]);
    assert_eq!((part_feed.rated_mva_sum, part_feed.rated_count), (100.0, 1));
    assert_eq!(substation_power(&rating, &part_feed).0, Some(100.0));
    assert_eq!(
        joins
            .substation_feed(("way", 9), &square(5000, 5000, 100))
            .rated_count,
        0
    );
    assert_eq!(
        joins
            .substation_feed(("way", 404), &square(0, 0, 100))
            .rated_count,
        1
    );
    assert_eq!(joins.substation_feed(("node", 405), &[]).rated_count, 0);
}

#[test]
fn generators_inside_a_solar_plant_defer_to_it() {
    let mut joins = FacilityJoins::default();
    let plant = parse_tags(r#"{"power":"plant","plant:source":"solar"}"#);
    let generator = parse_tags(r#"{"power":"generator","generator:source":"solar"}"#);
    joins.add_row(13, ("way", 1), (500, 500), &square(0, 0, 1000), &plant);
    joins.add_row(
        13,
        ("way", 2),
        (8500, 8500),
        &square(8000, 8000, 1000),
        &generator,
    );
    joins.add_row(14, ("way", 3), (500, 500), &square(0, 0, 1000), &tags(&[]));
    assert!(joins.inside_solar_plant((500, 500)));
    assert!(!joins.inside_solar_plant((5000, 5000)));
    assert!(!joins.inside_solar_plant((8500, 8500)), "only plants index");
}
