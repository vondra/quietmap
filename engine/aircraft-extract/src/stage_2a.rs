//! Stage 2A — Airborne segments → per-z9 `airborne.arrow`.
//!
//! One row per (flight, z9) crossing. Sub-segments (each carrying its
//! own period / date / flags) are kept in a `List<Struct>` so a long
//! crossing that straddles 19:00 still gets the correct Lden weighting.
//!
//! Consumes the per-z9 airborne shards produced by
//! [`crate::shuffle::shuffle_per_square`] — one input file per z9 means each
//! worker owns its z9's segments + accumulator, no global merge.
//!
//! Each sub-segment retains the start/end terrain elevations sampled in Stage 1.
//! Intermediate chord terrain is not stored; see SPEC §6.1's filtering contract.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use rayon::prelude::*;

use crate::arrow_io::read_segments;
use crate::flight::{segment_flags, AirborneEvent, AirborneSubSegment, FlightSegment, Phase};
use crate::geo::square_path;
use crate::progress::{finished, human, started, ts, Milestone};
use crate::scope::ScopeBbox;
use crate::shuffle::list_square_shards;

/// Run Stage 2A against the shuffled per-z9 airborne shards under
/// `segments_by_square_dir/<z9>/airborne.arrow`. When `scope` is set, z9
/// subdirs outside it are skipped — scope was applied during shuffle
/// already, so this is defensive and cheap.
///
pub fn run_stage_2a(
    segments_by_square_dir: &Path,
    prepared_year_dir: &Path,
    n_days: u16,
    // GA-class window (0 = single-window extract). Stamped into the
    // airborne.arrow metadata so popup/heatmap weight GA rows at
    // `1/ga_n_days`.
    ga_n_days: u16,
    scope: Option<&ScopeBbox>,
) -> Result<usize> {
    // Wipe stale airborne.arrow from in-scope z9s before workers write
    // fresh files. z9s with no airborne activity this run would otherwise
    // retain a prior-run file (possibly older schema) and the popup
    // reader would fatal-fail on schema_version mismatch. Symmetric to
    // the Stage 2B/2C guards.
    let wiped =
        crate::wipe::wipe_stale_arrows_for_scope(prepared_year_dir, "airborne.arrow", scope)?;
    if wiped > 0 {
        eprintln!(
            "{} [stage2a] wiped {wiped} stale airborne.arrow file(s) before write",
            ts()
        );
    }
    let square_inputs = list_square_shards(segments_by_square_dir, "airborne.arrow", scope)?;
    let n_square = square_inputs.len();
    started("stage2a", &format!("{n_square} z9 cells"));
    let stage_start = std::time::Instant::now();

    let square_counter = Milestone::new("stage2a", "z9 cells", 100);
    let seg_counter = Milestone::new("stage2a", "segments in", 1_000_000);
    let evt_counter = Milestone::new("stage2a", "events out", 100_000);
    let written = std::sync::atomic::AtomicUsize::new(0);
    square_inputs
        .par_iter()
        .try_for_each(|(square, shard_path)| -> Result<()> {
            let segments = read_segments(shard_path)
                .with_context(|| format!("read {}", shard_path.display()))?;
            seg_counter.add(segments.len() as u64);
            let events = aggregate_events_for_square(&segments);
            square_counter.add(1);
            if events.is_empty() {
                return Ok(());
            }
            evt_counter.add(events.len() as u64);
            let dir = prepared_year_dir.join(square_path(*square));
            std::fs::create_dir_all(&dir)?;
            crate::arrow_io::write_airborne(
                &dir.join("airborne.arrow"),
                &events,
                n_days,
                ga_n_days,
            )?;
            written.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(())
        })?;
    let written = written.load(std::sync::atomic::Ordering::Relaxed);
    let empty = n_square.saturating_sub(written);
    finished(
        "stage2a",
        &format!(
            "{written}/{n_square} z9s wrote {} events from {} segments ({} empty after gates) in {:?}",
            human(evt_counter.total()),
            human(seg_counter.total()),
            empty,
            stage_start.elapsed()
        ),
    );
    Ok(written)
}

