//! Foliage attenuation (ISO 9613-2:2024 Annex A.2.2, Table A.1) from the metres a state's direct
//! ray spends in canopy: every profile interval contributes its slant length times the fraction of
//! its ends inside the canopy volume times the mean canopy cover. The canopy stands a constant
//! [`CANOPY_HEIGHT_M`] above the ground wherever there is cover.

use crate::bands::BANDS;
use crate::cnossos::MeteorologicalState;
use crate::cnossos::rubber_band::StateRay;
use crate::profile::Profile;

/// Canopy top above the bare earth wherever cover is above zero (dev4's canopy raster moved the
/// level by a median 0.17 dB against a constant; PLAN-z13 SIMPLIFY).
pub const CANOPY_HEIGHT_M: f64 = 20.0;
/// Table A.1 row 1: fixed attenuation for 10-20 m of foliage (dB).
pub const FOLIAGE_SHORT_DB: [f64; BANDS] = [0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 2.0, 3.0];
/// Table A.1 rows 20-200 m: attenuation rate (dB/m).
pub const FOLIAGE_DB_PER_M: [f64; BANDS] = [0.02, 0.03, 0.04, 0.05, 0.06, 0.08, 0.09, 0.12];
/// Below this depth the table gives nothing (m).
pub const FOLIAGE_MIN_DEPTH_M: f64 = 10.0;
/// At and above this depth the per-metre rate applies (m).
pub const FOLIAGE_RATE_DEPTH_M: f64 = 20.0;
/// The rate stops accumulating past this depth (m).
pub const FOLIAGE_MAX_DEPTH_M: f64 = 200.0;

/// Table A.1 literally: nothing below 10 m, the short row below 20 m, the rate times the depth
/// (capped at 200 m) above; the table's own step at 20 m stands.
pub fn foliage_attenuation(depth_m: f64) -> [f64; BANDS] {
    if depth_m < FOLIAGE_MIN_DEPTH_M {
        return [0.0; BANDS];
    }
    if depth_m < FOLIAGE_RATE_DEPTH_M {
        return FOLIAGE_SHORT_DB;
    }
    let capped = depth_m.min(FOLIAGE_MAX_DEPTH_M);
    std::array::from_fn(|band| FOLIAGE_DB_PER_M[band] * capped)
}

/// Cover-weighted metres of one state's direct ray inside the canopy volume.
pub fn canopy_depth_on_ray(
    profile: &Profile,
    source_altitude_m: f64,
    receiver_altitude_m: f64,
    state: MeteorologicalState,
) -> f64 {
    let length = profile.horizontal_m;
    let (source, receiver) = ((0.0, source_altitude_m), (length, receiver_altitude_m));
    let ray = StateRay::between(state, source, receiver);
    let inside = |k: usize, ray_altitude: f64| {
        let ground = profile.ground_m[k];
        let top = if profile.forest_cover[k] > 0.0 {
            ground + CANOPY_HEIGHT_M
        } else {
            ground
        };
        ground < ray_altitude && ray_altitude <= top
    };
    let mut depth = 0.0;
    let mut previous = (0.0, ray.altitude_at(source, receiver, 0.0));
    for i in 1..profile.t.len() {
        let x = profile.t[i] * length;
        let altitude = ray.altitude_at(source, receiver, x);
        let fraction =
            f64::from(u8::from(inside(i - 1, previous.1)) + u8::from(inside(i, altitude))) / 2.0;
        if fraction > 0.0 {
            let cover = (profile.forest_cover[i - 1] + profile.forest_cover[i]) / 2.0;
            depth += fraction * cover * (x - previous.0).hypot(altitude - previous.1);
        }
        previous = (x, altitude);
    }
    depth
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_a1_steps_at_ten_and_twenty_metres_and_caps_at_two_hundred() {
        assert_eq!(foliage_attenuation(9.9), [0.0; BANDS]);
        assert_eq!(foliage_attenuation(15.0), FOLIAGE_SHORT_DB);
        assert!((foliage_attenuation(100.0)[4] - 6.0).abs() < 1e-12);
        assert_eq!(foliage_attenuation(500.0), foliage_attenuation(200.0));
    }

    fn forest(horizontal_m: f64, cover: f64) -> Profile {
        let mut profile = Profile::default();
        profile.reset(horizontal_m);
        let n = profile.t.len();
        profile.ground_m = vec![100.0; n];
        profile.ground_factor = vec![1.0; n];
        profile.forest_cover = vec![cover; n];
        profile
    }

    /// A 4 m ray through a full forest counts its whole length; half cover counts half; above the
    /// canopy nothing.
    #[test]
    fn depth_is_the_cover_weighted_ray_length_inside_the_canopy() {
        let full = canopy_depth_on_ray(
            &forest(300.0, 1.0),
            104.0,
            104.0,
            MeteorologicalState::Homogeneous,
        );
        assert!((full - 300.0).abs() < 1e-9, "{full}");
        let half = canopy_depth_on_ray(
            &forest(300.0, 0.5),
            104.0,
            104.0,
            MeteorologicalState::Homogeneous,
        );
        assert!((half - 150.0).abs() < 1e-9, "{half}");
        let above = canopy_depth_on_ray(
            &forest(300.0, 1.0),
            125.0,
            125.0,
            MeteorologicalState::Homogeneous,
        );
        assert_eq!(above, 0.0);
        let bare = canopy_depth_on_ray(
            &forest(300.0, 0.0),
            104.0,
            104.0,
            MeteorologicalState::Favourable,
        );
        assert_eq!(bare, 0.0);
    }
}
