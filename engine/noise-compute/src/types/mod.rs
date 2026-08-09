//! Core types for noise computation.

use serde::Serialize;

/// Noise source category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LayerKind {
    Road,
    Railway,
    Building,
    Industrial,
    Aircraft,
}

impl LayerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Road => "road",
            Self::Railway => "railway",
            Self::Building => "building",
            Self::Industrial => "industrial",
            Self::Aircraft => "aircraft",
        }
    }
}

impl std::fmt::Display for LayerKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Number of octave bands (63 Hz to 8 kHz).
pub const NUM_BANDS: usize = 8;

/// Leisure-area sources (settlement v2 phase 2) fold into the building layer
/// but keep their own emission classes. A leisure `PointSource` carries
/// `source_type = LEISURE_TYPE_BASE + sport` so the popup naming + (future)
/// metadata can tell a padel court from a residential block without a new layer
/// kind. 100 leaves the whole `building_type` 0–13 range free.
pub const LEISURE_TYPE_BASE: u8 = 100;

mod aircraft_detail;
mod config;
mod inputs;
mod metadata;
mod propagation;
mod result;
mod trace_types;

pub use aircraft_detail::*;
pub use config::*;
pub use inputs::*;
pub use metadata::*;
pub use propagation::*;
pub use result::*;
pub use trace_types::*;
