//! Cruise aggregation and spill regressions.
use super::*;
use crate::flight::CruiseBucket;
use crate::geo::flat_dist;

fn admitted(day_paths: &[PathBuf]) -> Vec<AdmittedDay> {
    day_paths
        .iter()
        .map(|path| AdmittedDay {
            segments: path.clone(),
            increment: false,
        })
        .collect()
}

/// Spill each shard to its own day dir under `root`, then list every part
/// with merged counts — the fold's `(parts, counts)` input pair.
fn spill_shards_to_parts(
    root: &Path,
    shards: [&mut HashMap<u64, HashMap<CruiseKey, CruiseAccum>>; 2],
) -> (Vec<PathBuf>, crate::arrow_io::CruiseSpillCounts) {
    for (day, shard) in shards.into_iter().enumerate() {
        let mut buffers = SpillBuffers::new(day);
        buffers.buffer_flush(shard);
        buffers
            .write_all_buffers(&root.join(format!("spill{day}")), &AtomicU64::new(0))
            .unwrap();
    }
    let parts: Vec<PathBuf> = (0..SPILL_HASH_BUCKETS)
        .flat_map(|bucket| {
            (0..2)
                .flat_map(|day| {
                    list_spill_parts(&spill_bucket_dir(&root.join(format!("spill{day}")), bucket))
                        .unwrap()
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let mut counts = crate::arrow_io::CruiseSpillCounts::default();
    for part in &parts {
        counts.merge(crate::arrow_io::CruiseSpillCounts::read(part).unwrap());
    }
    (parts, counts)
}

fn run_stage_2b(
    day_paths: &[PathBuf],
    prepared_year_dir: &Path,
    n_days: u16,
    scope: Option<&ScopeBbox>,
) -> Result<usize> {
    run_stage_2b_phase(
        &admitted(day_paths),
        prepared_year_dir,
        &prepared_year_dir.parent().unwrap().join("spill_cruise"),
        &crate::provider_receipt::window_of(n_days, 0),
        scope,
        CruisePhase::All,
    )
}

pub(super) fn cruise(flight_id: u64, lat0: f32, lon0: f32, lat1: f32, lon1: f32) -> FlightSegment {
    FlightSegment {
        callsign: String::new(),
        aircraft_type: [0u8; 4],
        flight_id,
        profile_idx: 0,
        source_id: 0,
        origin: 0,
        veh_kind: 0,
        gse_class: 0,
        period: 0,
        date_id: 0,
        phase: Phase::Cruise,
        flags: 0,
        start_lat: lat0,
        start_lon: lon0,
        start_alt_m: 11_000.0,
        end_lat: lat1,
        end_lon: lon1,
        end_alt_m: 11_000.0,
        speed_kt: 460.0,
        length_m: flat_dist(lat0, lon0, lat1, lon1),
        agl_avg_m: 11_000.0,
        start_elev_m: 0.0,
        end_elev_m: 0.0,
        departure_field_elev_m: f32::NAN,
    }
}

#[test]
fn flight_id_dedup_per_cruise_bucket() {
    // Same flight crossing three densified points in the same z15
    // cell — flight_id should appear once per bucket.
    let seg = cruise(42, 50.10, 14.20, 50.10, 14.205);
    let mut by_square = HashMap::new();
    let luts = NpdLuts::shared();
    process_segment(&seg, &mut by_square, luts);
    for buckets in by_square.values() {
        for accum in buckets.values() {
            assert_eq!(accum.fid_set.len(), 1);
            assert_eq!(accum.top.len(), 1);
        }
    }
}

/// Bucket with >K=50 unique fids must report `unique_count == 60`
/// (the full count) AND `top_candidates.len() == 50` (capped).
#[test]
fn top_k_caps_at_50_while_unique_count_tracks_full() {
    let mut by_square = HashMap::new();
    let luts = NpdLuts::shared();
    for i in 0..60u64 {
        // Same z15 cell, all distinct flight_ids → 60 fids in one bucket.
        let seg = cruise(i + 1, 50.10, 14.20, 50.10, 14.205);
        process_segment(&seg, &mut by_square, luts);
    }
    // Take the largest bucket (the one with all 60 fids landing in
    // the same z15 cell).
    let max_unique = by_square
        .values()
        .flat_map(|m| m.values())
        .map(|a| a.fid_set.len())
        .max()
        .unwrap_or(0);
    assert_eq!(max_unique, 60, "unique_count must track full fid set");
    let max_top = by_square
        .values()
        .flat_map(|m| m.values())
        .map(|a| a.top.len())
        .max()
        .unwrap_or(0);
    assert_eq!(max_top, CRUISE_TOP_K, "top_candidates must cap at K=50");
}

#[test]
fn merge_matches_sequential() {
    // Sequential scatter of 4 segments must produce the same
    // bucket-level state as the same 4 segments split into two
    // shards then merged. f32 sums tolerate ~1e-3 relative drift
    // from re-association.
    let segs: Vec<FlightSegment> = (0..4)
        .map(|i| {
            cruise(
                (i as u64) + 1,
                50.10,
                14.20 + 0.001 * i as f32,
                50.10,
                14.21 + 0.001 * i as f32,
            )
        })
        .collect();

    let luts = NpdLuts::shared();
    let mut seq: HashMap<u64, HashMap<CruiseKey, CruiseAccum>> = HashMap::new();
    for s in &segs {
        process_segment(s, &mut seq, luts);
    }

    let mut shard_a: HashMap<u64, HashMap<CruiseKey, CruiseAccum>> = HashMap::new();
    let mut shard_b: HashMap<u64, HashMap<CruiseKey, CruiseAccum>> = HashMap::new();
    process_segment(&segs[0], &mut shard_a, luts);
    process_segment(&segs[1], &mut shard_a, luts);
    process_segment(&segs[2], &mut shard_b, luts);
    process_segment(&segs[3], &mut shard_b, luts);
    // Both shards round-trip through spill files; the fold merges them.
    let directory = tempfile::tempdir().unwrap();
    let (parts, counts) = spill_shards_to_parts(directory.path(), [&mut shard_a, &mut shard_b]);
    let mut folded: Vec<(u64, crate::flight::CruiseBucket)> = Vec::new();
    fold_bucket_sorted(&parts, None, counts, |square, mut rows| {
        for row in rows.drain(..) {
            folded.push((square, row));
        }
        Ok(())
    })
    .unwrap();

    let mut seq_rows: Vec<(u64, crate::flight::CruiseBucket)> = seq
        .into_iter()
        .flat_map(|(square, inner)| {
            inner
                .into_iter()
                .map(move |(key, accum)| (square, accum.finalize(key)))
        })
        .collect();
    seq_rows.sort_by_key(|(square, row)| {
        (
            *square,
            row.cruise_cell_id,
            row.class,
            row.fl_bin,
            row.period,
            row.heading_bin,
        )
    });
    folded.sort_by_key(|(square, row)| {
        (
            *square,
            row.cruise_cell_id,
            row.class,
            row.fl_bin,
            row.period,
            row.heading_bin,
        )
    });
    assert_eq!(seq_rows.len(), folded.len());
    for ((seq_square, sa), (par_square, pa)) in seq_rows.iter().zip(folded.iter()) {
        assert_eq!(seq_square, par_square);
        let close = |a: f32, b: f32| {
            let denom = a.abs().max(b.abs()).max(1.0);
            (a - b).abs() <= denom * 1e-3
        };
        assert!(close(sa.sum_length_m, pa.sum_length_m), "sum_length_m");
        assert!(close(sa.rep_alt_m, pa.rep_alt_m), "rep_alt_m");
        assert!(close(sa.rep_speed_kt, pa.rep_speed_kt), "rep_speed_kt");
        assert_eq!(sa.unique_count, pa.unique_count, "fid_set size");
        assert_eq!(
            sa.top_candidates.len(),
            pa.top_candidates.len(),
            "top size"
        );
    }
}

/// Finalized cruise rows land in their owner z9 and leave no spill scratch.
#[test]
fn run_stage_2b_spill_and_merge_one_square() {
    use crate::arrow_io::write_segments;
    let tmp = tempfile::tempdir().unwrap();
    let segments_dir = tmp.path().join("segments");
    let prepared_year = tmp.path().join("prepared_year");
    std::fs::create_dir_all(&segments_dir).unwrap();
    std::fs::create_dir_all(&prepared_year).unwrap();
    // Eight cruise segments at LKPR cruise altitude, tightly packed
    // so they all bucket into the same z9 (and likely same z15).
    let segs: Vec<FlightSegment> = (0..8)
        .map(|i| {
            cruise(
                (i as u64) + 100,
                50.10,
                14.20 + 0.0005 * i as f32,
                50.10,
                14.21 + 0.0005 * i as f32,
            )
        })
        .collect();
    let day_path = segments_dir.join("2025-01-21.arrow");
    write_segments(&day_path, &segs).unwrap();
    let n = run_stage_2b(&[day_path], &prepared_year, 1, None).unwrap();
    assert!(n >= 1, "expected at least one z9 written, got {n}");
    // Spill dir must be cleaned up after merge.
    assert!(
        !tmp.path().join("spill_cruise").exists(),
        "spill_cruise scratch dir must be removed after merge"
    );
    // At least one z9 subdir under prepared_year with a cruise.arrow file.
    let square_dirs = crate::spatial::square_directories(&prepared_year).unwrap();
    let found = square_dirs
        .iter()
        .any(|(_, path)| path.join("cruise.arrow").exists());
    assert!(found, "cruise.arrow must exist in at least one z9 dir");
}

/// Empty input: no spill files, no z9 writes, but the spill scratch
/// dir is created + cleaned anyway. Guards against a worker-loop
/// short-circuit that would leave stale dirs behind.
#[test]
fn run_stage_2b_empty_segments_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let prepared_year = tmp.path().join("prepared_year");
    std::fs::create_dir_all(&prepared_year).unwrap();
    let n = run_stage_2b(&[], &prepared_year, 1, None).unwrap();
    assert_eq!(n, 0);
    assert!(!tmp.path().join("spill_cruise").exists());
}

/// Secondary-only cruise transits form their own buckets (they carry the
/// increment weight) and count only on increment days.
#[test]
fn secondary_cruise_transits_bucket_apart_and_only_on_increment_days() {
    use crate::arrow_io::write_segments;
    let mut secondary = cruise(7, 50.10, 14.20, 50.10, 14.21);
    secondary.flags |= crate::flight::segment_flags::SECONDARY_ONLY;
    let tmp = tempfile::tempdir().unwrap();
    let day_path = tmp.path().join("segments/2025-07-01.arrow");
    write_segments(&day_path, &[cruise(6, 50.10, 14.20, 50.10, 14.21), secondary]).unwrap();
    let buckets = |increment: bool, name: &str| -> Vec<bool> {
        let prepared = tmp.path().join(name).join("prepared");
        run_stage_2b_phase(
            &[AdmittedDay {
                segments: day_path.clone(),
                increment,
            }],
            &prepared,
            &tmp.path().join(name).join("spill_cruise"),
            &crate::provider_receipt::window_of(1, 1),
            None,
            CruisePhase::All,
        )
        .unwrap();
        let mut flags = Vec::new();
        for (_, dir) in crate::spatial::square_directories(&prepared).unwrap() {
            let (_, batches) = crate::arrow_io::read_record_batches(&dir.join("cruise.arrow")).unwrap();
            for batch in batches {
                let column = batch
                    .column_by_name("secondary_only")
                    .unwrap()
                    .as_any()
                    .downcast_ref::<arrow::array::UInt8Array>()
                    .unwrap()
                    .clone();
                flags.extend(column.values().iter().map(|v| *v != 0));
            }
        }
        flags.sort_unstable();
        flags
    };
    let increment = buckets(true, "increment");
    assert!(increment.contains(&true) && increment.contains(&false));
    assert!(buckets(false, "baseline").iter().all(|secondary| !secondary));
}

#[test]
fn merge_dedup_same_flight_id_across_shards() {
    // Same flight_id in two shards must collapse to one entry
    // in `unique_count` (and `top_candidates`) after the fold.
    let s1 = cruise(99, 50.10, 14.20, 50.10, 14.205);
    let s2 = cruise(99, 50.10, 14.205, 50.10, 14.21);
    let mut shard_a: HashMap<u64, HashMap<CruiseKey, CruiseAccum>> = HashMap::new();
    let mut shard_b: HashMap<u64, HashMap<CruiseKey, CruiseAccum>> = HashMap::new();
    let luts = NpdLuts::shared();
    process_segment(&s1, &mut shard_a, luts);
    process_segment(&s2, &mut shard_b, luts);
    let directory = tempfile::tempdir().unwrap();
    let (parts, counts) = spill_shards_to_parts(directory.path(), [&mut shard_a, &mut shard_b]);
    fold_bucket_sorted(&parts, None, counts, |_, rows| {
        for row in &rows {
            assert_eq!(row.unique_count, 1, "fid 99 must dedupe across shards");
            assert!(
                row.top_candidates.len() <= 1,
                "top entry for fid 99 also dedupes"
            );
        }
        Ok(())
    })
    .unwrap();
}

/// Regression for the wipe-on-scope bug applied to cruise: a stale
/// `cruise.arrow` in an in-scope z9 must be wiped before
/// `run_stage_2b` returns, even if no cruise segments hit that z9
/// this run.
#[test]
fn run_stage_2b_wipes_in_scope_stale_cruise() {
    use crate::geo::square_path;
    use crate::scope::ScopeBbox;
    let tmp = tempfile::tempdir().unwrap();
    let prepared_year = tmp.path().join("prepared_year");
    // Praha z9 — in-scope.
    let square = crate::spatial::square_id(50.10, 14.26).unwrap();
    let square_dir = prepared_year.join(square_path(square));
    std::fs::create_dir_all(&square_dir).unwrap();
    let stale = square_dir.join("cruise.arrow");
    std::fs::write(&stale, b"stale-prev-run").unwrap();
    let scope = ScopeBbox::parse("48.65,12.00,51.55,16.90").unwrap();
    let n = run_stage_2b(&[], &prepared_year, 1, Some(&scope)).unwrap();
    assert_eq!(n, 0, "no day shards → no z9 written");
    assert!(
        !stale.exists(),
        "stale cruise.arrow must be wiped from in-scope z9"
    );
}

/// Out-of-scope counterexample for the wipe: a stale `cruise.arrow`
/// in an z9 OUTSIDE the scope bbox must survive.
#[test]
fn run_stage_2b_leaves_out_of_scope_stale_cruise() {
    use crate::geo::square_path;
    use crate::scope::ScopeBbox;
    let tmp = tempfile::tempdir().unwrap();
    let prepared_year = tmp.path().join("prepared_year");
    // Gran Canaria z9 — outside Praha scope.
    let square = crate::spatial::square_id(27.93, -15.39).unwrap();
    let square_dir = prepared_year.join(square_path(square));
    std::fs::create_dir_all(&square_dir).unwrap();
    let stale = square_dir.join("cruise.arrow");
    std::fs::write(&stale, b"stale-prev-run").unwrap();
    let praha = ScopeBbox::parse("48.65,12.00,51.55,16.90").unwrap();
    let _ = run_stage_2b(&[], &prepared_year, 1, Some(&praha)).unwrap();
    assert!(
        stale.exists(),
        "out-of-scope z9 cruise.arrow must survive a scoped reextract"
    );
}

#[test]
fn equal_rank_top_candidates_are_order_independent() {
    let make = |ids: Vec<u64>| {
        let mut accum = CruiseAccum::default();
        for id in ids {
            accum.merge_top_entry(CruiseTopCandidate {
                flight_id: id,
                callsign: format!("F{id}"),
                aircraft_type: *b"A320",
                peak_lmax_25m_db: 95.0,
                altitude_m: 10000.0,
            });
        }
        let mut ids: Vec<_> = accum.top.keys().copied().collect();
        ids.sort_unstable();
        ids
    };
    assert_eq!(make((0..60).collect()), (0..50).collect::<Vec<_>>());
    assert_eq!(make((0..60).rev().collect()), (0..50).collect::<Vec<_>>());
}

/// Each finalized bucket is published once, in its owner z9, sorted by key,
/// across long, polar and seam segments; a scope keeps
/// owners inside its buffered bbox.
#[test]
fn fold_publishes_each_canonical_row_once_in_its_owner_square() {
    use crate::arrow_io::read_record_batches;
    for (start, end, scoped) in [
        ([49.0, 14.25], [51.0, 14.25], false),
        ([49.0, 14.25], [51.0, 14.25], true),
        ([80.178_71, 0.0], [80.18, 0.002], false),
        ([0.0, 179.99], [0.0, -179.99], false),
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
        let scope = scoped.then(|| ScopeBbox::parse("50.3,14.0,50.5,14.5").unwrap());
        let prepared = directory.path().join("prepared");
        let written = run_stage_2b(&[day], &prepared, 12, scope.as_ref()).unwrap();
        let mut canonical = HashMap::new();
        for segment in &segments {
            process_segment(segment, &mut canonical, NpdLuts::shared());
        }
        let mut expected: HashMap<u64, Vec<CruiseBucket>> = HashMap::new();
        let mut canonical_length = 0.0f64;
        for (owner, map) in canonical {
            if scope.as_ref().is_some_and(|scope| !scope.contains_square(owner)) {
                continue;
            }
            for (key, accum) in map {
                let bucket = accum.finalize(key);
                assert_eq!(bucket.unique_count, 2);
                assert_eq!(bucket.top_candidates.len(), 2);
                assert_eq!(bucket.top_candidates[0].callsign, "COPY42");
                canonical_length += f64::from(bucket.sum_length_m);
                expected.entry(owner).or_default().push(bucket);
            }
        }
        if scoped {
            let southern = crate::spatial::square_id(49.0, 14.25).unwrap();
            assert!(
                !expected.contains_key(&southern) && !expected.is_empty(),
                "scope must drop owners beyond its 50 km buffer"
            );
        } else {
            let original_length = segments.iter().map(|s| f64::from(s.length_m)).sum::<f64>();
            assert!((canonical_length - original_length).abs() <= original_length * 1e-6);
        }
        assert_eq!(written, expected.len());
        assert_eq!(
            crate::spatial::square_directories(&prepared).unwrap().len(),
            expected.len()
        );
        for (square, mut rows) in expected {
            rows.sort_unstable_by_key(|r| {
                (r.cruise_cell_id, r.class, r.fl_bin, r.period, r.heading_bin)
            });
            let reference = directory.path().join("reference.arrow");
            write_cruise(&reference, &rows, &crate::provider_receipt::window_of(12, 0)).unwrap();
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
fn axial_buckets_merge_opposite_tracks_and_preserve_crossing_length_through_spill() {
    let cell = grid::cruise::cruise_cell_id(50.1, 14.2);
    let (lon, lat) = grid::cruise::cruise_centroid(cell);
    let (lon, lat) = (lon as f32, lat as f32);
    let segments = [
        cruise(1, lat, lon - 0.0001, lat, lon + 0.0001),
        cruise(2, lat, lon + 0.0001, lat, lon - 0.0001),
        cruise(3, lat - 0.0001, lon, lat + 0.0001, lon),
    ];
    let mut by_square = HashMap::new();
    for segment in &segments {
        process_segment(segment, &mut by_square, NpdLuts::shared());
    }
    let directory = tempfile::tempdir().unwrap();
    let mut buffers = SpillBuffers::new(0);
    buffers.buffer_flush(&mut by_square);
    buffers
        .write_all_buffers(directory.path(), &AtomicU64::new(0))
        .unwrap();
    let parts: Vec<_> = (0..SPILL_HASH_BUCKETS)
        .flat_map(|bucket| list_spill_parts(&spill_bucket_dir(directory.path(), bucket)).unwrap())
        .collect();
    let mut counts = crate::arrow_io::CruiseSpillCounts::default();
    for part in &parts {
        counts.merge(crate::arrow_io::CruiseSpillCounts::read(part).unwrap());
    }
    let mut rows = Vec::new();
    fold_bucket_sorted(&parts, None, counts, |_, mut square_rows| {
        rows.append(&mut square_rows);
        Ok(())
    })
    .unwrap();
    rows.sort_unstable_by_key(|row| row.heading_bin);
    assert_eq!(rows.len(), 2);
    assert_eq!((rows[0].heading_bin, rows[0].unique_count), (0, 2));
    assert_eq!((rows[1].heading_bin, rows[1].unique_count), (4, 1));
    let source_length: f32 = segments.iter().map(|segment| segment.length_m).sum();
    let stored_length: f32 = rows.iter().map(|row| row.sum_length_m).sum();
    assert!((stored_length - source_length).abs() < source_length * 1e-6);
}

#[test]
fn retained_spill_checks_input_window_inventory_and_refuses_partial_fold_resume() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("2025-01-01.arrow");
    crate::arrow_io::write_segments(&input, &[cruise(42, 50.1, 14.2, 50.1, 14.20001)]).unwrap();
    let prepared = directory.path().join("prepared");
    let paths = [input];
    let spill = directory.path().join("work/spill_cruise");
    run_stage_2b_phase(
        &admitted(&paths),
        &prepared,
        &spill,
        &crate::provider_receipt::window_of(1, 0),
        None,
        CruisePhase::Spill,
    )
    .unwrap();
    assert!(!directory.path().join("spill_cruise").exists());
    assert!(!prepared.exists());
    let identities = receipt::input_identities(&paths).unwrap();
    assert!(receipt::verify(&spill, &identities, &crate::provider_receipt::window_of(2, 0), None).is_err());
    let mut changed = identities.clone();
    changed[0].1.push_str("changed");
    assert!(receipt::verify(&spill, &changed, &crate::provider_receipt::window_of(1, 0), None).is_err());
    let parts: Vec<_> = (0..SPILL_HASH_BUCKETS)
        .flat_map(|bucket| list_spill_parts(&spill_bucket_dir(&spill, bucket)).unwrap())
        .collect();
    assert!(!parts.is_empty());
    let hidden = parts[0].with_extension("hidden");
    std::fs::rename(&parts[0], &hidden).unwrap();
    assert!(receipt::verify(&spill, &identities, &crate::provider_receipt::window_of(1, 0), None).is_err());
    std::fs::rename(&hidden, &parts[0]).unwrap();
    // Recreate the receipt because rename changed the recorded inode ctime.
    std::fs::remove_file(spill.join("state.sqlite")).unwrap();
    use std::os::unix::fs::OpenOptionsExt;
    let invalid_filesystem = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_PATH)
        .open(&spill)
        .unwrap();
    assert!(receipt::create(&spill, &invalid_filesystem, &identities, &crate::provider_receipt::window_of(1, 0), None).is_err());
    assert!(
        !spill.join("state.sqlite").exists(),
        "failed durability cannot seal raw spill"
    );
    let filesystem = std::fs::File::open(&spill).unwrap();
    receipt::create(&spill, &filesystem, &identities, &crate::provider_receipt::window_of(1, 0), None).unwrap();
    receipt::verify(&spill, &identities, &crate::provider_receipt::window_of(1, 0), None).unwrap();
    receipt::begin_fold(&spill).unwrap();
    assert!(receipt::begin_fold(&spill).is_err());
    let error = run_stage_2b_phase(
        &admitted(&paths),
        &prepared,
        &spill,
        &crate::provider_receipt::window_of(1, 0),
        None,
        CruisePhase::Finish,
    )
    .unwrap_err();
    assert!(error.to_string().contains("partial-fold resume"));
    assert!(parts[0].exists());
    assert!(!prepared.exists());
}

#[test]
fn old_sealed_spill_schema_is_rejected_before_prepared_outputs_are_wiped() {
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow::ipc::{reader::FileReader, writer::FileWriter};
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("2025-01-01.arrow");
    crate::arrow_io::write_segments(&input, &[cruise(42, 50.1, 14.2, 50.1, 14.20001)]).unwrap();
    let prepared = directory.path().join("prepared");
    let paths = [input];
    let spill = directory.path().join("work/spill_cruise");
    run_stage_2b_phase(
        &admitted(&paths),
        &prepared,
        &spill,
        &crate::provider_receipt::window_of(1, 0),
        None,
        CruisePhase::Spill,
    )
    .unwrap();
    assert!(!directory.path().join("spill_cruise").exists());
    let part = (0..SPILL_HASH_BUCKETS)
        .flat_map(|bucket| list_spill_parts(&spill_bucket_dir(&spill, bucket)).unwrap())
        .next()
        .unwrap();
    let schema = FileReader::try_new(std::fs::File::open(&part).unwrap(), None)
        .unwrap()
        .schema();
    let mut fields: Vec<_> = schema
        .fields()
        .iter()
        .map(|field| field.as_ref().clone())
        .collect();
    fields[12] = Field::new("rep_len_m", DataType::Float32, false);
    fields.insert(13, Field::new("rep_len_w", DataType::Float32, false));
    let old_schema = Schema::new(fields).with_metadata(schema.metadata().clone());
    FileWriter::try_new(std::fs::File::create(&part).unwrap(), &old_schema)
        .unwrap()
        .finish()
        .unwrap();
    std::fs::remove_file(spill.join("state.sqlite")).unwrap();
    receipt::create(
        &spill,
        &std::fs::File::open(&spill).unwrap(),
        &receipt::input_identities(&paths).unwrap(), &crate::provider_receipt::window_of(1, 0), None)
    .unwrap();
    let retained = prepared.join("z9/275/173/cruise.arrow");
    std::fs::create_dir_all(retained.parent().unwrap()).unwrap();
    std::fs::write(&retained, b"retained prepared output").unwrap();
    let error = run_stage_2b_phase(
        &admitted(&paths),
        &prepared,
        &spill,
        &crate::provider_receipt::window_of(1, 0),
        None,
        CruisePhase::Finish,
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("incompatible cruise spill schema"),
        "{error}"
    );
    assert_eq!(
        std::fs::read(retained).unwrap(),
        b"retained prepared output"
    );
    receipt::verify(
        &spill,
        &receipt::input_identities(&paths).unwrap(), &crate::provider_receipt::window_of(1, 0), None)
    .unwrap();
}

#[test]
fn spill_buffers_write_sorted_chunks_in_deterministic_order() {
    let directory = tempfile::tempdir().unwrap();
    let mut first = cruise(7, 50.1, 14.2, 50.1001, 14.2001);
    first.callsign = "SORT7".into();
    let mut second = cruise(8, 51.1, 15.2, 51.1001, 15.2001);
    second.callsign = "SORT8".into();
    // Identical accumulator states built twice: HashMap drain order differs,
    // the written rows must not.
    let mut rows = Vec::new();
    for _ in 0..2 {
        let mut local = HashMap::new();
        process_segment(&first, &mut local, NpdLuts::shared());
        process_segment(&second, &mut local, NpdLuts::shared());
        let mut buffers = SpillBuffers::new(3);
        buffers.buffer_flush(&mut local);
        assert!(local.is_empty());
        let spill = directory.path().join(format!("spill{}", rows.len()));
        buffers
            .write_all_buffers(&spill, &AtomicU64::new(0))
            .unwrap();
        let mut parts = Vec::new();
        for bucket in 0..SPILL_HASH_BUCKETS {
            parts.extend(list_spill_parts(&spill_bucket_dir(&spill, bucket)).unwrap());
        }
        assert!(parts.iter().all(|part| part
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("part_0003_")));
        let mut back = Vec::new();
        for part in &parts {
            crate::arrow_io::for_each_cruise_spill(part, |row| {
                // Bitwise fingerprint: floats by bits, so -0.0/NaN
                // differences fail loudly instead of hiding in ==.
                back.push(format!(
                    "{}|{}|{}|{}|{}|{}|{}|{}|{:x}|{:x}|{:x}|{:x}|{:?}|{:?}",
                    part.file_name().unwrap().to_str().unwrap(),
                    row.square,
                    row.cruise_cell_id,
                    row.class,
                    row.fl_bin,
                    row.period,
                    row.heading_bin,
                    row.secondary_only,
                    row.sum_length_m.to_bits(),
                    row.weight.to_bits(),
                    row.rep_alt_m.to_bits(),
                    row.rep_speed_kt.to_bits(),
                    row.fid_set,
                    row.top_candidates,
                ));
                Ok(())
            })
            .unwrap();
        }
        rows.push(back);
    }
    assert_eq!(rows[0], rows[1]);
    assert!(!rows[0].is_empty());
}

#[test]
fn repeated_extracts_publish_identical_cruise_rows() {
    use crate::arrow_io::read_record_batches;
    let mut outputs = Vec::new();
    for _ in 0..2 {
        let directory = tempfile::tempdir().unwrap();
        let segments: Vec<_> = (0..50)
            .map(|i| {
                cruise(
                    100 + i as u64,
                    50.0 + 0.002 * i as f32,
                    14.0,
                    50.001 + 0.002 * i as f32,
                    14.001,
                )
            })
            .collect();
        let day = directory.path().join("segments.arrow");
        crate::arrow_io::write_segments(&day, &segments).unwrap();
        let prepared = directory.path().join("prepared");
        let written = run_stage_2b(&[day], &prepared, 12, None).unwrap();
        assert!(written > 0);
        let mut files = Vec::new();
        for (square, _) in crate::spatial::square_directories(&prepared).unwrap() {
            let path = prepared.join(square_path(square)).join("cruise.arrow");
            files.push((square_path(square), read_record_batches(&path).unwrap()));
        }
        files.sort_by(|a, b| a.0.cmp(&b.0));
        outputs.push(files);
    }
    assert_eq!(outputs[0].len(), outputs[1].len());
    for (a, b) in outputs[0].iter().zip(outputs[1].iter()) {
        assert_eq!(a.0, b.0);
        assert_eq!(a.1, b.1, "cruise rows at {}", a.0);
    }
}
