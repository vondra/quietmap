//! Bounded-large, zero-drop, unmasked CPU judge node construction.

use std::collections::BTreeSet;

use crate::compute::element::{LineLayer, LinePiece};

use super::h0_streaming_reduction::{
    judge_nodes_with_u_hints, H0Candidate, H0Node, H0ReductionError,
};
use super::h0_v3::{H0V3Error, H_JUDGE_MAX};
use super::shared_math::{qm_atan, qm_atan2, qm_wrap_pi};
use super::streaming_reduction::{
    candidate_overlaps_source_span, candidate_wedge_owns, dot, orient, radial_range_root, same_ray,
    total_f64, MetricVector, SpanDecision, WedgeDecision,
};

const JUDGE_HINT_LOGICAL_BYTES: usize = 34;
const RANGE_BOUNDARY_FIRST_BIN: i16 = -18;

// Generated once from Rust `1.5f32.powi(k)` for k=-18..=32. The source bytes
// are the table authority; pass H never calls host libm to classify a range.
// The current audibility domain ends below k=32, and leaving a final boundary
// above it makes an out-of-domain future reach a hard error instead of silently
// inventing a bin.
const RANGE_BOUNDARY_F32_BITS: [u32; 51] = [
    0x3a316082, 0x3a850862, 0x3ac78c92, 0x3b15a96e, 0x3b607e24, 0x3ba85e9b, 0x3bfc8de9, 0x3c3d6a6f,
    0x3c8e0fd3, 0x3cd517bc, 0x3d1fd1cd, 0x3d6fbab4, 0x3db3cc07, 0x3e06d905, 0x3e4a4588, 0x3e97b426,
    0x3ee38e39, 0x3f2aaaab, 0x3f800000, 0x3fc00000, 0x40100000, 0x40580000, 0x40a20000, 0x40f30000,
    0x41364000, 0x4188b000, 0x41cd0800, 0x4219c600, 0x4266a900, 0x42acfec0, 0x4301bf10, 0x43429e98,
    0x4391f6f2, 0x43daf26b, 0x442435d0, 0x447650b8, 0x44b8bc8a, 0x450a8d68, 0x454fd41b, 0x459bdf14,
    0x45e9ce9f, 0x462f5af7, 0x46838439, 0x46c54656, 0x4713f4c0, 0x475def21, 0x47a67358, 0x47f9ad05,
    0x483b41c4, 0x488c7153, 0x48d2a9fc,
];

/// The exact 34-byte logical SR-4 key retained by the bounded-large judge.
/// Rust padding/container overhead is not part of the frozen logical layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct JudgeHintRecord {
    range_bin: i16,
    wedge_lo_bits: u64,
    wedge_hi_bits: u64,
    source_id_bits: u64,
    u_bits: u64,
}

impl Ord for JudgeHintRecord {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.range_bin
            .cmp(&other.range_bin)
            .then_with(|| {
                total_key_from_canonical_bits(self.wedge_lo_bits)
                    .cmp(&total_key_from_canonical_bits(other.wedge_lo_bits))
            })
            .then_with(|| {
                total_key_from_canonical_bits(self.wedge_hi_bits)
                    .cmp(&total_key_from_canonical_bits(other.wedge_hi_bits))
            })
            .then_with(|| self.source_id_bits.cmp(&other.source_id_bits))
            .then_with(|| {
                total_key_from_canonical_bits(self.u_bits)
                    .cmp(&total_key_from_canonical_bits(other.u_bits))
            })
    }
}

