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

fn admitted(paths: &[PathBuf]) -> Vec<AdmittedDay> {
    paths
        .iter()
        .map(|path| AdmittedDay {
            segments: path.clone(),
            increment: false,
        })
        .collect()
}

fn seg(flight_id: u64, phase: Phase, lat: f32, lon: f32) -> FlightSegment {
    let mut segment = FlightSegment::airborne_fixture(flight_id, lat, lon);
    segment.phase = phase;
    segment
}

/// The 221 km polar chord of the long-chord tests, split as the shuffle splits it.
fn polar_chord_pieces(flight_id: u64) -> (FlightSegment, Vec<FlightSegment>) {
    let mut chord = seg(flight_id, Phase::Airborne, 82.0, 0.0);
    chord.end_lat = 80.0;
    chord.end_lon = 0.0;
    chord.length_m = crate::geo::flat_dist(82.0, 0.0, 80.0, 0.0);
    let mut pieces = Vec::new();
    split_airborne_segment(chord.clone(), &mut pieces);
    assert_eq!(pieces.len(), 56, "221 km / 4 km cap");
    (chord, pieces)
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
    shuffle_per_square(&admitted(&[day_path]), &out_dir, None).unwrap();

    // temp_shuffle must be cleaned up.
    assert!(!tmp.path().join("temp_shuffle").exists());
    // A primary-only extract has no increment days.
    assert_eq!(
        std::fs::read_to_string(out_dir.join(completion::INCREMENT_DAYS_MANIFEST)).unwrap(),
        ""
    );
    // Both phases: one shard, the owner of the segment's midpoint.
    for (name, flight_id, phase) in [
        ("airborne.arrow", 1, Phase::Airborne),
        ("ground.arrow", 2, Phase::Ground),
    ] {
        let shards = list_square_shards(&out_dir, name, None).unwrap();
        assert_eq!(shards.len(), 1);
        assert_eq!(
            shards[0].0,
            owner_square(&seg(flight_id, phase, 50.10, 14.26), None).unwrap()
        );
        let rows = read_segments(&shards[0].1).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].flight_id, flight_id);
        crate::arrow_io::require_owner_shard(
            crate::arrow_io::read_record_batches(&shards[0].1)
                .unwrap()
                .0
                .metadata(),
        )
        .unwrap();
    }
    let ground = list_square_shards(&out_dir, "ground.arrow", None).unwrap();
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
    shuffle_per_square(&admitted(&[day_path]), &out_dir, Some(&scope)).unwrap();

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
    shuffle_per_square(&admitted(&[]), &out_dir, None).unwrap();
    assert!(out_dir.exists(), "out_dir must be created");
    // No z9 shard dirs — only the sampling manifests, which record a
    // zero-day window for empty input.
    let subdirs = std::fs::read_dir(&out_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .count();
    assert_eq!(subdirs, 0, "no z9 shard dirs for empty input");
    for manifest in [
        completion::BASELINE_DAYS_MANIFEST,
        completion::INCREMENT_DAYS_MANIFEST,
    ] {
        assert_eq!(std::fs::read_to_string(out_dir.join(manifest)).unwrap(), "");
    }
    assert!(!tmp.path().join("temp_shuffle").exists());
}

