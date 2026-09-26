//! Ground traffic aggregated once per airport microsegment owner.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};
use noise_compute::emission::aircraft::{
    SamplingWindow, GROUND_OPS_KIND_RUNWAY_ROLL, GROUND_OPS_KIND_TAXI,
};
use noise_compute::emission::airport_traffic::{
    compute_aircraft_lw_per_meter_lin, compute_gse_band_energy_lin,
};
use noise_compute::emission::gse::NUM_GSE_CLASSES;
use noise_compute::emission::profiles_generated::noise_class_of;
use noise_compute::types::{AirportArea, NUM_BANDS};
use rayon::prelude::*;

use crate::arrow_io::{write_airport_traffic, AirportTrafficRow};
use crate::arrow_schemas::GEOMETRY_KIND_LINE;
use crate::flight::{FlightSegment, Phase};
use crate::geo::square_path;
use crate::progress::{finished, started, Milestone};
use crate::scope::ScopeBbox;

use super::airport_traffic::{AirportLineSegment, AIRPORT_LINE_SNAP_BUFFER_M};

mod accumulate;
mod cache;
mod routing;
pub use routing::{plan_ground_traffic, GroundTrafficPlan, GroundTrafficWork};
mod summary_parts;
use super::admission::{AllocationBudget, AllocationLimitExceeded};
use super::airport_line_grid::AirportLineGrid;
use super::movements::{self, MovementUnion};
use accumulate::{accumulate_segment, counters_to_rows};
use cache::SquareCache;
use summary_parts::write_airport_summary_parts;
pub(crate) use summary_parts::AirportSummaryPartRow;

fn ops_kind_from_aeroway(aeroway_type: u8) -> Option<u8> {
    match aeroway_type {
        0 | 6 | 7 => Some(GROUND_OPS_KIND_RUNWAY_ROLL), // runway / stopway / airstrip
        1 => Some(GROUND_OPS_KIND_TAXI),
        _ => None,
    }
}

#[derive(Eq, PartialEq, Ord, PartialOrd, Hash, Clone)]
struct CounterKey {
    /// Interned airport id into the cache table, which is sorted: id order
    /// is key order, so counters sort exactly as with owned strings.
    airport: u32,
    osm_id: u64,
    segment_idx: u16,
    ops_kind: u8,
    is_departure: u8,
    veh_kind: u8,
    class_idx: u8,
    period: u8,
    /// Secondary-only energy accumulates apart; it carries the increment weight.
    secondary_only: bool,
}

#[derive(Default)]
struct CounterAcc {
    fid_set: HashSet<u64>,
    band_energy_lin: [f64; NUM_BANDS],
    start_gx: i32,
    start_gy: i32,
    end_gx: i32,
    end_gy: i32,
    length_m: f32,
}

/// Validate all inputs before replacing any prepared traffic output.
pub(crate) fn run_airport_traffic(
    segments_by_square_dir: &Path,
    airport_areas: &[AirportArea],
    prepared_year_dir: &Path,
    output_year_dir: &Path,
    window: &SamplingWindow,
    scope: Option<&ScopeBbox>,
) -> Result<usize> {
    let aerodrome_index = crate::airport_index::AerodromeIndex::build(airport_areas);
    let plan = plan_ground_traffic(
        segments_by_square_dir,
        prepared_year_dir,
        scope,
        &aerodrome_index,
    )?;
    let largest = plan.works.iter().try_fold(0u64, |largest, work| {
        work.indexed_allocation().map(|bytes| largest.max(bytes))
    })?;
    // The shared plan stays resident beside every worker: size the pool off
    // the remaining budget and cap each worker off the remaining memory, so
    // one heavy square cannot starve the other workers into serial running.
    let process_limit = crate::memory::available_memory_bytes();
    let shared = plan.shared_bytes;
    let largest_with_shared = shared
        .checked_add(largest)
        .context("ground plan exceeds the memory limit")?;
    let budget = crate::memory::concurrent_allocation_budget_for(largest_with_shared)?;
    let threads = rayon::current_num_threads();
    let workers = ((budget.saturating_sub(shared) / largest.max(1)) as usize).clamp(1, threads);
    let worker_limit = (process_limit - shared) / workers as u64;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .build()?;
    let parts_root = output_year_dir.join("airport_summary_parts");
    crate::arrow_io::create_directory_all_synced(&parts_root)?;
    started(
        "stage2c/airport_traffic",
        &format!("{} line-owner squares", plan.works.len()),
    );
    let counter = Milestone::new("stage2c/airport_traffic", "line-owner squares", 50);
    let count = pool.install(|| {
        run_with_retry_pool(&plan.works, worker_limit, process_limit - shared, |work, limit| {
            let outcome = run_ground_traffic_work(
                work,
                prepared_year_dir,
                output_year_dir,
                &aerodrome_index,
                window,
                limit,
            )?;
            counter.add(1);
            Ok(outcome.counter_rows > 0)
        })
    })?;
    finished(
        "stage2c/airport_traffic",
        &format!("{count} line-owner squares written"),
    );
    Ok(count)
}