impl PartialOrd for JudgeHintRecord {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Bounded-large, zero-drop judge construction facts for one pair.
#[derive(Debug, Clone)]
pub struct H0JudgeNodes {
    pub nodes: Vec<H0Node>,
    pub distinct_hint_records: usize,
    pub unique_u_hints: usize,
    pub logical_hint_storage_bytes: usize,
}

/// Construct the independent CPU judge. No production admission mask is built
/// or consulted, and every returned node executes the full vector walk.
pub fn build_h0_judge_nodes<I>(
    piece_in_receiver_frame: LinePiece,
    layer: LineLayer,
    epsilon_degrees: f64,
    candidates: I,
) -> Result<H0JudgeNodes, H0V3Error>
where
    I: IntoIterator<Item = H0Candidate>,
{
    if !epsilon_degrees.is_finite() || epsilon_degrees <= 0.0 {
        return Err(H0V3Error::InvalidPairGeometry);
    }
    let frame = PairFrame::new(piece_in_receiver_frame, layer)?;
    let mut records = BTreeSet::new();
    for candidate in candidates {
        match candidate_overlaps_source_span(
            frame.source0,
            frame.source1,
            candidate.endpoint0_m(),
            candidate.endpoint1_m(),
            candidate.near_f32(),
        ) {
            SpanDecision::Outside | SpanDecision::NearGuardedDegenerate => continue,
            SpanDecision::HardFault => return Err(H0ReductionError::CandidateHardFault.into()),
            SpanDecision::Overlaps => {}
        }
        frame.emit_candidate_hints(candidate, &mut records)?;
        if records.len() > H_JUDGE_MAX {
            return Err(H0V3Error::JudgeHintCapacityExceeded);
        }
    }
    let distinct_hint_records = records.len();
    let logical_hint_storage_bytes = distinct_hint_records
        .checked_mul(JUDGE_HINT_LOGICAL_BYTES)
        .and_then(|bytes| bytes.checked_add(32))
        .ok_or(H0V3Error::InvalidPairGeometry)?;
    let mut u_hints: Vec<f64> = records
        .into_iter()
        .map(|record| f64::from_bits(record.u_bits))
        .collect();
    u_hints.sort_unstable_by(f64::total_cmp);
    u_hints.dedup_by(|left, right| *left == *right);
    let nodes = judge_nodes_with_u_hints(
        piece_in_receiver_frame,
        layer,
        epsilon_degrees.to_radians(),
        H_JUDGE_MAX,
        &u_hints,
    )?;
    Ok(H0JudgeNodes {
        nodes,
        distinct_hint_records,
        unique_u_hints: u_hints.len(),
        logical_hint_storage_bytes,
    })
}

fn total_key_from_canonical_bits(bits: u64) -> u64 {
    if (bits >> 63) != 0 {
        !bits
    } else {
        bits ^ (1_u64 << 63)
    }
}

fn range_bin(near_f32: f32) -> Result<i16, H0V3Error> {
    if !near_f32.is_finite() || near_f32 < 0.0 {
        return Err(H0V3Error::InvalidPairGeometry);
    }
    let classified = near_f32.max(1.0e-3);
    let mut bin = None;
    for (index, bits) in RANGE_BOUNDARY_F32_BITS.iter().copied().enumerate() {
        if f32::from_bits(bits) <= classified {
            bin = Some(RANGE_BOUNDARY_FIRST_BIN + index as i16);
        } else {
            break;
        }
    }
    let bin = bin.ok_or(H0V3Error::InvalidPairGeometry)?;
    if classified >= f32::from_bits(*RANGE_BOUNDARY_F32_BITS.last().unwrap()) {
        return Err(H0V3Error::InvalidPairGeometry);
    }
    Ok(bin)
}

#[derive(Debug, Clone, Copy)]
struct PairFrame {
    source0: MetricVector,
    source1: MetricVector,
    direction: MetricVector,
    foot_vector: MetricVector,
    foot_from_start_m: f64,
    length_m: f64,
    h_m: f64,
    low_x_m: f64,
    high_x_m: f64,
    radial_direction: Option<MetricVector>,
    receiver_crossing: bool,
}

impl PairFrame {
    fn new(piece: LinePiece, layer: LineLayer) -> Result<Self, H0V3Error> {
        let source0 = MetricVector::new(piece.start_m[0], piece.start_m[1]);
        let source1 = MetricVector::new(piece.end_m[0], piece.end_m[1]);
        let dx = source1.x - source0.x;
        let dy = source1.y - source0.y;
        let length_m = (dx * dx + dy * dy).sqrt();
        if !length_m.is_finite() || length_m == 0.0 {
            return Err(H0V3Error::InvalidPairGeometry);
        }
        let direction = MetricVector::new(dx / length_m, dy / length_m);
        let foot_from_start_m = -(source0.x * direction.x + source0.y * direction.y);
        let foot_vector = MetricVector::new(
            source0.x + foot_from_start_m * direction.x,
            source0.y + foot_from_start_m * direction.y,
        );
        let d_perp = (foot_vector.x * foot_vector.x + foot_vector.y * foot_vector.y).sqrt();
        let h_m = d_perp.max(layer.d_floor_m());
        let low_x_m = (-foot_from_start_m).min(length_m - foot_from_start_m);
        let high_x_m = (-foot_from_start_m).max(length_m - foot_from_start_m);
        let source0_zero = source0.x == 0.0 && source0.y == 0.0;
        let source1_zero = source1.x == 0.0 && source1.y == 0.0;
        let radial_direction = if source0_zero {
            (!source1_zero).then_some(source1)
        } else if source1_zero || same_ray(source0, source1) == Ok(true) {
            Some(source0)
        } else {
            None
        };
        let receiver_crossing = !source0_zero
            && !source1_zero
            && orient(source0, source1) == 0.0
            && dot(source0, source1) < 0.0;
        Ok(Self {
            source0,
            source1,
            direction,
            foot_vector,
            foot_from_start_m,
            length_m,
            h_m,
            low_x_m,
            high_x_m,
            radial_direction,
            receiver_crossing,
        })
    }

