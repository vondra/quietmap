//! Per-region surface tile builder — the GROUND family (road/rail line,
//! industrial/building point, airport ground-ops). One region = one output
//! R4: it loads the region's `grid_disk(1)` rows for every requested layer,
//! batches its base-zoom tiles sharing ONE terrain halo, scatters each layer onto
//! the hot `Arc<FusedGrid>`, and writes `{layer}/{z}/{x}/{y}.bin`.
//!
//! Factored out of `bin/build_heatmap_surface.rs` so the binary stays a thin
//! CLI + orchestration shell and the regions run on an outer rayon over a
//! Morton curve (axis 2), exactly like the aircraft `region_runner`. Unlike
//! aircraft, surface holds a real 10 km halo per region, so the binary caps
//! how many regions build at once (see its `region_concurrency`).
//!
//! Equivalence to the old sequential per-region build: a tile is owned by its
//! CENTRE R4, the kernels sum commutatively onto a fresh per-tile accumulator,
//! and each region writes only its own tiles — so region order never changes a
//! byte of any tile (bit-identical; verified by an A/B byte-diff).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::ValueEnum;
use h3o::{CellIndex, LatLng};
use raster_reader::fused_tile_z13::TileBatch;
use raster_reader::{FusedPixel, RealRasters};

use crate::accumulator::TileAccumulator;
use crate::source_line::LineRow;
use crate::source_loader_barrier::BarrierData;
use crate::source_loader_building::BuildingData;
use crate::source_loader_industrial::IndustrialData;
use crate::source_loader_obstacle::ObstacleData;
use crate::source_loader_rail::RailData;
use crate::source_loader_road::RoadData;
use crate::source_loader_traffic::AirportTrafficData;
use crate::source_point::PointRow;
use crate::wire_hm3::{
    collapse_lden_surface_u8, collapse_lden_u8, fill_area_median, write_tile, AREA_FILL_RADIUS_PX,
    SOURCE_ID_AIRCRAFT, SOURCE_ID_BUILDING, SOURCE_ID_INDUSTRIAL, SOURCE_ID_RAIL, SOURCE_ID_ROAD,
};
use crate::{ground_ops, scatter_line, scatter_point};
use noise_compute::admin;
use noise_compute::constants::{
    GROUND_OPS_RUNWAY_MAX_RADIUS, INDUSTRIAL_MAX_RADIUS, RAILWAY_REACH_CEILING,
};

/// Per-layer halo: covers the source→receiver ray at the layer's max reach.
/// Road = motorway-class cap (10 km); rail + industrial reference the single
/// reach the loader gates on; building is capped at 2 km by
/// `prepare_building_points`. In `--source ground` the shared halo is the MAX
/// of the requested layers (= road 10 km); a shorter-reach layer only
/// ray-marches its own inner disk, so a larger halo leaves its output unchanged.
const ROAD_HALO_M: f64 = 10_000.0;
const RAIL_HALO_M: f64 = RAILWAY_REACH_CEILING;
const INDUSTRIAL_HALO_M: f64 = INDUSTRIAL_MAX_RADIUS;
const BUILDING_HALO_M: f64 = 2_000.0;
const GROUNDOPS_HALO_M: f64 = GROUND_OPS_RUNWAY_MAX_RADIUS;

#[derive(Clone, Copy, Debug, PartialEq, ValueEnum)]
pub enum Source {
    Road,
    Rail,
    Industrial,
    Building,
    /// Airport ground ops (taxi/runway/apron) — a terrain-ray-march source like
    /// the others, but event energy ÷ n_days (`collapse_lden_u8`) loaded from the
    /// airport_traffic arrows. Emitted as `aircraft-ground` (SOURCE_ID_AIRCRAFT).
    AircraftGround,
    /// All five ground (terrain-ray-march) layers in one pass, sharing the halo.
    Ground,
}

/// `(HM3 source_id, halo metres, layer subdir name)` for a concrete layer.
pub fn layer_meta(s: Source) -> (u8, f64, &'static str) {
    match s {
        Source::Road => (SOURCE_ID_ROAD, ROAD_HALO_M, "road"),
        Source::Rail => (SOURCE_ID_RAIL, RAIL_HALO_M, "rail"),
        Source::Industrial => (SOURCE_ID_INDUSTRIAL, INDUSTRIAL_HALO_M, "industrial"),
        Source::Building => (SOURCE_ID_BUILDING, BUILDING_HALO_M, "building"),
        Source::AircraftGround => (SOURCE_ID_AIRCRAFT, GROUNDOPS_HALO_M, "aircraft-ground"),
        Source::Ground => unreachable!("Ground is expanded to its concrete layers"),
    }
}

