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

use crate::arrow_io::for_each_segment_batch;
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
    let square_inputs = list_square_shards(segments_by_square_dir, "airborne.arrow", scope)?;
    let mut largest_allocation = 0;
    for (_, path) in &square_inputs {
        largest_allocation = largest_allocation.max(shard_allocation_allowance(path)?);
    }
    let workers =
        crate::memory::max_concurrent_tasks(rayon::current_num_threads(), largest_allocation)?;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .build()?;
    // Validate every input before replacing any prior output.
    let wiped =
        crate::wipe::wipe_stale_arrows_for_scope(prepared_year_dir, "airborne.arrow", scope)?;
    if wiped > 0 {
        eprintln!(
            "{} [stage2a] wiped {wiped} stale airborne.arrow file(s) before write",
            ts()
        );
    }
    let n_square = square_inputs.len();
    started("stage2a", &format!("{n_square} z9 cells; {workers} workers; {largest_allocation} B allocation allowance each"));
    let stage_start = std::time::Instant::now();

    let square_counter = Milestone::new("stage2a", "z9 cells", 100);
    let seg_counter = Milestone::new("stage2a", "segments in", 1_000_000);
    let evt_counter = Milestone::new("stage2a", "events out", 100_000);
    let written = std::sync::atomic::AtomicUsize::new(0);
    pool.install(|| {
        square_inputs
            .par_iter()
            .try_for_each(|(square, shard_path)| -> Result<()> {
                let mut events = AirborneEvents::default();
                for_each_segment_batch(shard_path, |segments| {
                    seg_counter.add(segments.len() as u64);
                    events.extend(&segments);
                    Ok(())
                })
                .with_context(|| format!("read {}", shard_path.display()))?;
                let events = events.finish();
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
            })
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

/// Raw IPC bytes cover both the growing subsegment vectors and their two Arrow
/// representations during spatial sorting. Flight runs bound hash/event overhead;
/// the source shard itself is released one decoded batch at a time.
fn shard_allocation_allowance(path: &Path) -> Result<u64> {
    let file = std::fs::File::open(path)?;
    let bytes = file.metadata()?.len();
    let reader = arrow::ipc::reader::FileReader::try_new(std::io::BufReader::new(file), None)?;
    let schema = reader.schema();
    crate::arrow_schemas::assert_schema_version(schema.metadata())?;
    let count = |key: &str| -> Result<u64> {
        schema
            .metadata()
            .get(key)
            .with_context(|| {
                format!(
                    "{} lacks {key}; run shuffle with the current writer",
                    path.display()
                )
            })?
            .parse()
            .with_context(|| format!("invalid {key} in {}", path.display()))
    };
    Ok(3 * bytes
        + 768 * count("flight_runs")?
        + 2 * count("flight_run_callsign_bytes")?
        + (1 << 30))
}

#[derive(Default)]
struct AirborneEvents(HashMap<u64, AirborneEventBuilder>);

impl AirborneEvents {
    fn extend(&mut self, segments: &[FlightSegment]) {
        for seg in segments {
            if seg.phase != Phase::Airborne || seg.veh_kind != 0 {
                continue;
            }
            self.0
                .entry(seg.flight_id)
                .or_insert_with(|| AirborneEventBuilder::new(seg))
                .push(seg);
        }
    }

    fn finish(self) -> Vec<AirborneEvent> {
        let mut events: Vec<_> = self
            .0
            .into_values()
            .map(AirborneEventBuilder::finish)
            .collect();
        // Stable input order also stabilizes equal-key spatial batches and their bytes.
        events.sort_unstable_by_key(|event| event.flight_id);
        events
    }
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
#[path = "stage_2a_tests.rs"]
mod tests;
