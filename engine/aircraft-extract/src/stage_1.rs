//! Stage 1 — flights → segments. Per flight we sample DEM AGL,
//! truncate the bogus tail with [`crate::filters::validate_flight_trajectory`],
//! infer composite ground flags, classify phase, then build segments.
//!
//! Output: `segments/<day>.arrow` (one row per surviving segment).

use std::path::Path;

use anyhow::{Context, Result};
use rayon::prelude::*;

use crate::arrow_io::write_segments;
use crate::classify::{self, ClassifyInput};
use crate::filters;
use crate::flight::{typecode_bytes, Flight, FlightSegment};
use crate::ground_inference::ground_flags;
use crate::period::parse_date_id;
use crate::progress::{finished, started, Milestone};
use crate::segment::{build_segments, SegmentMeta};
use raster_reader::{CheckedRasters, RealRasters};

/// Run Stage 1 for one day. Reads `input_dir/<day>.arrow`, writes
/// `output_dir/<day>.arrow`. Caller owns `rasters` so multi-day
/// orchestration can share one tile cache across days.
pub fn run_stage_1(
    input_dir: &Path,
    output_dir: &Path,
    day_str: &str,
    rasters: &RealRasters,
) -> Result<usize> {
    let in_path = input_dir.join(format!("{day_str}.arrow"));
    let flights =
        read_flights(&in_path).with_context(|| format!("read flights {}", in_path.display()))?;
    let date_id = parse_date_id(day_str)?;
    let n_flights = flights.len();
    started("stage1", &format!("day={day_str}, {n_flights} flights"));

    // Last-tile and shared raster caches serve sparse flight paths without a global bbox preload.
    // Heavy work: per-flight AGL + truncate + ground + classify + segments.
    let flight_counter = Milestone::new("stage1", "flights", 1_000);
    let checked = CheckedRasters::new(rasters);
    let segments: Vec<FlightSegment> = flights
        .par_iter()
        .try_fold(Vec::new, |mut segments, flight| {
            segments.extend(stage_1_one_flight(flight, &checked, date_id)?);
            flight_counter.add(1);
            Ok::<_, anyhow::Error>(segments)
        })
        .try_reduce(Vec::new, |mut segments, other| {
            segments.extend(other);
            Ok(segments)
        })?;
    checked.ensure_valid()?;

    let out_path = output_dir.join(format!("{day_str}.arrow"));
    write_segments(&out_path, &segments)?;
    finished(
        "stage1",
        &format!(
            "day={day_str}, {n_flights} flights → {} segments",
            segments.len()
        ),
    );
    Ok(segments.len())
}

fn stage_1_one_flight(
    flight: &Flight,
    rasters: &CheckedRasters<'_>,
    date_id: i16,
) -> Result<Vec<FlightSegment>> {
    if flight.points.len() < 2 {
        return Ok(Vec::new());
    }
    let mut points = flight.points.clone();
    // Ground-flagged points pin AGL = 0, but still need real terrain at segment endpoints.
    let mut agl_m: Vec<f32> = Vec::with_capacity(points.len());
    // Per-point terrain elevation in meters, sampled from `rasters.dem`.
    // For ground-flagged points we still sample the raster so v15
    // sub-segment `terrain_*_elev_m` carries a meaningful value at
    // takeoff / landing endpoints (the popup gates on the on-ground
    // flag earlier, but downstream layers benefit from having a real
    // elevation rather than 0). Stage 1's per-point loop already pays
    // the raster lookup cost; one branch removed is essentially free.
    let mut elev_m: Vec<f32> = Vec::with_capacity(points.len());
    // Caller-threaded DEM cache across the per-point sweep. One flight
    // retains its current z9 mmap, skipping shared-cache work until the next window.
    let mut dem_key = (i32::MIN, i32::MIN);
    let mut dem_tile: Option<std::sync::Arc<raster_reader::RawTile>> = None;
    for p in &points {
        // NN DEM lookup — Stage 1 only ever consumes elevation through
        // hard AGL gates (HARD_AGL_FLOOR_M = -300, 7 620 m phase seed,
        // etc.) with 15-30 m slack. Bilinear blend is unnecessary here.
        let elev = rasters.elevation_nearest_cached(
            p.lat as f64,
            p.lon as f64,
            &mut dem_key,
            &mut dem_tile,
        )? as f32;
        elev_m.push(elev);
        match p.airborne_alt_ft() {
            None => agl_m.push(0.0),
            Some(alt_ft) => agl_m.push(alt_ft * 0.3048 - elev),
        }
    }

    filters::validate_flight_trajectory(&mut points, &mut agl_m, &mut elev_m);
    if points.len() < 2 {
        return Ok(Vec::new());
    }

    let g_flags = ground_flags(&points, &agl_m);
    let phases = classify::classify_points(ClassifyInput {
        on_ground: &g_flags,
        agl_m: &agl_m,
    });

    let meta = SegmentMeta {
        flight_id: flight.flight_id,
        callsign: &flight.callsign,
        aircraft_type: typecode_bytes(&flight.aircraft_type),
        profile_idx: flight.profile_idx,
        source_id: flight.source_id,
        origin: flight.origin,
        veh_kind: flight.veh_kind,
        gse_class: flight.gse_class,
        date_id,
    };
    let segments = build_segments(&points, &agl_m, &elev_m, &phases, &meta);
    // K3 + tightening: chord q1/mid/q3 check from the v15 popup is
    // intentionally NOT carried over — at 1-15 s ADS-B sampling the
    // linear chord between adjacent points tracks the real trajectory
    // well enough for flat-to-rolling terrain. Mountain-airport STAR
    // approaches (LOWI / SEQM / KASE) can in principle pass a chord
    // midpoint under a peak with both endpoints above; accepted for
    // the Praha-150km scope, revisit for global extracts. The
    // jet 150 m AGL floor (popup `segment_filters.rs:252-255`) is
    // not at Stage 1 either — moves to Stage 2A where the resolved
    // aerodrome centroid is available.
    Ok(segments
        .into_iter()
        .filter(airborne_endpoints_above_terrain)
        .collect())
}

/// Endpoint AGL ≥ −30 m gate. Catches Mode-S altitude decode errors
/// and "transponder on but aircraft already landed somewhere unmapped"
/// leakage. Ground-flagged segments bypass — they re-enter through
/// Stage 2C ground ops. The popup's airport-context bypass
/// (`segment_filters.rs:219` `ground_context != GROUND_CONTEXT_NONE`)
/// is NOT mirrored — that field is resolved per-receiver at Stage 2A.
/// The −30 m slack absorbs DEM error near runways and sub-sea airports
/// (AMS −4 m, Atyrau −22 m). NaN-bearing endpoints drop here
/// (popup kept them under `<` semantics, but NaN propagation downstream
/// is worse than early drop).
fn airborne_endpoints_above_terrain(seg: &crate::flight::FlightSegment) -> bool {
    use crate::flight::{segment_flags, Phase};
    if seg.phase == Phase::Ground || (seg.flags & segment_flags::ON_GROUND) != 0 {
        return true;
    }
    let start_agl = (seg.start_alt_m - seg.start_elev_m) as f64;
    let end_agl = (seg.end_alt_m - seg.end_elev_m) as f64;
    start_agl >= -30.0 && end_agl >= -30.0
}

#[path = "stage_1/flight_reader.rs"]
mod flight_reader;
pub use flight_reader::read_flights;

#[cfg(test)]
#[path = "stage_1/tests.rs"]
mod tests;
