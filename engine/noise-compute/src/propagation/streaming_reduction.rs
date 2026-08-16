//! Shared typed geometry and provenience for the model-v2 streaming reducer.
//!
//! This module owns decisions whose exact bits or branches must match CUDA.
//! Its device mirror is `noise-gpu/kernels/qm_streaming_reduction.cuh`; both use
//! the `SRM-*` equation labels below. It deliberately contains no reducer, node
//! generator, retained interval, hint selector, or production model switch.

use crate::constants::m_per_deg_lon;

/// High-bit namespace tag for a wall microsegment.
pub const WALL_SOURCE_TAG: u64 = 1_u64 << 63;
/// The extraction contract leaves 47 payload bits for a non-negative OSM id.
pub const WALL_OSM_ID_LIMIT: i64 = 1_i64 << 47;

/// Stable lane-identical identity of one physical screening candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct SourceId64(u64);

/// A source identity cannot be represented without violating its namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceIdError {
    /// An obstacle ordinal entered the wall namespace.
    ObstacleOrdinalOutOfRange,
    /// A wall OSM id cannot fit the frozen 47-bit payload.
    WallOsmIdOutOfRange,
}

impl SourceId64 {
    /// Obstacle edge identity: its ordinal in the packed ordered `ObstacleSet`.
    pub fn obstacle(flattened_edge_ordinal: u64) -> Result<Self, SourceIdError> {
        if flattened_edge_ordinal >= WALL_SOURCE_TAG {
            return Err(SourceIdError::ObstacleOrdinalOutOfRange);
        }
        Ok(Self(flattened_edge_ordinal))
    }

    /// Wall identity: exact `(osm_id, segment_idx)` without hashing.
    pub fn wall(osm_id: i64, segment_idx: i16) -> Result<Self, SourceIdError> {
        if !(0..WALL_OSM_ID_LIMIT).contains(&osm_id) {
            return Err(SourceIdError::WallOsmIdOutOfRange);
        }
        let segment_bits = segment_idx as u16 as u64;
        Ok(Self(
            WALL_SOURCE_TAG | ((osm_id as u64) << 16) | segment_bits,
        ))
    }

    /// Exact transport bits. They may form a NaN when carried in an f64 lane.
    pub const fn bits(self) -> u64 {
        self.0
    }

    /// Reconstruct a typed ID after bit-preserving device/ABI transport.
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    pub const fn is_wall(self) -> bool {
        (self.0 & WALL_SOURCE_TAG) != 0
    }

    pub const fn as_f64_bits(self) -> f64 {
        f64::from_bits(self.0)
    }
}

/// Receiver-centred metric vector `(east, north)` in metres.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct MetricVector {
    pub x: f64,
    pub y: f64,
}

impl MetricVector {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }

    fn is_zero(self) -> bool {
        self.x == 0.0 && self.y == 0.0
    }
}

/// Hard geometry failures cannot produce an accepted tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeometryError {
    NonFinite,
    AmbiguousDirection,
}

/// Candidate/node angular ownership including the fail-safe direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum WedgeDecision {
    /// The node direction is outside this candidate's half-open wedge.
    DoesNotOwn = 0,
    /// The node direction is inside this candidate's half-open wedge.
    Owns = 1,
    /// Legal receiver-touch/crossing geometry below the 1 m range guard.
    NearGuardedDegenerate = 2,
    /// Non-finite or ambiguous geometry; no tile containing it is accepted.
    HardFault = 3,
}

/// Compute the one source-frame longitude scale packed for both lanes.
///
/// `mlon` is host-owned; CUDA consumes these exact bits and never calls cosine.
pub fn source_frame_mlon(start_lat: f64, end_lat: f64) -> Result<f64, GeometryError> {
    // SRM-MLON-1  Preserve the frozen midpoint expression order.
    if !start_lat.is_finite() || !end_lat.is_finite() {
        return Err(GeometryError::NonFinite);
    }
    let midpoint_rad = ((start_lat + end_lat) * 0.5).to_radians();
    let mlon = m_per_deg_lon(midpoint_rad);
    if mlon.is_finite() {
        Ok(mlon)
    } else {
        Err(GeometryError::NonFinite)
    }
}

/// Ordered two-dimensional orientation determinant.
#[inline]
pub fn orient(a: MetricVector, b: MetricVector) -> f64 {
    // SRM-ORIENT-1  RN(RN(ax*by) - RN(ay*bx)); no mul_add/reassociation.
    let positive = a.x * b.y;
    let negative = a.y * b.x;
    canonical_zero(positive - negative)
}

