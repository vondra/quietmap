//! Regression cases for a station's true geometric support and a route count's bounded identity.

use super::{
    matching::Matcher,
    observation_target::{
        DistanceRule, Identity, IdentityField, ReferenceFallback, ReferenceRewrite, ReferenceRule,
    },
    tests::{model, observation, road},
};

#[test]
fn a_counter_near_the_end_of_a_long_piece_uses_its_declared_point_support() {
    let mut observed = observation();
    observed.geometry = vec![vec![[0.00085, 0.00001]]];
    observed.search_radius_m = 5.0;
    observed.target.distance_rule = DistanceRule::PointToPiece;
    let mut target = road(1);
    target.source_id = 0;
    Matcher::new(vec![observed.clone()])
        .unwrap()
        .apply(&mut target, &model())
        .unwrap();
    assert_eq!(target.source_id, 1);
    observed.target.distance_rule = DistanceRule::Midpoint;
    target.source_id = 0;
    Matcher::new(vec![observed])
        .unwrap()
        .apply(&mut target, &model())
        .unwrap();
    assert_eq!(target.source_id, 0);
}

#[test]
fn proximity_cannot_override_a_known_route_whose_counter_is_farther_away() {
    let mut near = observation();
    near.reference = "B2".into();
    near.road_class_mask = 1 << 1;
    near.geometry = vec![vec![[0.0004, 0.0]]];
    near.target.reference_rule = ReferenceRule::Whole;
    near.target.reference_fallback = Some(ReferenceFallback {
        road_class_mask: 2,
        search_radius_m: 100.0,
        reference_sources: vec![1],
    });
    let mut far = near.clone();
    far.identity = "far".into();
    far.reference = "A1".into();
    far.geometry = vec![vec![[1.0, 0.0]]];
    let matcher = Matcher::new(vec![near, far]).unwrap();
    let mut target = road(1);
    target.class = 1;
    target.source_id = 0;
    matcher.apply(&mut target, &model()).unwrap();
    assert_eq!(target.source_id, 0);
    target.reference = "unrecorded".into();
    matcher.apply(&mut target, &model()).unwrap();
    assert_eq!(target.source_id, 1);
}

#[test]
fn a_distant_parallel_part_cannot_rescue_a_section_whose_nearest_part_crosses_the_road() {
    let mut observed = observation();
    observed.geometry = vec![
        vec![[0.0004, -0.001], [0.0004, 0.001]],
        vec![[0.0, 0.0005], [0.001, 0.0005]],
    ];
    observed.target.minimum_alignment_cosine = 0.8660254037844387;
    observed.target.distance_rule = DistanceRule::NearestPiece;
    let mut target = road(1);
    target.source_id = 0;
    Matcher::new(vec![observed.clone()])
        .unwrap()
        .apply(&mut target, &model())
        .unwrap();
    assert_eq!(target.source_id, 0);
    observed.target.distance_rule = DistanceRule::Midpoint;
    Matcher::new(vec![observed])
        .unwrap()
        .apply(&mut target, &model())
        .unwrap();
    assert_eq!(target.source_id, 1);
}

#[test]
fn equally_near_parts_at_one_endpoint_keep_an_aligned_count_in_either_geometry_order() {
    let mut observed = observation();
    observed.geometry = vec![
        vec![[0.0, -0.001], [0.0, 0.0]],
        vec![[0.0, 0.0], [0.001, 0.0]],
    ];
    observed.target.minimum_alignment_cosine = 0.8660254037844387;
    observed.target.distance_rule = DistanceRule::NearestPiece;
    for reverse in [false, true] {
        if reverse {
            observed.geometry.reverse();
        }
        let mut target = road(1);
        target.source_id = 0;
        target.start = (-50.0, 10.0);
        target.end = (50.0, 10.0);
        Matcher::new(vec![observed.clone()])
            .unwrap()
            .apply(&mut target, &model())
            .unwrap();
        assert_eq!(target.source_id, 1);
    }
}

#[test]
fn route_aliases_have_country_class_and_municipal_support_without_invented_positions() {
    let mut observed = observation();
    observed.country = 1;
    observed.geometry.clear();
    observed.reference_required = false;
    observed.road_class_mask = (1 << 2) | (1 << 3);
    observed.target.identities = vec![
        Identity {
            field: IdentityField::Reference,
            pattern: "^A1$".into(),
            road_class_mask: 1 << 2,
            priority: 1,
            prefer_first_match: false,
        },
        Identity {
            field: IdentityField::Name,
            pattern: "^main street$".into(),
            road_class_mask: 1 << 3,
            priority: 1,
            prefer_first_match: false,
        },
    ];
    observed.target.coverage = vec![vec![vec![
        [-1.0, -1.0],
        [1.0, -1.0],
        [1.0, 1.0],
        [-1.0, 1.0],
        [-1.0, -1.0],
    ]]];
    let matcher = Matcher::new(vec![observed]).unwrap();
    let apply = |class, country, longitude| {
        let mut target = road(1);
        target.source_id = 0;
        target.country.0 = country;
        target.class = class;
        target.start.0 += longitude * 111_320.0;
        target.end.0 += longitude * 111_320.0;
        matcher.apply(&mut target, &model()).unwrap();
        target.source_id
    };
    assert_eq!(apply(2, 1, 0.0), 1);
    assert_eq!(apply(3, 1, 0.0), 1);
    assert_eq!(apply(2, 2, 0.0), 0);
    assert_eq!(apply(4, 1, 0.0), 0);
    assert_eq!(apply(2, 1, 2.0), 0);
    assert_ne!(
        super::canonical_road_name("ガード"),
        super::canonical_road_name("カード")
    );
}

