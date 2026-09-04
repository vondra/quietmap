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

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

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

use crate::accumulator::{CoarseLattice, TileAccumulator, NUM_PERIODS};
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
    /// Buckets routed to the broadcast lattice (segment above the far-field gate).
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

/// A cruise bucket admitted to this tile: the synthetic segment it stands for, the
/// terrain sampled for its row, its flight density, and the path it routes to.
///
/// The admitted set is materialised in INPUT order and both scatter passes below
/// parallelise over RECEIVERS, never over buckets, so every accumulator cell is summed
/// by exactly one task over one fixed bucket order. That is what makes a painted tile
/// bit-reproducible. Splitting over buckets instead (a rayon `fold` + `reduce` of
/// per-thread accumulators, as this painter did until 2026-09-04) merges partial f32
/// sums in work-stealing order: two runs of the same binary on the same host disagreed
/// in 2 cells of the Dobris R4's 44 040 192, each by the 0.5 dB quantisation step.
struct AdmittedBucket<'a> {
    segment: AircraftSegment,
    terrain: &'a SegmentTerrain,
    density: f64,
    period_idx: u8,
    /// The segment's lowest endpoint clears the tile's highest receiver by
    /// [`AIRCRAFT_FAR_FIELD_THRESHOLD_M`], so one coarse lattice stands in for the
    /// 262 144 per-pixel kernel calls.
    broadcast: bool,
}

/// The per-tile constants every bucket is admitted against.
struct TileAdmission {
    centre_lat: f64,
    centre_lon: f64,
    m_per_lat: f64,
    m_per_lon: f64,
    /// Reach cap measured from the tile centre, before the bucket's own half length.
    cap_radius_m: f64,
    /// Segment altitude above which the bucket takes the broadcast lattice.
    broadcast_gate_alt_m: f64,
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
    assert_eq!(
        cruise.len(),
        row_terrain.len(),
        "row terrain is sampled once per cruise row"
    );
    let npd_luts = NpdLuts::shared();

    let bbox = &tile.bbox;
    let tile_centre_lat = (bbox.north_lat + bbox.south_lat) * 0.5;
    let tile_centre_lon = (bbox.east_lon + bbox.west_lon) * 0.5;
    let px_m = tile_pixel_size_m(tile.zoom, tile_centre_lat);
    let half_diag_m = (TILE_PX as f64) * px_m * std::f64::consts::SQRT_2 * 0.5;

    // Tile-broadcast fast-path gate (M1 restore, mirrors old pipeline-worker
    // `b0736909^:engine/pipeline-worker/src/compute/aircraft.rs:498-540`): uses
    // tile-MAX rx_alt (the receiver farthest from the source has the smallest
    // slant-margin, so its altitude dominates the safety test). Far buckets
    // above the gate scatter onto a `CoarseLattice` — the same far-field
    // coarse-lattice + bilinear-upsample path airborne uses — instead of one
    // constant per tile: the old constant broadcast stepped up to ~1 dB at every
    // base-zoom edge (a flat plateau per tile, visible as hard lines under the
    // quiet-zones threshold); the lattice is continuous across edges. The
    // broadcast path is a small share of cruise kernel evals, so the extra nodes
    // per bucket (vs 1) are free.
    let rx_alt_max = tile
        .rx_alt_m
        .iter()
        .fold(f32::NEG_INFINITY, |a, &b| a.max(b)) as f64;
    let admission = TileAdmission {
        centre_lat: tile_centre_lat,
        centre_lon: tile_centre_lon,
        m_per_lat: noise_compute::constants::M_PER_DEG_LAT,
        m_per_lon: noise_compute::constants::m_per_deg_lon(tile_centre_lat.to_radians()),
        cap_radius_m: AIRCRAFT_MAX_HORIZONTAL_REACH_M + half_diag_m,
        broadcast_gate_alt_m: rx_alt_max + AIRCRAFT_FAR_FIELD_THRESHOLD_M,
    };

    let in_reach = AtomicUsize::new(0);
    let terrain_rejected = AtomicUsize::new(0);

