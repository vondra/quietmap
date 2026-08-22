//! Stage 2C airport_traffic.arrow aggregator.
//!
//! Walks Stage 1 ground segments, projects each leg onto OSM airport_line
//! microsegments via [`super::airport_traffic`], computes per-band per-
//! movement SEL via [`noise_compute::emission::airport_traffic`], and
//! aggregates into sparse per-R4 [`AirportTrafficRow`]s.
//!
//! ## What this writer does NOT do
//!
//! - **Per-event total-length normalization**: the energy kernel uses a
//!   fixed 1 km nominal event length. Real taxi events vary 200 m–3 km;
//!   per-row energy conservation is approximate.
//! - **strip:R7 fallback** for misses against `airport_areas.arrow`:
//!   segments whose midpoint has no nearest aerodrome within range fall
//!   under `airport_key = "strip:<R7_hex>"`.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};
use h3o::Resolution;
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

use crate::airport_io::{nearest_aerodrome_within, read_airport_lines, AirportLineRow};
use crate::arrow_io::{write_airport_traffic, AirportTrafficRow};
use crate::arrow_schemas::GEOMETRY_KIND_LINE;
use crate::flight::{FlightSegment, Phase};
use crate::geo::{lat_lon_to_cell, midpoint, r4_hex_str};
use crate::progress::{finished, human, started, ts, Milestone};
use crate::scope::ScopeBbox;
use crate::synth_airport_io::{is_synthetic_osm_id, read_synth_airport_lines, SYNTH_LINES_FILE};

use super::airport_traffic::{
    project_leg_onto_airport_lines, AirportLineSegment, AIRPORT_LINE_SNAP_BUFFER_M,
};

/// Per-R4 partial airport-summary fid set dumped during Stage 2C so
/// the reduce phase can UNION across R4s without holding the full
/// global state in RAM. One entry per `(airport_key, R4)` pair, with
/// raw fid sets per arr / dep / GSE-class / ops-kind dimension. The
/// final reduce reads these, unions, and writes the global
/// `airport_summary.arrow`.
pub(crate) struct AirportSummaryPartRow {
    pub airport_key: String,
    /// NON-GA-class arr/dep/ops fid sets (airline 12-day window).
    pub arr_fids: Vec<u64>,
    pub dep_fids: Vec<u64>,
    pub gse_fids_per_class: [Vec<u64>; NUM_GSE_CLASSES],
    /// Index 0=runway, 1=taxi, 2=apron — matches `GROUND_OPS_KIND_*`
    /// minus 1. VEH_KIND=0 only; GSE has separate class sets.
    pub ops_fids_per_kind: [Vec<u64>; 3],
    /// GA-class (PROP_C172 + HELICOPTER) arr/dep/ops fid sets. The popup
    /// divides these by `ga_n_days`; GSE has no GA split (airline-pass only).
    pub ga_arr_fids: Vec<u64>,
    pub ga_dep_fids: Vec<u64>,
    pub ga_ops_fids_per_kind: [Vec<u64>; 3],
}

/// OSM aeroway_type → `ops_kind` mapping. osm-extract only
/// emits aeroway_type ∈ {0=runway, 1=taxiway, 6=stopway, 7=airstrip}
/// to `airport_lines.arrow` (aprons are area features, in
/// `airport_areas.arrow`). Anything else is corrupt input — return
/// `None` so the caller skips the row rather than fabricate
/// apron-movement emission for a runway sentinel.
fn ops_kind_from_aeroway(aeroway_type: u8) -> Option<u8> {
    match aeroway_type {
        0 | 6 | 7 => Some(GROUND_OPS_KIND_RUNWAY_ROLL), // runway / stopway / airstrip
        1 => Some(GROUND_OPS_KIND_TAXI),
        _ => None,
    }
}

