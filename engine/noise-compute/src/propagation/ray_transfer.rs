//! Linear transfer of one source→receiver ray per period, band and popup variant (everything
//! but divergence and the receiver reflection) — the per-node physics of the line quadrature and
//! of point sources, shared by every popup kernel.

use super::iso9613::{ground_atten_bands, ground_or_barrier_db};
use super::obstacle_index::{CellPrune, CrossingCandidate, ObstacleSet};
use super::path_effects::{
    cnossos_ground_path_from_profile, screening_attenuation_with_meta,
    terrain_attenuation_with_meta, vegetation_attenuation_path, ObstacleInput,
};
use super::PathProfile;
use crate::constants::ALPHA_ATM;
use crate::types::{RasterSampler, ScreeningObstacleTrace, TerrainTrace, NUM_BANDS};

/// The popup's "what if this effect were off" hypotheses, in [`VariantBands`] order.
pub const VARIANT_COUNT: usize = 7;
pub const VARIANT_FULL: usize = 0;
pub const VARIANT_FREE_FIELD: usize = 1;
pub const VARIANT_NO_TERRAIN: usize = 2;
pub const VARIANT_NO_SCREENING: usize = 3;
pub const VARIANT_NO_VEGETATION: usize = 4;
pub const VARIANT_NO_GROUND: usize = 5;
pub const VARIANT_NO_ATMOSPHERE: usize = 6;

/// Linear transfer `10^(−A/10)` per variant and band.
pub type VariantBands = [[f64; NUM_BANDS]; VARIANT_COUNT];

/// One ray's transfer for each period (day, evening, night).
#[derive(Debug, Clone, Copy)]
pub struct RayTransfer {
    pub slant_distance_m: f64,
    pub periods: [VariantBands; 3],
}

/// The receiver end of every ray of one popup.
pub struct RayReceiver {
    pub lat: f64,
    pub lon: f64,
    pub altitude_m: f64,
}

/// The source end of one ray.
#[derive(Debug, Clone, Copy)]
pub struct RaySource {
    pub lat: f64,
    pub lon: f64,
    /// Height of the source above the ground under it.
    pub height_m: f64,
    /// A bridge deck: the path ground is hard.
    pub on_bridge: bool,
    /// Buildings nearer the source than this are its own footprint, not obstacles.
    pub exclusion_radius_m: f64,
}

/// Reusable per-thread buffers.
#[derive(Default)]
pub struct RayScratch {
    profile: PathProfile,
    candidates: Vec<CrossingCandidate>,
}

/// What the popup trace shows of one ray.
pub struct RayDetail {
    pub source_altitude_m: f64,
    pub ground_factor: f64,
    pub ground_bands: [f64; NUM_BANDS],
    pub terrain: TerrainTrace,
    pub screening_bands: [f64; NUM_BANDS],
    pub obstacle: ScreeningObstacleTrace,
    pub vegetation_bands: [f64; NUM_BANDS],
    pub profile: PathProfile,
}

