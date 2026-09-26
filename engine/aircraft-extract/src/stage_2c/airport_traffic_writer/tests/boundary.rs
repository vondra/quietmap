//! Ground ownership, cross-border normalization, and input integrity regressions.
use super::*;
use crate::arrow_io::{
    read_airport_summaries, read_airport_traffic, write_segments, AirportTrafficRow,
};
use crate::stage_2c::run_stage_2c;

fn leg(lat: f32, lon: f32) -> FlightSegment {
    FlightSegment {
        flight_id: 42,
        callsign: "TEST".into(),
        aircraft_type: *b"B738",
        profile_idx: 23,
        source_id: 0,
        origin: 0,
        veh_kind: 0,
        gse_class: 0,
        period: 0,
        date_id: 0,
        phase: Phase::Ground,
        flags: 0,
        start_lat: lat,
        start_lon: lon - 0.0001,
        start_alt_m: 0.0,
        end_lat: lat + 0.002,
        end_lon: grid::geo::normalize_longitude(f64::from(lon) + 0.0002) as f32,
        end_alt_m: 0.0,
        speed_kt: 30.0,
        length_m: crate::geo::flat_dist(lat, lon - 0.0001, lat + 0.002, lon + 0.0002),
        agl_avg_m: 0.0,
        start_elev_m: 0.0,
        end_elev_m: 0.0,
        departure_field_elev_m: f32::NAN,
    }
}

fn partitioned_run(lat: f64, lon: f64, split: bool) -> Vec<AirportTrafficRow> {
    let temp = tempfile::tempdir().unwrap();
    let prepared = temp.path().join("2025");
    let inputs = temp.path().join("segments");
    let left = crate::spatial::square_id(lat, lon - 0.0003).unwrap();
    let right = crate::spatial::square_id(lat, lon + 0.0003).unwrap();
    assert_ne!(left, right);
    let make_line = |id, offset| FakeRealLine {
        osm_id: id,
        segment_idx: 0,
        start_lat: lat,
        end_lat: lat + 0.002,
        start_lon: grid::geo::normalize_longitude(lon + offset),
        end_lon: grid::geo::normalize_longitude(lon + offset),
        length_m: crate::geo::flat_dist(lat as f32, 0.0, (lat + 0.002) as f32, 0.0),
        aeroway_type: 1,
    };
    if split {
        for (owner, line) in [(left, make_line(1, -0.0003)), (right, make_line(2, 0.0003))] {
            write_real_airport_lines_arrow(
                &prepared
                    .join(square_path(owner))
                    .join("airport_lines.arrow"),
                &[line],
            );
        }
    } else {
        write_real_airport_lines_arrow(
            &prepared
                .join(square_path(right))
                .join("airport_lines.arrow"),
            &[make_line(1, -0.0003), make_line(2, 0.0003)],
        );
    }
    write_segments(
        &inputs.join(square_path(right)).join("ground.arrow"),
        &[leg(lat as f32, lon as f32)],
    )
    .unwrap();
    let area = AirportArea::new(
        99,
        AERODROME_AEROWAY_TYPE,
        "Test".into(),
        "TEST".into(),
        lat,
        lon,
        Vec::new(),
        1e6,
    );
    let index = crate::airport_index::AerodromeIndex::build(std::slice::from_ref(&area));
    let plan = plan_ground_traffic(&inputs, &prepared, None, &index).unwrap();
    assert_eq!(plan.len(), if split { 2 } else { 1 });
    let input = inputs.join(square_path(right)).join("ground.arrow");
    for work in &plan {
        assert_eq!(work.inputs, vec![input.clone()]);
        assert_eq!(work.input_rows, 1);
        assert_eq!(work.input_bytes, input.metadata().unwrap().len());
        assert_eq!(work.candidates.len(), if split { 2 } else { 1 });
        assert_eq!(work.cached_lines, 2);
        assert_eq!(work.owned_lines, if split { 1 } else { 2 });
        assert_eq!(work.maximum_counter_rows, 54 * work.owned_lines);
        assert!(work.maximum_airport_key_bytes >= "TEST".len());
    }
    assert!(!prepared.join(".airport_traffic_pending").exists());
    let n = run_stage_2c(&inputs, &[area], &prepared, &crate::provider_receipt::window_of(12, 365), None).unwrap();
    assert_eq!(n, if split { 2 } else { 1 });
    let mut rows = Vec::new();
    for (owner, dir) in crate::spatial::square_directories(&prepared).unwrap() {
        let path = dir.join("airport_traffic.arrow");
        if !path.exists() {
            continue;
        }
        let summary = read_airport_summaries(&path).unwrap();
        assert_eq!(summary.len(), 1);
        assert_eq!(summary["TEST"].ops_count_per_kind[1], 1);
        for row in read_airport_traffic(&path).unwrap() {
            if split {
                assert_eq!(owner, if row.osm_id == 1 { left } else { right });
            }
            rows.push(row);
        }
    }
    rows.sort_by_key(|row| row.osm_id);
    assert_eq!(rows.len(), 2, "each line is emitted exactly once");
    for row in &rows {
        assert_eq!(row.unique_movement_count, 1);
        assert_eq!(row.microseg_unique_count, 1);
        assert!(row.band_energy_lin.iter().any(|energy| *energy > 0.0));
    }
    assert!(!prepared.join("aircraft").exists());
    rows
}

