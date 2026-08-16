//! Bounded CPU reference for the model-v2 H0 streaming screening reducer.

use crate::compute::element::{
    LineLayer, LineNode, LineNodeError, LineNodeIterator, LinePiece, H0_NODE_CAP,
};

use super::streaming_reduction::{
    candidate_wedge_owns, node_horizontal_range_from_receiver, origin_to_segment_distance_f32,
    range_ordered, GeometryError, MetricVector, SourceId64, WedgeDecision,
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
