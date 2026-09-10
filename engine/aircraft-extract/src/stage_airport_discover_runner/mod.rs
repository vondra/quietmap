//! Observed aircraft data processing on the canonical square grid.

mod admission;
mod geometry;
mod planning;
use geometry::*;
pub use planning::{plan_ground_discovery, GroundDiscoveryInput};

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use noise_compute::types::AirportArea;
use rayon::prelude::*;

use crate::airport_index::AerodromeIndex;
use crate::airport_io::AirportLineRow;
use crate::extent::Extent;
use crate::flight::{FlightSegment, Phase};
use crate::geo::{square_path, M_PER_DEG_LAT, M_PER_DEG_LON_EQUATOR};
use crate::progress::{finished, started, Milestone};
use crate::scope::ScopeBbox;
use crate::stage_2c::airport_traffic::{
    project_leg_onto_airport_lines, AirportLineSegment, AIRPORT_LINE_SNAP_BUFFER_M,
};
use crate::stage_airport_discover::{discover_strips, DiscoveredStrip};
use crate::synth_airport_io::{
    synth_airport_key_for, synth_osm_id_for, write_synth_airport_areas, write_synth_airport_lines,
    SynthAirportAreaRow, SynthAirportLineRow, AIRSTRIP_AEROWAY_TYPE, DISCOVERED_AIRSTRIP_NAME,
    SYNTH_AERODROME_AEROWAY_TYPE, SYNTH_AREAS_FILE, SYNTH_LINES_FILE,
};

/// DBSCAN cluster radius. 200 m bridges adjacent ADS-B fixes along
/// a typical 1–3 km airstrip without merging two physically distinct
/// strips (real airport runways are ~1.5–4 km long, two-runway
/// airports have ≥500 m between them).
const DBSCAN_EPS_M: f32 = 200.0;

/// DBSCAN min cluster size. 5 vertices over the extraction window
/// admits strips with as few as 1-2 flights/14 days (each rotation
/// contributes ~4-10 ground vertices). Sized low so popup
/// observability surfaces low-confidence strips for triage rather
/// than silently dropping them — line-shape (in `classify_cluster`)
/// plus the `CLUSTER_MAX_*` caps below are the only accept/reject
/// gates.
const DBSCAN_MIN_SAMPLES: usize = 5;

/// Reject clusters longer than this even if `is_line=true`. Real
/// runways top out around 4000 m (Doha 4850 m, Madrid 4350 m are
/// the longest commercial runways in service); a synth line longer
/// than that is almost certainly an approach corridor mis-merged
/// across multiple aircraft trajectories.
const CLUSTER_MAX_LENGTH_M: f32 = 4000.0;

/// Reject unusually dense candidate clusters that may be misclassified approach
/// corridors. This counts candidate vertices, not distinct flights or visits.
const CLUSTER_MAX_VERTICES: u32 = 20_000;

const REAL_LINE_NEAR_BUFFER_M: f64 = 300.0;

/// Microsegment length cap for the emitted synthetic runway. Matches
/// the real `airport_lines.arrow` writer (osm-extract `main.rs:269,
/// max_len = 250.0`) and the road/rail microsegment cap. The Stage 2C
/// projection buffer (50 m perpendicular) is spatial-only, so segment
/// LENGTH doesn't affect its snap correctness; matching road/rail
/// keeps per-microsegment compute scaling uniform across all layers.
const SYNTH_MICROSEGMENT_M: f32 = 250.0;

/// Disposition of a cluster after the acceptance + identity pass.
enum ClusterDisposition<'a> {
    Reject,
    /// Cluster centroid sits inside a real aerodrome's snap window.
    /// Emit synth lines under the real airport's key so Stage 2C
    /// unifies them with the (incomplete) real OSM lines.
    Reattribute(&'a AirportArea),
    SynthAirport,
}