#[test]
fn neighboring_line_owners_preserve_unsplit_energy_and_rotation_unions() {
    for (lat, lon) in [(50.0, 0.0), (50.0, 180.0), (85.0, 0.0)] {
        let unsplit = partitioned_run(lat, lon, false);
        let split = partitioned_run(lat, lon, true);
        for (expected, actual) in unsplit.iter().zip(split.iter()) {
            assert_eq!(expected.osm_id, actual.osm_id);
            assert_eq!(expected.microseg_unique_count, actual.microseg_unique_count);
            for (a, b) in expected
                .band_energy_lin
                .iter()
                .zip(actual.band_energy_lin.iter())
            {
                assert!(
                    (a - b).abs() <= a.abs() * 1e-6,
                    "partition changed energy: {a} vs {b}"
                );
            }
        }
    }
}

#[test]
fn all_corrupt_ground_or_line_inputs_fail_before_prior_output_is_removed() {
    for filename in ["ground.arrow", "airport_lines.arrow", SYNTH_LINES_FILE] {
        let temp = tempfile::tempdir().unwrap();
        let inputs = temp.path().join("segments");
        let prepared = temp.path().join("2025");
        let first = crate::spatial::square_id(50.0, 0.0003).unwrap();
        let second = crate::spatial::square_id(50.0, 1.0).unwrap();
        write_segments(
            &inputs.join(square_path(first)).join("ground.arrow"),
            &[leg(50.0, 0.0)],
        )
        .unwrap();
        let prior = prepared
            .join(square_path(first))
            .join("airport_traffic.arrow");
        std::fs::create_dir_all(prior.parent().unwrap()).unwrap();
        std::fs::write(&prior, b"prior-good-output").unwrap();
        let corrupt = if filename == "ground.arrow" {
            inputs.join(square_path(second)).join(filename)
        } else {
            prepared.join(square_path(second)).join(filename)
        };
        std::fs::create_dir_all(corrupt.parent().unwrap()).unwrap();
        std::fs::write(corrupt, b"corrupt").unwrap();
        assert!(run_stage_2c(&inputs, &[], &prepared, &crate::provider_receipt::window_of(12, 365), None).is_err());
        assert_eq!(std::fs::read(&prior).unwrap(), b"prior-good-output");
    }
}

