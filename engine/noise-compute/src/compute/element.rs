//! Closed-form V2 source-element decomposition and streamed line nodes.

use crate::constants::ALPHA_ATM;
use crate::propagation::shared_math::{qm_atan, qm_tan};

/// Frozen three-degree V3 arm and legacy hinted-generator dial.
pub const H0_V3_THETA_3_RAD: f64 = core::f64::consts::PI / 60.0;
#[cfg(not(feature = "h0-production-selection"))]
pub const THETA_MAX_RAD: f64 = H0_V3_THETA_3_RAD;
#[cfg(feature = "h0-production-selection")]
pub const THETA_MAX_RAD: f64 =
    f64::from_bits(crate::h0_production_selection_record::H0_PRODUCTION_THETA_RADIANS_BITS);
/// Legacy hinted generator's bounded refinement allowance; H0 admits no hints.
pub const H_MAX: usize = 32;
/// H0 node cap: ceil(pi/theta) + H_MAX(0) + floor(L_max/(theta R)) + 2.
#[cfg(not(feature = "h0-production-selection"))]
pub const H0_NODE_CAP: usize = 66;
#[cfg(feature = "h0-production-selection")]
pub const H0_NODE_CAP: usize = crate::h0_production_selection_record::H0_PRODUCTION_NODE_CAP;
const _: () = assert!(H0_NODE_CAP <= 128);
/// Converts angular cell width to distance on atmospheric-loss bands, whose
/// coefficients are expressed per kilometre.
pub const R_ATM_BASE_M_PER_RAD: f64 = 1_000.0;
/// Bands attenuated by more than 25 dB cannot control node placement.
pub const A_LIVE_DB: f64 = 25.0;
/// A 0.05 m road source plus half a 7 m two-lane carriageway.
pub const ROAD_D_FLOOR_M: f64 = 3.55;
/// A 0.5 m rail source plus half the standard 1.435 m gauge.
pub const RAIL_D_FLOOR_M: f64 = 1.2175;
/// Source loaders split longer road and rail rows before node generation.
pub const LINE_MAX_LENGTH_M: f64 = 250.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecompositionPolicy {
    Point,
    Area,
    Line(LineLayer),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineLayer {
    Road,
    Rail,
}

impl LineLayer {
    #[must_use]
    pub const fn d_floor_m(self) -> f64 {
        match self {
            Self::Road => ROAD_D_FLOOR_M,
            Self::Rail => RAIL_D_FLOOR_M,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct LinePiece {
    pub start_m: [f64; 2],
    pub end_m: [f64; 2],
    pub emission_per_m: f64,
}

#[derive(Debug, Clone, Copy)]
pub enum SkylineHintKind {
    Azimuth { relative_u_rad: f64 },
    Range { radius_m: f64 },
}

/// Canonical N-04c hint metadata. It refines nodes and never decides screening.
#[derive(Debug, Clone, Copy)]
pub struct SkylineHint {
    pub kind: SkylineHintKind,
    pub range_stratum: u16,
    pub start_rad: f64,
    pub end_rad: f64,
    pub source_id: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineNode {
    pub position_m: [f64; 2],
    /// Parameter on the original storage piece. The road/rail adapter maps it
    /// back to the geographic segment before its exact 3D path evaluation.
    pub piece_fraction: f64,
    pub weight: f64,
    pub placement_distance_m: f64,
    /// Signed abscissa from the perpendicular foot in the source frame.
    pub node_x_m: f64,
    pub arm: i8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineNodeError {
    DegeneratePiece,
    OverlengthPiece,
    NodeCapOverflow,
}

#[derive(Debug, Clone, Copy)]
struct Cell {
    near_s: f64,
    far_s: f64,
    arm: i8,
}

/// N-04c's total admission order. `u` is deliberately the final key: the
/// two physical range roots must consume separate slots deterministically.
#[derive(Debug, Clone, Copy)]
struct HintCandidate {
    range_stratum: u16,
    start_rad: f64,
    end_rad: f64,
    source_id: u64,
    u: f64,
}

impl HintCandidate {
    fn compare(self, other: Self) -> core::cmp::Ordering {
        self.range_stratum
            .cmp(&other.range_stratum)
            .then_with(|| self.start_rad.total_cmp(&other.start_rad))
            .then_with(|| self.end_rad.total_cmp(&other.end_rad))
            .then_with(|| self.source_id.cmp(&other.source_id))
            .then_with(|| self.u.total_cmp(&other.u))
    }
}

/// N-02 through N-09, with bounded cell state and streamed nodes.
pub struct LineNodeIterator {
    foot_m: [f64; 2],
    direction: [f64; 2],
    piece_length_m: f64,
    foot_from_start_m: f64,
    h_m: f64,
    emission_per_m: f64,
    theta_max_rad: f64,
    cells: Vec<Cell>,
    cell_index: usize,
    current_s: Option<f64>,
    hard_cap: usize,
    emitted: usize,
    hard_fault_count: u32,
    hint_drop_count: u32,
}

impl LineNodeIterator {
    /// Frozen H0 generator: equal-u cells only, with no skyline hints.
    pub fn new_h0(
        piece: LinePiece,
        receiver_m: [f64; 2],
        layer: LineLayer,
    ) -> Result<Self, LineNodeError> {
        let iterator = Self::with_dials(
            piece,
            receiver_m,
            layer.d_floor_m(),
            THETA_MAX_RAD,
            0,
            &[],
            None,
        )?;
        assert_eq!(
            iterator.hard_cap, H0_NODE_CAP,
            "the selected H0 theta and theorem cap must stay paired"
        );
        Ok(iterator)
    }

    pub fn new(
        piece: LinePiece,
        receiver_m: [f64; 2],
        layer: LineLayer,
        hints: &[SkylineHint],
    ) -> Result<Self, LineNodeError> {
        let iterator = Self::with_dials(
            piece,
            receiver_m,
            layer.d_floor_m(),
            H0_V3_THETA_3_RAD,
            H_MAX,
            hints,
            None,
        )?;
        assert_eq!(
            iterator.hard_cap, 98,
            "the pinned production constants must derive the N-09 cap"
        );
        Ok(iterator)
    }

    #[cfg(test)]
    fn with_test_cap(
        piece: LinePiece,
        receiver_m: [f64; 2],
        layer: LineLayer,
        hints: &[SkylineHint],
        cap: usize,
    ) -> Result<Self, LineNodeError> {
        Self::with_dials(
            piece,
            receiver_m,
            layer.d_floor_m(),
            H0_V3_THETA_3_RAD,
            H_MAX,
            hints,
            Some(cap),
        )
    }

    fn with_dials(
        piece: LinePiece,
        receiver_m: [f64; 2],
        d_floor_m: f64,
        theta_max_rad: f64,
        h_max: usize,
        hints: &[SkylineHint],
        cap_override: Option<usize>,
    ) -> Result<Self, LineNodeError> {
        let d = [
            piece.end_m[0] - piece.start_m[0],
            piece.end_m[1] - piece.start_m[1],
        ];
        let length = (d[0] * d[0] + d[1] * d[1]).sqrt();
        if length == 0.0 {
            return Err(LineNodeError::DegeneratePiece);
        }
        if length > LINE_MAX_LENGTH_M {
            return Err(LineNodeError::OverlengthPiece);
        }
        let direction = [d[0] / length, d[1] / length];
        let r = [
            receiver_m[0] - piece.start_m[0],
            receiver_m[1] - piece.start_m[1],
        ];
        let foot_from_start = r[0] * direction[0] + r[1] * direction[1];
        let foot_m = [
            piece.start_m[0] + foot_from_start * direction[0],
            piece.start_m[1] + foot_from_start * direction[1],
        ];
        let offset = [receiver_m[0] - foot_m[0], receiver_m[1] - foot_m[1]];
        let d_perp = (offset[0] * offset[0] + offset[1] * offset[1]).sqrt();
        let h_m = d_perp.max(d_floor_m);
        let low_s = (-foot_from_start).min(length - foot_from_start);
        let high_s = (-foot_from_start).max(length - foot_from_start);
        let low_u = qm_atan(low_s / h_m);
        let high_u = qm_atan(high_s / h_m);
        let max_range = (d_perp * d_perp + low_s.abs().max(high_s.abs()).powi(2)).sqrt();

        // The sparse skyline can contain hundreds of thousands of candidates.
        // N-04c permits only H_MAX admitted boundaries, so retaining all of
        // them before sorting would make the proof's bounded admission false.
        let mut candidates = Vec::with_capacity(h_max);
        let mut hint_drop_count = 0;
        for hint in hints {
            match hint.kind {
                SkylineHintKind::Azimuth { relative_u_rad } if d_perp >= d_floor_m => {
                    Self::admit_hint(
                        &mut candidates,
                        h_max,
                        &mut hint_drop_count,
                        *hint,
                        relative_u_rad,
                        low_u,
                        high_u,
                    );
                }
                SkylineHintKind::Range { radius_m }
                    if d_perp < d_floor_m && d_perp < radius_m && radius_m <= max_range =>
                {
                    let root = (radius_m * radius_m - d_perp * d_perp).sqrt();
                    Self::admit_hint(
                        &mut candidates,
                        h_max,
                        &mut hint_drop_count,
                        *hint,
                        qm_atan(-root / h_m),
                        low_u,
                        high_u,
                    );
                    Self::admit_hint(
                        &mut candidates,
                        h_max,
                        &mut hint_drop_count,
                        *hint,
                        qm_atan(root / h_m),
                        low_u,
                        high_u,
                    );
                }
                SkylineHintKind::Range { .. } => {}
                SkylineHintKind::Azimuth { .. } => {}
            }
        }
        candidates.sort_unstable_by(|a, b| a.compare(*b));
        let mut bounds = vec![low_u, high_u];
        if low_s < 0.0 && 0.0 < high_s {
            bounds.push(0.0);
        }
        bounds.extend(candidates.into_iter().map(|entry| entry.u));
        bounds.sort_unstable_by(f64::total_cmp);
        bounds.dedup_by(|a, b| *a == *b);

        let mut cells = Vec::new();
        for pair in bounds.windows(2) {
            let n = ((pair[1] - pair[0]) / theta_max_rad).ceil().max(1.0) as usize;
            for i in 0..n {
                let u0 = pair[0] + (pair[1] - pair[0]) * i as f64 / n as f64;
                let u1 = pair[0] + (pair[1] - pair[0]) * (i + 1) as f64 / n as f64;
                let s0 = if i == 0 && u0 == low_u {
                    low_s
                } else {
                    h_m * qm_tan(u0)
                };
                let s1 = if i + 1 == n && u1 == high_u {
                    high_s
                } else {
                    h_m * qm_tan(u1)
                };
                let arm = if (s0 + s1) * 0.5 < 0.0 { -1 } else { 1 };
                let (near_s, far_s) = if arm < 0 { (s1, s0) } else { (s0, s1) };
                cells.push(Cell { near_s, far_s, arm });
            }
        }
        let derived = derived_node_cap(theta_max_rad, h_max, LINE_MAX_LENGTH_M);
        Ok(Self {
            foot_m,
            direction,
            piece_length_m: length,
            foot_from_start_m: foot_from_start,
            h_m,
            emission_per_m: piece.emission_per_m,
            theta_max_rad,
            cells,
            cell_index: 0,
            current_s: None,
            hard_cap: cap_override.unwrap_or(derived),
            emitted: 0,
            hard_fault_count: 0,
            hint_drop_count,
        })
    }

    fn admit_hint(
        admitted: &mut Vec<HintCandidate>,
        h_max: usize,
        hint_drop_count: &mut u32,
        hint: SkylineHint,
        u: f64,
        low_u: f64,
        high_u: f64,
    ) {
        if !(low_u < u && u < high_u) {
            return;
        }
        let candidate = HintCandidate {
            range_stratum: hint.range_stratum,
            start_rad: hint.start_rad,
            end_rad: hint.end_rad,
            source_id: hint.source_id,
            u,
        };
        if h_max == 0 {
            *hint_drop_count += 1;
            return;
        }
        if admitted.len() < h_max {
            admitted.push(candidate);
            return;
        }
        let (worst_index, worst) = admitted
            .iter()
            .copied()
            .enumerate()
            .max_by(|(_, left), (_, right)| left.compare(*right))
            .expect("H_MAX > 0 is required by the node contract");
        if candidate.compare(worst).is_lt() {
            admitted[worst_index] = candidate;
        }
        *hint_drop_count += 1;
    }

    #[must_use]
    pub fn hard_fault_count(&self) -> u32 {
        self.hard_fault_count
    }

    /// N-04c soft diagnostic: omitted hints reduce only local refinement.
    #[must_use]
    pub fn hint_drop_count(&self) -> u32 {
        self.hint_drop_count
    }

    /// The iterator remains streamable, while production callers can make the
    /// N-09 cap channel fail-closed instead of treating a truncated sequence as
    /// a valid empty tail.
    pub fn next_checked(&mut self) -> Result<Option<LineNode>, LineNodeError> {
        let node = self.next();
        if node.is_none() && self.hard_fault_count != 0 {
            Err(LineNodeError::NodeCapOverflow)
        } else {
            Ok(node)
        }
    }

    #[cfg(test)]
    fn boundaries(&self) -> Vec<f64> {
        let mut result: Vec<_> = self
            .cells
            .iter()
            .flat_map(|cell| {
                [
                    qm_atan(cell.near_s / self.h_m),
                    qm_atan(cell.far_s / self.h_m),
                ]
            })
            .collect();
        result.sort_unstable_by(f64::total_cmp);
        result.dedup_by(|a, b| *a == *b);
        result
    }
}

impl Iterator for LineNodeIterator {
    type Item = LineNode;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let cell = *self.cells.get(self.cell_index)?;
            let current_s = self.current_s.unwrap_or(cell.near_s);
            if current_s == cell.far_s {
                self.cell_index += 1;
                self.current_s = None;
                continue;
            }
            if self.emitted >= self.hard_cap {
                self.hard_fault_count += 1;
                self.cell_index = self.cells.len();
                return None;
            }
            let current_d = (self.h_m * self.h_m + current_s * current_s).sqrt();
            let far_d = (self.h_m * self.h_m + cell.far_s * cell.far_s).sqrt();
            let alpha_live = ALPHA_ATM
                .into_iter()
                .filter(|alpha| *alpha * current_d / 1000.0 <= A_LIVE_DB)
                .fold(0.0_f64, f64::max);
            let step = self.theta_max_rad * R_ATM_BASE_M_PER_RAD * ALPHA_ATM[7] / alpha_live;
            let next_d = if far_d - current_d > step {
                current_d + step
            } else {
                far_d
            };
            let next_s = if next_d == far_d {
                cell.far_s
            } else {
                cell.arm as f64 * (next_d * next_d - self.h_m * self.h_m).sqrt()
            };
            self.current_s = Some(next_s);
            let (low_s, high_s) = if cell.arm < 0 {
                (next_s, current_s)
            } else {
                (current_s, next_s)
            };
            let length = high_s - low_s;
            let delta_u = qm_atan(high_s / self.h_m) - qm_atan(low_s / self.h_m);
            let d2 = length * self.h_m / delta_u;
            let node_s = cell.arm as f64 * (d2 - self.h_m * self.h_m).max(0.0).sqrt();
            self.emitted += 1;
            return Some(LineNode {
                position_m: [
                    self.foot_m[0] + node_s * self.direction[0],
                    self.foot_m[1] + node_s * self.direction[1],
                ],
                piece_fraction: (self.foot_from_start_m + node_s) / self.piece_length_m,
                weight: self.emission_per_m * length,
                placement_distance_m: d2.sqrt(),
                node_x_m: node_s,
                arm: cell.arm,
            });
        }
    }
}

#[must_use]
pub fn derived_node_cap(theta_max_rad: f64, h_max: usize, line_max_length_m: f64) -> usize {
    (core::f64::consts::PI / theta_max_rad).ceil() as usize
        + h_max
        + (line_max_length_m / (theta_max_rad * R_ATM_BASE_M_PER_RAD)).floor() as usize
        + 2
}

#[cfg(test)]
mod tests {
    use super::*;
    fn piece(a: [f64; 2], b: [f64; 2]) -> LinePiece {
        LinePiece {
            start_m: a,
            end_m: b,
            emission_per_m: 1.0,
        }
    }
    fn sum(nodes: impl Iterator<Item = LineNode>) -> f64 {
        nodes
            .map(|node| node.weight / node.placement_distance_m.powi(2))
            .sum()
    }

    fn db_ratio(numerator: f64, denominator: f64) -> f64 {
        10.0 * (numerator / denominator).log10()
    }

    fn exact_x_axis_integral(start_x: f64, end_x: f64, receiver_x: f64, lateral_m: f64) -> f64 {
        (qm_atan((end_x - receiver_x) / lateral_m) - qm_atan((start_x - receiver_x) / lateral_m))
            / lateral_m
    }

    fn evaluation_sum(nodes: impl Iterator<Item = LineNode>, receiver: [f64; 2], dz_m: f64) -> f64 {
        nodes
            .map(|node| {
                let dx = node.position_m[0] - receiver[0];
                let dy = node.position_m[1] - receiver[1];
                node.weight / (dx * dx + dy * dy + dz_m * dz_m)
            })
            .sum()
    }

    #[test]
    fn free_field_identity_b1_b5() {
        for (line, receiver) in [
            (piece([-125.0, 0.0], [125.0, 0.0]), [0.0, 50.0]),
            (piece([0.0, 0.0], [250.0, 0.0]), [500.0, 30.0]),
            (piece([-125.0, 0.0], [125.0, 0.0]), [0.0, 0.0]),
            (piece([0.0, 0.0], [250.0, 0.0]), [0.0, 1.0]),
            (piece([-100.0, -50.0], [100.0, 50.0]), [40.0, 90.0]),
        ] {
            let dx = line.end_m[0] - line.start_m[0];
            let dy = line.end_m[1] - line.start_m[1];
            let length = (dx * dx + dy * dy).sqrt();
            let unit = [dx / length, dy / length];
            let r = [receiver[0] - line.start_m[0], receiver[1] - line.start_m[1]];
            let foot = r[0] * unit[0] + r[1] * unit[1];
            let point = [
                line.start_m[0] + foot * unit[0],
                line.start_m[1] + foot * unit[1],
            ];
            let d = ((receiver[0] - point[0]).powi(2) + (receiver[1] - point[1]).powi(2)).sqrt();
            let h = d.max(ROAD_D_FLOOR_M);
            let exact = (qm_atan((length - foot) / h) - qm_atan(-foot / h)) / h;
            let got = sum(LineNodeIterator::new(line, receiver, LineLayer::Road, &[]).unwrap());
            assert!((got - exact).abs() <= 3e-13, "{got:e} vs {exact:e}");
        }
    }

    /// V0 B1/B2: exact line energy is independent of the equal-u mesh density.
    #[test]
    fn v0_free_field_matrix_b1_b2() {
        let geometries: [(LinePiece, [f64; 2]); 9] = [
            (piece([-125.0, 0.0], [125.0, 0.0]), [0.0, 5.0]),
            (piece([-125.0, 0.0], [125.0, 0.0]), [0.0, 30.0]),
            (piece([-125.0, 0.0], [125.0, 0.0]), [0.0, 50.0]),
            (piece([-125.0, 0.0], [125.0, 0.0]), [0.0, 500.0]),
            (piece([-125.0, 0.0], [125.0, 0.0]), [0.0, 5000.0]),
            (piece([-62.5, 0.0], [187.5, 0.0]), [0.0, 45.0]),
            (piece([0.0, 0.0], [250.0, 0.0]), [0.0, 45.0]),
            (piece([250.0, 0.0], [500.0, 0.0]), [0.0, 45.0]),
            (piece([2000.0, 0.0], [2250.0, 0.0]), [0.0, 45.0]),
        ];
        let meshes: [f64; 6] = [30.0, 10.0, 3.0, 1.0, 0.5, 0.1];
        for (line, receiver) in geometries {
            let lateral = receiver[1].max(ROAD_D_FLOOR_M);
            let exact = exact_x_axis_integral(line.start_m[0], line.end_m[0], receiver[0], lateral);
            let mut levels = Vec::new();
            for degrees in meshes {
                let nodes = LineNodeIterator::with_dials(
                    line,
                    receiver,
                    ROAD_D_FLOOR_M,
                    degrees.to_radians(),
                    0,
                    &[],
                    None,
                )
                .unwrap();
                let got = sum(nodes);
                assert!(
                    db_ratio(got, exact).abs() <= 1e-9,
                    "{degrees} degrees: {got:e} vs {exact:e}"
                );
                levels.push(10.0 * got.log10());
            }
            let min = levels.iter().copied().fold(f64::INFINITY, f64::min);
            let max = levels.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            assert!(max - min <= 1e-9, "mesh spread: {} dB", max - min);
        }
    }

    /// V0 B3: placement uses the floored lever, evaluation retains exact 3D.
    #[test]
    fn v0_placement_metric_is_bounded_against_exact_three_d() {
        let geometries: [(LinePiece, [f64; 2]); 6] = [
            (piece([-125.0, 0.0], [125.0, 0.0]), [0.0, 5.0]),
            (piece([-125.0, 0.0], [125.0, 0.0]), [0.0, 25.0]),
            (piece([-125.0, 0.0], [125.0, 0.0]), [0.0, 125.0]),
            (piece([-125.0, 0.0], [125.0, 0.0]), [0.0, 1000.0]),
            (piece([250.0, 0.0], [500.0, 0.0]), [0.0, 5.0]),
            (piece([250.0, 0.0], [500.0, 0.0]), [0.0, 45.0]),
        ];
        for (line, receiver) in geometries {
            let effective_lateral = (receiver[1] * receiver[1] + 3.95_f64.powi(2)).sqrt();
            let exact = exact_x_axis_integral(
                line.start_m[0],
                line.end_m[0],
                receiver[0],
                effective_lateral,
            );
            for degrees in [3.0_f64, 1.0] {
                let nodes = LineNodeIterator::with_dials(
                    line,
                    receiver,
                    ROAD_D_FLOOR_M,
                    degrees.to_radians(),
                    0,
                    &[],
                    None,
                )
                .unwrap();
                let got = evaluation_sum(nodes, receiver, -3.95);
                assert!(
                    db_ratio(got, exact).abs() <= 0.005,
                    "{degrees} degrees: {} dB",
                    db_ratio(got, exact)
                );
            }
        }
    }

    /// V0 B4: freeze the old six-term line form without calling `geo.rs`.
    #[test]
    fn v0_normalization_shift_is_one_point_nine_five_three_three_db() {
        let line = piece([-125.0, 0.0], [125.0, 0.0]);
        let receiver = [0.0, 1000.0];
        let node_energy = 10f64.powf(-1.1)
            * sum(LineNodeIterator::new(line, receiver, LineLayer::Road, &[]).unwrap());
        let theta = qm_atan(125.0 / 1000.0) - qm_atan(-125.0 / 1000.0);
        let d_div = 1000.0_f64;
        let d_perp = 1000.0_f64;
        let d_slant = 1000.0_f64;
        let today_line_form = 10f64.powf(
            (10.0 * (theta / core::f64::consts::PI).log10() + 10.0 * (d_div / d_perp).log10()
                - 10.0 * (2.0 * core::f64::consts::PI * d_slant).log10())
                / 10.0,
        );
        assert!(
            (db_ratio(node_energy, today_line_form) - 1.9533).abs() <= 0.0010,
            "shift={:.7} dB",
            db_ratio(node_energy, today_line_form)
        );
    }

    /// V0 B5: radial/end-on evaluation never relies on a perpendicular floor.
    #[test]
    fn v0_end_on_piece_is_independent_of_placement_floor() {
        let line = piece([500.0, 0.0], [750.0, 0.0]);
        let receiver = [0.0, 0.0];
        let exact = 1.0 / 500.0 - 1.0 / 750.0;
        for d_floor_m in [0.5_f64, 1.0, 2.0, 5.0] {
            for degrees in [3.0_f64, 0.75, 0.1] {
                let nodes = LineNodeIterator::with_dials(
                    line,
                    receiver,
                    d_floor_m,
                    degrees.to_radians(),
                    0,
                    &[],
                    None,
                )
                .unwrap();
                let got = evaluation_sum(nodes, receiver, 0.0);
                assert!(
                    db_ratio(got, exact).abs() <= 0.0001,
                    "floor={d_floor_m}, theta={degrees}: {} dB",
                    db_ratio(got, exact)
                );
            }
        }
    }

    #[test]
    fn split_merge_end_on_radial_and_fault() {
        let receiver = [37.0, 44.0];
        let whole = sum(LineNodeIterator::new(
            piece([-120.0, 0.0], [120.0, 0.0]),
            receiver,
            LineLayer::Road,
            &[],
        )
        .unwrap());
        let split: f64 = (0..33)
            .map(|index| {
                let start = -120.0 + 240.0 * f64::from(index) / 33.0;
                let end = -120.0 + 240.0 * f64::from(index + 1) / 33.0;
                sum(LineNodeIterator::new(
                    piece([start, 0.0], [end, 0.0]),
                    receiver,
                    LineLayer::Road,
                    &[],
                )
                .unwrap())
            })
            .sum();
        assert!((whole - split).abs() <= 3e-13);
        let end: Vec<_> = LineNodeIterator::new(
            piece([0.0, 0.0], [250.0, 0.0]),
            [500.0, 0.0],
            LineLayer::Road,
            &[],
        )
        .unwrap()
        .collect();
        assert!(!end.is_empty());
        let hint = SkylineHint {
            kind: SkylineHintKind::Range { radius_m: 50.0 },
            range_stratum: 0,
            start_rad: 0.0,
            end_rad: 1.0,
            source_id: 7,
        };
        let iterator = LineNodeIterator::new(
            piece([-100.0, 0.0], [100.0, 0.0]),
            [0.0, 1.0],
            LineLayer::Road,
            &[hint],
        )
        .unwrap();
        let u = qm_atan((50.0_f64.powi(2) - 1.0).sqrt() / ROAD_D_FLOOR_M);
        assert!(
            iterator.boundaries().iter().any(|b| (*b - u).abs() < 1e-14)
                && iterator.boundaries().iter().any(|b| (*b + u).abs() < 1e-14)
        );
        assert_eq!(
            derived_node_cap(H0_V3_THETA_3_RAD, H_MAX, LINE_MAX_LENGTH_M),
            98
        );
        assert_eq!(
            derived_node_cap(THETA_MAX_RAD, 0, LINE_MAX_LENGTH_M),
            H0_NODE_CAP
        );
        assert_eq!(LineLayer::Road.d_floor_m(), 3.55);
        assert_eq!(LineLayer::Rail.d_floor_m(), 1.2175);
        let mut faulted = LineNodeIterator::with_test_cap(
            piece([-125.0, 0.0], [125.0, 0.0]),
            [0.0, 0.0],
            LineLayer::Road,
            &[],
            1,
        )
        .unwrap();
        assert!(faulted.next().is_some());
        assert!(matches!(
            faulted.next_checked(),
            Err(LineNodeError::NodeCapOverflow)
        ));
        assert_eq!(faulted.hard_fault_count(), 1);
    }

    #[test]
    fn hint_admission_is_bounded_deterministic_and_keeps_near_roots() {
        let mut hints: Vec<_> = (0..20)
            .map(|i| SkylineHint {
                kind: SkylineHintKind::Range {
                    radius_m: 10.0 + f64::from(i),
                },
                range_stratum: i,
                start_rad: f64::from(i),
                end_rad: f64::from(i) + 0.5,
                source_id: u64::from(i),
            })
            .collect();
        let line = piece([-100.0, 0.0], [100.0, 0.0]);
        let receiver = [0.0, 1.0];
        let forward = LineNodeIterator::new(line, receiver, LineLayer::Road, &hints).unwrap();
        hints.reverse();
        let reverse = LineNodeIterator::new(line, receiver, LineLayer::Road, &hints).unwrap();
        assert_eq!(forward.hint_drop_count(), 8);
        assert_eq!(reverse.hint_drop_count(), 8);
        assert_eq!(forward.boundaries(), reverse.boundaries());

        let nearest_positive_u = qm_atan((10.0_f64.powi(2) - 1.0).sqrt() / ROAD_D_FLOOR_M);
        let dropped_positive_u = qm_atan((29.0_f64.powi(2) - 1.0).sqrt() / ROAD_D_FLOOR_M);
        assert!(forward
            .boundaries()
            .iter()
            .any(|u| (*u - nearest_positive_u).abs() < 1e-14));
        assert!(!forward
            .boundaries()
            .iter()
            .any(|u| (*u - dropped_positive_u).abs() < 1e-14));

        let azimuth_only = SkylineHint {
            kind: SkylineHintKind::Azimuth {
                relative_u_rad: 0.2,
            },
            range_stratum: 0,
            start_rad: 0.0,
            end_rad: 0.0,
            source_id: 1,
        };
        let near = LineNodeIterator::new(line, receiver, LineLayer::Road, &[azimuth_only]).unwrap();
        assert!(near.boundaries().iter().all(|u| (*u - 0.2).abs() > 1e-14));

        let disabled = LineNodeIterator::with_dials(
            line,
            receiver,
            ROAD_D_FLOOR_M,
            H0_V3_THETA_3_RAD,
            0,
            &hints,
            None,
        )
        .unwrap();
        assert_eq!(disabled.hint_drop_count(), 40);
    }

    #[test]
    fn near_133_degree_fan_stays_within_the_derived_bound() {
        let receiver = [0.0, 55.0];
        let mut iterator = LineNodeIterator::new(
            piece([-125.0, 0.0], [125.0, 0.0]),
            receiver,
            LineLayer::Road,
            &[],
        )
        .unwrap();
        let nodes: Vec<_> = iterator.by_ref().collect();
        let count = nodes.len();
        assert!(count <= derived_node_cap(H0_V3_THETA_3_RAD, H_MAX, LINE_MAX_LENGTH_M));
        assert_eq!(iterator.hard_fault_count(), 0);
        assert!(nodes
            .iter()
            .all(|node| (0.0..=1.0).contains(&node.piece_fraction)));
        let span = 2.0 * qm_atan(125.0 / 55.0).to_degrees();
        assert!(span > 132.0 && span < 134.0, "span={span}");
    }
}
