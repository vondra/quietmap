//! The sources tile round trip: pieces as lines and points, attributes, decks, shared emission
//! rows and displays, every display field kind with its nulls.

use super::*;
use serde_json::{Value, json};

fn road_display() -> Vec<Value> {
    let Value::Array(fields) = json!([
        "Legerova",
        "",
        "secondary",
        12_345.6,
        1_234.5,
        345.6,
        0.4,
        7,
        245_000.0,
        null,
        42.3,
        "osm_posted",
        "asphalt",
        -1.5,
        4,
        true,
        false,
        9_875,
        [0.712, 0.123, 0.165]
    ]) else {
        unreachable!()
    };
    fields
}

fn road() -> Attribute {
    let mut emission = [[70.25; BANDS]; PERIODS];
    emission[2][7] = f64::NEG_INFINITY;
    Attribute {
        layer: Layer::Road,
        height_m: 0.05,
        ground_percent: 0,
        platform_half_width_m: 5.0,
        exclusion_radius_m: 0.0,
        footprint_id: 0,
        group_key: 7,
        emission,
        display: road_display(),
    }
}

fn building(footprint_id: u64, display: Value) -> Attribute {
    let Value::Array(display) = display else {
        unreachable!()
    };
    Attribute {
        layer: Layer::Building,
        height_m: 7.5,
        ground_percent: GROUND_FROM_TERRAIN,
        exclusion_radius_m: 8.9,
        footprint_id,
        group_key: footprint_id,
        display,
        ..road()
    }
}

fn without_display(attribute: &Attribute) -> Attribute {
    Attribute {
        display: Vec::new(),
        ..attribute.clone()
    }
}

#[test]
fn sources_round_trip_with_lines_before_points() {
    let bell = building(
        0x0123_4567_89ab_cdef,
        json!([
            "St Ludmila",
            "worship",
            31.2,
            0,
            0.0,
            "",
            96.4,
            null,
            [4.25, 1.0, 0.0],
            [0.00125, 0.0, null],
            "dbd97e971aed953e"
        ]),
    );
    let pieces = [
        Piece {
            ends: [[5, 5], [5, 5]],
            attribute: 1,
        },
        Piece {
            ends: [[-100, 200], [16_000, -16_384]],
            attribute: 0,
        },
        Piece {
            ends: [[16_000, -16_384], [16_383, 0]],
            attribute: 0,
        },
    ];
    let attributes = [road(), bell.clone()];
    let decks = [Deck {
        piece: 2,
        altitude_m: [312.4, -12.2],
    }];
    let bytes = encode(&pieces, &attributes, &decks);
    let parsed = Sources::parse(&bytes).unwrap();
    assert_eq!(parsed.piece_count(), 3);
    // The lines first, then the point; the deck follows its line to index 1.
    assert_eq!(parsed.piece(0).unwrap(), pieces[1]);
    assert_eq!(parsed.piece(1).unwrap(), pieces[2]);
    assert_eq!(parsed.piece(2).unwrap(), pieces[0]);
    assert!(!parsed.piece(2).unwrap().is_line());
    let deck = parsed.deck(1).unwrap();
    assert!(
        (deck[0] - 312.4).abs() < 1e-9 && (deck[1] + 12.2).abs() < 1e-9,
        "0.2 m steps"
    );
    assert_eq!((parsed.deck(0), parsed.deck(2)), (None, None));
    assert_eq!(parsed.attribute(0).unwrap(), without_display(&road()));
    assert_eq!(parsed.attribute(1).unwrap(), without_display(&bell));
    assert_eq!(parsed.group_key(0).unwrap(), 7);
    assert_eq!(parsed.display(0).unwrap(), road().display);
    assert_eq!(parsed.display(1).unwrap(), bell.display);
    assert!(Sources::parse(&bytes[..bytes.len() - 1]).is_err());
    // The deck after the header, two lines, a point, two attributes and their row: one under a
    // line beyond the lines is refused.
    let deck = HEADER_BYTES + 2 * LINE_BYTES + POINT_BYTES + 2 * ATTRIBUTE_BYTES + EMISSION_BYTES;
    let mut beyond = bytes.clone();
    beyond[deck] = 2;
    assert!(Sources::parse(&beyond).is_err());
    assert!(parsed.piece(3).is_err());
}

