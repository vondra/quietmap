//! Per-microsegment per-movement Z-weighted band energy kernel for
//! the `airport_traffic.arrow` writer + popup/heatmap consumer.
//!
//! Two physical models, **two storage semantics** in the same
//! `band_energy_lin` column (consumer branches on `row.veh_kind`):
//!
//! - **Aircraft (`veh_kind == 0`)**: per-metre sound power `LW'` (linear,
//!   Z-weighted, per band). Stage 2C accumulates as
//!   `band_energy_lin += LW'_lin × (overlap / line.length_m)` — the
//!   density-weighted contribution to the microsegment's line source.
//!   Receiver applies CNOSSOS-EU §2.5.5 line-source formula
//!   `recv_lin = stored_lin × (θ / d_perp_to_extended_line)` over the
//!   FULL microsegment geometry. Refinement invariance follows by
//!   Chasles' theorem (`Σ θᵢ = θ_total` over sub-segments).
//!
//! - **GSE (`veh_kind == 1`)**: per-event absolute SEL@25m (linear).
//!   Stage 2C accumulates as `band_energy_lin += SEL_lin` — no density
//!   scaling, the kinematic moving-point integral already accounts for
//!   `hit.length_within_segment_m` traversal length. Receiver applies
//!   point-source divergence from the 25 m reference.
//!
//! Stage 2C stores **raw Σ over n_days** (v7 Convention B). Popup /
//! heatmap divide by `n_days × period_s` (via `period_leq`) for the
//! period Leq, after A-weight fold at the receiver (storing A-weighted
//! at source would double-count A across frequency-dependent
//! propagation).
//!
//! ### Aircraft LW' calibration
//!
//! `GROUND_OPS_REFERENCE_LW_PER_METER_DB[class][kind]` ≡ historical
//! 1 km event SEL anchor + `10·log10(25/π) = +9.01 dB`. Identity at
//! the legacy calibration point (1 km event, 25 m perpendicular,
//! midpoint receiver): `recv ≈ anchor − 0.14 dB` under both old and
//! new kernels (the 0.14 dB residual is `FLC(1000m, 25m, 0.5) =
//! 10·log10(θ_1km / π)`).
//!
//! Day/eve/night penalties applied per-row by `row.period`.

use crate::types::NUM_BANDS;

use super::aircraft::{
    GROUND_OPS_APRON_SPECTRUM_SHAPE, GROUND_OPS_KIND_APRON_MOVEMENT, GROUND_OPS_KIND_RUNWAY_ROLL,
    GROUND_OPS_KIND_TAXI, GROUND_OPS_REF_OFFSET_M, GROUND_OPS_RUNWAY_DEPARTURE_BONUS_DB,
    GROUND_OPS_RUNWAY_SPECTRUM_SHAPE, GROUND_OPS_SPEED_CLAMP_DB, GROUND_OPS_TAXI_SPECTRUM_SHAPE,
    SURFACE_APRON_SPEED_KT, SURFACE_RUNWAY_SPEED_KT, SURFACE_TAXIWAY_SPEED_KT,
};
use super::gse::GSE_LW_BANDS_DB;
use super::profiles_generated::GROUND_OPS_REFERENCE_LW_PER_METER_DB;

const KT_TO_M_S: f64 = 0.514_444_44;

/// Aircraft: per-metre Z-weighted sound power `LW'` (linear units).
///
/// Returns `[0; 8]` for degenerate input (`leg_avg_speed_kt ≤ 0`).
/// Panics on unknown `ops_kind`. **Aircraft only** — GSE callers
/// must use [`compute_gse_band_energy_lin`].
pub fn compute_aircraft_lw_per_meter_lin(
    class_idx: u8,
    ops_kind: u8,
    is_departure: u8,
    leg_avg_speed_kt: f32,
) -> [f32; NUM_BANDS] {
    // `!(v > 0.0)` NOT `v <= 0.0`: a NaN speed must return the zero-emission
    // default, which only the negated `>` gives (`<=` is false for NaN and would
    // fall through to compute with a NaN speed).
    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    if !(leg_avg_speed_kt > 0.0) {
        return [0.0; NUM_BANDS];
    }
    let bands_db = aircraft_lw_per_meter_z_db(class_idx, ops_kind, is_departure, leg_avg_speed_kt);
    let mut bands = [0.0f32; NUM_BANDS];
    for i in 0..NUM_BANDS {
        bands[i] = 10f64.powf(bands_db[i] / 10.0) as f32;
    }
    bands
}

