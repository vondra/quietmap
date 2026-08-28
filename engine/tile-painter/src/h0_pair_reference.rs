//! CPU physical-path authority for one model-v2 H0 line/receiver pair.

use std::f64::consts::LN_10;

use noise_compute::compute::element::{LineLayer, LinePiece};
use noise_compute::constants::{ALPHA_ATM, A_WEIGHTING, M_PER_DEG_LAT};
use noise_compute::propagation::h0_streaming_reduction::{
    reduce_h0, H0Candidate, H0Node, H0Reduction, H0ReductionError,
};
use noise_compute::propagation::iso9613::{fast_exp_f64, ground_atten_bands};
use noise_compute::propagation::node_eval::{evaluate_node_attenuation_bands, NodePathTerms};
use noise_compute::propagation::obstacle_index::{CellPrune, CrossingCandidate, ObstacleSet};
use noise_compute::propagation::path_effects::{self, ObstacleInput};
use noise_compute::propagation::path_profile::CoarseMid;
use noise_compute::propagation::streaming_reduction::{
    source_frame_mlon, source_frame_vector_from_world, GeometryError,
};
use noise_compute::propagation::PathProfile;
use noise_compute::types::{Barrier, RasterSampler, NUM_BANDS};
use raster_reader::fused_tile_z13::{FusedTileZ13, TILE_PX};

use crate::accumulator::NUM_PERIODS;
use crate::scatter_band::build_surface_profile;
use crate::source_line::LineRow;

/// A complete one-pair CPU H0 result used by the CUDA parity harness.
#[derive(Debug)]
pub struct H0PairReference {
    pub reduction: H0Reduction,
    /// A-weighted linear energy before the final f32 period cast.
    pub period_band_power: [[f64; NUM_BANDS]; NUM_PERIODS],
    /// Production accumulation payload, one f32 value per period.
    pub period_power_f32: [f32; NUM_PERIODS],
}

/// Fail-closed errors from the physical H0 CPU reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum H0PairReferenceError {
    PixelOutsideTile,
    PairOutsideReach,
    Geometry(GeometryError),
    Reduction(H0ReductionError),
    NonFinitePath,
}

impl From<GeometryError> for H0PairReferenceError {
    fn from(error: GeometryError) -> Self {
        Self::Geometry(error)
    }
}

impl From<H0ReductionError> for H0PairReferenceError {
    fn from(error: H0ReductionError) -> Self {
        Self::Reduction(error)
    }
}

/// Evaluate one real line/receiver pair with the landed H0 reducer and the
/// production CPU path-effect primitives.
///
/// `candidates` is the raw acceleration-store superset. It is consumed once by
/// [`reduce_h0`]; no retained interval or source-membership list is formed.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_h0_pair<I>(
    tile: &FusedTileZ13,
    line: &LineRow,
    barriers: &[Barrier],
    obstacles: Option<&ObstacleSet>,
    pixel_y: usize,
    pixel_x: usize,
    layer: LineLayer,
    candidates: I,
    cadence: Option<CoarseMid>,
) -> Result<H0PairReference, H0PairReferenceError>
where
    I: IntoIterator<Item = H0Candidate>,
{
    let pair = prepare_pair_geometry(tile, line, pixel_y, pixel_x)?;
    let reduction = reduce_h0(pair.piece, layer, candidates)?;
    let (period_band_power, period_power_f32) = evaluate_node_powers(
        tile,
        line,
        barriers,
        obstacles,
        pair,
        reduction.nodes(),
        cadence,
        |node_index, _| reduction.node_is_admitted(node_index) == Some(true),
    )?;
    Ok(H0PairReference {
        reduction,
        period_band_power,
        period_power_f32,
    })
}

#[derive(Debug, Clone, Copy)]
struct PairGeometry {
    piece: LinePiece,
    receiver_latitude: f64,
    receiver_longitude: f64,
    receiver_altitude_m: f64,
    receiver_reflection_db: f64,
    source_mlon: f64,
}

