//! A shared survey endpoint with conflicting counts cannot choose traffic by traversal order or rounding.

use super::{
    matching::Matcher,
    matching_distance::segment_distance,
    tests::{model, observation, road},
};

#[test]
fn shared_survey_endpoints_have_one_exact_distance_and_conflicting_counts_stay_unmatched() {
    let point = [-98.88179568487334, 19.524300701769647];
    let a = [-98.88131, 19.522781];
    let joint = [-98.881157, 19.522884];
    let b = [-98.880951, 19.522928];
    assert_eq!(
        segment_distance(point, a, joint),
        segment_distance(point, joint, b)
    );
    assert_eq!(
        segment_distance(point, a, joint),
        segment_distance(point, joint, a)
    );
    let mut first = observation();
    first.identity = "first".into();
    first.reference_required = false;
    first.road_class_mask = (1 << 13) - 1;
    first.search_radius_m = 200.0;
    first.geometry = vec![vec![[-0.01, 0.0], [0.0, 0.0]]];
    let mut second = first.clone();
    second.identity = "second".into();
    second.geometry = vec![vec![[0.0, 0.0], [0.01, 0.0]]];
    second.vehicle_classes[0] = Some(first.vehicle_classes[0].unwrap() + 100.0);
    second.total = second.total.map(|n| n + 100.0);
    for observations in [
        vec![first.clone(), second.clone()],
        vec![second, first.clone()],
    ] {
        let matcher = Matcher::new(observations).unwrap();
        let mut piece = road(1);
        piece.source_id = 0;
        piece.counts = [0.0; 4];
        piece.country.0 = first.country;
        piece.start = (0.0, 10.0);
        piece.end = (0.0, 20.0);
        matcher.apply(&mut piece, &model()).unwrap();
        assert_eq!(piece.source_id, 0);
    }
}

#[test]
fn equally_near_survey_endpoints_use_the_support_of_the_whole_mapped_piece() {
    let mut first = observation();
    first.identity = "west".into();
    first.reference_required = false;
    first.geometry = vec![vec![[-0.01, 0.0], [0.0, 0.0]]];
    first.search_radius_m = 100.0;
    let mut second = first.clone();
    second.identity = "north".into();
    second.geometry = vec![vec![[0.0, 0.0], [0.0, 0.01]]];
    second.vehicle_classes = [160.0, 20.0, 10.0, 10.0].map(Some);
    second.total = Some(200.0);
    for observations in [vec![first.clone(), second.clone()], vec![second, first]] {
        let mut piece = road(1);
        piece.start = (-20.0, -10.0);
        piece.end = (40.0, -10.0);
        let matcher = Matcher::new(observations).unwrap();
        matcher.apply(&mut piece, &model()).unwrap();
        assert_eq!(piece.observation, "west");
    }
}
