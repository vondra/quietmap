//! Regression tests for shuffle behavior.

use super::*;
use crate::flight::Phase;

fn write_segments(path: &Path, rows: &[FlightSegment]) -> Result<()> {
    let day = path.file_stem().unwrap().to_str().unwrap();
    let mut rows = rows.to_vec();
    for row in &mut rows {
        row.date_id = crate::period::parse_date_id(day)?;
    }
    crate::arrow_io::write_segments(path, &rows)
}

fn seg(flight_id: u64, phase: Phase, lat: f32, lon: f32) -> FlightSegment {
    FlightSegment {
        callsign: format!("FL{flight_id:04}"),
        aircraft_type: [b'A', b'3', b'2', b'0'],
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

#[test]
fn round_trip_airborne_and_ground() {
    let tmp = tempfile::tempdir().unwrap();
    let segments_dir = tmp.path().join("segments");
    std::fs::create_dir_all(&segments_dir).unwrap();
    // One day with mixed phases at one location; cruise is dropped
    // by shuffle.
    let day_path = segments_dir.join("2025-01-21.arrow");
    write_segments(
        &day_path,
        &[
            seg(1, Phase::Airborne, 50.10, 14.26),
            seg(2, Phase::Ground, 50.10, 14.26),
            seg(3, Phase::Cruise, 50.10, 14.26),
        ],
    )
    .unwrap();

    let out_dir = tmp.path().join("segments_by_square");
    shuffle_per_square(&[day_path], &[], &out_dir, None).unwrap();

    // temp_shuffle must be cleaned up.
    assert!(!tmp.path().join("temp_shuffle").exists());
    // Single-window extract: no ga_n_days manifest (read_ga_n_days → 0).
    assert_eq!(
        std::fs::read_to_string(out_dir.join("ga_days")).unwrap(),
        ""
    );
    let airborne = list_square_shards(&out_dir, "airborne.arrow", None).unwrap();
    let expected =
        crate::support::airborne_segment_support(&seg(1, Phase::Airborne, 50.10, 14.26)).unwrap();
    assert_eq!(airborne.len(), expected.cell_count());
    for (square, path) in airborne {
        assert!(expected.contains(grid::square_from_id(square as i64).unwrap()));
        let rows = read_segments(&path).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].flight_id, 1);
    }
    let ground = list_square_shards(&out_dir, "ground.arrow", None).unwrap();
    assert_eq!(ground.len(), 1);
    assert_eq!(
        ground[0].0,
        square_of_midpoint(&seg(2, Phase::Ground, 50.10, 14.26)).unwrap()
    );
    let rows = read_segments(&ground[0].1).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].flight_id, 2);
}

#[test]
fn scope_filters_out_of_scope_squares() {
    let tmp = tempfile::tempdir().unwrap();
    let segments_dir = tmp.path().join("segments");
    std::fs::create_dir_all(&segments_dir).unwrap();
    let day_path = segments_dir.join("2025-01-21.arrow");
    write_segments(
        &day_path,
        &[
            seg(1, Phase::Airborne, 50.10, 14.26), // CZ
            seg(2, Phase::Airborne, 35.0, 139.0),  // Tokyo — out of scope
        ],
    )
    .unwrap();

    let out_dir = tmp.path().join("segments_by_square");
    let scope = ScopeBbox::parse("48.65,12.00,51.55,16.90").unwrap();
    shuffle_per_square(&[day_path], &[], &out_dir, Some(&scope)).unwrap();

    let airborne = list_square_shards(&out_dir, "airborne.arrow", None).unwrap();
    assert!(!airborne.is_empty());
    for (square, path) in airborne {
        assert!(scope.contains_square(square));
        let rows = read_segments(&path).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].flight_id, 1);
    }
}

