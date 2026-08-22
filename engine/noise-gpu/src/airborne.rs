//! Region-resident GPU airborne scatter. Two entry points share the `airborne_sel` physics:
//!   - [`AirborneGpu::scatter_tile`] — per-tile, CPU classify → near (exact per-pixel) + far
//!     (coarse lattice). The `e2-airborne` validator checks THIS against the CPU
//!     `airborne::scatter_tile` (byte-exact: LKPR 0.0090 dB, LOWI 0.0065 dB, 0 zero-sided).
//!   - [`AirborneGpu::scatter_region`] — the production builder's path (M3+M4): GPU classify
//!     (counting-sort) + batched near/coarse over a whole tile-block. Same gate + physics, so
//!     parity holds at the dB level (compare_hm3 over Ruzyně: 0 cells > 0.5 dB) — but NOT
//!     byte-identical run-to-run: the device atomic-scatter cursors order the near/far lists
//!     nondeterministically, injecting ≪0.5 dB float-reduction jitter.
//!
//! Lifecycle, mirroring `airborne::scatter_tile`'s adaptive near/far split but on the GPU:
//!   1. [`AirborneGpu::new`] — load the PTX + upload the NPD LUTs ONCE (device-global).
//!   2. [`AirborneGpu::upload_region`] (production: SoA packed on the prep thread) or [`load_region`]
//!      (the `e2-airborne` validator: packs on-device) — candidates → device SoA, ONCE per R4.
//!   3. [`AirborneGpu::scatter_region`] (production, whole tile-block) / [`AirborneGpu::scatter_tile`]
//!      (validator, per-tile byte-exact reference) — classify → near +
//!      far kernels → host bilinear expand → one `TileAccumulator` per tile.

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
use noise_compute::types::AircraftSegment;
use raster_reader::fused_tile_z13::{tile_pixel_size_m, FusedTileZ13, TILE_PX};
use rayon::prelude::*;
use tile_painter::accumulator::{CoarseLattice, TileAccumulator, COARSE_LEVELS_N};

use crate::{pack_airborne_receivers, pack_airborne_receivers_batch, pack_airborne_segs};

const AIRBORNE_PTX: &str = include_str!(concat!(env!("OUT_DIR"), "/airborne.ptx"));
const NEAR_SLANT_SQ: f64 = 500.0 * 500.0; // NEAR_SLANT_M² (airborne.rs:48)
const COARSE_BAND_M: [f64; 2] = [2_000.0, 8_000.0]; // airborne.rs:96

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
/// region-wide. The per-tile near/far slant gate is deferred to [`classify_tile`].
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