/// Validate ground shards, discover unmapped strips and replace both synthetic sidecars.
pub fn run_stage_airport_discover(
    segments_by_square_dir: &Path,
    aerodrome_index: &AerodromeIndex,
    airport_lines_global: &[AirportLineRow],
    prepared_year_dir: &Path,
    scope: Option<&ScopeBbox>,
) -> Result<usize> {
    let inputs = plan_ground_discovery(
        segments_by_square_dir,
        aerodrome_index,
        airport_lines_global,
        scope,
    )?;
    // Current shards and stale sidecars share one rewrite pass, including empty results.
    let stale = stale_synth_sidecar_squares(prepared_year_dir, scope, &inputs)?;
    if inputs.is_empty() && stale.is_empty() {
        return Ok(0);
    }
    let largest_allocation = match inputs.values().map(|input| input.allocation_bytes).max() {
        Some(bytes) => bytes,
        None => admission::working_set_allowance(
            0,
            0,
            0,
            admission::shared_allocation_allowance(aerodrome_index, airport_lines_global)?,
        )?,
    };
    let workers =
        crate::memory::max_concurrent_tasks(rayon::current_num_threads(), largest_allocation)?;
    let raw = prepared_year_dir.join(".ground_discovery_raw");
    let canonical = prepared_year_dir.join(".ground_discovery_canonical");
    anyhow::ensure!(
        !raw.exists() && !canonical.exists(),
        "incomplete discovery generation requires inspection before rerun"
    );
    crate::arrow_io::create_directory_all_synced(&raw)?;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .build()?;
    let mut square_keys: Vec<u64> = inputs.keys().copied().collect();
    square_keys.extend(stale);
    square_keys.sort_unstable();
    started(
        "stage1.5",
        &format!(
            "{} z9 cells ({} active, {} stale-only); {workers} workers; {largest_allocation} B allocation allowance each",
            square_keys.len(),
            inputs.len(),
            square_keys.len() - inputs.len()
        ),
    );
    let stage_start = std::time::Instant::now();
    let square_counter = Milestone::new("stage1.5", "z9 cells", 50);
    // Per-z9 detail log only for cells whose body takes longer than
    // this — keeps the log readable when 95% of cells finish in
    // milliseconds (polygon gate covers them), surfaces the long-tail
    // hub z9s without further configuration.
    const PER_SQUARE_SLOW_LOG_THRESHOLD: std::time::Duration = std::time::Duration::from_secs(5);

    let results: Vec<Result<bool>> = pool.install(|| {
        square_keys
            .par_iter()
            .map(|square| {
                let square_start = std::time::Instant::now();
                let (extent, n_segs, candidate_bound) = inputs
                    .get(square)
                    .map(|input| {
                        (
                            input.extent,
                            input.rows,
                            input.candidate_vertices_upper_bound,
                        )
                    })
                    .unwrap_or((Extent::empty(*square), 0, 0));
                let lines = nearby_airport_lines(*square, extent, airport_lines_global);
                let mut candidates = Vec::with_capacity(candidate_bound);
                if let Some(input) = inputs.get(square) {
                    let shard = &input.path;
                    crate::arrow_io::for_each_segment_batch(shard, |segments| {
                        collect_miss_snap_vertices(
                            &segments,
                            &lines,
                            aerodrome_index,
                            &mut candidates,
                        );
                        anyhow::ensure!(
                            candidates.len() <= candidate_bound,
                            "ground candidates changed after validation"
                        );
                        Ok(())
                    })
                    .with_context(|| format!("read {}", shard.display()))?;
                }
                let out = run_one_square(*square, &candidates, aerodrome_index, &lines, &raw)
                    .with_context(|| format!("z9 {square:015x}"))?;
                let elapsed = square_start.elapsed();
                if elapsed >= PER_SQUARE_SLOW_LOG_THRESHOLD {
                    eprintln!(
                        "{} [stage1.5] z9 {} done in {:?} ({} ground segs, populated={})",
                        crate::progress::ts(),
                        square_path(*square),
                        elapsed,
                        n_segs,
                        out,
                    );
                }
                square_counter.add(1);
                Ok(out)
            })
            .collect()
    });

    for result in results {
        result?;
    }
    let square_count = square_keys.len();
    drop(inputs);
    drop(square_keys);
    drop(pool);
    let retained_bytes =
        admission::shared_allocation_allowance(aerodrome_index, airport_lines_global)?;
    let populated = crate::ground_discovery_finalize::finalize_ground_discovery(
        &raw,
        &canonical,
        scope,
        retained_bytes,
    )?;
    crate::ground_discovery_finalize::promote_ground_discovery(&canonical, prepared_year_dir)?;
    std::fs::remove_dir_all(&raw)?;
    std::fs::remove_dir_all(&canonical)?;
    std::fs::File::open(prepared_year_dir)?.sync_all()?;
    finished(
        "stage1.5",
        &format!(
            "{populated} canonical z9s populated from {square_count} source z9s in {:?}",
            stage_start.elapsed()
        ),
    );
    Ok(populated)
}

