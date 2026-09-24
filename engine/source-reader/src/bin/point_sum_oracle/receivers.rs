//! Real receivers on a prepared release: the production popup kernel for the reference, then
//! every road and rail microsegment it admits under every variant, with per-segment and per-ray
//! checks that the oracle's `today` reproduces production.

use crate::lines::{a_weighted_db, lden_bands, variant_rows};
use crate::ray::Bands;
use crate::segment::{add_energy, evaluate_segment, LineSegment, ReceiverPoint, SegmentResult, World, VARIANTS};
use noise_compute::constants::SOURCE_HEIGHT_RAIL;
use noise_compute::emission::railway::{self, RailType};
use noise_compute::emission::road;
use noise_compute::propagation::geo;
use noise_compute::propagation::obstacle_index::{CrossingCandidate, ObstacleSet, VectorReflectionSampler};
use noise_compute::propagation::point_sum::NodeSpacing;
use noise_compute::types::{LayerKind, RasterSampler, Receiver, TraceCollector, NUM_BANDS};
use rayon::prelude::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;

struct RealWorld<'a> {
    rasters: &'a dyn RasterSampler,
    obstacles: &'a ObstacleSet,
}

impl World for RealWorld<'_> {
    fn rasters(&self) -> &dyn RasterSampler {
        self.rasters
    }
    fn crossings(&self, src_lat: f64, src_lon: f64, rcv_lat: f64, rcv_lon: f64, out: &mut Vec<CrossingCandidate>) {
        self.obstacles.crossings(src_lat, src_lon, rcv_lat, rcv_lon, out);
    }
    fn obstacle_set(&self) -> Option<&ObstacleSet> {
        Some(self.obstacles)
    }
}

/// Production's per-segment admission and emission, one `(layer, trace key, segment)` per row.
fn admitted_segments(
    sources: &source_reader::PointQueryData,
    rasters: &dyn RasterSampler,
    receiver: &Receiver,
) -> Vec<(LayerKind, (i64, i16), LineSegment)> {
    let rcv_alt = receiver.altitude_m();
    let receiver_city = noise_compute::square_country_city::square_country_city_for_latlng(receiver.lat, receiver.lon);
    let mut rows = Vec::new();
    for seg in &sources.roads {
        let Some(norm) =
            noise_compute::normalize::normalize_road_segment(seg, seg.square_country_city.unwrap_or(receiver_city))
        else {
            continue;
        };
        let src_alt = rasters.elevation(seg.cp_lat, seg.cp_lon) + norm.source_height_m;
        if seg.dist_m > norm.max_distance_m || geo::slant_dist(seg.dist_m, src_alt, rcv_alt) < 1.0 {
            continue;
        }
        let pcts = norm.period_pcts();
        let emission: [Bands; 3] = std::array::from_fn(|p| {
            let flows = road::build_period_flows(
                norm.light_aadt,
                norm.medium_aadt,
                norm.heavy_aadt,
                norm.moto_aadt,
                norm.speed_kmh,
                pcts[p],
                [12.0, 4.0, 8.0][p],
            );
            road::line_source_emission(&flows, norm.surf_corr_db)
        });
        if geo::below_free_field_threshold_line(emission[0].iter().copied().fold(f64::NEG_INFINITY, f64::max), seg.dist_m, 0.0) {
            continue;
        }
        rows.push((LayerKind::Road, (seg.osm_id, seg.segment_idx), line(seg.start_lat, seg.start_lon, seg.end_lat, seg.end_lon, seg.cp_lat, seg.cp_lon, seg.length_m, seg.dist_m, norm.source_height_m, seg.bridge, emission)));
    }
    for seg in &sources.railways {
        if seg.tunnel || seg.traffic.is_silent() {
            continue;
        }
        let rail_type = RailType::from_u8(seg.rail_type);
        let src_alt = rasters.elevation(seg.cp_lat, seg.cp_lon) + SOURCE_HEIGHT_RAIL;
        if seg.dist_m > railway::rail_reach_m(rail_type, seg.speed_kmh, seg.traffic)
            || geo::slant_dist(seg.dist_m, src_alt, rcv_alt) < 1.0
        {
            continue;
        }
        let periods = seg.traffic.periods();
        let emission: [Bands; 3] = std::array::from_fn(|p| {
            let (passenger, freight, hours) = periods[p];
            railway::railway_emission(rail_type, seg.speed_kmh, passenger, freight, hours)
        });
        let loudest = emission.iter().flatten().copied().fold(f64::NEG_INFINITY, f64::max);
        if geo::below_free_field_threshold_line(loudest, seg.dist_m, 0.0) {
            continue;
        }
        rows.push((LayerKind::Railway, (seg.osm_id, seg.segment_idx), line(seg.start_lat, seg.start_lon, seg.end_lat, seg.end_lon, seg.cp_lat, seg.cp_lon, seg.length_m, seg.dist_m, SOURCE_HEIGHT_RAIL, seg.bridge, emission)));
    }
    rows
}

