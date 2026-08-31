//! Stage 1 — flights → segments. Per flight we sample DEM AGL,
//! truncate the bogus tail with [`crate::filters::validate_flight_trajectory`],
//! infer composite ground flags, classify phase, then build segments.
//!
//! Output: `segments/<day>.arrow` (one row per surviving segment).

use std::path::Path;

use anyhow::{Context, Result};
use rayon::prelude::*;

use crate::arrow_io::{read_record_batches, write_segments};
use crate::classify::{self, ClassifyInput};
use crate::filters;
use crate::flight::{typecode_bytes, Flight, FlightSegment};
use crate::ground_inference::ground_flags;
use crate::period::parse_date_id;
use crate::progress::{finished, started, Milestone};
use crate::segment::{build_segments, SegmentMeta};
use crate::trace::TracePoint;
use raster_reader::RealRasters;

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
    let date_id = parse_date_id(day_str);
    let n_flights = flights.len();
    started("stage1", &format!("day={day_str}, {n_flights} flights"));

    // Pre-load DEM tiles covering the flight bbox so per-point lookups
    // are lock-free during rayon par_iter.
    if let Some(bbox) = bbox_of_flights(&flights) {
        rasters.dem.preload_bbox(bbox.0, bbox.1, bbox.2, bbox.3);
    }

    // Heavy work: per-flight AGL + truncate + ground + classify + segments.
    let flight_counter = Milestone::new("stage1", "flights", 1_000);
    let segments: Vec<FlightSegment> = flights
        .par_iter()
        .flat_map_iter(|f| {
            let out = stage_1_one_flight(f, rasters, date_id);
            flight_counter.add(1);
            out.into_iter()
        })
        .collect();

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

