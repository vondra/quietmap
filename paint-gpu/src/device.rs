//! One card and one square on it: the kernels compiled for the card (NVRTC), the square's terrain
//! and obstacles files as stored, its candidates and attributes, the weather table, and the pair
//! evaluation. The device works in f32 in the square's frame: here, in f64, every absolute
//! coordinate (Mercator, terrain nodes) becomes an offset from the frame's origin. The `#[repr(C)]`
//! structs mirror kernels/scene.cuh, evaluate.cuh and paint.cu field by field.

use cudarc::driver::sys::CUdevice_attribute;
use cudarc::driver::{CudaDevice, CudaSlice, DevicePtr, DeviceSlice, LaunchAsync, LaunchConfig};
use cudarc::nvrtc::{CompileOptions, compile_ptx_with_opts};
use paint::square::{Files, Square};
use physics::bands::{BAND_FREQUENCY_HZ, BANDS, PERIODS, SPEED_OF_SOUND_M_PER_S};
use physics::weather::WeatherTable;
use std::collections::HashMap;
use std::sync::Arc;
use tiles::Kind;
use tiles::geo::{TILES_PER_AXIS, TileId};
use tiles::sources::GROUND_FROM_TERRAIN;
use tiles::terrain::{NODES_PER_DEGREE, Terrain};

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

/// A pair's failure (kernels/common.cuh): the host evaluates such a pair on the CPU.
pub const FAILED_NOT_READ: u32 = 1;
pub const FAILED_NO_TERRAIN: u32 = 2;
pub const FAILED_CAPACITY: u32 = 4;

/// Threads per block of the pair kernel, one pair a thread: one warp, 5 % faster than 64 and
/// 15-20 % faster than 256 (RTX 5070, pairs within 1 km grouped by 32, 2026-10-09).
const THREADS_PER_BLOCK: u32 = 32;