fn cell_u64(lat: f64, lon: f64, res: Resolution) -> Option<u64> {
    lat_lon_to_cell(lat, lon, res).map(u64::from)
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

/// Per-R4 worker telemetry rolled up into one summary across all R4s
/// for the diagnostic log lines + flight_id=0 warning.
struct R4Totals {
    rows_written: usize,
    ground_seg_count: usize,
    fid_zero_count: usize,
}

#[derive(Clone, Default)]
struct CounterAcc {
    /// Distinct fids that crossed this microsegment-row regardless
    /// of `ops_kind / is_departure / veh_kind`. Display source for
    /// `unique_movement_count`.
    fid_set: HashSet<u64>,
    /// Subset for runway-roll arrivals (ops_kind=RUNWAY_ROLL,
    /// is_departure=0, veh_kind=0). Populated only when the row's
    /// key matches arrival semantics.
    fid_set_arr: HashSet<u64>,
    /// Analogous for departures.
    fid_set_dep: HashSet<u64>,
    /// Per-GSE-class fids that touched this row (veh_kind=1 only).
    fid_set_gse_per_class: [HashSet<u64>; NUM_GSE_CLASSES],
    band_energy_lin: [f64; NUM_BANDS],
    // Geometry copy for the eventual row (denormalized so popup
    // doesn't have to join back against airport_lines).
    start_lat: f32,
    start_lon: f32,
    end_lat: f32,
    end_lon: f32,
    length_m: f32,
}

/// Per-microsegment aggregator. UNION across
/// every row of the same `(osm_id, segment_idx)` regardless of period
/// / class / ops_kind. Replicated onto every row of the microsegment
/// at write time so the popup loader can populate per-microsegment
/// observed_movements directly without a HashSet union over rows.
#[derive(Clone, Default)]
struct MicrosegAcc {
    fid_set: HashSet<u64>,
    fid_set_arr: HashSet<u64>,
    fid_set_dep: HashSet<u64>,
    fid_set_gse_per_class: [HashSet<u64>; NUM_GSE_CLASSES],
    /// v9 GA-class split of the three aircraft fid sets above, so the popup
    /// divides `non_ga / n_days + ga / ga_n_days`. GSE is airline-pass only.
    fid_set_ga: HashSet<u64>,
    fid_set_ga_arr: HashSet<u64>,
    fid_set_ga_dep: HashSet<u64>,
}

/// Per-R4 airport-key aggregate — partial UNIONs computed inside ONE
/// R4. Cross-R4 UNION happens in the Stage 2C v5 reduce phase from
/// the per-airport-R4 `airport_summary_parts/` dumps.
#[derive(Clone, Default)]
struct AirportAggregateAcc {
    arr: HashSet<u64>,
    dep: HashSet<u64>,
    gse_per_class: [HashSet<u64>; NUM_GSE_CLASSES],
    /// Index 0=runway, 1=taxi, 2=apron (VEH_KIND=0 only).
    ops_per_kind: [HashSet<u64>; 3],
    /// v9 GA-class split of arr/dep/ops for the airport-level union.
    ga_arr: HashSet<u64>,
    ga_dep: HashSet<u64>,
    ga_ops_per_kind: [HashSet<u64>; 3],
}

/// Run Stage 2C against the shuffled per-R4 ground shards under
/// `segments_by_r4_dir/<R4>/ground.arrow`. Per-R4 worker owns its
/// cache + counters + rows; no shared state. Worst-R4 working set
/// bounds peak RAM at `cores × per-R4`. Scope filter is defensive —
/// shuffle already excluded out-of-scope R4s.
pub fn run_airport_traffic(
    segments_by_r4_dir: &Path,
    airport_areas: &[AirportArea],
    h3r4_dir: &Path,
    n_days: u16,
    // GA-class window (0 = single-window extract). Stamped into the
    // arrow metadata so the popup normalizes GA-split movement counts and
    // energy by `ga_n_days`.
    ga_n_days: u16,
    scope: Option<&ScopeBbox>,
) -> Result<usize> {
    // Wipe stale airport_summary_parts/ before workers fill it.
    // Mirrors stage_2b's spill_cruise wipe pattern (stage_2b.rs:125).
    // Without this, a prior crash + different scope can leave stale
    // per-R4 partials that the reduce phase would silently UNION
    // into the new global summary, over-counting fids for airports
    // touched by both extracts.
    let parts_root = h3r4_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("h3r4_dir has no parent for airport_summary_parts"))?
        .join("airport_summary_parts");
    if parts_root.exists() {
        std::fs::remove_dir_all(&parts_root)?;
    }

    let r4_inputs = crate::shuffle::list_r4_shards(segments_by_r4_dir, "ground.arrow", scope)?;
    let n_input_r4 = r4_inputs.len();
    started("stage2c/airport_traffic", &format!("{n_input_r4} R4 cells"));

    let stage_start = std::time::Instant::now();
    let r4_counter = Milestone::new("stage2c/airport_traffic", "R4 cells", 50);
    let seg_counter = Milestone::new("stage2c/airport_traffic", "ground segments", 100_000);
    let totals: Vec<R4Totals> = r4_inputs
        .into_par_iter()
        .map(|(r4, shard_path)| {
            let segments = crate::arrow_io::read_segments(&shard_path)
                .with_context(|| format!("read {}", shard_path.display()))?;
            seg_counter.add(segments.len() as u64);
            let totals = process_one_r4(r4, &segments, airport_areas, h3r4_dir, n_days, ga_n_days)?;
            r4_counter.add(1);
            Ok(totals)
        })
        .collect::<Result<Vec<_>>>()?;

    let n_r4 = totals.iter().filter(|t| t.rows_written > 0).count();
    let total_segs: usize = totals.iter().map(|t| t.ground_seg_count).sum();
    let fid_zero_count: usize = totals.iter().map(|t| t.fid_zero_count).sum();
    let empty = n_input_r4.saturating_sub(n_r4);
    finished(
        "stage2c/airport_traffic",
        &format!(
            "{n_r4}/{n_input_r4} R4s with airport_traffic, {} ground segments processed ({} empty R4s) in {:?}",
            human(total_segs as u64),
            empty,
            stage_start.elapsed()
        ),
    );

    if fid_zero_count > 0 {
        eprintln!(
            "{} [stage2c] WARNING: {fid_zero_count} of {total_segs} ground segments \
             carry flight_id=0 — likely a stale Stage 0/1 binary. All such segments \
             collapse into one HashSet entry per counter row, so `unique_movement_count` \
             will UNDER-COUNT real rotations by the unobserved rotation diversity. \
             Rebuild aircraft-extract and re-extract Stage 0/1 before trusting the \
             popup display.",
            ts()
        );
    }
    Ok(n_r4)
}

