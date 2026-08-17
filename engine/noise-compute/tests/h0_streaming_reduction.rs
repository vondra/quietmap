//! Direct and property fixtures for the bounded H0 streaming reference.

use noise_compute::compute::element::{LineLayer, LinePiece, H0_NODE_CAP};
use noise_compute::propagation::h0_streaming_reduction::{reduce_h0, H0Candidate, H0Reduction};
use noise_compute::propagation::streaming_reduction::{
    candidate_wedge_owns, range_ordered, GeometryError, MetricVector, SourceId64, WedgeDecision,
};

fn line(start_m: [f64; 2], end_m: [f64; 2]) -> LinePiece {
    LinePiece {
        start_m,
        end_m,
        emission_per_m: 1.0,
    }
}

fn obstacle(ordinal: u64, endpoint0_m: [f64; 2], endpoint1_m: [f64; 2]) -> H0Candidate {
    H0Candidate::from_metric_segment(
        SourceId64::obstacle(ordinal).unwrap(),
        MetricVector::new(endpoint0_m[0], endpoint0_m[1]),
        MetricVector::new(endpoint1_m[0], endpoint1_m[1]),
        12.0,
    )
    .unwrap()
}

fn admitted_indices(reduction: &H0Reduction) -> Vec<usize> {
    (0..reduction.nodes().len())
        .filter(|index| reduction.node_is_admitted(*index) == Some(true))
        .collect()
}

#[test]
fn radial_zero_width_source_keeps_only_the_on_ray_wall() {
    let source = line([10.0, 0.0], [100.0, 0.0]);
    let on_ray = reduce_h0(
        source,
        LineLayer::Road,
        [obstacle(0, [5.0, -1.0], [5.0, 1.0])],
    )
    .unwrap();
    let off_ray = reduce_h0(
        source,
        LineLayer::Road,
        [obstacle(1, [5.0, 5.0], [5.0, 6.0])],
    )
    .unwrap();

    assert!(on_ray.any_node_is_admitted());
    assert!(!off_ray.any_node_is_admitted());
    assert!(on_ray.nodes().len() <= H0_NODE_CAP);
    assert!(on_ray
        .nodes()
        .iter()
        .all(|node| node.line_node.node_x_m > 0.0 && node.line_node.arm == 1));
}

#[test]
fn receiver_crossing_has_disjoint_radial_arms_and_no_receiver_node() {
    let source = line([-100.0, 0.0], [100.0, 0.0]);
    let east = reduce_h0(
        source,
        LineLayer::Road,
        [obstacle(0, [5.0, -1.0], [5.0, 1.0])],
    )
    .unwrap();
    let west = reduce_h0(
        source,
        LineLayer::Road,
        [obstacle(1, [-5.0, -1.0], [-5.0, 1.0])],
    )
    .unwrap();

    let east_indices = admitted_indices(&east);
    let west_indices = admitted_indices(&west);
    // The record-absent prep epoch is the reviewed 3-degree arm. Keep its
    // per-arm count exact while later selected theta values retain the
    // structural symmetry assertions below.
    #[cfg(not(feature = "h0-production-selection"))]
    {
        assert_eq!(east.admitted_node_count(), 11);
        assert_eq!(west.admitted_node_count(), 11);
    }
    assert!(east.admitted_node_count() > 0);
    assert_eq!(east.admitted_node_count(), west.admitted_node_count());
    assert!(east_indices
        .iter()
        .all(|index| east.nodes()[*index].line_node.arm == 1));
    assert!(west_indices
        .iter()
        .all(|index| west.nodes()[*index].line_node.arm == -1));
    assert!(east_indices
        .iter()
        .all(|index| !west_indices.contains(index)));
    assert!(east
        .nodes()
        .iter()
        .all(|node| { node.receiver_vector_m.x != 0.0 || node.receiver_vector_m.y != 0.0 }));
}

#[test]
fn half_open_boundary_belongs_to_the_wedge_it_opens() {
    let source = line([10.0, 0.0], [100.0, 0.0]);
    let closing_at_east = reduce_h0(
        source,
        LineLayer::Road,
        [obstacle(0, [0.0, -5.0], [5.0, 0.0])],
    )
    .unwrap();
    let opening_at_east = reduce_h0(
        source,
        LineLayer::Road,
        [obstacle(1, [5.0, 0.0], [0.0, 5.0])],
    )
    .unwrap();
    let clockwise_opening_at_east = reduce_h0(
        source,
        LineLayer::Road,
        [obstacle(2, [5.0, 0.0], [0.0, -5.0])],
    )
    .unwrap();

    assert!(!closing_at_east.any_node_is_admitted());
    assert!(opening_at_east.any_node_is_admitted());
    assert!(clockwise_opening_at_east.any_node_is_admitted());
}