/// Per-tile classify (CHEAP — no prepare_segment): which region candidates are near / far[level]
/// for THIS tile. The slant gate subsumes the per-tile envelope + clamped-CPA (slant-pass ⟹
/// clamped-pass), so it reproduces `scatter_tile`'s admit exactly. Emits index lists into the
/// region SoA. Reads the prepared `d_lon`/`sdy` directly — no AircraftSegment / endpoint rebuild.
fn classify_tile(
    tile: &FusedTileZ13,
    region: &[(SegmentPrepared, u8)],
) -> (Vec<i32>, [Vec<i32>; 3]) {
    let b = &tile.bbox;
    let centre_lat = (b.north_lat + b.south_lat) * 0.5;
    let centre_lon = (b.east_lon + b.west_lon) * 0.5;
    let px_m = tile_pixel_size_m(tile.zoom, centre_lat);
    let half_diag = (TILE_PX as f64) * px_m * std::f64::consts::SQRT_2 * 0.5;
    let m_per_deg_lon = M_PER_DEG_LAT * centre_lat.to_radians().cos().max(0.2);
    let tile_max_rx_alt = tile
        .rx_alt_m
        .iter()
        .copied()
        .fold(f32::NEG_INFINITY, f32::max) as f64;

    let mut near = Vec::new();
    let mut far: [Vec<i32>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for (idx, (p, _)) in region.iter().enumerate() {
        // dx,dy expand exactly to d_lon·m and sdy (x2−x1, y2−y1 in `scatter_tile`,
        // airborne.rs:228) — skip rebuilding the endpoint, which only added an f64 roundtrip.
        let x1 = (p.start_lon - centre_lon) * m_per_deg_lon;
        let y1 = (p.start_lat - centre_lat) * M_PER_DEG_LAT;
        let dx = p.d_lon * m_per_deg_lon;
        let dy = p.sdy;
        let len_sq = dx * dx + dy * dy;
        let min_d_sq = if len_sq < 1.0 {
            x1 * x1 + y1 * y1
        } else {
            let t_num = -(x1 * dx + y1 * dy);
            if t_num <= 0.0 {
                x1 * x1 + y1 * y1
            } else if t_num >= len_sq {
                (x1 + dx) * (x1 + dx) + (y1 + dy) * (y1 + dy)
            } else {
                let cross = dx * y1 - dy * x1;
                (cross * cross) / len_sq
            }
        };
        let horiz = (min_d_sq.sqrt() - half_diag).max(0.0);
        let seg_min_alt = p.start_alt_m.min(p.start_alt_m + p.sdz);
        let rel_alt = (seg_min_alt - tile_max_rx_alt).max(0.0);
        let best_slant_sq = horiz * horiz + rel_alt * rel_alt;
        if best_slant_sq > p.reach_sq {
            continue;
        }
        if best_slant_sq < NEAR_SLANT_SQ {
            near.push(idx as i32);
        } else {
            let best_slant = best_slant_sq.sqrt();
            let lvl = if best_slant < COARSE_BAND_M[0] {
                0
            } else if best_slant < COARSE_BAND_M[1] {
                1
            } else {
                2
            };
            far[lvl].push(idx as i32);
        }
    }
    (near, far)
}

/// GPU handle: the device, the two airborne kernels, the NPD LUTs, and the
/// GA full-year hybrid per-class weight LUT (all uploaded once, device-global).
/// Construct once per build; reuse across every region and tile.
pub struct AirborneGpu {
    dev: Arc<CudaDevice>,
    f_near: CudaFunction,
    f_coarse: CudaFunction,
    /// M3 batched variants (one launch per block of tiles); same physics as f_near/f_coarse.
    f_near_batched: CudaFunction,
    f_coarse_batched: CudaFunction,
    /// M4 GPU classify (counting-sort the per-tile near/far gate on device, replacing the
    /// single-threaded CPU `classify_tile` wall) → device-built CSR for the M3 batched kernels.
    f_classify_count: CudaFunction,
    f_classify_scatter: CudaFunction,
    d_npd: CudaSlice<f32>,
    /// `NUM_CLASSES`-length GA hybrid weight LUT (f32). The kernel scales
    /// each sub-segment's energy by `d_w[class]`.
    d_w: CudaSlice<f32>,
    /// Total VRAM (bytes) of this device, queried once at open — the M2 chunked build derives its
    /// candidate-chunk size from it (no hand-set chunk knob; see `gpu_airborne::max_candidates_per_chunk`).
    vram_total: u64,
}

/// One R4's candidate sub-segs, resident on the device (the expensive `prepare_segment` +
/// pack + upload done once). `region` is kept host-side for the per-tile [`classify_tile`].
pub struct RegionResident {
    region: Vec<(SegmentPrepared, u8)>,
    d_sll: CudaSlice<f64>,
    d_sf: CudaSlice<f32>,
    d_si: CudaSlice<i32>,
    nreg: usize,
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
    /// — see `gpu_surface`): a dead device or missing kernel is fatal to the whole build, so
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
        dev.load_ptx(
            Ptx::from_src(AIRBORNE_PTX),
            "air",
            &[
                "airborne_exact",
                "airborne_coarse",
                "airborne_exact_batched",
                "airborne_coarse_batched",
                "airborne_classify_count",
                "airborne_classify_scatter",
            ],
        )
        .expect("load airborne ptx");
        let f_near = dev.get_func("air", "airborne_exact").expect("fn near");
        let f_coarse = dev.get_func("air", "airborne_coarse").expect("fn coarse");
        let f_near_batched = dev
            .get_func("air", "airborne_exact_batched")
            .expect("fn near_batched");
        let f_coarse_batched = dev
            .get_func("air", "airborne_coarse_batched")
            .expect("fn coarse_batched");
        let f_classify_count = dev
            .get_func("air", "airborne_classify_count")
            .expect("fn classify_count");
        let f_classify_scatter = dev
            .get_func("air", "airborne_classify_scatter")
            .expect("fn classify_scatter");
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
            f_near,
            f_coarse,
            f_near_batched,
            f_coarse_batched,
            f_classify_count,
            f_classify_scatter,
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

    /// Pack + upload a region's candidate sub-segs to the device (ONCE per R4). The returned
    /// handle is reused by every [`scatter_tile`] of that region. Delegates the device upload to
    /// [`upload_region`] (the GPU half) and keeps `region` host-side for [`scatter_tile`].
    pub fn load_region(&self, region: Vec<(SegmentPrepared, u8)>) -> Result<RegionResident> {
        let (sll, sf, si) = pack_airborne_segs(&region);
        let nreg = region.len();
        let mut r = self.upload_region(sll, sf, si, nreg)?;
        r.region = region;
        Ok(r)
    }

    /// htod-upload pre-packed region SoA (the GPU half of [`load_region`]). `region` stays
    /// host-side only for [`scatter_tile`] (the e2 validator path); the stream / `scatter_region`
    /// path never reads it, so the A2 pipeline packs on the CPU prep thread and only this upload
    /// touches the device — letting CPU prep of the NEXT cell overlap this cell's GPU build.
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
            region: Vec::new(),
            d_sll,
            d_sf,
            d_si,
            nreg,
        })
    }

    /// Scatter one tile against the resident region: classify into near/far[3] index lists,
    /// launch the near (exact per-pixel) + far (coarse lattice) kernels, bilinear-expand each
    /// far level on the host, and return the fused `TileAccumulator` (3 periods × TILE_PX² cells).
    pub fn scatter_tile(&self, region: &RegionResident, tile: &FusedTileZ13) -> TileAccumulator {
        // Empty region → silent tile; skip all device work (common for rural R4s at world scale).
        if region.nreg == 0 {
            return TileAccumulator::new();
        }
        let n = TILE_PX * TILE_PX;
        let block: u32 = 256;
        let nreg_i = region.nreg as i32;

        let (near_idx, far_idx) = classify_tile(tile, &region.region);
        let near_len = near_idx.len();
        let (rll, rxa) = pack_airborne_receivers(tile);
        let d_rll = self.dev.htod_copy(rll).expect("upload rll");
        let d_rxa = self.dev.htod_copy(rxa).expect("upload rxa");
        let d_nidx = self.dev.htod_copy(near_idx).expect("upload near idx");
        let mut d_near = self.dev.alloc_zeros::<f32>(n * 3).expect("alloc near out");
        // (far index list, nidx, lattice side n, coarse out) per level.
        let mut fardev: Vec<(CudaSlice<i32>, usize, usize, CudaSlice<f32>)> =
            Vec::with_capacity(far_idx.len());
        for (lvl, idxs) in far_idx.into_iter().enumerate() {
            let nn = COARSE_LEVELS_N[lvl];
            let nidx = idxs.len();
            fardev.push((
                self.dev.htod_copy(idxs).expect("upload far idx"),
                nidx,
                nn,
                self.dev
                    .alloc_zeros::<f32>(nn * nn * 3)
                    .expect("alloc coarse"),
            ));
        }

        let cfg_near = LaunchConfig {
            grid_dim: ((n as u32).div_ceil(block), 1, 1),
            block_dim: (block, 1, 1),
            shared_mem_bytes: 0,
        };
        unsafe {
            self.f_near
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
                        &d_nidx,
                        near_len as i32,
                        nreg_i,
                        &mut d_near,
                    ),
                )
                .expect("launch near");
        }
        for (d_idx, nidx, nn, d_coarse) in fardev.iter_mut() {
            if *nidx == 0 {
                continue;
            }
            let cfg = LaunchConfig {
                grid_dim: ((*nidx as u32).div_ceil(block), 1, 1),
                block_dim: (block, 1, 1),
                shared_mem_bytes: 0,
            };
            unsafe {
                self.f_coarse
                    .clone()
                    .launch(
                        cfg,
                        (
                            &d_rll,
                            &d_rxa,
                            &region.d_sll,
                            &region.d_sf,
                            &region.d_si,
                            &self.d_npd,
                            &self.d_w,
                            &*d_idx,
                            *nidx as i32,
                            nreg_i,
                            *nn as i32,
                            &mut *d_coarse,
                        ),
                    )
                    .expect("launch coarse");
            }
        }
        self.dev.synchronize().expect("kernel sync");

        let gpu_near = self.dev.dtoh_sync_copy(&d_near).expect("dtoh near");
        let mut fine = TileAccumulator::new();
        fine.energy.copy_from_slice(&gpu_near);
        for (_, nidx, nn, d_coarse) in fardev.iter() {
            if *nidx == 0 {
                continue;
            }
            let coarse = self.dev.dtoh_sync_copy(d_coarse).expect("dtoh coarse");
            CoarseLattice::from_energy(*nn, coarse).expand_into(&mut fine);
        }
        fine
    }

    /// M3+M4 batched scatter over a BLOCK of tiles: the per-tile near/far candidate gate
    /// (`classify_tile`'s O(nreg) loop — the single-threaded CPU wall that capped airborne GPU
    /// util) now runs ON the device as a counting-sort, then the M3 batched kernels consume the
    /// device-built CSR with ~1 sync + 1 copyback for the whole block. Per block:
    ///   1. host packs a tiny per-tile `meta` (centre, m/deg lon, half-diag, max rx alt);
    ///   2. `airborne_classify_count` → per-(tile,bucket) counts (only tile×4 ints cross PCIe);
    ///   3. host prefix-sums counts → near CSR + far base offsets;
    ///   4. `airborne_classify_scatter` fills near_idx + far (seg,tile) lists on device;
    ///   5. `airborne_exact_batched` + ≤3 `airborne_coarse_batched` → ONE sync → ONE dtoh →
    ///      host bilinear-expand per tile.
    ///
    /// Returns one `TileAccumulator` per input tile, in order. Same `airborne_sel` physics and
    /// same gate as the CPU `scatter_tile`, so parity holds by construction (dB-level: the
    /// device atomic-scatter order and f32 gate inputs admit ≪0.5 dB run-to-run jitter).
    pub fn scatter_region(
        &self,
        region: &RegionResident,
        tiles: &[&FusedTileZ13],
    ) -> Result<Vec<TileAccumulator>> {
        let t = tiles.len();
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

        // 2. Pass 1 (count): thread per (tile, seg) → counts[tile*4 + bucket].
        let threads = (t as u64) * (nreg as u64);
        let cfg_classify = LaunchConfig {
            grid_dim: (threads.div_ceil(block as u64) as u32, 1, 1),
            block_dim: (block, 1, 1),
            shared_mem_bytes: 0,
        };
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
                        &mut d_counts,
                    ),
                )
                .context("launch classify_count")?;
        }
        self.dev.synchronize().context("count sync")?;
        let counts = self.dev.dtoh_sync_copy(&d_counts).context("dtoh counts")?;

        // 3. Prefix-sum → off[4*(t+1)]: block 0 = near CSR (also the batched near kernel's
        //    near_off), blocks 1/2/3 = per-tile far-level base offset. Totals size the buffers.
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
        let d_off = self.dev.htod_copy(off).context("off")?;

        // 4. Pass 2 (scatter): fill near_idx + per-level far (seg,tile) lists on device.
        //    alloc_zeros rejects 0 bytes → dummy-size empty buckets; the kernel never writes them.
        let mut d_fill = self.dev.alloc_zeros::<i32>(t * 4).context("fill")?;
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
                        &mut d_fill,
                        nreg_i,
                        t as i32,
                        &mut d_near_idx,
                        &mut d_far0,
                        &mut d_far1,
                        &mut d_far2,
                    ),
                )
                .context("launch classify_scatter")?;
        }

        // 5. Physics: receivers (concatenated) → batched near (reads off block 0 as near CSR)
        //    + ≤3 batched coarse levels (each over its far (seg,tile) list).
        let (rll_b, rxa_b) = pack_airborne_receivers_batch(tiles);
        let d_rll = self.dev.htod_copy(rll_b).context("rll_b")?;
        let d_rxa = self.dev.htod_copy(rxa_b).context("rxa_b")?;
        let mut d_out = self.dev.alloc_zeros::<f32>(npix * 3 * t).context("out_b")?;
        let cfg_near = LaunchConfig {
            grid_dim: (((t * npix) as u32).div_ceil(block), 1, 1),
            block_dim: (block, 1, 1),
            shared_mem_bytes: 0,
        };
        unsafe {
            self.f_near_batched
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
                        &d_off,
                        &d_near_idx,
                        nreg_i,
                        t as i32,
                        &mut d_out,
                    ),
                )
                .context("launch near_batched")?;
        }
        // (far (seg,tile) device buf, nfar, lattice side n, coarse out) per level.
        let mut fardev: Vec<(&CudaSlice<i32>, usize, usize, CudaSlice<f32>)> =
            Vec::with_capacity(3);
        for (lvl, d_far) in [&d_far0, &d_far1, &d_far2].into_iter().enumerate() {
            let nn = COARSE_LEVELS_N[lvl];
            let d_coarse = self
                .dev
                .alloc_zeros::<f32>(nn * nn * 3 * t)
                .context("coarse_b")?;
            fardev.push((d_far, total[lvl + 1], nn, d_coarse));
        }
        for (d_far, nfar, nn, d_coarse) in fardev.iter_mut() {
            if *nfar == 0 {
                continue;
            }
            let cfg = LaunchConfig {
                grid_dim: ((*nfar as u32).div_ceil(block), 1, 1),
                block_dim: (block, 1, 1),
                shared_mem_bytes: 0,
            };
            unsafe {
                self.f_coarse_batched
                    .clone()
                    .launch(
                        cfg,
                        (
                            &d_rll,
                            &d_rxa,
                            &region.d_sll,
                            &region.d_sf,
                            &region.d_si,
                            &self.d_npd,
                            &self.d_w,
                            *d_far,
                            *nfar as i32,
                            nreg_i,
                            *nn as i32,
                            &mut *d_coarse,
                        ),
                    )
                    .context("launch coarse_batched")?;
            }
        }
        self.dev.synchronize().context("batched kernel sync")?;

        let near_all = self.dev.dtoh_sync_copy(&d_out).context("dtoh near_b")?;
        let coarse_all: Vec<(usize, Vec<f32>)> = fardev
            .iter()
            .map(|(_, nfar, nn, d_coarse)| -> Result<(usize, Vec<f32>)> {
                if *nfar == 0 {
                    Ok((*nn, Vec::new()))
                } else {
                    Ok((
                        *nn,
                        self.dev.dtoh_sync_copy(d_coarse).context("dtoh coarse_b")?,
                    ))
                }
            })
            .collect::<Result<Vec<_>>>()?;

        Ok((0..t)
            .map(|ti| {
                let mut fine = TileAccumulator::new();
                fine.energy
                    .copy_from_slice(&near_all[ti * npix * 3..(ti + 1) * npix * 3]);
                for (nn, data) in &coarse_all {
                    if data.is_empty() {
                        continue;
                    }
                    let per = nn * nn * 3;
                    CoarseLattice::from_energy(*nn, data[ti * per..(ti + 1) * per].to_vec())
                        .expand_into(&mut fine);
                }
                fine
            })
            .collect())
    }
}
