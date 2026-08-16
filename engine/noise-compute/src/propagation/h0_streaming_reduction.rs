//! Bounded CPU reference for the model-v2 H0 streaming screening reducer.

use crate::compute::element::{
    LineLayer, LineNode, LineNodeError, LineNodeIterator, LinePiece, H0_NODE_CAP,
};

use super::streaming_reduction::{
    candidate_overlaps_source_span, candidate_wedge_owns, node_horizontal_range_from_receiver,
    origin_to_segment_distance_f32, range_ordered, GeometryError, MetricVector, SourceId64,
    SpanDecision, WedgeDecision,
};

/// One immutable physical screening candidate in receiver-centred metres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct H0Candidate {
    source_id: SourceId64,
    endpoint0_m: MetricVector,
    endpoint1_m: MetricVector,
    near_f32: f32,
    height_f32: f32,
}

impl H0Candidate {
    /// Build the candidate and round its full-edge near distance exactly once.
    pub fn from_metric_segment(
        source_id: SourceId64,
        endpoint0_m: MetricVector,
        endpoint1_m: MetricVector,
        height_f32: f32,
    ) -> Result<Self, GeometryError> {
        if !height_f32.is_finite() {
            return Err(GeometryError::NonFinite);
        }
        let near_f32 = origin_to_segment_distance_f32(endpoint0_m, endpoint1_m)?;
        Ok(Self {
            source_id,
            endpoint0_m,
            endpoint1_m,
            near_f32,
            height_f32,
        })
    }

    #[must_use]
    pub const fn source_id(self) -> SourceId64 {
        self.source_id
    }

    #[must_use]
    pub const fn endpoint0_m(self) -> MetricVector {
        self.endpoint0_m
    }

    #[must_use]
    pub const fn endpoint1_m(self) -> MetricVector {
        self.endpoint1_m
    }

    #[must_use]
    pub const fn near_f32(self) -> f32 {
        self.near_f32
    }

    #[must_use]
    pub const fn height_f32(self) -> f32 {
        self.height_f32
    }
}

/// One H0 quadrature node plus the physical geometry used by screening.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct H0Node {
    /// Frozen equal-u node and its source-frame abscissa.
    pub line_node: LineNode,
    /// Direction from the receiver-centred pair-frame origin to the node.
    pub receiver_vector_m: MetricVector,
    /// Physical horizontal receiver range used by source-granular admission.
    pub node_distance_m: f64,
}

/// Fail-closed errors from node generation or the streaming predicate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum H0ReductionError {
    /// Shared geometry rejected a non-finite or ambiguous pair input.
    Geometry(GeometryError),
    /// The one frozen node generator faulted, including its hard node cap.
    NodeGeneration(LineNodeError),
    /// A typed CPU candidate reached an otherwise unreachable hard wedge fault.
    CandidateHardFault,
    /// One provenience identified two different immutable candidate records.
    ProvenienceGeometryMismatch,
}

/// One exact multiplicity-free `S_pair` record used only by parity diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct H0SemanticCandidateRecord {
    pub source_id_bits: u64,
    pub endpoint0_x_bits: u64,
    pub endpoint0_y_bits: u64,
    pub endpoint1_x_bits: u64,
    pub endpoint1_y_bits: u64,
    pub near_f32_bits: u32,
    pub height_f32_bits: u32,
}

impl From<H0Candidate> for H0SemanticCandidateRecord {
    fn from(candidate: H0Candidate) -> Self {
        Self {
            source_id_bits: candidate.source_id.bits(),
            endpoint0_x_bits: candidate.endpoint0_m.x.to_bits(),
            endpoint0_y_bits: candidate.endpoint0_m.y.to_bits(),
            endpoint1_x_bits: candidate.endpoint1_m.x.to_bits(),
            endpoint1_y_bits: candidate.endpoint1_m.y.to_bits(),
            near_f32_bits: candidate.near_f32.to_bits(),
            height_f32_bits: candidate.height_f32.to_bits(),
        }
    }
}

