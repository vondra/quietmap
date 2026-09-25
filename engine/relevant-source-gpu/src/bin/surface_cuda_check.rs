//! Bounded CUDA numerical check on synthetic scenes: every surface pair kernel against the CPU
//! physics it mirrors (noise-compute ray_transfer and line_piece), over flat ground of four
//! ground factors and a relief scene with buildings and walls, for a road, a rail row at both
//! source heights above a raised railhead (both with the track dipole) and a point. Checks
//! default weather and distinct period/direction probabilities and absorption moments; never paints tiles.
use anyhow::{ensure, Result};
use noise_compute::{
    compute::line_piece::{evaluate_line_piece, LinePiece, LinePieceScratch},
    constants::{A_WEIGHTING, DEFAULT_RECEIVER_HEIGHT},
    propagation::{
        line_quadrature::LineDirectivity,
        meteorology::Meteorology,
        obstacle_index::{ObstacleIndex, ObstacleKind, ObstacleSet},
        ray_transfer::{evaluate_ray_transfer, RayReceiver, RayScratch, RaySource, SourceGround, VARIANT_FULL},
    },
    types::RasterSampler,
};
use raster_reader::{FusedGrid, FusedPixel};
use relevant_source_gpu::{
    cuda_bridge::{DeviceBuffer, DeviceScenePointers, DeviceWeather, RelevantSourceCuda},
    obstacle_transfer::{DeviceRasterGeometry, FlattenedObstacleGeometry},
    source_frame::{DeviceLineSource, RegionMetricFrame, SOURCE_FLAG_POINT, SOURCE_FLAG_TRACK_DIPOLE},
};

const ORIGIN: (f64, f64) = (50.0, 14.0);
const EMISSION_DB: [f64; 8] = [70.0, 72.0, 75.0, 78.0, 80.0, 77.0, 72.0, 65.0];

/// One synthetic scene: a 1″ raster around the origin and its obstacles.
struct Scene {
    name: String,
    frame: RegionMetricFrame,
    grid: FusedGrid,
    obstacles: ObstacleSet,
}

/// A source as both lanes see it.
enum Source {
    Line { start: [f32; 2], end: [f32; 2], height_m: f64, ground: f64, platform_m: f64, directivity: LineDirectivity },
    Point { at: [f32; 2], height_m: f64, exclusion_m: f64 },
}

impl Source {
    fn device(&self) -> DeviceLineSource {
        let emission_linear = std::array::from_fn(|i| 10f32.powf(EMISSION_DB[i % 8] as f32 / 10.0));
        match *self {
            Source::Line { start, end, height_m, ground, platform_m, directivity } => DeviceLineSource {
                start_x_m: start[0],
                start_y_m: start[1],
                end_x_m: end[0],
                end_y_m: end[1],
                extent_m: (end[0] - start[0]).hypot(end[1] - start[1]),
                max_distance_m: 5000.0,
                source_height_m: height_m as f32,
                flags: if directivity == LineDirectivity::TrackDipole { SOURCE_FLAG_TRACK_DIPOLE } else { 0 },
                source_ground_factor: ground as f32,
                platform_half_width_m: platform_m as f32,
                emission_linear,
            },
            Source::Point { at, height_m, exclusion_m } => DeviceLineSource {
                start_x_m: at[0],
                start_y_m: at[1],
                end_x_m: at[0],
                end_y_m: at[1],
                extent_m: exclusion_m as f32,
                max_distance_m: 5000.0,
                source_height_m: height_m as f32,
                flags: SOURCE_FLAG_POINT,
                source_ground_factor: 0.0,
                platform_half_width_m: 0.0,
                emission_linear,
            },
        }
    }