/// Ordered dot product used by the exact same/opposite-ray distinction.
#[inline]
pub fn dot(a: MetricVector, b: MetricVector) -> f64 {
    // SRM-DOT-1  RN(RN(ax*bx) + RN(ay*by)); no mul_add/reassociation.
    let x = a.x * b.x;
    let y = a.y * b.y;
    canonical_zero(x + y)
}

/// Whether two nonzero finite vectors name exactly the same ray.
pub fn same_ray(a: MetricVector, b: MetricVector) -> Result<bool, GeometryError> {
    if !a.is_finite() || !b.is_finite() {
        return Err(GeometryError::NonFinite);
    }
    if a.is_zero() || b.is_zero() {
        return Err(GeometryError::AmbiguousDirection);
    }
    Ok(orient(a, b) == 0.0 && dot(a, b) > 0.0)
}

/// Branch-exact circular order, upper half-plane first.
pub fn direction_less(a: MetricVector, b: MetricVector) -> Result<bool, GeometryError> {
    if !a.is_finite() || !b.is_finite() {
        return Err(GeometryError::NonFinite);
    }
    if a.is_zero() || b.is_zero() {
        return Err(GeometryError::AmbiguousDirection);
    }
    // SRM-DIRECTION-1  +x is in the upper half; -x is in the lower half.
    let upper = |v: MetricVector| v.y > 0.0 || (v.y == 0.0 && v.x >= 0.0);
    let a_upper = upper(a);
    let b_upper = upper(b);
    if a_upper != b_upper {
        return Ok(a_upper);
    }
    let turn = orient(a, b);
    if turn != 0.0 {
        return Ok(turn > 0.0);
    }
    Ok(false)
}

/// Half-open ownership of the shorter angular image of finite edge `a..b`.
pub fn candidate_wedge_owns(
    a: MetricVector,
    b: MetricVector,
    node: MetricVector,
    near_f32: f32,
) -> WedgeDecision {
    if !a.is_finite() || !b.is_finite() || !node.is_finite() || !near_f32.is_finite() {
        return WedgeDecision::HardFault;
    }
    if node.is_zero() {
        return WedgeDecision::HardFault;
    }
    if a.is_zero() || b.is_zero() {
        return if near_f32 < 1.0 {
            WedgeDecision::NearGuardedDegenerate
        } else {
            WedgeDecision::HardFault
        };
    }

    let turn = orient(a, b);
    if turn == 0.0 {
        let alignment = dot(a, b);
        if alignment > 0.0 {
            return match same_ray(a, node) {
                Ok(true) => WedgeDecision::Owns,
                Ok(false) => WedgeDecision::DoesNotOwn,
                Err(_) => WedgeDecision::HardFault,
            };
        }
        return if alignment < 0.0 && near_f32 < 1.0 {
            WedgeDecision::NearGuardedDegenerate
        } else {
            WedgeDecision::HardFault
        };
    }

    // SRM-WEDGE-1  The physical candidate owns endpoint a and excludes b.
    // For a clockwise edge the equivalent CCW interval is (b,a], so its two
    // endpoint decisions are the reverse of a normalised [b,a) interval.
    if turn < 0.0 {
        let (Ok(node_is_start), Ok(node_is_end)) = (same_ray(a, node), same_ray(b, node)) else {
            return WedgeDecision::HardFault;
        };
        if node_is_start {
            return WedgeDecision::Owns;
        }
        if node_is_end {
            return WedgeDecision::DoesNotOwn;
        }
    }
    let (start, end) = if turn > 0.0 { (a, b) } else { (b, a) };
    let Ok(start_before_end) = direction_less(start, end) else {
        return WedgeDecision::HardFault;
    };
    let (Ok(node_before_start), Ok(node_before_end)) =
        (direction_less(node, start), direction_less(node, end))
    else {
        return WedgeDecision::HardFault;
    };
    let owns = if start_before_end {
        !node_before_start && node_before_end
    } else {
        !node_before_start || node_before_end
    };
    if owns {
        WedgeDecision::Owns
    } else {
        WedgeDecision::DoesNotOwn
    }
}

