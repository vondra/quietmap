//! Today's single-ray ground, terrain and screening terms resolved per meteorological state,
//! with the audit's method switches (#7 favourable ground floor, #27 diffraction slope).
//! `Method::TODAY` mixed at `P_FAV` reproduces the production kernel; `validate_against_production`
//! measures that on every ray the oracle evaluates.

use crate::edges::RayGeometry;
use noise_compute::constants::{BAND_FREQ, GROUND_HARD_FLOOR_DB, P_FAV, SPEED_OF_SOUND};
use noise_compute::propagation::iso9613::{
    ground_atten_bands, ground_or_barrier_db, GroundPath, CNOSSOS_GROUND_ALPHA0,
    CNOSSOS_GROUND_DELTA_ZT_COEFF, CNOSSOS_GROUND_SHORT_PATH_FACTOR,
};
use noise_compute::propagation::obstacle_index::CrossingCandidate;
use noise_compute::propagation::path_effects::{self, ObstacleInput};
use noise_compute::propagation::PathProfile;
use noise_compute::types::NUM_BANDS;

pub type Bands = [f64; NUM_BANDS];

/// The two audit switches that act inside one ray.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Method {
    /// #7: CNOSSOS 2015/996 (2.5.20) favourable lower bound `−3(1−G′)(1 + 2(1 − 30(zs+zr)/dp))`
    /// on unmodified heights, also the whole favourable ground term when `Gpath = 0`.
    pub favourable_ground_floor: bool,
    /// #27: slope of the δ ≥ 0 arm, 20 (ISO 9613-2, today) or 40 (CNOSSOS 2.5.21).
    pub diffraction_positive_slope: f64,
}

impl Method {
    pub const TODAY: Method = Method {
        favourable_ground_floor: false,
        diffraction_positive_slope: 20.0,
    };
}

/// One term in the homogeneous and the favourable state.
#[derive(Debug, Clone, Copy, Default)]
pub struct StateBands {
    pub homogeneous: Bands,
    pub favourable: Bands,
}

impl StateBands {
    /// The engine's CNOSSOS (2.5.9) energy mix at `P_FAV`.
    pub fn mixed(&self) -> Bands {
        std::array::from_fn(|band| {
            let energy = P_FAV * 10f64.powf(-self.favourable[band] / 10.0)
                + (1.0 - P_FAV) * 10f64.powf(-self.homogeneous[band] / 10.0);
            -10.0 * energy.log10()
        })
    }

    pub fn state(&self, favourable: bool) -> Bands {
        if favourable {
            self.favourable
        } else {
            self.homogeneous
        }
    }
}

/// Everything one ray contributes besides divergence and air absorption.
#[derive(Debug, Clone)]
pub struct RayTerms {
    pub ground: StateBands,
    pub terrain: StateBands,
    /// Combined single-edge attenuation of each vector crossing, per state.
    pub crossings: Vec<StateBands>,
    pub vegetation: Bands,
}

impl RayTerms {
    /// Production screening increment: envelope of `max(0, C_j − T)` on MIXED values.
    pub fn screening_mixed(&self) -> Bands {
        let terrain = self.terrain.mixed();
        envelope(self.crossings.iter().map(StateBands::mixed), &terrain)
    }

    /// Today's composite: each term mixed first, then `max(A_ground, A_terrain + A_screen)`.
    pub fn barrier_or_ground_mixed_first(&self) -> Bands {
        let (ground, terrain, screening) = (self.ground.mixed(), self.terrain.mixed(), self.screening_mixed());
        std::array::from_fn(|band| ground_or_barrier_db(ground[band], terrain[band], screening[band]))
    }

    /// The same composite inside one meteorological state (the L_H / L_F of a test case).
    pub fn barrier_or_ground_in_state(&self, favourable: bool) -> Bands {
        let terrain = self.terrain.state(favourable);
        let screening = envelope(self.crossings.iter().map(|c| c.state(favourable)), &terrain);
        let ground = self.ground.state(favourable);
        std::array::from_fn(|band| ground_or_barrier_db(ground[band], terrain[band], screening[band]))
    }
}

fn envelope(crossings: impl Iterator<Item = Bands>, terrain: &Bands) -> Bands {
    let mut screen: Bands = [0.0; NUM_BANDS];
    for combined in crossings {
        for band in 0..NUM_BANDS {
            screen[band] = screen[band].max((combined[band] - terrain[band]).max(0.0));
        }
    }
    screen
}