/// Owns all state for one R4 worker — peer workers never observe
/// these counters; per-R4 working set bounds peak RAM.
fn process_one_r4(
    r4: u64,
    segments: &[FlightSegment],
    airport_areas: &[AirportArea],
    h3r4_dir: &Path,
    n_days: u16,
    ga_n_days: u16,
) -> Result<R4Totals> {
    let cache = R4Cache::load(h3r4_dir, r4, airport_areas);
    let mut counters: HashMap<CounterKey, CounterAcc> = HashMap::new();
    let mut micro_accs: HashMap<(u64, u16), MicrosegAcc> = HashMap::new();
    let mut airport_aggs: HashMap<String, AirportAggregateAcc> = HashMap::new();
    let mut fid_zero_count = 0usize;
    let mut ground_seg_count = 0usize;
    for seg in segments {
        // Defensive — shuffle already filtered by phase, but the
        // schema can't enforce it.
        if seg.phase != Phase::Ground {
            continue;
        }
        ground_seg_count += 1;
        if seg.flight_id == 0 {
            fid_zero_count += 1;
        }
        accumulate_segment(
            seg,
            &cache,
            &mut counters,
            &mut micro_accs,
            &mut airport_aggs,
        );
    }
    let rows = counters_to_rows(counters, &micro_accs);
    let rows_written = rows.len();
    if rows_written > 0 {
        let dir = h3r4_dir.join(r4_hex_str(r4));
        std::fs::create_dir_all(&dir)?;
        write_airport_traffic(&dir.join("airport_traffic.arrow"), &rows, n_days, ga_n_days)?;
        // Stage 2C v5 reduce input: dump per-airport raw fid sets so
        // the global reduce can UNION across R4s without holding all
        // airport state in worker RAM.
        let summary_dir = h3r4_dir
            .parent()
            .ok_or_else(|| anyhow::anyhow!("h3r4_dir has no parent for airport_summary_parts"))?
            .join("airport_summary_parts")
            .join(r4_hex_str(r4));
        write_airport_summary_parts(&summary_dir, &airport_aggs)?;
    }
    Ok(R4Totals {
        rows_written,
        ground_seg_count,
        fid_zero_count,
    })
}