    // Admission is bucket-parallel, but `collect` keeps the input order, so the two
    // receiver-parallel passes below sum every cell over one fixed bucket sequence.
    let admitted: Vec<AdmittedBucket<'_>> = cruise
        .par_iter()
        .zip(row_terrain.par_iter())
        .filter_map(|(row, terrain)| {
            admit_bucket(
                row,
                terrain.as_ref(),
                &admission,
                &in_reach,
                &terrain_rejected,
            )
        })
        .collect();
    let (broadcast_buckets, near_buckets): (Vec<&AdmittedBucket<'_>>, Vec<&AdmittedBucket<'_>>) =
        admitted.iter().partition(|bucket| bucket.broadcast);

    // Far-lattice node count sized to the tile's physical span so the broadcast
    // node spacing stays ~constant (CRUISE_FAR_NODE_SPACING_M) regardless of the
    // receiver-grid zoom. Keeps the bilinear error invariant when the coarse-field
    // pass scatters at the cruise compute zoom. The deferred stamp — sum onto the
    // lattice, expand once at the end vs one stamp per bucket — avoids the
    // store-buffer pressure that regressed the old pipeline-worker by 25 s
    // (`81bd15ca`).
    let mut broadcast = CoarseLattice::new(far_lattice_n((TILE_PX as f64) * px_m));
    let far_evals = scatter_broadcast_lattice(&broadcast_buckets, tile, npd_luts, &mut broadcast);
    let near_evals = scatter_near_pixels(&near_buckets, tile, npd_luts, accum);
    broadcast.expand_into(accum);

    ScatterStats {
        buckets_seen: cruise.len(),
        buckets_in_reach: in_reach.load(Ordering::Relaxed),
        buckets_terrain_rejected: terrain_rejected.load(Ordering::Relaxed),
        buckets_broadcast: broadcast_buckets.len(),
        pairs_evaluated: far_evals.0 + near_evals.0,
        pairs_below_threshold: far_evals.1 + near_evals.1,
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

/// Resolve one cruise row against this tile: reach cap, flight density, row terrain,
/// then the near/broadcast routing. `None` = the bucket contributes nothing here.
fn admit_bucket<'a>(
    row: &CruiseRowView<'_>,
    row_terrain: Option<&'a SegmentTerrain>,
    tile: &TileAdmission,
    in_reach: &AtomicUsize,
    terrain_rejected: &AtomicUsize,
) -> Option<AdmittedBucket<'a>> {
    let (src_lat, src_lon) = r7_cell_center(row.r7_hex)?;
    let rep_len_m = (row.rep_len_m as f64).max(5.0);
    let half_len_m = rep_len_m * 0.5;

    // Antimeridian-safe longitude delta (same as popup cruise.rs:103).
    let mut dlon = src_lon - tile.centre_lon;
    if dlon > 180.0 {
        dlon -= 360.0;
    } else if dlon < -180.0 {
        dlon += 360.0;
    }
    let dlat_m = (src_lat - tile.centre_lat) * tile.m_per_lat;
    let dlon_m = dlon * tile.m_per_lon;
    let cap_m = tile.cap_radius_m + half_len_m;
    if dlat_m * dlat_m + dlon_m * dlon_m > cap_m * cap_m {
        return None;
    }
    in_reach.fetch_add(1, Ordering::Relaxed);

    let density = if row.rep_len_m > 0.0 {
        (row.sum_length_m / row.rep_len_m) as f64
    } else {
        0.0
    };
    if density <= 0.0 {
        return None;
    }

    let terrain = match row_terrain {
        Some(terrain) => terrain,
        None => {
            terrain_rejected.fetch_add(1, Ordering::Relaxed);
            return None;
        }
    };
    let (lat_off, lon_off) = cruise_synth_offsets(src_lat, half_len_m);
    let segment = synth_segment(row, src_lat, src_lon, lat_off, lon_off, density);
    // The segment's lowest endpoint against the tile's highest receiver: the CFFK
    // regime (Doc 29 §A.3.4 ΔF / Λ / ΔI all collapse) where one coarse lattice node
    // stands in for a whole neighbourhood of pixels.
    let broadcast = (segment.start_alt_m.min(segment.end_alt_m) as f64) > tile.broadcast_gate_alt_m;
    Some(AdmittedBucket {
        segment,
        terrain,
        density,
        period_idx: row.period.min(2),
        broadcast,
    })
}