/// Process one z9: build candidate set, cluster, classify, emit. Always
/// rewrites both sidecars (even empty) so a previously-populated z9
/// that this run finds nothing in is monotonically cleared.
fn run_one_square(
    square: u64,
    candidates: &[(f32, f32)],
    aerodrome_index: &AerodromeIndex,
    lines: &[AirportLineSegment],
    prepared_year_dir: &Path,
) -> Result<bool> {
    let square_dir = prepared_year_dir.join(square_path(square));
    let strips = if candidates.len() >= DBSCAN_MIN_SAMPLES {
        discover_strips(candidates, DBSCAN_EPS_M, DBSCAN_MIN_SAMPLES)
    } else {
        Vec::new()
    };

    let classified: Vec<_> = strips
        .into_iter()
        .filter_map(
            |strip| match classify_cluster(&strip, aerodrome_index, lines) {
                ClusterDisposition::Reject => None,
                ClusterDisposition::Reattribute(area) => Some((strip, Some(area))),
                ClusterDisposition::SynthAirport => Some((strip, None)),
            },
        )
        .collect();
    let line_rows = classified.iter().flat_map(|(strip, area)| {
        let key = area.map_or_else(
            || synth_airport_key_for(f64::from(strip.center_lat), f64::from(strip.center_lon)),
            |area| area.airport_key.clone(),
        );
        let mut rows = Vec::new();
        emit_lines_for_strip(strip, key, &mut rows);
        rows
    });
    write_synth_airport_lines(&square_dir.join(SYNTH_LINES_FILE), line_rows)?;
    let area_rows = classified
        .iter()
        .filter(|(_, area)| area.is_none())
        .map(|(strip, _)| {
            let centroid_lat = f64::from(strip.center_lat);
            let centroid_lon = f64::from(strip.center_lon);
            SynthAirportAreaRow {
                osm_id: synth_osm_id_for(centroid_lat, centroid_lon),
                airport_key: synth_airport_key_for(centroid_lat, centroid_lon),
                name: DISCOVERED_AIRSTRIP_NAME.into(),
                aeroway_type: SYNTH_AERODROME_AEROWAY_TYPE,
                centroid_lat,
                centroid_lon,
                area_m2: strip.length_m * strip.width_m,
            }
        });
    // Empty streams also atomically replace stale sidecars.
    write_synth_airport_areas(&square_dir.join(SYNTH_AREAS_FILE), area_rows)?;
    Ok(!classified.is_empty())
}

/// In-scope z9 subdirs holding a synth sidecar on disk but absent
/// from `already_known` (the current-run ground-segment set). Used to
/// reach z9s a prior run discovered but that have no segments today —
/// without this scan, their stale sidecars feed Stage 2C zombie data.
fn stale_synth_sidecar_squares(
    prepared_year_dir: &Path,
    scope: Option<&ScopeBbox>,
    already_known: &BTreeMap<u64, GroundDiscoveryInput>,
) -> Result<Vec<u64>> {
    let mut out = Vec::new();
    for (id, path) in crate::spatial::square_directories(prepared_year_dir)? {
        if already_known.contains_key(&id) || scope.is_some_and(|scope| !scope.contains_square(id))
        {
            continue;
        }
        if path.join(SYNTH_LINES_FILE).exists() || path.join(SYNTH_AREAS_FILE).exists() {
            out.push(id);
        }
    }
    Ok(out)
}

fn nearby_airport_lines(
    square: u64,
    extent: Extent,
    lines: &[AirportLineRow],
) -> Vec<AirportLineSegment> {
    // Classification uses a wider radius than the unchanged 50 m snap gate.
    let extent = extent.padded(REAL_LINE_NEAR_BUFFER_M as f32);
    lines
        .iter()
        .filter_map(|line| {
            let mut bounds = Extent::empty(square);
            bounds.include(line.start_lat, line.start_lon);
            bounds.include(line.end_lat, line.end_lon);
            extent.intersects(bounds).then_some(AirportLineSegment {
                osm_id: line.osm_id,
                segment_idx: line.segment_idx,
                grid: line.grid,
                start_lat: line.start_lat,
                start_lon: line.start_lon,
                end_lat: line.end_lat,
                end_lon: line.end_lon,
                length_m: line.length_m,
                aeroway_type: line.aeroway_type,
            })
        })
        .collect()
}