/// Caller pre-filtered the segment for phase + R4 + scope, so this
/// body is the hot loop.
fn accumulate_segment(
    seg: &FlightSegment,
    cache: &R4Cache,
    counters: &mut HashMap<CounterKey, CounterAcc>,
    micro_accs: &mut HashMap<(u64, u16), MicrosegAcc>,
    airport_aggs: &mut HashMap<String, AirportAggregateAcc>,
) {
    if cache.lines.is_empty() {
        return;
    }
    let intersections = project_leg_onto_airport_lines(
        seg.start_lat,
        seg.start_lon,
        seg.end_lat,
        seg.end_lon,
        &cache.lines,
        AIRPORT_LINE_SNAP_BUFFER_M,
    );
    if intersections.is_empty() {
        return;
    }
    let class_idx = if seg.veh_kind == 1 {
        seg.gse_class
    } else {
        noise_class_of(seg.profile_idx)
    };
    let is_dep = (seg.veh_kind == 0 && seg.is_departure()) as u8;
    // GA full-year-window membership for the microseg + airport
    // union splits. GSE (`veh_kind == 1`) is never GA — its `class_idx`
    // indexes the GSE class space, and GSE is airline-pass only.
    let is_ga =
        seg.veh_kind == 0 && noise_compute::emission::aircraft::is_ga_sampled_class(class_idx);

    for hit in intersections.iter() {
        let Some(&line_idx) = cache.line_index.get(&(hit.osm_id, hit.segment_idx)) else {
            continue;
        };
        let line = &cache.lines[line_idx];
        let Some(ops_kind) = ops_kind_from_aeroway(line.aeroway_type) else {
            continue;
        };
        // GSE class_idx beyond the Lw table would panic the kernel.
        if seg.veh_kind == 1
            && (class_idx as usize) >= noise_compute::emission::gse::GSE_LW_BANDS_DB.len()
        {
            continue;
        }
        // v7: aircraft stores per-metre `LW'` density-weighted by
        // (overlap / line.length_m) so the receiver-side line-source
        // formula (`+ 10·log10(θ/d_perp)` over the full microsegment
        // geometry) gives refinement-invariant Lden. GSE stores
        // absolute per-event SEL@25m — the kinematic integral already
        // bakes in `hit.length_within_segment_m`, so no density scale.
        let bands = if seg.veh_kind == 0 {
            let lw = compute_aircraft_lw_per_meter_lin(
                class_idx,
                ops_kind,
                if ops_kind == GROUND_OPS_KIND_RUNWAY_ROLL {
                    is_dep
                } else {
                    0
                },
                seg.speed_kt,
            );
            let density = (hit.length_within_segment_m as f64) / (line.length_m as f64).max(1e-9);
            let mut out = [0.0f32; NUM_BANDS];
            for i in 0..NUM_BANDS {
                out[i] = (lw[i] as f64 * density) as f32;
            }
            out
        } else {
            compute_gse_band_energy_lin(
                class_idx,
                ops_kind,
                seg.speed_kt,
                hit.length_within_segment_m,
            )
        };
        let airport_key = &cache.airport_keys[line_idx];
        let row_is_dep_value = if ops_kind == GROUND_OPS_KIND_RUNWAY_ROLL {
            is_dep
        } else {
            0
        };
        let key = CounterKey {
            airport_key: airport_key.clone(),
            osm_id: line.osm_id,
            segment_idx: line.segment_idx,
            ops_kind,
            is_departure: row_is_dep_value,
            veh_kind: seg.veh_kind,
            class_idx,
            period: seg.period,
        };
        let entry = counters.entry(key).or_insert_with(|| CounterAcc {
            start_lat: line.start_lat,
            start_lon: line.start_lon,
            end_lat: line.end_lat,
            end_lon: line.end_lon,
            length_m: line.length_m,
            ..Default::default()
        });
        entry.fid_set.insert(seg.flight_id);
        for (acc, &band) in entry.band_energy_lin.iter_mut().zip(&bands) {
            *acc += band as f64;
        }

        // Ops-kind splits gate on `veh_kind == 0`; GSE counts go to their
        // own per-class sets. The microseg + airport UNIONs are
        // class-MIXED, so each splits into a non-GA and a GA set
        // (`is_ga`) — the per-counter `entry.fid_set*` are class-pure
        // (CounterKey carries class_idx), so they need no split.
        let micro_entry = micro_accs
            .entry((line.osm_id, line.segment_idx))
            .or_default();
        if is_ga {
            micro_entry.fid_set_ga.insert(seg.flight_id);
        } else {
            micro_entry.fid_set.insert(seg.flight_id);
        }
        let airport_entry = airport_aggs.entry(airport_key.clone()).or_default();
        if seg.veh_kind == 0 {
            if ops_kind == GROUND_OPS_KIND_RUNWAY_ROLL {
                if is_dep == 1 {
                    entry.fid_set_dep.insert(seg.flight_id);
                    if is_ga {
                        micro_entry.fid_set_ga_dep.insert(seg.flight_id);
                        airport_entry.ga_dep.insert(seg.flight_id);
                    } else {
                        micro_entry.fid_set_dep.insert(seg.flight_id);
                        airport_entry.dep.insert(seg.flight_id);
                    }
                } else {
                    entry.fid_set_arr.insert(seg.flight_id);
                    if is_ga {
                        micro_entry.fid_set_ga_arr.insert(seg.flight_id);
                        airport_entry.ga_arr.insert(seg.flight_id);
                    } else {
                        micro_entry.fid_set_arr.insert(seg.flight_id);
                        airport_entry.arr.insert(seg.flight_id);
                    }
                }
            }
            // ops_per_kind: index = ops_kind - 1 (RUNWAY=1→0, TAXI=2→1,
            // APRON=3→2). VEH_KIND=0 only, split by GA window.
            let ops_idx = match ops_kind {
                GROUND_OPS_KIND_RUNWAY_ROLL => 0,
                GROUND_OPS_KIND_TAXI => 1,
                GROUND_OPS_KIND_APRON_MOVEMENT => 2,
                _ => continue,
            };
            if is_ga {
                airport_entry.ga_ops_per_kind[ops_idx].insert(seg.flight_id);
            } else {
                airport_entry.ops_per_kind[ops_idx].insert(seg.flight_id);
            }
        } else if seg.veh_kind == 1 {
            let ci = class_idx as usize;
            if ci < NUM_GSE_CLASSES {
                entry.fid_set_gse_per_class[ci].insert(seg.flight_id);
                micro_entry.fid_set_gse_per_class[ci].insert(seg.flight_id);
                airport_entry.gse_per_class[ci].insert(seg.flight_id);
            }
        }
    }
}