#[allow(clippy::too_many_arguments)]
fn line(
    start_lat: f64,
    start_lon: f64,
    end_lat: f64,
    end_lon: f64,
    cp_lat: f64,
    cp_lon: f64,
    length_m: f32,
    dist_m: f64,
    source_height_m: f64,
    bridge: bool,
    emission_db_per_m: [Bands; 3],
) -> LineSegment {
    LineSegment {
        start: (start_lat, start_lon),
        end: (end_lat, end_lon),
        closest: (cp_lat, cp_lon),
        length_m: length_m as f64,
        dist_m,
        source_height_m,
        force_hard_ground: bridge,
        emission_db_per_m,
    }
}

/// Median, 95th percentile and maximum of absolute values.
fn spread(mut values: Vec<f64>) -> Value {
    values.iter_mut().for_each(|v| *v = v.abs());
    values.sort_by(f64::total_cmp);
    let at = |q: f64| values.get(((values.len().max(1) - 1) as f64 * q).round() as usize).copied();
    json!({"matched": values.len(), "median_abs": at(0.5), "p95_abs": at(0.95), "max_abs": values.last()})
}

type SegmentKey = (bool, i64, i16, u64, u64);

/// Rows split at square edges repeat (osm_id, segment_idx); the start point tells them apart.
fn segment_key(road: bool, osm_id: i64, segment_idx: i16, start: (f64, f64)) -> SegmentKey {
    (road, osm_id, segment_idx, start.0.to_bits(), start.1.to_bits())
}

/// One layer: every variant's Lden, today through the production arc quadrature, and the
/// per-segment differences of both `today` forms from the production traces.
fn layer_summary(
    layer: LayerKind,
    segments: &[(LayerKind, (i64, i16), LineSegment)],
    results: &[SegmentResult],
    production_lden: Option<f64>,
    production_by_key: &HashMap<SegmentKey, f64>,
) -> Value {
    let mut totals = vec![[[0.0; NUM_BANDS]; 3]; VARIANTS.len()];
    let mut arc_total = [[0.0; NUM_BANDS]; 3];
    let (mut closest_point_residuals, mut arc_residuals, mut validation, mut count) = (Vec::new(), Vec::new(), 0.0_f64, 0);
    let mut nodes = 0;
    for ((kind, key, segment), result) in segments.iter().zip(results) {
        if *kind != layer {
            continue;
        }
        count += 1;
        nodes += result.point_sum_nodes;
        validation = validation.max(result.validation_db);
        for (total, energy) in totals.iter_mut().zip(&result.energies) {
            add_energy(total, energy);
        }
        let arc = result.today_with_production_arc.expect("real worlds carry obstacles");
        add_energy(&mut arc_total, &arc);
        let key = segment_key(layer == LayerKind::Road, key.0, key.1, segment.start);
        if let Some(production) = production_by_key.get(&key) {
            arc_residuals.push(a_weighted_db(&lden_bands(&arc)) - production);
            closest_point_residuals.push(a_weighted_db(&lden_bands(&result.energies[0])) - production);
        }
    }
    json!({
        "segments": count,
        "point_sum_nodes": nodes,
        "production_lden": production_lden,
        "variants": if count > 0 { variant_rows(&totals) } else { Value::Null },
        "replica_vs_production_terms_max_db": validation,
        "today_with_production_arc_lden": (count > 0).then(|| a_weighted_db(&lden_bands(&arc_total))),
        "segment_residual_arc_today_minus_production_db": spread(arc_residuals),
        "segment_residual_today_minus_production_db": spread(closest_point_residuals),
    })
}

