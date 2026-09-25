//! Linear transfer of one source→receiver ray per period, band and popup variant (everything
//! but divergence and the receiver reflection) — the per-node physics of the line quadrature and
//! of point sources. CNOSSOS-EU per meteorological state (`cnossos`), states mixed per period
//! with that period's p for the ray's direction (2.5.9), air absorption per period, forest on the
//! plan-view depth.

use super::cnossos::{state_boundary, MeteorologicalState, StateBoundary, VerticalPathScratch};
use super::meteorology::Meteorology;
use super::obstacle_index::{CrossingCandidate, ObstacleKind, ObstacleSet};
use super::ray_path::{RayPathBuffers, RayPathInputs};
use super::PathProfile;
use crate::types::{EdgePoint, ObstacleEdge, RasterSampler, ScreeningObstacleTrace, TerrainTrace, NUM_BANDS};

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

/// Gs of (2.5.14): the ground the source stands on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SourceGround {
    /// Road platform or deck 0, ballast 1.
    Fixed(f64),
    /// A point source: the ground factor sampled under it (ISO/TR 17534-4 §5.6).
    UnderSource,
}

/// The source end of one ray.
#[derive(Debug, Clone, Copy)]
pub struct RaySource {
    pub lat: f64,
    pub lon: f64,
    /// Height of the source above the ground under it.
    pub height_m: f64,
    pub ground: SourceGround,
    /// Line sources: the platform half-width within which terrain may not rise above the source
    /// ground; 0 for points.
    pub platform_half_width_m: f64,
    /// Buildings nearer the source than this are its own footprint, not obstacles.
    pub exclusion_radius_m: f64,
}

/// Reusable per-thread buffers.
#[derive(Default)]
pub struct RayScratch {
    profile: PathProfile,
    crossings: Vec<CrossingCandidate>,
    path: RayPathBuffers,
    bare_path: RayPathBuffers,
    vertical: VerticalPathScratch,
}

/// What the popup trace shows of one ray.
pub struct RayDetail {
    pub source_altitude_m: f64,
    pub ground_factor: f64,
    /// A_ground of the whole path without diffraction, states mixed with the day p.
    pub ground_bands: [f64; NUM_BANDS],
    /// Level the terrain takes away (full minus no-terrain boundary, day mix).
    pub terrain: TerrainTrace,
    /// Level the buildings and walls take away (full minus no-obstacle boundary, day mix).
    pub screening_bands: [f64; NUM_BANDS],
    pub obstacle: ScreeningObstacleTrace,
    pub vegetation_bands: [f64; NUM_BANDS],
    /// Density-weighted forest depth behind `vegetation_bands`.
    pub forest_depth_m: f64,
    pub profile: PathProfile,
}

fn energy(db: f64) -> f64 {
    10f64.powf(-db / 10.0)
}

fn mixed_db(p: f64, homogeneous: &[f64; NUM_BANDS], favourable: &[f64; NUM_BANDS]) -> [f64; NUM_BANDS] {
    std::array::from_fn(|b| -10.0 * (p * energy(favourable[b]) + (1.0 - p) * energy(homogeneous[b])).log10())
}

