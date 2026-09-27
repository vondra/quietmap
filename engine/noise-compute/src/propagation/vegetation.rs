//! ISO 9613-2:2024 Annex A.2.2 — foliage attenuation from the ray's metres in canopy.
//!
//! The depth is measured along each state's direct ray (straight in the homogeneous state,
//! the Γ arc in the favourable one), not in plan view: every profile interval contributes
//! its slant length times the fraction of its ends inside the canopy volume (above the
//! bare-earth ground, at or below ground plus canopy height) times the mean forest cover.
//! The CUDA foliage kernel walks the same samples. A missing canopy height records a
//! fault in CheckedRasters, which substitutes finite fallbacks so the computation
//! completes and then refuses the click before anything publishes.

use crate::propagation::cnossos::ground::MeteorologicalState;
use crate::propagation::cnossos::rubber_band::StateRay;
use crate::propagation::path_profile::PathProfile;
use crate::types::{ForestRun, NUM_BANDS};

/// Table A.1 row 1: fixed attenuation for 10–20 m of foliage [dB].
pub const FOLIAGE_SHORT_DB: [f64; NUM_BANDS] = [0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 2.0, 3.0];
/// Table A.1 rows 20–200 m: attenuation rate [dB/m].
pub const FOLIAGE_DB_PER_M: [f64; NUM_BANDS] = [0.02, 0.03, 0.04, 0.05, 0.06, 0.08, 0.09, 0.12];
/// Below this depth the table gives no attenuation [m].
pub const FOLIAGE_MIN_DEPTH_M: f64 = 10.0;
/// At and above this depth the per-metre rate applies [m].
pub const FOLIAGE_RATE_DEPTH_M: f64 = 20.0;
/// The rate stops accumulating past this depth [m].
pub const FOLIAGE_MAX_DEPTH_M: f64 = 200.0;

/// Table A.1 literally: nothing below 10 m, the short row below 20 m, the rate times the
/// depth (capped at 200 m) above. The table's own step at 20 m stands (row 1 against the
/// rate times 20).
pub fn foliage_attenuation(depth_m: f64) -> [f64; NUM_BANDS] {
    if depth_m < FOLIAGE_MIN_DEPTH_M {
        return [0.0; NUM_BANDS];
    }
    if depth_m < FOLIAGE_RATE_DEPTH_M {
        return FOLIAGE_SHORT_DB;
    }
    let capped = depth_m.min(FOLIAGE_MAX_DEPTH_M);
    std::array::from_fn(|band| FOLIAGE_DB_PER_M[band] * capped)
}

/// Mix the two state attenuations at favourable probability `p` (energy mix, CNOSSOS 2.5.9).
pub fn mixed_foliage_bands(homogeneous_db: &[f64; NUM_BANDS], favourable_db: &[f64; NUM_BANDS], p: f64) -> [f64; NUM_BANDS] {
    std::array::from_fn(|band| {
        -10.0 * (p * 10f64.powf(-favourable_db[band] / 10.0) + (1.0 - p) * 10f64.powf(-homogeneous_db[band] / 10.0)).log10()
    })
}

/// Cover-weighted ray metres inside the canopy volume on one state's direct ray. NaN when
/// any sampled canopy height is missing.
pub fn canopy_depth_on_ray(
    profile: &PathProfile,
    source_altitude_m: f64,
    receiver_altitude_m: f64,
    state: MeteorologicalState,
) -> f64 {
    canopy_intervals(profile, source_altitude_m, receiver_altitude_m, state).map(|(_, _, len)| len).sum()
}

/// Maximal runs of profile intervals the homogeneous ray spends in canopy, for the popup
/// trace: fractional start/end with the cover-weighted ray metres of each run.
pub fn canopy_runs_on_ray(
    profile: &PathProfile,
    source_altitude_m: f64,
    receiver_altitude_m: f64,
) -> Vec<ForestRun> {
    let mut runs = Vec::new();
    let mut open: Option<ForestRun> = None;
    let flush = |open: &mut Option<ForestRun>, runs: &mut Vec<ForestRun>| {
        if let Some(run) = open.take() {
            runs.push(run);
        }
    };
    for (t0, t1, len) in canopy_intervals(
        profile,
        source_altitude_m,
        receiver_altitude_m,
        MeteorologicalState::Homogeneous,
    ) {
        if len > 0.0 {
            match open.as_mut() {
                Some(run) => {
                    run.t_end = t1;
                    run.len_m += len;
                }
                None => open = Some(ForestRun { t_start: t0, t_end: t1, len_m: (len * 10.0).round() / 10.0 }),
            }
        } else {
            flush(&mut open, &mut runs);
        }
    }
    flush(&mut open, &mut runs);
    runs
}