/// Row order within a file is HashMap iteration order —
/// non-deterministic across runs, fine because no downstream consumer
/// hashes file bytes today.
fn counters_to_rows(
    counters: HashMap<CounterKey, CounterAcc>,
    micro_accs: &HashMap<(u64, u16), MicrosegAcc>,
) -> Vec<AirportTrafficRow> {
    let mut rows = Vec::with_capacity(counters.len());
    for (key, acc) in counters {
        // band_energy_lin: raw Σ over n_days of per-event SEL at 25 m
        // perpendicular. Consumer divides via `period_leq(e, n_days_f,
        // period_seconds)` to recover Leq. Matches the "raw in extract,
        // consumer divides" convention used by airborne/cruise/summary.
        let bands_lin: [f32; NUM_BANDS] = std::array::from_fn(|i| acc.band_energy_lin[i] as f32);
        let unique_movement_count = acc.fid_set.len() as u32;
        let unique_arr_count = acc.fid_set_arr.len() as u32;
        let unique_dep_count = acc.fid_set_dep.len() as u32;
        let unique_gse_count_per_class: [u32; NUM_GSE_CLASSES] =
            std::array::from_fn(|i| acc.fid_set_gse_per_class[i].len() as u32);
        // Per-microsegment UNION row-replicated; same value for every
        // row of this (osm_id, segment_idx). v9: the three aircraft unions
        // are NON-GA only, split out from the GA-class union.
        let micro = micro_accs.get(&(key.osm_id, key.segment_idx));
        let microseg_unique_count = micro.map_or(0, |m| m.fid_set.len() as u32);
        let microseg_unique_arr_count = micro.map_or(0, |m| m.fid_set_arr.len() as u32);
        let microseg_unique_dep_count = micro.map_or(0, |m| m.fid_set_dep.len() as u32);
        let microseg_unique_gse_count_per_class: [u32; NUM_GSE_CLASSES] =
            std::array::from_fn(|i| micro.map_or(0, |m| m.fid_set_gse_per_class[i].len() as u32));
        let microseg_unique_ga_count = micro.map_or(0, |m| m.fid_set_ga.len() as u32);
        let microseg_unique_ga_arr_count = micro.map_or(0, |m| m.fid_set_ga_arr.len() as u32);
        let microseg_unique_ga_dep_count = micro.map_or(0, |m| m.fid_set_ga_dep.len() as u32);
        rows.push(AirportTrafficRow {
            airport_key: key.airport_key,
            osm_id: key.osm_id,
            segment_idx: key.segment_idx,
            geometry_kind: GEOMETRY_KIND_LINE,
            start_lat: acc.start_lat,
            start_lon: acc.start_lon,
            end_lat: acc.end_lat,
            end_lon: acc.end_lon,
            length_m: acc.length_m,
            ops_kind: key.ops_kind,
            is_departure: key.is_departure,
            veh_kind: key.veh_kind,
            class_idx: key.class_idx,
            period: key.period,
            band_energy_lin: bands_lin,
            unique_movement_count,
            unique_arr_count,
            unique_dep_count,
            unique_gse_count_per_class,
            microseg_unique_count,
            microseg_unique_arr_count,
            microseg_unique_dep_count,
            microseg_unique_gse_count_per_class,
            microseg_unique_ga_count,
            microseg_unique_ga_arr_count,
            microseg_unique_ga_dep_count,
        });
    }
    rows
}