/// Per-region loaded rows, dispatched to the matching scatter kernel: road +
/// rail are ISO line sources; industrial + building are point sources; airport
/// ground ops are airport-traffic microsegments (event energy ÷ n_days).
enum SurfaceRows {
    Line(Vec<LineRow>),
    Point(Vec<PointRow>),
    GroundOps(AirportTrafficData),
}

impl SurfaceRows {
    fn len(&self) -> usize {
        match self {
            SurfaceRows::Line(rows) => rows.len(),
            SurfaceRows::Point(rows) => rows.len(),
            SurfaceRows::GroundOps(data) => data.n_rows(),
        }
    }
}

/// Load one layer's `grid_disk(1)` rows for a region. Road needs the admin table
/// for default-AADT; rail needs it for the C1 per-region day/evening/night split
/// (EU freight runs ~55 % at night); the other layers have no admin dependency.
/// The admin table is a process-wide `OnceLock` filled ONCE before the parallel
/// section, so this read-only lookup is concurrency-safe.
fn load_layer_rows(
    s: Source,
    h3r4_dir: &Path,
    ring: &[u64],
    cell: CellIndex,
) -> Result<SurfaceRows> {
    // Region admin (centre-R4 centroid) — shared by road's default-AADT cascade
    // and rail's period split. Resolved on demand; cheap OnceLock read.
    let region_admin = || {
        let ll = LatLng::from(cell);
        admin::admin_for_latlng(ll.lat(), ll.lng())
    };
    Ok(match s {
        Source::Road => SurfaceRows::Line(
            RoadData::load_for_r4s(h3r4_dir, ring, region_admin())
                .context("load roads")?
                .into_rows(),
        ),
        Source::Rail => SurfaceRows::Line(
            RailData::load_for_r4s(h3r4_dir, ring, region_admin())
                .context("load railways")?
                .into_rows(),
        ),
        Source::Industrial => SurfaceRows::Point(
            IndustrialData::load_for_r4s(h3r4_dir, ring)
                .context("load industrial")?
                .into_rows(),
        ),
        Source::Building => SurfaceRows::Point(
            BuildingData::load_for_r4s(h3r4_dir, ring)
                .context("load buildings")?
                .into_rows(),
        ),
        Source::AircraftGround => SurfaceRows::GroundOps(
            AirportTrafficData::load_for_r4s(h3r4_dir, ring).context("load airport traffic")?,
        ),
        Source::Ground => unreachable!("Ground is expanded to its concrete layers"),
    })
}

/// Per-layer loaded rows + scatter telemetry. `loaded_rows` is counted once per
/// region load; the other counters are tile-level work counters.
#[derive(Default, Clone)]
pub struct LayerStats {
    pub loaded_rows: u64,
    pub scatter: Duration,
    pub path_calls: u64,
    pub skipped_calls: u64,
    /// (source, receiver) pairs priced by the cheap pass — the denominator the
    /// skip fraction wants (`path_calls` counts RAYS, several per pair).
    pub pairs: u64,
    pub raster_samples: u64,
    pub ground_rows_in_reach: u64,
    pub ground_unique_microsegs: u64,
}

impl LayerStats {
    fn merge(&mut self, o: &LayerStats) {
        self.loaded_rows += o.loaded_rows;
        self.scatter += o.scatter;
        self.path_calls += o.path_calls;
        self.skipped_calls += o.skipped_calls;
        self.pairs += o.pairs;
        self.raster_samples += o.raster_samples;
        self.ground_rows_in_reach += o.ground_rows_in_reach;
        self.ground_unique_microsegs += o.ground_unique_microsegs;
    }
}

/// All telemetry one worker accumulates over its chunk of regions; merged into
/// the build total after the parallel section. Phase durations sum across
/// regions (CPU-like under outer rayon), as with the aircraft builder.
#[derive(Default)]
pub struct SurfaceStats {
    pub written: usize,
    pub skipped: usize,
    pub bytes: usize,
    pub t_load: Duration,
    pub t_raster: Duration,
    pub t_write: Duration,
    /// Ray-march raster reads vs batch-halo cells = per-cell re-read factor.
    pub raster_samples: u64,
    pub grid_cells: u64,
    pub by_layer: BTreeMap<&'static str, LayerStats>,
    /// Whether the first batch's halo size was already logged by this worker
    /// (once per worker, not per build — close enough for an L3-residency note).
    pub halo_logged: bool,
}