/// Every layer's fields come back as written, nulls and a short array included; a number keeps its
/// field's step (a mean speed of 101.39999999999999 km/h reads 101.4).
#[test]
fn displays_keep_their_fields_types_and_steps() {
    let mut speed = road();
    speed.display[10] = json!(101.399_999_999_999_99);
    let rail = Attribute {
        layer: Layer::Railway,
        display: json!([
            "",
            "S1",
            "rail",
            "main",
            0.0412,
            12.3,
            0.000123,
            1529.0,
            0.0,
            3.5,
            2,
            1,
            120.0,
            "osm_posted",
            false,
            2044,
            9863
        ])
        .as_array()
        .unwrap()
        .clone(),
        ..road()
    };
    let turbine = Attribute {
        layer: Layer::Industry,
        display: json!(["", "wind_turbine", 0.0, null, 1, 117.5, 2300.0, 105.3, 0])
            .as_array()
            .unwrap()
            .clone(),
        ..road()
    };
    let ship = Attribute {
        layer: Layer::Ship,
        display: json!(["large_ships", 697_078.0, [12.25, 3.0, 0.5], 115.7, 9901])
            .as_array()
            .unwrap()
            .clone(),
        ..road()
    };
    let ground = Attribute {
        layer: Layer::Aircraft,
        display: json!([
            "LKPR ground operations",
            "airport_traffic:LKPR",
            "Václav Havel Airport Prague",
            403.3,
            339.2,
            0.1
        ])
        .as_array()
        .unwrap()
        .clone(),
        ..road()
    };
    let house = building(5, json!(["", "house", 6.0, 2, 120.0, "Na Výsluní 7", 61.2]));
    let tower = building(6, json!(["", "tower", 828.0, 255, 3_000.0, "", 90.1]));
    let quarry = Attribute {
        layer: Layer::Industry,
        display: json!([
            "",
            "quarry",
            7_394_341.0,
            "0810",
            89_024,
            null,
            null,
            118.2,
            0
        ])
        .as_array()
        .unwrap()
        .clone(),
        ..road()
    };
    let short = Attribute {
        display: vec![json!("test road")],
        ..road()
    };
    let attributes = [
        speed, rail, turbine, ship, ground, house, short, tower, quarry,
    ];
    let pieces: Vec<Piece> = (0..attributes.len() as u32)
        .map(|attribute| Piece {
            ends: [[1, 1], [2, 2]],
            attribute,
        })
        .collect();
    let parsed_bytes = encode(&pieces, &attributes, &[]);
    let parsed = Sources::parse(&parsed_bytes).unwrap();
    let mut expected: Vec<Vec<Value>> = attributes.iter().map(|a| a.display.clone()).collect();
    expected[0][10] = json!(101.4);
    for (index, display) in expected.iter().enumerate() {
        assert_eq!(&parsed.display(index as u32).unwrap(), display, "{index}");
    }
    // Field names, as the panel shows them.
    assert_eq!(display_fields(Layer::Road)[10].name, "speed_kmh");
    assert_eq!(display_fields(Layer::Building).len(), 11);
}

/// Two rows of a road that differ in emission keep two records and one copy of their display.
#[test]
fn a_shared_display_is_stored_once() {
    let mut uphill = road();
    uphill.emission[0][0] += 1.5;
    let pieces = [0, 1].map(|attribute| Piece {
        ends: [[0, 0], [10, 10]],
        attribute,
    });
    let bytes = encode(&pieces, &[road(), uphill.clone()], &[]);
    let parsed = Sources::parse(&bytes).unwrap();
    assert_eq!(parsed.attribute_count(), 2);
    assert_eq!(parsed.attribute(1).unwrap().emission, uphill.emission);
    for index in [0, 1] {
        assert_eq!(parsed.display(index).unwrap(), road().display);
    }
    let single = encode(&pieces[..1], &[road()], &[]);
    assert_eq!(
        bytes.len(),
        single.len() + LINE_BYTES + ATTRIBUTE_BYTES + EMISSION_BYTES
    );
}

/// Buildings of one spectrum keep a record each and one copy of their emission; a text shared by
/// two displays is stored once; a platform wider than a byte's 25.5 m keeps its width.
#[test]
fn a_shared_emission_and_text_are_stored_once() {
    let house = |footprint_id, address: &str| {
        building(
            footprint_id,
            json!(["", "house", 6.0, 2, 120.0, address, 61.2]),
        )
    };
    let pieces = [0, 1, 2].map(|attribute| Piece {
        ends: [[3, 3], [3, 3]],
        attribute,
    });
    let wide = Attribute {
        platform_half_width_m: 36.5,
        ..road()
    };
    let bytes = encode(
        &pieces,
        &[house(1, "Na Výsluní 7"), house(2, "Na Výsluní 9"), wide],
        &[],
    );
    let parsed = Sources::parse(&bytes).unwrap();
    assert_eq!(parsed.attribute(1).unwrap().footprint_id, 2);
    assert_eq!(parsed.attribute(1).unwrap().emission, road().emission);
    assert_eq!(parsed.attribute(2).unwrap().platform_half_width_m, 36.5);
    let one = encode(
        &pieces[..2],
        &[house(1, "Na Výsluní 7"), house(2, "Na Výsluní 7")],
        &[],
    );
    let two = encode(
        &pieces[..2],
        &[house(1, "Na Výsluní 7"), house(2, "Na Výsluní 9")],
        &[],
    );
    // A second address costs its record (22 B) and its text (2 + 13 B), "house" and "" once.
    assert_eq!(two.len() - one.len(), 22 + 2 + "Na Výsluní 9".len());
}

#[test]
#[should_panic(expected = "display field lanes cannot hold")]
fn a_display_value_its_field_cannot_hold_is_refused() {
    let mut wide = road();
    wide.display[14] = json!(300);
    encode(
        &[Piece {
            ends: [[0, 0], [1, 1]],
            attribute: 0,
        }],
        &[wide],
        &[],
    );
}