/// The transfer of the ray from `source` to `receiver`; vector obstacles are read only when
/// `obstacles_on_ray` (a node in a clear gap of the line quadrature's mask has none). `variants`
/// asks for the popup's effect hypotheses besides the full transfer.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_ray_transfer(
    receiver: &RayReceiver,
    source: &RaySource,
    obstacles: &ObstacleSet,
    obstacles_on_ray: bool,
    rasters: &dyn RasterSampler,
    weather: &Meteorology,
    variants: bool,
    scratch: &mut RayScratch,
    detail: Option<&mut Option<RayDetail>>,
) -> RayTransfer {
    let horizontal_m = grid::geo::flat_dist(source.lat, source.lon, receiver.lat, receiver.lon).max(1.0);
    let profile = &mut scratch.profile;
    rasters.build_path_profile(source.lat, source.lon, receiver.lat, receiver.lon, horizontal_m, profile);
    let source_altitude = f64::from(profile.elevation_m[0]) + source.height_m;
    let slant = horizontal_m.hypot(receiver.altitude_m - source_altitude).max(1.0);
    // Plan-view forest depth until the canopy channel and the bare-earth DEM land together (S4):
    // on today's surface model the canopy is part of the terrain, which this term was sized for.
    let forest_depth_m = super::path_profile::vegetation_run_length(&profile.t, &profile.forest_u8, profile.dist_m);
    let vegetation = super::vegetation::vegetation_attenuation(forest_depth_m);
    let source_ground_factor = match source.ground {
        SourceGround::Fixed(g) => g,
        SourceGround::UnderSource => 1.0 - f64::from(profile.imd_u8[0].min(100)) / 100.0,
    };
    scratch.crossings.clear();
    if obstacles_on_ray {
        obstacles.crossings(source.lat, source.lon, receiver.lat, receiver.lon, &mut scratch.crossings);
    }
    let inputs = RayPathInputs {
        source_altitude_m: source_altitude,
        receiver_altitude_m: receiver.altitude_m,
        source_ground_factor,
        platform_half_width_m: source.platform_half_width_m,
        exclusion_radius_m: source.exclusion_radius_m,
    };
    scratch.path.fill(profile, &scratch.crossings, &inputs, true);
    let states = [MeteorologicalState::Homogeneous, MeteorologicalState::Favourable];
    let full: [StateBoundary; 2] =
        states.map(|state| state_boundary(&scratch.path.path(&inputs, true), state, &mut scratch.vertical));
    let (no_terrain, no_screening) = if variants {
        let no_terrain =
            states.map(|state| state_boundary(&scratch.path.path(&inputs, false), state, &mut scratch.vertical));
        let no_screening = if scratch.crossings.is_empty() {
            full.clone()
        } else {
            scratch.bare_path.fill(profile, &scratch.crossings, &inputs, false);
            states.map(|state| state_boundary(&scratch.bare_path.path(&inputs, true), state, &mut scratch.vertical))
        };
        (no_terrain, no_screening)
    } else {
        (full.clone(), full.clone())
    };
    let azimuth = {
        let m_per_deg_lon = grid::geo::m_per_deg_lon(source.lat.to_radians());
        let east = grid::geo::wrapped_longitude_delta(source.lon, receiver.lon) * m_per_deg_lon;
        let north = (receiver.lat - source.lat) * grid::geo::M_PER_DEG_LAT;
        north.atan2(east)
    };
    let pair = |boundaries: &[StateBoundary; 2], pick: fn(&StateBoundary) -> [f64; NUM_BANDS]| {
        (pick(&boundaries[0]), pick(&boundaries[1]))
    };
    let attenuation = |b: &StateBoundary| b.attenuation_db;
    let hypotheses = [
        pair(&full, attenuation),
        pair(&full, |b| b.whole_path_ground_db),
        pair(&no_terrain, attenuation),
        pair(&no_screening, attenuation),
        pair(&full, attenuation),
        pair(&full, |b| b.without_ground_db),
        pair(&full, attenuation),
    ];
    let periods: [VariantBands; 3] = std::array::from_fn(|period| {
        let p = weather.favourable_probability(period, azimuth);
        let absorption: [f64; NUM_BANDS] =
            std::array::from_fn(|band| energy(weather.absorption[period][band].attenuation_db(slant)));
        std::array::from_fn(|variant| {
            let (homogeneous, favourable) = &hypotheses[variant];
            std::array::from_fn(|band| {
                let air = if variant == VARIANT_NO_ATMOSPHERE { 1.0 } else { absorption[band] };
                let forest = if variant == VARIANT_NO_VEGETATION { 1.0 } else { energy(vegetation[band]) };
                air * forest * (p * energy(favourable[band]) + (1.0 - p) * energy(homogeneous[band]))
            })
        })
    });
    if let Some(slot) = detail {
        let p = weather.favourable_probability(0, azimuth);
        let boundary_full = mixed_db(p, &full[0].attenuation_db, &full[1].attenuation_db);
        let effect = |other: &[StateBoundary; 2]| -> [f64; NUM_BANDS] {
            let base = mixed_db(p, &other[0].attenuation_db, &other[1].attenuation_db);
            std::array::from_fn(|b| (boundary_full[b] - base[b]).max(0.0))
        };
        let homogeneous_path = &full[0].path;
        let length = horizontal_m;
        let crossings = &scratch.crossings;
        let is_top = |point: &(f64, f64)| crossings.iter().any(|c| (c.t * length - point.0).abs() < 1e-6);
        let terrain_edges: Vec<EdgePoint> = homogeneous_path
            .points
            .iter()
            .filter(|point| !is_top(point))
            .map(|&(x, z)| EdgePoint { t: x / length, elevation_m: z })
            .collect();
        let chord = |x: f64| source_altitude + (receiver.altitude_m - source_altitude) * x / length;
        let representative = homogeneous_path
            .points
            .iter()
            .filter_map(|point| {
                crossings
                    .iter()
                    .find(|c| (c.t * length - point.0).abs() < 1e-6)
                    .map(|c| (c, point.1 - chord(point.0)))
            })
            .max_by(|a, b| a.1.total_cmp(&b.1));
        *slot = Some(RayDetail {
            source_altitude_m: source_altitude,
            ground_factor: 1.0 - profile.imd_u8.iter().map(|&v| f64::from(v.min(100))).sum::<f64>()
                / (100.0 * profile.imd_u8.len().max(1) as f64),
            ground_bands: mixed_db(p, &full[0].whole_path_ground_db, &full[1].whole_path_ground_db),
            terrain: TerrainTrace {
                delta_m: no_screening[0].path_difference_m,
                attenuation_bands: effect(&no_terrain),
                edges: terrain_edges,
                delta_star_m: 0.0,
            },
            screening_bands: effect(&no_screening),
            obstacle: ScreeningObstacleTrace {
                delta_m: full[0].path_difference_m,
                step_m: f64::from(profile.step_m_med),
                edge: representative.map(|(crossing, above)| ObstacleEdge {
                    kind: match crossing.kind {
                        ObstacleKind::Building => "building",
                        ObstacleKind::Barrier => "barrier",
                    },
                    t: crossing.t,
                    height_m: f64::from(crossing.height_m),
                    screen_h_m: above,
                    obstacle_id: crossing.id,
                }),
            },
            vegetation_bands: vegetation,
            forest_depth_m,
            profile: std::mem::take(profile),
        });
    }
    RayTransfer {
        slant_distance_m: slant,
        periods,
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