impl SurfaceStats {
    pub fn merge(&mut self, o: SurfaceStats) {
        self.written += o.written;
        self.skipped += o.skipped;
        self.bytes += o.bytes;
        self.t_load += o.t_load;
        self.t_raster += o.t_raster;
        self.t_write += o.t_write;
        self.raster_samples += o.raster_samples;
        self.grid_cells += o.grid_cells;
        for (name, ls) in o.by_layer {
            self.by_layer.entry(name).or_default().merge(&ls);
        }
        self.halo_logged |= o.halo_logged;
    }
}

/// Immutable per-build settings shared (read-only) across every region/worker. The layer SET to
/// build is deliberately NOT a field here — it is passed explicitly to `process_surface_region`
/// instead (see that function's own doc), so `--stream` can narrow it per cell (paint-pipeline-v4
/// PR#1 §3) without rebuilding this otherwise-shared, read-only struct for every cell.
pub struct SurfaceCtx<'a> {
    pub zoom: u8,
    pub halo_m: f64,
    pub batch_n: u32,
    pub n_days: f64,
    /// GA 365-day hybrid per-class weight LUT — only the ground-ops
    /// (aircraft traffic) layer consumes it; other surface layers ignore
    /// it. Resolved build-wide from `airport_traffic.arrow`'s
    /// `sample_days_by_class` (`ga-365d-hybrid-plan.md` §2). Uniform when
    /// ground ops isn't in the build.
    pub class_weights: noise_compute::emission::aircraft::ClassWeights,
    pub write_empty: bool,
    pub h3r4_dir: &'a Path,
    pub output: &'a Path,
    pub rasters: &'a RealRasters,
}

/// Live cross-region progress shared by every worker. A heartbeat (~30 s) names
/// the region that crossed the tick and the global tile count, so a multi-day
/// run is observable instead of a wall of identical halo lines.
pub struct Heartbeat {
    pub tiles_done: AtomicUsize,
    pub n_tiles: usize,
    pub n_regions: usize,
    /// `Instant` isn't atomic; the millis-since-`start` of the last beat is.
    pub start: Instant,
    pub last_beat_ms: AtomicUsize,
}

impl Heartbeat {
    pub fn new(n_tiles: usize, n_regions: usize) -> Self {
        Self {
            tiles_done: AtomicUsize::new(0),
            n_tiles,
            n_regions,
            start: Instant::now(),
            last_beat_ms: AtomicUsize::new(0),
        }
    }

    /// Count one finished tile; emit a beat at most every 30 s. The region id is
    /// prefixed so interleaved lines from concurrent workers stay attributable.
    fn tick(&self, region_r4: u64) {
        let done = self.tiles_done.fetch_add(1, Ordering::Relaxed) + 1;
        let now_ms = self.start.elapsed().as_millis() as usize;
        let last = self.last_beat_ms.load(Ordering::Relaxed);
        if now_ms.saturating_sub(last) < 30_000 {
            return;
        }
        // One winner per tick: only the worker that swaps the stamp prints, so a
        // burst of concurrent finishers doesn't spam 30 lines at once.
        if self
            .last_beat_ms
            .compare_exchange(last, now_ms, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            if self.n_tiles == 0 {
                // --stream: total is unknown (cells arrive on stdin), so report just the count.
                eprintln!("[surface] r4 {region_r4:015x} · {done} tiles built (stream)");
            } else {
                eprintln!(
                    "[surface] r4 {region_r4:015x} · {done}/{} tiles ({:.1}%) over {} region(s)",
                    self.n_tiles,
                    done as f64 / self.n_tiles as f64 * 100.0,
                    self.n_regions,
                );
            }
        }
    }
}

