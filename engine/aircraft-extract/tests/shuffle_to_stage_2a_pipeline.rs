//! Actual shuffle and Stage2A IPC preserve support coverage, original multiplicity, and rerun cleanup.

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

fn support_of(segment: &FlightSegment) -> Vec<u64> {
    let mut ids: Vec<_> = aircraft_extract::support::airborne_segment_support(segment)
        .unwrap()
        .iter()
        .map(|square| grid::square_id(square) as u64)
        .collect();
    ids.sort_unstable();
    ids
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

    let square_cz = square_id(cz_lat as f64, cz_lon as f64).unwrap();
    let square_nyc = square_id(nyc_lat as f64, nyc_lon as f64).unwrap();
    let mut expected = support_of(&day1[0]);
    expected.extend(support_of(&day1[3]));
    expected.sort_unstable();
    expected.dedup();
    assert_eq!(list_square_dirs(&by_square_dir), expected);
    let n_square = run_stage_2a(&by_square_dir, &prepared_year_dir, 2, 0, None).unwrap();
    assert_eq!(n_square, expected.len());

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
    let square_nyc = square_id(nyc_lat as f64, nyc_lon as f64).unwrap();
    assert!(list_square_dirs(&by_square_dir).contains(&square_nyc));

    // Second run: only the CZ segment. NYC z9 dir must be wiped.
    let cz_only = vec![seg(99, Phase::Airborne, cz_lat, cz_lon)];
    let cz_only_path = write_day(&segments_dir, "2025-01-22", &cz_only);
    shuffle_per_square(&[cz_only_path], &[], &by_square_dir, None).unwrap();

    let after = list_square_dirs(&by_square_dir);
    assert_eq!(
        after,
        support_of(&cz_only[0]),
        "second shuffle must replace the complete airborne support footprint"
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

#[test]
fn intact_support_copies_preserve_ipc_and_multiplicity_at_long_polar_and_seam_receivers() {
    use aircraft_extract::arrow_io::read_segments;
    use aircraft_extract::scope::ScopeBbox;
    use arrow::array::ListArray;
    for (start, end, receiver, scoped) in [
        ([52.001, 14.26], [50.001, 14.26], [50.001, 14.261], false),
        ([52.001, 14.26], [50.001, 14.26], [50.001, 14.261], true),
        (
            [80.178_71, 0.0],
            [80.178_71, 0.001],
            [80.05804856215623, 0.0],
            false,
        ),
        ([82.0, 0.0], [80.0, 0.0], [80.0, 0.001], false),
        ([0.0, 179.99], [0.0, -179.99], [0.001, 180.0], false),
        ([0.0, 0.0], [0.001, 0.001], [0.0, 0.0], false),
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
        let scope = scoped.then(|| ScopeBbox::parse("50.001,14.261,50.001,14.261").unwrap());
        let by_square = tmp.path().join("shuffled");
        shuffle_per_square(std::slice::from_ref(&day), &[], &by_square, scope.as_ref()).unwrap();
        let receiver_square = square_id(receiver[0], receiver[1]).unwrap();
        let expected: Vec<_> = support_of(&airborne)
            .into_iter()
            .filter(|&id| scope.as_ref().is_none_or(|scope| scope.contains_square(id)))
            .collect();
        assert!(expected.contains(&receiver_square));
        let output = tmp.path().join("prepared");
        assert_eq!(
            run_stage_2a(&by_square, &output, 12, 0, scope.as_ref()).unwrap(),
            expected.len()
        );
        let original = read_segments(&day).unwrap();
        let reference_input = tmp.path().join("reference_input");
        write_segments(
            &reference_input
                .join(square_path(receiver_square))
                .join("airborne.arrow"),
            &original[..2],
        )
        .unwrap();
        let reference_output = tmp.path().join("reference_output");
        run_stage_2a(&reference_input, &reference_output, 12, 0, None).unwrap();
        let reference = read_record_batches(
            &reference_output
                .join(square_path(receiver_square))
                .join("airborne.arrow"),
        )
        .unwrap();
        let subs = reference.1[0]
            .column_by_name("sub_segments")
            .unwrap()
            .as_any()
            .downcast_ref::<ListArray>()
            .unwrap();
        assert_eq!(
            subs.value_length(0),
            2,
            "identical original observations both contribute"
        );
        for id in expected {
            assert_eq!(
                read_record_batches(&output.join(square_path(id)).join("airborne.arrow")).unwrap(),
                reference
            );
        }
        let (mid_lat, mid_lon) =
            aircraft_extract::geo::midpoint(start[0], start[1], end[0], end[1]);
        let ground_owner = square_id(f64::from(mid_lat), f64::from(mid_lon)).unwrap();
        let ground_paths =
            aircraft_extract::shuffle::list_square_shards(&by_square, "ground.arrow", None)
                .unwrap();
        if scoped {
            assert!(
                !scope.unwrap().contains_square(ground_owner),
                "source owner must be outside this destination scope"
            );
            assert!(ground_paths.is_empty());
        } else {
            assert_eq!(ground_paths.len(), 1);
            assert_eq!(ground_paths[0].0, ground_owner);
            let ground_reference = tmp.path().join("ground_reference.arrow");
            write_segments(&ground_reference, &original[2..]).unwrap();
            assert_eq!(
                read_record_batches(&ground_paths[0].1).unwrap(),
                read_record_batches(&ground_reference).unwrap()
            );
        }
    }
}
