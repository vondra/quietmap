//! One line piece two ways: the production quadrature and the fine point sum over the same
//! production ray, as received band energies per period.

use noise_compute::compute::line_piece::{evaluate_line_piece, LinePiece, LinePieceScratch};
use noise_compute::constants::A_WEIGHTING;
use noise_compute::propagation::meteorology::Meteorology;
use noise_compute::propagation::obstacle_index::ObstacleSet;
use noise_compute::propagation::point_sum::{line_nodes, NodeSpacing, POINT_SOURCE_DIVERGENCE_OFFSET_DB};
use noise_compute::propagation::ray_transfer::{
    evaluate_ray_transfer, RayReceiver, RayScratch, RaySource, SourceGround, VariantBands, VARIANT_COUNT,
    VARIANT_FULL,
};
use noise_compute::types::{RasterSampler, NUM_BANDS};

pub type Bands = [f64; NUM_BANDS];

/// A piece with its per-metre emission `L_W′` for day, evening and night.
pub struct Piece {
    pub line: LinePiece,
    pub cp: (f64, f64),
    pub emission_db_per_m: [Bands; 3],
}

/// Full-variant received band energies (unweighted) per period.
pub fn energies(transfer: &[VariantBands; 3], emission: &[Bands; 3], reflection_db: f64) -> [Bands; 3] {
    let lift = 10f64.powf(reflection_db / 10.0);
    std::array::from_fn(|p| {
        std::array::from_fn(|b| 10f64.powf(emission[p][b] / 10.0) * transfer[p][VARIANT_FULL][b] * lift)
    })
}

/// The production quadrature of `piece`.
pub fn production(
    receiver: &RayReceiver,
    piece: &Piece,
    obstacles: &ObstacleSet,
    rasters: &dyn RasterSampler,
) -> Option<[VariantBands; 3]> {
    evaluate_line_piece(
        receiver,
        &piece.line,
        piece.cp.0,
        piece.cp.1,
        obstacles,
        rasters,
        &Meteorology::defaults(),
        &mut LinePieceScratch::default(),
        None,
    )
    .map(|transfer| transfer.periods)
}

/// The fine point sum of `piece`: nodes placed by `line_nodes` on the same 3D line, each node on
/// the production ray, `Δx/(10^1.1·r²)` per node.
pub fn point_sum(
    receiver: &RayReceiver,
    piece: &Piece,
    obstacles: &ObstacleSet,
    rasters: &dyn RasterSampler,
    spacing: NodeSpacing,
) -> ([VariantBands; 3], usize) {
    let line = &piece.line;
    let m_per_deg_lon = grid::geo::m_per_deg_lon(receiver.lat.to_radians());
    let local = |lat: f64, lon: f64, altitude: f64| {
        [
            grid::geo::wrapped_longitude_delta(receiver.lon, lon) * m_per_deg_lon,
            (lat - receiver.lat) * grid::geo::M_PER_DEG_LAT,
            altitude,
        ]
    };
    let start = local(line.start_lat, line.start_lon, rasters.elevation(line.start_lat, line.start_lon) + line.source_height_m);
    let end = local(line.end_lat, line.end_lon, rasters.elevation(line.end_lat, line.end_lon) + line.source_height_m);
    let nodes = line_nodes(start, end, [0.0, 0.0, receiver.altitude_m], spacing);
    let divergence = 10f64.powf(POINT_SOURCE_DIVERGENCE_OFFSET_DB / 10.0);
    let mut sum = [[[0.0; NUM_BANDS]; VARIANT_COUNT]; 3];
    let mut scratch = RayScratch::default();
    let weather = Meteorology::defaults();
    let longitude_span = grid::geo::wrapped_longitude_delta(line.start_lon, line.end_lon);
    for node in &nodes {
        let f = node.piece_fraction.clamp(0.0, 1.0);
        let source = RaySource {
            lat: line.start_lat + f * (line.end_lat - line.start_lat),
            lon: grid::geo::normalize_longitude(line.start_lon + f * longitude_span),
            height_m: line.source_height_m,
            ground: SourceGround::Fixed(line.source_ground_factor),
            platform_half_width_m: line.platform_half_width_m,
            exclusion_radius_m: 0.0,
        };
        let transfer =
            evaluate_ray_transfer(receiver, &source, obstacles, true, rasters, &weather, false, &mut scratch, None);
        let along = [end[0] - start[0], end[1] - start[1]];
        let horizontal_range_sq = node.position_m[0].powi(2) + node.position_m[1].powi(2);
        let horizontal_line_sq = along[0].powi(2) + along[1].powi(2);
        let cross = node.position_m[0] * along[1] - node.position_m[1] * along[0];
        let sin_squared = if horizontal_line_sq == 0.0 { 1.0 }
            else if horizontal_range_sq == 0.0 { 0.0 }
            else { (cross * cross / (horizontal_range_sq * horizontal_line_sq)).clamp(0.0, 1.0) };
        let weight = line.directivity.factor(sin_squared) * node.length_m
            / (divergence * node.slant_distance_m * node.slant_distance_m);
        for (period_sum, period) in sum.iter_mut().zip(&transfer.periods) {
            for (variant_sum, variant) in period_sum.iter_mut().zip(period) {
                for (value, band) in variant_sum.iter_mut().zip(variant) {
                    *value += weight * band;
                }
            }
        }
    }
    (sum, nodes.len())
}

pub fn add(total: &mut [Bands; 3], energy: &[Bands; 3]) {
    for (period_total, period) in total.iter_mut().zip(energy) {
        for (value, band) in period_total.iter_mut().zip(period) {
            *value += band;
        }
    }
}

/// A-weighted Lden of period band energies.
pub fn lden_a(energy: &[Bands; 3]) -> f64 {
    let period = |p: usize| {
        10.0 * (0..NUM_BANDS).map(|b| energy[p][b] * 10f64.powf(A_WEIGHTING[b] / 10.0)).sum::<f64>().max(1e-30).log10()
    };
    noise_compute::periods::compute_lden(period(0), period(1), period(2))
}