/// Dump per-airport raw fid sets so the global reduce phase can
/// UNION across R4s without holding all airport state in worker RAM.
/// One file per R4 keyed by airport_key — the reduce reads every R4
/// part for one airport, unions the HashSets, and writes the global
/// `airport_summary.arrow`. Output dir is `<spill_root>/<r4_hex>/`.
fn write_airport_summary_parts(
    out_dir: &Path,
    airport_aggs: &HashMap<String, AirportAggregateAcc>,
) -> Result<()> {
    if airport_aggs.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(out_dir)?;
    let mut rows: Vec<AirportSummaryPartRow> = airport_aggs
        .iter()
        .map(|(airport_key, acc)| {
            let mut arr_fids: Vec<u64> = acc.arr.iter().copied().collect();
            arr_fids.sort_unstable();
            let mut dep_fids: Vec<u64> = acc.dep.iter().copied().collect();
            dep_fids.sort_unstable();
            let gse_fids_per_class: [Vec<u64>; NUM_GSE_CLASSES] = std::array::from_fn(|i| {
                let mut v: Vec<u64> = acc.gse_per_class[i].iter().copied().collect();
                v.sort_unstable();
                v
            });
            let sorted = |set: &HashSet<u64>| {
                let mut v: Vec<u64> = set.iter().copied().collect();
                v.sort_unstable();
                v
            };
            let ops_fids_per_kind: [Vec<u64>; 3] =
                std::array::from_fn(|i| sorted(&acc.ops_per_kind[i]));
            let ga_ops_fids_per_kind: [Vec<u64>; 3] =
                std::array::from_fn(|i| sorted(&acc.ga_ops_per_kind[i]));
            AirportSummaryPartRow {
                airport_key: airport_key.clone(),
                arr_fids,
                dep_fids,
                gse_fids_per_class,
                ops_fids_per_kind,
                ga_arr_fids: sorted(&acc.ga_arr),
                ga_dep_fids: sorted(&acc.ga_dep),
                ga_ops_fids_per_kind,
            }
        })
        .collect();
    // Deterministic on-disk order.
    rows.sort_by(|a, b| a.airport_key.cmp(&b.airport_key));
    crate::arrow_io::write_airport_summary_part(&out_dir.join("part.arrow"), &rows)
}

