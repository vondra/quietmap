//! Device storage and launches for source rays, weighted receiver spectra and pixel loudness.
//!
//! GPU compilation, scene upload and pair evaluation in the square's frame.
// Coordinates are rebased in f64 before the GPU uses f32 frame metres. The repr(C) records mirror
// scene.cuh, evaluate.cuh and paint.cu field by field.

pub use crate::eval_slot::PairResults;
use crate::eval_slot::{EvalSlot, EvalSlotGuard, OutputPool, next_rays_per_pair};
pub(crate) use crate::eval_slot::{ensure_device, trim_device};
use crate::stream::{PinnedBuf, download_default, upload_default};
use crate::turns::CardTurns;
use cudarc::driver::sys::CUdevice_attribute;
use cudarc::driver::{
    CudaDevice, CudaSlice, DevicePtr, DeviceRepr, DeviceSlice, LaunchAsync, LaunchConfig,
};
use cudarc::nvrtc::{CompileOptions, compile_ptx_with_opts};
use paint::exact::LAYERS;
use paint::square::{Files, Square};
use physics::bands::{BAND_FREQUENCY_HZ, BANDS, PERIODS, SPEED_OF_SOUND_M_PER_S};
use physics::weather::WeatherTable;
use rayon::prelude::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use tiles::Kind;
use tiles::geo::{Mercator, TILES_PER_AXIS, TileId};
use tiles::obstacles::Obstacles;
use tiles::sources::GROUND_FROM_TERRAIN;
use tiles::terrain::{NODES_PER_DEGREE, Terrain, Window};

/// A launch of one thread, for the kernels that write one result (cudarc's `for_num_elems(1)` is
/// a whole block of 1,024 writers).
const ONE_THREAD: LaunchConfig = LaunchConfig {
    grid_dim: (1, 1, 1),
    block_dim: (1, 1, 1),
    shared_mem_bytes: 0,
};

/// The device code, in the order its parts build on each other: the shared types, then the
/// constants the host computes ([`band_constants`]), then the physics and the kernels.
const COMMON: &str = include_str!("../kernels/common.cuh");
const PHYSICS: &str = concat!(
    include_str!("../kernels/ground.cuh"),
    include_str!("../kernels/cnossos.cuh"),
    include_str!("../kernels/terrain.cuh"),
    include_str!("../kernels/terrain_walk.cuh"),
    include_str!("../kernels/scene.cuh"),
    include_str!("../kernels/ray_stream.cuh"),
    include_str!("../kernels/ray_path.cuh"),
    include_str!("../kernels/ray.cuh"),
    include_str!("../kernels/line.cuh"),
    include_str!("../kernels/evaluate.cuh"),
    include_str!("../kernels/paint.cu"),
);

/// ground.rs's BAND_CONSTANTS as device constants: computed in f64 as the CPU does, rounded once
/// to f32 (f32's Debug is the shortest decimal that parses back to it).
fn band_constants() -> String {
    let table = |name: &str, value: &dyn Fn(f64) -> f64| {
        let values: Vec<String> = BAND_FREQUENCY_HZ
            .iter()
            .map(|&f| format!("{:?}f", value(f) as f32))
            .collect();
        format!(
            "__constant__ float {name}[BANDS] = {{{}}};\n",
            values.join(", ")
        )
    };
    [
        table("BAND_F_2_5", &|f| f.powf(2.5)),
        table("BAND_F_1_5", &|f| f.powf(1.5)),
        table("BAND_F_0_75", &|f| f.powf(0.75)),
        table("BAND_WAVENUMBER", &|f| {
            2.0 * std::f64::consts::PI * f / SPEED_OF_SOUND_M_PER_S
        }),
    ]
    .concat()
}