#[test]
fn empty_input_writes_no_shards() {
    let tmp = tempfile::tempdir().unwrap();
    let out_dir = tmp.path().join("segments_by_square");
    shuffle_per_square(&[], &[], &out_dir, None).unwrap();
    assert!(out_dir.exists(), "out_dir must be created");
    // No z9 shard dirs — only the n_days manifest, which records a
    // zero-day window for empty input.
    let subdirs = std::fs::read_dir(&out_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .count();
    assert_eq!(subdirs, 0, "no z9 shard dirs for empty input");
    assert_eq!(std::fs::read_to_string(out_dir.join("days")).unwrap(), "");
    assert_eq!(
        std::fs::read_to_string(out_dir.join("ga_days")).unwrap(),
        ""
    );
    assert!(!tmp.path().join("temp_shuffle").exists());
}

/// Hybrid merge with COLLIDING day stems — `2025-07-01` exists in
/// both passes (first-of-month overlap). Both segments must reach
/// the z9 shard: the former undiscriminated Pass-A
/// temp path raced between passes and one silently vanished. Day
/// counts come from the two input lists, never a combined `len()`.
#[test]
fn hybrid_colliding_day_stems_merge_and_write_dual_manifests() {
    let tmp = tempfile::tempdir().unwrap();
    let air_dir = tmp.path().join("segments");
    let ga_dir = tmp.path().join("ga_segments");
    std::fs::create_dir_all(&air_dir).unwrap();
    std::fs::create_dir_all(&ga_dir).unwrap();
    let air_day = air_dir.join("2025-07-01.arrow");
    let ga_day = ga_dir.join("2025-07-01.arrow");
    // Same location → same (phase, hash, day-stem) Pass-A bucket.
    write_segments(&air_day, &[seg(1, Phase::Airborne, 50.10, 14.26)]).unwrap();
    let mut ga = seg(2, Phase::Airborne, 50.10, 14.26);
    ga.profile_idx = crate::profile::profile_idx("C172");
    write_segments(&ga_day, &[ga]).unwrap();

    let out_dir = tmp.path().join("segments_by_square");
    shuffle_per_square(&[air_day], &[ga_day], &out_dir, None).unwrap();

    assert_eq!(
        std::fs::read_to_string(out_dir.join("days")).unwrap(),
        "2025-07-01"
    );
    assert_eq!(
        std::fs::read_to_string(out_dir.join("ga_days")).unwrap(),
        "2025-07-01"
    );
    let airborne = list_square_shards(&out_dir, "airborne.arrow", None).unwrap();
    assert!(!airborne.is_empty());
    for (_, path) in airborne {
        let mut fids: Vec<u64> = read_segments(&path)
            .unwrap()
            .iter()
            .map(|s| s.flight_id)
            .collect();
        fids.sort_unstable();
        assert_eq!(
            fids,
            [1, 2],
            "both sampling passes must survive in every support cell"
        );
    }
}

/// Duplicate day stems WITHIN one pass list would collide on one
/// Pass-A temp path — refuse loudly instead of dropping segments.
#[test]
fn duplicate_day_stem_within_one_pass_bails() {
    let tmp = tempfile::tempdir().unwrap();
    let dir_a = tmp.path().join("a");
    let dir_b = tmp.path().join("b");
    std::fs::create_dir_all(&dir_a).unwrap();
    std::fs::create_dir_all(&dir_b).unwrap();
    let day_a = dir_a.join("2025-07-01.arrow");
    let day_b = dir_b.join("2025-07-01.arrow");
    write_segments(&day_a, &[seg(1, Phase::Airborne, 50.10, 14.26)]).unwrap();
    write_segments(&day_b, &[seg(2, Phase::Airborne, 50.10, 14.26)]).unwrap();
    let out_dir = tmp.path().join("segments_by_square");
    let err = shuffle_per_square(&[day_a, day_b], &[], &out_dir, None).unwrap_err();
    assert!(err.to_string().contains("duplicate day stem"), "{err}");
}

#[test]
fn hybrid_shuffle_rejects_class_or_date_window_leakage() {
    let tmp = tempfile::tempdir().unwrap();
    let air = tmp.path().join("air/2025-07-01.arrow");
    let ga = tmp.path().join("ga/2025-07-01.arrow");
    write_segments(&air, &[seg(1, Phase::Airborne, 50.1, 14.2)]).unwrap();
    write_segments(&ga, &[seg(2, Phase::Airborne, 50.1, 14.2)]).unwrap();
    let error = shuffle_per_square(
        std::slice::from_ref(&air),
        &[ga],
        &tmp.path().join("out"),
        None,
    )
    .unwrap_err();
    assert!(
        format!("{error:#}").contains("other sampling window"),
        "{error:#}"
    );
    crate::arrow_io::write_segments(&air, &[seg(1, Phase::Airborne, 50.1, 14.2)]).unwrap();
    let error = shuffle_per_square(&[air], &[], &tmp.path().join("out"), None).unwrap_err();
    assert!(format!("{error:#}").contains("segment date"), "{error:#}");
}

#[test]
fn airborne_destination_hash_collisions_do_not_multiply_original_rows() {
    let mut original = seg(42, Phase::Airborne, 82.0, 0.0);
    original.end_lat = 80.0;
    original.end_lon = 0.0;
    let destinations: Vec<_> = destination_squares(&original, None).unwrap().collect();
    let hashes: std::collections::HashSet<_> =
        destinations.iter().map(|&id| shuffle_bucket(id)).collect();
    assert!(
        hashes.len() < destinations.len(),
        "fixture must exercise shared destination hashes"
    );
    let tmp = tempfile::tempdir().unwrap();
    let day = tmp.path().join("2025-07-01.arrow");
    write_segments(&day, &[original.clone(), original]).unwrap();
    let out = tmp.path().join("shuffled");
    shuffle_per_square(&[day], &[], &out, None).unwrap();
    let shards = list_square_shards(&out, "airborne.arrow", None).unwrap();
    assert_eq!(shards.len(), destinations.len());
    for (_, path) in shards {
        assert_eq!(read_segments(&path).unwrap().len(), 2);
    }
}

#[test]
fn streamed_parts_preserve_order_and_fields_across_flushes_and_input_batches() {
    use crate::arrow_io::{read_record_batches, write_record_batches};
    let tmp = tempfile::tempdir().unwrap();
    let day = tmp.path().join("2025-07-01.arrow");
    let mut airborne = seg(42, Phase::Airborne, 82.0, 0.0);
    airborne.end_lat = 80.0;
    airborne.end_lon = 0.0;
    let ground = seg(99, Phase::Ground, 50.1, 14.26);
    let mut later = airborne.clone();
    later.flight_id = 43;
    write_segments(&day, &[airborne.clone(), ground, airborne, later]).unwrap();
    let (schema, original) = read_record_batches(&day).unwrap();
    write_record_batches(
        &day,
        &schema,
        &[original[0].slice(0, 2), original[0].slice(2, 2)],
    )
    .unwrap();
    let rows = read_segments(&day).unwrap();
    let temp = tmp.path().join("parts");
    let payload_limit = std::mem::size_of::<FlightSegment>() + rows[0].callsign.len();
    let copies = scatter_day(&day, "air", false, &temp, None, payload_limit).unwrap();
    let mut scattered_rows = 0;
    let mut largest_part_count = 0;
    for phase in ["airborne", "ground"] {
        for hash in 0..SHUFFLE_HASH_BUCKETS {
            let parts = list_pass_a_parts(&pass_a_bucket_dir(&temp, phase, hash)).unwrap();
            largest_part_count = largest_part_count.max(parts.len());
            for path in parts {
                let part = read_segments(&path).unwrap();
                assert_eq!(
                    part.len(),
                    1,
                    "one-row budget must flush inside support expansion"
                );
                scattered_rows += part.len() as u64;
            }
        }
    }
    assert!(
        largest_part_count >= 3,
        "later batches must add parts instead of overwriting"
    );
    assert_eq!(copies, scattered_rows);
    let gathered = tmp.path().join("gathered");
    pass_b(&temp, &gathered, None).unwrap();
    let mut expected: HashMap<(&str, u64), Vec<FlightSegment>> = HashMap::new();
    for row in rows {
        for square in destination_squares(&row, None).unwrap() {
            expected
                .entry((phase_name(row.phase).unwrap(), square))
                .or_default()
                .push(row.clone());
        }
    }
    let mut actual_count = 0;
    for phase in ["airborne", "ground"] {
        actual_count += list_square_shards(&gathered, &format!("{phase}.arrow"), None)
            .unwrap()
            .len();
    }
    assert_eq!(actual_count, expected.len());
    for ((phase, square), rows) in expected {
        let reference = tmp.path().join("reference.arrow");
        crate::arrow_io::write_segments(&reference, &rows).unwrap();
        assert_eq!(
            read_record_batches(
                &gathered
                    .join(square_path(square))
                    .join(format!("{phase}.arrow"))
            )
            .unwrap(),
            read_record_batches(&reference).unwrap(),
            "all original fields, order, and repetitions in {phase}/{}",
            square_path(square)
        );
    }
}