/// A secondary-only row enters from an increment day only; the manifests
/// carry the admitted baseline and increment days.
#[test]
fn secondary_rows_enter_only_from_increment_days_and_manifests_bind_the_window() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("segments");
    let mut secondary = seg(2, Phase::Airborne, 50.10, 14.26);
    secondary.flags |= crate::flight::segment_flags::SECONDARY_ONLY;
    let days = ["2025-07-01", "2025-07-02"].map(|day| {
        let path = dir.join(format!("{day}.arrow"));
        write_segments(&path, &[seg(1, Phase::Airborne, 50.10, 14.26), secondary.clone()]).unwrap();
        path
    });
    let out_dir = tmp.path().join("segments_by_square");
    let admitted = vec![
        AdmittedDay {
            segments: days[0].clone(),
            increment: true,
        },
        AdmittedDay {
            segments: days[1].clone(),
            increment: false,
        },
    ];
    shuffle_per_square(&admitted, &out_dir, None).unwrap();
    assert_eq!(
        std::fs::read_to_string(out_dir.join(completion::BASELINE_DAYS_MANIFEST)).unwrap(),
        "2025-07-01\n2025-07-02"
    );
    assert_eq!(
        std::fs::read_to_string(out_dir.join(completion::INCREMENT_DAYS_MANIFEST)).unwrap(),
        "2025-07-01"
    );
    let window = completion::sampling_window(&out_dir).unwrap();
    assert_eq!((window.baseline_days, window.increment_days), (2, 1));
    let airborne = list_square_shards(&out_dir, "airborne.arrow", None).unwrap();
    let mut rows: Vec<(u64, bool)> = airborne
        .iter()
        .flat_map(|(_, path)| read_segments(path).unwrap())
        .map(|s| (s.flight_id, s.is_secondary_only()))
        .collect();
    rows.sort_unstable();
    assert_eq!(rows, [(1, false), (1, false), (2, true)]);
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
    let err = shuffle_per_square(&admitted(&[day_a, day_b]), &out_dir, None).unwrap_err();
    assert!(err.to_string().contains("duplicate day stem"), "{err}");
}

#[test]
fn shuffle_rejects_a_segment_from_another_date() {
    let tmp = tempfile::tempdir().unwrap();
    let day = tmp.path().join("air/2025-07-01.arrow");
    crate::arrow_io::write_segments(&day, &[seg(1, Phase::Airborne, 50.1, 14.2)]).unwrap();
    let error = shuffle_per_square(&admitted(&[day]), &tmp.path().join("out"), None).unwrap_err();
    assert!(format!("{error:#}").contains("segment date"), "{error:#}");
}

/// A long chord is stored as pieces, each once, in the square owning its
/// midpoint; identical original observations keep their multiplicity.
#[test]
fn long_chord_pieces_are_owned_once_by_their_midpoint_squares() {
    let (chord, pieces) = polar_chord_pieces(42);
    let mut owners: Vec<u64> = pieces
        .iter()
        .map(|piece| owner_square(piece, None).unwrap())
        .collect();
    owners.sort_unstable();
    owners.dedup();
    assert!(owners.len() > 1, "fixture must span several owner squares");
    let tmp = tempfile::tempdir().unwrap();
    let day = tmp.path().join("2025-07-01.arrow");
    write_segments(&day, &[chord.clone(), chord]).unwrap();
    let out = tmp.path().join("shuffled");
    shuffle_per_square(&admitted(&[day]), &out, None).unwrap();
    let shards = list_square_shards(&out, "airborne.arrow", None).unwrap();
    assert_eq!(
        shards.iter().map(|(square, _)| *square).collect::<Vec<_>>(),
        owners
    );
    let mut stored = 0;
    for (square, path) in shards {
        let rows = read_segments(&path).unwrap();
        assert!(rows.len() >= 2 && rows.len().is_multiple_of(2));
        for row in &rows {
            assert_eq!(owner_square(row, None), Some(square));
            assert!(
                row.length_m
                    <= noise_compute::emission::aircraft::AIRBORNE_SUB_SEGMENT_MAX_LENGTH_M
            );
        }
        stored += rows.len();
    }
    assert_eq!(stored, 2 * 56);
}

