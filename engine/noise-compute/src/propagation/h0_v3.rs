//! CPU-only H0 V3 arm and its independent judge/scorer authorities.

use serde::{Deserialize, Serialize};

use crate::compute::element::{LineLayer, LinePiece, THETA_MAX_RAD};

use super::h0_streaming_reduction::{
    reduce_h0_with_theta, H0Candidate, H0Reduction, H0ReductionError,
};

pub use super::h0_v3_judge::{build_h0_judge_nodes, H0JudgeNodes};
pub use super::h0_v3_score::{
    score_h0_v3, score_h0_v3_periods, select_h0_theta, H0V3DeltaSummary, H0V3Observation,
    H0V3PeriodDeltaSummary,
};

pub const H_JUDGE_MAX: usize = 65_536;
/// Hard CPU replay bound per live pair. The input-store replay is transient;
/// reducer/judge retained state remains independent of this count.
pub const H0_V3_RAW_CANDIDATES_PER_PAIR_MAX: usize = 65_536;
/// Design-v3's exact maximum logical judge-hint storage: 65,536 34-byte
/// records plus the 32-byte boundary-list header.
pub const JUDGE_LOGICAL_HINT_BYTES_MAX: usize = H_JUDGE_MAX * 34 + 32;
pub const JUDGE_COARSE_EPSILON_DEGREES: f64 = 0.5;
pub const JUDGE_FINE_EPSILON_DEGREES: f64 = 0.25;
pub const JUDGE_HALVING_MAX_DELTA_DB: f64 = 0.01;
pub const H0_V3_MAX_DELTA_DB: f64 = 0.5;
pub const H0_V3_JUDGE_MACHINE_HOUR_CEILING: f64 = 192.0;

/// Exact CPU fields pre-registered for the H0 decision. Each arm renders all
/// 512x512 pixels; these are not probe coordinates.
pub const H0_V3_CASES: [H0V3Case; 4] = [
    H0V3Case::new("dobris-road", 12, 2206, 1391, LineLayer::Road),
    H0V3Case::new("dobris-rail", 12, 2206, 1391, LineLayer::Rail),
    H0V3Case::new("praha-road", 12, 2212, 1387, LineLayer::Road),
    H0V3Case::new("praha-rail", 12, 2212, 1387, LineLayer::Rail),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct H0V3Case {
    pub name: &'static str,
    pub zoom: u8,
    pub x: u32,
    pub y: u32,
    pub layer: LineLayer,
}

impl H0V3Case {
    pub const fn new(name: &'static str, zoom: u8, x: u32, y: u32, layer: LineLayer) -> Self {
        Self {
            name,
            zoom,
            x,
            y,
            layer,
        }
    }
}

/// The H0-only grid. H32 values are deliberately absent until this sweep asks
/// for refinement and the pass-H price arm passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum H0V3Theta {
    Degrees5,
    Degrees4,
    Degrees3,
    Degrees2,
}

impl H0V3Theta {
    pub const COARSE_TO_FINE: [Self; 4] = [
        Self::Degrees5,
        Self::Degrees4,
        Self::Degrees3,
        Self::Degrees2,
    ];

    #[must_use]
    pub const fn degrees(self) -> u8 {
        match self {
            Self::Degrees5 => 5,
            Self::Degrees4 => 4,
            Self::Degrees3 => 3,
            Self::Degrees2 => 2,
        }
    }

    #[must_use]
    pub fn radians(self) -> f64 {
        match self {
            Self::Degrees5 => core::f64::consts::PI / 36.0,
            Self::Degrees4 => core::f64::consts::PI / 45.0,
            Self::Degrees3 => THETA_MAX_RAD,
            Self::Degrees2 => core::f64::consts::PI / 90.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum H0V3Error {
    Reduction(H0ReductionError),
    JudgeHintCapacityExceeded,
    InvalidPairGeometry,
    InvalidObservation,
    ObservationKeyMismatch,
}

impl From<H0ReductionError> for H0V3Error {
    fn from(error: H0ReductionError) -> Self {
        Self::Reduction(error)
    }
}

/// Run one pre-registered H0 production arm through the landed reducer.
pub fn reduce_h0_v3<I>(
    piece_in_receiver_frame: LinePiece,
    layer: LineLayer,
    theta: H0V3Theta,
    candidates: I,
) -> Result<H0Reduction, H0V3Error>
where
    I: IntoIterator<Item = H0Candidate>,
{
    Ok(reduce_h0_with_theta(
        piece_in_receiver_frame,
        layer,
        theta.radians(),
        candidates,
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::element::{derived_node_cap, LINE_MAX_LENGTH_M};

    #[test]
    fn h0_sweep_caps_are_derived_for_every_frozen_theta() {
        let caps = H0V3Theta::COARSE_TO_FINE
            .map(|theta| derived_node_cap(theta.radians(), 0, LINE_MAX_LENGTH_M));
        assert_eq!(caps, [40, 50, 66, 99]);
        assert_eq!(
            H0V3Theta::Degrees3.radians().to_bits(),
            THETA_MAX_RAD.to_bits()
        );
    }
}