/// Phase 1 runs every owner on the current pool with the worker cap; owners
/// refused there retry with the process cap on a narrow pool. Retries are
/// growth-dominated (charged ≥3× actual: hash tables at 4× plus coexistence),
/// so 8 concurrent retries peak far below the process limit, while a serial
/// tail would stall the stage on every heavy airport square.
fn run_with_retry_pool(
    plan: &[GroundTrafficWork],
    worker_limit: u64,
    process_limit: u64,
    run: impl Fn(&GroundTrafficWork, u64) -> Result<bool> + Sync,
) -> Result<usize> {
    let results: Vec<Option<bool>> = plan
        .par_iter()
        .map(|work| match run(work, worker_limit) {
            Ok(written) => Ok(Some(written)),
            Err(error) if worker_limit < process_limit && error.is::<AllocationLimitExceeded>() => {
                eprintln!(
                    "[stage2c] {} deferred for retry: {error:#}",
                    square_path(work.owner)
                );
                Ok(None)
            }
            Err(error) => Err(error),
        })
        .collect::<Result<_>>()?;
    // All parallel accumulators are gone. Admission failures precede output writes.
    let mut count = 0;
    let mut deferred = Vec::new();
    for (work, result) in plan.iter().zip(results) {
        match result {
            Some(written) => count += usize::from(written),
            None => deferred.push(work),
        }
    }
    if deferred.is_empty() {
        return Ok(count);
    }
    let width = rayon::current_num_threads()
        .clamp(1, 8)
        .min(deferred.len());
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(width)
        .build()?;
    let retried: Vec<bool> = pool.install(|| {
        deferred
            .par_iter()
            .map(|work| run(work, process_limit))
            .collect::<Result<_>>()
    })?;
    count += retried.iter().map(|written| usize::from(*written)).sum::<usize>();
    Ok(count)
}

/// Actual allocated membership groups for the exact producer work item.
pub struct GroundTrafficOutcome {
    pub counter_rows: usize,
    pub microsegments: usize,
    pub airports: usize,
    pub counter_memberships: usize,
    pub microsegment_memberships: usize,
    pub airport_memberships: usize,
    pub charged_bytes: u64,
}

/// Execute one validated routing item with the same source order and energy accumulation.
pub fn run_ground_traffic_work(
    work: &GroundTrafficWork,
    prepared_year_dir: &Path,
    output_year_dir: &Path,
    aerodrome_index: &crate::airport_index::AerodromeIndex,
    window: &SamplingWindow,
    worker_limit: u64,
) -> Result<GroundTrafficOutcome> {
    let mut budget = AllocationBudget::new(worker_limit, work.indexed_allocation()?)?;
    let cache = SquareCache::load_many(prepared_year_dir, &work.candidates, aerodrome_index)?;
    let mut line_index = AirportLineGrid::new(
        &cache.lines,
        AIRPORT_LINE_SNAP_BUFFER_M,
        AirportLineGrid::allocation_allowance(work.cached_lines)?,
    )?;
    let mut counters = HashMap::new();
    let mut micro_accs = HashMap::new();
    let mut airport_aggs = HashMap::new();
    let mut intersections = Vec::new();
    for path in &work.inputs {
        crate::arrow_io::for_each_segment_batch(path, |segments| {
            for segment in segments {
                accumulate_segment(
                    &segment,
                    (work.owner, &mut line_index),
                    &cache,
                    &mut intersections,
                    &mut counters,
                    &mut micro_accs,
                    &mut airport_aggs,
                    &mut budget,
                )?;
            }
            Ok(())
        })
        .with_context(|| format!("read {}", path.display()))?;
    }
    let outcome = GroundTrafficOutcome {
        counter_rows: counters.len(),
        microsegments: micro_accs.len(),
        airports: airport_aggs.len(),
        counter_memberships: counters.values().map(|value| value.fid_set.len()).sum(),
        microsegment_memberships: micro_accs.values().map(|value| value.members.len()).sum(),
        airport_memberships: airport_aggs.values().map(|value| value.members.len()).sum(),
        charged_bytes: budget.reserved(),
    };
    let rows = counters_to_rows(counters, &micro_accs, &cache.airports);
    if !rows.is_empty() {
        let relative = square_path(work.owner);
        write_airport_traffic(
            &output_year_dir
                .join(&relative)
                .join("airport_traffic.arrow"),
            &rows,
            window,
        )?;
        write_airport_summary_parts(
            &output_year_dir.join("airport_summary_parts").join(relative),
            &airport_aggs,
            &cache.airports,
        )?;
    }
    eprintln!(
        "[stage2c] {} rows={} microsegments={} airports={} charged={} B",
        square_path(work.owner),
        rows.len(),
        micro_accs.len(),
        airport_aggs.len(),
        budget.reserved()
    );
    Ok(outcome)
}

#[cfg(test)]
pub(crate) mod tests;