const TILE_NOT_READ: i32 = 0;
const TILE_EMPTY: i32 = 1;
const TILE_READ: i32 = 2;
const TERRAIN_HEADER_BYTES: usize = 24;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct TerrainTile {
    state: i32,
    rows: u32,
    columns: u32,
    pad: u32,
    column_at_origin: f32,
    columns_per_metre: f32,
    nodes_offset: u64,
    rows_offset: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct TerrainScene {
    tile_x_at_origin: f32,
    tile_y_at_origin: f32,
    tiles_per_metre_east: f32,
    tiles_per_metre_north: f32,
    centre_x: u32,
    centre_y: u32,
    radius: i64,
    tiles: u64,
    nodes: u64,
    row_north_m: u64,
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

fn driver<T>(result: Result<T, cudarc::driver::DriverError>) -> Result<T, String> {
    result.map_err(|error| format!("cuda: {error}"))
}

/// One card with the kernels loaded.
pub struct Gpu {
    device: Arc<CudaDevice>,
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
        let source = format!("{COMMON}{}{PHYSICS}", band_constants());
        let ptx = compile_ptx_with_opts(
            source,
            CompileOptions {
                arch: Some(arch),
                maxrregcount: std::env::var("QM_GPU_REGISTERS")
                    .ok()
                    .and_then(|v| v.parse().ok()),
                options: {
                    let mut options = Vec::new();
                    if std::env::var("QM_GPU_FAST_MATH").is_ok_and(|v| v == "1") {
                        options.push("--use_fast_math".into());
                    }
                    if std::env::var("QM_GPU_LINEINFO").is_ok_and(|v| v == "1") {
                        options.push("-lineinfo".into());
                    }
                    options
                },
                ..Default::default()
            },
        )
        .map_err(|error| format!("nvrtc: {error}"))?;
        driver(device.load_ptx(
            ptx,
            "paint",
            &[
                "evaluate_pairs",
                "ray_crossings",
                "line_pair_nodes",
                "struct_sizes",
            ],
        ))?;
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

    /// Per pair (receiver position in the square's frame, building it stands in or 0, candidate
    /// index): the period energies the candidate delivers there and the failure, 0 for none.
    pub fn evaluate_pairs(
        &self,
        square: &DeviceSquare,
        receivers: &[[f64; 2]],
        own_footprint: &[u64],
        candidates: &[u32],
    ) -> Result<(Vec<[f64; PERIODS]>, Vec<u32>), String> {
        let count = candidates.len();
        if receivers.len() != count || own_footprint.len() != count {
            return Err("evaluate_pairs: a receiver and an owner per candidate".into());
        }
        if candidates.iter().any(|&index| index >= square.candidates) {
            return Err("evaluate_pairs: a candidate outside the square".into());
        }
        if count == 0 {
            return Ok((Vec::new(), Vec::new()));
        }
        let xy: Vec<f32> = receivers.iter().flatten().map(|&v| v as f32).collect();
        let xy = driver(self.device.htod_sync_copy(&xy))?;
        let own = driver(self.device.htod_sync_copy(own_footprint))?;
        let index = driver(self.device.htod_sync_copy(candidates))?;
        let periods = driver(self.device.alloc_zeros::<f32>(PERIODS * count))?;
        let failed = driver(self.device.alloc_zeros::<u32>(count))?;
        let function = self
            .device
            .get_func("paint", "evaluate_pairs")
            .ok_or("kernel evaluate_pairs missing")?;
        let config = LaunchConfig {
            grid_dim: ((count as u32).div_ceil(THREADS_PER_BLOCK), 1, 1),
            block_dim: (THREADS_PER_BLOCK, 1, 1),
            shared_mem_bytes: 0,
        };
        // SAFETY: every buffer holds what the kernel indexes: 2 receiver coordinates, an owner and
        // a candidate per pair in, 3 periods and a failure per pair out.
        driver(unsafe {
            function.launch(
                config,
                (
                    &square.scene,
                    &xy,
                    &own,
                    &index,
                    count as u32,
                    &periods,
                    &failed,
                ),
            )
        })?;
        let periods = driver(self.device.dtoh_sync_copy(&periods))?;
        let failed = driver(self.device.dtoh_sync_copy(&failed))?;
        Ok((
            periods
                .chunks_exact(PERIODS)
                .map(|chunk| [0, 1, 2].map(|period| f64::from(chunk[period])))
                .collect(),
            failed,
        ))
    }
}

/// One crossing of a ray as the device walks it: t along the ray, footprint, height, building.
pub type DeviceCrossing = (f32, u64, f32, bool);

impl Gpu {
    /// The crossings of the segment `from -> to` (frame metres) in the device walk's order, and
    /// the walk's failure (0 for none).
    pub fn ray_crossings(
        &self,
        square: &DeviceSquare,
        from: [f64; 2],
        to: [f64; 2],
    ) -> Result<(Vec<DeviceCrossing>, u32), String> {
        const CAPACITY: usize = 4096;
        let t = driver(self.device.alloc_zeros::<f32>(CAPACITY))?;
        let footprint = driver(self.device.alloc_zeros::<u64>(CAPACITY))?;
        let height = driver(self.device.alloc_zeros::<f32>(CAPACITY))?;
        let building = driver(self.device.alloc_zeros::<u32>(CAPACITY))?;
        let out = driver(self.device.alloc_zeros::<u32>(2))?;
        let function = self
            .device
            .get_func("paint", "ray_crossings")
            .ok_or("kernel ray_crossings missing")?;
        // SAFETY: the kernel writes at most CAPACITY crossings and two counts.
        driver(unsafe {
            function.launch(
                ONE_THREAD,
                (
                    &square.scene,
                    from[0] as f32,
                    from[1] as f32,
                    to[0] as f32,
                    to[1] as f32,
                    &t,
                    &footprint,
                    &height,
                    &building,
                    CAPACITY as u32,
                    &out,
                ),
            )
        })?;
        let out = driver(self.device.dtoh_sync_copy(&out))?;
        let count = (out[0] as usize).min(CAPACITY);
        let (t, footprint) = (
            driver(self.device.dtoh_sync_copy(&t))?,
            driver(self.device.dtoh_sync_copy(&footprint))?,
        );
        let (height, building) = (
            driver(self.device.dtoh_sync_copy(&height))?,
            driver(self.device.dtoh_sync_copy(&building))?,
        );
        Ok((
            (0..count)
                .map(|k| (t[k], footprint[k], height[k], building[k] != 0))
                .collect(),
            out[1],
        ))
    }
}

/// One line node as the device places it: along the piece (m), weight (rad), whether obstacles
/// stand on its ray, its weighted period energies.
pub type DeviceNode = (f32, f32, bool, [f64; PERIODS]);

impl Gpu {
    /// The quadrature nodes of one line pair (receiver outdoors at `receiver`, frame metres) and
    /// the failure (0 for none).
    pub fn line_pair_nodes(
        &self,
        square: &DeviceSquare,
        receiver: [f64; 2],
        candidate: u32,
    ) -> Result<(Vec<DeviceNode>, u32), String> {
        const CAPACITY: usize = 512;
        let along = driver(self.device.alloc_zeros::<f32>(CAPACITY))?;
        let weight = driver(self.device.alloc_zeros::<f32>(CAPACITY))?;
        let blocked = driver(self.device.alloc_zeros::<u32>(CAPACITY))?;
        let periods = driver(self.device.alloc_zeros::<f32>(PERIODS * CAPACITY))?;
        let out = driver(self.device.alloc_zeros::<u32>(2))?;
        let function = self
            .device
            .get_func("paint", "line_pair_nodes")
            .ok_or("kernel line_pair_nodes missing")?;
        // SAFETY: the kernel writes at most CAPACITY nodes and two counts.
        driver(unsafe {
            function.launch(
                LaunchConfig {
                    grid_dim: (1, 1, 1),
                    block_dim: (1, 1, 1),
                    shared_mem_bytes: 0,
                },
                (
                    &square.scene,
                    receiver[0] as f32,
                    receiver[1] as f32,
                    candidate,
                    &along,
                    &weight,
                    &blocked,
                    &periods,
                    CAPACITY as u32,
                    &out,
                ),
            )
        })?;
        let out = driver(self.device.dtoh_sync_copy(&out))?;
        let count = (out[0] as usize).min(CAPACITY);
        let (along, weight) = (
            driver(self.device.dtoh_sync_copy(&along))?,
            driver(self.device.dtoh_sync_copy(&weight))?,
        );
        let (blocked, periods) = (
            driver(self.device.dtoh_sync_copy(&blocked))?,
            driver(self.device.dtoh_sync_copy(&periods))?,
        );
        Ok((
            (0..count)
                .map(|k| {
                    (
                        along[k],
                        weight[k],
                        blocked[k] != 0,
                        [0, 1, 2].map(|p| f64::from(periods[PERIODS * k + p])),
                    )
                })
                .collect(),
            out[1],
        ))
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
        let (mut nodes, mut row_north_m) = (Vec::new(), Vec::<f32>::new());
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
                        *slot = TerrainTile {
                            state: TILE_READ,
                            rows: window.rows,
                            columns: window.columns,
                            pad: 0,
                            column_at_origin: column_at_origin as f32,
                            columns_per_metre: (columns_per_unit / frame.east_m_per_unit) as f32,
                            nodes_offset: append_aligned(
                                &mut nodes,
                                &bytes[TERRAIN_HEADER_BYTES..],
                            ),
                            rows_offset: row_north_m.len() as u64,
                        };
                        row_north_m.extend(
                            terrain
                                .row_mercator_y()
                                .iter()
                                .map(|&y| ((frame.origin.y - y) * frame.north_m_per_unit) as f32),
                        );
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
            .iter()
            .map(|candidate| {
                let [a, b] = candidate.ends_m;
                DeviceCandidate {
                    ax: a[0] as f32,
                    ay: a[1] as f32,
                    bx: b[0] as f32,
                    by: b[1] as f32,
                    ground0: candidate.ground_m[0] as f32,
                    ground1: candidate.ground_m[1] as f32,
                    line: u32::from(candidate.line),
                    attribute: first_of_list[candidate.attribute.list as usize]
                        + candidate.attribute.index,
                }
            })
            .collect();
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
        let terrain_tiles = upload(bytes_of(&terrain_tiles))?;
        let nodes = upload(nonempty(nodes))?;
        let row_north_m = driver(device.htod_sync_copy(&if row_north_m.is_empty() {
            vec![0.0]
        } else {
            row_north_m
        }))?;
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
                centre_x: square.tile.x,
                centre_y: square.tile.y,
                radius,
                tiles: pointer(&terrain_tiles),
                nodes: pointer(&nodes),
                row_north_m: *row_north_m.device_ptr(),
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
            + 4 * (row_north_m.len() + cell_max.len()) as u64;
        Ok(DeviceSquare {
            scene,
            candidates: square.candidates.len() as u32,
            _buffers: buffers,
            _floats: vec![row_north_m, cell_max],
            bytes,
        })
    }
}
