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
use std::sync::Arc;
use tiles::Kind;
use tiles::geo::{Mercator, TILES_PER_AXIS, TileId};
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
    include_str!("../kernels/cnossos.cuh"),
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
const PAIR_SUMS: usize = 3 * PERIODS;
/// Floats a point's bands take: per layer, period and octave band.
pub const POINT_BANDS: usize = LAYERS * PERIODS * BANDS;

/// A pair's failure (kernels/common.cuh): the host evaluates such a pair on the CPU.
pub const FAILED_NOT_READ: u32 = 1;
pub const FAILED_NO_TERRAIN: u32 = 2;
pub const FAILED_CAPACITY: u32 = 4;

/// One warp per pair-kernel block allows 128 registers per thread with the ray kernel's launch
/// bounds. Against two-warp blocks at 64 registers, real pixel launches improve by about 5% on
/// compute capability 12.0, and fixed-pair checks on 7.0 and 7.5 improve by more than half.
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

/// The sizes of the structs above, in the order the kernel `struct_sizes` reports the device's.
const HOST_STRUCT_SIZES: [usize; 7] = [
    std::mem::size_of::<TerrainTile>(),
    std::mem::size_of::<TerrainScene>(),
    std::mem::size_of::<ObstacleTile>(),
    std::mem::size_of::<ObstacleScene>(),
    std::mem::size_of::<DeviceCandidate>(),
    std::mem::size_of::<DeviceAttribute>(),
    std::mem::size_of::<SquareScene>(),
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
        let name = driver(device.name())?;
        Ok(Gpu {
            device,
            name: format!("{name} (sm_{major}{minor}, {multiprocessors} SMs)"),
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

    /// Per pair (receiver position in the square's frame, building it stands in or 0, candidate
    /// index, its point's index in `bands`): what the candidate delivers there per period, mixed,
    /// calm and downwind, and the failure, 0 for none; its mixed bands added to the point's.
    pub fn evaluate_pairs(
        &self,
        square: &DeviceSquare,
        (receivers, own_footprint): (&[[f64; 2]], &[u64]),
        (candidates, points): (&[u32], &[u32]),
        bands: &mut CudaSlice<f32>,
    ) -> Result<(Vec<[f32; PAIR_SUMS]>, Vec<u32>), String> {
        let count = candidates.len();
        if receivers.len() != count || own_footprint.len() != count || points.len() != count {
            return Err("evaluate_pairs: a receiver, an owner and a point per candidate".into());
        }
        if candidates.iter().any(|&index| index >= square.candidates)
            || points
                .iter()
                .any(|&point| (point as usize + 1) * POINT_BANDS > bands.len())
        {
            return Err(
                "evaluate_pairs: a candidate outside the square or a point outside the bands"
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
        let index = driver(device.htod_sync_copy(candidates))?;
        let point_index = driver(device.htod_sync_copy(points))?;
        let ray_counts = driver(device.alloc_zeros::<u32>(count))?;
        let failed = driver(device.alloc_zeros::<u32>(count))?;
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
        let stream = driver(device.fork_default_stream())?;
        // SAFETY (each launch): every buffer holds what the kernel indexes: 2 receiver coordinates,
        // an owner, a candidate and a point (within `bands`, checked) per pair; a count, an offset,
        // PAIR_SUMS sums, PERIODS x BANDS bands and a failure per pair; a PairRay (24 bytes) per ray.
        driver(unsafe {
            kernel("pair_rays")?.launch_on_stream(
                &stream,
                threads(count),
                (
                    &square.scene,
                    &xy,
                    &own,
                    &index,
                    count as u32,
                    none,
                    none,
                    &ray_counts,
                    &failed,
                ),
            )
        })?;
        let mut offsets = read_back(&stream, &ray_counts)?;
        let mut total = 0u32;
        for offset in &mut offsets {
            (*offset, total) = (total, total + *offset);
        }
        let sums = driver(device.alloc_zeros::<f32>(PAIR_SUMS * count))?;
        if total > 0 {
            let offsets = driver(device.htod_sync_copy(&offsets))?;
            let rays = driver(device.alloc_zeros::<u8>(PAIR_RAY_BYTES * total as usize))?;
            let pair_bands = driver(device.alloc_zeros::<f32>(PERIODS * BANDS * count))?;
            driver(stream.wait_for_default())?;
            driver(unsafe {
                kernel("pair_rays")?.launch_on_stream(
                    &stream,
                    threads(count),
                    (
                        &square.scene,
                        &xy,
                        &own,
                        &index,
                        count as u32,
                        &offsets,
                        &rays,
                        &ray_counts,
                        &failed,
                    ),
                )
            })?;
            driver(unsafe {
                kernel("ray_transfers")?.launch_on_stream(
                    &stream,
                    threads(total as usize),
                    (
                        &square.scene,
                        &xy,
                        &own,
                        &index,
                        &rays,
                        total,
                        &sums,
                        &pair_bands,
                        &failed,
                    ),
                )
            })?;
            driver(unsafe {
                kernel("pair_bands_to_points")?.launch_on_stream(
                    &stream,
                    threads(count),
                    (
                        &square.scene,
                        &index,
                        &point_index,
                        count as u32,
                        &pair_bands,
                        &failed,
                        &mut *bands,
                    ),
                )
            })?;
            // The rays' buffers are freed on the default stream as this block ends: their kernels
            // finish first.
            // SAFETY: a stream this function created and still holds.
            driver(unsafe { result::stream::synchronize(stream.stream) })?;
        }
        let sums = read_back(&stream, &sums)?;
        let failed = read_back(&stream, &failed)?;
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

/// Appends a file to a blob at an 8-byte boundary; its offset.
fn append_aligned(blob: &mut Vec<u8>, bytes: &[u8]) -> u64 {
    while !blob.len().is_multiple_of(8) {
        blob.push(0);
    }
    let offset = blob.len() as u64;
    blob.extend_from_slice(bytes);
    offset
}

impl DeviceSquare {
    /// Uploads a square read on the CPU: the ground and obstacles of its rings as stored, its
    /// candidates with their attributes, and the weather table.
    pub fn upload(
        gpu: &Gpu,
        square: &Square,
        files: &Files,
        weather: &WeatherTable,
    ) -> Result<Self, String> {
        let rings = files.rings();
        let index_of: HashMap<TileId, usize> = rings
            .tiles
            .iter()
            .enumerate()
            .map(|(index, &tile)| (tile, index))
            .collect();
        // The ground: popup::scene::Ground of the square, every tile within the ground rings.
        let radius = i64::from(files.ground_rings());
        let side = 2 * radius + 1;
        let n = i64::from(TILES_PER_AXIS);
        let mut terrain_tiles = vec![TerrainTile::default(); (side * side) as usize];
        let frame = square.frame;
        // tiles::terrain::Terrain::sample's node columns per Mercator unit.
        let columns_per_unit = f64::from(360 * NODES_PER_DEGREE) / f64::from(TILES_PER_AXIS);
        let mut nodes = Vec::new();
        // popup::scene::Ground's row table: every lattice row over the scene's tiles, metres north of
        // the origin, the northernmost first.
        let nodes_per_degree = f64::from(NODES_PER_DEGREE);
        let latitude_at = |tile_y: i64| {
            let y = tile_y.clamp(0, n) as f64;
            Mercator { x: 0.0, y }.to_degrees().0
        };
        let north_row =
            (latitude_at(i64::from(square.tile.y) - radius) * nodes_per_degree).ceil() as i64;
        let south_row =
            (latitude_at(i64::from(square.tile.y) + radius + 1) * nodes_per_degree).floor() as i64;
        let scene_rows: Vec<f32> = (south_row..=north_row)
            .rev()
            .map(|row| {
                let y = Mercator::from_degrees(row as f64 / nodes_per_degree, 0.0).y;
                ((frame.origin.y - y) * frame.north_m_per_unit) as f32
            })
            .collect();
        // Each land tile's fine detail, decoded on every core (a tile is megabytes of it).
        let land_tiles: Vec<TileId> = index_of
            .iter()
            .filter(|&(_, &index)| rings.file(index, Kind::Terrain).is_some())
            .map(|(&tile, _)| tile)
            .collect();
        let fine_codes: HashMap<TileId, Vec<i16>> = land_tiles
            .par_iter()
            .filter_map(|&tile| match square.ground.fine_of(tile) {
                Ok(Some(fine)) => Some(
                    fine.codes()
                        .map(|codes| (tile, codes))
                        .map_err(|e| e.to_string()),
                ),
                Ok(None) => None,
                Err(error) => Some(Err(error)),
            })
            .collect::<Result<_, String>>()?;
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                let y = i64::from(square.tile.y) + dy;
                if !(0..n).contains(&y) {
                    continue;
                }
                let tile = TileId {
                    x: (i64::from(square.tile.x) + dx).rem_euclid(n) as u32,
                    y: y as u32,
                };
                let slot = &mut terrain_tiles[((dy + radius) * side + dx + radius) as usize];
                let Some(&index) = index_of.get(&tile) else {
                    continue;
                };
                match rings.file(index, Kind::Terrain) {
                    None => slot.state = TILE_EMPTY,
                    Some(bytes) => {
                        let terrain = Terrain::parse(bytes).map_err(|e| e.to_string())?;
                        let window = terrain.window();
                        let column_at_origin = frame.origin.x * columns_per_unit
                            - f64::from(180 * NODES_PER_DEGREE)
                            - f64::from(window.west_node);
                        let nodes_offset = append_aligned(&mut nodes, terrain.node_bytes());
                        let canopy_offset = append_aligned(&mut nodes, terrain.canopy_bytes());
                        let fine_offset = fine_codes
                            .get(&tile)
                            .map_or(0, |codes| append_aligned(&mut nodes, &bytes_of(codes)));
                        *slot = TerrainTile {
                            state: TILE_READ,
                            rows: window.rows,
                            columns: window.columns,
                            row_shift: (north_row - i64::from(window.north_node)) as i32,
                            column_at_origin: column_at_origin as f32,
                            columns_per_metre: (columns_per_unit / frame.east_m_per_unit) as f32,
                            nodes_offset,
                            canopy_offset,
                            fine_offset,
                        };
                    }
                }
            }
        }
        // The obstacles: the scene's own layout, its files as stored.
        let layout = square.obstacles.layout();
        let mut obstacle_tiles = Vec::with_capacity(layout.slots.len());
        let (mut blob, mut cell_max) = (Vec::new(), Vec::<f32>::new());
        for slot in &layout.slots {
            let mut device_tile = ObstacleTile::default();
            match slot {
                None => device_tile.state = TILE_NOT_READ,
                Some(slot) => match slot.cell_max_height_m {
                    None => device_tile.state = TILE_EMPTY,
                    Some(heights) => {
                        let index = index_of
                            .get(&slot.tile)
                            .ok_or("an obstacles tile outside the rings read")?;
                        let bytes = rings
                            .file(*index, Kind::Obstacles)
                            .ok_or("an obstacles tile without its file")?;
                        let count =
                            |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
                        device_tile = ObstacleTile {
                            state: TILE_READ,
                            outline_count: count(8),
                            vertex_count: count(12),
                            run_count: count(16),
                            offset_x: slot.offset[0] as f32,
                            offset_y: slot.offset[1] as f32,
                            base: append_aligned(&mut blob, bytes),
                            cell_max_offset: cell_max.len() as u64,
                        };
                        cell_max.extend(heights.iter().map(|&height| height as f32));
                    }
                },
            }
            obstacle_tiles.push(device_tile);
        }
        // Candidates and their attributes, the lists flattened.
        let lists = square.attributes.lists();
        let mut first_of_list = Vec::with_capacity(lists.len());
        let mut attributes = Vec::new();
        for list in lists {
            first_of_list.push(attributes.len() as u32);
            attributes.extend(list.iter().map(|source| DeviceAttribute {
                height_m: source.height_m as f32,
                ground_factor: if source.ground_percent == GROUND_FROM_TERRAIN {
                    -1.0
                } else {
                    (f64::from(source.ground_percent) / 100.0) as f32
                },
                platform_half_width_m: source.platform_half_width_m as f32,
                exclusion_radius_m: source.exclusion_radius_m as f32,
                footprint_id: source.footprint_id,
                energy: source.energy.map(|period| period.map(|value| value as f32)),
            }));
        }
        let candidates: Vec<DeviceCandidate> = square
            .candidates
            .par_iter()
            .map(|candidate| {
                let [a, b] = candidate.ends_m;
                let ground_m = candidate.traced_ground_m(&square.ground)?;
                Ok(DeviceCandidate {
                    ax: a[0] as f32,
                    ay: a[1] as f32,
                    bx: b[0] as f32,
                    by: b[1] as f32,
                    ground0: ground_m[0] as f32,
                    ground1: ground_m[1] as f32,
                    line: u32::from(candidate.line),
                    attribute: first_of_list[candidate.attribute.list as usize]
                        + candidate.attribute.index,
                    on_deck: u32::from(candidate.deck_m.is_some()),
                    layer: candidate.layer as u32,
                })
            })
            .collect::<Result<_, String>>()?;
        let device = &gpu.device;
        let upload = |bytes: Vec<u8>| driver(device.htod_sync_copy(&bytes));
        let pointer = |slice: &CudaSlice<u8>| *slice.device_ptr();
        // An empty buffer still needs an allocation to point at.
        let nonempty = |mut bytes: Vec<u8>| {
            if bytes.is_empty() {
                bytes.push(0);
            }
            bytes
        };
        let has_fine = terrain_tiles.iter().any(|tile| tile.fine_offset != 0);
        let terrain_tiles = upload(bytes_of(&terrain_tiles))?;
        let nodes = upload(nonempty(nodes))?;
        let scene_row_count = scene_rows.len() as u32;
        let scene_rows = driver(device.htod_sync_copy(&scene_rows))?;
        let centre_west_node = Window::of_tile(square.tile).west_node;
        let obstacle_tiles = upload(nonempty(bytes_of(&obstacle_tiles)))?;
        let blob = upload(nonempty(blob))?;
        let cell_max = driver(device.htod_sync_copy(&if cell_max.is_empty() {
            vec![0.0]
        } else {
            cell_max
        }))?;
        let weather_nodes = upload(weather.node_bytes().to_vec())?;
        let candidates_bytes = upload(nonempty(bytes_of(&candidates)))?;
        let attributes_bytes = upload(nonempty(bytes_of(&attributes)))?;
        let descriptor = SquareScene {
            ground: TerrainScene {
                tile_x_at_origin: (frame.origin.x - f64::from(square.tile.x)) as f32,
                tile_y_at_origin: (frame.origin.y - f64::from(square.tile.y)) as f32,
                tiles_per_metre_east: (1.0 / frame.east_m_per_unit) as f32,
                tiles_per_metre_north: (1.0 / frame.north_m_per_unit) as f32,
                radius,
                tiles: pointer(&terrain_tiles),
                nodes: pointer(&nodes),
                scene_rows: *scene_rows.device_ptr(),
                scene_row_count,
                column_at_origin: (frame.origin.x * columns_per_unit
                    - f64::from(180 * NODES_PER_DEGREE)
                    - f64::from(centre_west_node)) as f32,
                columns_per_metre: (columns_per_unit / frame.east_m_per_unit) as f32,
                has_fine: u32::from(has_fine),
                equator_row: if (south_row..=north_row).contains(&0) {
                    north_row
                } else {
                    -1
                },
            },
            obstacles: ObstacleScene {
                origin_x: layout.origin[0] as f32,
                origin_y: layout.origin[1] as f32,
                steps_per_metre_x: (1.0 / layout.metres_per_step[0]) as f32,
                steps_per_metre_y: (1.0 / layout.metres_per_step[1]) as f32,
                metres_per_step_x: layout.metres_per_step[0] as f32,
                metres_per_step_y: layout.metres_per_step[1] as f32,
                centre_x: layout.centre.x,
                centre_y: layout.centre.y,
                origin_cell_in_centre_x: layout.origin_cell_in_centre[0],
                origin_cell_in_centre_y: layout.origin_cell_in_centre[1],
                radius: layout.radius,
                tiles: pointer(&obstacle_tiles),
                blob: pointer(&blob),
                cell_max_height: *cell_max.device_ptr(),
            },
            weather_nodes: pointer(&weather_nodes),
            candidates: pointer(&candidates_bytes),
            attributes: pointer(&attributes_bytes),
            frame_origin_x: frame.origin.x as f32,
            frame_origin_y: frame.origin.y as f32,
            frame_east_m_per_unit: frame.east_m_per_unit as f32,
            frame_north_m_per_unit: frame.north_m_per_unit as f32,
        };
        let scene = upload(bytes_of(&[descriptor]))?;
        let buffers = vec![
            terrain_tiles,
            nodes,
            obstacle_tiles,
            blob,
            weather_nodes,
            candidates_bytes,
            attributes_bytes,
        ];
        let bytes = buffers.iter().map(|b| b.len() as u64).sum::<u64>()
            + 4 * (scene_rows.len() + cell_max.len()) as u64;
        Ok(DeviceSquare {
            scene,
            candidates: square.candidates.len() as u32,
            _buffers: buffers,
            _floats: vec![cell_max, scene_rows],
            bytes,
        })
    }
}
