//! Per-airport `AircraftGroundOpsDetail` popup payload builder — folds the
//! airport ground-ops accumulator + `airport_summary.arrow` counts into the popup struct.
use super::*;

/// Build the per-airport `AircraftGroundOpsDetail` payload from the
/// accumulator. Caller passes the already-computed `NoisePeriods` so
/// the per-period Leqs don't get recomputed.
///
/// `summary_entry` is the `airport_summary.arrow` UNION counts for
/// this airport (v5). When `None`, the popup MUST NOT silently fall
/// back to per-row sum — per-row sum would over-count rotations
/// crossing N microsegments by ~N×. Return zeros so the frontend
/// renders "—" or hides the row.
pub(super) fn build_ground_ops_metadata(
    acc: &AirportAcc,
    periods: &crate::types::NoisePeriods,
    n_days_f: f64,
    // GA-class divisor for the split-union counts;
    // non-GA counts divide by `n_days_f`.
    ga_n_days_f: f64,
    summary_entry: Option<AirportSummaryEntry>,
) -> AircraftGroundOpsDetail {
    // v5: arr/dep/gse/observed counts come from the global
    // `airport_summary.arrow` sidecar (UNION across all R4s).
    // Missing summary = popup refuses to display arr/dep — see
    // function docstring. v9: each split count = `non_ga / n_days +
    // ga / ga_n_days` so a one-off GA rotation reads at its true
    // full-year frequency.
    let (
        arrivals_per_day,
        departures_per_day,
        gse_per_day,
        observed_movements_per_day,
        runway_ops,
        taxi_ops,
        apron_ops,
    ) = if let Some(entry) = summary_entry {
        let split = |non_ga: u32, ga: u32| non_ga as f64 / n_days_f + ga as f64 / ga_n_days_f;
        let arr = split(entry.arr_count, entry.ga_arr_count);
        let dep = split(entry.dep_count, entry.ga_dep_count);
        // GSE is airline-pass only — no GA split.
        let gse: [f64; NUM_GSE_CLASSES] =
            std::array::from_fn(|i| entry.gse_count_per_class[i] as f64 / n_days_f);
        // observed_movements_per_day = runway ops (arr ∪ dep). The
        // summary's `ops_count_per_kind[0]` is already the airport-
        // level runway UNION (VEH_KIND=0); use it instead of
        // recomputing from arr + dep so a fid that contributed both
        // arrival AND departure rotations in n_days dedupes once.
        let observed = split(entry.ops_count_per_kind[0], entry.ga_ops_count_per_kind[0]);
        let runway = split(entry.ops_count_per_kind[0], entry.ga_ops_count_per_kind[0]);
        let taxi = split(entry.ops_count_per_kind[1], entry.ga_ops_count_per_kind[1]);
        let apron = split(entry.ops_count_per_kind[2], entry.ga_ops_count_per_kind[2]);
        (arr, dep, gse, observed, runway, taxi, apron)
    } else {
        // No sidecar → return zeros. The FE renders `arrivals_per_day
        // == 0` as a hidden row, matching the "no ADS-B data" state.
        (0.0, 0.0, [0.0; NUM_GSE_CLASSES], 0.0, 0.0, 0.0, 0.0)
    };

    // profile_mix: top-N aircraft classes by linear received energy
    // at this receiver. Share denominator is aircraft-only
    // (`class_energy` is gated on veh_kind==0), so GSE energy doesn't
    // dilute the per-class percentages — the popup renders GSE
    // separately via `gse_per_day`.
    //
    // Stable tiebreak on class_idx ascending mirrors the airborne
    // `build_top_flights` contract so two near-zero classes don't
    // flip-flop in display order across compute runs.
    let total_class_energy: f64 = acc.class_energy.iter().sum();
    let mut sorted_classes: Vec<(u8, f64)> = acc
        .class_energy
        .iter()
        .enumerate()
        .filter(|(_, e)| **e > 0.0)
        .map(|(i, e)| (i as u8, *e))
        .collect();
    sorted_classes.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    // `sorted_classes` is already filtered to `energy > 0`, so
    // `total_class_energy > 0` is implied — no guard needed.
    let profile_mix: Vec<ProfileMixEntry> = sorted_classes
        .into_iter()
        .take(PROFILE_MIX_TOP_N)
        .map(|(class_idx, energy)| {
            let rep_profile_idx = aircraft::CLASS_REP_PROFILE_IDX[class_idx as usize] as usize;
            ProfileMixEntry {
                class: class_idx,
                share: energy / total_class_energy,
                rep_typecode: aircraft::profile_typecode(rep_profile_idx).to_string(),
            }
        })
        .collect();

    // Per-ops-kind sub-periods. Each gets its own Lden combining
    // the class-typed periods that touched runway / taxi / apron
    // rows. When the kind had zero energy (no rows of that
    // ops_kind hit this airport), `period_leq(0.0, ...)` returns
    // `f64::NEG_INFINITY` which `serde_json` cannot serialize.
    // Short-circuit to `NoisePeriods::silence()` to keep the popup
    // payload encodable.
    let class_periods = |period_energy: [f64; 3]| -> NoisePeriods {
        let total: f64 = period_energy.iter().sum();
        if total <= 0.0 {
            return NoisePeriods::silence();
        }
        let ld = aircraft::period_leq(period_energy[0], n_days_f, aircraft::PERIOD_SECONDS[0]);
        let le = aircraft::period_leq(period_energy[1], n_days_f, aircraft::PERIOD_SECONDS[1]);
        let ln = aircraft::period_leq(period_energy[2], n_days_f, aircraft::PERIOD_SECONDS[2]);
        periods::periods(ld, le, ln)
    };

    let distance_m = if acc.sum_energy > 0.0 {
        acc.sum_energy_x_dist / acc.sum_energy
    } else {
        0.0
    };

    // Per-effect ΔL_A from the variant energy accumulators.
    // `period_energy_no_X[i]` is the A-weighted linear energy at
    // the receiver in period i if effect X were removed. To get
    // an Lden delta (not a raw energy delta), build per-variant
    // Lden via the standard 12h day + 4h × 10⁰·⁵ eve + 8h × 10¹
    // night weighting (`periods::periods` does this exactly) and
    // subtract from the full Lden. Negative for attenuating
    // effects.
    let lden_delta_db = |variant_period_energy: [f64; 3]| -> f64 {
        let v_ld = aircraft::period_leq(
            variant_period_energy[0],
            n_days_f,
            aircraft::PERIOD_SECONDS[0],
        );
        let v_le = aircraft::period_leq(
            variant_period_energy[1],
            n_days_f,
            aircraft::PERIOD_SECONDS[1],
        );
        let v_ln = aircraft::period_leq(
            variant_period_energy[2],
            n_days_f,
            aircraft::PERIOD_SECONDS[2],
        );
        let v_periods = periods::periods(v_ld, v_le, v_ln);
        if !periods.lden_db.is_finite() || !v_periods.lden_db.is_finite() {
            return 0.0;
        }
        periods.lden_db - v_periods.lden_db
    };
    let terrain_impact_db = lden_delta_db(acc.period_energy_no_terrain);
    let screening_impact_db = lden_delta_db(acc.period_energy_no_screening);
    let vegetation_impact_db = lden_delta_db(acc.period_energy_no_vegetation);
    let atmospheric_impact_db = lden_delta_db(acc.period_energy_no_atmospheric);
    let ground_impact_db = lden_delta_db(acc.period_energy_no_ground);

    // Emission Lw at the 25 m anchor — exact, taken from the per-row
    // A-weighted band sum at 25 m accumulated during the kernel loop
    // (`aw_band_sum_25m` pre-propagation). Avoids the Jensen-bias of
    // inverse-propagating receiver energy back through the weighted
    // centroid distance (which only matches for compact airports).
    // Divide by `n_days_f` because v6 `band_energy_lin` is raw Σ over
    // the extraction window; emission display shows daily-average
    // A-weighted energy at 25 m in dB.
    let emission_db = if acc.sum_energy_25m > 0.0 {
        10.0 * (acc.sum_energy_25m / n_days_f).log10()
    } else {
        0.0
    };

    AircraftGroundOpsDetail {
        periods: periods.clone(),
        periods_free: periods.clone(),
        observed_movements_per_day,
        modeled_movements_per_day: 0.0,
        distance_m,
        emission_db,
        received_bands: [0.0; NUM_BANDS],
        // Per-ops-kind unique movements come from the airport_summary
        // sidecar UNION (v5). When the sidecar is missing they're
        // zero — matches the arr/dep behaviour above.
        runway_roll: AircraftGroundOpsClassDetail {
            periods: class_periods(acc.runway_period_energy),
            observed_movements_per_day: runway_ops,
            modeled_movements_per_day: 0.0,
        },
        taxi: AircraftGroundOpsClassDetail {
            periods: class_periods(acc.taxi_period_energy),
            observed_movements_per_day: taxi_ops,
            modeled_movements_per_day: 0.0,
        },
        apron_movement: AircraftGroundOpsClassDetail {
            periods: class_periods(acc.apron_period_energy),
            observed_movements_per_day: apron_ops,
            modeled_movements_per_day: 0.0,
        },
        profile_mix,
        baseline: PropagationBaseline::default(),
        terrain: Default::default(),
        screening: Default::default(),
        vegetation: Default::default(),
        terrain_impact_db,
        screening_impact_db,
        vegetation_impact_db,
        atmospheric_impact_db,
        ground_impact_db,
        arrivals_per_day,
        departures_per_day,
        gse_per_day,
    }
}