/// Diagnostic-only `S_pair` plus raw-multiplicity facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct H0SemanticCandidateSet {
    pub records: Vec<H0SemanticCandidateRecord>,
    pub raw_candidate_visits: u64,
    pub guarded_degenerate_visits: u64,
}

/// Construct the exact multiplicity-free `S_pair` for diagnostic parity.
///
/// This function is deliberately separate from [`reduce_h0`]. Production H0
/// streams its raw superset and never materialises this set.
pub fn collect_h0_semantic_candidates<I>(
    piece_in_receiver_frame: LinePiece,
    candidates: I,
) -> Result<H0SemanticCandidateSet, H0ReductionError>
where
    I: IntoIterator<Item = H0Candidate>,
{
    validate_pair_inputs(piece_in_receiver_frame)?;
    let source0 = MetricVector::new(
        piece_in_receiver_frame.start_m[0],
        piece_in_receiver_frame.start_m[1],
    );
    let source1 = MetricVector::new(
        piece_in_receiver_frame.end_m[0],
        piece_in_receiver_frame.end_m[1],
    );
    let mut by_source_id = std::collections::BTreeMap::new();
    let mut raw_candidate_visits = 0_u64;
    let mut guarded_degenerate_visits = 0_u64;
    for candidate in candidates {
        raw_candidate_visits += 1;
        match candidate_overlaps_source_span(
            source0,
            source1,
            candidate.endpoint0_m,
            candidate.endpoint1_m,
            candidate.near_f32,
        ) {
            SpanDecision::Outside => {}
            SpanDecision::NearGuardedDegenerate => guarded_degenerate_visits += 1,
            SpanDecision::HardFault => return Err(H0ReductionError::CandidateHardFault),
            SpanDecision::Overlaps => {
                let record = H0SemanticCandidateRecord::from(candidate);
                if let Some(previous) = by_source_id.insert(candidate.source_id, record) {
                    if previous != record {
                        return Err(H0ReductionError::ProvenienceGeometryMismatch);
                    }
                }
            }
        }
    }
    let mut records: Vec<_> = by_source_id.into_values().collect();
    records.sort_unstable();
    Ok(H0SemanticCandidateSet {
        records,
        raw_candidate_visits,
        guarded_degenerate_visits,
    })
}

impl From<GeometryError> for H0ReductionError {
    fn from(error: GeometryError) -> Self {
        Self::Geometry(error)
    }
}

impl From<LineNodeError> for H0ReductionError {
    fn from(error: LineNodeError) -> Self {
        Self::NodeGeneration(error)
    }
}

/// Complete H0 reference result. Scratch is bounded by `H0_NODE_CAP`, never by
/// the number of streamed candidates.
#[derive(Debug, Clone)]
pub struct H0Reduction {
    nodes: Vec<H0Node>,
    admitted_mask_words: Vec<u64>,
    candidate_visit_count: u64,
    guarded_degenerate_candidate_count: u64,
}

impl H0Reduction {
    #[must_use]
    pub fn nodes(&self) -> &[H0Node] {
        &self.nodes
    }

    #[must_use]
    pub fn admitted_mask_words(&self) -> &[u64] {
        &self.admitted_mask_words
    }

    #[must_use]
    pub fn node_is_admitted(&self, node_index: usize) -> Option<bool> {
        if node_index >= self.nodes.len() {
            return None;
        }
        Some((self.admitted_mask_words[node_index / 64] & (1_u64 << (node_index % 64))) != 0)
    }

    #[must_use]
    pub fn any_node_is_admitted(&self) -> bool {
        self.admitted_mask_words.iter().any(|word| *word != 0)
    }

    #[must_use]
    pub fn admitted_node_count(&self) -> usize {
        self.admitted_mask_words
            .iter()
            .map(|word| word.count_ones() as usize)
            .sum()
    }

