//! Finalized support-copy streaming, admission and seam regressions.

use super::super::tests::cruise;
use super::*;

#[test]
#[cfg(target_os = "linux")]
fn oversized_gather_is_rejected_before_deleting_prepared_data() {
    let directory = tempfile::tempdir().unwrap();
    let support = directory.path().join("support");
    let prepared = directory.path().join("prepared");
    let square = crate::spatial::square_id(50.1, 14.2).unwrap();
    let parts = support.join(square_path(square));
    std::fs::create_dir_all(&parts).unwrap();
    let host_bytes = std::fs::read_to_string("/proc/meminfo")
        .unwrap()
        .lines()
        .find(|line| line.starts_with("MemTotal:"))
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse::<u64>()
        .unwrap()
        * 1024;
    // Sparse metadata provides an oversized destination without allocating its body.
    std::fs::File::create(parts.join("part.arrow"))
        .unwrap()
        .set_len(host_bytes)
        .unwrap();
    let existing = prepared.join(square_path(square)).join("cruise.arrow");
    std::fs::create_dir_all(existing.parent().unwrap()).unwrap();
    std::fs::write(&existing, b"existing prepared generation").unwrap();
    let error = gather_finalized_cruise(&support, &prepared, None).unwrap_err();
    assert!(
        error.to_string().contains("allocation allowance"),
        "{error}"
    );
    assert_eq!(
        std::fs::read(existing).unwrap(),
        b"existing prepared generation"
    );
}

#[test]
fn native_fold_copies_final_values_once_across_long_polar_and_seam_destinations() {
    for (start, end, receiver, scoped) in [
        ([49.0, 14.25], [51.0, 14.25], [50.0, 15.83], false),
        ([49.0, 14.25], [51.0, 14.25], [50.0, 15.83], true),
        (
            [80.178_71, 0.0],
            [80.18, 0.002],
            [80.05804856215623, 0.0],
            false,
        ),
        ([0.0, 179.99], [0.0, -179.99], [0.001, -180.0], false),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let mut first = cruise(42, start[0], start[1], end[0], end[1]);
        first.callsign = "COPY42".into();
        first.aircraft_type = *b"B738";
        first.source_id = 2;
        let mut second = first.clone();
        second.flight_id = 43;
        second.callsign = "COPY43".into();
        let segments = [first.clone(), first, second];
        let day = directory.path().join("segments.arrow");
        crate::arrow_io::write_segments(&day, &segments).unwrap();
        let scope = scoped.then(|| ScopeBbox::parse("50,15.83,50,15.83").unwrap());
        let prepared = directory.path().join("prepared");
        let written = run_stage_2b(&[day], &prepared, 12, scope.as_ref(), false).unwrap();
        let mut canonical = HashMap::new();
        for segment in &segments {
            process_segment(segment, &mut canonical, NpdLuts::shared());
        }
        if scoped {
            assert!(
                canonical
                    .keys()
                    .all(|&owner| !scope.unwrap().contains_square(owner)),
                "scope must exclude every canonical owner while keeping reached destinations"
            );
        }
        let mut expected: HashMap<u64, Vec<CruiseBucket>> = HashMap::new();
        let mut canonical_length = 0.0f64;
        for map in canonical.into_values() {
            for (key, accum) in map {
                let bucket = accum.finalize(key);
                assert_eq!(bucket.unique_count, 2);
                assert_eq!(bucket.top_candidates.len(), 2);
                assert_eq!(bucket.top_candidates[0].callsign, "COPY42");
                canonical_length += f64::from(bucket.sum_length_m);
                let (lon, lat) = grid::cruise::cruise_centroid(bucket.cruise_cell_id);
                for square in noise_compute::emission::aircraft::cruise_support_cells(
                    lat,
                    lon,
                    bucket.rep_len_m,
                )
                .unwrap()
                .iter()
                {
                    let id = grid::square_id(square) as u64;
                    if scope.as_ref().is_none_or(|scope| scope.contains_square(id)) {
                        expected.entry(id).or_default().push(bucket.clone());
                    }
                }
            }
        }
        let original_length = segments.iter().map(|s| f64::from(s.length_m)).sum::<f64>();
        assert!((canonical_length - original_length).abs() <= original_length * 1e-6);
        assert!(expected
            .contains_key(&(grid::square_id(grid::square_of(receiver[0], receiver[1])) as u64)));
        assert_eq!(written, expected.len());
        assert_eq!(
            crate::spatial::square_directories(&prepared).unwrap().len(),
            expected.len()
        );
        for (square, mut rows) in expected {
            rows.sort_unstable_by_key(|r| (r.cruise_cell_id, r.class, r.fl_bin, r.period));
            let reference = directory.path().join("reference.arrow");
            write_cruise(&reference, &rows, 12).unwrap();
            assert_eq!(
                read_record_batches(&prepared.join(square_path(square)).join("cruise.arrow"))
                    .unwrap(),
                read_record_batches(&reference).unwrap(),
                "all final columns and stamps at {}",
                square_path(square)
            );
        }
    }
}

#[test]
fn gather_streams_batch_boundaries_and_retires_only_committed_parts() {
    let directory = tempfile::tempdir().unwrap();
    let support = directory.path().join("support");
    let prepared = directory.path().join("prepared");
    let square = crate::spatial::square_id(50.1, 14.2).unwrap();
    let input = support.join(square_path(square)).join("part.arrow");
    let mut canonical = HashMap::new();
    process_segment(
        &cruise(42, 50.1, 14.2, 50.1, 14.20001),
        &mut canonical,
        NpdLuts::shared(),
    );
    let (key, accum) = canonical
        .into_values()
        .next()
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let original = accum.finalize(key);
    let rows: Vec<_> = (0..(2 * arrow_batching::TARGET_ROWS_PER_BATCH + 1))
        .map(|index| {
            let mut row = original.clone();
            row.cruise_cell_id += index as u64;
            row
        })
        .collect();
    write_cruise(&input, &rows, 12).unwrap();
    let (_, expected) = read_record_batches(&input).unwrap();
    let (destinations, written) = gather_finalized_cruise(&support, &prepared, None).unwrap();
    assert_eq!((destinations, written), (1, rows.len() as u64));
    assert!(!input.exists());
    let (schema, actual) =
        read_record_batches(&prepared.join(square_path(square)).join("cruise.arrow")).unwrap();
    assert_eq!(
        actual
            .iter()
            .map(|batch| batch.num_rows())
            .collect::<Vec<_>>(),
        vec![4096, 4096, 1]
    );
    assert_eq!(
        arrow::compute::concat_batches(&std::sync::Arc::new(schema), &actual).unwrap(),
        expected[0]
    );

    write_cruise(&input, &rows[..1], 12).unwrap();
    let blocked = directory.path().join("blocked");
    std::fs::write(&blocked, b"not a directory").unwrap();
    assert!(gather_finalized_cruise(&support, &blocked, None).is_err());
    assert!(
        input.exists(),
        "failed destination must retain reconstruction parts"
    );
}