fn prepare_pair_geometry(
    tile: &FusedTileZ13,
    line: &LineRow,
    pixel_y: usize,
    pixel_x: usize,
) -> Result<PairGeometry, H0PairReferenceError> {
    if pixel_y >= TILE_PX || pixel_x >= TILE_PX {
        return Err(H0PairReferenceError::PixelOutsideTile);
    }
    let receiver_latitude = tile.rx_lat[pixel_y];
    let receiver_longitude = tile.rx_lon[pixel_x];
    let receiver_index = pixel_y * TILE_PX + pixel_x;
    let receiver_altitude_m = tile.rx_alt_m[receiver_index] as f64;
    let receiver_reflection_db = tile.rx_refl_db[receiver_index] as f64;
    let source_mlon = source_frame_mlon(line.start_lat, line.end_lat)?;
    let start_m = source_frame_vector_from_world(
        line.start_lat,
        line.start_lon,
        receiver_latitude,
        receiver_longitude,
        source_mlon,
    )?;
    let end_m = source_frame_vector_from_world(
        line.end_lat,
        line.end_lon,
        receiver_latitude,
        receiver_longitude,
        source_mlon,
    )?;
    if point_to_segment_distance(start_m, end_m) > line.max_distance_m {
        return Err(H0PairReferenceError::PairOutsideReach);
    }
    Ok(PairGeometry {
        piece: LinePiece {
            start_m: [start_m.x, start_m.y],
            end_m: [end_m.x, end_m.y],
            emission_per_m: 1.0,
        },
        receiver_latitude,
        receiver_longitude,
        receiver_altitude_m,
        receiver_reflection_db,
        source_mlon,
    })
}

#[allow(clippy::too_many_arguments)]
fn evaluate_node_powers<F>(
    tile: &FusedTileZ13,
    line: &LineRow,
    barriers: &[Barrier],
    obstacles: Option<&ObstacleSet>,
    pair: PairGeometry,
    nodes: &[H0Node],
    cadence: Option<CoarseMid>,
    mut node_is_admitted: F,
) -> Result<([[f64; NUM_BANDS]; NUM_PERIODS], [f32; NUM_PERIODS]), H0PairReferenceError>
where
    F: FnMut(usize, &H0Node) -> bool,
{
    let vector_mode = !barriers.is_empty() || obstacles.is_some_and(|set| !set.indexes.is_empty());
    let mut profile = PathProfile::new();
    let mut crossing_candidates: Vec<CrossingCandidate> = Vec::new();
    let mut period_band_power = [[0.0_f64; NUM_BANDS]; NUM_PERIODS];
    for (node_index, node) in nodes.iter().enumerate() {
        let source_latitude = pair.receiver_latitude + node.line_node.position_m[1] / M_PER_DEG_LAT;
        let source_longitude =
            pair.receiver_longitude + node.line_node.position_m[0] / pair.source_mlon;
        let source_altitude_m =
            tile.elevation(source_latitude, source_longitude) + line.source_height_m;
        let vertical_m = source_altitude_m - pair.receiver_altitude_m;
        let exact_slant_m =
            (node.node_distance_m * node.node_distance_m + vertical_m * vertical_m).sqrt();
        if !exact_slant_m.is_finite() {
            return Err(H0PairReferenceError::NonFinitePath);
        }
        build_surface_profile(
            tile,
            cadence,
            source_latitude,
            source_longitude,
            pair.receiver_latitude,
            pair.receiver_longitude,
            node.node_distance_m,
            &mut profile,
        );
        let ground_path = path_effects::cnossos_ground_path_from_profile(
            &mut profile,
            source_altitude_m,
            pair.receiver_altitude_m,
            line.bridge,
        );
        let ground_db = ground_atten_bands(ground_path);
        let terrain_db = path_effects::terrain_attenuation(
            &mut profile,
            source_altitude_m,
            pair.receiver_altitude_m,
        );

        let admitted = !vector_mode || node_is_admitted(node_index, node);
        crossing_candidates.clear();
        if admitted {
            if let Some(set) = obstacles {
                set.crossings_pruned(
                    source_latitude,
                    source_longitude,
                    pair.receiver_latitude,
                    pair.receiver_longitude,
                    &CellPrune::for_profile(&profile, source_altitude_m, pair.receiver_altitude_m),
                    &mut crossing_candidates,
                );
            }
        }
        let active_barriers = if admitted { barriers } else { &[] };
        let vector_path_present =
            path_effects::line_vector_path_present(&profile, active_barriers, &crossing_candidates);
        let screening_db = path_effects::screening_attenuation(
            &mut profile,
            active_barriers,
            if vector_mode {
                ObstacleInput {
                    candidates: &crossing_candidates,
                    replace_sample_buildings: true,
                }
            } else {
                ObstacleInput::CANDIDATES_OFF
            },
            source_altitude_m,
            pair.receiver_altitude_m,
            0.0,
            &terrain_db,
        );
        let vegetation_db = path_effects::vegetation_attenuation_path(&profile);
        let attenuation_db = evaluate_node_attenuation_bands(
            exact_slant_m,
            NodePathTerms {
                atmospheric_db: std::array::from_fn(|band| {
                    ALPHA_ATM[band] * exact_slant_m / 1000.0
                }),
                ground_db,
                terrain_db,
                screening_db,
                vegetation_db,
                meteo_db: [-pair.receiver_reflection_db; NUM_BANDS],
                barrier_present: vector_path_present,
            },
        );
        for band in 0..NUM_BANDS {
            let transfer = fast_exp_f64((-attenuation_db[band] + A_WEIGHTING[band]) * LN_10 * 0.1);
            for (period, powers) in period_band_power.iter_mut().enumerate() {
                powers[band] +=
                    line.emission_lin[period][band] as f64 * node.line_node.weight * transfer;
            }
        }
    }

    let period_power_f32 = std::array::from_fn(|period| {
        let power: f64 = period_band_power[period].iter().sum();
        if power.is_finite() && power > 0.0 {
            power as f32
        } else {
            0.0
        }
    });
    Ok((period_band_power, period_power_f32))
}

