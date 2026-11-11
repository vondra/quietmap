//! Device storage and launches for source rays, weighted receiver spectra and pixel loudness.
//!
//! GPU compilation, scene upload and pair evaluation in the square's frame.
// Coordinates are rebased in f64 before the GPU uses f32 frame metres. The repr(C) records mirror
// scene.cuh, evaluate.cuh and paint.cu field by field.

use cudarc::driver::sys::CUdevice_attribute;
use cudarc::driver::{
    CudaDevice, CudaSlice, CudaStream, DevicePtr, DeviceRepr, DeviceSlice, LaunchAsync,
    LaunchConfig, result,
};
use cudarc::nvrtc::{CompileOptions, compile_ptx_with_opts};
use paint::exact::LAYERS;
use paint::square::{Files, Square};
use physics::bands::{BAND_FREQUENCY_HZ, BANDS, PERIODS, SPEED_OF_SOUND_M_PER_S};
use physics::weather::WeatherTable;
use rayon::prelude::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
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
const PAIR_RAY_BYTES: usize = 24;

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
}

impl Gpu {
    /// Opens card `ordinal` and compiles the kernels for its architecture.
    pub fn new(ordinal: usize) -> Result<Self, String> {
        let device = driver(CudaDevice::new(ordinal))?;
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
        let ptx = compile_ptx_with_opts(
            source,
            CompileOptions {
                arch: Some(arch),
                // Fast math reduces point and line pair time by about 44% and 42%, with unchanged
                // agreement with the CPU (2026-10-09). The `use_fast_math` field only adds
                // --fmad=true; the flag goes in `options`. Line numbers cost nothing at run time
                // and let a profiler name the kernel's lines.
                options: vec!["--use_fast_math".into(), "-lineinfo".into()],
                ..Default::default()
            },
        )
        .map_err(|error| format!("nvrtc: {error}"))?;
        driver(device.load_ptx(
            ptx,
            "paint",
            &[
                "point_receivers",
                "pair_rays",
                "ray_transfers",
                "pair_bands_to_points",
                "struct_sizes",
            ],
        ))?;
        // The pixels' loudness, in full precision (fast math moved a sum's bin at its edges).
        let loudness = format!("{COMMON}{}", crate::loudness::loudness_source());
        let ptx = compile_ptx_with_opts(
            loudness,
            CompileOptions {
                arch: Some(arch),
                options: vec!["-lineinfo".into()],
                ..Default::default()
            },
        )
        .map_err(|error| format!("nvrtc (loudness): {error}"))?;
        driver(device.load_ptx(ptx, "loudness", &["pixel_mixtures", "pixel_ndens"]))?;
        // The aircraft boxes' sums at the corners' lattices, with the Rust's constants.
        let aircraft = format!(
            "{COMMON}{}{}",
            crate::aircraft::aircraft_constants(),
            include_str!("../kernels/aircraft.cuh")
        );
        let ptx = compile_ptx_with_opts(
            aircraft,
            CompileOptions {
                arch: Some(arch),
                options: vec!["--use_fast_math".into(), "-lineinfo".into()],
                ..Default::default()
            },
        )
        .map_err(|error| format!("nvrtc (aircraft): {error}"))?;
        driver(device.load_ptx(ptx, "aircraft", &["flight_sums", "flight_struct_sizes"]))?;
        let sizes = driver(device.alloc_zeros::<u64>(HOST_STRUCT_SIZES.len()))?;
        let function = device
            .get_func("paint", "struct_sizes")
            .ok_or("kernel struct_sizes missing")?;
        // SAFETY: the kernel writes one u64 per struct into `sizes`.
        driver(unsafe { function.launch(ONE_THREAD, (&sizes,)) })?;
        let device_sizes = driver(device.dtoh_sync_copy(&sizes))?;
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
        let device_sizes = driver(device.dtoh_sync_copy(&sizes))?;
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
        let name = driver(device.name())?;
        Ok(Gpu {
            device,
            name: format!("{name} (sm_{major}{minor}, {multiprocessors} SMs)"),
            event: Mutex::new(()),
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
        driver(self.device.dtoh_sync_copy(bands))
    }

    /// Per receiver its position in the square's frame and the building it stands in (or 0); per
    /// pair its candidate index, its receiver and its point's index in `bands`: what the candidate
    /// delivers there per period, mixed, calm and downwind, and the failure, 0 for none; its mixed
    /// bands added to the point's.
    pub fn evaluate_pairs(
        &self,
        square: &DeviceSquare,
        (receivers, own_footprint): (&[[f64; 2]], &[u64]),
        (candidates, receiver_of_pair, points, band_weights): (&[u32], &[u32], &[u32], &[f32]),
        bands: &mut CudaSlice<f32>,
        pair_spectra: Option<&mut Vec<f32>>,
    ) -> Result<(Vec<[f32; PAIR_SUMS]>, Vec<u32>), String> {
        let count = candidates.len();
        if own_footprint.len() != receivers.len()
            || receiver_of_pair.len() != count
            || points.len() != count
            || band_weights.len() != count
        {
            return Err(
                "evaluate_pairs: an owner a receiver, a receiver and a point a candidate".into(),
            );
        }
        if candidates.iter().any(|&index| index >= square.candidates)
            || receiver_of_pair
                .iter()
                .any(|&at| at as usize >= receivers.len())
            || points
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
            return Ok((Vec::new(), Vec::new()));
        }
        let xy: Vec<f32> = receivers.iter().flatten().map(|&v| v as f32).collect();
        let device = &self.device;
        let xy = driver(device.htod_sync_copy(&xy))?;
        let own = driver(device.htod_sync_copy(own_footprint))?;
        let receiver_count = receivers.len();
        // SAFETY (the next two): `point_receivers` writes every receiver's failure and every
        // receiver that does not fail, which alone are read.
        let point_receivers =
            driver(unsafe { device.alloc::<u8>(RECEIVER_BYTES * receiver_count) })?;
        let receiver_failed = driver(unsafe { device.alloc::<u32>(receiver_count) })?;
        let receiver_of_pair = driver(device.htod_sync_copy(receiver_of_pair))?;
        let index = driver(device.htod_sync_copy(candidates))?;
        let point_index = driver(device.htod_sync_copy(points))?;
        let band_weights = driver(device.htod_sync_copy(band_weights))?;
        let ray_counts = driver(device.alloc_zeros::<u32>(count))?;
        let failed = driver(device.alloc_zeros::<u32>(count))?;
        // The pairs' bands, which keep each pair's first rays from its counting to its writing
        // (kernels/paint.cu PAIR_RAY_SLOTS), zeroed again before the rays fill them.
        let pair_bands = driver(device.alloc_zeros::<f32>(PERIODS * BANDS * count))?;
        // Zeroed before the pairs' stream forks: a launch without rays reads them back unwritten.
        let sums = driver(device.alloc_zeros::<f32>(PAIR_SUMS * count))?;
        let kernel = |name: &str| {
            device
                .get_func("paint", name)
                .ok_or(format!("kernel {name} missing"))
        };
        let threads = |n: usize| LaunchConfig {
            grid_dim: ((n as u32).div_ceil(THREADS_PER_BLOCK), 1, 1),
            block_dim: (THREADS_PER_BLOCK, 1, 1),
            shared_mem_bytes: 0,
        };
        // A null pointer: the listing kernel counts the rays instead of writing them.
        let none = 0u64;
        // The pairs' kernels on a stream of their own (once the buffers above are zeroed): another
        // pass's or square's kernels run beside them on the card.
        let stream = self.fork()?;
        // SAFETY (each launch): every buffer holds what the kernel indexes: 2 coordinates, an owner,
        // a receiver (RECEIVER_BYTES) and a failure per receiver; a receiver (within the receivers,
        // checked), a candidate and a point (within `bands`, checked) per pair; a count, an offset,
        // PAIR_SUMS sums, PERIODS x BANDS bands and a failure per pair; a PairRay (PAIR_RAY_BYTES) per ray.
        driver(unsafe {
            kernel("point_receivers")?.launch_on_stream(
                &stream,
                threads(receiver_count),
                (
                    &square.scene,
                    &xy,
                    &own,
                    receiver_count as u32,
                    &point_receivers,
                    &receiver_failed,
                ),
            )
        })?;
        driver(unsafe {
            kernel("pair_rays")?.launch_on_stream(
                &stream,
                threads(count),
                (
                    &square.scene,
                    &point_receivers,
                    &receiver_failed,
                    &receiver_of_pair,
                    &index,
                    count as u32,
                    none,
                    none,
                    &ray_counts,
                    &failed,
                    &pair_bands,
                ),
            )
        })?;
        let mut offsets = read_back(&stream, &ray_counts)?;
        let mut total = 0u32;
        for offset in &mut offsets {
            (*offset, total) = (total, total + *offset);
        }
        if total > 0 {
            let offsets = driver(device.htod_sync_copy(&offsets))?;
            let rays = driver(device.alloc_zeros::<u8>(PAIR_RAY_BYTES * total as usize))?;
            stream.wait_for_default()?;
            driver(unsafe {
                kernel("pair_rays")?.launch_on_stream(
                    &stream,
                    threads(count),
                    (
                        &square.scene,
                        &point_receivers,
                        &receiver_failed,
                        &receiver_of_pair,
                        &index,
                        count as u32,
                        &offsets,
                        &rays,
                        &ray_counts,
                        &failed,
                        &pair_bands,
                    ),
                )
            })?;
            // SAFETY: the bands' buffer, on the stream the listing just wrote it on.
            driver(unsafe {
                result::memset_d8_async(
                    *pair_bands.device_ptr(),
                    0,
                    pair_bands.num_bytes(),
                    stream.stream,
                )
            })?;
            driver(unsafe {
                kernel("ray_transfers")?.launch_on_stream(
                    &stream,
                    threads(total as usize),
                    (
                        &square.scene,
                        &point_receivers,
                        &receiver_of_pair,
                        &index,
                        &rays,
                        total,
                        &sums,
                        &pair_bands,
                        &failed,
                    ),
                )
            })?;
            if pair_spectra.is_none() {
                driver(unsafe {
                    kernel("pair_bands_to_points")?.launch_on_stream(
                        &stream,
                        threads(count),
                        (
                            &square.scene,
                            &index,
                            &point_index,
                            &band_weights,
                            count as u32,
                            &pair_bands,
                            &failed,
                            &mut *bands,
                        ),
                    )
                })?;
            }
            // The rays' buffers are freed on the default stream as this block ends: their kernels
            // finish first.
            // SAFETY: a stream this function created and still holds.
            driver(unsafe { result::stream::synchronize(stream.stream) })?;
        }
        let sums = read_back(&stream, &sums)?;
        let failed = read_back(&stream, &failed)?;
        if let Some(spectra) = pair_spectra {
            *spectra = read_back(&stream, &pair_bands)?;
        }
        Ok((
            sums.chunks_exact(PAIR_SUMS)
                .map(|chunk| std::array::from_fn(|k| chunk[k]))
                .collect(),
            failed,
        ))
    }
}

/// `source` read back once `stream`'s work so far is done.
pub(crate) fn read_back<T: DeviceRepr + Default + Clone>(
    stream: &CudaStream,
    source: &CudaSlice<T>,
) -> Result<Vec<T>, String> {
    let mut values = vec![T::default(); source.len()];
    // SAFETY: `values` holds `source.len()` values, read once the stream is synchronized.
    unsafe {
        driver(result::memcpy_dtoh_async(
            &mut values,
            *source.device_ptr(),
            stream.stream,
        ))?;
        driver(result::stream::synchronize(stream.stream))?;
    }
    Ok(values)
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