    #[must_use]
    pub const fn candidate_visit_count(&self) -> u64 {
        self.candidate_visit_count
    }

    #[must_use]
    pub const fn guarded_degenerate_candidate_count(&self) -> u64 {
        self.guarded_degenerate_candidate_count
    }
}

/// Generate frozen equal-u H0 nodes in a receiver-centred pair frame, then
/// stream the candidate store once.
///
/// The receiver is structurally the origin: callers provide the source piece
/// and every candidate in that same frame, so a non-origin representation
/// cannot reach the reference. The physical pass intentionally does not
/// materialise or clip an interval union. Nodes already lie inside the source
/// span; each raw candidate directly ORs the labelled orientation/range
/// predicate into the fixed node mask.
pub fn reduce_h0<I>(
    piece_in_receiver_frame: LinePiece,
    layer: LineLayer,
    candidates: I,
) -> Result<H0Reduction, H0ReductionError>
where
    I: IntoIterator<Item = H0Candidate>,
{
    validate_pair_inputs(piece_in_receiver_frame)?;

    let mut iterator = LineNodeIterator::new_h0(piece_in_receiver_frame, [0.0, 0.0], layer)?;
    let mut nodes = Vec::with_capacity(H0_NODE_CAP);
    while let Some(line_node) = iterator.next_checked()? {
        nodes.push(h0_node_geometry(line_node)?);
    }

    let mut admitted_mask_words = vec![0_u64; nodes.len().div_ceil(64)];
    let mut candidate_visit_count = 0_u64;
    let mut guarded_degenerate_candidate_count = 0_u64;
    for candidate in candidates {
        candidate_visit_count += 1;
        let mut guarded_degenerate = false;
        for (node_index, node) in nodes.iter().enumerate() {
            match candidate_wedge_owns(
                candidate.endpoint0_m,
                candidate.endpoint1_m,
                node.receiver_vector_m,
                candidate.near_f32,
            ) {
                WedgeDecision::DoesNotOwn => {}
                WedgeDecision::Owns => {
                    if range_ordered(node.node_distance_m, candidate.near_f32)? {
                        admitted_mask_words[node_index / 64] |= 1_u64 << (node_index % 64);
                    }
                }
                WedgeDecision::NearGuardedDegenerate => {
                    guarded_degenerate = true;
                    break;
                }
                WedgeDecision::HardFault => return Err(H0ReductionError::CandidateHardFault),
            }
        }
        if guarded_degenerate {
            guarded_degenerate_candidate_count += 1;
        }
    }

    Ok(H0Reduction {
        nodes,
        admitted_mask_words,
        candidate_visit_count,
        guarded_degenerate_candidate_count,
    })
}

fn validate_pair_inputs(piece: LinePiece) -> Result<(), H0ReductionError> {
    let values = [
        piece.start_m[0],
        piece.start_m[1],
        piece.end_m[0],
        piece.end_m[1],
        piece.emission_per_m,
    ];
    if values.into_iter().all(f64::is_finite) {
        Ok(())
    } else {
        Err(H0ReductionError::Geometry(GeometryError::NonFinite))
    }
}

fn h0_node_geometry(line_node: LineNode) -> Result<H0Node, H0ReductionError> {
    let receiver_vector_m = MetricVector::new(line_node.position_m[0], line_node.position_m[1]);
    let node_distance_m = node_horizontal_range_from_receiver(receiver_vector_m)?;
    Ok(H0Node {
        line_node,
        receiver_vector_m,
        node_distance_m,
    })
}

#[cfg(test)]
mod semantic_set_tests {
    use super::*;

    fn candidate(id: u64, endpoint0: (f64, f64), endpoint1: (f64, f64)) -> H0Candidate {
        H0Candidate::from_metric_segment(
            SourceId64::obstacle(id).unwrap(),
            MetricVector::new(endpoint0.0, endpoint0.1),
            MetricVector::new(endpoint1.0, endpoint1.1),
            12.0,
        )
        .unwrap()
    }