/// One flight seen by both providers on a microsegment keeps every counter
/// row (provenance is a row key) yet counts once, as a primary movement.
#[test]
fn one_flight_of_both_provenances_keeps_every_counter_row_and_counts_as_primary() {
    let temp = tempfile::tempdir().unwrap();
    let inputs = temp.path().join("input");
    let prepared = temp.path().join("prepared");
    let owner = crate::spatial::square_id(50.0, 14.0).unwrap();
    let dir = prepared.join(square_path(owner));
    let segment = leg(50.0, 14.0);
    write_real_airport_lines_arrow(
        &dir.join("airport_lines.arrow"),
        &[FakeRealLine {
            osm_id: 1,
            segment_idx: 0,
            start_lat: segment.start_lat as f64,
            start_lon: segment.start_lon as f64,
            end_lat: segment.end_lat as f64,
            end_lon: segment.end_lon as f64,
            length_m: segment.length_m,
            aeroway_type: 0,
        }],
    );
    let mut segments = Vec::new();
    for (provenance, period) in [(0, 0), (crate::flight::segment_flags::SECONDARY_ONLY, 1)] {
        for departure in [false, true] {
            let mut row = segment.clone();
            row.period = period;
            row.flags = provenance
                | if departure {
                    crate::flight::segment_flags::IS_DEPARTURE
                } else {
                    0
                };
            segments.push(row);
        }
    }
    for class in 0..NUM_GSE_CLASSES {
        let mut row = segment.clone();
        row.veh_kind = 1;
        row.gse_class = class as u8;
        row.period = 2;
        segments.push(row);
    }
    segments.extend(segments.clone());
    write_segments(
        &inputs.join(square_path(owner)).join("ground.arrow"),
        &segments,
    )
    .unwrap();
    let area = AirportArea::new(
        1,
        AERODROME_AEROWAY_TYPE,
        "A".into(),
        "A".into(),
        50.0,
        14.0,
        Vec::new(),
        0.0,
    );
    let index = crate::airport_index::AerodromeIndex::build(std::slice::from_ref(&area));
    let plan = plan_ground_traffic(&inputs, &prepared, None, &index).unwrap();
    assert_eq!(plan.len(), 1);
    assert_eq!(plan[0].maximum_counter_rows, 99);
    run_stage_2c(&inputs, &[area], &prepared, &crate::provider_receipt::window_of(12, 365), None).unwrap();
    let rows = read_airport_traffic(&dir.join("airport_traffic.arrow")).unwrap();
    assert_eq!(rows.len(), 7);
    for row in rows {
        assert_eq!(row.unique_movement_count, 1);
        assert_eq!(
            (row.microseg_unique_count, row.microseg_unique_secondary_count),
            (1, 0)
        );
        assert_eq!(
            (row.microseg_unique_arr_count, row.microseg_unique_dep_count),
            (1, 1)
        );
        assert_eq!(
            (
                row.microseg_unique_secondary_arr_count,
                row.microseg_unique_secondary_dep_count
            ),
            (0, 0)
        );
        assert_eq!(row.microseg_unique_gse_count_per_class, [1, 1, 1]);
        if row.veh_kind == 0 {
            assert_eq!(row.unique_arr_count, u32::from(row.is_departure == 0));
            assert_eq!(row.unique_dep_count, u32::from(row.is_departure == 1));
        } else {
            assert_eq!(row.unique_gse_count_per_class[row.class_idx as usize], 1);
        }
    }
    let summary = read_airport_summaries(&dir.join("airport_traffic.arrow")).unwrap()["A"];
    assert_eq!((summary.arr_count, summary.dep_count), (1, 1));
    assert_eq!(
        (summary.secondary_arr_count, summary.secondary_dep_count),
        (0, 0)
    );
    assert_eq!(summary.gse_count_per_class, [1, 1, 1]);
}