fn aggregate_events_for_square(segments: &[FlightSegment]) -> Vec<AirborneEvent> {
    let mut by_flight: HashMap<u64, AirborneEventBuilder> = HashMap::new();
    for seg in segments {
        // Shuffle pre-filtered by Phase; veh_kind only filtered here
        // because the schema can't distinguish aircraft from GSE at
        // shuffle time (Phase::Ground covers both; Airborne should
        // already be aircraft-only, but defense-in-depth is cheap).
        if seg.phase != Phase::Airborne || seg.veh_kind != 0 {
            continue;
        }
        by_flight
            .entry(seg.flight_id)
            .or_insert_with(|| AirborneEventBuilder::new(seg))
            .push(seg);
    }
    by_flight
        .into_values()
        .map(AirborneEventBuilder::finish)
        .collect()
}

struct AirborneEventBuilder {
    flight_id: u64,
    callsign: String,
    aircraft_type: [u8; 4],
    profile_idx: u8,
    source_id: u8,
    origin: u8,
    sub_segments: Vec<AirborneSubSegment>,
}

impl AirborneEventBuilder {
    fn new(seed: &FlightSegment) -> Self {
        Self {
            flight_id: seed.flight_id,
            callsign: seed.callsign.clone(),
            aircraft_type: seed.aircraft_type,
            profile_idx: seed.profile_idx,
            source_id: seed.source_id,
            origin: seed.origin,
            sub_segments: Vec::new(),
        }
    }
    fn push(&mut self, seg: &FlightSegment) {
        let mut flags = 0u8;
        if seg.is_departure() {
            flags |= segment_flags::IS_DEPARTURE;
        }
        // Only Stage 1's start/end terrain elevations are stored. Runtime
        // consumers use them for stale-ground and Filter D checks; the removed
        // q1/mid/q3 chord check remains the documented SPEC §6.1 gap.
        self.sub_segments.push(AirborneSubSegment {
            start_lat: seg.start_lat,
            start_lon: seg.start_lon,
            start_alt_m: seg.start_alt_m,
            end_lat: seg.end_lat,
            end_lon: seg.end_lon,
            end_alt_m: seg.end_alt_m,
            speed_kt: seg.speed_kt,
            length_m: seg.length_m,
            period: seg.period,
            date_id: seg.date_id,
            flags,
            terrain_start_elev_m: seg.start_elev_m,
            terrain_end_elev_m: seg.end_elev_m,
        });
    }
    fn finish(self) -> AirborneEvent {
        AirborneEvent {
            flight_id: self.flight_id,
            callsign: self.callsign,
            aircraft_type: self.aircraft_type,
            profile_idx: self.profile_idx,
            source_id: self.source_id,
            origin: self.origin,
            sub_segments: self.sub_segments,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(flight_id: u64, lat: f32, lon: f32) -> FlightSegment {
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
            phase: Phase::Airborne,
            flags: 0,
            start_lat: lat,
            start_lon: lon,
            start_alt_m: 1000.0,
            end_lat: lat + 0.001,
            end_lon: lon + 0.001,
            end_alt_m: 1100.0,
            speed_kt: 250.0,
            length_m: 200.0,
            agl_avg_m: 500.0,
            start_elev_m: 250.0,
            end_elev_m: 260.0,
        }
    }

    #[test]
    fn aggregate_groups_per_flight() {
        let s1 = seg(1, 50.10, 14.26);
        let s2 = seg(1, 50.10, 14.27);
        let s3 = seg(2, 50.10, 14.26);
        let segs = vec![s1, s2, s3];
        let events = aggregate_events_for_square(&segs);
        assert_eq!(events.len(), 2);
        let f1 = events.iter().find(|e| e.flight_id == 1).unwrap();
        assert_eq!(f1.sub_segments.len(), 2);
        let f2 = events.iter().find(|e| e.flight_id == 2).unwrap();
        assert_eq!(f2.sub_segments.len(), 1);
    }

    #[test]
    fn aggregate_filters_non_aircraft_and_non_airborne() {
        let mut gse = seg(1, 50.10, 14.26);
        gse.veh_kind = 1;
        let mut ground = seg(2, 50.10, 14.26);
        ground.phase = Phase::Ground;
        let ok = seg(3, 50.10, 14.26);
        let events = aggregate_events_for_square(&[gse, ground, ok]);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].flight_id, 3);
    }

    #[test]
    fn aggregate_propagates_terrain_elevs_from_stage1() {
        // Synthetic case: Stage 1 said start=250 m, end=260 m. Verify both
        // endpoint elevations pass through unchanged.
        let events = aggregate_events_for_square(&[seg(1, 50.10, 14.26)]);
        assert_eq!(events.len(), 1);
        let sub = &events[0].sub_segments[0];
        assert!((sub.terrain_start_elev_m - 250.0).abs() < 1e-3);
        assert!((sub.terrain_end_elev_m - 260.0).abs() < 1e-3);
    }

    /// Round trip: write a per-z9 airborne shard via the shuffle
    /// schema, run Stage 2A, verify an airborne.arrow lands in prepared_year.
    #[test]
    fn run_stage_2a_consumes_per_square_shard() {
        use crate::arrow_io::write_segments;
        let tmp = tempfile::tempdir().unwrap();
        let by_square = tmp.path().join("segments_by_square");
        let prepared_year = tmp.path().join("prepared_year");
        let square = { crate::spatial::square_id(50.10, 14.26).unwrap() };
        let square_dir = by_square.join(square_path(square));
        std::fs::create_dir_all(&square_dir).unwrap();
        write_segments(
            &square_dir.join("airborne.arrow"),
            &[seg(1, 50.10, 14.26), seg(2, 50.10, 14.27)],
        )
        .unwrap();

        let n = run_stage_2a(&by_square, &prepared_year, 1, 0, None).unwrap();
        assert_eq!(n, 1);
        let out = prepared_year
            .join(square_path(square))
            .join("airborne.arrow");
        assert!(out.exists(), "Stage 2A must write airborne.arrow");
    }

    /// Regression for wipe-on-scope applied to airborne: a stale
    /// `airborne.arrow` in an in-scope z9 must be wiped before
    /// `run_stage_2a` returns, even if no airborne segments hit that
    /// z9 this run. Symmetric to Stage 2B/2C tests.
    #[test]
    fn run_stage_2a_wipes_in_scope_stale_airborne() {
        let tmp = tempfile::tempdir().unwrap();
        let by_square = tmp.path().join("segments_by_square");
        let prepared_year = tmp.path().join("prepared_year");
        // Praha z9 — in-scope. No segments_by_square input → writer does
        // not emit a fresh airborne.arrow for this run.
        let square = crate::spatial::square_id(50.10, 14.26).unwrap();
        let square_dir = prepared_year.join(square_path(square));
        std::fs::create_dir_all(&square_dir).unwrap();
        let stale = square_dir.join("airborne.arrow");
        std::fs::write(&stale, b"stale-prev-run").unwrap();
        std::fs::create_dir_all(&by_square).unwrap();
        let scope = ScopeBbox::parse("48.65,12.00,51.55,16.90").unwrap();
        let n = run_stage_2a(&by_square, &prepared_year, 1, 0, Some(&scope)).unwrap();
        assert_eq!(n, 0, "no z9 shards → no z9 written");
        assert!(
            !stale.exists(),
            "stale airborne.arrow must be wiped from in-scope z9"
        );
    }

    /// Out-of-scope counterexample for the airborne wipe: a stale
    /// `airborne.arrow` in an z9 OUTSIDE the scope bbox must survive.
    #[test]
    fn run_stage_2a_leaves_out_of_scope_stale_airborne() {
        let tmp = tempfile::tempdir().unwrap();
        let by_square = tmp.path().join("segments_by_square");
        let prepared_year = tmp.path().join("prepared_year");
        // Gran Canaria z9 — outside Praha scope.
        let square = crate::spatial::square_id(27.93, -15.39).unwrap();
        let square_dir = prepared_year.join(square_path(square));
        std::fs::create_dir_all(&square_dir).unwrap();
        let stale = square_dir.join("airborne.arrow");
        std::fs::write(&stale, b"stale-prev-run").unwrap();
        std::fs::create_dir_all(&by_square).unwrap();
        let praha = ScopeBbox::parse("48.65,12.00,51.55,16.90").unwrap();
        let _ = run_stage_2a(&by_square, &prepared_year, 1, 0, Some(&praha)).unwrap();
        assert!(
            stale.exists(),
            "out-of-scope z9 airborne.arrow must survive a scoped reextract"
        );
    }
}