    fn emit_candidate_hints(
        self,
        candidate: H0Candidate,
        records: &mut BTreeSet<JudgeHintRecord>,
    ) -> Result<(), H0V3Error> {
        let radial = self.radial_direction.is_some() || self.receiver_crossing;
        let normal_wedge = if radial {
            None
        } else {
            Some(self.clipped_wedge_bits(candidate, None)?)
        };
        if !radial {
            for endpoint in [candidate.endpoint0_m(), candidate.endpoint1_m()] {
                if candidate_wedge_owns(self.source0, self.source1, endpoint, 1.0)
                    == WedgeDecision::Owns
                {
                    if let Some(u) = self.u_for_ray(endpoint)? {
                        self.insert_hint(candidate, normal_wedge.unwrap(), u, records)?;
                    }
                }
            }
        }
        if let Some(root) = radial_range_root(self.perpendicular_distance_m(), candidate.near_f32())
            .map_err(H0ReductionError::from)?
        {
            for x_m in [-root, root] {
                if !(self.low_x_m < x_m && x_m < self.high_x_m) {
                    continue;
                }
                let node = MetricVector::new(
                    self.foot_vector.x + self.direction.x * x_m,
                    self.foot_vector.y + self.direction.y * x_m,
                );
                if candidate_wedge_owns(
                    candidate.endpoint0_m(),
                    candidate.endpoint1_m(),
                    node,
                    candidate.near_f32(),
                ) == WedgeDecision::Owns
                {
                    let wedge = if radial {
                        self.clipped_wedge_bits(candidate, Some(node))?
                    } else {
                        normal_wedge.unwrap()
                    };
                    self.insert_hint(candidate, wedge, qm_atan(x_m / self.h_m), records)?;
                }
            }
        }
        Ok(())
    }

    fn clipped_wedge_bits(
        self,
        candidate: H0Candidate,
        radial_hint_direction: Option<MetricVector>,
    ) -> Result<(u64, u64), H0V3Error> {
        if let Some(direction) = radial_hint_direction.or(self.radial_direction) {
            let angle = canonical_angle(qm_atan2(direction.y, direction.x))?;
            return Ok((angle.to_bits(), angle.to_bits()));
        }
        if self.receiver_crossing {
            return Err(H0V3Error::InvalidPairGeometry);
        }

        let candidate0 = candidate.endpoint0_m();
        let candidate1 = candidate.endpoint1_m();
        let mut boundaries = Vec::with_capacity(4);
        for direction in [self.source0, self.source1, candidate0, candidate1] {
            if closed_wedge_contains(self.source0, self.source1, direction, 1.0)?
                && closed_wedge_contains(candidate0, candidate1, direction, candidate.near_f32())?
                && !boundaries
                    .iter()
                    .copied()
                    .any(|previous| same_ray(previous, direction) == Ok(true))
            {
                boundaries.push(direction);
            }
        }
        if boundaries.is_empty() {
            return Err(H0V3Error::InvalidPairGeometry);
        }
        let source_start = qm_atan2(self.source0.y, self.source0.x);
        let source_delta = qm_wrap_pi(qm_atan2(self.source1.y, self.source1.x) - source_start);
        if source_delta == 0.0 || !source_delta.is_finite() {
            return Err(H0V3Error::InvalidPairGeometry);
        }
        let mut angles = Vec::with_capacity(boundaries.len());
        for direction in boundaries {
            let mut relative = qm_wrap_pi(qm_atan2(direction.y, direction.x) - source_start);
            if source_delta > 0.0 && relative < 0.0 {
                relative += std::f64::consts::TAU;
            } else if source_delta < 0.0 && relative > 0.0 {
                relative -= std::f64::consts::TAU;
            }
            angles.push(canonical_angle(source_start + relative)?);
        }
        angles.sort_unstable_by(f64::total_cmp);
        Ok((
            angles.first().unwrap().to_bits(),
            angles.last().unwrap().to_bits(),
        ))
    }

    fn perpendicular_distance_m(self) -> f64 {
        (self.foot_vector.x * self.foot_vector.x + self.foot_vector.y * self.foot_vector.y).sqrt()
    }

