//! Wind turbine noise emission (IEC 61400-11).

use crate::types::NUM_BANDS;

/// Wind turbine broadband spectrum [dB relative, unweighted, 1 kHz = 0].
/// Energy mean of five published max-mode octave spectra, each relative to
/// its own A-total (so the +2 dB EIA uncertainty cancels), converted back to
/// unweighted with [`crate::constants::A_WEIGHTING`]: Vestas V162-7.2,
/// Siemens SWT-101-3.2, Vestas V150-5.6 and Vestas V117-4.2 (Oliver Forest
/// Wind Farm EIA Technical Appendix 13.2 Table 3, Statkraft) plus Siemens
/// Gamesa SG 6.0-155 AM0 (Ballinagree Wind Farm EIAR Appendix 7.4 Table
/// 7.4.2). The legacy 2 MW pair of the same appendix (Siemens SWT-93,
/// Vestas V80) is excluded — the mapped fleet's median hub (105 m) is modern.
/// The old shape peaked at 500 Hz–1 kHz and ran 1.5–3.6 dB hot at 500 m–1 km
/// against these five; the published shape is low-frequency-heavy, as trailing
/// edge noise is.
const TURBINE_SPECTRUM: [f64; NUM_BANDS] = [13.2, 11.1, 7.8, 4.4, 0.0, -3.9, -9.7, -19.4];

/// Generic normalised LwA(v) curve [dB rel Lmax] over 1 m/s hub-height wind
/// bins 0–25: arithmetic mean of (Lw−Lmax) over nine published type curves —
/// the six Oliver Forest Appendix 13.2 Table 2 rows (standardised 10 m wind)
/// plus Ballinagree Appendix 7.4 SG 6.0-155 AM0 (hub height), Nordex N149
/// (standardised 10 m) and Vestas V150 mode 0 (hub height). Reference heights
/// mix because only two hub-height curves are published; the SHAPE (what the
/// duty sum needs) barely moves between them. Below cut-in (3 m/s) the rotor
/// is silent; above the tabulated 12 m/s the turbine holds Lmax (Dutch
/// Reken- en meetvoorschrift windturbines, 2011: LW above rated = LW(rated)).
const GENERIC_LW_REL_V: [f64; 26] = [
    f64::NEG_INFINITY, // 0 m/s — below cut-in: silent
    f64::NEG_INFINITY, // 1 m/s
    f64::NEG_INFINITY, // 2 m/s
    -10.88, -9.73, -6.24, -2.90, -1.19, -0.32, -0.14, -0.10, 0.0, 0.0, // 3–12
    0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, // 13–25
];

/// PLACEHOLDER hub-height mean wind speed [m/s] until the meteorology raster
/// arrives (W6-met ERA5 100 m wind-speed classes per period). A Rayleigh
/// distribution with this mean reproduces the published V150 check (−3.4 dB
/// at 7.5 m/s mean); every period shares it, so day/evening/night duties are
/// equal and the map carries no diurnal wind cycle yet.
const PLACEHOLDER_MEAN_WIND_MS: f64 = 7.5;

/// Rayleigh CDF — the placeholder wind-speed distribution is Rayleigh, the
/// maximum-entropy choice for a known mean with no diurnal or orographic
/// information.
fn rayleigh_cdf(v_ms: f64, mean_ms: f64) -> f64 {
    if v_ms <= 0.0 {
        return 0.0;
    }
    let sigma = mean_ms / (std::f64::consts::PI / 2.0).sqrt();
    1.0 - (-v_ms * v_ms / (2.0 * sigma * sigma)).exp()
}

/// Share of time in the 1 m/s bin centred at `v_ms` under the placeholder
/// distribution; the tail past 25.5 m/s folds into the top bin so the shares
/// sum to exactly 1.
fn placeholder_bin_share(v_ms: usize) -> f64 {
    let upper = if v_ms >= 25 {
        f64::INFINITY
    } else {
        v_ms as f64 + 0.5
    };
    let lower = (v_ms as f64 - 0.5).max(0.0);
    let cdf = |v: f64| {
        if v.is_infinite() {
            1.0
        } else {
            rayleigh_cdf(v, PLACEHOLDER_MEAN_WIND_MS)
        }
    };
    (cdf(upper) - cdf(lower)).max(0.0)
}