    fn piece() -> LinePiece {
        LinePiece {
            start_m: [100.0, 0.0],
            end_m: [0.0, 100.0],
            emission_per_m: 1.0,
        }
    }

    #[test]
    fn semantic_set_is_stable_under_duplicates_and_permutations() {
        let first = candidate(9, (50.0, -5.0), (50.0, 50.0));
        let second = candidate(3, (60.0, 10.0), (10.0, 60.0));
        let outside = candidate(7, (-50.0, -10.0), (-10.0, -50.0));
        let a = collect_h0_semantic_candidates(piece(), [first, second, first, outside]).unwrap();
        let b = collect_h0_semantic_candidates(piece(), [outside, first, second, first]).unwrap();
        assert_eq!(a.records, b.records);
        assert_eq!(a.records.len(), 2);
        assert_eq!(a.raw_candidate_visits, 4);
        assert_eq!(b.raw_candidate_visits, 4);
    }

    #[test]
    fn semantic_set_keeps_equal_geometry_with_distinct_provenience() {
        let first = candidate(1, (50.0, -5.0), (50.0, 50.0));
        let second = candidate(2, (50.0, -5.0), (50.0, 50.0));
        let set = collect_h0_semantic_candidates(piece(), [first, second]).unwrap();
        assert_eq!(set.records.len(), 2);
        assert_ne!(set.records[0].source_id_bits, set.records[1].source_id_bits);
    }

    #[test]
    fn semantic_set_rejects_one_provenience_with_different_geometry() {
        let first = candidate(1, (50.0, -5.0), (50.0, 50.0));
        let changed = candidate(1, (50.0, -6.0), (50.0, 50.0));
        assert_eq!(
            collect_h0_semantic_candidates(piece(), [first, changed]),
            Err(H0ReductionError::ProvenienceGeometryMismatch)
        );
    }

    #[test]
    fn pass_a_mask_is_idempotent_under_duplicates_and_permutations() {
        let first = candidate(9, (50.0, -5.0), (50.0, 50.0));
        let second = candidate(3, (60.0, 10.0), (10.0, 60.0));
        let a = reduce_h0(piece(), LineLayer::Road, [first, second, first]).unwrap();
        let b = reduce_h0(piece(), LineLayer::Road, [second, first]).unwrap();
        assert_eq!(a.nodes(), b.nodes());
        assert_eq!(a.admitted_mask_words(), b.admitted_mask_words());
        assert_eq!(a.candidate_visit_count(), 3);
        assert_eq!(b.candidate_visit_count(), 2);
    }

    #[test]
    fn equal_u_piece_collapses_to_zero_nodes_without_a_fault() {
        let start_x = 10.0_f64;
        let end_x = f64::from_bits(start_x.to_bits() + 1);
        let result = reduce_h0(
            LinePiece {
                start_m: [start_x, 3.0],
                end_m: [end_x, 3.0],
                emission_per_m: 1.0,
            },
            LineLayer::Road,
            [candidate(0, (2.0, -1.0), (2.0, 1.0))],
        )
        .expect("equal-u dedup is a clean empty quadrature");
        assert!(result.nodes().is_empty());
        assert!(result.admitted_mask_words().is_empty());
        assert_eq!(result.candidate_visit_count(), 1);
    }
}

#[cfg(test)]
mod cross_lane_tests {
    use super::*;

    const INPUT_WORDS: usize = 11;
    const OUTPUT_WORDS: usize = 9;
    const RECORD_WORDS: usize = INPUT_WORDS + OUTPUT_WORDS;
    const RECORD_COUNT: usize = 1_000_000;
    const NODE_WORDS: usize = 8;
    const FIXTURE_HEADER_WORDS: usize = 2;
    const FIXTURE_SCALAR_WORDS: usize = 7;
    const FIXTURE_OUTPUT_WORDS: usize = FIXTURE_SCALAR_WORDS + H0_NODE_CAP * NODE_WORDS;
    const FIXTURE_MAGIC: u64 = 0x514d_4830_4649_5831;

