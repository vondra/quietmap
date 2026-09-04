//! Terrain endpoint gates and optional real-source integration.
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