/// Full finite-edge origin distance, rounded exactly once to f32.
pub fn origin_to_segment_distance_f32(
    a: MetricVector,
    b: MetricVector,
) -> Result<f32, GeometryError> {
    if !a.is_finite() || !b.is_finite() {
        return Err(GeometryError::NonFinite);
    }
    // SRM-NEAR-1  Clamp the closest point to the immutable full edge.
    let vx = b.x - a.x;
    let vy = b.y - a.y;
    let vv = vx * vx + vy * vy;
    let projection = -(a.x * vx + a.y * vy);
    let t = if vv > 0.0 {
        (projection / vv).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let qx = a.x + t * vx;
    let qy = a.y + t * vy;
    let distance = (qx * qx + qy * qy).sqrt();
    if !distance.is_finite() {
        return Err(GeometryError::NonFinite);
    }
    Ok(distance as f32)
}

/// Horizontal receiver range of one node in the receiver-centred pair frame.
///
/// This mask-affecting expression is mirrored by CUDA in step 3. Keeping it in
/// the shared decision module prevents the CPU reducer from growing a second
/// arithmetic authority.
pub fn node_horizontal_range_from_receiver(
    node_vector: MetricVector,
) -> Result<f64, GeometryError> {
    if !node_vector.is_finite() {
        return Err(GeometryError::NonFinite);
    }
    if node_vector.is_zero() {
        return Err(GeometryError::AmbiguousDirection);
    }
    // SRM-NODE-RANGE-1  RN(sqrt(RN(x*x) + RN(y*y))); no reassociation.
    let x_squared = node_vector.x * node_vector.x;
    let y_squared = node_vector.y * node_vector.y;
    let distance = (x_squared + y_squared).sqrt();
    if distance.is_finite() {
        Ok(distance)
    } else {
        Err(GeometryError::NonFinite)
    }
}

/// Physical radial hint root. Equality is owned by the mandatory foot boundary.
pub fn radial_range_root(d_perp: f64, near_f32: f32) -> Result<Option<f64>, GeometryError> {
    let b = f64::from(near_f32);
    if !d_perp.is_finite() || !b.is_finite() || d_perp < 0.0 {
        return Err(GeometryError::NonFinite);
    }
    // SRM-ROOT-1  Strict guard precedes the radicand and square root.
    if d_perp >= b {
        return Ok(None);
    }
    let radicand = b * b - d_perp * d_perp;
    if radicand < 0.0 || !radicand.is_finite() {
        return Err(GeometryError::NonFinite);
    }
    Ok(Some(radicand.sqrt()))
}

/// Exact source-granular range decision; subtraction may not be reassociated.
pub fn range_ordered(node_distance: f64, near_f32: f32) -> Result<bool, GeometryError> {
    let near = f64::from(near_f32);
    if !node_distance.is_finite() || !near.is_finite() {
        return Err(GeometryError::NonFinite);
    }
    // SRM-RANGE-1  Keep this as subtraction followed by comparison.
    Ok(near >= 1.0 && node_distance - near > 1.0)
}

/// Ascending finite-f64 key with signed zero canonicalised.
pub fn total_f64(value: f64) -> Result<u64, GeometryError> {
    if !value.is_finite() {
        return Err(GeometryError::NonFinite);
    }
    let bits = canonical_zero(value).to_bits();
    // SRM-TOTAL-1  Negative values reverse; non-negative values follow them.
    Ok(if (bits >> 63) != 0 {
        !bits
    } else {
        bits ^ (1_u64 << 63)
    })
}

#[inline]
fn canonical_zero(value: f64) -> f64 {
    if value == 0.0 {
        0.0
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::propagation::shared_math::{qm_atan2, qm_wrap_pi};

    #[test]
    fn source_mlon_bits_cover_zero_floor_high_latitude_and_antimeridian() {
        assert_eq!(
            source_frame_mlon(0.0, -0.0).unwrap().to_bits(),
            111_320.0f64.to_bits()
        );
        assert_eq!(
            source_frame_mlon(90.0, 90.0).unwrap().to_bits(),
            (111_320.0f64 * 0.01).to_bits()
        );
        assert_eq!(
            source_frame_mlon(80.0, 80.001).unwrap().to_bits(),
            source_frame_mlon(80.001, 80.0).unwrap().to_bits()
        );
        assert_eq!(
            source_frame_mlon(f64::NAN, 0.0),
            Err(GeometryError::NonFinite)
        );
    }

    #[test]
    fn source_id_namespaces_round_trip_without_numeric_f64_conversion() {
        for ordinal in [0, 1, WALL_SOURCE_TAG - 1] {
            let id = SourceId64::obstacle(ordinal).unwrap();
            assert!(!id.is_wall());
            assert_eq!(SourceId64::from_bits(id.as_f64_bits().to_bits()), id);
        }
        for (osm_id, segment_idx) in [
            (0, i16::MIN),
            (1, -1),
            (42, 0),
            (WALL_OSM_ID_LIMIT - 1, i16::MAX),
        ] {
            let id = SourceId64::wall(osm_id, segment_idx).unwrap();
            assert!(id.is_wall());
            assert_eq!(SourceId64::from_bits(id.as_f64_bits().to_bits()), id);
        }
    }

    #[test]
    fn source_id_rejects_invalid_wall_osm_ids() {
        assert_eq!(
            SourceId64::wall(-1, 0),
            Err(SourceIdError::WallOsmIdOutOfRange)
        );
        assert_eq!(
            SourceId64::wall(WALL_OSM_ID_LIMIT, 0),
            Err(SourceIdError::WallOsmIdOutOfRange)
        );
        assert_eq!(
            SourceId64::obstacle(WALL_SOURCE_TAG),
            Err(SourceIdError::ObstacleOrdinalOutOfRange)
        );
    }

    #[test]
    fn radial_same_ray_and_half_open_wedge_truth_table() {
        let east = MetricVector::new(1.0, 0.0);
        let north = MetricVector::new(0.0, 1.0);
        let west = MetricVector::new(-1.0, 0.0);
        let south = MetricVector::new(0.0, -1.0);
        assert!(same_ray(east, MetricVector::new(2.0, -0.0)).unwrap());
        assert!(!same_ray(east, west).unwrap());
        assert_eq!(
            candidate_wedge_owns(east, north, east, 2.0),
            WedgeDecision::Owns
        );
        assert_eq!(
            candidate_wedge_owns(east, north, north, 2.0),
            WedgeDecision::DoesNotOwn
        );
        assert_eq!(
            candidate_wedge_owns(east, north, west, 2.0),
            WedgeDecision::DoesNotOwn
        );
        assert_eq!(
            candidate_wedge_owns(south, east, south, 2.0),
            WedgeDecision::Owns
        );
        assert_eq!(
            candidate_wedge_owns(south, east, east, 2.0),
            WedgeDecision::DoesNotOwn
        );
        assert_eq!(
            candidate_wedge_owns(east, north, north, 2.0),
            WedgeDecision::DoesNotOwn,
            "the closing ray belongs to the next half-open wedge"
        );
        assert_eq!(
            candidate_wedge_owns(north, west, north, 2.0),
            WedgeDecision::Owns,
            "the next half-open wedge owns its opening ray"
        );
        assert_eq!(
            candidate_wedge_owns(south, east, MetricVector::new(1.0, -0.0), 2.0),
            WedgeDecision::DoesNotOwn,
            "a seam-crossing wedge still excludes its closing +x ray"
        );
        assert_eq!(
            candidate_wedge_owns(east, west, north, 0.5),
            WedgeDecision::NearGuardedDegenerate
        );
        assert_eq!(
            candidate_wedge_owns(MetricVector::new(0.0, 0.0), east, east, 2.0),
            WedgeDecision::HardFault
        );
        assert_eq!(
            candidate_wedge_owns(MetricVector::new(0.0, 0.0), east, east, 0.5),
            WedgeDecision::NearGuardedDegenerate
        );

        let spike_tip = MetricVector::new(1.0, 1.0);
        let spike_return = MetricVector::new(1.0, 0.5);
        assert_eq!(
            candidate_wedge_owns(east, spike_tip, spike_tip, 2.0),
            WedgeDecision::DoesNotOwn,
            "the counter-clockwise edge excludes its end"
        );
        assert_eq!(
            candidate_wedge_owns(spike_tip, spike_return, spike_tip, 2.0),
            WedgeDecision::Owns,
            "the clockwise neighbour owns its physical start"
        );
    }

    #[test]
    fn direction_and_wedge_decisions_survive_order_and_duplicate_permutations() {
        let directions = [
            MetricVector::new(1.0, 0.0),
            MetricVector::new(1.0, 1.0),
            MetricVector::new(0.0, 1.0),
            MetricVector::new(-1.0, 1.0),
            MetricVector::new(-1.0, 0.0),
            MetricVector::new(-1.0, -1.0),
            MetricVector::new(0.0, -1.0),
            MetricVector::new(1.0, -1.0),
        ];
        for (left_index, left) in directions.iter().copied().enumerate() {
            for (right_index, right) in directions.iter().copied().enumerate() {
                assert_eq!(
                    direction_less(left, right).unwrap(),
                    left_index < right_index,
                    "circular order differs at {left_index},{right_index}"
                );
            }
        }

        let east = directions[0];
        let north = directions[2];
        let west = directions[4];
        let candidates = [
            (SourceId64::obstacle(9).unwrap(), east, north),
            (SourceId64::obstacle(3).unwrap(), north, west),
            (SourceId64::obstacle(9).unwrap(), east, north),
        ];
        let collect = |order: &[usize]| {
            let mut decisions = std::collections::BTreeMap::new();
            for &index in order {
                let (source_id, a, b) = candidates[index];
                let decision = candidate_wedge_owns(a, b, north, 2.0);
                if let Some(previous) = decisions.insert(source_id, decision) {
                    assert_eq!(previous, decision, "duplicate source changed meaning");
                }
            }
            decisions.into_iter().collect::<Vec<_>>()
        };
        assert_eq!(collect(&[0, 1, 2]), collect(&[2, 0, 1]));
        assert_eq!(collect(&[0, 1, 2]), collect(&[1, 2, 0]));
    }

    #[test]
    fn hard_geometry_faults_reject_nonfinite_and_ambiguous_inputs() {
        let east = MetricVector::new(1.0, 0.0);
        let zero = MetricVector::new(0.0, -0.0);
        let nonfinite = MetricVector::new(f64::INFINITY, 0.0);
        assert_eq!(same_ray(zero, east), Err(GeometryError::AmbiguousDirection));
        assert_eq!(
            direction_less(east, zero),
            Err(GeometryError::AmbiguousDirection)
        );
        assert_eq!(same_ray(nonfinite, east), Err(GeometryError::NonFinite));
        assert_eq!(
            candidate_wedge_owns(nonfinite, east, east, 2.0),
            WedgeDecision::HardFault
        );
        assert_eq!(
            origin_to_segment_distance_f32(nonfinite, east),
            Err(GeometryError::NonFinite)
        );
        assert_eq!(
            node_horizontal_range_from_receiver(nonfinite),
            Err(GeometryError::NonFinite)
        );
        assert_eq!(
            node_horizontal_range_from_receiver(zero),
            Err(GeometryError::AmbiguousDirection)
        );
        assert_eq!(
            radial_range_root(f64::NAN, 2.0),
            Err(GeometryError::NonFinite)
        );
        assert_eq!(
            range_ordered(f64::INFINITY, 2.0),
            Err(GeometryError::NonFinite)
        );
    }

    #[test]
    fn origin_to_segment_near_rounds_once_to_f32() {
        let a = MetricVector::new(3.0, 4.0);
        let b = MetricVector::new(6.0, 8.0);
        assert_eq!(
            origin_to_segment_distance_f32(a, b).unwrap().to_bits(),
            5.0f32.to_bits()
        );
        assert_eq!(
            origin_to_segment_distance_f32(
                MetricVector::new(-1.0, 2.0),
                MetricVector::new(1.0, 2.0)
            )
            .unwrap()
            .to_bits(),
            2.0f32.to_bits()
        );
        assert_eq!(
            origin_to_segment_distance_f32(a, a).unwrap().to_bits(),
            5.0f32.to_bits()
        );
        let lo = 1.0f32;
        let hi = lo.next_up();
        let midpoint = (f64::from(lo) + f64::from(hi)) * 0.5;
        assert_eq!(
            origin_to_segment_distance_f32(
                MetricVector::new(midpoint, 0.0),
                MetricVector::new(midpoint, 0.0)
            )
            .unwrap()
            .to_bits(),
            lo.to_bits(),
            "ties-to-even owns the exact f32 midpoint"
        );
        assert_eq!(
            origin_to_segment_distance_f32(
                MetricVector::new(midpoint.next_up(), 0.0),
                MetricVector::new(midpoint.next_up(), 0.0)
            )
            .unwrap()
            .to_bits(),
            hi.to_bits(),
            "one f64 ULP above the midpoint rounds upward"
        );
    }

    #[test]
    fn range_root_strict_domain_covers_equal_and_adjacent_ulps() {
        let b = 2.0f32;
        assert!(radial_range_root(2.0, b).unwrap().is_none());
        assert!(radial_range_root(f64::from(b).next_up(), b)
            .unwrap()
            .is_none());
        assert!(radial_range_root(f64::from(b).next_down(), b)
            .unwrap()
            .is_some());
    }

    #[test]
    fn admission_subtraction_is_not_reassociated() {
        let near = 2.0f32;
        assert!(!range_ordered(3.0, near).unwrap());
        assert!(range_ordered(3.0f64.next_up(), near).unwrap());
        assert!(!range_ordered(3.0f64.next_down(), near).unwrap());
        assert!(!range_ordered(100.0, 1.0f32.next_down()).unwrap());
    }

    #[test]
    fn total_f64_orders_every_finite_class_and_canonicalises_signed_zero() {
        let values = [f64::MIN, -1.0, -0.0, 0.0, 1.0, f64::MAX];
        let keys: Vec<_> = values.into_iter().map(|v| total_f64(v).unwrap()).collect();
        assert!(keys.windows(2).all(|pair| pair[0] <= pair[1]));
        assert_eq!(keys[2], keys[3]);
        assert_eq!(total_f64(f64::NAN), Err(GeometryError::NonFinite));
    }

    const STREAMING_INPUT_WORDS: usize = 11;
    const STREAMING_OUTPUT_WORDS: usize = 10;
    const STREAMING_RECORD_WORDS: usize = STREAMING_INPUT_WORDS + STREAMING_OUTPUT_WORDS;

    fn parity_record(input: [u64; STREAMING_INPUT_WORDS]) -> [u64; STREAMING_RECORD_WORDS] {
        let value = |index: usize| f64::from_bits(input[index]);
        let a = MetricVector::new(value(0), value(1));
        let b = MetricVector::new(value(2), value(3));
        let node = MetricVector::new(value(4), value(5));
        let d_perp = value(6);
        let node_distance = value(7);
        let osm_id = input[8] as i64;
        let segment_idx = input[9] as i16;

        let atan2_bits = qm_atan2(node.y, node.x).to_bits();
        let wrap_bits = qm_wrap_pi(qm_atan2(b.y, b.x) - qm_atan2(a.y, a.x)).to_bits();
        let orient_bits = orient(a, b).to_bits();
        let dot_bits = dot(a, b).to_bits();
        let near = origin_to_segment_distance_f32(a, b);
        let wedge = candidate_wedge_owns(a, b, node, near.unwrap_or(f32::NAN));

        let mut flags = (wedge as u64) << 4;
        match same_ray(a, b) {
            Ok(value) => flags |= u64::from(value),
            Err(_) => flags |= 1 << 1,
        }
        match direction_less(a, b) {
            Ok(value) => flags |= u64::from(value) << 2,
            Err(_) => flags |= 1 << 3,
        }
        if near.is_ok() {
            flags |= 1 << 6;
        }
        let near_value = near.unwrap_or(f32::NAN);
        let root = radial_range_root(d_perp, near_value);
        let root_bits = match root {
            Ok(Some(value)) => {
                flags |= 1 << 7;
                value.to_bits()
            }
            Ok(None) => 0,
            Err(_) => {
                flags |= 1 << 8;
                0
            }
        };
        match range_ordered(node_distance, near_value) {
            Ok(value) => flags |= u64::from(value) << 9,
            Err(_) => flags |= 1 << 10,
        }
        let total_key = match total_f64(node_distance) {
            Ok(value) => {
                flags |= 1 << 11;
                value
            }
            Err(_) => 0,
        };
        let source_id = match SourceId64::wall(osm_id, segment_idx) {
            Ok(value) => {
                flags |= 1 << 12;
                value.bits()
            }
            Err(_) => 0,
        };

        let mut record = [0_u64; STREAMING_RECORD_WORDS];
        record[..STREAMING_INPUT_WORDS].copy_from_slice(&input);
        record[STREAMING_INPUT_WORDS..].copy_from_slice(&[
            atan2_bits,
            wrap_bits,
            orient_bits,
            dot_bits,
            flags,
            u64::from(near_value.to_bits()),
            f64::from(near_value).to_bits(),
            root_bits,
            total_key,
            source_id,
        ]);
        record
    }

    /// Produce the exact Rust authority consumed by both host-C and sm_120
    /// device mirrors. Off by default; the sealed GPU runner sets the path.
    #[test]
    fn cross_lane_streaming_decision_dump() {
        let Ok(path) = std::env::var("QM_STREAMING_REDUCTION_DUMP") else {
            return;
        };

        struct Lcg(u64);
        impl Lcg {
            fn next(&mut self) -> u64 {
                self.0 = self
                    .0
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                self.0
            }
            fn finite(&mut self) -> f64 {
                let unit = (self.next() >> 11) as f64 * (1.0 / 9_007_199_254_740_992.0);
                (unit * 2.0 - 1.0) * 20_000.0
            }
        }

        let explicit_record =
            |geometry: [f64; 8], osm_id: i64, segment_idx: i16, source_lats: [f64; 2]| {
                let mut record = [0_u64; STREAMING_INPUT_WORDS];
                for (slot, value) in record[..8].iter_mut().zip(geometry) {
                    *slot = value.to_bits();
                }
                record[8] = osm_id as u64;
                record[9] = u64::from(segment_idx as u16);
                record[10] = source_frame_mlon(source_lats[0], source_lats[1])
                    .unwrap()
                    .to_bits();
                record
            };
        let f32_lo = 1.0f32;
        let f32_hi = f32_lo.next_up();
        let f32_midpoint = (f64::from(f32_lo) + f64::from(f32_hi)) * 0.5;
        let tiny = f64::from_bits(1);
        let explicit = [
            // Every signed-zero atan2 axis combination.
            explicit_record([0.0, 0.0, 1.0, 0.0, 1.0, -0.0, 0.0, 1.0], 0, 0, [-0.0, 0.0]),
            explicit_record(
                [-0.0, 0.0, 1.0, 0.0, -1.0, 0.0, 0.0, 1.0],
                0,
                i16::MIN,
                [0.0, -0.0],
            ),
            explicit_record(
                [1.0, 0.0, 0.0, 1.0, -1.0, -0.0, 0.0, 1.0],
                1,
                -1,
                [0.0, 0.0],
            ),
            explicit_record(
                [1.0, 0.0, 0.0, 1.0, 0.0, -0.0, 0.0, 1.0],
                1,
                1,
                [-0.0, -0.0],
            ),
            // Four open quadrants and both exact +/-pi representations.
            explicit_record([1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 0.0, 2.0], 2, 2, [45.0, 45.0]),
            explicit_record(
                [1.0, 0.0, 0.0, 1.0, -1.0, 1.0, 0.0, 2.0],
                3,
                3,
                [45.0, 45.0],
            ),
            explicit_record(
                [1.0, 0.0, 0.0, 1.0, -1.0, -1.0, 0.0, 2.0],
                4,
                4,
                [-45.0, -45.0],
            ),
            explicit_record(
                [1.0, 0.0, 0.0, 1.0, 1.0, -1.0, 0.0, 2.0],
                5,
                5,
                [-45.0, -45.0],
            ),
            explicit_record(
                [-1.0, 0.0, -1.0, -0.0, -1.0, 0.0, 0.0, 2.0],
                6,
                6,
                [80.0, 80.001],
            ),
            explicit_record(
                [-1.0, -0.0, -1.0, 0.0, -1.0, -0.0, 0.0, 2.0],
                7,
                7,
                [-80.0, -80.001],
            ),
            // Values immediately on either side of the +/-pi seam.
            explicit_record(
                [-1.0, tiny, -1.0, 0.0, -1.0, tiny, 0.0, 2.0],
                8,
                8,
                [89.99, 89.99],
            ),
            explicit_record(
                [-1.0, -tiny, -1.0, -0.0, -1.0, -tiny, 0.0, 2.0],
                9,
                9,
                [-89.99, -89.99],
            ),
            // Same ray, opposite ray, zero endpoint and a seam-crossing wedge.
            explicit_record(
                [1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 1.0, 2.0],
                10,
                -1,
                [90.0, 90.0],
            ),
            explicit_record(
                [1.0, 0.0, -1.0, 0.0, 0.0, 1.0, 1.0, 3.0],
                11,
                -7,
                [80.0, 80.001],
            ),
            explicit_record(
                [1.0, 1.0, 2.0, 2.0, -1.0, -1.0, 1.0, 3.0],
                12,
                i16::MAX,
                [-89.5, -89.6],
            ),
            explicit_record(
                [0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 0.0, 3.0],
                13,
                i16::MIN,
                [0.0, 0.0],
            ),
            explicit_record(
                [0.0, -1.0, 1.0, 0.0, 1.0, -1.0, 0.0, 3.0],
                14,
                14,
                [80.0, 80.001],
            ),
            // Exact f32 midpoint and its next-f64 upper neighbour.
            explicit_record(
                [f32_midpoint, 0.0, f32_midpoint, 0.0, 1.0, 0.0, 1.0, 3.0],
                15,
                15,
                [0.0, 0.0],
            ),
            explicit_record(
                [
                    f32_midpoint.next_up(),
                    0.0,
                    f32_midpoint.next_up(),
                    0.0,
                    1.0,
                    0.0,
                    1.0,
                    3.0,
                ],
                16,
                16,
                [0.0, 0.0],
            ),
            // d_perp == b and adjacent f64 ULPs; d-near == 1 and both neighbours.
            explicit_record([2.0, 0.0, 2.0, 0.0, 1.0, 0.0, 2.0, 3.0], 17, 17, [0.0, 0.0]),
            explicit_record(
                [
                    2.0,
                    0.0,
                    2.0,
                    0.0,
                    1.0,
                    0.0,
                    2.0f64.next_down(),
                    3.0f64.next_down(),
                ],
                18,
                18,
                [0.0, 0.0],
            ),
            explicit_record(
                [
                    2.0,
                    0.0,
                    2.0,
                    0.0,
                    1.0,
                    0.0,
                    2.0f64.next_up(),
                    3.0f64.next_up(),
                ],
                19,
                19,
                [0.0, 0.0],
            ),
            // Last legal wall namespace and raw signed segment bits.
            explicit_record(
                [3.0, 4.0, 6.0, 8.0, 1.0, 1.0, 0.0, 7.0],
                WALL_OSM_ID_LIMIT - 1,
                i16::MIN,
                [90.0, 90.0],
            ),
            // Mixed-orientation shared vertex: first edge closes, second owns.
            explicit_record([1.0, 0.0, 1.0, 1.0, 1.0, 1.0, 0.0, 3.0], 30, 30, [0.0, 0.0]),
            explicit_record([1.0, 1.0, 1.0, 0.5, 1.0, 1.0, 0.0, 3.0], 31, 31, [0.0, 0.0]),
            // Fault direction: non-finite geometry, root/range operands and ids.
            explicit_record(
                [f64::NAN, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 2.0],
                20,
                20,
                [0.0, 0.0],
            ),
            explicit_record(
                [1.0, 0.0, 0.0, 1.0, f64::NAN, 0.0, 0.0, 2.0],
                21,
                21,
                [0.0, 0.0],
            ),
            explicit_record(
                [2.0, 0.0, 2.0, 0.0, 1.0, 0.0, f64::NAN, 3.0],
                22,
                22,
                [0.0, 0.0],
            ),
            explicit_record(
                [2.0, 0.0, 2.0, 0.0, 1.0, 0.0, -1.0, 3.0],
                23,
                23,
                [0.0, 0.0],
            ),
            explicit_record(
                [2.0, 0.0, 2.0, 0.0, 1.0, 0.0, 0.0, f64::NAN],
                24,
                24,
                [0.0, 0.0],
            ),
            // Signed total-order branch used by negative angular boundaries.
            explicit_record(
                [2.0, 0.0, 2.0, 0.0, 1.0, 0.0, 0.0, -2.0],
                25,
                25,
                [0.0, 0.0],
            ),
            explicit_record([2.0, 0.0, 2.0, 0.0, 1.0, 0.0, 0.0, 3.0], -1, 26, [0.0, 0.0]),
            explicit_record(
                [2.0, 0.0, 2.0, 0.0, 1.0, 0.0, 0.0, 3.0],
                WALL_OSM_ID_LIMIT,
                27,
                [0.0, 0.0],
            ),
        ];

        // This assertion is part of dump generation: the sealed device run
        // cannot accidentally regress to a finite-only, positive-only corpus.
        let explicit_records: Vec<_> = explicit.iter().copied().map(parity_record).collect();
        let flags: Vec<_> = explicit_records
            .iter()
            .map(|record| record[STREAMING_INPUT_WORDS + 4])
            .collect();
        for bit in [6_u32, 8, 10, 11, 12] {
            assert!(
                flags.iter().any(|value| value & (1_u64 << bit) == 0)
                    && flags.iter().any(|value| value & (1_u64 << bit) != 0),
                "parity corpus must cover both branches of flag bit {bit}"
            );
        }
        let wedge_decisions: std::collections::BTreeSet<_> =
            flags.iter().map(|value| (value >> 4) & 3).collect();
        assert_eq!(wedge_decisions, [0_u64, 1, 2, 3].into_iter().collect());
        let negative_total = explicit_records
            .iter()
            .find(|record| f64::from_bits(record[7]).is_sign_negative())
            .expect("explicit negative total_f64 operand");
        assert_eq!(
            negative_total[STREAMING_INPUT_WORDS + 8],
            !negative_total[7],
            "negative total_f64 branch must be in the device corpus"
        );

        let mut rng = Lcg(0x6a09_e667_f3bc_c909);
        let mut bytes = Vec::with_capacity(1_000_000 * STREAMING_RECORD_WORDS * 8);
        for index in 0..1_000_000 {
            let input = explicit.get(index).copied().unwrap_or_else(|| {
                let mut values = [0_u64; STREAMING_INPUT_WORDS];
                for slot in &mut values[..8] {
                    *slot = rng.finite().to_bits();
                }
                values[6] = rng.finite().abs().to_bits();
                values[7] = rng.finite().abs().to_bits();
                values[8] = rng.next() % WALL_OSM_ID_LIMIT as u64;
                values[9] = (rng.next() as i16) as u16 as u64;
                let source_start_lat = rng.finite() * (90.0 / 20_000.0);
                let source_end_lat = rng.finite() * (90.0 / 20_000.0);
                values[10] = source_frame_mlon(source_start_lat, source_end_lat)
                    .unwrap()
                    .to_bits();
                values
            });
            for word in parity_record(input) {
                bytes.extend_from_slice(&word.to_le_bytes());
            }
        }
        std::fs::write(&path, &bytes).expect("streaming dump path writable");
        println!("wrote {} streaming-decision bytes to {path}", bytes.len());
    }
}