    fn hash_nodes(nodes: &[H0Node]) -> (u64, u64) {
        let mut left = 0xcbf2_9ce4_8422_2325_u64;
        let mut right = 0x6a09_e667_f3bc_c909_u64;
        for node in nodes {
            let words = [
                node.line_node.position_m[0].to_bits(),
                node.line_node.position_m[1].to_bits(),
                node.line_node.piece_fraction.to_bits(),
                node.line_node.weight.to_bits(),
                node.line_node.placement_distance_m.to_bits(),
                node.line_node.node_x_m.to_bits(),
                node.node_distance_m.to_bits(),
                node.line_node.arm as i64 as u64,
            ];
            for word in words {
                left ^= word;
                left = left.wrapping_mul(0x0000_0100_0000_01b3);
                right = right
                    .wrapping_add(word.wrapping_add(0x9e37_79b9_7f4a_7c15))
                    .rotate_left(27)
                    .wrapping_mul(0x94d0_49bb_1331_11eb);
            }
        }
        (left, right)
    }

    fn node_words(node: H0Node) -> [u64; NODE_WORDS] {
        [
            node.line_node.position_m[0].to_bits(),
            node.line_node.position_m[1].to_bits(),
            node.line_node.piece_fraction.to_bits(),
            node.line_node.weight.to_bits(),
            node.line_node.placement_distance_m.to_bits(),
            node.line_node.node_x_m.to_bits(),
            node.node_distance_m.to_bits(),
            node.line_node.arm as i64 as u64,
        ]
    }

    fn explicit_inputs() -> Vec<[u64; INPUT_WORDS]> {
        let values = [
            [-125.0, 0.0, 125.0, 0.0, 1.0, 1.0, 2.0, -1.0, 2.0, 1.0, 12.0],
            [-100.0, 0.0, 100.0, 0.0, 1.0, 0.0, 5.0, -1.0, 5.0, 1.0, 12.0],
            [10.0, 0.0, 100.0, 0.0, 1.0, 0.0, 5.0, -1.0, 5.0, 1.0, 12.0],
            [10.0, 0.0, 100.0, 0.0, 1.0, 0.0, 5.0, 0.0, 0.0, -5.0, 12.0],
            [0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 5.0, -1.0, 5.0, 1.0, 12.0],
            [0.0, 0.0, 251.0, 0.0, 1.0, 0.0, 5.0, -1.0, 5.0, 1.0, 12.0],
            [f64::NAN, 0.0, 1.0, 0.0, 1.0, 0.0, 5.0, -1.0, 5.0, 1.0, 12.0],
            [
                10.0,
                0.0,
                100.0,
                0.0,
                1.0,
                0.0,
                f64::NAN,
                0.0,
                5.0,
                1.0,
                12.0,
            ],
            [
                10.0,
                3.0,
                f64::from_bits(10.0_f64.to_bits() + 1),
                3.0,
                1.0,
                0.0,
                2.0,
                -1.0,
                2.0,
                1.0,
                12.0,
            ],
        ];
        values
            .into_iter()
            .map(|values| {
                let mut words = values.map(f64::to_bits);
                words[5] = values[5] as u64;
                words
            })
            .collect()
    }

    fn reduction_from_input(input: [u64; INPUT_WORDS]) -> Result<H0Reduction, H0ReductionError> {
        let value = |index: usize| f64::from_bits(input[index]);
        let piece = LinePiece {
            start_m: [value(0), value(1)],
            end_m: [value(2), value(3)],
            emission_per_m: value(4),
        };
        let layer = if input[5] == 0 {
            LineLayer::Road
        } else {
            LineLayer::Rail
        };
        let candidate = H0Candidate::from_metric_segment(
            SourceId64::obstacle(0).unwrap(),
            MetricVector::new(value(6), value(7)),
            MetricVector::new(value(8), value(9)),
            value(10) as f32,
        );
        candidate
            .map_err(H0ReductionError::from)
            .and_then(|candidate| reduce_h0(piece, layer, [candidate]))
    }

