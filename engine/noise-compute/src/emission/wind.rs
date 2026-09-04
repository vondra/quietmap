//! Wind turbine noise emission (IEC 61400-11).

use crate::types::NUM_BANDS;

/// Wind turbine broadband spectrum [dB relative].
const TURBINE_SPECTRUM: [f64; NUM_BANDS] = [-2.0, -1.0, 0.0, 1.0, 1.0, 0.0, -2.0, -5.0];

/// Maximum A-weighted sound power LwA from rated power.
///
/// Published max LwA clusters at 104–106.5 dB(A) nearly independent of rating
/// across 1.8–6.6 MW — a flat band, not the old 98..107 slope (audit 2026-06
/// industrial-report.md I-10; per-type sources: Enercon type list at
/// de.wikipedia.org, Linton WF noise chapter, wind-watch.org V112 general
/// specification, vestas.com V150, nordex-online.com N163). Max-mode LwA is
/// the conservative pick — serrated/noise-reduced modes go down to ~99.
pub fn turbine_lw(rated_power_kw: f64) -> f64 {
    if !rated_power_kw.is_finite() {
        return f64::NEG_INFINITY; // truly invalid data
    }
    // rated_power_kw == 0 means "unknown" in OSM — mid-band 105 dB(A)
    if rated_power_kw <= 0.0 {
        return 105.0;
    }
    match rated_power_kw as u32 {
        // unknown rating (sentinel 0 from normalize): mid-band guess — fleet
        // median sits between the 1-2 MW legacy and 3+ MW modern classes
        0 => 105.0,
        // <1 MW: small/legacy machines — kept from the pre-audit table (no I-10 source disputes it)
        1..=999 => 98.0,
        // 1–2 MW era incl. exactly 2.0 MW: Vestas V90-2.0 = 104.0, Enercon E-82 E2 (2.0 MW) = 104.0
        1000..=2000 => 104.0,
        // 2–3 MW: Enercon E-92 (2.35 MW) = 105.0
        2001..=2999 => 105.0,
        // 3–5 MW: Vestas V112-3.0 = 106.5, Nordex N149 = 106.1
        3000..=4999 => 106.0,
        // ≥5 MW: Nordex N163 = 106.4, Enercon E-160 = 106.0, Vestas V150-6.0 = 104.9
        _ => 106.5,
    }
}

/// Compute emission bands for a wind turbine, normalized so
/// `a_weighted_total(bands) == turbine_lw(rated_power_kw)`.
pub fn turbine_emission_bands(rated_power_kw: f64) -> [f64; NUM_BANDS] {
    super::spectrum::normalized_emission_bands(turbine_lw(rated_power_kw), &TURBINE_SPECTRUM)
}

/// Combined: returns (LwA, emission_bands).
pub fn wind_turbine_emission(rated_power_kw: f64) -> (f64, [f64; NUM_BANDS]) {
    let lw = turbine_lw(rated_power_kw);
    let bands = turbine_emission_bands(rated_power_kw);
    (lw, bands)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::propagation::iso9613::a_weighted_total;

    #[test]
    fn test_turbine_lw() {
        // Flat published band per audit I-10 — pins the whole LUT.
        assert_eq!(turbine_lw(500.0), 98.0);
        assert_eq!(turbine_lw(999.0), 98.0);
        assert_eq!(turbine_lw(1000.0), 104.0);
        assert_eq!(turbine_lw(1999.0), 104.0);
        // exactly 2.0 MW belongs to the V90/E-82 = 104 evidence, not the 105 band
        assert_eq!(turbine_lw(2000.0), 104.0);
        assert_eq!(turbine_lw(2001.0), 105.0);
        assert_eq!(turbine_lw(2350.0), 105.0);
        // unknown rating (sentinel 0 from normalize) — mid-band guess
        assert_eq!(turbine_lw(0.0), 105.0);
        assert_eq!(turbine_lw(3000.0), 106.0);
        assert_eq!(turbine_lw(4999.0), 106.0);
        assert_eq!(turbine_lw(5000.0), 106.5);
        assert_eq!(turbine_lw(6600.0), 106.5);
    }

    #[test]
    fn test_unknown_rated_power_uses_default() {
        assert_eq!(turbine_lw(0.0), 105.0); // unknown → mid-band
        assert!(turbine_lw(f64::NAN).is_infinite() && turbine_lw(f64::NAN).is_sign_negative());
    }

    #[test]
    fn test_turbine_bands() {
        // 3 MW turbine: LwA = 106 and the normalized bands sum back to it
        // exactly — the spectrum shape no longer adds hidden energy
        // (pre-normalization this read 111.4 dB(A)).
        let bands = turbine_emission_bands(3000.0);
        let aw = a_weighted_total(&bands);
        assert!((aw - 106.0).abs() < 1e-9, "3MW turbine: {:.12}", aw);
    }
}
