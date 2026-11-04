//! An unnamed minor-road counter belongs to one original way and remains unknown at an ambiguous crossing.

use super::{
    continuity_tests::{CENTER, Piece, count, square},
    input,
    matching::Matcher,
    observation_target::{DistanceRule, WayAssignment},
    tests::model,
};

#[test]
fn a_point_counts_its_whole_way_but_no_way_when_another_class_is_too_close() {
    let directory = tempfile::tempdir().unwrap();
    let piece = Piece {
        way: 1,
        nodes: [1, 2],
        eastings: [CENTER, CENTER + 10_000],
        class: 2,
    };
    let mut counter = count(&piece, "minor");
    counter.reference_required = false;
    counter.search_radius_m = 20.0;
    counter.target.distance_rule = DistanceRule::GreatCircle;
    counter.geometry[0][0][0] += 0.0005;
    counter.target.way_assignment = Some(WayAssignment {
        maximum_distance_m: 12.0,
        clearance_m: 20.0,
        require_class_clearance: true,
        extend_along_way: true,
    });
    let assigned = |pieces: &[Piece]| {
        square(directory.path(), 256, pieces);
        let mut matcher = Matcher::new(vec![counter.clone()]).unwrap();
        matcher.assign_counter_ways(directory.path()).unwrap();
        let batch = input::load(&directory.path().join("z9/256/255/roads.arrow")).unwrap();
        let mut road = input::roads(&batch[0]).unwrap().remove(0);
        road.start.0 += 1000.0;
        road.end.0 += 1000.0;
        matcher.apply(&mut road, &model()).unwrap();
        road
    };
    let counted = assigned(std::slice::from_ref(&piece));
    assert_eq!(counted.source_id, 20);
    assert_eq!(counted.counts, [80.0, 10.0, 5.0, 5.0]);
    let other = Piece {
        way: 2,
        nodes: [3, 4],
        class: 3,
        ..piece.clone()
    };
    assert_eq!(assigned(&[piece, other]).source_id, 0);
}

#[test]
fn a_named_point_counter_cannot_count_two_nearby_ways_of_the_same_class() {
    let directory = tempfile::tempdir().unwrap();
    let first = Piece {
        way: 1,
        nodes: [1, 2],
        eastings: [CENTER, CENTER + 10_000],
        class: 2,
    };
    let other = Piece {
        way: 2,
        nodes: [3, 4],
        ..first.clone()
    };
    let mut counter = count(&first, "station");
    counter.search_radius_m = 20.0;
    counter.target.distance_rule = DistanceRule::PointToPiece;
    counter.target.way_assignment = Some(WayAssignment {
        maximum_distance_m: 12.0,
        clearance_m: 20.0,
        require_class_clearance: false,
        extend_along_way: true,
    });
    square(directory.path(), 256, &[other, first]);
    let mut matcher = Matcher::new(vec![counter]).unwrap();
    matcher.assign_counter_ways(directory.path()).unwrap();
    let batch = input::load(&directory.path().join("z9/256/255/roads.arrow")).unwrap();
    let mut roads = input::roads(&batch[0]).unwrap();
    for road in &mut roads {
        matcher.apply(road, &model()).unwrap();
    }
    assert_eq!(
        roads
            .iter()
            .map(|r| (r.way_id, r.source_id))
            .collect::<Vec<_>>(),
        [(1, 20), (2, 0)]
    );
}

#[test]
fn a_known_link_counter_is_not_rejected_by_its_nearby_mainline() {
    let directory = tempfile::tempdir().unwrap();
    let ramp = Piece {
        way: 1,
        nodes: [1, 2],
        eastings: [CENTER, CENTER + 10000],
        class: 10,
    };
    let mainline = Piece {
        way: 2,
        nodes: [3, 4],
        class: 0,
        ..ramp.clone()
    };
    let mut counter = count(&ramp, "slip");
    counter.reference_required = false;
    counter.road_class_mask = 1 << 10;
    counter.search_radius_m = 20.0;
    counter.target.way_assignment = Some(WayAssignment {
        maximum_distance_m: 12.0,
        clearance_m: 20.0,
        require_class_clearance: false,
        extend_along_way: true,
    });
    square(directory.path(), 256, &[ramp, mainline]);
    let mut matcher = Matcher::new(vec![counter]).unwrap();
    matcher.assign_counter_ways(directory.path()).unwrap();
    let batch = input::load(&directory.path().join("z9/256/255/roads.arrow")).unwrap();
    let mut roads = input::roads(&batch[0]).unwrap();
    for road in &mut roads {
        matcher.apply(road, &model()).unwrap();
    }
    assert_eq!(
        roads
            .iter()
            .map(|r| (r.way_id, r.source_id))
            .collect::<Vec<_>>(),
        [(1, 20), (2, 0)]
    );
}

#[test]
fn two_counters_on_one_way_follow_the_counted_piece_instead_of_the_nearest_midpoint() {
    let directory = tempfile::tempdir().unwrap();
    let piece = Piece {
        way: 1,
        nodes: [1, 2],
        eastings: [CENTER, CENTER + 10000],
        class: 2,
    };
    square(directory.path(), 256, std::slice::from_ref(&piece));
    let batch = input::load(&directory.path().join("z9/256/255/roads.arrow")).unwrap();
    let road = input::roads(&batch[0]).unwrap().remove(0);
    let points = super::matching_distance::road_points(&road);
    let mut on_piece = count(&piece, "on-piece");
    on_piece.geometry = vec![vec![points[0]]];
    on_piece.target.source_way = Some(1);
    on_piece.target.way_assignment = Some(WayAssignment {
        maximum_distance_m: 12.0,
        clearance_m: 20.0,
        require_class_clearance: false,
        extend_along_way: true,
    });
    let mut beside_midpoint = on_piece.clone();
    beside_midpoint.identity = "other-piece".into();
    beside_midpoint.geometry = vec![vec![[points[1][0], points[1][1] + 0.0001]]];
    beside_midpoint.total = Some(200.0);
    beside_midpoint.vehicle_classes = [160.0, 20.0, 10.0, 10.0].map(Some);
    for rows in [
        vec![on_piece.clone(), beside_midpoint.clone()],
        vec![beside_midpoint, on_piece],
    ] {
        let mut target = road.clone();
        let mut matcher = Matcher::new(rows).unwrap();
        matcher.assign_counter_ways(directory.path()).unwrap();
        matcher.apply(&mut target, &model()).unwrap();
        assert_eq!(target.counts, [80.0, 10.0, 5.0, 5.0]);
    }
}
