//! Upper bound of the aircraft energy a box or a single segment can deliver at a receiver, for
//! the popup's stop rule (PLAN section 5): its levels at the ten NPD distances read at its nearest
//! slant with no attenuation, plus the largest installation gain and a finite-geometry margin. It
//! stays above [`segment_sel_at_receiver`](super::segment::segment_sel_at_receiver) for every
//! segment inside the box (`bound_tests`).
//!
//! Why each step holds: every level falls with distance, so the box's nearest slant reads at
//! least what each piece's own does; each piece's level is linear in lg d between two NPD
//! distances, so the level of their energy sum is convex there and the chord through the box's
//! levels lies above it; below 200 ft no piece rises faster than the steepest first interval of any
//! class; past 25,000 ft every tail falls at least as fast as spherical divergence; Delta_F, Lambda
//! and screening only attenuate, and Delta_I adds at most `INSTALLATION_CORRECTION_MAX_DB`. Only
//! the geometry step is measured, not proven: the kernel reads the curve at the slant to the
//! closest point of the segment's infinite line, nearer than the segment itself, and Delta_F must
//! more than pay for it.

use super::corrections::INSTALLATION_CORRECTION_MAX_DB;
use super::npd::{
    METRES_PER_FOOT, NPD_DISTANCES, NPD_DISTANCES_FT, NPD_NEAREST_SLANT_M,
    STEEPEST_FIRST_INTERVAL_DB_PER_DECADE, curve_level_db,
};

/// dev4 dropped a segment whose SEL at a receiver, free or screened, stayed below this level.
/// r051 drops nothing by its own level (ARCHITECTURE: never a per-source threshold), and a box
/// sums its pieces before any receiver exists, so the reference kernel drops nothing either. The
/// floor survives inside the bound (PLAN section 8): a box or segment whose bound, as an SEL, is
/// below it cannot reach it.
pub const EVENT_FLOOR_SEL_DB: f64 = 20.0;

/// Spare above the measured geometry step: over every class, operation and power row and 5.3 M
/// segments whose closest point lies off the segment (low, short, receiver on the extension,
/// d_lambda up to three times the slant), NPD + Delta_F stayed at least 2.8 dB under the curve at
/// the segment's nearest distance; with the closest point on the segment Delta_F <= 0 alone keeps
/// it under, reached only overhead a long segment. It also covers boxes storing their levels in
/// 0.01 dB steps.
pub const FINITE_GEOMETRY_MARGIN_DB: f64 = 0.5;

/// Upper bound (dB) of the SEL a box or segment delivers at a receiver `nearest_slant_m` from its
/// nearest point, from its levels at the ten NPD distances: a segment's
/// [`SegmentEmission::npd_distance_levels`](super::segment::SegmentEmission::npd_distance_levels),
/// a box's level of the summed weighted energies at each distance (-inf when silent). Its curve is
/// read linearly in log distance as Doc 29 does, past 25,000 ft by spherical divergence alone, and
/// below 200 ft rising at the steepest first interval of any class.
pub fn aircraft_sel_bound_db(levels_db: &[f64; NPD_DISTANCES], nearest_slant_m: f64) -> f64 {
    if levels_db[0] == f64::NEG_INFINITY {
        return f64::NEG_INFINITY;
    }
    let first_m = NPD_DISTANCES_FT[0] * METRES_PER_FOOT;
    let slant_m = nearest_slant_m.max(NPD_NEAREST_SLANT_M);
    let curve_db = if slant_m < first_m {
        levels_db[0] + *STEEPEST_FIRST_INTERVAL_DB_PER_DECADE * (first_m / slant_m).log10()
    } else {
        curve_level_db(levels_db, 0.0, slant_m)
    };
    curve_db + INSTALLATION_CORRECTION_MAX_DB + FINITE_GEOMETRY_MARGIN_DB
}

#[cfg(test)]
#[path = "bound_tests.rs"]
mod tests;