#[test]
fn streamed_parts_preserve_order_and_fields_across_flushes_and_input_batches() {
    use crate::arrow_io::{read_record_batches, write_record_batches};
    let tmp = tempfile::tempdir().unwrap();
    let day = tmp.path().join("2025-07-01.arrow");
    let (airborne, _) = polar_chord_pieces(42);
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
    let counts = scatter_day(
        &AdmittedDay {
            segments: day.clone(),
            increment: false,
        },
        &temp, None, payload_limit).unwrap();
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
                    "one-row budget must flush between the pieces of one chord"
                );
                scattered_rows += part.len() as u64;
            }
        }
    }
    assert!(
        largest_part_count >= 3,
        "later batches must add parts instead of overwriting"
    );
    assert_eq!(counts.scattered_rows, scattered_rows);
    let gathered = tmp.path().join("gathered");
    pass_b(&temp, &gathered, None, &counts).unwrap();
    let mut expected: HashMap<(&str, u64), Vec<FlightSegment>> = HashMap::new();
    let mut pieces = Vec::new();
    for row in rows {
        let phase = phase_name(row.phase).unwrap();
        split_airborne_segment(row, &mut pieces);
        for piece in pieces.drain(..) {
            let square = owner_square(&piece, None).unwrap();
            expected.entry((phase, square)).or_default().push(piece);
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
        crate::arrow_io::write_owner_shard(&reference, &rows).unwrap();
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

#[test]
fn failed_gather_reclaims_only_completed_phases_and_restarts_from_original_days() {
    let tmp = tempfile::tempdir().unwrap();
    let day = tmp.path().join("2025-07-01.arrow");
    let airborne = seg(42, Phase::Airborne, 50.1, 14.26);
    let ground = seg(99, Phase::Ground, 50.1, 14.26);
    write_segments(&day, &[airborne, ground.clone()]).unwrap();
    let original_bytes = std::fs::read(&day).unwrap();
    let temporary = tmp.path().join("temp_shuffle");
    let counts = scatter_day(
        &AdmittedDay {
            segments: day.clone(),
            increment: false,
        },
        &temporary, None, PASS_A_SPILL_BYTES).unwrap();
    let owner = owner_square(&ground, None).unwrap();
    let hash = shuffle_bucket(owner);
    let airborne_parts =
        list_pass_a_parts(&pass_a_bucket_dir(&temporary, "airborne", hash)).unwrap();
    let ground_parts = list_pass_a_parts(&pass_a_bucket_dir(&temporary, "ground", hash)).unwrap();
    assert!(!airborne_parts.is_empty() && !ground_parts.is_empty());
    let output = tmp.path().join("segments_by_square");
    let owner_dir = output.join(square_path(owner));
    // A directory at the final filename fails rename after the preceding phase succeeds.
    std::fs::create_dir_all(owner_dir.join("ground.arrow")).unwrap();
    assert!(pass_b(&temporary, &output, None, &counts).is_err());
    assert!(airborne_parts.iter().all(|part| !part.exists()));
    assert!(ground_parts.iter().all(|part| part.is_file()));
    assert_eq!(
        read_segments(&owner_dir.join("airborne.arrow")).unwrap()[0].flight_id,
        42
    );
    assert!(!output.join(completion::BASELINE_DAYS_MANIFEST).exists());
    assert_eq!(std::fs::read(&day).unwrap(), original_bytes);

    shuffle_per_square(&admitted(std::slice::from_ref(&day)), &output, None).unwrap();
    assert!(!temporary.exists());
    assert_eq!(std::fs::read(&day).unwrap(), original_bytes);
    assert_eq!(
        read_segments(&owner_dir.join("ground.arrow")).unwrap()[0].flight_id,
        99
    );
    assert_eq!(
        std::fs::read_to_string(output.join(completion::BASELINE_DAYS_MANIFEST)).unwrap(),
        "2025-07-01"
    );
}

/// Every piece is counted for its owner so gather can reserve each destination.
#[test]
fn gather_budget_counts_pieces_and_reserves_each_owner() {
    let tmp = tempfile::tempdir().unwrap();
    let day = tmp.path().join("2025-07-01.arrow");
    let (chord, pieces) = polar_chord_pieces(7);
    let mut per_owner: HashMap<u64, usize> = HashMap::new();
    for piece in &pieces {
        *per_owner
            .entry(owner_square(piece, None).unwrap())
            .or_default() += 1;
    }
    write_segments(&day, &vec![chord; 17]).unwrap();
    let counts = scatter_day(
        &AdmittedDay {
            segments: day.clone(),
            increment: false,
        },
        &tmp.path().join("parts"),
        None,
        PASS_A_SPILL_BYTES,
    )
    .unwrap();
    assert_eq!(counts.scattered_rows, 17 * pieces.len() as u64);
    for (square, owned) in per_owner {
        assert_eq!(counts.rows(Phase::Airborne, square), 17 * owned);
        assert_eq!(counts.rows(Phase::Ground, square), 0);
    }
    // The budget must cover more than the compact on-disk hash rows; it also
    // retains destination rows, their strings, and the active Arrow writer.
    assert!(counts.largest_gather_allocation() > 5 * PASS_A_SPILL_BYTES);
}

/// A whole missing phase or destination must fail, even if all surviving rows match.
#[test]
fn gather_rejects_missing_destination_parts() {
    for with_survivor in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let day = tmp.path().join("2025-07-01.arrow");
        let row = seg(1, Phase::Ground, 50.10, 14.26);
        let owner = owner_square(&row, None).unwrap();
        let hash = shuffle_bucket(owner);
        let missing = (0..=grid::MAX_SQUARE_ID as u64)
            .find(|&square| square != owner && shuffle_bucket(square) == hash)
            .unwrap();
        let temporary = tmp.path().join("parts");
        let mut counts = DestinationCounts::new();
        if with_survivor {
            write_segments(&day, &[row]).unwrap();
            counts = scatter_day(
        &AdmittedDay {
            segments: day.clone(),
            increment: false,
        },
        &temporary, None, PASS_A_SPILL_BYTES).unwrap();
        }
        // Simulate a counted destination whose complete temporary part vanished.
        counts.add(Phase::Ground, missing, 0);
        let error = pass_b(&temporary, &tmp.path().join("output"), None, &counts).unwrap_err();
        assert!(
            error.to_string().contains("destination square"),
            "{error:#}"
        );
        if with_survivor {
            assert!(
                !list_pass_a_parts(&pass_a_bucket_dir(&temporary, "ground", hash))
                    .unwrap()
                    .is_empty()
            );
        }
    }
}

#[test]
fn completed_shuffle_inventory_rejects_partial_changed_and_missing_successors() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("segments/2025-01-01.arrow");
    write_segments(&source, &[seg(1, Phase::Ground, 50.0, 14.0)]).unwrap();
    let root = temp.path().join("segments_by_square");
    shuffle_per_square(&admitted(std::slice::from_ref(&source)), &root, None).unwrap();
    completion::validate(&root, None).unwrap();
    std::fs::remove_file(source).unwrap();
    completion::validate(&root, None).unwrap();
    std::fs::create_dir(temp.path().join("temp_shuffle")).unwrap();
    assert!(completion::validate(&root, None).is_err());
    std::fs::remove_dir(temp.path().join("temp_shuffle")).unwrap();
    assert!(completion::validate(&root, Some(&ScopeBbox::parse("49,13,51,15").unwrap())).is_err());
    let days = root.join(completion::BASELINE_DAYS_MANIFEST);
    std::fs::write(&days, "2025-01-02").unwrap();
    assert!(completion::validate(&root, None).is_err());
    std::fs::write(days, "2025-01-01").unwrap();
    completion::validate(&root, None).unwrap();
    let (_, file) = list_square_shards(&root, "ground.arrow", None)
        .unwrap()
        .pop()
        .unwrap();
    let original = std::fs::read(&file).unwrap();
    std::fs::write(&file, &original).unwrap();
    assert!(
        completion::validate(&root, None).is_err(),
        "same bytes with changed identity require verification"
    );
    std::fs::remove_file(file).unwrap();
    assert!(completion::validate(&root, None).is_err());
}
