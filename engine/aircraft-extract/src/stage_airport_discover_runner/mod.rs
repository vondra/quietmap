//! Observed aircraft data processing on the canonical square grid.

mod geometry;
use geometry::*;

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
    synth_airport_key_for, synth_display_name, synth_osm_id_for, write_synth_airport_areas,
    write_synth_airport_lines, SynthAirportAreaRow, SynthAirportLineRow, AIRSTRIP_AEROWAY_TYPE,
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

/// Reject clusters whose vertex_count over the extraction window
/// implies > ~700 ground visits/day. The busiest microsegments at
/// LKPR see ~50-100 movements/day; a count an order of magnitude
/// higher means the cluster captured non-ground vertices (typically
/// dense ATC waypoints on the STAR/SID corridor).
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

/// Drive Stage 1.5 over the Stage 1 multi-day segment set. Returns
/// the total number of z9s that received at least one synthetic /
/// re-attributed line row.
///
/// `airport_areas_global` is the union of every z9's
/// `airport_areas.arrow` (the same global set Stage 2C consumes for
/// `nearest_aerodrome_within`). The runner uses it for both the
/// re-attribution check (cluster inside a real aerodrome → use its
/// key) and the implicit "new airfield" path (no nearby real area).
///
/// `airport_lines_global` is the union of every z9's
/// `airport_lines.arrow`. Inside an aerodrome's polygon buffer the
/// runner cross-checks each cluster against this set: clusters that
/// sit on or right next to a real OSM aeroway line are folded into
/// the airport's key; clusters that pass the polygon test but sit
/// far from every real line are rejected as DBSCAN false positives.
pub fn run_stage_airport_discover(
    segments_by_square_dir: &Path,
    aerodrome_index: &AerodromeIndex,
    airport_lines_global: &[AirportLineRow],
    prepared_year_dir: &Path,
    scope: Option<&ScopeBbox>,
) -> Result<usize> {
    let active: BTreeMap<u64, std::path::PathBuf> =
        crate::shuffle::list_square_shards(segments_by_square_dir, "ground.arrow", scope)?
            .into_iter()
            .collect();
    // Union with z9s holding stale synth sidecars on disk so a
    // previously-populated z9 with no ground signal this run gets its
    // sidecars cleared (run_one_square with empty segments writes empty
    // arrows); otherwise Stage 2C reads zombie airport areas.
    let stale = stale_synth_sidecar_squares(prepared_year_dir, scope, &active)?;
    if active.is_empty() && stale.is_empty() {
        return Ok(0);
    }
    // Validate all source shards before replacing any generation's sidecars.
    for shard in active.values() {
        crate::arrow_io::for_each_segment_batch(shard, |_| Ok(()))
            .with_context(|| format!("validate {}", shard.display()))?;
    }
    let mut square_keys: Vec<u64> = active.keys().copied().collect();
    square_keys.extend(stale);
    square_keys.sort_unstable();
    started(
        "stage1.5",
        &format!(
            "{} z9 cells ({} active, {} stale-only)",
            square_keys.len(),
            active.len(),
            square_keys.len() - active.len()
        ),
    );
    let stage_start = std::time::Instant::now();
    let square_counter = Milestone::new("stage1.5", "z9 cells", 50);
    // Per-z9 detail log only for cells whose body takes longer than
    // this — keeps the log readable when 95% of cells finish in
    // milliseconds (polygon gate covers them), surfaces the long-tail
    // hub z9s without further configuration.
    const PER_SQUARE_SLOW_LOG_THRESHOLD: std::time::Duration = std::time::Duration::from_secs(5);

    let results: Vec<Result<bool>> = square_keys
        .par_iter()
        .map(|square| {
            let square_start = std::time::Instant::now();
            let segments = match active.get(square) {
                Some(shard) => crate::arrow_io::read_segments(shard)
                    .with_context(|| format!("read {}", shard.display()))?,
                None => Vec::new(),
            };
            let n_segs = segments.len();
            let out = run_one_square(
                *square,
                &segments,
                aerodrome_index,
                airport_lines_global,
                prepared_year_dir,
            )
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
        .collect();

    let mut populated = 0usize;
    for r in results {
        if r? {
            populated += 1;
        }
    }
    finished(
        "stage1.5",
        &format!(
            "{populated} of {} z9s populated with synth airport_lines in {:?}",
            square_keys.len(),
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
    segments: &[FlightSegment],
    aerodrome_index: &AerodromeIndex,
    airport_lines_global: &[AirportLineRow],
    prepared_year_dir: &Path,
) -> Result<bool> {
    let square_dir = prepared_year_dir.join(square_path(square));
    let lines = nearby_airport_lines(square, segments, airport_lines_global);

    let candidates = collect_miss_snap_vertices(segments, &lines, aerodrome_index);
    let strips = if candidates.len() >= DBSCAN_MIN_SAMPLES {
        discover_strips(&candidates, DBSCAN_EPS_M, DBSCAN_MIN_SAMPLES)
    } else {
        Vec::new()
    };

    let mut line_rows = Vec::new();
    let mut area_rows = Vec::new();
    for strip in &strips {
        match classify_cluster(strip, aerodrome_index, airport_lines_global) {
            ClusterDisposition::Reject => continue,
            ClusterDisposition::Reattribute(real_area) => {
                emit_lines_for_strip(strip, real_area.airport_key.clone(), &mut line_rows);
                // Do NOT emit a synth area — the real aerodrome
                // polygon already exists in `airport_areas.arrow`.
            }
            ClusterDisposition::SynthAirport => {
                let centroid_lat = strip.center_lat as f64;
                let centroid_lon = strip.center_lon as f64;
                let key = synth_airport_key_for(centroid_lat, centroid_lon);
                emit_lines_for_strip(strip, key.clone(), &mut line_rows);
                area_rows.push(SynthAirportAreaRow {
                    osm_id: synth_osm_id_for(centroid_lat, centroid_lon),
                    airport_key: key,
                    name: synth_display_name(
                        centroid_lat,
                        centroid_lon,
                        strip.length_m,
                        strip.vertex_count,
                    ),
                    aeroway_type: SYNTH_AERODROME_AEROWAY_TYPE,
                    centroid_lat,
                    centroid_lon,
                    area_m2: strip.length_m * strip.width_m,
                });
            }
        }
    }

    // Idempotency: always rewrite even when both vecs are empty.
    // `write_synth_airport_*` truncate-and-replaces via
    // `arrow_io::write_record_batches`, so a previously-populated z9
    // that this run finds nothing in is cleared on disk.
    write_synth_airport_lines(&square_dir.join(SYNTH_LINES_FILE), &line_rows)?;
    write_synth_airport_areas(&square_dir.join(SYNTH_AREAS_FILE), &area_rows)?;

    Ok(!line_rows.is_empty())
}

/// In-scope z9 subdirs holding a synth sidecar on disk but absent
/// from `already_known` (the current-run ground-segment set). Used to
/// reach z9s a prior run discovered but that have no segments today —
/// without this scan, their stale sidecars feed Stage 2C zombie data.
fn stale_synth_sidecar_squares(
    prepared_year_dir: &Path,
    scope: Option<&ScopeBbox>,
    already_known: &BTreeMap<u64, std::path::PathBuf>,
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
    segments: &[FlightSegment],
    lines: &[AirportLineRow],
) -> Vec<AirportLineSegment> {
    let mut extent = Extent::empty(square);
    for segment in segments {
        extent.include(segment.start_lat, segment.start_lon);
        extent.include(segment.end_lat, segment.end_lon);
    }
    let extent = extent.padded(AIRPORT_LINE_SNAP_BUFFER_M);
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

/// One ground segment becomes 0 or 2 miss-snap vertices, depending on
/// two gates applied in order:
///
/// 1. **Known-aerodrome polygon gate** — drop the whole segment when
///    EITHER endpoint sits inside any OSM aerodrome's centroid-radius
///    window. Stage 1.5 is for OSM-MISSING airfields, so a leg with
///    one foot at a known aerodrome can never seed an unmapped strip:
///    the exterior endpoint is either a takeoff climb / final-approach
///    point (en-route ADS-B noise) or a cross-country waypoint, not a
///    strip vertex. Skipping the segment entirely also skips the
///    expensive line-snap kernel below — at hub z9s this is the
///    decisive win because takeoff / landing transitions
///    (one foot at the aerodrome, the other 6-10 km out) make up the
///    bulk of ground-tagged segments and they all flowed through the
///    O(M_lines) kernel under the BOTH-inside rule.
/// 2. **OSM aeroway line gate** — for segments fully outside every
///    aerodrome polygon, drop the segment when the leg projects onto
///    any local OSM aeroway microsegment within
///    [`AIRPORT_LINE_SNAP_BUFFER_M`]. Catches isolated taxi /
///    runway segments at airports whose polygon coverage in OSM is
///    incomplete (only `aeroway=runway` line, no `aerodrome`
///    polygon — common for small fields).
///
/// Both endpoints flow into DBSCAN as cluster seeds. Mid-leg emission
/// is intentionally not used; cluster geometry should capture the
/// actual ADS-B trajectory shape, and the leg-pair is what shapes
/// the synth runway centreline downstream.
fn collect_miss_snap_vertices(
    segments: &[FlightSegment],
    lines: &[AirportLineSegment],
    index: &AerodromeIndex,
) -> Vec<(f32, f32)> {
    let mut out = Vec::with_capacity(segments.len() * 2);
    for seg in segments {
        // Stage 1.5 only clusters aircraft ground vertices — GSE
        // service-road clusters can look line-like but don't represent
        // runway activity. Shuffle merged both veh_kinds into
        // ground.arrow; filter here.
        if seg.phase != Phase::Ground || seg.veh_kind != 0 {
            continue;
        }
        // Stage 1's classifier shouldn't produce non-finite endpoints,
        // but a downstream rayon panic would be harder to diagnose
        // than a silent skip.
        if !seg.start_lat.is_finite()
            || !seg.start_lon.is_finite()
            || !seg.end_lat.is_finite()
            || !seg.end_lon.is_finite()
        {
            continue;
        }
        // Polygon gate — bool check, no per-line allocation. EITHER
        // endpoint inside any known aerodrome → drop the whole leg
        // (see docstring for the takeoff/landing transition rationale).
        if index.contains(seg.start_lat as f64, seg.start_lon as f64)
            || index.contains(seg.end_lat as f64, seg.end_lon as f64)
        {
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
    out
}

fn classify_cluster<'a>(
    strip: &DiscoveredStrip,
    aerodrome_index: &'a AerodromeIndex<'_>,
    airport_lines_global: &[AirportLineRow],
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
    // `nearest_aerodrome_within` admits name-only entries (key may
    // still be empty if the OSM extract carried a `name=` tag with
    // no ICAO `ref=`). Re-attribution requires a non-empty key
    // because that's what flows into airport_traffic.arrow rows.
    let nearby_aerodrome = aerodrome_index
        .nearest(strip.center_lat as f64, strip.center_lon as f64)
        .filter(|a| !a.airport_key.is_empty());
    // Inside a real aerodrome's polygon buffer the cluster must ALSO
    // sit within `REAL_LINE_NEAR_BUFFER_M` of at least one real OSM
    // aeroway microsegment. Otherwise it's a DBSCAN false positive
    // (ADS-B vertices from cars on access roads, GSE in parking
    // lots) that previously slipped through and got mis-labeled with
    // the airport's key. Far-from-any-aerodrome clusters still flow
    // through the existing `auto-<z20>` path — the line gate is
    // intentionally gated inside the polygon arm so genuinely-new
    // airfields with zero OSM coverage still get auto-discovered.
    // Three-way disposition driven by observability: never silently
    // drop a cluster, only relabel. A cluster in an aerodrome's
    // polygon buffer AND near a real OSM line → fold into that
    // airport. A cluster in the buffer but FAR from any real line
    // (typical false-positive: ADS-B noise from access-road cars,
    // approach-corridor mis-classifications) → relabel as a
    // synthetic `auto-<z20>` airfield so it stays visible in the
    // popup with full provenance, not erased. A cluster far from
    // every aerodrome → auto-* as before.
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
/// Linear `.any()` scan with short-circuit. For CZ scope `airport_
/// lines_global` is ~10-15 k microsegs; global OSM aeroway coverage
/// is in the low millions. Worst-case per cluster ~10 µs (CZ) to a
/// few ms (global); ~dozens of clusters per z9 makes this a single-
/// digit ms contribution to Stage 1.5 versus the DBSCAN itself.
fn cluster_near_real_aeroway_line(
    cluster_lat: f64,
    cluster_lon: f64,
    airport_lines_global: &[AirportLineRow],
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