#[test]
fn duplicate_geometry_is_mask_idempotent_and_input_order_is_irrelevant() {
    let source = line([-125.0, 20.0], [125.0, 20.0]);
    let first = obstacle(7, [8.0, -4.0], [8.0, 4.0]);
    let duplicate = first;
    let distinct_same_geometry = obstacle(8, [8.0, -4.0], [8.0, 4.0]);
    let other = obstacle(9, [-12.0, -6.0], [-12.0, 6.0]);
    let permutations = [
        [first, distinct_same_geometry, other],
        [first, other, distinct_same_geometry],
        [distinct_same_geometry, first, other],
        [distinct_same_geometry, other, first],
        [other, first, distinct_same_geometry],
        [other, distinct_same_geometry, first],
    ];
    let expected = reduce_h0(source, LineLayer::Rail, permutations[0]).unwrap();
    assert_eq!(expected.candidate_visit_count(), 3);
    for candidates in permutations.into_iter().skip(1) {
        let got = reduce_h0(source, LineLayer::Rail, candidates).unwrap();
        assert_eq!(got.admitted_mask_words(), expected.admitted_mask_words());
    }

    let with_duplicate = reduce_h0(
        source,
        LineLayer::Rail,
        [first, duplicate, distinct_same_geometry, other],
    )
    .unwrap();
    assert_eq!(
        with_duplicate.admitted_mask_words(),
        expected.admitted_mask_words()
    );
    assert_eq!(with_duplicate.candidate_visit_count(), 4);
    assert_ne!(first.source_id(), distinct_same_geometry.source_id());
}

#[test]
fn source_stratum_minimum_matches_all_1024_direct_subsets() {
    let mut near_values = [0.5_f32; 10];
    near_values[1] = 1.0;
    for index in 2..near_values.len() {
        near_values[index] = near_values[index - 1] * 1.5;
    }
    for subset in 0_u16..(1_u16 << near_values.len()) {
        for half_metres in 0..=12 {
            let node_distance = f64::from(half_metres) * 0.5;
            let direct = near_values.iter().enumerate().any(|(index, near)| {
                (subset & (1 << index)) != 0 && range_ordered(node_distance, *near).unwrap()
            });
            let minimum_legal = near_values
                .iter()
                .enumerate()
                .filter(|(index, near)| (subset & (1 << index)) != 0 && **near >= 1.0)
                .map(|(_, near)| *near)
                .min_by(f32::total_cmp);
            let reduced = minimum_legal
                .map(|near| range_ordered(node_distance, near).unwrap())
                .unwrap_or(false);
            assert_eq!(direct, reduced, "subset={subset:#05x} d={node_distance}");
        }
    }
}

#[test]
fn range_limb_rejects_a_covered_candidate_below_one_metre() {
    let candidate = obstacle(0, [0.5, -0.1], [0.5, 0.1]);
    assert!(candidate.near_f32() < 1.0);
    let reduction = reduce_h0(
        line([10.0, 0.0], [100.0, 0.0]),
        LineLayer::Road,
        [candidate],
    )
    .unwrap();
    assert!(reduction.nodes().iter().all(|node| {
        candidate_wedge_owns(
            candidate.endpoint0_m(),
            candidate.endpoint1_m(),
            node.receiver_vector_m,
            candidate.near_f32(),
        ) == WedgeDecision::Owns
    }));
    assert!(!reduction.any_node_is_admitted());
}

#[test]
fn physical_node_range_not_placement_distance_owns_the_p2b_fork() {
    let candidate = obstacle(0, [2.0, -1.0], [2.0, 1.0]);
    let reduction = reduce_h0(
        line([-100.0, 0.0], [100.0, 0.0]),
        LineLayer::Road,
        [candidate],
    )
    .unwrap();
    let fork_indices: Vec<_> = reduction
        .nodes()
        .iter()
        .enumerate()
        .filter(|(_, node)| {
            candidate_wedge_owns(
                candidate.endpoint0_m(),
                candidate.endpoint1_m(),
                node.receiver_vector_m,
                candidate.near_f32(),
            ) == WedgeDecision::Owns
                && !range_ordered(node.node_distance_m, candidate.near_f32()).unwrap()
                && range_ordered(node.line_node.placement_distance_m, candidate.near_f32()).unwrap()
        })
        .map(|(index, _)| index)
        .collect();
    assert!(!fork_indices.is_empty());
    assert!(fork_indices
        .into_iter()
        .all(|index| reduction.node_is_admitted(index) == Some(false)));
}

#[test]
fn rail_on_track_worst_case_stays_below_the_selected_theorem_cap() {
    let reduction = reduce_h0(line([-125.0, 0.0], [125.0, 0.0]), LineLayer::Rail, []).unwrap();
    assert!(reduction.nodes().len() < H0_NODE_CAP);
}

#[test]
fn legal_receiver_touch_is_counted_and_never_admitted() {
    let source = line([10.0, 0.0], [100.0, 0.0]);
    let receiver_touch = obstacle(0, [0.0, 0.0], [0.0, 4.0]);
    assert_eq!(receiver_touch.near_f32(), 0.0);
    let reduction = reduce_h0(source, LineLayer::Road, [receiver_touch]).unwrap();
    assert!(!reduction.any_node_is_admitted());
    assert_eq!(reduction.guarded_degenerate_candidate_count(), 1);
}

#[test]
fn nonfinite_candidate_and_pair_geometry_fail_closed() {
    assert_eq!(
        H0Candidate::from_metric_segment(
            SourceId64::obstacle(0).unwrap(),
            MetricVector::new(f64::NAN, 0.0),
            MetricVector::new(1.0, 0.0),
            12.0,
        ),
        Err(GeometryError::NonFinite)
    );
    assert!(reduce_h0(line([f64::INFINITY, 0.0], [1.0, 0.0]), LineLayer::Road, [],).is_err());
}