fn outside_known_aerodromes(seg: &FlightSegment, index: &AerodromeIndex) -> bool {
    seg.phase == Phase::Ground
        && seg.veh_kind == 0
        && seg.start_lat.is_finite()
        && seg.start_lon.is_finite()
        && seg.end_lat.is_finite()
        && seg.end_lon.is_finite()
        && !index.contains(f64::from(seg.start_lat), f64::from(seg.start_lon))
        && !index.contains(f64::from(seg.end_lat), f64::from(seg.end_lon))
}

/// Keep endpoints in source order only when both lie outside known aerodromes
/// and the leg misses every local line's unchanged 50 m snap corridor. A leg
/// with one endpoint at a known airport cannot seed an unmapped airfield.
fn collect_miss_snap_vertices(
    segments: &[FlightSegment],
    lines: &[AirportLineSegment],
    index: &AerodromeIndex,
    out: &mut Vec<(f32, f32)>,
) {
    for seg in segments {
        if !outside_known_aerodromes(seg, index) {
            continue;
        }
        // Line gate — only fully-exterior legs reach here, so the
        // O(M_lines) cost is bounded by genuine ambiguous-airport
        // segments (small fields with line but no polygon).
        let intersections = project_leg_onto_airport_lines(
            seg.start_lat,
            seg.start_lon,
            seg.end_lat,
            seg.end_lon,
            lines,
            AIRPORT_LINE_SNAP_BUFFER_M,
        );
        if !intersections.is_empty() {
            continue;
        }
        out.push((seg.start_lat, seg.start_lon));
        out.push((seg.end_lat, seg.end_lon));
    }
}

fn classify_cluster<'a>(
    strip: &DiscoveredStrip,
    aerodrome_index: &'a AerodromeIndex<'_>,
    airport_lines_global: &[AirportLineSegment],
) -> ClusterDisposition<'a> {
    if !strip.is_line {
        // Commits 1-4 ship line clusters only — apron-equivalent
        // blobs need a `geometry_kind` extension to Stage 2C that's
        // out of scope (see plan "Out of scope").
        return ClusterDisposition::Reject;
    }
    // Length / vertex caps for approach-corridor ghost clusters.
    // Even with the AGL filter on input vertices, occasional
    // mis-classified samples sneak through and DBSCAN's eps=200m
    // can bridge them into multi-km lines. Reject those rather
    // than emit garbage geometry that draws across residential
    // areas miles from any actual runway.
    if strip.length_m > CLUSTER_MAX_LENGTH_M {
        return ClusterDisposition::Reject;
    }
    if strip.vertex_count > CLUSTER_MAX_VERTICES {
        return ClusterDisposition::Reject;
    }
    // A name-only aerodrome remains eligible for proximity, but cannot supply
    // a traffic key. Distant-from-line clusters retain synthetic identity.
    let nearby_aerodrome = aerodrome_index
        .nearest(strip.center_lat as f64, strip.center_lon as f64)
        .filter(|a| !a.airport_key.is_empty());
    match nearby_aerodrome {
        Some(area)
            if cluster_near_real_aeroway_line(
                strip.center_lat as f64,
                strip.center_lon as f64,
                airport_lines_global,
            ) =>
        {
            ClusterDisposition::Reattribute(area)
        }
        Some(_) | None => ClusterDisposition::SynthAirport,
    }
}

/// True iff some microsegment in `airport_lines_global` is within
/// `REAL_LINE_NEAR_BUFFER_M` PERPENDICULAR distance of
/// `(cluster_lat, cluster_lon)`. Uses
/// [`noise_compute::propagation::geo::point_to_segment`] (the same
/// kernel road / rail / Stage 2C projection use) so a long microseg
/// whose midpoint is >300 m from the cluster but whose body passes
/// within 300 m still counts as "near".
///
fn cluster_near_real_aeroway_line(
    cluster_lat: f64,
    cluster_lon: f64,
    airport_lines_global: &[AirportLineSegment],
) -> bool {
    use noise_compute::propagation::geo::point_to_segment;
    airport_lines_global.iter().any(|line| {
        let (dist_m, _, _, _) = point_to_segment(
            cluster_lat,
            cluster_lon,
            line.start_lat as f64,
            line.start_lon as f64,
            line.end_lat as f64,
            line.end_lon as f64,
        );
        dist_m < REAL_LINE_NEAR_BUFFER_M
    })
}

#[cfg(test)]
mod tests;