/// Annual operating duty per the Dutch statutory method (Reken- en
/// meetvoorschrift windturbines, Activiteitenregeling bijlage 4, 2011):
/// ΔL = 10·lg Σ_j U_j·10^((Lw_j − Lmax)/10), with `U_j` the share of time in
/// wind bin `j` and `curve_rel_lmax` the (Lw_j − Lmax) curve (silent bins are
/// −∞). Negative: a turbine at max mode all year is the fiction this removes
/// (−1.2…−3.4 dB across the nine type curves at 7.5 m/s mean; −2.1 generic).
/// Per-period shares plug in here once W6-met publishes the raster.
pub fn annual_duty_db(curve_rel_lmax: &[f64; 26], bin_share: impl Fn(usize) -> f64) -> f64 {
    let mut energy = 0.0;
    for (v, rel) in curve_rel_lmax.iter().enumerate() {
        if rel.is_finite() {
            energy += bin_share(v) * 10f64.powf(rel / 10.0);
        }
    }
    10.0 * energy.max(f64::MIN_POSITIVE).log10()
}

/// Current operating duty: the statutory sum of the generic curve over the
/// placeholder distribution (≈ −2.1 dB), identical in every period.
pub fn wind_duty_db() -> f64 {
    annual_duty_db(&GENERIC_LW_REL_V, placeholder_bin_share)
}

/// Maximum A-weighted sound power LwA from rated power (the Lmax anchor
/// the operating duty subtracts from — NOT the emitted level; see
/// `wind_turbine_emission`).
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
/// `a_weighted_total(bands) == ` the annual operating level (max-mode LUT
/// plus [`wind_duty_db`]).
pub fn turbine_emission_bands(rated_power_kw: f64) -> [f64; NUM_BANDS] {
    let lw = turbine_lw(rated_power_kw);
    let operating = if lw.is_finite() { lw + wind_duty_db() } else { lw };
    super::spectrum::normalized_emission_bands(operating, &TURBINE_SPECTRUM)
}

/// Combined: returns (annual operating LwA, emission_bands).
pub fn wind_turbine_emission(rated_power_kw: f64) -> (f64, [f64; NUM_BANDS]) {
    let lw = turbine_lw(rated_power_kw);
    let operating = if lw.is_finite() { lw + wind_duty_db() } else { lw };
    let bands = super::spectrum::normalized_emission_bands(operating, &TURBINE_SPECTRUM);
    (operating, bands)
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
        // 3 MW turbine: operating LwA = 106 + duty and the normalized bands
        // sum back to it exactly — the spectrum shape adds no hidden energy
        // (pre-normalization this read 111.4 dB(A)).
        let (lw, bands) = wind_turbine_emission(3000.0);
        let aw = a_weighted_total(&bands);
        assert!((lw - (106.0 + wind_duty_db())).abs() < 1e-9, "3MW Lw: {lw:.12}");
        assert!((aw - lw).abs() < 1e-9, "3MW turbine: {aw:.12} != {lw:.12}");
    }

    #[test]
    fn duty_is_the_statutory_sum_over_the_placeholder_distribution() {
        // The placeholder shares sum to exactly 1 (tail folded into bin 25).
        let total: f64 = (0..26).map(placeholder_bin_share).sum();
        assert!((total - 1.0).abs() < 1e-12, "shares sum to {total:.15}");
        // Generic duty at 7.5 m/s mean: −2.14 dB (pins the curve + Rayleigh).
        assert!((wind_duty_db() + 2.14).abs() < 0.02, "duty {:.3}", wind_duty_db());
        // Below cut-in the rotor contributes nothing: all time in bins 0–2
        // silences the turbine; all time at rated holds Lmax exactly.
        let calm = annual_duty_db(&GENERIC_LW_REL_V, |v| if v == 1 { 1.0 } else { 0.0 });
        assert!(calm < -300.0, "calm year must silence: {calm}");
        let storm = annual_duty_db(&GENERIC_LW_REL_V, |v| if v == 20 { 1.0 } else { 0.0 });
        assert!((storm - 0.0).abs() < 1e-9, "rated year holds Lmax: {storm}");
    }

    #[test]
    fn published_v150_check() {
        // Ballinagree V150 mode 0 (hub-height curve 3–20 m/s, max 104.9):
        // −3.4 dB at 7.5 m/s mean — the w7-sources cross-check of the #29
        // implementation against published inputs.
        let v150 = [
            91.3, 91.8, 94.1, 96.9, 100.0, 102.7, 104.0, 104.1, 104.9, 104.9, 104.9, 104.9,
            104.9, 104.9, 104.9, 104.9, 104.9, 104.9,
        ];
        let curve: [f64; 26] = std::array::from_fn(|v| {
            if v < 3 {
                f64::NEG_INFINITY
            } else if v - 3 < v150.len() {
                v150[v - 3] - 104.9
            } else {
                0.0
            }
        });
        let duty = annual_duty_db(&curve, placeholder_bin_share);
        assert!((duty + 3.4).abs() < 0.05, "V150 duty {duty:.3}");
    }
}