pub fn run(prepared_year_dir: &Path, receivers: &[(String, f64, f64)], spacing: NodeSpacing) -> Result<Value, String> {
    let real = raster_reader::RealRasters::new(prepared_year_dir);
    noise_compute::square_country_city::set_square_country_city_prepared_directory(prepared_year_dir);
    let mut out = Vec::new();
    for (name, lat, lon) in receivers {
        let started = std::time::Instant::now();
        // The popup's receiver: a click inside a building moves to its facade.
        let mut obstacles = source_reader::structure_store::load_obstacle_set(prepared_year_dir, *lat, *lon)?;
        let (facade_lat, facade_lon, inside) = source_reader::structure_store::locate_facade_receiver(&obstacles, *lat, *lon);
        if (facade_lat, facade_lon) != (*lat, *lon) {
            obstacles = source_reader::structure_store::load_obstacle_set(prepared_year_dir, facade_lat, facade_lon)?;
        }
        let sources = source_reader::collect_sources_at_point(prepared_year_dir, facade_lat, facade_lon)?;
        let checked = raster_reader::CheckedRasters::new(&real);
        let rasters = VectorReflectionSampler { inner: &checked, set: &obstacles };
        let receiver = Receiver::new(facade_lat, facade_lon, rasters.elevation(facade_lat, facade_lon));
        let mut traces = TraceCollector::new();
        let production = noise_compute::compute_at_point(
            &receiver,
            &sources.roads,
            &sources.railways,
            &[],
            &[],
            &[],
            &obstacles,
            &rasters,
            Some(&mut traces),
        );
        checked.ensure_valid().map_err(|e| e.to_string())?;
        let reflection_db = rasters.building_enclosure(facade_lat, facade_lon);
        let point = ReceiverPoint { lat: facade_lat, lon: facade_lon, altitude_m: receiver.altitude_m(), reflection_db };
        let world = RealWorld { rasters: &rasters, obstacles: &obstacles };
        let segments = admitted_segments(&sources, &rasters, &receiver);
        let results: Vec<SegmentResult> = segments
            .par_iter()
            .map(|(_, _, segment)| evaluate_segment(&world, segment, &point, spacing))
            .collect();
        let production_by_key: HashMap<SegmentKey, f64> = traces
            .segments
            .iter()
            .filter(|t| matches!(t.kind, LayerKind::Road | LayerKind::Railway))
            .map(|t| {
                let key = segment_key(t.kind == LayerKind::Road, t.osm_id.unwrap_or(0), t.segment_idx, (t.start_lat, t.start_lon));
                (key, t.received_lden.full)
            })
            .collect();
        let mut layers = serde_json::Map::new();
        for layer in [LayerKind::Road, LayerKind::Railway] {
            let production_lden = production.sources.iter().find(|s| s.source_type == layer).map(|s| s.periods.lden_db);
            let summary = layer_summary(layer, &segments, &results, production_lden, &production_by_key);
            layers.insert(layer.as_str().to_owned(), summary);
        }
        out.push(json!({
            "receiver": name, "lat": lat, "lon": lon, "facade_lat": facade_lat, "facade_lon": facade_lon,
            "inside_building": inside.is_some(), "ground_elevation_m": receiver.elevation_m,
            "reflection_boost_db": reflection_db, "layers": layers,
            "seconds": started.elapsed().as_secs_f64(),
        }));
        eprintln!("{name}: {:.1} s", started.elapsed().as_secs_f64());
    }
    Ok(Value::Array(out))
}
