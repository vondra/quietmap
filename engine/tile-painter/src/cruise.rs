//! Cruise scatter onto a Web Mercator tile.
//!
//! Mirrors the per-bucket Doc 29 kernel from
//! `noise_compute::compute::aircraft_v6::cruise::scatter`, but writes
//! into a [`TileAccumulator`] instead of the popup `FlightAccum` table.
//! The popup-only state (`cruise_flight_stats`, `top_flight_candidates`)
//! is dropped — heatmap energy is commutative, no per-flight dedup
//! needed.
//!
//! Bounding-cylinder prefilter per Decision #7: skip the bucket entirely
//! when its R7 centre lies further than `AIRCRAFT_MAX_HORIZONTAL_REACH_M`
//! plus half the segment length from the tile's centre extended by half the
//! tile diagonal (synth endpoints lie half a length from the centre —
//! `cruise_synth_offsets`). Same math as the popup, just applied at
//! tile granularity.

use std::sync::atomic::{AtomicUsize, Ordering};

use h3o::CellIndex;
use noise_compute::compute::aircraft_v6::cruise::cruise_synth_offsets;
use noise_compute::compute::aircraft_v6::CruiseRowView;
use noise_compute::emission::aircraft::{
    self, NpdLuts, SegmentTerrain, AIRCRAFT_FAR_FIELD_THRESHOLD_M, AIRCRAFT_MAX_HORIZONTAL_REACH_M,
    GROUND_CONTEXT_NONE, GROUND_OPS_KIND_NONE,
};
use noise_compute::flight_id::pack_synth;
use noise_compute::types::{AircraftSegment, RasterSampler};
use raster_reader::fused_tile_z13::{tile_pixel_size_m, FusedTileZ13};
use rayon::prelude::*;

use crate::accumulator::{CoarseLattice, TileAccumulator};
use crate::grid::TILE_PX;

/// Far-field broadcast lattice node spacing, in metres. The far path
/// (segments clearing every receiver by `AIRCRAFT_FAR_FIELD_THRESHOLD_M`)
/// is a smooth field sampled on a coarse lattice and bilinearly expanded;
/// the per-node bilinear error is `≈ 8.69·(spacing/2)²/slant²` dB, which at
/// the ≥7.6 km cruise slant is < 0.1 dB for this spacing. Sizing the lattice
/// by absolute spacing (not a fixed node count) keeps that error invariant
/// across compute zooms AND across the 512 shift: a z12 base tile (~6.2 km)
/// yields 5×5 (the historical 3×3 belonged to the 3.1 km z13@256 tile), the
/// z9 coarse-field tile (~50 km) ≈ 33×33 — same node density, same accuracy,
/// computed once per z9 extent instead of re-derived per base-zoom child
/// tile (`region_runner` cruise coarse-field pass).
const CRUISE_FAR_NODE_SPACING_M: f64 = 1570.0;

/// Far-lattice node count for a tile spanning `tile_span_m` metres, sized so
/// node spacing stays ≈ [`CRUISE_FAR_NODE_SPACING_M`]. Clamped to ≥3 (the
/// minimum bilinear lattice with a centre node).
fn far_lattice_n(tile_span_m: f64) -> usize {
    ((tile_span_m / CRUISE_FAR_NODE_SPACING_M).round() as usize + 1).max(3)
}

/// Per-tile statistics surfaced to perf log + tests.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScatterStats {
    pub buckets_seen: usize,
    pub buckets_in_reach: usize,
    pub buckets_terrain_rejected: usize,
    pub buckets_broadcast: usize,
    pub pairs_evaluated: u64,
    pub pairs_below_threshold: u64,
}

impl ScatterStats {
    pub fn merge(&mut self, o: &ScatterStats) {
        self.buckets_seen += o.buckets_seen;
        self.buckets_in_reach += o.buckets_in_reach;
        self.buckets_terrain_rejected += o.buckets_terrain_rejected;
        self.buckets_broadcast += o.buckets_broadcast;
        self.pairs_evaluated += o.pairs_evaluated;
        self.pairs_below_threshold += o.pairs_below_threshold;
    }
}