/// Per profile interval: fractional start/end and cover-weighted ray metres in canopy
/// (NaN when a sampled canopy height is missing).
fn canopy_intervals(
    profile: &PathProfile,
    source_altitude_m: f64,
    receiver_altitude_m: f64,
    state: MeteorologicalState,
) -> impl Iterator<Item = (f64, f64, f64)> + '_ {
    let ray = StateRay::between(state, (0.0, source_altitude_m), (profile.dist_m, receiver_altitude_m));
    let source = (0.0, source_altitude_m);
    let receiver = (profile.dist_m, receiver_altitude_m);
    (1..profile.t.len()).map(move |i| {
        let (t0, t1) = (profile.t[i - 1], profile.t[i]);
        let (x0, x1) = (t0 * profile.dist_m, t1 * profile.dist_m);
        let (r0, r1) = (ray.altitude_at(source, receiver, x0), ray.altitude_at(source, receiver, x1));
        let ray_altitude = |k: usize| if k == i - 1 { r0 } else { r1 };
        let inside = |k: usize| {
            let ground = f64::from(profile.elevation_m[k]);
            let top = ground + f64::from(profile.canopy_m[k]);
            ground < ray_altitude(k) && ray_altitude(k) <= top
        };
        let fraction = f64::from(u8::from(inside(i - 1)) + u8::from(inside(i))) / 2.0;
        let cover = (f64::from(profile.forest_u8[i - 1].min(100)) + f64::from(profile.forest_u8[i].min(100))) / 200.0;
        let slant = ((x1 - x0).powi(2) + (r1 - r0).powi(2)).sqrt();
        let missing = !profile.canopy_m[i - 1].is_finite() || !profile.canopy_m[i].is_finite();
        (t0, t1, if missing { f64::NAN } else { fraction * cover * slant })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::propagation::path_profile::fill_t_values;

    fn flat_profile(dist_m: f64, canopy_m: f32, cover: u8) -> PathProfile {
        let mut t = Vec::new();
        fill_t_values(dist_m, &mut t);
        let n = t.len();
        PathProfile {
            t,
            elevation_m: vec![0.0; n],
            canopy_m: vec![canopy_m; n],
            forest_u8: vec![cover; n],
            imd_u8: vec![0; n],
            dist_m,
            step_m_med: 0.0,
            src_lat: 0.0,
            src_lon: 0.0,
            rcv_lat: 0.0,
            rcv_lon: 0.0,
            elevation_f64_scratch: Vec::new(),
        }
    }

    #[test]
    fn the_table_gives_nothing_below_10m() {
        assert_eq!(foliage_attenuation(0.0), [0.0; NUM_BANDS]);
        assert_eq!(foliage_attenuation(9.99), [0.0; NUM_BANDS]);
    }

    #[test]
    fn the_table_steps_through_its_short_row_and_rate() {
        assert_eq!(foliage_attenuation(10.0), FOLIAGE_SHORT_DB);
        assert_eq!(foliage_attenuation(19.99), FOLIAGE_SHORT_DB);
        let at_rate_start = foliage_attenuation(20.0);
        assert!((at_rate_start[2] - 0.8).abs() < 1e-12, "{at_rate_start:?}");
        let capped = foliage_attenuation(250.0);
        assert!((capped[7] - 24.0).abs() < 1e-12, "{capped:?}");
        assert_eq!(capped, foliage_attenuation(200.0));
    }

    #[test]
    fn a_level_ray_in_full_canopy_collects_the_whole_path() {
        let profile = flat_profile(100.0, 20.0, 100);
        let depth = canopy_depth_on_ray(&profile, 10.0, 10.0, MeteorologicalState::Homogeneous);
        assert!((depth - 100.0).abs() < 1e-9, "{depth}");
    }

    #[test]
    fn a_ray_above_the_canopy_collects_nothing() {
        let profile = flat_profile(100.0, 20.0, 100);
        let depth = canopy_depth_on_ray(&profile, 25.0, 25.0, MeteorologicalState::Homogeneous);
        assert_eq!(depth, 0.0);
    }

    #[test]
    fn cover_weights_the_depth() {
        let profile = flat_profile(100.0, 20.0, 50);
        let depth = canopy_depth_on_ray(&profile, 10.0, 10.0, MeteorologicalState::Homogeneous);
        assert!((depth - 50.0).abs() < 1e-9, "{depth}");
    }

    #[test]
    fn the_favourable_arc_collects_less_than_the_chord_over_a_canopy_block() {
        // Ground at 0, canopy 10 m on the middle half only; the 9 m chord runs
        // inside it while the favourable arc (3.1 m above the chord at 200 m)
        // flies over.
        let mut profile = flat_profile(200.0, 0.0, 100);
        for (k, t) in profile.t.clone().iter().enumerate() {
            if *t >= 0.25 && *t <= 0.75 {
                profile.canopy_m[k] = 10.0;
            }
        }
        let homogeneous = canopy_depth_on_ray(&profile, 9.0, 9.0, MeteorologicalState::Homogeneous);
        let favourable = canopy_depth_on_ray(&profile, 9.0, 9.0, MeteorologicalState::Favourable);
        assert!((homogeneous - 100.0).abs() < 2.0, "{homogeneous}");
        assert_eq!(favourable, 0.0);
    }

    #[test]
    fn missing_canopy_poisons_the_depth() {
        let mut profile = flat_profile(100.0, 20.0, 100);
        profile.canopy_m[3] = f32::NAN;
        let depth = canopy_depth_on_ray(&profile, 10.0, 10.0, MeteorologicalState::Homogeneous);
        assert!(depth.is_nan(), "{depth}");
    }

    #[test]
    fn runs_group_contiguous_intervals() {
        let mut profile = flat_profile(200.0, 0.0, 100);
        for (k, t) in profile.t.clone().iter().enumerate() {
            if *t >= 0.25 && *t <= 0.75 {
                profile.canopy_m[k] = 10.0;
            }
        }
        let runs = canopy_runs_on_ray(&profile, 5.0, 5.0);
        assert_eq!(runs.len(), 1);
        assert!((runs[0].len_m - 100.0).abs() < 2.0, "{runs:?}");
    }
}
