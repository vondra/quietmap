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
    Ship,
}

impl LayerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Road => "road",
            Self::Railway => "railway",
            Self::Building => "building",
            Self::Industrial => "industrial",
            Self::Aircraft => "aircraft",
            Self::Ship => "ship",
        }
    }

    /// Declaration-order ordinal: the top-K total-order tiebreak between kinds.
    pub fn sort_ordinal(self) -> u8 {
        match self {
            Self::Road => 0,
            Self::Railway => 1,
            Self::Building => 2,
            Self::Industrial => 3,
            Self::Aircraft => 4,
            Self::Ship => 5,
        }
    }
}

/// Top-K order over traces: received full Lden desc, then kind, subtype,
/// then the stable per-(kind, subtype) row index. A total order — every
/// builder assigns seqs unique within its (kind, subtype) class — so
/// in-kernel per-kind pre-selection and the global re-sort agree exactly,
/// unlike the old Lden-only unstable sort, whose tied order depended on the
/// input it happened to see. Behaviour change vs the old sort: none, unless
/// two traces tie received Lden to the bit (then segment order wins, before:
/// pdqsort artifact).
pub fn cmp_traces_for_top_k(
    a: &trace_types::SegmentTrace,
    b: &trace_types::SegmentTrace,
) -> std::cmp::Ordering {
    b.received_lden
        .full
        .total_cmp(&a.received_lden.full)
        .then_with(|| a.kind.sort_ordinal().cmp(&b.kind.sort_ordinal()))
        .then_with(|| a.aircraft_subtype.cmp(&b.aircraft_subtype))
        .then_with(|| a.sort_seq.cmp(&b.sort_seq))
}

/// Per-kind restriction of [`cmp_traces_for_top_k`] over stub keys: the kernel
/// pre-selection sort. Same kind on both sides, so kind/subtype are constant
/// and only (key desc, seq asc) decides — identical to the global order
/// restricted to this kind.
pub fn cmp_stub_keys_for_top_k(
    (key_a, seq_a): (f64, u64),
    (key_b, seq_b): (f64, u64),
) -> std::cmp::Ordering {
    key_b
        .total_cmp(&key_a)
        .then_with(|| seq_a.cmp(&seq_b))
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
mod inputs;
mod metadata;
mod propagation;
mod raster_sampler;
mod result;
mod trace_types;

pub use aircraft_detail::*;
pub use inputs::*;
pub use metadata::*;
pub use propagation::*;
pub use raster_sampler::*;
pub use result::*;
pub use trace_types::*;
