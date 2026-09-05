//! Cruise aggregation and spill regressions.
use super::*;
use crate::geo::flat_dist;

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
    let par = merge_by_square(shard_a, shard_b);

    // Same z9 cells produced; iterate in sorted-by-key order so
    // HashMap iteration noise can't masquerade as a real bug.
    let mut seq_squares: Vec<u64> = seq.keys().copied().collect();
    let mut par_squares: Vec<u64> = par.keys().copied().collect();
    seq_squares.sort_unstable();
    par_squares.sort_unstable();
    assert_eq!(seq_squares, par_squares);

    for square in seq_squares {
        let seq_inner = seq.get(&square).unwrap();
        let par_inner = par.get(&square).unwrap();
        let mut seq_keys: Vec<CruiseKey> = seq_inner.keys().copied().collect();
        let mut par_keys: Vec<CruiseKey> = par_inner.keys().copied().collect();
        seq_keys.sort_unstable_by_key(|k| (k.cruise_cell_id, k.class, k.fl_bin, k.period));
        par_keys.sort_unstable_by_key(|k| (k.cruise_cell_id, k.class, k.fl_bin, k.period));
        assert_eq!(seq_keys, par_keys);

        for k in seq_keys {
            let sa = seq_inner.get(&k).unwrap();
            let pa = par_inner.get(&k).unwrap();
            let close = |a: f32, b: f32| {
                let denom = a.abs().max(b.abs()).max(1.0);
                (a - b).abs() <= denom * 1e-3
            };
            assert!(close(sa.sum_length_m, pa.sum_length_m), "sum_length_m");
            assert!(close(sa.weight, pa.weight), "weight");
            assert!(close(sa.rep_alt_m, pa.rep_alt_m), "rep_alt_m");
            assert!(close(sa.rep_speed_kt, pa.rep_speed_kt), "rep_speed_kt");
            assert!(close(sa.rep_len_m, pa.rep_len_m), "rep_len_m");
            assert_eq!(sa.fid_set.len(), pa.fid_set.len(), "fid_set size");
            assert_eq!(sa.top.len(), pa.top.len(), "top size");
        }
    }
}

/// Finalized cruise rows reach their supported destinations and leave no spill scratch.
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
    let n = run_stage_2b(&[day_path], &prepared_year, 1, None, false).unwrap();
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
    let n = run_stage_2b(&[], &prepared_year, 1, None, false).unwrap();
    assert_eq!(n, 0);
    assert!(!tmp.path().join("spill_cruise").exists());
}

/// GA-class cruise cross-check:
/// a C172-profile cruise segment warns but still processes by
/// default (plain extracts byte-identical), and hard-fails when
/// `fail_on_ga_cruise` is set.
#[test]
fn ga_class_cruise_warns_by_default_and_fails_behind_flag() {
    use crate::arrow_io::write_segments;
    let c172 = noise_compute::emission::aircraft::profile_idx("C172");
    let mut seg = cruise(7, 50.10, 14.20, 50.10, 14.21);
    seg.profile_idx = c172;
    let tmp = tempfile::tempdir().unwrap();
    let segments_dir = tmp.path().join("segments");
    std::fs::create_dir_all(&segments_dir).unwrap();
    let day_path = segments_dir.join("2025-07-01.arrow");
    write_segments(&day_path, &[seg]).unwrap();

    let prepared_year_warn = tmp.path().join("prepared_year_warn");
    std::fs::create_dir_all(&prepared_year_warn).unwrap();
    let n = run_stage_2b(
        std::slice::from_ref(&day_path),
        &prepared_year_warn,
        1,
        None,
        false,
    )
    .unwrap();
    assert!(n >= 1, "warn-only mode must still process the segment");

    let prepared_year_fail = tmp.path().join("prepared_year_fail");
    std::fs::create_dir_all(&prepared_year_fail).unwrap();
    let err = run_stage_2b(&[day_path], &prepared_year_fail, 1, None, true).unwrap_err();
    assert!(err.to_string().contains("GA-class cruise"), "{err}");
}

#[test]
fn merge_dedup_same_flight_id_across_shards() {
    // Same flight_id in two shards must collapse to one entry
    // in `fid_set` (and `top`) after `merge_by_square`.
    let s1 = cruise(99, 50.10, 14.20, 50.10, 14.205);
    let s2 = cruise(99, 50.10, 14.205, 50.10, 14.21);
    let mut shard_a: HashMap<u64, HashMap<CruiseKey, CruiseAccum>> = HashMap::new();
    let mut shard_b: HashMap<u64, HashMap<CruiseKey, CruiseAccum>> = HashMap::new();
    let luts = NpdLuts::shared();
    process_segment(&s1, &mut shard_a, luts);
    process_segment(&s2, &mut shard_b, luts);
    let merged = merge_by_square(shard_a, shard_b);
    for inner in merged.values() {
        for accum in inner.values() {
            assert_eq!(accum.fid_set.len(), 1, "fid 99 must dedupe across shards");
            assert!(accum.top.len() <= 1, "top entry for fid 99 also dedupes");
        }
    }
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
    let n = run_stage_2b(&[], &prepared_year, 1, Some(&scope), false).unwrap();
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
    let _ = run_stage_2b(&[], &prepared_year, 1, Some(&praha), false).unwrap();
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

/// Test-only symmetric merger over two `(z9 → bucket)` maps, used to
/// verify `CruiseAccum::merge` matches the sequential `add` path. The
/// production spill-merge does the same per-entry `merge` inline.
fn merge_by_square(
    mut a: HashMap<u64, HashMap<CruiseKey, CruiseAccum>>,
    mut b: HashMap<u64, HashMap<CruiseKey, CruiseAccum>>,
) -> HashMap<u64, HashMap<CruiseKey, CruiseAccum>> {
    if a.len() < b.len() {
        std::mem::swap(&mut a, &mut b);
    }
    for (square, b_inner) in b {
        let entry = a.entry(square).or_default();
        for (key, b_accum) in b_inner {
            match entry.get_mut(&key) {
                Some(existing) => existing.merge(b_accum),
                None => {
                    entry.insert(key, b_accum);
                }
            }
        }
    }
    a
}