    /// The CPU lane's A-weighted power per period at `receiver` (no reflection).
    fn cpu_power(&self, scene: &Scene, receiver: [f32; 2], weather: &Meteorology) -> [f64; 3] {
        let [lat, lon] = scene.frame.decode(receiver[0], receiver[1]);
        let ray_receiver = RayReceiver { lat, lon, altitude_m: scene.grid.elevation(lat, lon) + DEFAULT_RECEIVER_HEIGHT };
        let weights: [f64; 8] = std::array::from_fn(|band| 10f64.powf((EMISSION_DB[band] + A_WEIGHTING[band]) / 10.0));
        let transfer = match *self {
            Source::Line { start, end, height_m, ground, platform_m, directivity } => {
                let [start_lat, start_lon] = scene.frame.decode(start[0], start[1]);
                let [end_lat, end_lon] = scene.frame.decode(end[0], end[1]);
                evaluate_line_piece(
                    &ray_receiver,
                    &LinePiece {
                        start_lat,
                        start_lon,
                        end_lat,
                        end_lon,
                        source_height_m: height_m,
                        source_ground_factor: ground,
                        platform_half_width_m: platform_m,
                        directivity,
                    },
                    start_lat,
                    start_lon,
                    &scene.obstacles,
                    &scene.grid,
                    weather,
                    &mut LinePieceScratch::default(),
                    None,
                )
                .expect("the piece has length")
                .periods
            }
            Source::Point { at, height_m, exclusion_m } => {
                let [source_lat, source_lon] = scene.frame.decode(at[0], at[1]);
                let ray = evaluate_ray_transfer(
                    &ray_receiver,
                    &RaySource {
                        lat: source_lat,
                        lon: source_lon,
                        height_m,
                        ground: SourceGround::UnderSource,
                        platform_half_width_m: 0.0,
                        exclusion_radius_m: exclusion_m,
                    },
                    &scene.obstacles,
                    true,
                    &scene.grid,
                    weather,
                    false,
                    &mut RayScratch::default(),
                    None,
                );
                let distance = grid::geo::flat_dist(source_lat, source_lon, lat, lon);
                let source_altitude = scene.grid.elevation(source_lat, source_lon) + height_m;
                let slant = distance.max(exclusion_m).hypot(source_altitude - ray_receiver.altitude_m).max(1.0);
                let divergence = 10f64.powf(-(20.0 * slant.log10() + 11.0) / 10.0);
                ray.periods.map(|variants| variants.map(|bands| bands.map(|t| t * divergence)))
            }
        };
        std::array::from_fn(|period| (0..8).map(|band| weights[band] * transfer[period][VARIANT_FULL][band]).sum())
    }
}

/// The scene's arrays on the card.
struct Uploaded {
    sources: DeviceBuffer<DeviceLineSource>,
    raster: DeviceBuffer<FusedPixel>,
    grids: DeviceBuffer<relevant_source_gpu::obstacle_transfer::DeviceObstacleGrid>,
    starts: DeviceBuffer<u32>,
    references: DeviceBuffer<u32>,
    endpoints: DeviceBuffer<relevant_source_gpu::obstacle_transfer::DeviceObstacleEdgeEndpoints>,
    heights: DeviceBuffer<f32>,
    buildings: DeviceBuffer<u8>,
    footprints: DeviceBuffer<u32>,
    maximum_heights: DeviceBuffer<f32>,
    weather: DeviceBuffer<DeviceWeather>,
    raster_geometry: DeviceRasterGeometry,
    source_count: u32,
}

impl Uploaded {
    fn new(scene: &Scene, sources: &[DeviceLineSource], weather: &Meteorology) -> Result<Self> {
        let flat = FlattenedObstacleGeometry::from_set(&scene.frame, &scene.obstacles);
        Ok(Self {
            sources: DeviceBuffer::from_slice(sources)?,
            raster: DeviceBuffer::from_slice(scene.grid.pixels())?,
            grids: DeviceBuffer::from_slice(&flat.grids)?,
            starts: DeviceBuffer::from_slice(&flat.cell_starts)?,
            references: DeviceBuffer::from_slice(&flat.edge_references)?,
            endpoints: DeviceBuffer::from_slice(&flat.edge_endpoints)?,
            heights: DeviceBuffer::from_slice(&flat.edge_height_m)?,
            buildings: DeviceBuffer::from_slice(&flat.edge_is_building)?,
            footprints: DeviceBuffer::from_slice(&flat.edge_footprint_id)?,
            maximum_heights: DeviceBuffer::from_slice(&flat.cell_maximum_heights)?,
            weather: DeviceBuffer::from_slice(&[DeviceWeather::from_meteorology(weather)])?,
            raster_geometry: DeviceRasterGeometry::for_grid(&scene.frame, &scene.grid),
            source_count: sources.len() as u32,
        })
    }

    fn pointers(&self) -> DeviceScenePointers {
        DeviceScenePointers {
            sources: self.sources.as_ptr(),
            raster_pixels: self.raster.as_ptr(),
            obstacle_grids: self.grids.as_ptr(),
            obstacle_cell_starts: self.starts.as_ptr(),
            obstacle_edge_references: self.references.as_ptr(),
            obstacle_edge_endpoints: self.endpoints.as_ptr(),
            obstacle_edge_height_m: self.heights.as_ptr(),
            obstacle_cell_maximum_heights: self.maximum_heights.as_ptr(),
            obstacle_edge_is_building: self.buildings.as_ptr(),
            obstacle_edge_footprint_id: self.footprints.as_ptr(),
            weather: self.weather.as_ptr(),
            source_count: self.source_count,
            obstacle_grid_count: self.grids.element_count() as u32,
            pixel_floor_m: 1.0,
            raster_geometry: self.raster_geometry,
        }
    }
}