/// Per-R4 amortization cache: OSM line geometry + index +
/// pre-resolved airport_key. Building the airport_key once per
/// microsegment (instead of per-leg-intersection) cuts N_legs ×
/// N_aerodromes_global to N_microsegments × N_aerodromes_global,
/// roughly two orders of magnitude on a busy airport.
///
/// Real OSM lines come first; synthetic lines emitted by Stage 1.5
/// (`stage_airport_discover_runner.rs`) are appended after. Synthetic
/// lines carry their own `airport_key` and bypass the
/// `nearest_aerodrome_within` resolver — re-resolving them would
/// assign auto-discovered strips to whichever nearby OSM aerodrome
/// happens to be within the 6 km floor, defeating the whole point
/// of Stage 1.5. The synthetic high bit on `osm_id` keeps the
/// `(osm_id, segment_idx)` index unique across the union (real OSM
/// IDs always have bit 63 = 0).
struct R4Cache {
    lines: Vec<AirportLineSegment>,
    line_index: HashMap<(u64, u16), usize>,
    airport_keys: Vec<String>,
}

impl R4Cache {
    fn load(h3r4_dir: &Path, r4: u64, airport_areas: &[AirportArea]) -> Self {
        let r4_dir = h3r4_dir.join(r4_hex_str(r4));
        let real_lines = load_airport_lines_for_r4(&r4_dir);
        let synth_lines = load_synth_airport_lines_for_r4(&r4_dir);

        let total = real_lines.len() + synth_lines.len();
        let mut lines: Vec<AirportLineSegment> = Vec::with_capacity(total);
        let mut line_index: HashMap<(u64, u16), usize> = HashMap::with_capacity(total);
        let mut airport_keys: Vec<String> = Vec::with_capacity(total);

        for line in real_lines {
            debug_assert!(
                !is_synthetic_osm_id(line.osm_id),
                "real OSM osm_id must never set the synthetic high bit",
            );
            let key = resolve_airport_key(&line, airport_areas);
            let prev = line_index.insert((line.osm_id, line.segment_idx), lines.len());
            debug_assert!(
                prev.is_none(),
                "real airport_lines.arrow contains duplicate (osm_id, segment_idx) — \
                 osm-extract microsegment uniqueness invariant broken",
            );
            airport_keys.push(key);
            lines.push(line);
        }
        for (line, key) in synth_lines {
            debug_assert!(
                is_synthetic_osm_id(line.osm_id),
                "synth osm_id must set the synthetic high bit (Stage 1.5 encoding)",
            );
            // No `resolve_airport_key` here — the synth row's key is
            // either the content-addressed `auto-<H3-R11>` for a newly
            // discovered airfield, or the real `airport_key` from
            // Stage 1.5's re-attribution path. Either way the answer
            // already arrived with the row.
            let prev = line_index.insert((line.osm_id, line.segment_idx), lines.len());
            debug_assert!(
                prev.is_none(),
                "synth line collided with a prior (real or synth) (osm_id, segment_idx) \
                 — the synthetic high bit + per-row uniqueness invariants are broken",
            );
            airport_keys.push(key);
            lines.push(line);
        }
        Self {
            lines,
            line_index,
            airport_keys,
        }
    }
}