/// GSE: per-event absolute Z-weighted band SEL@25m (linear units),
/// from the kinematic moving-point integral over `segment_length_m`.
///
/// Returns `[0; 8]` for degenerate input. Panics on unknown
/// `ops_kind` (validated for parity with the aircraft branch even
/// though the GSE math ignores it).
pub fn compute_gse_band_energy_lin(
    class_idx: u8,
    ops_kind: u8,
    leg_avg_speed_kt: f32,
    segment_length_m: f32,
) -> [f32; NUM_BANDS] {
    if !(segment_length_m > 0.0 && leg_avg_speed_kt > 0.0) {
        return [0.0; NUM_BANDS];
    }
    let _ = ops_kind_idx(ops_kind);
    let bands_db = gse_band_sel_z_db(class_idx, leg_avg_speed_kt, segment_length_m);
    let mut bands = [0.0f32; NUM_BANDS];
    for i in 0..NUM_BANDS {
        bands[i] = 10f64.powf(bands_db[i] / 10.0) as f32;
    }
    bands
}

/// dB variant of [`compute_aircraft_lw_per_meter_lin`]. `pub(crate)`
/// because the on-disk writer contract is the linear form.
pub(crate) fn aircraft_lw_per_meter_z_db(
    class_idx: u8,
    ops_kind: u8,
    is_departure: u8,
    speed_kt: f32,
) -> [f64; NUM_BANDS] {
    let kind_idx = ops_kind_idx(ops_kind);
    assert!(
        (class_idx as usize) < GROUND_OPS_REFERENCE_LW_PER_METER_DB.len(),
        "class_idx {class_idx} out of range for {} aircraft classes",
        GROUND_OPS_REFERENCE_LW_PER_METER_DB.len()
    );
    let mut total_lw = GROUND_OPS_REFERENCE_LW_PER_METER_DB[class_idx as usize][kind_idx];
    if ops_kind == GROUND_OPS_KIND_RUNWAY_ROLL && is_departure == 1 {
        total_lw += GROUND_OPS_RUNWAY_DEPARTURE_BONUS_DB;
    }
    let nominal = match ops_kind {
        GROUND_OPS_KIND_RUNWAY_ROLL => SURFACE_RUNWAY_SPEED_KT as f64,
        GROUND_OPS_KIND_TAXI => SURFACE_TAXIWAY_SPEED_KT as f64,
        GROUND_OPS_KIND_APRON_MOVEMENT => SURFACE_APRON_SPEED_KT as f64,
        _ => unreachable!(),
    };
    if speed_kt > 1.0 {
        // Doc 29 Vol 2 Eq 4-14 / AEDT Eq 4-26 duration (dwell) correction:
        // per-metre energy ∝ P/v — a slower pass dwells longer per metre,
        // so LW' RISES as speed drops. Only the dwell sign ports here: the
        // per-kind anchors already bake the kind-nominal speeds (70/18/12 kt),
        // not Doc 29's 160 kt V_ref. ±3 dB clamp keeps outlier ADS-B leg
        // speeds from dominating.
        let adj = -10.0 * ((speed_kt as f64) / nominal.max(1.0)).log10();
        total_lw += adj.clamp(-GROUND_OPS_SPEED_CLAMP_DB, GROUND_OPS_SPEED_CLAMP_DB);
    }

    let shape = match ops_kind {
        GROUND_OPS_KIND_RUNWAY_ROLL => GROUND_OPS_RUNWAY_SPECTRUM_SHAPE,
        GROUND_OPS_KIND_TAXI => GROUND_OPS_TAXI_SPECTRUM_SHAPE,
        GROUND_OPS_KIND_APRON_MOVEMENT => GROUND_OPS_APRON_SPECTRUM_SHAPE,
        _ => unreachable!(),
    };
    let shape_z_sum_db = 10.0
        * shape
            .iter()
            .map(|s| 10f64.powf(s / 10.0))
            .sum::<f64>()
            .log10();
    let c = total_lw - shape_z_sum_db;
    let mut bands = [0.0f64; NUM_BANDS];
    for i in 0..NUM_BANDS {
        bands[i] = c + shape[i];
    }
    bands
}

