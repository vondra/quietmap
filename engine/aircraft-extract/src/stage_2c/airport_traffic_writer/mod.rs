//! Ground traffic aggregated once per airport microsegment owner.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};
use noise_compute::emission::aircraft::{
    GROUND_OPS_KIND_APRON_MOVEMENT, GROUND_OPS_KIND_RUNWAY_ROLL, GROUND_OPS_KIND_TAXI,
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

use super::airport_traffic::{
    project_leg_onto_airport_lines, AirportLineSegment, AIRPORT_LINE_SNAP_BUFFER_M,
};

mod accumulate;
mod cache;
mod routing;
mod summary_parts;
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

#[derive(Eq, PartialEq, Hash, Clone)]
struct CounterKey {
    airport_key: String,
    osm_id: u64,
    segment_idx: u16,
    ops_kind: u8,
    is_departure: u8,
    veh_kind: u8,
    class_idx: u8,
    period: u8,
}

#[derive(Clone, Default)]
struct CounterAcc {
    fid_set: HashSet<u64>,
    fid_set_arr: HashSet<u64>,
    fid_set_dep: HashSet<u64>,
    fid_set_gse_per_class: [HashSet<u64>; NUM_GSE_CLASSES],
    band_energy_lin: [f64; NUM_BANDS],
    start_gx: i32,
    start_gy: i32,
    end_gx: i32,
    end_gy: i32,
    length_m: f32,
}

#[derive(Clone, Default)]
struct MicrosegAcc {
    fid_set: HashSet<u64>,
    fid_set_arr: HashSet<u64>,
    fid_set_dep: HashSet<u64>,
    fid_set_gse_per_class: [HashSet<u64>; NUM_GSE_CLASSES],
    fid_set_ga: HashSet<u64>,
    fid_set_ga_arr: HashSet<u64>,
    fid_set_ga_dep: HashSet<u64>,
}

#[derive(Clone, Default)]
struct AirportAggregateAcc {
    arr: HashSet<u64>,
    dep: HashSet<u64>,
    gse_per_class: [HashSet<u64>; NUM_GSE_CLASSES],
    ops_per_kind: [HashSet<u64>; 3],
    ga_arr: HashSet<u64>,
    ga_dep: HashSet<u64>,
    ga_ops_per_kind: [HashSet<u64>; 3],
}

/// Validate all inputs before replacing any prepared traffic output.
pub fn run_airport_traffic(
    segments_by_square_dir: &Path,
    airport_areas: &[AirportArea],
    prepared_year_dir: &Path,
    n_days: u16,
    ga_n_days: u16,
    scope: Option<&ScopeBbox>,
) -> Result<usize> {
    let plan = routing::ground_work_plan(segments_by_square_dir, prepared_year_dir, scope)?;
    crate::wipe::wipe_stale_arrows_for_scope(prepared_year_dir, "airport_traffic.arrow", scope)?;
    crate::wipe::wipe_stale_arrows_for_scope(
        prepared_year_dir,
        super::AIRPORT_SUMMARY_FILENAME,
        scope,
    )?;
    let parts_root = prepared_year_dir.join("airport_summary_parts");
    if parts_root.exists() {
        std::fs::remove_dir_all(&parts_root)?;
    }
    std::fs::create_dir_all(&parts_root)?;
    started(
        "stage2c/airport_traffic",
        &format!("{} line-owner squares", plan.len()),
    );
    let counter = Milestone::new("stage2c/airport_traffic", "line-owner squares", 50);
    let results: Vec<bool> = plan
        .par_iter()
        .map(|work| {
            let cache = SquareCache::load_many(prepared_year_dir, &work.candidates, airport_areas)?;
            let mut counters = HashMap::new();
            let mut micro_accs = HashMap::new();
            let mut airport_aggs = HashMap::new();
            for path in &work.inputs {
                crate::arrow_io::for_each_segment_batch(path, |segments| {
                    for segment in segments {
                        accumulate_segment(
                            &segment,
                            work.owner,
                            &cache,
                            &mut counters,
                            &mut micro_accs,
                            &mut airport_aggs,
                        );
                    }
                    Ok(())
                })
                .with_context(|| format!("read {}", path.display()))?;
            }
            let rows = counters_to_rows(counters, &micro_accs);
            if !rows.is_empty() {
                let relative = square_path(work.owner);
                write_airport_traffic(
                    &prepared_year_dir
                        .join(&relative)
                        .join("airport_traffic.arrow"),
                    &rows,
                    n_days,
                    ga_n_days,
                )?;
                write_airport_summary_parts(&parts_root.join(relative), &airport_aggs)?;
            }
            counter.add(1);
            Ok(!rows.is_empty())
        })
        .collect::<Result<_>>()?;
    let count = results.into_iter().filter(|written| *written).count();
    finished(
        "stage2c/airport_traffic",
        &format!("{count} line-owner squares written"),
    );
    Ok(count)
}

#[cfg(test)]
pub(crate) mod tests;