/// A raster of `half_extent_m` around the origin with elevation and IMD from `pixel(x, y)`.
fn raster(frame: &RegionMetricFrame, half_extent_m: f64, pixel: impl Fn(f64, f64) -> (f32, u8)) -> FusedGrid {
    let cell_deg = 1.0 / 3600.0;
    let rows = (2.0 * half_extent_m / (cell_deg * grid::geo::M_PER_DEG_LAT)).ceil() as usize + 2;
    let cols = (2.0 * half_extent_m / (cell_deg * frame.metres_per_longitude_degree())).ceil() as usize + 2;
    let lat_min = ORIGIN.0 - half_extent_m / grid::geo::M_PER_DEG_LAT;
    let lon_min = ORIGIN.1 - half_extent_m / frame.metres_per_longitude_degree();
    let mut data = Vec::with_capacity(rows * cols);
    for row in 0..rows {
        for column in 0..cols {
            let [x, y] = frame.encode(lat_min + row as f64 * cell_deg, lon_min + column as f64 * cell_deg);
            let (elevation, imd) = pixel(f64::from(x), f64::from(y));
            data.push(FusedPixel { elevation, forest: 0, imd, canopy_m: 0 });
        }
    }
    FusedGrid::from_pixels(lat_min, lon_min, rows, cols, data)
}

fn flat_scene(imd: u8) -> Scene {
    let frame = RegionMetricFrame::for_latitude_longitude(ORIGIN.0, ORIGIN.1);
    Scene {
        name: format!("flat imd={imd}"),
        grid: raster(&frame, 1500.0, |_, _| (0.0, imd)),
        frame,
        obstacles: ObstacleSet::empty(),
    }
}

/// A 40 m ridge north of the sources, a sealed strip, a row of buildings of varying height (two
/// of them touching, one overlapping), a courtyard block and two walls.
fn relief_scene() -> Scene {
    let frame = RegionMetricFrame::for_latitude_longitude(ORIGIN.0, ORIGIN.1);
    let grid = raster(&frame, 1500.0, |x, y| {
        let ridge = 40.0 * (-((y - 420.0) / 90.0).powi(2)).exp() + 0.004 * x;
        let imd = if (y - 150.0).abs() < 40.0 {
            100
        } else if x > 300.0 {
            30
        } else {
            0
        };
        (ridge as f32, imd)
    });
    let at = |x: f64, y: f64| {
        let [lat, lon] = frame.decode(x as f32, y as f32);
        (lat, lon)
    };
    let rectangle = |x0: f64, y0: f64, x1: f64, y1: f64| vec![at(x0, y0), at(x1, y0), at(x1, y1), at(x0, y1)];
    let mut builder = ObstacleIndex::builder(ORIGIN.0, ORIGIN.1);
    for (index, (x0, height)) in [(-200.0, 9.0), (-170.0, 15.0), (-140.0, 12.0), (-60.0, 24.0), (40.0, 6.0)]
        .into_iter()
        .enumerate()
    {
        builder.add_ring(&rectangle(x0, 80.0, x0 + 30.0, 110.0), height, ObstacleKind::Building, index as u32);
    }
    builder.add_ring(&rectangle(-50.0, 90.0, -20.0, 130.0), 18.0, ObstacleKind::Building, 5);
    builder.add_ring(&rectangle(120.0, 200.0, 220.0, 300.0), 20.0, ObstacleKind::Building, 6);
    builder.add_ring(&rectangle(140.0, 220.0, 200.0, 280.0), 20.0, ObstacleKind::Building, 6);
    builder.add_polyline(&[at(-300.0, 40.0), at(-100.0, 40.0)], 3.5, ObstacleKind::Barrier, 7);
    builder.add_polyline(&[at(250.0, 60.0), at(400.0, 70.0)], 5.0, ObstacleKind::Barrier, 8);
    Scene {
        name: "relief with buildings and walls".to_owned(),
        frame,
        grid,
        obstacles: ObstacleSet { indexes: vec![std::sync::Arc::new(builder.build())] },
    }
}