fn point_to_segment_distance(
    start: noise_compute::propagation::streaming_reduction::MetricVector,
    end: noise_compute::propagation::streaming_reduction::MetricVector,
) -> f64 {
    let edge_x = end.x - start.x;
    let edge_y = end.y - start.y;
    let length_squared = edge_x * edge_x + edge_y * edge_y;
    let fraction = if length_squared > 0.0 {
        (-(start.x * edge_x + start.y * edge_y) / length_squared).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let x = start.x + fraction * edge_x;
    let y = start.y + fraction * edge_y;
    (x * x + y * y).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use noise_compute::constants::m_per_deg_lon;
    use raster_reader::RealRasters;
    use std::path::Path;

    #[test]
    fn raster_fallback_pair_runs_every_h0_node_through_the_physical_path() {
        let rasters = RealRasters::new(Path::new("/nonexistent-h0-pair-reference-fixture"));
        let tile = FusedTileZ13::build_receiver_altitude_only(12, 2211, 1386, &rasters);
        let centre_lat = (tile.bbox.north_lat + tile.bbox.south_lat) * 0.5;
        let centre_lon = (tile.bbox.west_lon + tile.bbox.east_lon) * 0.5;
        let d_lat = |metres: f64| metres / M_PER_DEG_LAT;
        let d_lon = |metres: f64| metres / m_per_deg_lon(centre_lat.to_radians());
        let line = LineRow {
            start_lat: centre_lat,
            start_lon: centre_lon + d_lon(-50.0),
            end_lat: centre_lat,
            end_lon: centre_lon + d_lon(50.0),
            length_m: 100.0,
            max_distance_m: 2_000.0,
            source_height_m: 0.05,
            bridge: false,
            emission_lin: [[1.0; NUM_BANDS]; NUM_PERIODS],
        };
        let pixel_y = crate::scatter_band::lat_to_py(&tile.bbox, centre_lat + d_lat(100.0));
        let pixel_x = crate::scatter_band::lon_to_px(&tile.bbox, centre_lon);
        let result = evaluate_h0_pair(
            &tile,
            &line,
            &[],
            None,
            pixel_y,
            pixel_x,
            LineLayer::Road,
            [],
            None,
        )
        .expect("legal synthetic H0 pair");

        assert!(!result.reduction.nodes().is_empty());
        assert_eq!(result.reduction.admitted_node_count(), 0);
        assert!(result.period_power_f32.iter().all(|power| *power > 0.0));
        for period in 0..NUM_PERIODS {
            assert_eq!(
                result.period_power_f32[period].to_bits(),
                (result.period_band_power[period].iter().sum::<f64>() as f32).to_bits()
            );
        }
    }
}