/// The transfer of the ray from `source` to `receiver`; vector obstacles are read only when
/// `obstacles_on_ray` (a node in a clear gap of the line quadrature's mask has none).
pub fn evaluate_ray_transfer(
    receiver: &RayReceiver,
    source: &RaySource,
    obstacles: &ObstacleSet,
    obstacles_on_ray: bool,
    rasters: &dyn RasterSampler,
    scratch: &mut RayScratch,
    detail: Option<&mut Option<RayDetail>>,
) -> RayTransfer {
    let horizontal_m = grid::geo::flat_dist(source.lat, source.lon, receiver.lat, receiver.lon).max(1.0);
    let profile = &mut scratch.profile;
    rasters.build_path_profile(
        source.lat,
        source.lon,
        receiver.lat,
        receiver.lon,
        horizontal_m,
        profile,
    );
    let source_altitude = f64::from(profile.elevation_m[0]) + source.height_m;
    let slant = horizontal_m.hypot(receiver.altitude_m - source_altitude).max(1.0);
    let ground_path =
        cnossos_ground_path_from_profile(profile, source_altitude, receiver.altitude_m, source.on_bridge);
    let ground = ground_atten_bands(ground_path);
    let (terrain, _) = terrain_attenuation_with_meta(profile, source_altitude, receiver.altitude_m);
    scratch.candidates.clear();
    if obstacles_on_ray {
        obstacles.crossings_pruned(
            source.lat,
            source.lon,
            receiver.lat,
            receiver.lon,
            &CellPrune::for_profile(profile, source_altitude, receiver.altitude_m),
            &mut scratch.candidates,
        );
    }
    let (screening, obstacle) = screening_attenuation_with_meta(
        profile,
        ObstacleInput {
            candidates: &scratch.candidates,
        },
        source_altitude,
        receiver.altitude_m,
        source.exclusion_radius_m,
        &terrain.attenuation_bands,
    );
    let vegetation = vegetation_attenuation_path(profile);
    let atmosphere: [f64; NUM_BANDS] = std::array::from_fn(|band| ALPHA_ATM[band] * slant / 1000.0);
    let zero = [0.0; NUM_BANDS];
    let terrain_bands = terrain.attenuation_bands;
    let composite = |terrain: &[f64; NUM_BANDS], screening: &[f64; NUM_BANDS], band: usize| {
        ground_or_barrier_db(ground[band], terrain[band], screening[band])
    };
    let variants: VariantBands = [
        std::array::from_fn(|b| atmosphere[b] + composite(&terrain_bands, &screening, b) + vegetation[b]),
        std::array::from_fn(|b| atmosphere[b] + ground[b]),
        std::array::from_fn(|b| atmosphere[b] + composite(&zero, &screening, b) + vegetation[b]),
        std::array::from_fn(|b| atmosphere[b] + composite(&terrain_bands, &zero, b) + vegetation[b]),
        std::array::from_fn(|b| atmosphere[b] + composite(&terrain_bands, &screening, b)),
        std::array::from_fn(|b| {
            let barrier = terrain_bands[b] + screening[b];
            atmosphere[b] + if barrier > 0.0 { barrier } else { 0.0 } + vegetation[b]
        }),
        std::array::from_fn(|b| composite(&terrain_bands, &screening, b) + vegetation[b]),
    ]
    .map(|attenuation| attenuation.map(|db: f64| 10f64.powf(-db / 10.0)));
    if let Some(slot) = detail {
        *slot = Some(RayDetail {
            source_altitude_m: source_altitude,
            ground_factor: ground_path.ground_path_g,
            ground_bands: ground,
            terrain,
            screening_bands: screening,
            obstacle,
            vegetation_bands: vegetation,
            profile: std::mem::take(profile),
        });
    }
    RayTransfer {
        slant_distance_m: slant,
        periods: [variants; 3],
    }
}

/// Band energies (linear, A-weighted) and their per-variant sums for an emission `L_W` per band
/// through `transfer`. The receiver reflection lifts every variant but free field.
pub fn received_variants(
    transfer: &VariantBands,
    emission_db: &[f64; NUM_BANDS],
    reflection_db: f64,
) -> crate::types::PropagationVariants {
    let reflection = 10f64.powf(reflection_db / 10.0);
    let source: [f64; NUM_BANDS] = std::array::from_fn(|band| {
        10f64.powf((emission_db[band] + crate::constants::A_WEIGHTING[band]) / 10.0)
    });
    let sum = |variant: usize, lift: f64| -> f64 {
        (0..NUM_BANDS).map(|band| source[band] * transfer[variant][band] * lift).sum()
    };
    crate::types::PropagationVariants {
        full_energy: sum(VARIANT_FULL, reflection),
        free_field_energy: sum(VARIANT_FREE_FIELD, 1.0),
        no_terrain_energy: sum(VARIANT_NO_TERRAIN, reflection),
        no_screening_energy: sum(VARIANT_NO_SCREENING, reflection),
        no_vegetation_energy: sum(VARIANT_NO_VEGETATION, reflection),
        no_ground_energy: sum(VARIANT_NO_GROUND, reflection),
        no_atmospheric_energy: sum(VARIANT_NO_ATMOSPHERE, reflection),
        band_energy: std::array::from_fn(|band| source[band] * transfer[VARIANT_FULL][band] * reflection),
    }
}
