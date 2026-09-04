//! End-to-end coverage of the shuffle → list_square_shards → Stage 2A
//! chain that RunAll wires up.
//!
//! Specifically guards two `/gg`-flagged failure modes from Step 5:
//!  - `shuffle_per_square` must wipe `out_dir` so a previous run's z9
//!    shards can't leak into the next run as zombie data.
//!  - Stage 2A must consume the shuffle output (not an in-memory
//!    Vec) — the regression we hit when the orchestrator still held
//!    the global `all_segments` Vec while consumers had migrated.

use std::path::PathBuf;

use aircraft_extract::arrow_io::{read_record_batches, write_segments};
use aircraft_extract::flight::{FlightSegment, Phase};
use aircraft_extract::shuffle::shuffle_per_square;
use aircraft_extract::spatial::{square_directories, square_id, square_path};
use aircraft_extract::stage_2a::run_stage_2a;

fn seg(flight_id: u64, phase: Phase, lat: f32, lon: f32) -> FlightSegment {
    FlightSegment {
        callsign: format!("FL{flight_id:04}"),
        aircraft_type: *b"A320",
        flight_id,
        profile_idx: 0,
        source_id: 0,
        origin: 0,
        veh_kind: 0,
        gse_class: 0,
        period: 0,
        date_id: 0,
        phase,
        flags: 0,
        start_lat: lat,
        start_lon: lon,
        start_alt_m: 5000.0,
        end_lat: lat + 0.001,
        end_lon: lon + 0.001,
        end_alt_m: 5100.0,
        speed_kt: 300.0,
        length_m: 200.0,
        agl_avg_m: 1000.0,
        start_elev_m: 0.0,
        end_elev_m: 0.0,
    }
}

/// z9 of a `seg()` fixture's MIDPOINT — must match the production path
/// (`shuffle::square_of_midpoint`). Computing from start-only would be a
/// few-metre offset at the 0.001° step used here and happen to land
/// in the same z9 today, but a future test with longer segments
/// would diverge spuriously.
fn square_of_seg_at(lat: f32, lon: f32) -> u64 {
    let mid_lat = (lat + (lat + 0.001)) * 0.5;
    let mid_lon = (lon + (lon + 0.001)) * 0.5;
    square_id(mid_lat as f64, mid_lon as f64).unwrap()
}

fn write_day(segments_dir: &std::path::Path, day: &str, segs: &[FlightSegment]) -> PathBuf {
    let path = segments_dir.join(format!("{day}.arrow"));
    let mut segs = segs.to_vec();
    for seg in &mut segs {
        seg.date_id = aircraft_extract::period::parse_date_id(day).unwrap();
    }
    write_segments(&path, &segs).unwrap();
    path
}

