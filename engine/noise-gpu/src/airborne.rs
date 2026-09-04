//! Region-resident GPU airborne scatter. The production path classifies candidates on the GPU,
//! then runs the shared Doc 29, terrain-horizon, and vector-building physics for each tile's
//! exact pixels and coarse nodes. Production exposes only the screened scatter path.
//!
//! Lifecycle, mirroring `airborne::scatter_tile`'s adaptive near/far split but on the GPU:
//!   1. [`AirborneGpu::new`] — load the PTX + upload the NPD LUTs ONCE (device-global).
//!   2. [`AirborneGpu::upload_region`] (production: SoA packed on the prep thread) or [`load_region`]
//!      (the `e2-airborne` validator: packs on-device) — candidates → device SoA, ONCE per R4.
//!   3. [`AirborneGpu::scatter_region`] — classify, upload terrain, build roof horizons on-device,
//!      run near/far kernels, then host-bilinear-expand to one `TileAccumulator` per tile.

use std::sync::Arc;

use anyhow::{Context, Result};
use cudarc::driver::sys::CUresult;
use cudarc::driver::{CudaDevice, CudaFunction, CudaSlice, DriverError, LaunchAsync, LaunchConfig};
use cudarc::nvrtc::Ptx;
use h3o::CellIndex;
use noise_compute::compute::aircraft_v6::AirborneRowView;
use noise_compute::emission::aircraft::{
    self, is_ground_stale_with_terrain, prepare_segment, NpdLuts, SegmentPrepared, SegmentTerrain,
    AIRCRAFT_MAX_HORIZONTAL_REACH_M, GROUND_CONTEXT_NONE, GROUND_OPS_KIND_NONE, M_PER_DEG_LAT,
};
use noise_compute::propagation::obstacle_index::ObstacleSet;
use noise_compute::types::AircraftSegment;
use raster_reader::fused_grid::FusedGrid;
use raster_reader::fused_tile_z13::{tile_pixel_size_m, FusedTileZ13, TILE_PX};
use rayon::prelude::*;
use tile_painter::accumulator::{CoarseLattice, TileAccumulator, COARSE_LEVELS_N};
use tile_painter::airborne_screening::PackedReceiverScreening;
use tile_painter::source_loader_obstacle::InteriorEstimate;

pub use crate::airborne_building_horizon::AirborneScreeningEnvironment;
use crate::airborne_building_horizon::{AirborneBuildingHorizonGpu, BuildingHorizonDev};
use crate::airborne_screening_bounds::{AirborneScreeningBoundsGpu, ScreeningBoundsScratch};
use crate::airborne_terrain_horizon::{AirborneTerrainHorizonGpu, TerrainHorizonDev};
use crate::{pack_airborne_receivers, pack_airborne_segs};

/// The kernels as an ahead-of-time fatbin (this build's `NOISE_GPU_ARCH` SASS plus its PTX),
/// so the pinned card never JIT-compiles and a driver older than the toolkit still loads it.
const AIRBORNE_FATBIN: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/airborne.fatbin"));
const SCREEN_RECORDS: usize = 0;
const SCREEN_NREG: usize = 1;
const SCREEN_NEAR_BASE: usize = 2;
const SCREEN_NEAR_COUNT: usize = 3;
const SCREEN_FAR0_BASE: usize = 4;
const SCREEN_FAR0_COUNT: usize = 5;
const SCREEN_FAR1_BASE: usize = 6;
const SCREEN_FAR1_COUNT: usize = 7;
const SCREEN_FAR2_BASE: usize = 8;
const SCREEN_FAR2_COUNT: usize = 9;
const SCREEN_RECORD_OF_PIXEL: usize = 10;
const SCREEN_TERRAIN_ENTRIES: usize = 11;
const SCREEN_TERRAIN_MAX_SIN_SQ: usize = 12;
const SCREEN_BUILDING_GLOBAL_MAX_TAN_Q: usize = 13;
const SCREEN_BUILDING_LOCAL_ENTRIES: usize = 14;
const SCREEN_BUILDING_LOCAL_MAX_TAN_Q: usize = 15;
const SCREEN_TABLE_WORDS: usize = 16;
/// (node, part) blocks the coarse kernel aims for per launch; `airborne.cu` derives the
/// same part count from it, so the partial-sum buffer and the grid agree.
const COARSE_TARGET_BLOCKS: usize = 4096;