/// Scatter every applicable cruise bucket onto `accum` for the tile.
/// Receiver lattice (`rx_lat` / `rx_lon` / `rx_alt_m`) lives on the
/// `FusedTileZ13`; terrain arrives precomputed per row (`precompute_row_terrain`);
/// cruise's synth half-segment can extend ~25 km from R7 centre — past
/// the 16 km tile halo — so terrain has to come from the full mmap
/// store rather than the tile's clamped halo (/gg Codex #7).
pub fn scatter_tile(
    tile: &FusedTileZ13,
    cruise: &[CruiseRowView<'_>],
    row_terrain: &[Option<SegmentTerrain>],
    accum: &mut TileAccumulator,
) -> ScatterStats {
    let npd_luts = NpdLuts::shared();

    let bbox = &tile.bbox;
    let tile_centre_lat = (bbox.north_lat + bbox.south_lat) * 0.5;
    let tile_centre_lon = (bbox.east_lon + bbox.west_lon) * 0.5;
    let px_m = tile_pixel_size_m(tile.zoom, tile_centre_lat);
    let half_diag_m = (TILE_PX as f64) * px_m * std::f64::consts::SQRT_2 * 0.5;
    let m_per_lat = noise_compute::constants::M_PER_DEG_LAT;
    let m_per_lon = noise_compute::constants::m_per_deg_lon(tile_centre_lat.to_radians());
    let cap_radius_const = AIRCRAFT_MAX_HORIZONTAL_REACH_M + half_diag_m;

    // Tile-broadcast fast-path gate (M1 restore, mirrors old pipeline-worker
    // `b0736909^:engine/pipeline-worker/src/compute/aircraft.rs:498-540`): uses
    // tile-MAX rx_alt (the receiver farthest from the source has the smallest
    // slant-margin, so its altitude dominates the safety test). Far buckets
    // above the gate scatter onto a 3×3 `CoarseLattice` — the same far-field
    // coarse-lattice + bilinear-upsample path airborne uses — instead of one
    // constant per tile: the old constant broadcast stepped up to ~1 dB at every
    // base-zoom edge (a flat plateau per tile, visible as hard lines under the
    // quiet-zones threshold); the lattice is continuous across edges. The
    // broadcast path is ~0.005 % of cruise kernel evals, so the 9 evals/bucket
    // (vs 1) are free.
    let rx_alt_max = tile
        .rx_alt_m
        .iter()
        .fold(f32::NEG_INFINITY, |a, &b| a.max(b)) as f64;
    let cruise_gate_alt = rx_alt_max + AIRCRAFT_FAR_FIELD_THRESHOLD_M;

    // Far-lattice node count sized to the tile's physical span so the broadcast
    // node spacing stays ~constant (CRUISE_FAR_NODE_SPACING_M) regardless of the
    // receiver-grid zoom: 3 at base-zoom, ~17 at the z10 coarse-field extent. Keeps the
    // bilinear error invariant when the coarse-field pass scatters at z10.
    let tile_span_m = (TILE_PX as f64) * px_m;
    let far_n = far_lattice_n(tile_span_m);

    // Counters shared across threads. Microseg-style sharded compute:
    // each thread owns a private TileAccumulator and merges at the end
    // (same pattern as ground_ops). Memory: ~1.5 MB per thread.
    let in_reach = AtomicUsize::new(0);
    let terrain_rejected = AtomicUsize::new(0);
    let broadcast_count = AtomicUsize::new(0);
    let pairs_eval = AtomicUsize::new(0);
    let pairs_below = AtomicUsize::new(0);

    // Per-thread state: TileAccumulator (slow path) + a far-field CoarseLattice
    // (3×3, fast path). Deferred broadcast — sum onto the lattice, expand once
    // at the end vs N per-bucket stamps — avoids the store-buffer pressure that
    // regressed the old pipeline-worker by 25 s when stamping per bucket
    // (`81bd15ca`).
    let (mut local, broadcast) = cruise
        .par_iter()
        .enumerate()
        .fold(
            || (TileAccumulator::new(), CoarseLattice::new(far_n)),
            |(mut local, mut broadcast), (idx, row)| {
                scatter_one_bucket(
                    row,
                    row_terrain[idx].as_ref(),
                    tile,
                    npd_luts,
                    tile_centre_lat,
                    tile_centre_lon,
                    m_per_lat,
                    m_per_lon,
                    cap_radius_const,
                    cruise_gate_alt,
                    &in_reach,
                    &terrain_rejected,
                    &broadcast_count,
                    &pairs_eval,
                    &pairs_below,
                    &mut local,
                    &mut broadcast,
                );
                (local, broadcast)
            },
        )
        .reduce(
            || (TileAccumulator::new(), CoarseLattice::new(far_n)),
            |(mut a_acc, mut a_lat), (b_acc, b_lat)| {
                a_acc.merge_from(&b_acc);
                a_lat.merge_from(&b_lat);
                (a_acc, a_lat)
            },
        );

    // Stamp the deferred broadcast ONCE per tile: bilinearly upsample the 3×3
    // lattice into every pixel (continuous across tile edges — no plateau).
    broadcast.expand_into(&mut local);
    accum.merge_from(&local);

    ScatterStats {
        buckets_seen: cruise.len(),
        buckets_in_reach: in_reach.load(Ordering::Relaxed),
        buckets_terrain_rejected: terrain_rejected.load(Ordering::Relaxed),
        buckets_broadcast: broadcast_count.load(Ordering::Relaxed),
        pairs_evaluated: pairs_eval.load(Ordering::Relaxed) as u64,
        pairs_below_threshold: pairs_below.load(Ordering::Relaxed) as u64,
    }
}

/// Terrain of every cruise row, sampled ONCE per region build. A row's synth
/// segment geometry depends only on (r7 cell, profile, flight-level bin,
/// origin) — never on the tile being painted — but the row scatters into every
/// z9 tile its ~25 km reach covers, so sampling per (row, tile) re-read the
/// same five raster points up to a handful of times per row (times three
/// periods, which share geometry). `None` = sampled and rejected, or no cell
/// centre; the bucket's own centre check runs first, so uncentred rows are
/// never miscounted as terrain-rejected.
pub fn precompute_row_terrain(
    cruise: &[CruiseRowView<'_>],
    rasters: &dyn RasterSampler,
) -> Vec<Option<SegmentTerrain>> {
    use rayon::prelude::*;
    cruise
        .par_iter()
        .map(|row| {
            let (src_lat, src_lon) = r7_cell_center(row.r7_hex)?;
            let (lat_off, lon_off) =
                cruise_synth_offsets(src_lat, (row.rep_len_m as f64).max(5.0) * 0.5);
            let seg = synth_segment(row, src_lat, src_lon, lat_off, lon_off, 0.0);
            let terrain = SegmentTerrain::sample(&seg, rasters);
            aircraft::is_valid_airborne_with_terrain(&seg, &terrain).then_some(terrain)
        })
        .collect()
}

/// The row's synthetic half-segment. Geometry depends only on (r7 cell,
/// profile, flight-level bin); `density` only weights `count_weight`, so the
/// precompute pass may build it with a placeholder density for terrain
/// sampling — the bucket rebuilds with the live density.
fn synth_segment(
    row: &CruiseRowView<'_>,
    src_lat: f64,
    src_lon: f64,
    lat_off: f64,
    lon_off: f64,
    density: f64,
) -> AircraftSegment {
    let rep_len_m = (row.rep_len_m as f64).max(5.0);
    AircraftSegment {
        flight_id: pack_synth(0),
        profile_idx: row.rep_profile_idx,
        is_departure: true,
        on_ground: false,
        period: row.period,
        date_id: 0,
        start_lat: src_lat - lat_off,
        start_lon: src_lon - lon_off,
        start_alt_m: row.rep_alt_m,
        end_lat: src_lat + lat_off,
        end_lon: src_lon + lon_off,
        end_alt_m: row.rep_alt_m,
        speed_kt: row.rep_speed_kt,
        segment_length_m: rep_len_m as f32,
        count_weight: density as f32,
        surface_model: false,
        ground_context: GROUND_CONTEXT_NONE,
        ground_ops_kind: GROUND_OPS_KIND_NONE,
        source_id: row.source_id as u16,
    }
}

#[allow(clippy::too_many_arguments)]
fn scatter_one_bucket(
    row: &CruiseRowView<'_>,
    row_terrain: Option<&SegmentTerrain>,
    tile: &FusedTileZ13,
    npd_luts: &NpdLuts,
    tile_centre_lat: f64,
    tile_centre_lon: f64,
    m_per_lat: f64,
    m_per_lon: f64,
    cap_radius_const: f64,
    cruise_gate_alt: f64,
    in_reach: &AtomicUsize,
    terrain_rejected: &AtomicUsize,
    broadcast_count: &AtomicUsize,
    pairs_eval: &AtomicUsize,
    pairs_below: &AtomicUsize,
    accum: &mut TileAccumulator,
    broadcast: &mut CoarseLattice,
) {
    let Some((src_lat, src_lon)) = r7_cell_center(row.r7_hex) else {
        return;
    };
    let rep_len_m = (row.rep_len_m as f64).max(5.0);
    let half_len_m = rep_len_m * 0.5;

    // Antimeridian-safe longitude delta (same as popup cruise.rs:103).
    let mut dlon = src_lon - tile_centre_lon;
    if dlon > 180.0 {
        dlon -= 360.0;
    } else if dlon < -180.0 {
        dlon += 360.0;
    }
    let dlat_m = (src_lat - tile_centre_lat) * m_per_lat;
    let dlon_m = dlon * m_per_lon;
    let cap_m = cap_radius_const + half_len_m;
    if dlat_m * dlat_m + dlon_m * dlon_m > cap_m * cap_m {
        return;
    }
    in_reach.fetch_add(1, Ordering::Relaxed);

    let density = if row.rep_len_m > 0.0 {
        (row.sum_length_m / row.rep_len_m) as f64
    } else {
        0.0
    };
    if density <= 0.0 {
        return;
    }

    let Some(terrain) = row_terrain else {
        terrain_rejected.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let (lat_off, lon_off) = cruise_synth_offsets(src_lat, half_len_m);
    let seg = synth_segment(row, src_lat, src_lon, lat_off, lon_off, density);

    let period_idx = row.period.min(2);

    // Tile-broadcast fast-path: when the segment's lowest endpoint clears
    // the tile's highest receiver by AIRCRAFT_FAR_FIELD_THRESHOLD_M (7 620 m,
    // CFFK regime where Doc 29 §A.3.4 ΔF / Λ / ΔI all collapse), one kernel
    // call at tile centre / mean rx_alt stands in for 262 144 per-pixel calls.
    // Old pipeline-worker reference: `b0736909^:.../aircraft.rs:498-540`.
    // Energy goes into a per-period accumulator; the actual per-pixel write
    // is deferred to a single end-of-tile broadcast in `scatter_tile`.
    let seg_min_alt = (seg.start_alt_m.min(seg.end_alt_m)) as f64;
    if seg_min_alt > cruise_gate_alt {
        // Far field: scatter onto the tile's coarse lattice (3×3 at base-zoom, finer
        // at the z10 coarse-field extent — `far_lattice_n`), each node at its
        // actual pixel receiver (lat/lon + terrain altitude). `scatter_tile`
        // bilinearly upsamples once. Adjacent tiles sample their neighbouring
        // edge pixels, so the field stays continuous — no per-tile plateau.
        let n = broadcast.n();
        let mut below = 0usize;
        for ci in 0..n {
            let py = broadcast.coarse_pixel(ci);
            let rx_lat = tile.rx_lat[py];
            for cj in 0..n {
                let px = broadcast.coarse_pixel(cj);
                let rx_lon = tile.rx_lon[px];
                let rx_alt = tile.rx_alt_m[py * TILE_PX + px] as f64;
                match aircraft::segment_sel_with_terrain_energy(
                    &seg, rx_lat, rx_lon, rx_alt, terrain, npd_luts,
                ) {
                    // sel → linear energy via fast_exp_f64; see airborne.rs:208.
                    Some(sel) => {
                        let e = noise_compute::propagation::iso9613::fast_exp_f64(
                            sel * std::f64::consts::LN_10 * 0.1,
                        ) * density;
                        broadcast.add_energy_at(ci, cj, period_idx, e as f32);
                    }
                    None => below += 1,
                }
            }
        }
        if below < n * n {
            broadcast_count.fetch_add(1, Ordering::Relaxed);
        }
        pairs_eval.fetch_add(n * n, Ordering::Relaxed);
        pairs_below.fetch_add(below, Ordering::Relaxed);
        return;
    }

    let mut local_eval = 0usize;
    let mut local_below = 0usize;
    for py in 0..TILE_PX as u32 {
        let rx_lat = tile.rx_lat[py as usize];
        let row_base = (py as usize) * TILE_PX;
        for px in 0..TILE_PX as u32 {
            let rx_lon = tile.rx_lon[px as usize];
            let rx_alt = tile.rx_alt_m[row_base + px as usize] as f64;
            local_eval += 1;
            let Some(sel) = aircraft::segment_sel_with_terrain_energy(
                &seg, rx_lat, rx_lon, rx_alt, terrain, npd_luts,
            ) else {
                local_below += 1;
                continue;
            };
            // sel → linear energy via fast_exp_f64; see airborne.rs:208.
            let e = noise_compute::propagation::iso9613::fast_exp_f64(
                sel * std::f64::consts::LN_10 * 0.1,
            ) * density;
            accum.add_energy_at(py, px, period_idx, e as f32);
        }
    }
    pairs_eval.fetch_add(local_eval, Ordering::Relaxed);
    pairs_below.fetch_add(local_below, Ordering::Relaxed);
}

fn r7_cell_center(r7_hex: u64) -> Option<(f64, f64)> {
    let cell = CellIndex::try_from(r7_hex).ok()?;
    let ll = h3o::LatLng::from(cell);
    Some((ll.lat(), ll.lng()))
}