fn main() -> Result<()> {
    let cuda = RelevantSourceCuda::initialize()?;
    let default_weather = Meteorology::defaults();
    let mut varied_weather = default_weather.clone();
    varied_weather.favourable_probability = std::array::from_fn(|period|
        std::array::from_fn(|sector| ((sector + 3 * period) % 17) as f64 / 16.0));
    varied_weather.absorption = std::array::from_fn(|period| std::array::from_fn(|band| {
        let mean = default_weather.absorption[period][band].mean_db_per_km * (0.75 + 0.25 * period as f64);
        noise_compute::propagation::air_absorption::AbsorptionClimate {
            mean_db_per_km: mean,
            variance_db2_per_km2: 0.25 * mean * mean,
            minimum_db_per_km: 0.1 * mean,
        }
    }));
    // Formation is the raster datum; this known offset puts A/B at 0.5/4.0 m above railhead.
    let railhead_offset_m = 0.7;
    let sources = [
        Source::Line {
            start: [-125.0, 0.0],
            end: [125.0, 5.0],
            height_m: 0.05,
            ground: 0.0,
            platform_m: 5.0,
            directivity: LineDirectivity::Omnidirectional,
        },
        Source::Line {
            start: [-300.0, -20.0],
            end: [-60.0, 30.0],
            height_m: railhead_offset_m + 0.5,
            ground: 1.0,
            platform_m: 2.5,
            directivity: LineDirectivity::TrackDipole,
        },
        Source::Line {
            start: [-300.0, -20.0],
            end: [-60.0, 30.0],
            height_m: railhead_offset_m + 4.0,
            ground: 1.0,
            platform_m: 2.5,
            directivity: LineDirectivity::TrackDipole,
        },
        Source::Point { at: [180.0, 250.0], height_m: 4.0, exclusion_m: 20.0 },
    ];
    let mut receivers: Vec<[f32; 2]> = (-3..=3)
        .flat_map(|i| (-1..=5).map(move |j| [i as f32 * 140.0 + 7.0, j as f32 * 110.0 + 3.0]))
        .collect();
    // Horizontal directivity differs most from the old 3D angle on/near the track axis;
    // larger offsets exercise the near-coincident quadratic limit at source B's height.
    receivers.extend([5.0, 5.001, 5.01, 5.1, 6.0, 10.0, 15.0].map(|y| [-180.0, y]));
    receivers.extend([[-420.0, -45.0], [-420.0, -44.0]]);
    let mut worst = 0.0_f64;
    for (weather_name, weather) in [("default", default_weather), ("period-direction-moments", varied_weather)] {
        println!("weather: {weather_name}");
        for (scene, limit_db) in [
            (flat_scene(100), 0.05),
            (flat_scene(99), 0.05),
            (flat_scene(50), 0.05),
            (flat_scene(0), 0.05),
            (relief_scene(), 0.5),
        ] {
            let devices: Vec<_> = sources.iter().map(Source::device).collect();
            let uploaded = Uploaded::new(&scene, &devices, &weather)?;
            let pointers = uploaded.pointers();
            for (source_index, source) in sources.iter().enumerate() {
                let count = receivers.len();
                let (gpu, milliseconds) = cuda.evaluate_corners(
                    &pointers,
                    &DeviceBuffer::from_slice(&vec![1.0; count])?,
                    &DeviceBuffer::from_slice(&(0..=count as u32).collect::<Vec<_>>())?,
                    &DeviceBuffer::from_slice(&vec![source_index as u32; count])?,
                    &DeviceBuffer::from_slice(&receivers.iter().map(|r| r[0]).collect::<Vec<_>>())?,
                    &DeviceBuffer::from_slice(&receivers.iter().map(|r| r[1]).collect::<Vec<_>>())?,
                    &DeviceBuffer::from_slice(&vec![0.0; count])?,
                )?;
                let (mut largest, mut over_tenth, mut compared) = (0.0_f64, 0, 0);
                for (receiver, gpu_power) in receivers.iter().zip(&gpu) {
                    let cpu_power = source.cpu_power(&scene, *receiver, &weather);
                    for period in 0..3 {
                        let (cpu, gpu) = (cpu_power[period], f64::from(gpu_power[period]));
                        if cpu < 1e-3 && gpu < 1e-3 {
                            continue;
                        }
                        let error_db = 10.0 * (gpu / cpu).log10();
                        ensure!(error_db.is_finite(), "{}: source {source_index} at {receiver:?}: cpu {cpu} gpu {gpu}", scene.name);
                        compared += 1;
                        if error_db.abs() > 0.1 {
                            over_tenth += 1;
                            println!(
                                "  {} source {source_index} receiver {receiver:?} period {period}: cpu {:.3} dB gpu {:.3} dB",
                                scene.name,
                                10.0 * cpu.log10(),
                                10.0 * gpu.log10()
                            );
                        }
                        largest = largest.max(error_db.abs());
                    }
                }
                println!(
                    "{}: source {source_index}: {compared} values, max |error| {largest:.4} dB, {over_tenth} over 0.1 dB, kernel {milliseconds:.1} ms",
                    scene.name
                );
                ensure!(largest <= limit_db, "{}: source {source_index} differs by {largest:.3} dB (limit {limit_db})", scene.name);
                worst = worst.max(largest);
            }
        }
    }
    println!("all scenes within their limits; worst {worst:.4} dB");
    Ok(())
}
