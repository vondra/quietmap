//! Real receivers on a prepared release: every road and rail piece the popup admits, the
//! production quadrature against the fine point sum, per layer; plus the popup's own layer
//! totals as the check that the admitted set is the popup's.

use crate::piece::{add, energies, lden_a, point_sum, production, Bands, Piece};
use noise_compute::compute::line_piece::LinePiece;
use noise_compute::emission::railway::{self, RailType};
use noise_compute::propagation::obstacle_index::VectorReflectionSampler;
use noise_compute::propagation::point_sum::NodeSpacing;
use noise_compute::propagation::ray_transfer::RayReceiver;
use noise_compute::propagation::meteorology::Meteorology;
use noise_compute::propagation::relevance_bound::{surface_relevance_bound, SourceSpread, LINE_REACH_CEILING_M};
use noise_compute::types::{LayerKind, RasterSampler, Receiver, NUM_BANDS};
use rayon::prelude::*;
use serde_json::{json, Value};
use std::path::Path;

/// The popup's admission: the row's relevance-bound reach.
fn admitted_pieces(sources: &source_reader::PointQueryData, receiver: &Receiver) -> Vec<(LayerKind, Piece)> {
    let weather = Meteorology::defaults();
    let bound = surface_relevance_bound(&weather);
    let receiver_city = noise_compute::square_country_city::square_country_city_for_latlng(receiver.lat, receiver.lon);
    let mut rows = Vec::new();
    for seg in &sources.roads {
        let Some(norm) = noise_compute::normalize::normalize_road_segment(seg, seg.square_country_city.unwrap_or(receiver_city)) else {
            continue;
        };
        let emission = norm.period_emissions_db();
        if seg.dist_m > LINE_REACH_CEILING_M
            || !bound.within_reach(&emission, SourceSpread::Line, seg.dist_m)
        {
            continue;
        }
        rows.push((LayerKind::Road, Piece {
            line: LinePiece {
                start_lat: seg.start_lat, start_lon: seg.start_lon, end_lat: seg.end_lat, end_lon: seg.end_lon,
                source_height_m: norm.source_height_m,
                source_ground_factor: noise_compute::normalize::road::ROAD_SOURCE_GROUND_FACTOR,
                platform_half_width_m: noise_compute::normalize::road::road_platform_half_width_m(seg.lanes),
                directivity: noise_compute::propagation::line_quadrature::LineDirectivity::Omnidirectional,
            },
            cp: (seg.cp_lat, seg.cp_lon),
            emission_db_per_m: emission,
        }));
    }
    for seg in &sources.railways {
        if seg.tunnel || seg.traffic.is_silent() {
            continue;
        }
        let rail_type = RailType::from_u8(seg.rail_type);
        let emission = railway::rail_period_emissions(rail_type, seg.speed_kmh, seg.traffic);
        if seg.dist_m > LINE_REACH_CEILING_M
            || !bound.within_reach(&emission, SourceSpread::Line, seg.dist_m)
        {
            continue;
        }
        rows.push((LayerKind::Railway, Piece {
            line: LinePiece {
                start_lat: seg.start_lat, start_lon: seg.start_lon, end_lat: seg.end_lat, end_lon: seg.end_lon,
                source_height_m: noise_compute::normalize::rail::rail_source_height_m(rail_type),
                source_ground_factor: noise_compute::normalize::rail::rail_source_ground_factor(rail_type, seg.bridge),
                platform_half_width_m: noise_compute::normalize::rail::RAIL_PLATFORM_HALF_WIDTH_M,
                directivity: noise_compute::normalize::rail::RAIL_SOURCE_DIRECTIVITY,
            },
            cp: (seg.cp_lat, seg.cp_lon),
            emission_db_per_m: emission,
        }));
    }
    rows
}

/// Median, 95th percentile and maximum of absolute values.
fn spread(mut values: Vec<f64>) -> Value {
    values.iter_mut().for_each(|v| *v = v.abs());
    values.sort_by(f64::total_cmp);
    let at = |q: f64| values.get(((values.len().max(1) - 1) as f64 * q).round() as usize).copied();
    json!({"pieces": values.len(), "median_abs_db": at(0.5), "p95_abs_db": at(0.95), "max_abs_db": values.last()})
}