/// SEL (dB) → linear energy for one bucket. `fast_exp_f64` is the Padé exponential the
/// popup and the CUDA kernels share (see `airborne.rs:208`).
#[inline]
fn bucket_energy(sel: f64, density: f64) -> f32 {
    (noise_compute::propagation::iso9613::fast_exp_f64(sel * std::f64::consts::LN_10 * 0.1)
        * density) as f32
}

/// Sum the far-field buckets onto the tile's broadcast lattice through the
/// deterministic fixed-part split. Adjacent tiles sample their neighbouring edge
/// pixels, so the expanded field stays continuous — no per-tile plateau.
/// Returns `(kernel evals, evals below the SEL floor)`.
fn scatter_broadcast_lattice(
    buckets: &[&AdmittedBucket<'_>],
    tile: &FusedTileZ13,
    npd_luts: &NpdLuts,
    lattice: &mut CoarseLattice,
) -> (u64, u64) {
    lattice.scatter_in_fixed_parts(buckets, |chunk, part| {
        let n = part.n();
        let mut below = 0u64;
        for bucket in chunk {
            for ci in 0..n {
                let py = part.coarse_pixel(ci);
                let rx_lat = tile.rx_lat[py];
                let row_base = py * TILE_PX;
                for cj in 0..n {
                    let px = part.coarse_pixel(cj);
                    let rx_alt = tile.rx_alt_m[row_base + px] as f64;
                    match aircraft::segment_sel_with_terrain_energy(
                        &bucket.segment,
                        rx_lat,
                        tile.rx_lon[px],
                        rx_alt,
                        bucket.terrain,
                        npd_luts,
                    ) {
                        Some(sel) => part.add_energy_at(
                            ci,
                            cj,
                            bucket.period_idx,
                            bucket_energy(sel, bucket.density),
                        ),
                        None => below += 1,
                    }
                }
            }
        }
        ((chunk.len() * n * n) as u64, below)
    })
}

/// Sum the near buckets into `accum`, parallel over receiver pixel ROWS: one task owns
/// a pixel row and walks the buckets in order, so the pixel's energy has one fixed
/// summation order. Returns `(kernel evals, evals below the SEL floor)`.
fn scatter_near_pixels(
    buckets: &[&AdmittedBucket<'_>],
    tile: &FusedTileZ13,
    npd_luts: &NpdLuts,
    accum: &mut TileAccumulator,
) -> (u64, u64) {
    let below = AtomicU64::new(0);
    accum
        .energy
        .par_chunks_mut(TILE_PX * NUM_PERIODS)
        .enumerate()
        .for_each(|(py, pixel_row)| {
            let rx_lat = tile.rx_lat[py];
            let row_base = py * TILE_PX;
            let mut row_below = 0u64;
            for bucket in buckets {
                for px in 0..TILE_PX {
                    let rx_alt = tile.rx_alt_m[row_base + px] as f64;
                    let Some(sel) = aircraft::segment_sel_with_terrain_energy(
                        &bucket.segment,
                        rx_lat,
                        tile.rx_lon[px],
                        rx_alt,
                        bucket.terrain,
                        npd_luts,
                    ) else {
                        row_below += 1;
                        continue;
                    };
                    pixel_row[px * NUM_PERIODS + bucket.period_idx as usize] +=
                        bucket_energy(sel, bucket.density);
                }
            }
            below.fetch_add(row_below, Ordering::Relaxed);
        });
    (
        (buckets.len() * TILE_PX * TILE_PX) as u64,
        below.load(Ordering::Relaxed),
    )
}

fn r7_cell_center(r7_hex: u64) -> Option<(f64, f64)> {
    let cell = CellIndex::try_from(r7_hex).ok()?;
    let ll = h3o::LatLng::from(cell);
    Some((ll.lat(), ll.lng()))
}