#[test]
fn the_first_valid_reference_is_not_replaced_by_a_nearer_or_known_later_route() {
    let mut first = observation();
    first.reference = "A1".into();
    first.target.reference_rule = ReferenceRule::FirstValidToken;
    first.target.reference_rewrites = vec![ReferenceRewrite {
        pattern: "^A[0-9]+$".into(),
        replacement: "$0".into(),
    }];
    first.geometry = vec![vec![[0.0, 0.0001], [0.001, 0.0001]]];
    first.vehicle_classes = [Some(1.0); 4];
    first.total = Some(4.0);
    let mut second = first.clone();
    second.identity = "second".into();
    second.reference = "A2".into();
    second.geometry = vec![vec![[0.0, 0.0], [0.001, 0.0]]];
    second.vehicle_classes = [Some(2.0); 4];
    second.total = Some(8.0);
    let mut target = road(1);
    target.reference = "E9; A1; A2".into();
    target.source_id = 0;
    let matcher = Matcher::new(vec![first, second]).unwrap();
    matcher.apply(&mut target, &model()).unwrap();
    assert_eq!(target.counts, [1.0; 4]);
    target.reference = "A9;A1".into();
    target.source_id = 0;
    matcher.apply(&mut target, &model()).unwrap();
    assert_eq!(target.source_id, 0);
}

#[test]
fn an_unlocated_route_uses_the_first_known_number_before_any_name_alias() {
    let mut first = observation();
    first.country = 1;
    first.geometry.clear();
    first.reference_required = false;
    first.target.identities = vec![Identity {
        field: IdentityField::Reference,
        pattern: r"(?:^|[;,/])[^0-9]*6(?:[^0-9]|$)".into(),
        road_class_mask: 4,
        priority: 1,
        prefer_first_match: true,
    }];
    first.vehicle_classes = [Some(1.0); 4];
    first.total = Some(4.0);
    let mut second = first.clone();
    second.identity = "second".into();
    second.target.identities[0].pattern = r"(?:^|[;,/])[^0-9]*14(?:[^0-9]|$)".into();
    second.vehicle_classes = [Some(2.0); 4];
    second.total = Some(8.0);
    let matcher = Matcher::new(vec![second, first]).unwrap();
    let mut target = road(1);
    target.country.0 = 1;
    target.reference = "6;14".into();
    matcher.apply(&mut target, &model()).unwrap();
    assert_eq!(target.counts, [1.0; 4]);
    target.reference = "999;14;6".into();
    matcher.apply(&mut target, &model()).unwrap();
    assert_eq!(target.counts, [2.0; 4]);
}

#[test]
fn a_published_way_identity_cannot_override_its_country_or_road_class() {
    let mut observed = observation();
    observed.country = 1;
    observed.target.source_way = Some(123);
    let matcher = Matcher::new(vec![observed]).unwrap();
    for (country, class, expected) in [(1, 2, 1), (2, 2, 0), (1, 3, 0)] {
        let mut target = road(123);
        target.source_id = 0;
        target.country.0 = country;
        target.class = class;
        matcher.apply(&mut target, &model()).unwrap();
        assert_eq!(target.source_id, expected);
    }
}

#[test]
fn a_country_and_city_without_a_declared_count_cannot_invent_an_extra_traffic_multiplier() {
    let mut model = model();
    model.regions.clear();
    let mut first = road(1);
    first.country = (u16::from_le_bytes(*b"TH"), 22);
    let mut other = first.clone();
    other.country = (u16::from_le_bytes(*b"IN"), 0);
    let counts = |r: &super::input::Road| match model.default_traffic(r) {
        super::model::TrafficDefault::SectionBothDirections(v) => (0, v),
        super::model::TrafficDefault::Carriageway(v) => (1, v),
    };
    assert_eq!(counts(&first), counts(&other));
    let matcher = Matcher::new(Vec::new()).unwrap();
    first.source_id = 0;
    matcher.apply(&mut first, &model).unwrap();
    assert_eq!(first.source_id, 0);
}

#[test]
fn a_nearby_competing_network_prevents_a_parallel_roads_count_from_leaking() {
    let mut observed = observation();
    observed.target.excluded_segments = vec![[[-0.01, 0.0], [0.01, 0.0]]];
    observed.target.exclusion_radius_m = 50.0;
    let matcher = Matcher::new(vec![observed]).unwrap();
    let mut target = road(1);
    target.source_id = 0;
    matcher.apply(&mut target, &model()).unwrap();
    assert_eq!(target.source_id, 0);
    target.start.1 = 60.0;
    target.end.1 = 60.0;
    matcher.apply(&mut target, &model()).unwrap();
    assert_eq!(target.source_id, 1);
}