/// GSE: kinematic moving-point integration along the segment at
/// perpendicular distance 25 m. One closed-form integral handles the
/// time-at-segment, the source receding from the perpendicular foot,
/// and the finite-line geometry — no separate FLC or duration term.
///
/// `SEL_band_z = Lw_band_z + 10·log10(atan(L/(2r)) / (2π·v·r))`
///
/// where r = 25 m. For L → 0, atan(0) → 0, SEL → −∞ → caller
/// short-circuits to zeros via the degenerate guard.
fn gse_band_sel_z_db(
    class_idx: u8,
    leg_avg_speed_kt: f32,
    segment_length_m: f32,
) -> [f64; NUM_BANDS] {
    let speed_m_s = leg_avg_speed_kt as f64 * KT_TO_M_S;
    let r = GROUND_OPS_REF_OFFSET_M;
    let l = segment_length_m as f64;
    // Kinematic integral: E = Lw_lin · atan(L/(2r)) / (2π·v·r)
    let angle = (l / (2.0 * r)).atan();
    let geo_offset_db = 10.0 * (angle / (2.0 * std::f64::consts::PI * speed_m_s * r)).log10();
    assert!(
        (class_idx as usize) < GSE_LW_BANDS_DB.len(),
        "gse_class {class_idx} out of range for {} GSE classes",
        GSE_LW_BANDS_DB.len()
    );
    let lw_bands = &GSE_LW_BANDS_DB[class_idx as usize];
    let mut bands = [0.0f64; NUM_BANDS];
    for i in 0..NUM_BANDS {
        bands[i] = lw_bands[i] + geo_offset_db;
    }
    bands
}