    fn fault_word(result: &Result<H0Reduction, H0ReductionError>) -> u64 {
        match result {
            Ok(_) => 0,
            Err(H0ReductionError::Geometry(_)) => 1,
            Err(H0ReductionError::NodeGeneration(LineNodeError::DegeneratePiece)) => 2,
            Err(H0ReductionError::NodeGeneration(LineNodeError::OverlengthPiece)) => 4,
            Err(H0ReductionError::NodeGeneration(LineNodeError::NodeCapOverflow)) => 8,
            Err(H0ReductionError::CandidateHardFault) => 16,
            Err(H0ReductionError::ProvenienceGeometryMismatch) => 32,
        }
    }

    fn parity_record(input: [u64; INPUT_WORDS]) -> [u64; RECORD_WORDS] {
        let value = |index: usize| f64::from_bits(input[index]);
        let candidate0 = MetricVector::new(value(6), value(7));
        let candidate1 = MetricVector::new(value(8), value(9));
        let near_f32 = origin_to_segment_distance_f32(candidate0, candidate1).unwrap_or(f32::NAN);
        let span_decision = candidate_overlaps_source_span(
            MetricVector::new(value(0), value(1)),
            MetricVector::new(value(2), value(3)),
            candidate0,
            candidate1,
            near_f32,
        ) as u64;
        let result = reduction_from_input(input);

        let output = match result {
            Ok(reduction) => {
                let (left, right) = hash_nodes(reduction.nodes());
                [
                    reduction.nodes().len() as u64,
                    0,
                    left,
                    right,
                    reduction
                        .admitted_mask_words()
                        .first()
                        .copied()
                        .unwrap_or(0),
                    reduction.admitted_mask_words().get(1).copied().unwrap_or(0),
                    reduction.candidate_visit_count(),
                    reduction.guarded_degenerate_candidate_count(),
                    span_decision,
                ]
            }
            Err(error) => [0, fault_word(&Err(error)), 0, 0, 0, 0, 0, 0, span_decision],
        };
        let mut record = [0_u64; RECORD_WORDS];
        record[..INPUT_WORDS].copy_from_slice(&input);
        record[INPUT_WORDS..].copy_from_slice(&output);
        record
    }

    /// Emit the exact variable-node Rust authority as fixed-size per-record
    /// 128-bit node digests plus the complete two-word H0 mask. Direct fixtures
    /// separately compare every node field; the million-record corpus prices
    /// all count transitions without a multi-gigabyte dump.
    #[test]
    fn cross_lane_h0_node_and_mask_dump() {
        let Ok(path) = std::env::var("QM_H0_NODE_DUMP") else {
            return;
        };

        let explicit = explicit_inputs();

        struct Lcg(u64);
        impl Lcg {
            fn next(&mut self) -> u64 {
                self.0 = self
                    .0
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                self.0
            }
            fn finite(&mut self, extent: f64) -> f64 {
                let unit = (self.next() >> 11) as f64 * (1.0 / 9_007_199_254_740_992.0);
                (unit * 2.0 - 1.0) * extent
            }
        }

        let mut rng = Lcg(0xbb67_ae85_84ca_a73b);
        let mut bytes = Vec::with_capacity(RECORD_COUNT * RECORD_WORDS * 8);
        let mut counts = std::collections::BTreeSet::new();
        for index in 0..RECORD_COUNT {
            let input = explicit.get(index).copied().unwrap_or_else(|| {
                let start_x = rng.finite(10_000.0);
                let start_y = rng.finite(10_000.0);
                let end_x = start_x + rng.finite(125.0) * 0.7;
                let end_y = start_y + rng.finite(125.0) * 0.7;
                let values = [
                    start_x,
                    start_y,
                    end_x,
                    end_y,
                    0.25 + rng.finite(0.2).abs(),
                    0.0,
                    rng.finite(250.0),
                    rng.finite(250.0),
                    rng.finite(250.0),
                    rng.finite(250.0),
                    1.0 + rng.finite(30.0).abs(),
                ];
                let mut words = values.map(f64::to_bits);
                words[5] = rng.next() & 1;
                words
            });
            let record = parity_record(input);
            if record[INPUT_WORDS + 1] == 0 {
                counts.insert(record[INPUT_WORDS]);
            }
            for word in record {
                bytes.extend_from_slice(&word.to_le_bytes());
            }
        }
        assert!(
            counts.contains(&64),
            "rail worst-case count remains in corpus"
        );
        assert!(
            counts.len() >= 20,
            "corpus must exercise node-count transitions"
        );
        std::fs::write(&path, bytes).expect("H0 node dump path writable");
        println!("wrote {RECORD_COUNT} H0 node/mask authority records to {path}");
    }