fn load_airport_lines_for_r4(r4_dir: &Path) -> Vec<AirportLineSegment> {
    let path = r4_dir.join("airport_lines.arrow");
    match read_airport_lines(&path) {
        Ok(rows) => rows
            .into_iter()
            .map(|r: AirportLineRow| AirportLineSegment {
                osm_id: r.osm_id,
                segment_idx: r.segment_idx,
                start_lat: r.start_lat,
                start_lon: r.start_lon,
                end_lat: r.end_lat,
                end_lon: r.end_lon,
                length_m: r.length_m,
                aeroway_type: r.aeroway_type,
            })
            .collect(),
        Err(e) => {
            // `read_airport_lines` already converts a missing file to
            // `Ok(Vec::new())`, so any `Err` here is a genuine read
            // failure (corrupt arrow, permission denied) — load loud
            // for parity with the synth-side loader below.
            eprintln!(
                "{} [stage2c] airport_lines.arrow read failed at {}: {e}",
                ts(),
                path.display()
            );
            Vec::new()
        }
    }
}

/// Load Stage 1.5's synthetic per-R4 lines paired with their
/// pre-resolved `airport_key`. Returns `(segment, key)` so the
/// caller can drop them straight into `R4Cache.lines` /
/// `R4Cache.airport_keys` without re-resolving against
/// `airport_areas`. Missing file or read failure → empty.
fn load_synth_airport_lines_for_r4(r4_dir: &Path) -> Vec<(AirportLineSegment, String)> {
    let path = r4_dir.join(SYNTH_LINES_FILE);
    match read_synth_airport_lines(&path) {
        Ok(rows) => rows
            .into_iter()
            .map(|r| {
                let seg = AirportLineSegment {
                    osm_id: r.osm_id,
                    segment_idx: r.segment_idx,
                    start_lat: r.start_lat as f32,
                    start_lon: r.start_lon as f32,
                    end_lat: r.end_lat as f32,
                    end_lon: r.end_lon as f32,
                    length_m: r.length_m,
                    aeroway_type: r.aeroway_type,
                };
                (seg, r.airport_key)
            })
            .collect(),
        Err(e) => {
            eprintln!(
                "{} [stage2c] {SYNTH_LINES_FILE} read failed at {}: {e}",
                ts(),
                path.display()
            );
            Vec::new()
        }
    }
}

fn resolve_airport_key(line: &AirportLineSegment, airport_areas: &[AirportArea]) -> String {
    let (mid_lat, mid_lon) = midpoint(line.start_lat, line.start_lon, line.end_lat, line.end_lon);
    match nearest_aerodrome_within(mid_lat as f64, mid_lon as f64, airport_areas) {
        Some(area) if !area.airport_key.is_empty() => area.airport_key.clone(),
        _ => match cell_u64(mid_lat as f64, mid_lon as f64, Resolution::Seven) {
            Some(r7) => format!("strip:{r7:015x}"),
            None => "strip:unknown".to_string(),
        },
    }
}

#[cfg(test)]
pub(crate) mod tests;