fn ops_kind_idx(ops_kind: u8) -> usize {
    match ops_kind {
        GROUND_OPS_KIND_RUNWAY_ROLL => 0,
        GROUND_OPS_KIND_TAXI => 1,
        GROUND_OPS_KIND_APRON_MOVEMENT => 2,
        other => panic!(
            "airport_traffic: ops_kind must be RUNWAY_ROLL/TAXI/APRON_MOVEMENT (1/2/3), got {other}"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::A_WEIGHTING;
    use crate::emission::gse::{GSE_CLASS_HEAVY, GSE_CLASS_LIGHT, GSE_CLASS_MEDIUM};

    fn sum_z_band_levels_db(bands: &[f64; NUM_BANDS]) -> f64 {
        let lin: f64 = bands.iter().map(|b| 10f64.powf(b / 10.0)).sum();
        10.0 * lin.log10()
    }

    fn sum_a_weighted_band_levels_db(bands: &[f64; NUM_BANDS]) -> f64 {
        let lin: f64 = bands
            .iter()
            .zip(A_WEIGHTING.iter())
            .map(|(b, aw)| 10f64.powf((b + aw) / 10.0))
            .sum();
        10.0 * lin.log10()
    }

    #[test]
    fn aircraft_lw_per_meter_recovers_anchor_minus_zero_point_one_four_at_1km_test() {
        // Refinement-invariance calibration anchor: a 1 km event with
        // the receiver perpendicular to the midpoint at 25 m sees
        // `anchor − 10·log10(π / θ_1km)` = `anchor − 0.14 dB`. The
        // LW' table value is calibrated such that this identity holds
        // (+9.01 dB shift relative to the legacy 1 km event-SEL anchor).
        let bands = aircraft_lw_per_meter_z_db(2, GROUND_OPS_KIND_TAXI, 0, 18.0);
        let lw_total_per_meter = sum_z_band_levels_db(&bands);
        // Receiver applies `+ 10·log10(θ/d)` on top of LW' per metre.
        let theta = 2.0 * (500.0_f64 / 25.0).atan(); // 1km @ 25m midpoint
        let recv = lw_total_per_meter + 10.0 * (theta / 25.0).log10();
        let anchor = GROUND_OPS_REFERENCE_LW_PER_METER_DB[2][1]
            - 10.0 * (25.0_f64 / std::f64::consts::PI).log10();
        let expected = anchor - 10.0 * (std::f64::consts::PI / theta).log10();
        assert!(
            (recv - expected).abs() < 0.05,
            "1km / 25m receiver level {recv} ≠ legacy anchor − 0.14 dB ({expected})"
        );
    }

    #[test]
    fn aircraft_refinement_invariance_holds_for_split_segment() {
        // CNOSSOS-EU §2.5.5: Σ θᵢ = θ_total over collinear sub-segments
        // (Chasles' theorem). For the same physical 1 km line, a single
        // segment and the same line split into N equal sub-segments must
        // give the same total received energy at any receiver. Old kernel
        // violated this; the new per-metre LW' formulation satisfies it
        // by construction.
        let lw = compute_aircraft_lw_per_meter_lin(2, GROUND_OPS_KIND_TAXI, 0, 18.0);
        let lw_total: f64 = lw.iter().map(|b| *b as f64).sum();
        let recv = |theta: f64| -> f64 { lw_total * (theta / 25.0) };

        // 1 × 1000m line, receiver perpendicular to midpoint at 25 m.
        let theta_full = 2.0 * (500.0_f64 / 25.0).atan();
        let single = recv(theta_full);

        // 5 × 200m collinear sub-segs, same receiver: fractions are
        // 0.0/1.0 for the side sub-segs (foot at vertex) and similar
        // signed-d1/d2 for the rest. Their θᵢ sum to θ_full.
        let mut sum_theta = 0.0;
        for i in 0..5 {
            let d1_along = 200.0 * (i as f64) - 500.0; // signed distance to seg start from receiver foot
            let d2_along = 200.0 * (i as f64 + 1.0) - 500.0;
            sum_theta += (d2_along / 25.0).atan() - (d1_along / 25.0).atan();
        }
        let split = recv(sum_theta);
        assert!(
            (split - single).abs() / single < 1e-10,
            "refinement invariance violated: single={single} split={split}"
        );
        // And theta sums match.
        assert!(
            (sum_theta - theta_full).abs() < 1e-12,
            "Σ θᵢ ({sum_theta}) ≠ θ_total ({theta_full})"
        );
    }

    #[test]
    fn aircraft_runway_departure_gets_2_db_bonus() {
        let arr = aircraft_lw_per_meter_z_db(2, GROUND_OPS_KIND_RUNWAY_ROLL, 0, 70.0);
        let dep = aircraft_lw_per_meter_z_db(2, GROUND_OPS_KIND_RUNWAY_ROLL, 1, 70.0);
        let delta = sum_z_band_levels_db(&dep) - sum_z_band_levels_db(&arr);
        assert!(
            (delta - 2.0).abs() < 0.1,
            "departure bonus expected +2 dB, got {delta}"
        );
    }

    /// Dwell correction (Doc 29 Eq 4-14 / AEDT Eq 4-26): per-metre energy
    /// ∝ 1/v, so a slow roll is LOUDER per metre than a fast one. The
    /// pre-2026-06 kernel had the sign inverted (fast = louder).
    #[test]
    fn aircraft_dwell_slower_is_louder_monotonic() {
        let lw = |kt: f32| {
            sum_z_band_levels_db(&aircraft_lw_per_meter_z_db(
                2,
                GROUND_OPS_KIND_RUNWAY_ROLL,
                0,
                kt,
            ))
        };
        let (slow, nominal, fast) = (lw(20.0), lw(70.0), lw(150.0));
        assert!(
            slow > nominal && nominal > fast,
            "dwell monotonicity: 20kt {slow} > 70kt {nominal} > 150kt {fast}"
        );
    }

    /// At the kind-nominal speed the dwell adjust is exactly 0, so the
    /// Z-total recovers the per-kind anchor (arrival: no departure bonus).
    #[test]
    fn aircraft_dwell_identity_at_kind_nominal_speed() {
        for (kind, kind_idx, nominal_kt) in [
            (GROUND_OPS_KIND_RUNWAY_ROLL, 0, 70.0),
            (GROUND_OPS_KIND_TAXI, 1, 18.0),
            (GROUND_OPS_KIND_APRON_MOVEMENT, 2, 12.0),
        ] {
            let total = sum_z_band_levels_db(&aircraft_lw_per_meter_z_db(2, kind, 0, nominal_kt));
            let anchor = GROUND_OPS_REFERENCE_LW_PER_METER_DB[2][kind_idx];
            assert!(
                (total - anchor).abs() < 1e-6,
                "kind {kind}: Z-total {total} ≠ anchor {anchor} at nominal {nominal_kt} kt"
            );
        }
    }

    /// Clamp pins: 20 kt runway (−10·log10(20/70) = +5.4) and 8 kt taxi
    /// (+3.5) pin to exactly +3; 36 kt taxi (−10·log10(2) = −3.01) pins
    /// to exactly −3.
    #[test]
    fn aircraft_dwell_clamp_pins_at_plus_minus_3_db() {
        let total =
            |kind: u8, kt: f32| sum_z_band_levels_db(&aircraft_lw_per_meter_z_db(2, kind, 0, kt));
        let runway_nominal = total(GROUND_OPS_KIND_RUNWAY_ROLL, 70.0);
        let taxi_nominal = total(GROUND_OPS_KIND_TAXI, 18.0);
        let pin = |delta: f64, expected: f64, label: &str| {
            assert!(
                (delta - expected).abs() < 1e-6,
                "{label}: expected {expected:+} dB exactly, got {delta}"
            );
        };
        pin(
            total(GROUND_OPS_KIND_RUNWAY_ROLL, 20.0) - runway_nominal,
            3.0,
            "runway 20 kt",
        );
        pin(
            total(GROUND_OPS_KIND_TAXI, 8.0) - taxi_nominal,
            3.0,
            "taxi 8 kt",
        );
        pin(
            total(GROUND_OPS_KIND_TAXI, 36.0) - taxi_nominal,
            -3.0,
            "taxi 36 kt",
        );
    }

    #[test]
    fn gse_kinematic_integral_short_segment() {
        // L = 50 m, v = 5 m/s, r = 25 → atan(1) = π/4 = 0.785,
        // 2π·v·r = 785. geo_offset = 10·log10(0.785/785) = -30 dB.
        let bands = gse_band_sel_z_db(GSE_CLASS_HEAVY, 9.72, 50.0);
        let z_total = sum_z_band_levels_db(&bands);
        let lw_total = sum_z_band_levels_db(&GSE_LW_BANDS_DB[GSE_CLASS_HEAVY as usize]);
        let delta = z_total - lw_total;
        assert!(
            (delta + 30.0).abs() < 1.0,
            "expected geo offset ≈ -30 dB, got Δ={delta}"
        );
    }

    #[test]
    fn gse_class_ordering_light_lt_medium_lt_heavy() {
        let total = |cls: u8| sum_z_band_levels_db(&gse_band_sel_z_db(cls, 9.72, 250.0));
        let light = total(GSE_CLASS_LIGHT);
        let medium = total(GSE_CLASS_MEDIUM);
        let heavy = total(GSE_CLASS_HEAVY);
        assert!(light < medium, "LIGHT={light} not < MEDIUM={medium}");
        assert!(medium < heavy, "MEDIUM={medium} not < HEAVY={heavy}");
    }

    #[test]
    fn gse_longer_segment_higher_sel() {
        // Kinematic integral grows with L (more source-time near
        // perpendicular foot) → per-movement SEL should increase.
        let short = sum_z_band_levels_db(&gse_band_sel_z_db(GSE_CLASS_MEDIUM, 9.72, 50.0));
        let long = sum_z_band_levels_db(&gse_band_sel_z_db(GSE_CLASS_MEDIUM, 9.72, 250.0));
        assert!(
            long > short,
            "longer GSE seg should give more energy; short={short} long={long}"
        );
    }

    #[test]
    fn gse_no_aircraft_speed_clamp_faster_means_quieter() {
        // Kinematic: SEL ∝ 1/v → faster vehicle gives less per-pass
        // SEL (shorter time near receiver). No 3-dB clamp.
        let slow = sum_z_band_levels_db(&gse_band_sel_z_db(GSE_CLASS_MEDIUM, 5.0, 250.0));
        let fast = sum_z_band_levels_db(&gse_band_sel_z_db(GSE_CLASS_MEDIUM, 50.0, 250.0));
        let delta_db_expected = 10.0 * (5.0_f64 / 50.0).log10(); // -10 dB
        let actual = fast - slow;
        assert!(
            (actual - delta_db_expected).abs() < 0.5,
            "expected 10·log10(v_slow/v_fast)=−10 dB delta, got {actual}"
        );
    }

    #[test]
    fn aircraft_linear_form_matches_db_conversion() {
        let bands_db = aircraft_lw_per_meter_z_db(2, GROUND_OPS_KIND_TAXI, 0, 18.0);
        let bands_lin = compute_aircraft_lw_per_meter_lin(2, GROUND_OPS_KIND_TAXI, 0, 18.0);
        for i in 0..NUM_BANDS {
            let expected = 10f64.powf(bands_db[i] / 10.0) as f32;
            assert!(
                (bands_lin[i] - expected).abs() / expected.max(1e-6) < 1e-5,
                "band {i}: lin={} vs expected={}",
                bands_lin[i],
                expected
            );
        }
    }

    #[test]
    fn degenerate_zero_speed_returns_all_zeros() {
        let bands = compute_aircraft_lw_per_meter_lin(2, GROUND_OPS_KIND_TAXI, 0, 0.0);
        assert_eq!(bands, [0.0; NUM_BANDS]);
        let bands = compute_gse_band_energy_lin(GSE_CLASS_MEDIUM, GROUND_OPS_KIND_TAXI, 0.0, 250.0);
        assert_eq!(bands, [0.0; NUM_BANDS]);
    }

    #[test]
    fn gse_degenerate_zero_length_returns_all_zeros() {
        let bands = compute_gse_band_energy_lin(GSE_CLASS_MEDIUM, GROUND_OPS_KIND_TAXI, 18.0, 0.0);
        assert_eq!(bands, [0.0; NUM_BANDS]);
    }

    #[test]
    fn a_weighted_total_lower_than_z_total_for_aircraft() {
        // Sanity: A-weighting subtracts ~25 dB from 63 Hz and adds ~1 dB
        // to 2 kHz. For aircraft taxi spectrum (peak at 63 Hz),
        // A-weighted total is lower than Z-weighted by several dB.
        let bands = aircraft_lw_per_meter_z_db(2, GROUND_OPS_KIND_TAXI, 0, 18.0);
        let z = sum_z_band_levels_db(&bands);
        let a = sum_a_weighted_band_levels_db(&bands);
        assert!(
            a < z,
            "A-weighted total ({a}) must be < Z-weighted total ({z})"
        );
    }

    #[test]
    #[should_panic(expected = "ops_kind must be RUNWAY_ROLL/TAXI/APRON_MOVEMENT")]
    fn unknown_ops_kind_panics_aircraft() {
        let _ = aircraft_lw_per_meter_z_db(2, 0, 0, 18.0);
    }

    #[test]
    #[should_panic(expected = "ops_kind must be RUNWAY_ROLL/TAXI/APRON_MOVEMENT")]
    fn unknown_ops_kind_panics_gse() {
        // The GSE entrypoint validates ops_kind even though the GSE
        // kinematic math doesn't read it — keeps the writer contract
        // symmetric across veh_kind branches.
        let _ = compute_gse_band_energy_lin(GSE_CLASS_MEDIUM, 0, 10.0, 250.0);
    }

    #[test]
    #[should_panic(expected = "class_idx 99 out of range")]
    fn aircraft_class_idx_out_of_range_panics() {
        let _ = aircraft_lw_per_meter_z_db(99, GROUND_OPS_KIND_TAXI, 0, 18.0);
    }

    #[test]
    #[should_panic(expected = "gse_class 99 out of range")]
    fn gse_class_idx_out_of_range_panics() {
        let _ = gse_band_sel_z_db(99, 9.72, 250.0);
    }
}