    /// Emit complete node fields for every direct geometry. This closes the
    /// digest-shaped corpus's collision surface without making the one-million
    /// record authority several gigabytes large.
    #[test]
    fn cross_lane_h0_direct_fixture_dump() {
        let Ok(path) = std::env::var("QM_H0_NODE_FIXTURE_DUMP") else {
            return;
        };
        let fixtures = explicit_inputs();
        let fixture_count = fixtures.len();
        let mut words = Vec::with_capacity(
            FIXTURE_HEADER_WORDS + fixtures.len() * (INPUT_WORDS + FIXTURE_OUTPUT_WORDS),
        );
        words.push(FIXTURE_MAGIC);
        words.push(fixtures.len() as u64);
        for input in fixtures {
            words.extend_from_slice(&input);
            let span_decision = parity_record(input)[INPUT_WORDS + OUTPUT_WORDS - 1];
            let result = reduction_from_input(input);
            words.push(fault_word(&result));
            if let Ok(reduction) = result {
                words.push(reduction.nodes().len() as u64);
                words.push(
                    reduction
                        .admitted_mask_words()
                        .first()
                        .copied()
                        .unwrap_or(0),
                );
                words.push(reduction.admitted_mask_words().get(1).copied().unwrap_or(0));
                words.push(reduction.candidate_visit_count());
                words.push(reduction.guarded_degenerate_candidate_count());
                words.push(candidate_overlaps_source_span(
                    MetricVector::new(f64::from_bits(input[0]), f64::from_bits(input[1])),
                    MetricVector::new(f64::from_bits(input[2]), f64::from_bits(input[3])),
                    MetricVector::new(f64::from_bits(input[6]), f64::from_bits(input[7])),
                    MetricVector::new(f64::from_bits(input[8]), f64::from_bits(input[9])),
                    origin_to_segment_distance_f32(
                        MetricVector::new(f64::from_bits(input[6]), f64::from_bits(input[7])),
                        MetricVector::new(f64::from_bits(input[8]), f64::from_bits(input[9])),
                    )
                    .unwrap_or(f32::NAN),
                ) as u64);
                for index in 0..H0_NODE_CAP {
                    if let Some(node) = reduction.nodes().get(index) {
                        words.extend_from_slice(&node_words(*node));
                    } else {
                        words.extend_from_slice(&[0; NODE_WORDS]);
                    }
                }
            } else {
                words.extend_from_slice(&[0; FIXTURE_SCALAR_WORDS - 2]);
                words.push(span_decision);
                words.extend_from_slice(&[0; H0_NODE_CAP * NODE_WORDS]);
            }
        }
        let bytes: Vec<_> = words.into_iter().flat_map(u64::to_le_bytes).collect();
        std::fs::write(&path, bytes).expect("H0 direct-fixture dump path writable");
        println!("wrote {fixture_count} complete H0 node fixtures to {path}");
    }
}
