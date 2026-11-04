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