pub fn run(prepared_year_dir: &Path, receivers: &[(String, f64, f64)], spacing: NodeSpacing) -> Result<Value, String> {
    let real = raster_reader::RealRasters::new(prepared_year_dir);
    noise_compute::square_country_city::set_square_country_city_prepared_directory(prepared_year_dir);
    let mut out = Vec::new();
    for (name, lat, lon) in receivers {
        let started = std::time::Instant::now();
        // Both sides evaluate the named point as an outdoor receiver; a point inside a building
        // is reported, not moved (the popup's building exposure is a stored façade receiver).
        let obstacles = source_reader::structure_store::load_obstacle_set(prepared_year_dir, *lat, *lon)?;
        let inside = obstacles.enclosed_footprint_at(*lat, *lon);
        let sources = source_reader::collect_sources_at_point(prepared_year_dir, *lat, *lon)?;
        let checked = raster_reader::CheckedRasters::new(&real);
        let rasters = VectorReflectionSampler { inner: &checked, set: &obstacles, own_footprint: None };
        let receiver = Receiver::new(*lat, *lon, rasters.elevation(*lat, *lon));
        let popup = noise_compute::compute_at_point(&receiver, &sources.roads, &sources.railways, &[], &[], &[], &obstacles, &rasters, None);
        let reflection_db = rasters.building_enclosure(*lat, *lon);
        let ray_receiver = RayReceiver { lat: *lat, lon: *lon, altitude_m: receiver.altitude_m() };
        let pieces = admitted_pieces(&sources, &receiver);
        let results: Vec<(LayerKind, [Bands; 3], [Bands; 3], usize)> = pieces
            .par_iter()
            .map(|(layer, piece)| {
                let quadrature = production(&ray_receiver, piece, &obstacles, &rasters)
                    .map_or([[0.0; NUM_BANDS]; 3], |t| energies(&t, &piece.emission_db_per_m, reflection_db));
                let (transfer, nodes) = point_sum(&ray_receiver, piece, &obstacles, &rasters, spacing);
                (*layer, quadrature, energies(&transfer, &piece.emission_db_per_m, reflection_db), nodes)
            })
            .collect();
        checked.ensure_valid().map_err(|e| e.to_string())?;
        let mut layers = serde_json::Map::new();
        for layer in [LayerKind::Road, LayerKind::Railway] {
            let (mut q, mut r, mut nodes, mut residuals) = ([[0.0; NUM_BANDS]; 3], [[0.0; NUM_BANDS]; 3], 0, Vec::new());
            for (kind, quadrature, reference, count) in &results {
                if *kind != layer {
                    continue;
                }
                add(&mut q, quadrature);
                add(&mut r, reference);
                nodes += count;
                residuals.push(lden_a(quadrature) - lden_a(reference));
            }
            let popup_lden = popup.sources.iter().find(|s| s.source_type == layer).map(|s| s.periods.lden_db);
            let count = residuals.len();
            layers.insert(layer.as_str().to_owned(), json!({
                "pieces": count, "point_sum_nodes": nodes, "popup_lden": popup_lden,
                "quadrature_lden": (count > 0).then(|| lden_a(&q)), "point_sum_lden": (count > 0).then(|| lden_a(&r)),
                "quadrature_minus_point_sum_db": (count > 0).then(|| lden_a(&q) - lden_a(&r)),
                "piece_residuals": spread(residuals),
            }));
        }
        out.push(json!({
            "receiver": name, "lat": lat, "lon": lon, "inside_building": inside.is_some(), "reflection_boost_db": reflection_db, "layers": layers,
            "seconds": started.elapsed().as_secs_f64(),
        }));
        eprintln!("{name}: {:.1} s", started.elapsed().as_secs_f64());
    }
    Ok(Value::Array(out))
}