/// The fine terrain's lattice, reach and detail step, and the layers, as device constants
/// (kernels/scene.cuh, paint.cu).
fn lattice_constants() -> String {
    format!(
        "#define FINE_PER_NODE {}\n#define FINE_ENDS_M {:?}f\n#define DETAIL_STEP_M {:?}f\n#define LAYERS {}\n",
        tiles::terrain_fine::FINE_PER_NODE,
        physics::profile::FINE_ENDS_M as f32,
        tiles::terrain_fine::DETAIL_STEP_M as f32,
        LAYERS
    )
}

/// Floats a pair's sums take (kernels/evaluate.cuh PAIR_SUMS): mixed, calm and downwind per period.
pub(crate) const PAIR_SUMS: usize = 3 * PERIODS;
/// Floats a point's bands take: per layer, period and octave band.
pub const POINT_BANDS: usize = LAYERS * PERIODS * BANDS;

/// A pair's failure (kernels/common.cuh): the host evaluates such a pair on the CPU.
pub const FAILED_NOT_READ: u32 = 1;
pub const FAILED_NO_TERRAIN: u32 = 2;
pub const FAILED_CAPACITY: u32 = 4;
pub const FAILED_LINE_CUTOFF: u32 = 8;

/// One warp per pair-kernel block: the ray kernel's launch bounds (kernels/paint.cu) then give each
/// thread 128 registers or more. Against two-warp blocks at 64 registers, real pixel launches
/// improve by about 5% on compute capability 12.0, and fixed-pair checks on 7.0 and 7.5 by more
/// than half.
const THREADS_PER_BLOCK: u32 = 32;
/// Bytes of one listed ray (kernels/evaluate.cuh PairRay).
pub(crate) const PAIR_RAY_BYTES: usize = 24;

