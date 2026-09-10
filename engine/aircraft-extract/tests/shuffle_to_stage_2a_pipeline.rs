//! Actual shuffle and Stage 2A IPC store every sub-segment once, split long chords, and clean reruns.

use std::collections::BTreeMap;
use std::path::PathBuf;

use aircraft_extract::arrow_io::{read_record_batches, write_segments};
use aircraft_extract::flight::{FlightSegment, Phase};
use aircraft_extract::shuffle::shuffle_per_square;
use aircraft_extract::spatial::{square_directories, square_id};
use aircraft_extract::stage_2a::run_stage_2a;
use arrow::array::{Array, Float32Array, Int32Array, UInt64Array};
use noise_compute::emission::aircraft::AIRBORNE_SUB_SEGMENT_MAX_LENGTH_M;

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

/// Every prepared airborne row under `prepared_year`, keyed by its decoded
/// start point and flight: `(square, flight_id, length_m, start_lon, end_lon)`.
fn prepared_rows(prepared_year: &std::path::Path) -> Vec<(u64, u64, f32, f64, f64)> {
    let mut rows = Vec::new();
    for (square, directory) in square_directories(prepared_year).unwrap() {
        let (_, batches) = read_record_batches(&directory.join("airborne.arrow")).unwrap();
        for batch in &batches {
            let column = |name: &str| batch.column_by_name(name).unwrap();
            let flight_id = column("flight_id")
                .as_any()
                .downcast_ref::<UInt64Array>()
                .unwrap();
            let length = column("length_m")
                .as_any()
                .downcast_ref::<Float32Array>()
                .unwrap();
            let lon_of = |name: &str| {
                let gx = column(name).as_any().downcast_ref::<Int32Array>().unwrap();
                gx.values()
                    .iter()
                    .map(|&gx| square_store::grid_cols::grid_cell_lonlat(gx, 0).0)
                    .collect::<Vec<_>>()
            };
            let (start_lon, end_lon) = (lon_of("start_gx"), lon_of("end_gx"));
            for i in 0..batch.num_rows() {
                rows.push((
                    square,
                    flight_id.value(i),
                    length.value(i),
                    start_lon[i],
                    end_lon[i],
                ));
            }
        }
    }
    rows
}

/// Happy path: 2 days of mixed-phase segments → shuffle → Stage 2A
/// produces one airborne.arrow per owner z9 with one row per sub-segment.
#[test]
fn shuffle_then_stage_2a_writes_one_row_per_sub_segment_in_the_owner_square() {
    let tmp = tempfile::tempdir().unwrap();
    let segments_dir = tmp.path().join("segments");
    let by_square_dir = tmp.path().join("segments_by_square");
    let prepared_year_dir = tmp.path().join("prepared_year");
    std::fs::create_dir_all(&segments_dir).unwrap();

    let (cz_lat, cz_lon) = (50.10, 14.26);
    let (nyc_lat, nyc_lon) = (40.71, -74.00);
    let day1 = vec![
        seg(1, Phase::Airborne, cz_lat, cz_lon),
        seg(2, Phase::Ground, cz_lat, cz_lon),
        seg(3, Phase::Cruise, cz_lat, cz_lon), // dropped by shuffle
        seg(4, Phase::Airborne, nyc_lat, nyc_lon),
    ];
    let day2 = vec![
        seg(5, Phase::Airborne, cz_lat, cz_lon),
        seg(5, Phase::Airborne, cz_lat + 0.002, cz_lon),
    ];
    let day1_path = write_day(&segments_dir, "2025-01-21", &day1);
    let day2_path = write_day(&segments_dir, "2025-01-22", &day2);
    shuffle_per_square(&[day1_path, day2_path], &[], &by_square_dir, None).unwrap();

    let square_cz = square_id(cz_lat as f64, cz_lon as f64).unwrap();
    let square_nyc = square_id(nyc_lat as f64, nyc_lon as f64).unwrap();
    let mut expected = vec![square_cz, square_nyc];
    expected.sort_unstable();
    assert_eq!(list_square_dirs(&by_square_dir), expected);
    assert_eq!(
        run_stage_2a(&by_square_dir, &prepared_year_dir, 2, 0, None).unwrap(),
        2
    );
    let rows = prepared_rows(&prepared_year_dir);
    let mut per_square: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    for (square, flight_id, ..) in &rows {
        per_square.entry(*square).or_default().push(*flight_id);
    }
    assert_eq!(
        per_square[&square_cz],
        [1, 5, 5],
        "flight 5 has two rows in CZ"
    );
    assert_eq!(per_square[&square_nyc], [4]);
}