#[test]
fn oversized_ground_owner_retries_alone_without_rewriting_successes_or_retrying_corruption() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    };
    let temp = tempfile::tempdir().unwrap();
    let prepared = temp.path().join("prepared");
    let inputs = temp.path().join("inputs");
    let expected = temp.path().join("expected");
    let output = temp.path().join("output");
    let mut areas = Vec::new();
    for (lon, flights) in [(14.0, 1), (16.0, 100)] {
        let segment = leg(50.0, lon);
        let owner = crate::spatial::square_id(50.0, f64::from(lon)).unwrap();
        write_real_airport_lines_arrow(
            &prepared
                .join(square_path(owner))
                .join("airport_lines.arrow"),
            &[FakeRealLine {
                osm_id: owner.try_into().unwrap(),
                segment_idx: 0,
                start_lat: segment.start_lat.into(),
                start_lon: segment.start_lon.into(),
                end_lat: segment.end_lat.into(),
                end_lon: segment.end_lon.into(),
                length_m: segment.length_m,
                aeroway_type: 0,
            }],
        );
        let rows: Vec<_> = (1..=flights)
            .map(|flight_id| FlightSegment {
                flight_id,
                ..segment.clone()
            })
            .collect();
        write_segments(&inputs.join(square_path(owner)).join("ground.arrow"), &rows).unwrap();
        areas.push(AirportArea::new(
            owner.try_into().unwrap(),
            AERODROME_AEROWAY_TYPE,
            "Test".into(),
            owner.to_string(),
            50.0,
            lon.into(),
            Vec::new(),
            1e6,
        ));
    }
    let index = crate::airport_index::AerodromeIndex::build(&areas);
    let plan = plan_ground_traffic(&inputs, &prepared, None, &index).unwrap();
    assert_eq!(plan.len(), 2);
    let charges: Vec<_> = plan
        .iter()
        .map(|work| {
            run_ground_traffic_work(work, &prepared, &expected, &index, &crate::provider_receipt::window_of(12, 365), u64::MAX)
                .unwrap()
                .charged_bytes
        })
        .collect();
    let worker_limit = plan
        .iter()
        .map(|work| work.indexed_allocation().unwrap())
        .max()
        .unwrap()
        .max(*charges.iter().min().unwrap());
    let process_limit = 2 * worker_limit;
    assert!(charges.iter().any(|&bytes| bytes > worker_limit));
    assert!(charges.iter().all(|&bytes| bytes < process_limit));
    let calls = Mutex::new(Vec::new());
    let active = AtomicUsize::new(0);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(2)
        .build()
        .unwrap();
    let count = pool
        .install(|| {
            run_with_serial_retry(&plan, worker_limit, process_limit, |work, limit| {
                let concurrent = active.fetch_add(1, Ordering::SeqCst) + 1;
                assert!(concurrent as u64 * limit <= process_limit);
                calls.lock().unwrap().push((work.owner, limit));
                let result =
                    run_ground_traffic_work(work, &prepared, &output, &index, &crate::provider_receipt::window_of(12, 365), limit);
                active.fetch_sub(1, Ordering::SeqCst);
                result.map(|outcome| outcome.counter_rows > 0)
            })
        })
        .unwrap();
    assert_eq!(count, 2);
    for (work, charged) in plan.iter().zip(charges) {
        assert_eq!(
            calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(owner, _)| *owner == work.owner)
                .count(),
            if charged > worker_limit { 2 } else { 1 }
        );
        let relative = square_path(work.owner);
        for path in [
            format!("{relative}/airport_traffic.arrow"),
            format!("airport_summary_parts/{relative}/part.arrow"),
        ] {
            let read = |path: &Path| {
                let reader = arrow::ipc::reader::FileReader::try_new(
                    std::fs::File::open(path).unwrap(),
                    None,
                )
                .unwrap();
                (
                    reader.schema(),
                    reader.collect::<std::result::Result<Vec<_>, _>>().unwrap(),
                )
            };
            assert_eq!(read(&expected.join(&path)), read(&output.join(&path)));
        }
    }
    // A full-process refusal stops after one retry, preserving the typed cause.
    let work = &plan[..1];
    let base = work[0].indexed_allocation().unwrap();
    let attempts = AtomicUsize::new(0);
    let refused = run_with_serial_retry(work, base, base + 1, |work, limit| {
        attempts.fetch_add(1, Ordering::SeqCst);
        run_ground_traffic_work(work, &prepared, &output, &index, &crate::provider_receipt::window_of(12, 365), limit)
            .map(|outcome| outcome.counter_rows > 0)
    })
    .unwrap_err();
    assert!(refused.is::<AllocationLimitExceeded>());
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    // Corruption after the routing precheck must not trigger the memory retry.
    std::fs::write(&work[0].inputs[0], b"corrupt").unwrap();
    attempts.store(0, Ordering::SeqCst);
    let corrupt = run_with_serial_retry(work, worker_limit, process_limit, |work, limit| {
        attempts.fetch_add(1, Ordering::SeqCst);
        run_ground_traffic_work(work, &prepared, &output, &index, &crate::provider_receipt::window_of(12, 365), limit)
            .map(|outcome| outcome.counter_rows > 0)
    })
    .unwrap_err();
    assert!(!corrupt.is::<AllocationLimitExceeded>());
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}