fn stage_1_one_flight(flight: &Flight, rasters: &RealRasters, date_id: i16) -> Vec<FlightSegment> {
    if flight.points.len() < 2 {
        return Vec::new();
    }
    // Mirror of the Stage 0 glider drop (`trace_to_flight`): cached
    // `flights/<day>.arrow` written by pre-filter code still carries
    // sailplane traces, and the stage-reuse workflow (`--from-stage
    // stage1`) re-reads those caches without re-running Stage 0.
    if crate::profile::is_negligible_noise_typecode(&flight.aircraft_type) {
        return Vec::new();
    }

    let mut points = flight.points.clone();
    // Ground-flagged points pin AGL = 0 (and skip the DEM lookup); without
    // this, `validate_flight_trajectory` would read `0 - elev` for any
    // landing roll above ~300 m AMSL and truncate the whole tail.
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
    // typically stays in 1-2 DEM tiles (1° × 1°), so >99 % of lookups
    // hit the cache and skip the per-tile mutex + global use_counter atomic.
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
        ) as f32;
        elev_m.push(elev);
        match p.airborne_alt_ft() {
            None => agl_m.push(0.0),
            Some(alt_ft) => agl_m.push(alt_ft * 0.3048 - elev),
        }
    }

    filters::validate_flight_trajectory(&mut points, &mut agl_m, &mut elev_m);
    if points.len() < 2 {
        return Vec::new();
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
    segments
        .into_iter()
        .filter(airborne_endpoints_above_terrain)
        .collect()
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

fn bbox_of_flights(flights: &[Flight]) -> Option<(f64, f64, f64, f64)> {
    let mut min_lat = f64::MAX;
    let mut max_lat = f64::MIN;
    let mut min_lon = f64::MAX;
    let mut max_lon = f64::MIN;
    let mut any = false;
    for f in flights {
        for p in &f.points {
            min_lat = min_lat.min(p.lat as f64);
            max_lat = max_lat.max(p.lat as f64);
            min_lon = min_lon.min(p.lon as f64);
            max_lon = max_lon.max(p.lon as f64);
            any = true;
        }
    }
    if !any {
        return None;
    }
    Some((min_lat, max_lat, min_lon, max_lon))
}

/// Read a Stage 0 flights file back into [`Flight`] structs. Matches the
/// schema produced by [`crate::stage_0::write_flights_at`].
pub fn read_flights(path: &Path) -> Result<Vec<Flight>> {
    use arrow::array::{
        Array, FixedSizeBinaryArray, Float32Array, Float64Array, ListArray, StringArray,
        StructArray, UInt64Array, UInt8Array,
    };

    let (_, batches) = read_record_batches(path)?;
    let mut out = Vec::new();
    for b in batches {
        let flight_id = b
            .column_by_name("flight_id")
            .unwrap()
            .as_any()
            .downcast_ref::<UInt64Array>()
            .unwrap();
        let callsign = b
            .column_by_name("callsign")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        let atype = b
            .column_by_name("aircraft_type")
            .unwrap()
            .as_any()
            .downcast_ref::<FixedSizeBinaryArray>()
            .unwrap();
        let prof = b
            .column_by_name("profile_idx")
            .unwrap()
            .as_any()
            .downcast_ref::<UInt8Array>()
            .unwrap();
        let src = b
            .column_by_name("source_id")
            .unwrap()
            .as_any()
            .downcast_ref::<UInt8Array>()
            .unwrap();
        let orig = b
            .column_by_name("origin")
            .unwrap()
            .as_any()
            .downcast_ref::<UInt8Array>()
            .unwrap();
        let veh_kind = b
            .column_by_name("veh_kind")
            .unwrap()
            .as_any()
            .downcast_ref::<UInt8Array>()
            .unwrap();
        let gse_class = b
            .column_by_name("gse_class")
            .unwrap()
            .as_any()
            .downcast_ref::<UInt8Array>()
            .unwrap();
        let base_ts = b
            .column_by_name("base_timestamp")
            .unwrap()
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        let pts_list = b
            .column_by_name("points")
            .unwrap()
            .as_any()
            .downcast_ref::<ListArray>()
            .unwrap();
        let pts_struct = pts_list
            .values()
            .as_any()
            .downcast_ref::<StructArray>()
            .unwrap();
        let pt_ts = pts_struct
            .column(0)
            .as_any()
            .downcast_ref::<Float32Array>()
            .unwrap();
        let pt_lat = pts_struct
            .column(1)
            .as_any()
            .downcast_ref::<Float32Array>()
            .unwrap();
        let pt_lon = pts_struct
            .column(2)
            .as_any()
            .downcast_ref::<Float32Array>()
            .unwrap();
        let pt_alt = pts_struct
            .column(3)
            .as_any()
            .downcast_ref::<Float32Array>()
            .unwrap();
        let pt_speed = pts_struct
            .column(4)
            .as_any()
            .downcast_ref::<Float32Array>()
            .unwrap();
        let pt_track = pts_struct
            .column(5)
            .as_any()
            .downcast_ref::<Float32Array>()
            .unwrap();
        let pt_baro = pts_struct
            .column(6)
            .as_any()
            .downcast_ref::<Float32Array>()
            .unwrap();
        let pt_flags = pts_struct
            .column(7)
            .as_any()
            .downcast_ref::<UInt8Array>()
            .unwrap();
        let offsets = pts_list.value_offsets();

        for i in 0..b.num_rows() {
            let lo = offsets[i] as usize;
            let hi = offsets[i + 1] as usize;
            let mut points = Vec::with_capacity(hi - lo);
            let base = base_ts.value(i);
            for j in lo..hi {
                points.push(TracePoint {
                    timestamp: base + pt_ts.value(j) as f64,
                    lat: pt_lat.value(j),
                    lon: pt_lon.value(j),
                    alt_ft: pt_alt.value(j),
                    speed_kt: pt_speed.value(j),
                    track_deg: pt_track.value(j),
                    baro_rate_fpm: pt_baro.value(j),
                    flags: pt_flags.value(j),
                });
            }
            let bytes = atype.value(i);
            let aircraft_type = std::str::from_utf8(bytes)
                .unwrap_or("")
                .trim_end_matches(char::from(0))
                .to_string();
            out.push(Flight {
                flight_id: flight_id.value(i),
                callsign: callsign.value(i).to_string(),
                aircraft_type,
                profile_idx: prof.value(i),
                source_id: src.value(i),
                origin: orig.value(i),
                veh_kind: veh_kind.value(i),
                gse_class: gse_class.value(i),
                points,
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flight::{segment_flags, FlightSegment, Phase};
    use crate::source::FlightSource;
    use crate::source_adsb_tar::AdsbTarSource;
    use tempfile::tempdir;

    fn airborne_seg(start_alt: f32, start_elev: f32, end_alt: f32, end_elev: f32) -> FlightSegment {
        FlightSegment {
            flight_id: 0,
            callsign: String::new(),
            aircraft_type: [0; 4],
            profile_idx: 0,
            source_id: 0,
            origin: 0,
            veh_kind: 0,
            gse_class: 0,
            period: 0,
            date_id: 0,
            phase: Phase::Airborne,
            flags: 0,
            start_lat: 50.0,
            start_lon: 14.0,
            start_alt_m: start_alt,
            end_lat: 50.0,
            end_lon: 14.01,
            end_alt_m: end_alt,
            speed_kt: 100.0,
            length_m: 700.0,
            agl_avg_m: ((start_alt + end_alt) * 0.5) - ((start_elev + end_elev) * 0.5),
            start_elev_m: start_elev,
            end_elev_m: end_elev,
        }
    }

    #[test]
    fn endpoints_above_terrain_drops_underground_airborne() {
        // Endpoint 100 m below terrain — transponder spike, should drop.
        let seg = airborne_seg(500.0, 600.0, 500.0, 500.0);
        assert!(!airborne_endpoints_above_terrain(&seg));
    }

    #[test]
    fn endpoints_above_terrain_keeps_minus_30_boundary() {
        // Exactly at -30 m AGL on both endpoints — inclusive boundary
        // pins behavior against a future `>` rewrite that would flip it.
        let seg = airborne_seg(470.0, 500.0, 470.0, 500.0);
        assert!(airborne_endpoints_above_terrain(&seg));
    }

    #[test]
    fn endpoints_above_terrain_bypasses_on_ground_flag() {
        // ON_GROUND flag wins even when AGL nominally fails.
        let mut seg = airborne_seg(0.0, 500.0, 0.0, 500.0);
        seg.flags |= segment_flags::ON_GROUND;
        assert!(airborne_endpoints_above_terrain(&seg));
    }

    #[test]
    fn endpoints_above_terrain_bypasses_ground_phase() {
        // `Phase::Ground` wins even when AGL nominally fails.
        let mut seg = airborne_seg(0.0, 500.0, 0.0, 500.0);
        seg.phase = Phase::Ground;
        assert!(airborne_endpoints_above_terrain(&seg));
    }

    #[test]
    fn glider_flight_from_stale_stage0_cache_yields_no_segments() {
        // Stage 0 written by pre-glider-filter code can still carry
        // sailplane flights; the Stage 1 mirror drop must catch them.
        let tmp = tempdir().unwrap();
        let rasters = RealRasters::new(tmp.path()); // no tiles → elev 0 m
                                                    // Kinematics must satisfy `segment_is_keepable` for the blank
                                                    // control: blank → FALLBACK (jet-classed), so speed must clear
                                                    // JET_STALL_SPEED_KT. ~250 kt ≈ 3.9 km per 30 s step.
        let mk = |typecode: &str| Flight {
            flight_id: 1,
            callsign: String::new(),
            aircraft_type: typecode.to_string(),
            profile_idx: 123,
            source_id: 0,
            origin: 0,
            veh_kind: 0,
            gse_class: 0,
            points: vec![
                TracePoint {
                    timestamp: 0.0,
                    lat: 47.320,
                    lon: 11.48,
                    alt_ft: 8000.0,
                    speed_kt: 250.0,
                    track_deg: 0.0,
                    baro_rate_fpm: 0.0,
                    flags: 0,
                },
                TracePoint {
                    timestamp: 30.0,
                    lat: 47.355,
                    lon: 11.48,
                    alt_ft: 8000.0,
                    speed_kt: 250.0,
                    track_deg: 0.0,
                    baro_rate_fpm: 0.0,
                    flags: 0,
                },
                TracePoint {
                    timestamp: 60.0,
                    lat: 47.390,
                    lon: 11.48,
                    alt_ft: 8000.0,
                    speed_kt: 250.0,
                    track_deg: 0.0,
                    baro_rate_fpm: 0.0,
                    flags: 0,
                },
            ],
        };
        assert!(stage_1_one_flight(&mk("VENT"), &rasters, 0).is_empty());
        assert!(stage_1_one_flight(&mk("AS21"), &rasters, 0).is_empty());
        // Blank typecode is NOT a glider — it must still segment.
        assert!(!stage_1_one_flight(&mk(""), &rasters, 0).is_empty());
    }

    /// Skips unless QM_FLIGHTS_CACHE (radius cache with 2025/2025-01-21) and
    /// QM_PREPARED_DIR (the prepared data root, cf. PREPARED_DIR in
    /// scripts/run-aircraft-extract.sh) are both set and present.
    #[test]
    fn end_to_end_one_day_against_real_dem() {
        let (Ok(cache), Ok(prepared)) = (
            std::env::var("QM_FLIGHTS_CACHE"),
            std::env::var("QM_PREPARED_DIR"),
        ) else {
            return;
        };
        if !std::path::Path::new(&cache)
            .join("2025/2025-01-21")
            .exists()
            || !std::path::Path::new(&prepared).exists()
        {
            return;
        }

        let work = tempdir().unwrap();
        let stage0_dir = work.path().join("flights");
        let stage1_dir = work.path().join("segments");
        std::fs::create_dir_all(&stage0_dir).unwrap();
        std::fs::create_dir_all(&stage1_dir).unwrap();

        let sources: Vec<Box<dyn FlightSource>> = vec![Box::new(AdsbTarSource::new(cache))];
        crate::stage_0::run_stage_0(&sources, "2025-01-21", &stage0_dir).unwrap();

        let rasters = RealRasters::new(std::path::Path::new(&prepared));
        let n = run_stage_1(&stage0_dir, &stage1_dir, "2025-01-21", &rasters).unwrap();
        assert!(n > 1000, "got only {n} segments");
        let path = stage1_dir.join("2025-01-21.arrow");
        assert!(path.exists());
    }
}