const TILE_NOT_READ: i32 = 0;
const TILE_EMPTY: i32 = 1;
const TILE_READ: i32 = 2;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct TerrainTile {
    state: i32,
    rows: u32,
    columns: u32,
    /// The scene row of this tile's row 0: a node's local row is its scene row minus this.
    row_shift: i32,
    column_at_origin: f32,
    columns_per_metre: f32,
    nodes_offset: u64,
    canopy_offset: u64,
    /// Its fine terrain's detail codes (i16, tiles::terrain_fine) in the nodes' buffer; 0: none
    /// (the first tile's nodes come first).
    fine_offset: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct TerrainScene {
    tile_x_at_origin: f32,
    tile_y_at_origin: f32,
    tiles_per_metre_east: f32,
    tiles_per_metre_north: f32,
    radius: i64,
    tiles: u64,
    nodes: u64,
    scene_rows: u64,
    scene_row_count: u32,
    column_at_origin: f32,
    columns_per_metre: f32,
    has_fine: u32,
    /// The scene row on the equator, the one parallel on a tile's edge; -1 for none (i64: no
    /// padding in the struct the card reads).
    equator_row: i64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ObstacleTile {
    state: i32,
    outline_count: u32,
    vertex_count: u32,
    run_count: u32,
    occupied_count: u32,
    square_count: u32,
    offset_x: f32,
    offset_y: f32,
    base: u64,
    cell_max_offset: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ObstacleScene {
    origin_x: f32,
    origin_y: f32,
    steps_per_metre_x: f32,
    steps_per_metre_y: f32,
    metres_per_step_x: f32,
    metres_per_step_y: f32,
    centre_x: u32,
    centre_y: u32,
    origin_cell_in_centre_x: i64,
    origin_cell_in_centre_y: i64,
    radius: i64,
    tiles: u64,
    blob: u64,
    cell_max_height: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct DeviceCandidate {
    ax: f32,
    ay: f32,
    bx: f32,
    by: f32,
    ground0: f32,
    ground1: f32,
    line: u32,
    attribute: u32,
    on_deck: u32,
    layer: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct DeviceAttribute {
    height_m: f32,
    ground_factor: f32,
    platform_half_width_m: f32,
    exclusion_radius_m: f32,
    footprint_id: u64,
    energy: [[f32; BANDS]; PERIODS],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SquareScene {
    ground: TerrainScene,
    obstacles: ObstacleScene,
    weather_nodes: u64,
    candidates: u64,
    attributes: u64,
    frame_origin_x: f32,
    frame_origin_y: f32,
    frame_east_m_per_unit: f32,
    frame_north_m_per_unit: f32,
}

/// Bytes of one point's receiver on the card (kernels/evaluate.cuh Receiver: its position and
/// altitude, its building and its weather, PERIODS x 16 sectors and BANDS floats).
const RECEIVER_BYTES: usize = 248;

/// The sizes of the structs above, in the order the kernel `struct_sizes` reports the device's.
const HOST_STRUCT_SIZES: [usize; 8] = [
    std::mem::size_of::<TerrainTile>(),
    std::mem::size_of::<TerrainScene>(),
    std::mem::size_of::<ObstacleTile>(),
    std::mem::size_of::<ObstacleScene>(),
    std::mem::size_of::<DeviceCandidate>(),
    std::mem::size_of::<DeviceAttribute>(),
    std::mem::size_of::<SquareScene>(),
    RECEIVER_BYTES,
];

/// The bytes of plain old data (the `#[repr(C)]` structs above, without padding holes that
/// matter: every field is written).
fn bytes_of<T: Copy>(values: &[T]) -> Vec<u8> {
    let length = std::mem::size_of_val(values);
    // SAFETY: T is Copy plain data laid out by repr(C); the bytes are read, never reinterpreted.
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast::<u8>(), length) }.to_vec()
}

pub(crate) fn driver<T>(result: Result<T, cudarc::driver::DriverError>) -> Result<T, String> {
    result.map_err(|error| format!("cuda: {error}"))
}

/// One card with the kernels loaded.
pub struct Gpu {
    pub(crate) device: Arc<CudaDevice>,
    pub name: String,
    /// Held while cudarc records and waits on the device's one event (`crate::stream`).
    pub(crate) event: Mutex<()>,
    /// Shared turns across every square using this card, bounding concurrent `EvalSlot`s in VRAM.
    pub(crate) turns: CardTurns,
    /// Shared turns across concurrent `pixel_loudness` launches, bounding mixtures VRAM on the card.
    pub(crate) loudness_turns: CardTurns,
    eval_slots: Mutex<Vec<EvalSlot>>,
    output_pool: OutputPool,
    pub(crate) loudness_slots: Mutex<Vec<crate::loudness::LoudnessSlot>>,
}

impl Gpu {
    /// Opens card `ordinal` and compiles the kernels for its architecture.
    pub fn new(ordinal: usize) -> Result<Self, String> {
        let device = driver(CudaDevice::new(ordinal))?;
        // Resolve and verify `cuStreamSynchronize` before any host transfer is queued: cudarc's
        // wrapper `.expect()`s the dynamically loaded symbol, so calling it here on an empty
        // stream fails cleanly during startup instead of unwinding past a live download buffer.
        driver(device.synchronize())?;
        let attribute = |which| driver(device.attribute(which));
        let major = attribute(CUdevice_attribute::CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MAJOR)?;
        let minor = attribute(CUdevice_attribute::CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MINOR)?;
        let multiprocessors =
            attribute(CUdevice_attribute::CU_DEVICE_ATTRIBUTE_MULTIPROCESSOR_COUNT)? as u32;
        let arch: &'static str = Box::leak(format!("compute_{major}{minor}").into_boxed_str());
        let source = format!(
            "{COMMON}{}{}#define THREADS_PER_BLOCK {THREADS_PER_BLOCK}\n{PHYSICS}",
            band_constants(),
            lattice_constants()
        );
        // The pixels' loudness, in full precision (fast math moved a sum's bin at its edges).
        let loudness = format!("{COMMON}{}", crate::loudness::loudness_source());
        // The aircraft boxes' sums at the corners' lattices, with the Rust's constants.
        let aircraft = format!(
            "{COMMON}{}{}",
            crate::aircraft::aircraft_constants(),
            include_str!("../kernels/aircraft.cuh")
        );
        // Compile all three NVRTC modules concurrently to cut cold startup across invocations.
        let (paint_ptx, loudness_ptx, aircraft_ptx) = std::thread::scope(|scope| {
            let paint_handle = scope.spawn(move || {
                compile_ptx_with_opts(
                    source,
                    CompileOptions {
                        arch: Some(arch),
                        // Fast math reduces point and line pair time by about 44% and 42%, with
                        // unchanged agreement with the CPU (2026-10-09). The `use_fast_math` field
                        // only adds --fmad=true; the flag goes in `options`. Line numbers cost
                        // nothing at run time and let a profiler name the kernel's lines.
                        options: vec!["--use_fast_math".into(), "-lineinfo".into()],
                        ..Default::default()
                    },
                )
                .map_err(|error| format!("nvrtc: {error}"))
            });
            let loudness_handle = scope.spawn(move || {
                compile_ptx_with_opts(
                    loudness,
                    CompileOptions {
                        arch: Some(arch),
                        options: vec!["-lineinfo".into()],
                        ..Default::default()
                    },
                )
                .map_err(|error| format!("nvrtc (loudness): {error}"))
            });
            let aircraft_res = compile_ptx_with_opts(
                aircraft,
                CompileOptions {
                    arch: Some(arch),
                    options: vec!["--use_fast_math".into(), "-lineinfo".into()],
                    ..Default::default()
                },
            )
            .map_err(|error| format!("nvrtc (aircraft): {error}"));
            let paint_res = paint_handle
                .join()
                .unwrap_or_else(|_| Err("nvrtc thread panicked".into()));
            let loudness_res = loudness_handle
                .join()
                .unwrap_or_else(|_| Err("nvrtc (loudness) thread panicked".into()));
            (paint_res, loudness_res, aircraft_res)
        });
        driver(device.load_ptx(
            paint_ptx?,
            "paint",
            &[
                "point_receivers",
                "scan_blocks",
                "scan_block_sums",
                "pair_rays",
                "ray_transfers",
                "pair_bands_to_points",
                "struct_sizes",
            ],
        ))?;
        driver(device.load_ptx(
            loudness_ptx?,
            "loudness",
            &["pixel_mixtures", "pixel_ndens", "loud_struct_sizes"],
        ))?;
        driver(device.load_ptx(
            aircraft_ptx?,
            "aircraft",
            &["flight_sums", "flight_struct_sizes"],
        ))?;
        let sizes = driver(device.alloc_zeros::<u64>(HOST_STRUCT_SIZES.len()))?;
        let function = device
            .get_func("paint", "struct_sizes")
            .ok_or("kernel struct_sizes missing")?;
        // SAFETY: the kernel writes one u64 per struct into `sizes`.
        driver(unsafe { function.launch(ONE_THREAD, (&sizes,)) })?;
        let device_sizes = download_default(&device, &sizes)?;
        if device_sizes
            .iter()
            .zip(HOST_STRUCT_SIZES)
            .any(|(&d, h)| d != h as u64)
        {
            return Err(format!(
                "struct layouts differ: device {device_sizes:?}, host {HOST_STRUCT_SIZES:?}"
            ));
        }
        let sizes = driver(device.alloc_zeros::<u64>(3))?;
        let function = device
            .get_func("aircraft", "flight_struct_sizes")
            .ok_or("kernel flight_struct_sizes missing")?;
        // SAFETY: the kernel writes one u64 per struct into `sizes`.
        driver(unsafe { function.launch(ONE_THREAD, (&sizes,)) })?;
        let device_sizes = download_default(&device, &sizes)?;
        let host_sizes = crate::aircraft::FLIGHT_STRUCT_SIZES;
        if device_sizes
            .iter()
            .zip(host_sizes)
            .any(|(&d, h)| d != h as u64)
        {
            return Err(format!(
                "flight struct layouts differ: device {device_sizes:?}, host {host_sizes:?}"
            ));
        }
        let warp_bytes = driver(device.alloc_zeros::<u64>(1))?;
        let function = device
            .get_func("loudness", "loud_struct_sizes")
            .ok_or("kernel loud_struct_sizes missing")?;
        // SAFETY: the kernel writes one u64.
        driver(unsafe { function.launch(ONE_THREAD, (&warp_bytes,)) })?;
        let warp_bytes = download_default(&device, &warp_bytes)?[0];
        if warp_bytes != crate::loudness::loud_warp_bytes() as u64 {
            return Err(format!(
                "loudness warp layouts differ: device {warp_bytes}, host {}",
                crate::loudness::loud_warp_bytes()
            ));
        }
        let name = driver(device.name())?;
        Ok(Gpu {
            device,
            name: format!("{name} (sm_{major}{minor}, {multiprocessors} SMs)"),
            event: Mutex::new(()),
            turns: CardTurns::default(),
            loudness_turns: CardTurns::default(),
            eval_slots: Mutex::new(Vec::new()),
            output_pool: Arc::new(Mutex::new(Vec::new())),
            loudness_slots: Mutex::new(Vec::new()),
        })
    }

    /// Zeroed band sums for `points` points ([`POINT_BANDS`] floats a point).
    pub fn bands(&self, points: usize) -> Result<CudaSlice<f32>, String> {
        driver(
            self.device
                .alloc_zeros::<f32>((POINT_BANDS * points).max(1)),
        )
    }

    /// The band sums as the pairs left them.
    pub fn download(&self, bands: &CudaSlice<f32>) -> Result<Vec<f32>, String> {
        download_default(&self.device, bands)
    }

    /// Per receiver its position in the square's frame, the building it stands in (or 0) and its
    /// point index in `bands`; per pair its candidate index, its receiver and optional band weight
    /// (`None` means 1.0 for every pair): evaluates rays in strict candidate-sorted order using a
    /// device-only prefix sum, returning pinned host sums and failures while adding mixed bands
    /// into `bands` (or writing per-pair spectra when `pair_spectra` is `Some`).
    pub fn evaluate_pairs(
        &self,
        square: &DeviceSquare,
        (receivers, own_footprint, point_of_receiver): (&[[f64; 2]], &[u64], &[u32]),
        (candidates, receiver_of_pair, band_weights): (&[u32], &[u32], Option<&[f32]>),
        bands: &mut CudaSlice<f32>,
        mut pair_spectra: Option<&mut Vec<f32>>,
    ) -> Result<PairResults, String> {
        let count = candidates.len();
        let receiver_count = receivers.len();
        if own_footprint.len() != receiver_count
            || point_of_receiver.len() != receiver_count
            || receiver_of_pair.len() != count
            || band_weights.is_some_and(|w| w.len() != count)
        {
            return Err(
                "evaluate_pairs: an owner and point a receiver, a receiver and weight a candidate"
                    .into(),
            );
        }
        if candidates.iter().any(|&index| index >= square.candidates)
            || receiver_of_pair
                .iter()
                .any(|&at| at as usize >= receiver_count)
            || point_of_receiver
                .iter()
                .any(|&point| (point as usize + 1) * POINT_BANDS > bands.len())
        {
            return Err(
                "evaluate_pairs: a candidate outside the square, a receiver outside the receivers \
                 or a point outside the bands"
                    .into(),
            );
        }
        if count == 0 {
            if let Some(spectra) = pair_spectra {
                spectra.clear();
            }
            return Ok(PairResults {
                sums: None,
                failed: None,
                pool: Arc::clone(&self.output_pool),
                count: 0,
            });
        }
        let device = &self.device;
        let kernel = |name: &str| {
            device
                .get_func("paint", name)
                .ok_or(format!("kernel {name} missing"))
        };
        let threads = |n: usize| LaunchConfig {
            grid_dim: ((n as u32).div_ceil(THREADS_PER_BLOCK).max(1), 1, 1),
            block_dim: (THREADS_PER_BLOCK, 1, 1),
            shared_mem_bytes: 0,
        };
        let slot = self
            .eval_slots
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop();
        let mut slot_guard = EvalSlotGuard {
            pool: &self.eval_slots,
            slot: Some(slot.unwrap_or_else(|| EvalSlot::new(device))),
        };
        let (mut out_sums, mut out_failed) = self
            .output_pool
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop()
            .unwrap_or_else(|| (PinnedBuf::new(device), PinnedBuf::new(device)));
        out_sums.reserve(PAIR_SUMS * count)?;
        out_failed.reserve(count)?;

        let has_weights = band_weights.is_some();
        let num_scan_blocks = count.div_ceil(1024);
        let slot = slot_guard.slot.as_mut().expect("eval slot present");
        let device_xy = ensure_device(device, &mut slot.device_xy, 2 * receiver_count)?;
        let device_own = ensure_device(device, &mut slot.device_own, receiver_count)?;
        let device_point_of_receiver =
            ensure_device(device, &mut slot.device_point_of_receiver, receiver_count)?;
        let point_receivers = ensure_device(
            device,
            &mut slot.point_receivers,
            RECEIVER_BYTES * receiver_count,
        )?;
        let receiver_failed = ensure_device(device, &mut slot.receiver_failed, receiver_count)?;
        let device_receiver_of_pair =
            ensure_device(device, &mut slot.device_receiver_of_pair, count)?;
        let device_candidates = ensure_device(device, &mut slot.device_candidates, count)?;
        let mut device_band_weights = if has_weights {
            Some(ensure_device(device, &mut slot.device_band_weights, count)?)
        } else {
            None
        };
        let ray_counts = ensure_device(device, &mut slot.ray_counts, count)?;
        let ray_offsets = ensure_device(device, &mut slot.ray_offsets, count)?;
        let block_sums = ensure_device(device, &mut slot.block_sums, num_scan_blocks)?;
        let total_rays = ensure_device(device, &mut slot.total_rays, 1)?;
        let failed = ensure_device(device, &mut slot.failed, count)?;
        let pair_bands = ensure_device(device, &mut slot.pair_bands, PERIODS * BANDS * count)?;
        let sums = ensure_device(device, &mut slot.sums, PAIR_SUMS * count)?;

        slot.host_xy.reserve(2 * receiver_count)?;
        {
            let dst = slot.host_xy.as_mut_slice(2 * receiver_count);
            for (i, &[x, y]) in receivers.iter().enumerate() {
                dst[2 * i] = x as f32;
                dst[2 * i + 1] = y as f32;
            }
        }
        slot.host_own.copy_from_slice(own_footprint)?;
        slot.host_point_of_receiver
            .copy_from_slice(point_of_receiver)?;
        slot.host_receiver_of_pair
            .copy_from_slice(receiver_of_pair)?;
        slot.host_candidates.copy_from_slice(candidates)?;
        if let Some(weights) = band_weights {
            slot.host_band_weights.copy_from_slice(weights)?;
        }
        slot.host_total.reserve(1)?;

        let mut need_rays = count.saturating_mul(slot.rays_per_pair).max(4096);
        let mut uploaded = false;
        let none = 0u64;
        let weights_ptr: u64 = device_band_weights
            .as_deref()
            .map_or(0, |buf| *buf.device_ptr());
        let scan_blocks_cfg = LaunchConfig {
            grid_dim: (num_scan_blocks as u32, 1, 1),
            block_dim: (256, 1, 1),
            shared_mem_bytes: 0,
        };
        let scan_sums_cfg = LaunchConfig {
            grid_dim: (1, 1, 1),
            block_dim: (256, 1, 1),
            shared_mem_bytes: 0,
        };
        let host_xy = &slot.host_xy;
        let host_own = &slot.host_own;
        let host_point_of_receiver = &slot.host_point_of_receiver;
        let host_receiver_of_pair = &slot.host_receiver_of_pair;
        let host_candidates = &slot.host_candidates;
        let host_band_weights = &slot.host_band_weights;
        let host_total = &mut slot.host_total;
        loop {
            let rays = ensure_device(device, &mut slot.rays, PAIR_RAY_BYTES * need_rays)?;
            let ray_capacity = need_rays as u32;
            let overflow = {
                let stream = self.fork()?;
                let count_u32 = count as u32;
                if !uploaded {
                    // SAFETY: `stream` drops before `slot_guard` and all pinned/device buffers.
                    unsafe {
                        stream.copy_prefix(host_xy.as_slice(2 * receiver_count), device_xy)?;
                        stream.copy_prefix(host_own.as_slice(receiver_count), device_own)?;
                        stream.copy_prefix(
                            host_point_of_receiver.as_slice(receiver_count),
                            device_point_of_receiver,
                        )?;
                        stream.copy_prefix(
                            host_receiver_of_pair.as_slice(count),
                            device_receiver_of_pair,
                        )?;
                        stream.copy_prefix(host_candidates.as_slice(count), device_candidates)?;
                        if let Some(device_weights) = device_band_weights.as_deref_mut() {
                            stream
                                .copy_prefix(host_band_weights.as_slice(count), device_weights)?;
                        }
                    }
                    stream.zero_prefix(sums, PAIR_SUMS * count)?;
                    unsafe {
                        stream.launch(
                            kernel("point_receivers")?,
                            threads(receiver_count),
                            (
                                &square.scene,
                                &*device_xy,
                                &*device_own,
                                receiver_count as u32,
                                &*point_receivers,
                                &*receiver_failed,
                            ),
                        )?;
                    }
                    uploaded = true;
                }
                let direct_single: u32 = u32::from(pair_spectra.is_none());
                let mut pass1_params = [
                    (&square.scene).as_kernel_param(),
                    (&*point_receivers).as_kernel_param(),
                    (&*receiver_failed).as_kernel_param(),
                    (&*device_receiver_of_pair).as_kernel_param(),
                    (&*device_candidates).as_kernel_param(),
                    count_u32.as_kernel_param(),
                    none.as_kernel_param(),
                    none.as_kernel_param(),
                    none.as_kernel_param(),
                    ray_capacity.as_kernel_param(),
                    direct_single.as_kernel_param(),
                    none.as_kernel_param(),
                    (&*ray_counts).as_kernel_param(),
                    (&*failed).as_kernel_param(),
                    (&*pair_bands).as_kernel_param(),
                ];
                unsafe {
                    stream.launch(kernel("pair_rays")?, threads(count), &mut pass1_params[..])?;
                    stream.launch(
                        kernel("scan_blocks")?,
                        scan_blocks_cfg,
                        (&*ray_counts, count_u32, &*ray_offsets, &*block_sums),
                    )?;
                    stream.launch(
                        kernel("scan_block_sums")?,
                        scan_sums_cfg,
                        (&*block_sums, num_scan_blocks as u32, &*total_rays),
                    )?;
                }
                let mut pass2_params = [
                    (&square.scene).as_kernel_param(),
                    (&*point_receivers).as_kernel_param(),
                    (&*receiver_failed).as_kernel_param(),
                    (&*device_receiver_of_pair).as_kernel_param(),
                    (&*device_candidates).as_kernel_param(),
                    count_u32.as_kernel_param(),
                    (&*ray_offsets).as_kernel_param(),
                    (&*block_sums).as_kernel_param(),
                    (&*total_rays).as_kernel_param(),
                    ray_capacity.as_kernel_param(),
                    direct_single.as_kernel_param(),
                    (&*rays).as_kernel_param(),
                    (&*ray_counts).as_kernel_param(),
                    (&*failed).as_kernel_param(),
                    (&*pair_bands).as_kernel_param(),
                ];
                unsafe {
                    stream.launch(kernel("pair_rays")?, threads(count), &mut pass2_params[..])?;
                }
                if pair_spectra.is_some() {
                    stream.zero_prefix(pair_bands, PERIODS * BANDS * count)?;
                }
                let direct_bands_ptr: u64 = if pair_spectra.is_none() {
                    *bands.device_ptr()
                } else {
                    0
                };
                let mut ray_params = [
                    (&square.scene).as_kernel_param(),
                    (&*point_receivers).as_kernel_param(),
                    (&*device_receiver_of_pair).as_kernel_param(),
                    (&*device_candidates).as_kernel_param(),
                    (&*device_point_of_receiver).as_kernel_param(),
                    weights_ptr.as_kernel_param(),
                    direct_bands_ptr.as_kernel_param(),
                    (&*rays).as_kernel_param(),
                    (&*total_rays).as_kernel_param(),
                    ray_capacity.as_kernel_param(),
                    (&*sums).as_kernel_param(),
                    (&*pair_bands).as_kernel_param(),
                    (&*failed).as_kernel_param(),
                ];
                unsafe {
                    stream.launch(
                        kernel("ray_transfers")?,
                        threads(ray_capacity as usize),
                        &mut ray_params[..],
                    )?;
                    if pair_spectra.is_none() {
                        stream.launch(
                            kernel("pair_bands_to_points")?,
                            threads(count),
                            (
                                &square.scene,
                                &*device_receiver_of_pair,
                                &*device_candidates,
                                &*device_point_of_receiver,
                                weights_ptr,
                                count_u32,
                                &*total_rays,
                                ray_capacity,
                                &*ray_counts,
                                &*pair_bands,
                                &*failed,
                                &mut *bands,
                            ),
                        )?;
                    }
                }
                stream.read_back_into(total_rays, host_total.as_mut_slice(1))?;
                let total = host_total.as_slice(1)[0] as usize;
                if total > ray_capacity as usize {
                    // On rare overflow, `pair_rays` pass-2 skipped writing and `zero_prefix`
                    // cleared `pair_bands` (pass-1's slot cache), so re-run pass-1 on retry.
                    stream.finish()?;
                    Err(total)
                } else {
                    stream.read_back_into(sums, out_sums.as_mut_slice(PAIR_SUMS * count))?;
                    stream.read_back_into(failed, out_failed.as_mut_slice(count))?;
                    if let Some(spectra) = pair_spectra.as_deref_mut() {
                        *spectra = if total > 0 {
                            stream.read_back_prefix(pair_bands, PERIODS * BANDS * count)?
                        } else {
                            vec![0.0; PERIODS * BANDS * count]
                        };
                    }
                    stream.finish()?;
                    Ok(total)
                }
            };
            match overflow {
                Err(total) => {
                    need_rays = total.next_multiple_of(1024).max(need_rays);
                }
                Ok(total) => {
                    slot.rays_per_pair = next_rays_per_pair(slot.rays_per_pair, total, count);
                    break;
                }
            }
        }
        Ok(PairResults {
            sums: Some(out_sums),
            failed: Some(out_failed),
            pool: Arc::clone(&self.output_pool),
            count,
        })
    }
}

/// A square's buffers on the card; `scene` is the descriptor the kernels read.
pub struct DeviceSquare {
    scene: CudaSlice<u8>,
    /// The square's candidates, the most a pair may index.
    candidates: u32,
    // Kept alive while the descriptor points at them.
    _buffers: Vec<CudaSlice<u8>>,
    _floats: Vec<CudaSlice<f32>>,
    pub bytes: u64,
}

#[path = "square_upload.rs"]
mod square_upload;