fn list_square_dirs(root: &std::path::Path) -> Vec<u64> {
    square_directories(root)
        .unwrap()
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

/// Happy path: 2 days of mixed-phase segments → shuffle → Stage 2A
/// produces an airborne.arrow per z9 that the segments visited.
#[test]
fn shuffle_then_stage_2a_writes_per_square_outputs() {
    let tmp = tempfile::tempdir().unwrap();
    let segments_dir = tmp.path().join("segments");
    let by_square_dir = tmp.path().join("segments_by_square");
    let prepared_year_dir = tmp.path().join("prepared_year");
    std::fs::create_dir_all(&segments_dir).unwrap();

    let cz_lat = 50.10;
    let cz_lon = 14.26;
    let nyc_lat = 40.71;
    let nyc_lon = -74.00;

    let day1 = vec![
        seg(1, Phase::Airborne, cz_lat, cz_lon),
        seg(2, Phase::Ground, cz_lat, cz_lon),
        seg(3, Phase::Cruise, cz_lat, cz_lon), // dropped by shuffle
        seg(4, Phase::Airborne, nyc_lat, nyc_lon),
    ];
    let day2 = vec![seg(5, Phase::Airborne, cz_lat, cz_lon)];
    let day1_path = write_day(&segments_dir, "2025-01-21", &day1);
    let day2_path = write_day(&segments_dir, "2025-01-22", &day2);

    shuffle_per_square(&[day1_path, day2_path], &[], &by_square_dir, None).unwrap();

    let square_cz = square_of_seg_at(cz_lat, cz_lon);
    let square_nyc = square_of_seg_at(nyc_lat, nyc_lon);
    assert_eq!(
        list_square_dirs(&by_square_dir),
        {
            let mut v = vec![square_cz, square_nyc];
            v.sort_unstable();
            v
        },
        "shuffle output dir must contain exactly the two visited z9s"
    );

    let n_square = run_stage_2a(&by_square_dir, &prepared_year_dir, 2, 0, None).unwrap();
    assert_eq!(
        n_square, 2,
        "Stage 2A should emit airborne.arrow for both z9s"
    );

    let cz_airborne_path = prepared_year_dir
        .join(square_path(square_cz))
        .join("airborne.arrow");
    let nyc_airborne_path = prepared_year_dir
        .join(square_path(square_nyc))
        .join("airborne.arrow");
    assert!(cz_airborne_path.exists());
    assert!(nyc_airborne_path.exists());
    // CZ saw 2 airborne flights (fid=1, fid=5); NYC saw 1 (fid=4).
    let (_, cz_batches) = read_record_batches(&cz_airborne_path).unwrap();
    let (_, nyc_batches) = read_record_batches(&nyc_airborne_path).unwrap();
    let cz_rows: usize = cz_batches.iter().map(|b| b.num_rows()).sum();
    let nyc_rows: usize = nyc_batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(cz_rows, 2, "CZ airborne should have 2 flight rows");
    assert_eq!(nyc_rows, 1, "NYC airborne should have 1 flight row");
}

/// `shuffle_per_square` must wipe `out_dir` at start. Without this, a
/// narrower-scope rerun (or a recovery from a crashed run) would mix
/// stale z9 shards into the new shuffle output and feed Stage 1.5 /
/// 2A / 2C zombie data — the C1 CRITICAL caught at /gg review.
#[test]
fn second_shuffle_wipes_stale_square_shards() {
    let tmp = tempfile::tempdir().unwrap();
    let segments_dir = tmp.path().join("segments");
    let by_square_dir = tmp.path().join("segments_by_square");
    std::fs::create_dir_all(&segments_dir).unwrap();

    // First run: two z9s touched.
    let cz_lat = 50.10;
    let cz_lon = 14.26;
    let nyc_lat = 40.71;
    let nyc_lon = -74.00;
    let day_path = write_day(
        &segments_dir,
        "2025-01-21",
        &[
            seg(1, Phase::Airborne, cz_lat, cz_lon),
            seg(2, Phase::Airborne, nyc_lat, nyc_lon),
        ],
    );
    shuffle_per_square(std::slice::from_ref(&day_path), &[], &by_square_dir, None).unwrap();
    let square_cz = square_of_seg_at(cz_lat, cz_lon);
    let square_nyc = square_of_seg_at(nyc_lat, nyc_lon);
    assert!(list_square_dirs(&by_square_dir).contains(&square_nyc));

    // Second run: only the CZ segment. NYC z9 dir must be wiped.
    let cz_only = vec![seg(99, Phase::Airborne, cz_lat, cz_lon)];
    let cz_only_path = write_day(&segments_dir, "2025-01-22", &cz_only);
    shuffle_per_square(&[cz_only_path], &[], &by_square_dir, None).unwrap();

    let after = list_square_dirs(&by_square_dir);
    assert_eq!(
        after,
        vec![square_cz],
        "second shuffle must wipe the stale NYC z9 shard"
    );
}

/// `shuffle_per_square` with empty input creates an empty `out_dir` and
/// no temp dir. Stage 2A then sees zero z9 dirs and is a no-op.
#[test]
fn empty_input_pipeline_is_a_clean_noop() {
    let tmp = tempfile::tempdir().unwrap();
    let by_square_dir = tmp.path().join("segments_by_square");
    let prepared_year_dir = tmp.path().join("prepared_year");

    shuffle_per_square(&[], &[], &by_square_dir, None).unwrap();
    assert!(by_square_dir.exists(), "out_dir must be created");
    assert!(!tmp.path().join("temp_shuffle").exists());
    assert!(list_square_dirs(&by_square_dir).is_empty());

    let n = run_stage_2a(&by_square_dir, &prepared_year_dir, 1, 0, None).unwrap();
    assert_eq!(n, 0);
}
