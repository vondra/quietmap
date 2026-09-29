//! Doc 29 scaled distance `d_λ = d0 · 10^((SEL − LAmax)/10)` per class anchor, tabulated on the NPD LUT grid.

use super::{FT_PER_M, LOG_DIST, NPD_LUT_BINS, NPD_LUT_LOG_MIN, NPD_LUT_STEP};

/// Metres per second per knot.
const M_PER_S_PER_KT: f64 = 1852.0 / 3600.0;
/// SEL reference duration `t0` (Doc 29 §4.2), seconds.
const SEL_REFERENCE_DURATION_S: f64 = 1.0;
/// The profile generator fills a missing ANP LAmax curve with `SEL − 12 dB` on
/// every row. Such a curve carries no event duration, so `d_λ` falls back to the
/// slant distance itself (Doc 29 App. E, the dipole limit `d_λ = d_p`).
const GENERATOR_PLACEHOLDER_SEL_MINUS_LAMAX_DB: f64 = 12.0;

/// `d_λ` (m) at each LUT bin for one NPD curve pair (Doc 29 Eq. 4-11,
/// `d0 = (2/π) · V_ref · t0`). SEL and LAmax interpolate linearly in `log d`
/// between NPD rows and extrapolate with the slope of their two nearest rows
/// (Eq. 4-5a/b), so `ΔL = SEL − LAmax` is piecewise linear in `log d` on both
/// sides of the table. The energy tail of `interpolate_sel_logd` beyond
/// 25,000 ft is a separate choice and does not change `ΔL` here.
pub fn build_scaled_distance_curve_lut(
    sel: &[f64; 10],
    lmax: &[f64; 10],
    v_ref_kt: f64,
) -> [f64; NPD_LUT_BINS + 1] {
    std::array::from_fn(|bin| {
        scaled_distance_at(
            sel,
            lmax,
            v_ref_kt,
            NPD_LUT_LOG_MIN + bin as f64 * NPD_LUT_STEP,
        )
    })
}

fn scaled_distance_at(sel: &[f64; 10], lmax: &[f64; 10], v_ref_kt: f64, log_d: f64) -> f64 {
    let delta: [f64; 10] = std::array::from_fn(|row| sel[row] - lmax[row]);
    if delta
        .iter()
        .all(|&d| (d - GENERATOR_PLACEHOLDER_SEL_MINUS_LAMAX_DB).abs() < 1e-9)
    {
        return 10.0_f64.powf(log_d) / FT_PER_M;
    }
    let d0_m = 2.0 / std::f64::consts::PI * v_ref_kt * M_PER_S_PER_KT * SEL_REFERENCE_DURATION_S;
    let upper = (1..LOG_DIST.len() - 1)
        .find(|&row| log_d < LOG_DIST[row])
        .unwrap_or(LOG_DIST.len() - 1);
    let fraction = (log_d - LOG_DIST[upper - 1]) / (LOG_DIST[upper] - LOG_DIST[upper - 1]);
    let delta_db = delta[upper - 1] + fraction * (delta[upper] - delta[upper - 1]);
    d0_m * 10.0_f64.powf(delta_db / 10.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emission::aircraft::npd::{
        fast_npd_lookup, noise_class_of, profile_idx, NpdLuts, PROFILES,
    };
    use crate::emission::aircraft::thrust::{thrust_model_for_class, MAX_POWER_ROWS};

    #[test]
    fn b738_departure_scaled_distance_follows_sel_minus_lamax_and_extrapolates() {
        let profile = profile_idx("B738");
        let anchor = &PROFILES[profile as usize];
        let lut = build_scaled_distance_curve_lut(
            &anchor.departure_sel,
            &anchor.departure_lmax,
            anchor.v_ref_kt,
        );
        for (feet, expected) in [
            (1_000.0_f64, 288.0),
            (10_000.0, 2_566.5),
            (25_000.0, 4_259.3),
            (50_000.0, 6_780.3),
        ] {
            let exact = scaled_distance_at(
                &anchor.departure_sel,
                &anchor.departure_lmax,
                anchor.v_ref_kt,
                feet.log10(),
            );
            assert!(
                (exact - expected).abs() < 0.15,
                "{feet} ft exact: {exact} m"
            );
            let actual = fast_npd_lookup(&lut, feet.log10());
            assert!(
                (10.0 * (actual / exact).log10()).abs() < 0.02,
                "{feet} ft: {actual} m"
            );
            // The max departure power row carries the anchor's max-thrust
            // curve, so its LUT bin matches the anchor LUT bit for bit.
            let class = noise_class_of(profile) as usize;
            let max_row = thrust_model_for_class(class).dep_rows - 1;
            assert_eq!(
                actual,
                NpdLuts::shared().lookup_scaled_distance(class, true, max_row, 0.0, feet.log10())
            );
        }
    }

    #[test]
    fn device_lut_blocks_match_both_cpu_metrics_for_every_class_and_row() {
        use crate::emission::aircraft::npd::NUM_CLASSES;
        let luts = NpdLuts::shared();
        let flat = luts.device_luts_flat_f64();
        assert_eq!(
            flat.len(),
            4 * NUM_CLASSES * MAX_POWER_ROWS * (NPD_LUT_BINS + 1)
        );
        let stride = MAX_POWER_ROWS * (NPD_LUT_BINS + 1);
        for class in 0..NUM_CLASSES {
            for departure in [false, true] {
                for row in 0..MAX_POWER_ROWS {
                    for bin in [0, 1, NPD_LUT_BINS / 2, NPD_LUT_BINS - 1, NPD_LUT_BINS] {
                        let log_d = NPD_LUT_LOG_MIN + bin as f64 * NPD_LUT_STEP;
                        let index = (usize::from(departure) * NUM_CLASSES + class) * stride
                            + row * (NPD_LUT_BINS + 1)
                            + bin;
                        assert_eq!(
                            flat[index],
                            luts.lookup(class, departure, row as u8, 0.0, log_d)
                        );
                        assert_eq!(
                            flat[index + 2 * NUM_CLASSES * stride],
                            luts.lookup_scaled_distance(class, departure, row as u8, 0.0, log_d)
                        );
                    }
                }
            }
        }
    }
}
