//! Ground attenuation of one (sub-)path per meteorological state: CNOSSOS-EU (2.5.14)–(2.5.20)
//! with the favourable lower bound of (2.5.20) on unmodified heights (2021/1226; ISO/TR 17534-4
//! §5.8).

use super::mean_plane::EquivalentGeometry;
use crate::constants::{BAND_FREQ, SPEED_OF_SOUND};
use crate::types::NUM_BANDS;

/// (2.5.14): a path shorter than this many times its height sum blends in the source ground.
pub const SHORT_PATH_HEIGHT_FACTOR: f64 = 30.0;
/// (2.5.19) favourable-ray curvature coefficient a₀ [1/m].
pub const FAVOURABLE_CURVATURE_A0_PER_M: f64 = 2e-4;
/// (2.5.19) terrain-height term δz_T coefficient.
pub const FAVOURABLE_TERRAIN_HEIGHT_COEFFICIENT: f64 = 6e-3;
/// Height-sum guard of METHOD.md §2.2: below one millimetre the ratios of (2.5.14), (2.5.19)
/// and (2.5.20) use one millimetre.
const MINIMUM_HEIGHT_SUM_M: f64 = 1e-3;

/// One of the two CNOSSOS-EU meteorological states (2.5.5 homogeneous, 2.5.7 favourable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeteorologicalState {
    Homogeneous,
    Favourable,
}

/// Ground factors of one (sub-)path in Table 2.5.b's roles: `path` is Ḡpath (it also decides
/// the hard-ground branch), `impedance` is Ḡw inside (2.5.17), `floor` is Ḡm of the lower
/// bound.
#[derive(Debug, Clone, Copy)]
pub struct GroundFactors {
    pub path: f64,
    pub impedance: f64,
    pub floor: f64,
}

/// (2.5.14) G′path of a path whose source end stands on ground `source_ground_factor`.
pub fn ground_factor_near_source(
    geometry: &EquivalentGeometry,
    path_ground_factor: f64,
    source_ground_factor: f64,
) -> f64 {
    let height_sum = (geometry.source_side_height_m + geometry.receiver_side_height_m)
        .max(MINIMUM_HEIGHT_SUM_M);
    let ratio = geometry.projected_distance_m / (SHORT_PATH_HEIGHT_FACTOR * height_sum);
    if ratio <= 1.0 {
        path_ground_factor * ratio + source_ground_factor * (1.0 - ratio)
    } else {
        path_ground_factor
    }
}

/// (2.5.15)–(2.5.18) analytic term for heights `zs`, `zr` over `dp` with impedance factor `gw`.
fn analytic_ground_db(band: usize, dp: f64, zs: f64, zr: f64, gw: f64) -> f64 {
    let f = BAND_FREQ[band];
    let k = 2.0 * std::f64::consts::PI * f / SPEED_OF_SOUND;
    let gw13 = gw.max(0.0).powf(1.3);
    let gw26 = gw13 * gw13;
    let w = 0.0185 * f.powf(2.5) * gw26 / (f.powf(1.5) * gw26 + 1.3e3 * f.powf(0.75) * gw13 + 1.16e6);
    let wd = w * dp;
    let cf = dp * (1.0 + 3.0 * wd * (-wd.sqrt()).exp()) / (1.0 + wd);
    let root = (2.0 * cf / k).sqrt();
    let product = (zs * zs - root * zs + cf / k) * (zr * zr - root * zr + cf / k);
    -10.0 * (4.0 * k * k / (dp * dp) * product).log10()
}

/// (2.5.20) lower bound of the favourable state, on the unmodified heights.
fn favourable_floor_db(geometry: &EquivalentGeometry, floor_factor: f64) -> f64 {
    let height_sum = (geometry.source_side_height_m + geometry.receiver_side_height_m)
        .max(MINIMUM_HEIGHT_SUM_M);
    let base = -3.0 * (1.0 - floor_factor);
    let limit = SHORT_PATH_HEIGHT_FACTOR * height_sum;
    if geometry.projected_distance_m <= limit {
        base
    } else {
        base * (1.0 + 2.0 * (1.0 - limit / geometry.projected_distance_m))
    }
}

/// A_ground of one (sub-)path in one state, per octave band.
pub fn ground_attenuation_bands(
    geometry: &EquivalentGeometry,
    factors: GroundFactors,
    state: MeteorologicalState,
) -> [f64; NUM_BANDS] {
    let dp = geometry.projected_distance_m.max(1e-6);
    let (zs, zr) = (geometry.source_side_height_m, geometry.receiver_side_height_m);
    match state {
        MeteorologicalState::Homogeneous => {
            if factors.path == 0.0 {
                return [-3.0; NUM_BANDS];
            }
            let floor = -3.0 * (1.0 - factors.floor);
            std::array::from_fn(|band| analytic_ground_db(band, dp, zs, zr, factors.impedance).max(floor))
        }
        MeteorologicalState::Favourable => {
            let floor = favourable_floor_db(geometry, factors.floor);
            if factors.path == 0.0 {
                return [floor; NUM_BANDS];
            }
            let height_sum = (zs + zr).max(MINIMUM_HEIGHT_SUM_M);
            let curvature = FAVOURABLE_CURVATURE_A0_PER_M * dp * dp / 2.0;
            let terrain = FAVOURABLE_TERRAIN_HEIGHT_COEFFICIENT * dp / height_sum;
            let zs_favourable = zs + curvature * (zs / height_sum).powi(2) + terrain;
            let zr_favourable = zr + curvature * (zr / height_sum).powi(2) + terrain;
            std::array::from_fn(|band| {
                analytic_ground_db(band, dp, zs_favourable, zr_favourable, factors.impedance).max(floor)
            })
        }
    }
}