/// Canonical ownership: a sub-segment lives in exactly one square, and a
/// chord longer than the cap is stored as `ceil(length / cap)` pieces of
/// equal length whose geometry chains across the owner squares it crosses.
#[test]
fn long_chord_is_split_once_across_its_owner_squares() {
    let tmp = tempfile::tempdir().unwrap();
    let segments_dir = tmp.path().join("segments");
    std::fs::create_dir_all(&segments_dir).unwrap();
    // 18 km due east at 50 °N: crosses a z9 column boundary near 14.0625° E.
    let mut chord = seg(42, Phase::Airborne, 50.0, 13.95);
    chord.end_lat = 50.0;
    chord.end_lon = 13.95 + 18_000.0 / (111_320.0 * 50.0_f32.to_radians().cos());
    chord.length_m = aircraft_extract::geo::flat_dist(
        chord.start_lat,
        chord.start_lon,
        chord.end_lat,
        chord.end_lon,
    );
    let short = seg(7, Phase::Airborne, 50.0, 13.95);
    let day = write_day(&segments_dir, "2025-07-01", &[chord.clone(), short]);
    let by_square = tmp.path().join("shuffled");
    shuffle_per_square(std::slice::from_ref(&day), &[], &by_square, None).unwrap();
    let prepared = tmp.path().join("prepared");
    assert_eq!(run_stage_2a(&by_square, &prepared, 12, 0, None).unwrap(), 2);

    let mut rows = prepared_rows(&prepared);
    rows.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pieces: Vec<_> = rows.iter().filter(|row| row.1 == 42).collect();
    assert_eq!(pieces.len(), 5, "ceil(18 km / 4 km)");
    assert!(rows
        .iter()
        .all(|row| row.2 <= AIRBORNE_SUB_SEGMENT_MAX_LENGTH_M));
    assert!(pieces
        .iter()
        .all(|row| (row.2 - chord.length_m / 5.0).abs() < 0.05));
    let mut owners: Vec<u64> = pieces.iter().map(|row| row.0).collect();
    owners.dedup();
    assert_eq!(
        owners.len(),
        2,
        "pieces are owned by both squares the chord crosses"
    );
    // The pieces chain start-to-end with no gap and no overlap: one polyline
    // of the chord's length, each vertex stored exactly once.
    let mut by_start: Vec<(f64, f64)> = pieces.iter().map(|row| (row.3, row.4)).collect();
    by_start.sort_by(|a, b| a.partial_cmp(b).unwrap());
    for pair in by_start.windows(2) {
        assert_eq!(pair[0].1, pair[1].0, "piece ends where the next starts");
    }
    let unsplit: Vec<_> = rows.iter().filter(|row| row.1 == 7).collect();
    assert_eq!(unsplit.len(), 1);
    assert_eq!(unsplit[0].2, 200.0);
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
    let (cz_lat, cz_lon) = (50.10, 14.26);
    let (nyc_lat, nyc_lon) = (40.71, -74.00);
    let day_path = write_day(
        &segments_dir,
        "2025-01-21",
        &[
            seg(1, Phase::Airborne, cz_lat, cz_lon),
            seg(2, Phase::Airborne, nyc_lat, nyc_lon),
        ],
    );
    shuffle_per_square(std::slice::from_ref(&day_path), &[], &by_square_dir, None).unwrap();
    let square_nyc = square_id(nyc_lat as f64, nyc_lon as f64).unwrap();
    assert!(list_square_dirs(&by_square_dir).contains(&square_nyc));

    // Second run: only the CZ segment. NYC z9 dir must be wiped.
    let cz_only = vec![seg(99, Phase::Airborne, cz_lat, cz_lon)];
    let cz_only_path = write_day(&segments_dir, "2025-01-22", &cz_only);
    shuffle_per_square(&[cz_only_path], &[], &by_square_dir, None).unwrap();

    assert_eq!(
        list_square_dirs(&by_square_dir),
        [square_id(cz_lat as f64, cz_lon as f64).unwrap()],
        "second shuffle must replace the complete owner footprint"
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

/// Owner rows keep their multiplicity and bytes at polar and seam
/// midpoints, and a scope keeps only the pieces whose own midpoint is inside.
#[test]
fn owner_shards_preserve_multiplicity_at_polar_and_seam_midpoints_and_honour_scope() {
    for (start, end, scoped) in [
        ([52.001, 14.26], [50.001, 14.26], false),
        ([52.001, 14.26], [50.001, 14.26], true),
        ([80.178_71, 0.0], [80.178_71, 0.001], false),
        ([82.0, 0.0], [80.0, 0.0], false),
        ([0.0, 179.99], [0.0, -179.99], false),
        ([0.0, 0.0], [0.001, 0.001], false),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let mut airborne = seg(42, Phase::Airborne, start[0], start[1]);
        airborne.end_lat = end[0];
        airborne.end_lon = end[1];
        airborne.length_m = aircraft_extract::geo::flat_dist(start[0], start[1], end[0], end[1]);
        let mut ground = airborne.clone();
        ground.flight_id = 99;
        ground.phase = Phase::Ground;
        let day = write_day(
            &tmp.path().join("segments"),
            "2025-07-01",
            &[airborne.clone(), airborne.clone(), ground],
        );
        let scope = scoped.then(|| {
            aircraft_extract::scope::ScopeBbox::parse("50.001,14.261,50.001,14.261").unwrap()
        });
        let by_square = tmp.path().join("shuffled");
        shuffle_per_square(std::slice::from_ref(&day), &[], &by_square, scope.as_ref()).unwrap();
        let output = tmp.path().join("prepared");
        let written = run_stage_2a(&by_square, &output, 12, 0, scope.as_ref()).unwrap();
        let rows = prepared_rows(&output);
        let pieces = (airborne.length_m / AIRBORNE_SUB_SEGMENT_MAX_LENGTH_M)
            .ceil()
            .max(1.0);
        if scoped {
            // The scope is one point inside the southern square: only the
            // pieces owned there survive, and they survive twice.
            let scope = scope.unwrap();
            assert!(written >= 1);
            assert!(rows.iter().all(|row| scope.contains_square(row.0)));
            assert!(!rows.is_empty() && rows.len().is_multiple_of(2));
        } else {
            assert_eq!(rows.len(), 2 * pieces as usize, "{start:?}->{end:?}");
            // Two identical original observations remain two rows per owner.
            let mut per_square: BTreeMap<u64, usize> = BTreeMap::new();
            for row in &rows {
                *per_square.entry(row.0).or_default() += 1;
            }
            assert!(per_square.values().all(|count| count % 2 == 0));
            assert_eq!(per_square.len(), written);
            let (mid_lat, mid_lon) =
                aircraft_extract::geo::midpoint(start[0], start[1], end[0], end[1]);
            let ground_owner = square_id(f64::from(mid_lat), f64::from(mid_lon)).unwrap();
            let ground_paths =
                aircraft_extract::shuffle::list_square_shards(&by_square, "ground.arrow", None)
                    .unwrap();
            assert_eq!(ground_paths.len(), 1);
            assert_eq!(ground_paths[0].0, ground_owner);
        }
    }
}