/// All per-state terms of one sampled ray (`profile` from source to receiver).
pub fn ray_terms(
    profile: &PathProfile,
    candidates: &[CrossingCandidate],
    source_altitude_m: f64,
    receiver_altitude_m: f64,
    ground_path: GroundPath,
    method: Method,
) -> RayTerms {
    let ground = StateBands {
        homogeneous: ground_state_bands(ground_path, false, method),
        favourable: ground_state_bands(ground_path, true, method),
    };
    let geometry = RayGeometry::new(profile, source_altitude_m, receiver_altitude_m);
    let terrain = geometry
        .as_ref()
        .and_then(|g| g.terrain_edge(profile, source_altitude_m, receiver_altitude_m))
        .map_or_else(StateBands::default, |edge| edge.bands(method));
    let crossings = match &geometry {
        Some(g) if !candidates.is_empty() => candidates
            .iter()
            .map(|candidate| g.crossing_edge(candidate).bands(method))
            .collect(),
        _ => Vec::new(),
    };
    RayTerms {
        ground,
        terrain,
        crossings,
        vegetation: path_effects::vegetation_attenuation_path(profile),
    }
}

/// Largest |replica − production| [dB] of the mixed ground, terrain and screening bands.
pub fn validate_against_production(
    profile: &mut PathProfile,
    candidates: &[CrossingCandidate],
    source_altitude_m: f64,
    receiver_altitude_m: f64,
    ground_path: GroundPath,
    terms: &RayTerms,
) -> f64 {
    let ground = ground_atten_bands(ground_path);
    let terrain = path_effects::terrain_attenuation(profile, source_altitude_m, receiver_altitude_m);
    let screening = path_effects::screening_attenuation(
        profile,
        ObstacleInput { candidates },
        source_altitude_m,
        receiver_altitude_m,
        0.0,
        &terrain,
    );
    let pairs = [
        (terms.ground.mixed(), ground),
        (terms.terrain.mixed(), terrain),
        (terms.screening_mixed(), screening),
    ];
    pairs
        .iter()
        .flat_map(|(ours, theirs)| (0..NUM_BANDS).map(move |b| (ours[b] - theirs[b]).abs()))
        .fold(0.0, f64::max)
}

/// `iso9613::ground_state_db` per state, plus the #7 favourable floor when switched on.
fn ground_state_bands(path: GroundPath, favourable: bool, method: Method) -> Bands {
    let height_sum = path.zs_h_m + path.zr_h_m;
    let test_form = path.dp_m / (CNOSSOS_GROUND_SHORT_PATH_FACTOR * height_sum);
    let g_prime = if test_form <= 1.0 {
        path.ground_path_g * test_form + path.source_ground_g * (1.0 - test_form)
    } else {
        path.ground_path_g
    };
    let floor_factor = if favourable && method.favourable_ground_floor && test_form > 1.0 {
        1.0 + 2.0 * (1.0 - 1.0 / test_form)
    } else {
        1.0
    };
    if path.ground_path_g == 0.0 {
        let floor = if favourable && method.favourable_ground_floor {
            GROUND_HARD_FLOOR_DB * (1.0 - g_prime) * floor_factor
        } else {
            GROUND_HARD_FLOOR_DB
        };
        return [floor; NUM_BANDS];
    }
    let (zs, zr, gw) = if favourable {
        let delta_zt = CNOSSOS_GROUND_DELTA_ZT_COEFF * path.dp_m / height_sum;
        let half_dp_sq = path.dp_m * path.dp_m * 0.5;
        (
            path.zs_h_m + CNOSSOS_GROUND_ALPHA0 * (path.zs_h_m / height_sum).powi(2) * half_dp_sq + delta_zt,
            path.zr_h_m + CNOSSOS_GROUND_ALPHA0 * (path.zr_h_m / height_sum).powi(2) * half_dp_sq + delta_zt,
            path.ground_path_g,
        )
    } else {
        (path.zs_h_m, path.zr_h_m, g_prime)
    };
    let gw13 = gw.powf(1.3);
    let gw26 = gw13 * gw13;
    let floor = GROUND_HARD_FLOOR_DB * (1.0 - g_prime) * floor_factor;
    std::array::from_fn(|band| {
        let f = BAND_FREQ[band];
        let k = 2.0 * std::f64::consts::PI * f / SPEED_OF_SOUND;
        let w = 0.0185 * f.powf(2.5) * gw26 / (f.powf(1.5) * gw26 + 1.3e3 * f.powf(0.75) * gw13 + 1.16e6);
        let wd = w * path.dp_m;
        let cf = path.dp_m * (1.0 + 3.0 * wd * (-wd.sqrt()).exp()) / (1.0 + wd);
        let image = (zs * zs - (2.0 * cf / k).sqrt() * zs + cf / k) * (zr * zr - (2.0 * cf / k).sqrt() * zr + cf / k);
        let analytic = -10.0 * (4.0 * k * k / (path.dp_m * path.dp_m) * image).log10();
        analytic.max(floor)
    })
}