/// Build every tile of one output region: load its `grid_disk(1)` rows for each
/// requested layer + the barrier slice, batch its tiles (one shared halo per
/// batch), scatter each layer, collapse to the Lden byte, write the tiles, and
/// unlink any now-silent stale tile. Returns this region's telemetry.
///
/// `layers` is the layer set to build for THIS CALL — usually the build's whole
/// configured set (the batch path, `main()`'s non-`--stream` loop), but `--stream`
/// narrows it per cell to just the STALE layers a fresh lease named (paint-
/// pipeline-v4 PR#1 §3: a rail-only dv change must not repaint road). Passed
/// explicitly, not read off `ctx`, so the shared read-only `ctx` never needs
/// rebuilding just to vary this one thing per cell.
pub fn process_surface_region(
    ctx: &SurfaceCtx,
    region_r4: u64,
    tiles: &[(u32, u32)],
    heartbeat: &Heartbeat,
    layers: &[Source],
) -> Result<SurfaceStats> {
    let mut stats = SurfaceStats::default();
    if tiles.is_empty() {
        return Ok(stats);
    }
    let cell = CellIndex::try_from(region_r4).expect("valid R4");
    let ring: Vec<u64> = cell
        .grid_disk::<Vec<_>>(1)
        .into_iter()
        .map(u64::from)
        .collect();

    // Load every requested layer's rows for this region, held across all the
    // region's batches so the shared halo is built once, not once per layer;
    // resolve (source_id, output subdir) here so the per-tile loop is bare.
    let t_l = Instant::now();
    let layer_rows: Vec<(SurfaceRows, u8, &'static str)> = layers
        .iter()
        .map(|&s| {
            let (source_id, _, dir_name) = layer_meta(s);
            Ok((
                load_layer_rows(s, ctx.h3r4_dir, &ring, cell)?,
                source_id,
                dir_name,
            ))
        })
        .collect::<Result<_>>()
        .with_context(|| format!("region R4 {region_r4:015x}"))?;
    // Noise walls screen every layer (B8/C9): load the region's barriers.arrow
    // once (absent in 98.5% of regions → empty, zero cost), then slice per tile.
    let barrier_data = BarrierData::load_for_r4s(ctx.h3r4_dir, &ring)
        .with_context(|| format!("load barriers R4 {region_r4:015x}"))?;
    let obstacle_data = ObstacleData::load_for_r4s(ctx.h3r4_dir, region_r4, &ring)
        .with_context(|| format!("load obstacles R4 {region_r4:015x}"))?;
    stats.t_load += t_l.elapsed();
    for (rows, _, dir_name) in &layer_rows {
        stats.by_layer.entry(*dir_name).or_default().loaded_rows += rows.len() as u64;
    }

    let mut batches: BTreeMap<(u32, u32), Vec<(u32, u32)>> = BTreeMap::new();
    for &(x, y) in tiles {
        batches
            .entry((
                (x / ctx.batch_n) * ctx.batch_n,
                (y / ctx.batch_n) * ctx.batch_n,
            ))
            .or_default()
            .push((x, y));
    }
    for ((bx, by), batch_tiles) in &batches {
        // ONE halo per batch (10 km in ground mode), shared by every layer.
        let t_r = Instant::now();
        let mut batch = TileBatch::build(ctx.zoom, *bx, *by, ctx.batch_n, ctx.halo_m, ctx.rasters);
        // Vector mode: the pre-baked rx_refl (raster 3×3 enclosure) is
        // recomputed from footprints — the SAME 150 × 150 m probe — and the
        // GPU rxar upload carries it unchanged (gg review: pre-bake site).
        // NOTE: the POPUP still takes reflection from the raster probe; its
        // vector enclosure lands with the popup-reflection follow-up (plan
        // 1.4b) — until then flag-ON pipeline vs popup reflection may differ
        // by one 1.5 dB step at footprint edges.
        if let Some(set) = obstacle_data.set() {
            // Only the REQUESTED tiles are painted — rebaking the whole
            // batch_n² grid would triple the bake cost for nothing.
            for &(x, y) in batch_tiles {
                let tile = &mut batch.tiles[((y - by) * ctx.batch_n + (x - bx)) as usize];
                crate::source_loader_obstacle::bake_tile_vector_rx_refl(tile, set);
            }
        }
        stats.t_raster += t_r.elapsed();
        // Count the shared halo's cells once per batch (adjacent batch halos
        // overlap → allocated batch-halo cells, a slight over-count ⇒ a
        // conservative redundancy floor for multi-batch runs).
        stats.grid_cells += batch.tiles[0].halo.cell_count() as u64;
        if !stats.halo_logged {
            let mb = (batch.tiles[0].halo.cell_count() * std::mem::size_of::<FusedPixel>()) as f64
                / (1024.0 * 1024.0);
            eprintln!(
                "[surface] halo {mb:.1} MB — L3-resident, shared by {} layer(s)/batch",
                layer_rows.len(),
            );
            stats.halo_logged = true;
        }

        for &(x, y) in batch_tiles {
            let tile = &batch.tiles[((y - by) * ctx.batch_n + (x - bx)) as usize];
            // One sorted, conservative-distance barrier slice per tile, shared by
            // every layer (contract: types::Barrier docs).
            let tile_barriers = barrier_data.for_tile(&tile.bbox, ctx.halo_m);
            // Building-interior receiver mask (fix-pack Fix 4): ONE
            // point-in-footprint pass per tile, shared by every layer (they all
            // ride the same receiver lattice). Vector regions only — a
            // raster-fallback region keeps today's behavior.
            let t_m = Instant::now();
            let interior_mask = obstacle_data
                .set()
                .map(|set| crate::source_loader_obstacle::bake_tile_interior_mask(tile, set));
            stats.t_raster += t_m.elapsed();
            for (rows, source_id, dir_name) in &layer_rows {
                let mut accum = TileAccumulator::new();
                let t_s = Instant::now();
                let (walked, sk, npr, rs, ground_rows, ground_microsegs, time_divided) = match rows
                {
                    SurfaceRows::Line(r) => {
                        let st = scatter_line::scatter_tile(
                            tile,
                            r,
                            &tile_barriers,
                            obstacle_data.set(),
                            &mut accum,
                        );
                        (
                            st.path_calls,
                            st.skipped_calls,
                            st.pairs,
                            st.raster_samples,
                            0,
                            0,
                            false,
                        )
                    }
                    SurfaceRows::Point(r) => {
                        let st = scatter_point::scatter_tile(
                            tile,
                            r,
                            &tile_barriers,
                            obstacle_data.set(),
                            &mut accum,
                        );
                        (
                            st.path_calls,
                            st.skipped_calls,
                            st.pairs,
                            st.raster_samples,
                            0,
                            0,
                            false,
                        )
                    }
                    SurfaceRows::GroundOps(data) => {
                        let views = data.views();
                        let st = ground_ops::scatter_tile(
                            tile,
                            &views,
                            &tile_barriers,
                            obstacle_data.set(),
                            &ctx.class_weights,
                            &mut accum,
                        );
                        (
                            st.path_calls,
                            st.skipped_calls,
                            // ground-ops is still on the η budget skip, which has
                            // no cheap-pass pair count to report.
                            0,
                            0,
                            st.rows_in_reach as u64,
                            st.unique_microsegs as u64,
                            true,
                        )
                    }
                };
                stats.raster_samples += rs;
                let e = stats.by_layer.entry(*dir_name).or_default();
                e.scatter += t_s.elapsed();
                e.path_calls += walked;
                e.skipped_calls += sk;
                e.pairs += npr;
                e.raster_samples += rs;
                e.ground_rows_in_reach += ground_rows;
                e.ground_unique_microsegs += ground_microsegs;

                let t_w = Instant::now();
                // Ground ops sum event energy ÷ n_days; the surface layers are
                // steady-power (no time division).
                let mut cells = if time_divided {
                    collapse_lden_u8(&accum, ctx.n_days)
                } else {
                    collapse_lden_surface_u8(&accum)
                };
                // AREA sources (building / industrial / leisure) discretise into a
                // point grid that leaves an inter-point ripple; smooth it into a
                // solid footprint. Line + ground-ops layers are already continuous.
                if matches!(rows, SurfaceRows::Point(_)) {
                    fill_area_median(&mut cells, AREA_FILL_RADIUS_PX);
                }
                // Interiors are not receivers (Fix 4) — stamped LAST so the
                // area fill can't paint a masked footprint back in.
                if let Some(mask) = &interior_mask {
                    crate::source_loader_obstacle::apply_interior_mask(&mut cells, mask);
                }
                let out = ctx
                    .output
                    .join(*dir_name)
                    .join(ctx.zoom.to_string())
                    .join(x.to_string())
                    .join(format!("{y}.bin"));
                let n = write_tile(&out, &cells, *source_id, !ctx.write_empty)?;
                if n == 0 {
                    // Rebuilt all-silent: unlink any stale tile a prior build left
                    // so combine can't read stale source energy.
                    if out.exists() {
                        std::fs::remove_file(&out)
                            .with_context(|| format!("rm stale {}", out.display()))?;
                    }
                    stats.skipped += 1;
                } else {
                    stats.written += 1;
                    stats.bytes += n;
                }
                // Collapse/fill, Brotli encoding, write and stale-output removal are measured as
                // the complete existing composite, including an all-silent rebuild.
                stats.t_write += t_w.elapsed();
            }
            heartbeat.tick(region_r4);
        }
    }
    Ok(stats)
}
