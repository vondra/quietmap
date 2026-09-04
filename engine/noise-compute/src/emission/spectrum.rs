//! Emission-spectrum normalization shared by industrial, wind, and settlement
//! sources (audit 2026-06 fix B4+B6).
//!
//! Pre-fix, `bands[i] = lw + spectrum[i]` silently inflated every source: the
//! A-weighted energy sum of a relative spectrum is not 0 dB(A) but +4.9..+6.4
//! dB(A) depending on shape (audit industrial-report.md I-03 — e.g. a
//! "105 dB" wind turbine actually radiated 111.4 dB(A); popup `emission_db`
//! showed 113.4 for a nominal-107 one). [`normalized_emission_bands`] makes
//! `lw` the honest A-weighted total.
//!
//! Expectation ledger for the post-rebuild checks (/check-world,
//! /check-popup), relative to pre-C7 values:
//! - industrial sites: −4.9..−6.4 dB(A) per profile family (= the spectrum
//!   offsets above; heavy-industry undershoot deepens — accepted, the
//!   area-density model is backlog I-04);
//! - wind / Pchery transect: ≈ −4..−5 dB (spectrum −6.4, flat-LUT +1.0,
//!   small hub-geometry term; audit measured the transect +4..+5 dB hot);
//! - settlement/buildings: was net 0.0 via the W7 compensating bump; that
//!   bump was since REPLACED by honest measured-anchored constants
//!   (settlement v2 phase 1, 2026-06-12 — `settlement.rs` module doc). The
//!   C8a recalibration note now applies to industrial only.

use crate::propagation::iso9613::a_weighted_total;
use crate::types::NUM_BANDS;

/// Emission bands `lw + spectrum[i] − offset` where `offset =
/// a_weighted_total(spectrum)` (the relative spectrum treated as absolute band
/// levels), so that `a_weighted_total(result) == lw` — `lw` IS the radiated
/// A-weighted sound power.
///
/// `a_weighted_total` runs on `fast_exp_f64`, whose relative error varies
/// with the argument (5th-order Taylor, ≤ ~3e-6), so the plain subtraction
/// leaves a shift-variance residual of ~1e-6..1e-5 dB. One fixed-point
/// correction step (the map has slope −1 in `offset`) collapses it to
/// ~1e-10 dB, making the invariant strict enough to test at 1e-9.
pub fn normalized_emission_bands(lw: f64, spectrum: &[f64; NUM_BANDS]) -> [f64; NUM_BANDS] {
    let offset = a_weighted_total(spectrum);
    let mut bands: [f64; NUM_BANDS] = std::array::from_fn(|i| lw + spectrum[i] - offset);
    let residual = a_weighted_total(&bands) - lw;
    if residual.is_finite() {
        for band in &mut bands {
            *band -= residual;
        }
    }
    bands
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emission::{industrial, settlement, wind};

    const TOL: f64 = 1e-9;

    #[track_caller]
    fn assert_invariant(lw: f64, spectrum: &[f64; NUM_BANDS], what: &str) {
        let bands = normalized_emission_bands(lw, spectrum);
        let aw = a_weighted_total(&bands);
        assert!(
            (aw - lw).abs() < TOL,
            "{what}: a_weighted_total(normalized bands) {aw:.12} != lw {lw:.12}"
        );
    }

    fn check_industrial_profile(profile: &industrial::IndustrialProfile, what: &str) {
        // Natural Lw at the area-clamp extremes + the 1 ha reference + an
        // arbitrary level — the invariant must hold for any lw.
        for area in [100.0, 10_000.0, 500_000.0, 3_000_000.0] {
            let lw =
                industrial::industrial_lw(profile, area, industrial::INDUSTRIAL_AREA_CAP_HEAVY_M2);
            assert_invariant(lw, &profile.spectrum, what);
        }
        assert_invariant(61.7, &profile.spectrum, what);
    }

    /// The forever-invariant: for EVERY emission profile in the atlas,
    /// `a_weighted_total(normalized_emission_bands(lw, shape)) == lw`.
    #[test]
    fn every_industrial_profile_holds_invariant() {
        for source_type in 0..=u8::MAX {
            let p = industrial::industrial_profile(source_type);
            check_industrial_profile(&p, &format!("source_type {source_type}"));
        }
        // Exhaustive over the NACE domain: covers the 4-digit arms (3599,
        // 3511, 3512) and every 2-digit fallback arm.
        for nace in 0..=9999u16 {
            if let Some(p) = industrial::nace_profile(nace) {
                check_industrial_profile(&p, &format!("nace {nace}"));
            }
        }
        for subtype in 0..=u8::MAX {
            if let Some(p) = industrial::subtype_profile(subtype) {
                check_industrial_profile(&p, &format!("subtype {subtype}"));
            }
        }
    }

    #[test]
    fn every_building_class_holds_invariant() {
        for building_type in 0..=u8::MAX {
            // SILENT is deliberately sub-audible (lw → −∞, gated before scatter)
            // so the radiated-dB(A) invariant does not apply to it.
            if building_type == settlement::SILENT {
                continue;
            }
            let p = settlement::building_profile(building_type);
            for (area, floors) in [(100.0, 1u8), (200.0, 3), (5_000.0, 10)] {
                let lw = settlement::building_lw(&p, area, floors);
                assert_invariant(lw, &p.spectrum, &format!("building_type {building_type}"));
            }
        }
    }

    #[test]
    fn every_turbine_power_class_holds_invariant() {
        for power_kw in [
            0.0, 500.0, 999.0, 1000.0, 1999.0, 2000.0, 2999.0, 3000.0, 4999.0, 5000.0, 8000.0,
            12_000.0,
        ] {
            let (lw, bands) = wind::wind_turbine_emission(power_kw);
            let aw = a_weighted_total(&bands);
            assert!(
                (aw - lw).abs() < TOL,
                "turbine {power_kw} kW: a_weighted_total(bands) {aw:.12} != lw {lw:.12}"
            );
        }
    }
}
