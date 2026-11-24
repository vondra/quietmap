//! Regression cases for evidence.

use super::*;
fn tags(pairs: &[(&str, &str)]) -> Tags {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}
#[test]
fn railway_yard_predicate_needs_the_yard_tag() {
    let yes = |pairs: &[(&str, &str)]| {
        let t = tags(pairs);
        is_railway_yard(|k| t.get(k).map(String::as_str))
    };
    // `landuse=railway` alone is the corridor (trackside land, sidings,
    // station complexes), not a yard: only `railway=yard` emits.
    assert!(!yes(&[("landuse", "railway")]));
    assert!(yes(&[("railway", "yard")]));
    assert!(yes(&[("landuse", "railway"), ("railway", "yard")]));
    assert!(!yes(&[("landuse", "industrial")]));
    assert!(!yes(&[("railway", "rail")]));
    assert!(!yes(&[("railway", "station")]));
    assert!(!yes(&[]));
}

#[test]
fn inactive_industry_requires_lifecycle_of_the_facility() {
    for unrelated in [
        "disused:railway",
        "abandoned:railway",
        "construction:railway",
        "proposed:building",
        "construction:voltage",
        "construction:power:voltage",
    ] {
        let site = tags(&[("landuse", "industrial"), (unrelated, "yes")]);
        assert_eq!(industrial_class(&site), None, "{unrelated}");
        assert!(keep_model_tag(&FeatureType::Industrial, unrelated));
    }
    assert_eq!(
        industrial_class(&tags(&[
            ("power", "transformer"),
            ("construction:voltage", "110000")
        ])),
        Some(15)
    );
    for (key, value) in [
        ("disused:power", "transformer"),
        ("abandoned:landuse", "industrial"),
        ("construction:man_made", "works"),
        ("disused:quarry", "yes"),
    ] {
        assert_eq!(industrial_class(&tags(&[(key, value)])), Some(12), "{key}");
    }
    assert_eq!(
        industrial_class(&tags(&[("landuse", "quarry"), ("historic", "yes")])),
        Some(12)
    );
    assert_eq!(
        industrial_class(&tags(&[("power", "plant"), ("historic", "yes")])),
        None
    );
    for value in ["no", "false", "0", ""] {
        assert_eq!(
            industrial_class(&tags(&[
                ("landuse", "industrial"),
                ("disused:landuse", value)
            ])),
            None
        );
    }
}

#[test]
fn only_a_sole_wind_plant_source_makes_a_silent_outline() {
    for pairs in [
        vec![("power", "plant"), ("plant:source", "wind;gas")],
        vec![("power", "plant"), ("plant:source", "gas;wind")],
        vec![
            ("power", "plant"),
            ("plant:source", "gas"),
            ("generator:source", "wind"),
        ],
        vec![("power", "plant"), ("generator:source", "wind")],
        vec![
            ("power", "plant"),
            ("plant:source", "gas"),
            ("generator:source", "solar"),
        ],
    ] {
        let site = tags(&pairs);
        assert_eq!(industrial_class(&site), None, "{pairs:?}");
        assert!(!is_turbine(|key| site.get(key).map(String::as_str)));
    }
    for source in ["wind", " wind ", "wind;wind"] {
        assert_eq!(
            industrial_class(&tags(&[("power", "plant"), ("plant:source", source)])),
            Some(11)
        );
    }
    assert_eq!(
        industrial_class(&tags(&[
            ("power", "plant"),
            ("plant:source", "solar"),
            ("generator:source", "wind")
        ])),
        Some(13)
    );
    let turbine = tags(&[("power", "generator"), ("generator:source", "wind")]);
    assert!(is_turbine(|key| turbine.get(key).map(String::as_str)));
}

#[test]
fn power_classes_and_lifecycle_are_distinct() {
    for (pairs, expected) in [
        (vec![("power", "plant"), ("plant:source", "wind")], 11),
        (vec![("landuse", "quarry"), ("disused", "yes")], 12),
        (vec![("abandoned:landuse", "quarry")], 12),
        (vec![("power", "plant"), ("plant:source", "solar")], 13),
        (vec![("power", "substation")], 14),
        (vec![("power", "transformer"), ("rating", "25 MVA")], 15),
    ] {
        assert_eq!(industrial_class(&tags(&pairs)), Some(expected));
    }
    assert_eq!(
        industrial_class(&tags(&[("landuse", "quarry"), ("disused", "no")])),
        None
    );
}
#[test]
fn special_sports_keep_indoor_and_motor_variants() {
    // The classifier accepts every token the engine subtypes (including
    // radio-controlled, which the engine silences, and trial, which OSM
    // does not split from bicycle trials) with the engine's normalization
    // (case, `-`/space spellings); anything else is area-law leisure.
    for sport in [
        "motor",
        "karting",
        "motocross",
        "motor_sports",
        "speedway",
        "autocross",
        "rallycross",
        "car_racing",
        "formula_one",
        "road_racing",
        "drifting",
        "stockcar",
        "supermoto",
        "enduro",
        "kart",
        "go_kart",
        "trial",
        "motorsport",
        "motorcycle",
        "auto_racing",
        "stock_car_racing",
        "drag_racing",
        "rc_car",
        "rc_racing",
        "radiocontrol",
        "radio_control",
        "Kart",
        "go-kart",
        "RC-Car",
        "karting;chess",
    ] {
        assert_eq!(
            special_leisure_class(|key| (key == "sport").then_some(sport)),
            Some(10),
            "{sport}"
        );
    }
    for sport in ["", "chess", "soccer", "athletics", "cycling"] {
        assert_eq!(
            special_leisure_class(|key| (key == "sport").then_some(sport)),
            None,
            "{sport}"
        );
    }
    assert_eq!(
        special_leisure_class(|key| (key == "sport").then_some("shooting")),
        Some(11)
    );
    assert!(keep_model_tag(&FeatureType::Leisure, "shooting"));
    assert!(keep_model_tag(&FeatureType::Leisure, "shooting:range"));
    assert!(keep_model_tag(&FeatureType::Leisure, "indoor"));
    assert!(keep_model_tag(&FeatureType::Leisure, "building"));
}
#[test]
fn crossing_and_whistle_tags_keep_national_values() {
    for pairs in [
        vec![
            ("highway", "traffic_signals"),
            ("traffic_signals:direction", "forward"),
        ],
        vec![("railway", "level_crossing"), ("crossing:barrier", "half")],
        vec![("railway:signal:whistle", "DE-ESO:bu4")],
        vec![("railway:signal:whistle", "PL-PKP:w6a")],
        vec![("railway:signal:whistle", "PL-PKP:w6b")],
    ] {
        assert_eq!(
            transport_point_tags(pairs.iter().copied()),
            Some(tags(&pairs))
        );
    }
}

