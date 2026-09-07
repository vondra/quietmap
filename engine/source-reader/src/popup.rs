//! The popup's point computation over a loaded ring, shared by the N-API entry and the standalone runner.

use std::path::Path;

use crate::hex_store::HexData;
use crate::{
    aircraft_v6, apply_segment_top_k_with_cap, collect_from_hex_data, structure_store, wire,
};

/// Per-kind segment cap of the popup's default answer.
pub const SEGMENT_TOP_K_PER_KIND: usize = 150;

/// Upper bound for the "Show all" response. Higher than the default cap but
/// still bounded — NAPI's string return cannot carry the full airport payload
/// (millions of segments) and browsers can't parse it either.
pub const SEGMENT_TOP_K_PER_KIND_FULL: usize = 1000;

/// Full noise at a point from the ring's hexes: collect the sources, build
/// the exact obstacle set, move an indoor click to its façade, run every
/// kernel, cap the traces and build the wire shape. `data_dir` is the
/// prepared root (`…/data/prepared`) that holds the obstacle-index cache.
pub fn compute_point(
    hexes: &[&HexData],
    lat: f64,
    lng: f64,
    top_k_per_kind: usize,
    h3r4_dir: &Path,
    data_dir: &Path,
    rasters: &dyn noise_compute::types::RasterSampler,
) -> Result<wire::WireResult, String> {
    // Per-stage timing probes (env-gated: `POPUP_TIMING=1` to enable). Inline
    // `Instant::now()` is cheaper and less destructive than perf/flamegraph
    // for popup-scale work, and lets us watch one number per stage land in
    // the Fastify log per request.
    let timing_on = std::env::var("POPUP_TIMING").as_deref() == Ok("1");
    let t_start = std::time::Instant::now();
    // Sibling of the h3r4 dir under `aircraft/` (Stage 2C v5 reduce output);
    // a missing file means zero airport-level counts in the popup.
    let airport_summary_pathbuf = h3r4_dir
        .parent()
        .map(|p| p.join("aircraft").join("airport_summary.arrow"));

    // Sample receiver elevation up-front so the aircraft kernels see a
    // real ground reference. With the stub rasters (offline tests),
    // elevation is 0.0.
    let elevation = rasters.elevation(lat, lng);
    let sources = collect_from_hex_data(hexes, lat, lng)?;
    let t_collect = t_start.elapsed();

    let n_airborne = sources
        .aircraft_airborne_batches
        .iter()
        .map(|b| b.num_rows())
        .sum::<usize>();
    let n_cruise = sources
        .aircraft_cruise_batches
        .iter()
        .map(|b| b.num_rows())
        .sum::<usize>();
    let n_traffic = sources
        .aircraft_airport_traffic_batches
        .iter()
        .map(|b| b.num_rows())
        .sum::<usize>();
    let n_aircraft = n_airborne + n_cruise + n_traffic;
    let n_roads = sources.roads.len();
    let n_railways = sources.railways.len();

    // Vector obstacles: the exact building crossings screening runs on, built
    // per query from the ring-1 obstacle shards. There is no other building
    // representation, so a store that will not load fails the query.
    let obstacle_set = structure_store::load_obstacle_set(h3r4_dir, data_dir, lat, lng)?;
    // Select the enclosed footprint winner once; it supplies the effective
    // envelope delta for the aggregate indoor estimate while traces stay at
    // façade values.
    let inside_envelope = structure_store::point_inside_enclosed(&obstacle_set, lat, lng);
    // Search outward in one-metre cardinal steps using the same containment
    // rule. The ≤100 m shift stays inside the loaded R4 ring, so sources need
    // no reload.
    let (facade_lat, facade_lng) = if inside_envelope.is_some() {
        let step_lat = 1.0 / noise_compute::constants::M_PER_DEG_LAT;
        let step_lon = 1.0 / noise_compute::constants::m_per_deg_lon(lat.to_radians());
        let mut outside = None;
        {
            let set = &obstacle_set;
            for distance in 1..=100 {
                for (dy, dx) in [(1.0, 0.0), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0)] {
                    let candidate = (
                        lat + dy * distance as f64 * step_lat,
                        lng + dx * distance as f64 * step_lon,
                    );
                    if structure_store::point_inside_enclosed(set, candidate.0, candidate.1)
                        .is_none()
                    {
                        outside = Some(candidate);
                        break;
                    }
                }
                if outside.is_some() {
                    break;
                }
            }
        }
        outside.unwrap_or((lat, lng))
    } else {
        (lat, lng)
    };
    // 1.4b: with a loaded store, the receiver reflection probe answers from
    // exact footprints too (the popup twin of the pipeline rx_refl pre-bake)
    // — one wrapped sampler serves EVERY popup kernel.
    let vector_refl = Some(&obstacle_set).map(|set| {
        noise_compute::propagation::obstacle_index::VectorReflectionSampler {
            inner: rasters,
            set,
        }
    });
    let rasters: &dyn noise_compute::types::RasterSampler = match &vector_refl {
        Some(w) => w,
        None => rasters,
    };
    let receiver = noise_compute::types::Receiver::new(
        facade_lat,
        facade_lng,
        rasters.elevation(facade_lat, facade_lng),
    );

    let mut traces = noise_compute::types::TraceCollector::new();
    let mut result = noise_compute::compute_at_point(
        &receiver,
        &sources.roads,
        &sources.railways,
        &sources.buildings,
        &sources.industrial,
        &obstacle_set,
        rasters,
        Some(&mut traces),
    );
    aircraft_v6::add_v6_aircraft_to_result(
        &mut result,
        &mut traces,
        &receiver,
        &sources.aircraft_airborne_batches,
        &sources.aircraft_cruise_batches,
        &sources.aircraft_airport_traffic_batches,
        &sources.airport_lines_batches,
        airport_summary_pathbuf.as_deref(),
        rasters,
        &obstacle_set,
        sources.n_days,
        top_k_per_kind,
    )?;
    let t_compute = t_start.elapsed() - t_collect;

    let summary = apply_segment_top_k_with_cap(&mut traces, top_k_per_kind);
    result.segments = std::mem::take(&mut traces.segments);
    result.segments_meta = Some(summary);

    // Stamp wrapper timings before serializing so the popup JSON carries
    // the full per-component breakdown for the frontend debug overlay.
    if let Some(t) = result.timings.as_mut() {
        t.collect_ms = t_collect.as_secs_f64() * 1000.0;
    }
    let facade_lden = result.total.lden_db;
    let indoor = inside_envelope.and_then(|winner| {
        winner
            .effective_class
            .delta_db()
            .map(|delta| (winner.stored_class, delta))
    });
    // Inside a building the popup publishes the indoor estimate in every level
    // row, the same quantity the painted tile stores per layer.
    noise_compute::present::project_result_to_indoor_display(
        &mut result,
        indoor.map(|(_, delta)| delta),
    );
    if timing_on {
        eprintln!(
            "popup-timing total={:.0}ms collect={:.0}ms compute={:.0}ms (rd={} rl={} ac={})",
            t_start.elapsed().as_secs_f64() * 1000.0,
            t_collect.as_secs_f64() * 1000.0,
            t_compute.as_secs_f64() * 1000.0,
            n_roads,
            n_railways,
            n_aircraft,
        );
    }

    Ok(wire::build_wire_result(
        result,
        lat,
        lng,
        elevation,
        indoor.map(|(class, delta)| (class, delta, facade_lden)),
    ))
}