/// A region whose per-block far-list crosses `i32::MAX` entries — unbuildable in a single GPU pass
/// (it would overflow the device offsets / need ~16 GB of VRAM). Surfaced as a per-cell skip, the
/// same class as a VRAM OOM, NOT an engine abort.
#[derive(Debug)]
pub struct RegionTooDense(pub String);
impl std::fmt::Display for RegionTooDense {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for RegionTooDense {}

/// True when `e` is a per-cell GPU resource limit — a VRAM out-of-memory
/// (`CUDA_ERROR_OUT_OF_MEMORY`) or a [`RegionTooDense`] — i.e. THIS cell can't be built on THIS GPU,
/// so the warm stream engine should report it `fail` and move on. A clean alloc failure leaves the
/// CUDA context intact (cudarc returns `Err` before creating the slice), so the next cell is fine —
/// no `AirborneGpu` rebuild. Any OTHER CUDA error (illegal address, launch / sync failure) is NOT
/// skippable: it signals corrupted device state, so the caller must abort the engine (provision
/// restarts it clean) rather than silently fail every subsequent cell.
pub fn is_cell_unbuildable(e: &anyhow::Error) -> bool {
    e.chain().any(|c| {
        matches!(
            c.downcast_ref::<DriverError>(),
            Some(DriverError(CUresult::CUDA_ERROR_OUT_OF_MEMORY))
        ) || c.downcast_ref::<RegionTooDense>().is_some()
    })
}

/// Region-prep (ONCE per R4): every candidate sub-seg in the region's admit envelope,
/// ground-stale filtered, with `prepare_segment` applied — the expensive CPU work, done
/// region-wide. The per-tile near/far slant gate is deferred to the device classifier.
///
/// The envelope is the R4 hexagon's vertex bbox (a superset of every region tile's centre —
/// `region_tiles` keeps only centre-in-hexagon tiles) padded by the per-tile admit reach:
/// `scatter_tile` admits a sub-seg up to `AIRCRAFT_MAX_HORIZONTAL_REACH_M + half_diag` from a
/// tile centre. Deriving it from the actual R4 geometry — not a fixed radius around one tile —
/// is exact at any latitude: near the equator, tiles at the requested zoom are
/// widest. The worst R4 spans ~52 km centre-to-centre, and a fixed 70 km radius
/// around a corner tile dropped opposite-edge contributors.
pub fn region_candidates(
    views: &[AirborneRowView<'_>],
    r4: u64,
    zoom: u8,
) -> Vec<(SegmentPrepared, u8)> {
    let envelope = RegionEnvelope::new(r4, zoom).expect("valid R4 cell");

    // `prepare_segment` is the live stream's CPU wall. Parallelise the independent Arrow rows,
    // then concatenate their Vecs in input order: Rayon preserves the indexed `par_iter` order,
    // so this produces exactly the same candidate ordering as the bounded serial walker below.
    // Keeping one Vec per row also avoids shared locks/push contention. The one-pass host guard in
    // gpu_airborne::prep budgets the construction peak at 2× the final candidate Vec, which covers
    // these row Vecs plus the final concatenation; megahubs are routed to the bounded walker first.
    views
        .par_iter()
        .map(|view| candidates_for_view(view, &envelope))
        .collect::<Vec<_>>()
        .into_iter()
        .flatten()
        .collect()
}

/// Exact region-wide admit envelope shared by the parallel one-pass builder and the bounded
/// megahub walker. A mismatch here would make ordinary and chunked cells use different physics.
#[derive(Clone, Copy)]
struct RegionEnvelope {
    min_lat: f32,
    max_lat: f32,
    min_lon: f32,
    max_lon: f32,
    prune_lon: bool,
}

impl RegionEnvelope {
    fn new(r4: u64, zoom: u8) -> Result<Self> {
        let cell = CellIndex::try_from(r4).context("valid R4 cell")?;
        let (mut s, mut n, mut w, mut e) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
        for ll in cell.boundary().iter() {
            s = s.min(ll.lat());
            n = n.max(ll.lat());
            w = w.min(ll.lng());
            e = e.max(ll.lng());
        }
        // Pad = max horizontal reach + max tile half-diagonal. Size half_diag at
        // the equatorial tile at the requested zoom (widest, cos = 1) so it
        // bounds every tile in the region; the longitude pad uses the maximum
        // region |lat| (most degrees per metre). Over-padding only adds
        // one-time candidates that `classify_tile` rejects per tile;
        // under-padding silently drops them.
        let half_diag =
            (TILE_PX as f64) * tile_pixel_size_m(zoom, 0.0) * std::f64::consts::SQRT_2 * 0.5;
        let pad_m = AIRCRAFT_MAX_HORIZONTAL_REACH_M + half_diag;
        let pad_lat = aircraft::meters_to_lat_deg(pad_m);
        let pad_lon = aircraft::meters_to_lon_deg(s.abs().max(n.abs()), pad_m);
        // Antimeridian: a dateline R4's vertices straddle ±180, so [w,e] is the long way round —
        // disable the lon prune (lat alone still culls; mirrors `region_tiles`/`scatter_tile`).
        let prune_lon = e - w <= 180.0 && (w - pad_lon) >= -180.0 && (e + pad_lon) <= 180.0;
        Ok(Self {
            min_lat: (s - pad_lat) as f32,
            max_lat: (n + pad_lat) as f32,
            min_lon: (w - pad_lon) as f32,
            max_lon: (e + pad_lon) as f32,
            prune_lon,
        })
    }

    fn includes(self, view: &AirborneRowView<'_>) -> bool {
        let bb = &view.bbox;
        bb.max_lat >= self.min_lat
            && bb.min_lat <= self.max_lat
            && (!self.prune_lon || (bb.max_lon >= self.min_lon && bb.min_lon <= self.max_lon))
    }
}

/// Prepare one sub-segment after the shared terrain-staleness gate. Both candidate walkers call
/// this function, keeping AircraftSegment reconstruction and `prepare_segment` in one place.
fn prepare_candidate(view: &AirborneRowView<'_>, i: usize) -> Option<(SegmentPrepared, u8)> {
    let ss = &view.sub_segments;
    let seg = AircraftSegment {
        flight_id: view.flight_id,
        profile_idx: view.profile_idx,
        is_departure: ss.flags[i] & 0b001 != 0,
        on_ground: false,
        period: ss.period[i],
        date_id: ss.date_id[i],
        start_lat: ss.start_lat[i] as f64,
        start_lon: ss.start_lon[i] as f64,
        start_alt_m: ss.start_alt_m[i],
        end_lat: ss.end_lat[i] as f64,
        end_lon: ss.end_lon[i] as f64,
        end_alt_m: ss.end_alt_m[i],
        speed_kt: ss.speed_kt[i],
        segment_length_m: ss.length_m[i],
        count_weight: 1.0,
        surface_model: false,
        ground_context: GROUND_CONTEXT_NONE,
        ground_ops_kind: GROUND_OPS_KIND_NONE,
        source_id: view.source_id as u16,
    };
    let start_elev = ss.terrain_start_elev_m[i] as f64;
    let end_elev = ss.terrain_end_elev_m[i] as f64;
    let terrain = SegmentTerrain {
        start_elev,
        q1_elev: 0.0,
        mid_elev: 0.0,
        q3_elev: 0.0,
        end_elev,
    };
    if is_ground_stale_with_terrain(&seg, &terrain) {
        return None;
    }
    Some((
        prepare_segment(&seg, start_elev - 30.0, end_elev - 30.0),
        seg.period,
    ))
}

fn candidates_for_view(
    view: &AirborneRowView<'_>,
    envelope: &RegionEnvelope,
) -> Vec<(SegmentPrepared, u8)> {
    if !envelope.includes(view) {
        return Vec::new();
    }
    (0..view.sub_segments.start_lat.len())
        .filter_map(|i| prepare_candidate(view, i))
        .collect()
}

/// Build the region's candidate sub-segs in BOUNDED chunks, invoking `f` once per chunk of at most
/// `max_per_chunk` candidates (passed BY VALUE so the caller packs+uploads it, then the buffer is
/// reused for the next chunk → host RAM stays one chunk, not the whole Vec). Same envelope +
/// ground-stale filter + `prepare_segment` helpers as [`region_candidates`] — so chunked and
/// one-pass classify the SAME candidates; summing each chunk's per-tile energy
/// (`TileAccumulator::merge_from`, additive in the linear domain) reconstructs the one-pass result.
/// This is M2: the ~5 densest megahub cells whose full candidate Vec is tens of GB (Phoenix:
/// 308M subsegs ≈ 78 GiB host / >24 GB VRAM) build in VRAM/host-sized passes on ANY card, instead
/// of OOM-crashing or being `RegionTooDense`-skipped. `f`'s error aborts the walk.
pub fn for_each_region_chunk(
    views: &[AirborneRowView<'_>],
    r4: u64,
    zoom: u8,
    max_per_chunk: usize,
    mut f: impl FnMut(Vec<(SegmentPrepared, u8)>) -> Result<()>,
) -> Result<()> {
    let envelope = RegionEnvelope::new(r4, zoom)?;

    // Pre-size the buffer to a bounded chunk (so a pass fills without re-allocating); an unbounded
    // diagnostic caller grows from small — NOT a huge pre-allocation on every tiny cell.
    let cap = if max_per_chunk >= (1 << 28) {
        4096
    } else {
        max_per_chunk
    };
    let mut buf: Vec<(SegmentPrepared, u8)> = Vec::with_capacity(cap);
    for v in views {
        if !envelope.includes(v) {
            continue;
        }
        for i in 0..v.sub_segments.start_lat.len() {
            if let Some(prepared) = prepare_candidate(v, i) {
                buf.push(prepared);
                if buf.len() >= max_per_chunk {
                    f(std::mem::replace(&mut buf, Vec::with_capacity(cap)))?;
                }
            }
        }
    }
    if !buf.is_empty() {
        f(buf)?;
    }
    Ok(())
}

/// GPU handle: the device, airborne kernels, the NPD LUTs, and the
/// GA full-year hybrid per-class weight LUT (all uploaded once, device-global).
/// Construct once per build; reuse across every region and tile.
pub struct AirborneGpu {
    dev: Arc<CudaDevice>,
    f_near_screened: CudaFunction,
    f_coarse_screened: CudaFunction,
    /// GPU classify (counting-sort the per-tile near/far gate on device) → device-built CSR.
    f_classify_count: CudaFunction,
    f_classify_chunk_offsets: CudaFunction,
    f_classify_scatter: CudaFunction,
    f_coarse_reduce_parts: CudaFunction,
    terrain_horizon: AirborneTerrainHorizonGpu,
    building_horizon: AirborneBuildingHorizonGpu,
    screening_bounds: AirborneScreeningBoundsGpu,
    d_npd: CudaSlice<f32>,
    /// `NUM_CLASSES`-length GA hybrid weight LUT (f32). The kernel scales
    /// each sub-segment's energy by `d_w[class]`.
    d_w: CudaSlice<f32>,
    /// Total VRAM (bytes) of this device, queried once at open — the M2 chunked build derives its
    /// candidate-chunk size from it (no hand-set chunk knob; see `gpu_airborne::max_candidates_per_chunk`).
    vram_total: u64,
}

/// One R4's candidate sub-segments resident on the device.
pub struct RegionResident {
    d_sll: CudaSlice<f64>,
    d_sf: CudaSlice<f32>,
    d_si: CudaSlice<i32>,
    nreg: usize,
}

struct ReceiverScreeningDev {
    table: CudaSlice<u64>,
    _record_of_pixel: CudaSlice<u32>,
    _terrain: TerrainHorizonDev,
    _buildings: BuildingHorizonDev,
}

impl RegionResident {
    /// Number of resident candidate sub-segs (the classify indexes into these).
    pub fn len(&self) -> usize {
        self.nreg
    }
    pub fn is_empty(&self) -> bool {
        self.nreg == 0
    }
}

// No `Default`: `new()` opens a CUDA device, compiles/loads the PTX, and uploads the NPD LUTs —
// a `Default::default()` would silently hide all that I/O.
#[allow(clippy::new_without_default)]
impl AirborneGpu {
    /// Open CUDA device 0, load the airborne PTX, and upload the NPD LUTs + the GA full-year
    /// hybrid per-class weight LUT once. CUDA failures `expect`-panic (the codebase convention
    /// — see the surface runner): a dead device or missing kernel is fatal to the whole build, so
    /// the worker dies loudly and the chunk re-dispatches. `class_weights` is build-wide
    /// (resolved from the source arrows' `sample_days_by_class`); the weight LUT is constant
    /// across every region + tile, so it uploads here, not per-tile.
    pub fn new(class_weights: &aircraft::ClassWeights) -> Self {
        // `new_with_stream` (not `new`): every alloc/copy/launch/synchronize then runs on
        // THIS instance's OWN stream, not the shared default/null stream. The world builder
        // makes one AirborneGpu per rayon worker, so per-worker streams let the workers' GPU
        // launches + copies overlap instead of serializing device-wide on the null stream
        // across workers. Each worker holds its own CudaDevice instance ⇒ no shared-event hazard.
        let dev = CudaDevice::new_with_stream(0).expect("open cuda device 0");
        // cudarc 0.12 loads a binary image only through `cuModuleLoad` on a path (`from_src`
        // takes NUL-free PTX text), so the embedded fatbin goes through a per-process temp file.
        let fatbin_path =
            std::env::temp_dir().join(format!("quietmap-airborne-{}.fatbin", std::process::id()));
        std::fs::write(&fatbin_path, AIRBORNE_FATBIN).expect("write airborne fatbin");
        let loaded = dev.load_ptx(
            Ptx::from_file(&fatbin_path),
            "air",
            &[
                "airborne_exact_screened",
                "airborne_coarse_screened",
                "airborne_coarse_reduce_parts",
                "airborne_classify_count",
                "airborne_classify_chunk_offsets",
                "airborne_classify_scatter",
                "airborne_terrain_sample_tables",
                "airborne_terrain_horizon_build",
                "airborne_terrain_horizon_global_max",
                "airborne_terrain_horizon_range_quantization_probe",
                "airborne_building_horizon_build",
                "airborne_building_horizon_pack",
                "airborne_building_horizon_global_max",
                "airborne_building_horizon_mark_empty",
                "airborne_dem_pyramid_level0",
                "airborne_dem_pyramid_reduce",
                "airborne_lowest_source_tangent",
                "airborne_screening_floor",
                "airborne_building_cell_tops",
            ],
        );
        let _ = std::fs::remove_file(&fatbin_path);
        loaded.expect("load airborne fatbin");
        let f_near_screened = dev
            .get_func("air", "airborne_exact_screened")
            .expect("fn near_screened");
        let f_coarse_screened = dev
            .get_func("air", "airborne_coarse_screened")
            .expect("fn coarse_screened");
        let f_classify_count = dev
            .get_func("air", "airborne_classify_count")
            .expect("fn classify_count");
        let f_classify_chunk_offsets = dev
            .get_func("air", "airborne_classify_chunk_offsets")
            .expect("fn classify_chunk_offsets");
        let f_classify_scatter = dev
            .get_func("air", "airborne_classify_scatter")
            .expect("fn classify_scatter");
        let f_coarse_reduce_parts = dev
            .get_func("air", "airborne_coarse_reduce_parts")
            .expect("fn coarse_reduce_parts");
        let terrain_horizon = AirborneTerrainHorizonGpu::new(Arc::clone(&dev));
        let building_horizon = AirborneBuildingHorizonGpu::new(Arc::clone(&dev));
        let screening_bounds = AirborneScreeningBoundsGpu::new(Arc::clone(&dev));
        let d_npd = dev
            .htod_copy(NpdLuts::shared().sel_luts_flat_f32())
            .expect("upload npd");
        let d_w = dev
            .htod_copy(
                class_weights
                    .as_array()
                    .iter()
                    .map(|&x| x as f32)
                    .collect::<Vec<f32>>(),
            )
            .expect("upload class weights");
        dev.synchronize().expect("npd + weights upload sync");
        // Total VRAM, queried once (the context is current after `new_with_stream`) — the M2 chunked
        // build derives its candidate-chunk size from it. Fall back to the 11 GB fleet floor if the
        // query fails, so the chunk is always sized conservatively.
        let vram_total = cudarc::driver::result::mem_get_info()
            .map(|(_free, total)| total as u64)
            .unwrap_or(11 << 30);
        Self {
            dev,
            f_near_screened,
            f_coarse_screened,
            f_classify_count,
            f_classify_chunk_offsets,
            f_classify_scatter,
            f_coarse_reduce_parts,
            terrain_horizon,
            building_horizon,
            screening_bounds,
            d_npd,
            d_w,
            vram_total,
        }
    }

    /// Total VRAM (bytes) of this device (queried once at open). The M2 chunked build sizes its
    /// candidate chunk from this — a bigger card takes fewer passes — mirroring how
    /// `default_batch_size` derives the tile batch from L3, so there's no hand-set chunk knob.
    pub fn vram_total_bytes(&self) -> u64 {
        self.vram_total
    }

    /// Run the CUDA range-packing acceptance probe. It exercises the same
    /// packed terrain query used by production screening with a source placed
    /// between the true edge and the old nearest-range value.
    pub fn terrain_range_quantization_probe(
        &self,
        true_range_m: f32,
        source_range_m: f32,
    ) -> Result<f32> {
        self.terrain_horizon
            .range_quantization_probe(true_range_m, source_range_m)
    }

    /// Build one tile's receiver horizons and the pointer table the physics kernels read.
    /// The height-conditioned screening inputs come first: they tell the roof scan which
    /// obstacle cells are too low to shade the aircraft seen in their direction.
    #[allow(clippy::too_many_arguments)]
    fn upload_receiver_screening(
        &self,
        packed: &PackedReceiverScreening,
        environment: &AirborneScreeningEnvironment,
        bounds_scratch: &mut ScreeningBoundsScratch,
        tile: &FusedTileZ13,
        region: &RegionResident,
        near_idx: &CudaSlice<i32>,
        receiver_lat_lon: &CudaSlice<f64>,
        receiver_altitude: &CudaSlice<f32>,
        inner_elevation: &CudaSlice<f32>,
        tile_bbox: &CudaSlice<f64>,
        near: (usize, usize),
        far: [(usize, usize); 3],
    ) -> Result<ReceiverScreeningDev> {
        use cudarc::driver::DevicePtr;

        let nreg = region.nreg;
        let record_of_pixel = self
            .dev
            .htod_sync_copy(&packed.record_of_pixel)
            .context("screen pixel records")?;
        let pixel_of_record = self
            .dev
            .htod_copy(packed.pixel_of_record.clone())
            .context("screen record pixels")?;
        let bounds = self.screening_bounds.build(
            environment,
            bounds_scratch,
            tile,
            packed,
            &pixel_of_record,
            receiver_lat_lon,
            receiver_altitude,
            inner_elevation,
            tile_bbox,
            &region.d_sll,
            &region.d_sf,
            near_idx,
            nreg,
            near,
        )?;
        let terrain = self.terrain_horizon.build(
            environment,
            packed,
            &pixel_of_record,
            receiver_lat_lon,
            receiver_altitude,
            inner_elevation,
            tile_bbox,
        )?;
        let buildings = self.building_horizon.build(
            environment,
            packed,
            &pixel_of_record,
            receiver_lat_lon,
            receiver_altitude,
            inner_elevation,
            tile_bbox,
            &bounds,
        )?;
        let mut host_table = [0u64; SCREEN_TABLE_WORDS];
        host_table[SCREEN_RECORDS] = packed.records as u64;
        host_table[SCREEN_NREG] = nreg as u64;
        host_table[SCREEN_NEAR_BASE] = near.0 as u64;
        host_table[SCREEN_NEAR_COUNT] = near.1 as u64;
        host_table[SCREEN_FAR0_BASE] = far[0].0 as u64;
        host_table[SCREEN_FAR0_COUNT] = far[0].1 as u64;
        host_table[SCREEN_FAR1_BASE] = far[1].0 as u64;
        host_table[SCREEN_FAR1_COUNT] = far[1].1 as u64;
        host_table[SCREEN_FAR2_BASE] = far[2].0 as u64;
        host_table[SCREEN_FAR2_COUNT] = far[2].1 as u64;
        host_table[SCREEN_RECORD_OF_PIXEL] = *record_of_pixel.device_ptr();
        host_table[SCREEN_TERRAIN_ENTRIES] = *terrain.entries.device_ptr();
        host_table[SCREEN_TERRAIN_MAX_SIN_SQ] = *terrain.max_sin_sq.device_ptr();
        host_table[SCREEN_BUILDING_GLOBAL_MAX_TAN_Q] =
            *buildings.global_max_tangent_bits.device_ptr();
        host_table[SCREEN_BUILDING_LOCAL_ENTRIES] = *buildings.local_entries.device_ptr();
        host_table[SCREEN_BUILDING_LOCAL_MAX_TAN_Q] =
            *buildings.local_max_tangent_bits.device_ptr();
        let table = self
            .dev
            .htod_copy(host_table.to_vec())
            .context("screen pointer table")?;
        Ok(ReceiverScreeningDev {
            table,
            _record_of_pixel: record_of_pixel,
            _terrain: terrain,
            _buildings: buildings,
        })
    }

    /// Upload the region's obstacle CSR and its shared DEM halo once for every
    /// tile block scattered against that cell.
    pub fn upload_screening_environment(
        &self,
        obstacles: &ObstacleSet,
        halo: &FusedGrid,
    ) -> Result<AirborneScreeningEnvironment> {
        self.building_horizon.upload_environment(obstacles, halo)
    }

    /// Pack + upload a region's candidate sub-segments to the device once per R4.
    pub fn load_region(&self, region: Vec<(SegmentPrepared, u8)>) -> Result<RegionResident> {
        let (sll, sf, si) = pack_airborne_segs(&region);
        let nreg = region.len();
        self.upload_region(sll, sf, si, nreg)
    }

    /// Upload a pre-packed region SoA. The pipeline packs on the CPU prep thread so only this
    /// transfer touches the device, letting prep of the next cell overlap this cell's build.
    pub fn upload_region(
        &self,
        sll: Vec<f64>,
        sf: Vec<f32>,
        si: Vec<i32>,
        nreg: usize,
    ) -> Result<RegionResident> {
        let d_sll = self.dev.htod_copy(sll).context("upload sll")?;
        let d_sf = self.dev.htod_copy(sf).context("upload sf")?;
        let d_si = self.dev.htod_copy(si).context("upload si")?;
        self.dev.synchronize().context("region upload sync")?;
        Ok(RegionResident {
            d_sll,
            d_sf,
            d_si,
            nreg,
        })
    }

    /// Screened production scatter over a block of tiles. Candidate classification remains one
    /// device counting-sort for the whole block; each tile builds its terrain and vector-building
    /// horizons on-device, then runs the screened exact/coarse kernels.
    ///   1. host packs a tiny per-tile `meta` (centre, m/deg lon, half-diag, max rx alt);
    ///   2. `airborne_classify_count` → per-(tile,bucket) counts (only tile×4 ints cross PCIe);
    ///   3. host prefix-sums counts → near CSR + far base offsets;
    ///   4. `airborne_classify_scatter` fills near_idx + far (seg,tile) lists on device;
    ///   5. build only the receiver horizons that tile's exact/coarse paths read;
    ///   6. `airborne_exact_screened` + ≤3 `airborne_coarse_screened`, then host expansion.
    ///
    /// Returns one `TileAccumulator` per input tile, in order. Same `airborne_sel` physics and
    /// same gate as the CPU `scatter_tile`, so parity holds by construction (dB-level: the
    /// device atomic-scatter order and f32 gate inputs admit ≪0.5 dB run-to-run jitter).
    pub fn scatter_region(
        &self,
        region: &RegionResident,
        tiles: &[&FusedTileZ13],
        obstacles: &ObstacleSet,
        interiors: &[InteriorEstimate],
    ) -> Result<Vec<TileAccumulator>> {
        if region.nreg == 0 || tiles.is_empty() {
            return Ok((0..tiles.len()).map(|_| TileAccumulator::new()).collect());
        }
        let halo = &tiles[0].halo;
        assert!(
            tiles.iter().all(|tile| Arc::ptr_eq(&tile.halo, halo)),
            "one airborne scatter call must share one elevation halo"
        );
        let environment = self.upload_screening_environment(obstacles, halo)?;
        self.scatter_region_with_environment(region, tiles, &environment, interiors)
    }

    /// Scatter with a previously uploaded region environment. The production
    /// builder uses this entry point across all grid-aligned tile blocks.
    pub fn scatter_region_with_environment(
        &self,
        region: &RegionResident,
        tiles: &[&FusedTileZ13],
        environment: &AirborneScreeningEnvironment,
        interiors: &[InteriorEstimate],
    ) -> Result<Vec<TileAccumulator>> {
        let t = tiles.len();
        assert_eq!(
            interiors.len(),
            t,
            "one interior estimate per airborne tile"
        );
        if region.nreg == 0 || t == 0 {
            return Ok((0..t).map(|_| TileAccumulator::new()).collect());
        }
        let nreg = region.nreg;
        let nreg_i = nreg as i32;
        let npix = TILE_PX * TILE_PX;
        let block: u32 = 256;

        // 1. Per-tile classify meta (5 f64): centre lat/lon, m_per_deg_lon, half-diag, max rx
        //    elevation. The alt-max is O(npix) per tile — negligible vs the O(nreg) gate it feeds.
        let mut meta = Vec::with_capacity(5 * t);
        for &tile in tiles {
            let b = &tile.bbox;
            let centre_lat = (b.north_lat + b.south_lat) * 0.5;
            let centre_lon = (b.east_lon + b.west_lon) * 0.5;
            let px_m = tile_pixel_size_m(tile.zoom, centre_lat);
            let half_diag = (TILE_PX as f64) * px_m * std::f64::consts::SQRT_2 * 0.5;
            let m_per_deg_lon = M_PER_DEG_LAT * centre_lat.to_radians().cos().max(0.2);
            let max_rx_alt = tile
                .rx_alt_m
                .iter()
                .copied()
                .fold(f32::NEG_INFINITY, f32::max) as f64;
            meta.extend_from_slice(&[centre_lat, centre_lon, m_per_deg_lon, half_diag, max_rx_alt]);
        }
        let d_meta = self.dev.htod_copy(meta).context("meta")?;

        // 2. Pass 1 (count): one block per (tile, chunk of CLASSIFY_CHUNK segs) → per-chunk
        //    bucket counts plus counts[tile*4 + bucket]. Slots are then ranks in seg order, so
        //    the lists — and every f32 sum over them — come out the same bits every run.
        const CLASSIFY_CHUNK: usize = 512;
        let nchunks = nreg.div_ceil(CLASSIFY_CHUNK);
        let cfg_classify = LaunchConfig {
            grid_dim: ((t * nchunks) as u32, 1, 1),
            block_dim: (CLASSIFY_CHUNK as u32, 1, 1),
            shared_mem_bytes: 0,
        };
        let mut d_chunk_counts = self
            .dev
            .alloc_zeros::<i32>(t * 4 * nchunks)
            .context("chunk counts")?;
        let mut d_counts = self.dev.alloc_zeros::<i32>(t * 4).context("counts")?;
        unsafe {
            self.f_classify_count
                .clone()
                .launch(
                    cfg_classify,
                    (
                        &d_meta,
                        &region.d_sll,
                        &region.d_sf,
                        nreg_i,
                        t as i32,
                        nchunks as i32,
                        &mut d_chunk_counts,
                        &mut d_counts,
                    ),
                )
                .context("launch classify_count")?;
            self.f_classify_chunk_offsets
                .clone()
                .launch(
                    LaunchConfig {
                        grid_dim: (((t * 4) as u32).div_ceil(block), 1, 1),
                        block_dim: (block, 1, 1),
                        shared_mem_bytes: 0,
                    },
                    (t as i32, nchunks as i32, &mut d_chunk_counts),
                )
                .context("launch classify_chunk_offsets")?;
        }
        self.dev.synchronize().context("count sync")?;
        let counts = self.dev.dtoh_sync_copy(&d_counts).context("dtoh counts")?;

        // 3. Prefix-sum → off[4*(t+1)]: block 0 = near CSR, blocks 1/2/3 = per-tile
        //    far-level base offsets. Totals size the buffers.
        //    Accumulate in i64 and assert each block-wide total stays < i32::MAX: that keeps
        //    every device-side offset / scatter `pos` / far-entry `nfar` (all i32) sound — a
        //    dense megaregion whose far[2] list crosses 2^31 entries (and ~16 GB of VRAM) is what the
        //    M2 chunked build handles — failing loudly HERE is its trigger: `is_cell_unbuildable`
        //    catches this `RegionTooDense` and routes the cell to `gpu_build_cell_chunked` (VRAM-sized
        //    passes, additive accumulation). Never silently wrap an i32 into an OOB write.
        let t1 = t + 1;
        let mut off = vec![0i32; 4 * t1];
        let mut total = [0usize; 4];
        for bucket in 0..4 {
            let mut acc: i64 = 0;
            for ti in 0..t {
                off[bucket * t1 + ti] = acc as i32;
                acc += i64::from(counts[ti * 4 + bucket]);
            }
            off[bucket * t1 + t] = acc as i32;
            if acc >= i32::MAX as i64 {
                // Surfaced as a per-cell skip (see `is_cell_unbuildable`), not a panic: the warm
                // stream engine reports this cell `fail` and moves on instead of aborting.
                return Err(RegionTooDense(format!(
                    "airborne block bucket {bucket} total {acc} ≥ i32::MAX — region too dense for \
                     single-pass GPU (would overflow device offsets / ~16 GB VRAM)"
                ))
                .into());
            }
            total[bucket] = acc as usize;
        }
        let d_off = self.dev.htod_sync_copy(&off).context("off")?;

        // 4. Pass 2 (scatter): fill near_idx + per-level far (seg,tile) lists on device.
        //    alloc_zeros rejects 0 bytes → dummy-size empty buckets; the kernel never writes them.
        let mut d_near_idx = self
            .dev
            .alloc_zeros::<i32>(total[0].max(1))
            .context("near_idx")?;
        let mut d_far0 = self
            .dev
            .alloc_zeros::<i32>((2 * total[1]).max(2))
            .context("far0")?;
        let mut d_far1 = self
            .dev
            .alloc_zeros::<i32>((2 * total[2]).max(2))
            .context("far1")?;
        let mut d_far2 = self
            .dev
            .alloc_zeros::<i32>((2 * total[3]).max(2))
            .context("far2")?;
        unsafe {
            self.f_classify_scatter
                .clone()
                .launch(
                    cfg_classify,
                    (
                        &d_meta,
                        &region.d_sll,
                        &region.d_sf,
                        &d_off,
                        &d_chunk_counts,
                        nreg_i,
                        t as i32,
                        nchunks as i32,
                        &mut d_near_idx,
                        &mut d_far0,
                        &mut d_far1,
                        &mut d_far2,
                    ),
                )
                .context("launch classify_scatter")?;
        }

        // 5. Receiver horizons and screened physics. Terrain and roof horizons are constructed
        //    from the shared DEM halo and region-resident vector CSR on-device.
        //    The classify scatter precedes every following copy/launch on this GPU's stream.
        let far_device = [&d_far0, &d_far1, &d_far2];
        let mut bounds_scratch = self.screening_bounds.scratch(environment)?;
        let mut output = Vec::with_capacity(t);
        for (ti, &tile) in tiles.iter().enumerate() {
            let near = (off[ti] as usize, counts[ti * 4] as usize);
            let far: [(usize, usize); 3] = std::array::from_fn(|level| {
                (
                    off[(level + 1) * t1 + ti] as usize,
                    counts[ti * 4 + level + 1] as usize,
                )
            });
            let (rll, rxa) = pack_airborne_receivers(tile);
            let d_rll = self.dev.htod_copy(rll).context("screened rll")?;
            let d_rxa = self.dev.htod_copy(rxa).context("screened rxa")?;
            let d_inner_elevation = self
                .dev
                .htod_copy(tile.inner_elev_m.clone())
                .context("screened inner elevation")?;
            let d_tile_bbox = self
                .dev
                .htod_copy(vec![
                    tile.bbox.south_lat,
                    tile.bbox.north_lat,
                    tile.bbox.west_lon,
                    tile.bbox.east_lon,
                ])
                .context("screened tile bbox")?;
            let packed = PackedReceiverScreening::select(&interiors[ti], near.1 > 0);
            let screening = self.upload_receiver_screening(
                &packed,
                environment,
                &mut bounds_scratch,
                tile,
                region,
                &d_near_idx,
                &d_rll,
                &d_rxa,
                &d_inner_elevation,
                &d_tile_bbox,
                near,
                far,
            )?;
            drop(packed);
            let mut d_near = self
                .dev
                .alloc_zeros::<f32>(npix * 3)
                .context("screened exact output")?;
            // EXACT_BLOCK threads per block (airborne.cu): a block must stay inside one row.
            let cfg_near = LaunchConfig {
                grid_dim: ((npix as u32).div_ceil(256), 1, 1),
                block_dim: (256, 1, 1),
                shared_mem_bytes: 0,
            };
            unsafe {
                self.f_near_screened
                    .clone()
                    .launch(
                        cfg_near,
                        (
                            &d_rll,
                            &d_rxa,
                            &region.d_sll,
                            &region.d_sf,
                            &region.d_si,
                            &self.d_npd,
                            &self.d_w,
                            &d_near_idx,
                            &screening.table,
                            &mut d_near,
                        ),
                    )
                    .context("launch screened exact")?;
            }

            let mut coarse_device: Vec<(usize, CudaSlice<f32>)> = Vec::with_capacity(3);
            for level in 0..3 {
                if far[level].1 == 0 {
                    continue;
                }
                let nn = COARSE_LEVELS_N[level];
                let nodes = nn * nn;
                // One COARSE_BLOCK block per (lattice row, part): enough blocks to fill the
                // device even on the 5×5 lattice, each part a fixed range of the far list.
                let parts = COARSE_TARGET_BLOCKS.div_ceil(nn);
                let mut d_partial = self
                    .dev
                    .alloc_zeros::<f32>(nodes * parts * 3)
                    .context("screened coarse partial sums")?;
                let mut d_coarse = self
                    .dev
                    .alloc_zeros::<f32>(nodes * 3)
                    .context("screened coarse output")?;
                unsafe {
                    self.f_coarse_screened
                        .clone()
                        .launch(
                            LaunchConfig {
                                grid_dim: ((nn * parts) as u32, 1, 1),
                                block_dim: (256, 1, 1),
                                shared_mem_bytes: 0,
                            },
                            (
                                &d_rll,
                                &d_rxa,
                                &region.d_sll,
                                &region.d_sf,
                                &region.d_si,
                                &self.d_npd,
                                &self.d_w,
                                far_device[level],
                                level as i32,
                                nn as i32,
                                &screening.table,
                                &mut d_partial,
                            ),
                        )
                        .context("launch screened coarse")?;
                    self.f_coarse_reduce_parts
                        .clone()
                        .launch(
                            LaunchConfig {
                                grid_dim: (((nodes * 3) as u32).div_ceil(block), 1, 1),
                                block_dim: (block, 1, 1),
                                shared_mem_bytes: 0,
                            },
                            (&d_partial, nodes as i32, parts as i32, &mut d_coarse),
                        )
                        .context("launch coarse reduce")?;
                }
                coarse_device.push((nn, d_coarse));
            }
            self.dev.synchronize().context("screened kernel sync")?;

            let near_energy = self
                .dev
                .dtoh_sync_copy(&d_near)
                .context("dtoh screened exact")?;
            let mut fine = TileAccumulator::new();
            fine.energy.copy_from_slice(&near_energy);
            for (nn, d_coarse) in &coarse_device {
                let coarse = self
                    .dev
                    .dtoh_sync_copy(d_coarse)
                    .context("dtoh screened coarse")?;
                CoarseLattice::from_energy(*nn, coarse).expand_into(&mut fine);
            }
            output.push(fine);
        }
        Ok(output)
    }
}