#[test]
fn military_range_admission_separates_active_ranges_from_quiet_and_non_firing_land() {
    for admitted in [
        vec![("landuse", "military"), ("military", "range")],
        vec![("landuse", "military"), ("military", "training_area")],
        vec![("landuse", "military"), ("military", "danger_area")],
        vec![
            ("military", "range"),
            ("building", "no"),
            ("indoor", "no"),
            ("disused", "no"),
        ],
        vec![
            ("landuse", "military"),
            ("military", "danger_area"),
            ("boundary", "hazard"),
            ("hazard", "shooting_range"),
        ],
        vec![
            ("landuse", "military"),
            ("military", "danger_area"),
            ("boundary", "civil"),
        ],
    ] {
        assert!(is_active_military_range(&admitted), "{admitted:?}");
        assert_eq!(
            industrial_class(&tags(&admitted)),
            Some(physics::emission::industrial::SOURCE_MILITARY_RANGE),
            "{admitted:?}"
        );
    }

    for rejected in [
        vec![("landuse", "military")],
        vec![("landuse", "military"), ("military", "barracks")],
        vec![("landuse", "military"), ("military", "base")],
        vec![("landuse", "military"), ("military", "office")],
        vec![("landuse", "military"), ("military", "bunker")],
        vec![
            ("landuse", "military"),
            ("military", "range"),
            ("disused", "yes"),
        ],
        vec![("was:military", "training_area")],
        vec![
            ("landuse", "military"),
            ("military", "range"),
            ("end_date", "2015"),
        ],
        vec![
            ("landuse", "military"),
            ("military", "range"),
            ("building", "yes"),
        ],
        vec![
            ("landuse", "military"),
            ("military", "range"),
            ("indoor", "yes"),
        ],
        vec![
            ("landuse", "military"),
            ("military", "range"),
            ("area", "no"),
        ],
        vec![
            ("landuse", "military"),
            ("military", "range"),
            ("seamark:type", "restricted_area"),
        ],
        vec![
            ("landuse", "military"),
            ("military", "danger_area"),
            ("hazard", "unexploded_ordnance"),
        ],
        vec![
            ("landuse", "military"),
            ("military", "danger_area"),
            ("hazard", "minefield"),
        ],
        vec![
            ("landuse", "military"),
            ("military", "danger_area"),
            ("hazard", "nuclear"),
        ],
        vec![("military", "danger_area"), ("boundary", "civil")],
        vec![
            ("landuse", "military"),
            ("military", "danger_area"),
            ("boundary", "military"),
        ],
        vec![
            ("landuse", "military"),
            ("military", "danger_area"),
            ("boundary", "protected_area"),
        ],
        vec![("landuse", "industrial"), ("military", "danger_area")],
        vec![("landuse", "quarry"), ("military", "danger_area")],
        vec![("railway", "yard"), ("military", "training_area")],
        vec![("military", "range"), ("shooting:range", "indoor")],
        vec![("military", "range"), ("construction", "yes")],
        vec![("military", "range"), ("proposed", "yes")],
    ] {
        assert!(!is_active_military_range(&rejected), "{rejected:?}");
        assert_ne!(
            industrial_class(&tags(&rejected)),
            Some(physics::emission::industrial::SOURCE_MILITARY_RANGE),
            "{rejected:?}"
        );
    }

    assert!(has_military_shooting_veto(&[
        ("military", "range"),
        ("sport", "shooting")
    ]));
    assert!(has_military_shooting_veto(&[
        ("was:military", "range"),
        ("sport", "shooting")
    ]));
    assert!(has_military_shooting_veto(&[
        ("landuse", "military"),
        ("disused", "yes"),
        ("sport", "shooting")
    ]));
    assert!(has_military_shooting_veto(&[
        ("abandoned:landuse", "military"),
        ("sport", "shooting")
    ]));
    assert!(!has_military_shooting_veto(&[("sport", "shooting")]));
}
