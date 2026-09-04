//! Doc 29 4th Edition aircraft noise emission.
//!
//! Doc 29 supplies empirical NPD emission and lateral attenuation. Low
//! airborne paths then add the explicitly documented terrain/building
//! line-of-sight correction; cruise remains free-field.
//!
//! Master equation (Eq. 4-8b):
//!   SEL_seg = L_E(P, d_p) + ΔV + ΔI(φ) - Λ(β, l) + ΔF
//!
//! ## Module layout
//!
//! * [`npd`] — NPD profile metadata, alpha-eff back-out, NpdLuts cache,
//!   reach estimation, profiles_generated re-exports.
//! * [`doc29`] — CPA geometry, Δv/ΔF/ΔI/Λ corrections, shared
//!   `segment_energy_kernel`, period_leq.
//! * [`horizon`] — C2 receiver terrain horizon (32-sector signed
//!   tangents) + ISO 9613-2 §7.4 / AEDT LOS-blockage screening Dz for
//!   the airborne kernel.
//! * [`screening`] — receiver-local vector-building horizon and the shared
//!   anchored single-edge diffraction rule.
//! * [`screening_bounds`] — the obstacle-height criterion used by the GPU
//!   building-horizon prune: a roof screens only aircraft below its own
//!   elevation angle, taken per azimuth group.
//! * [`segment_filters`] — per-segment validity gates (airborne / ground
//!   stale / airport ground), `SegmentTerrain` cache, ground-ops kind /
//!   context constants.
//! * [`ground_ops`] — surface-model constants (per-kind reference
//!   speeds + spectrum shapes) consumed by the `airport_traffic`
//!   emission kernel.
//! * [`segment_sel`] — single-shot per-segment SEL wrappers (popup +
//!   tests).

mod doc29;
mod ground_ops;
mod horizon;
mod npd;
mod screening;
mod screening_bounds;
mod segment_filters;
mod segment_sel;

pub use doc29::*;
pub(crate) use ground_ops::*;
pub use horizon::*;
pub use npd::*;
pub use screening::*;
pub use screening_bounds::*;
pub use segment_filters::*;
pub use segment_sel::*;