    fn u_for_ray(self, ray: MetricVector) -> Result<Option<f64>, H0V3Error> {
        let line_dx = self.source1.x - self.source0.x;
        let line_dy = self.source1.y - self.source0.y;
        let denominator = line_dx * ray.y - line_dy * ray.x;
        if denominator == 0.0 {
            return Ok(None);
        }
        let fraction = -(self.source0.x * ray.y - self.source0.y * ray.x) / denominator;
        if !fraction.is_finite() {
            return Err(H0V3Error::InvalidPairGeometry);
        }
        if !(0.0 < fraction && fraction < 1.0) {
            return Ok(None);
        }
        let x_m = fraction * self.length_m - self.foot_from_start_m;
        Ok(Some(qm_atan(x_m / self.h_m)))
    }

    fn insert_hint(
        self,
        candidate: H0Candidate,
        wedge: (u64, u64),
        u: f64,
        records: &mut BTreeSet<JudgeHintRecord>,
    ) -> Result<(), H0V3Error> {
        let u = canonical_angle(u)?;
        records.insert(JudgeHintRecord {
            range_bin: range_bin(candidate.near_f32())?,
            wedge_lo_bits: wedge.0,
            wedge_hi_bits: wedge.1,
            source_id_bits: candidate.source_id().bits(),
            u_bits: u.to_bits(),
        });
        Ok(())
    }
}

fn canonical_angle(value: f64) -> Result<f64, H0V3Error> {
    total_f64(value).map_err(H0ReductionError::from)?;
    Ok(if value == 0.0 { 0.0 } else { value })
}

fn closed_wedge_contains(
    start: MetricVector,
    end: MetricVector,
    direction: MetricVector,
    near_f32: f32,
) -> Result<bool, H0V3Error> {
    match candidate_wedge_owns(start, end, direction, near_f32) {
        WedgeDecision::Owns => Ok(true),
        WedgeDecision::DoesNotOwn => same_ray(end, direction)
            .map_err(H0ReductionError::from)
            .map_err(H0V3Error::from),
        WedgeDecision::NearGuardedDegenerate => Ok(false),
        WedgeDecision::HardFault => Err(H0ReductionError::CandidateHardFault.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::super::streaming_reduction::SourceId64;
    use super::*;

    fn piece() -> LinePiece {
        LinePiece {
            start_m: [-125.0, 30.0],
            end_m: [125.0, 30.0],
            emission_per_m: 1.0,
        }
    }

    fn candidate(id: u64, x: f64, y0: f64, y1: f64) -> H0Candidate {
        H0Candidate::from_metric_segment(
            SourceId64::obstacle(id).unwrap(),
            MetricVector::new(x, y0),
            MetricVector::new(x, y1),
            8.0,
        )
        .unwrap()
    }

    #[test]
    fn hints_are_duplicate_and_order_independent() {
        let a = candidate(1, 8.0, -4.0, 4.0);
        let b = candidate(2, -12.0, -6.0, 6.0);
        let first = build_h0_judge_nodes(piece(), LineLayer::Road, 0.5, [a, b, a]).unwrap();
        let second = build_h0_judge_nodes(piece(), LineLayer::Road, 0.5, [b, a]).unwrap();
        assert_eq!(first.distinct_hint_records, second.distinct_hint_records);
        assert_eq!(first.unique_u_hints, second.unique_u_hints);
        assert_eq!(first.nodes, second.nodes);
        assert!(first.distinct_hint_records > 0);
    }

    #[test]
    fn range_bins_are_signed_and_boundary_exact() {
        assert_eq!(range_bin(0.0).unwrap(), -18);
        assert_eq!(range_bin(f32::from_bits(0x3f800000)).unwrap(), 0);
        assert_eq!(
            range_bin(f32::from_bits(0x3f800000) - f32::EPSILON).unwrap(),
            -1
        );
        assert_eq!(range_bin(f32::from_bits(0x3fc00000)).unwrap(), 1);
        assert_eq!(
            range_bin(f32::from_bits(0x3fc00000) - f32::EPSILON).unwrap(),
            0
        );
        assert_eq!(RANGE_BOUNDARY_F32_BITS.len(), 51);
    }

    #[test]
    fn radial_range_hint_survives_without_an_angular_span() {
        let radial_piece = LinePiece {
            start_m: [10.0, 0.0],
            end_m: [100.0, 0.0],
            emission_per_m: 1.0,
        };
        let wall = candidate(1, 50.0, -2.0, 2.0);
        let judge = build_h0_judge_nodes(radial_piece, LineLayer::Road, 0.5, [wall]).unwrap();
        assert!(judge.distinct_hint_records > 0);
        assert_eq!(
            judge.logical_hint_storage_bytes,
            judge.distinct_hint_records * JUDGE_HINT_LOGICAL_BYTES + 32
        );
        assert!(judge.nodes.iter().all(|node| node.line_node.arm == 1));
    }
}
