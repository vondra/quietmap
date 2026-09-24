//! Bounded CUDA ABI and numerical check; never paints or publishes map tiles.
use anyhow::{ensure, Result};
use noise_compute::{
    constants::{ALPHA_ATM, A_WEIGHTING, DEFAULT_RECEIVER_HEIGHT},
    propagation::{
        iso9613::ground_atten_bands,
        path_effects::cnossos_ground_path_from_profile,
        path_profile::{fill_t_values, PathProfile},
    },
};
use relevant_source_gpu::{
    cuda_bridge::{DeviceBuffer, DeviceScenePointers, RelevantSourceCuda},
    obstacle_transfer::DeviceRasterGeometry,
    source_frame::{DeviceLineSource, SOURCE_FLAG_POINT},
};
fn main() -> Result<()> {
    for imd in [100, 99, 50, 0] {
        check_uniform_ground(imd)?;
        check_uniform_line(imd)?;
    }
    Ok(())
}

/// Flat ground at 0 m with one ground factor, the CPU twin of the check's raster.
struct FlatGround {
    ground_g: f64,
}

impl noise_compute::types::RasterSampler for FlatGround {
    fn elevation(&self, _: f64, _: f64) -> f64 {
        0.0
    }
    fn ground_g(&self, _: f64, _: f64) -> f64 {
        self.ground_g
    }
}

/// A 250 m road piece over flat uniform ground: the CUDA line quadrature against the CPU line
/// piece (`noise_compute::compute::line_piece`) at receivers beside, in front of and past it.
fn check_uniform_line(imd: u8) -> Result<()> {
    use noise_compute::compute::line_piece::{evaluate_line_piece, LinePiece, LinePieceScratch};
    use noise_compute::propagation::obstacle_index::ObstacleSet;
    use noise_compute::propagation::ray_transfer::{RayReceiver, VARIANT_FULL};
    let cuda = RelevantSourceCuda::initialize()?;
    let (start, end) = ([-125.0_f32, 60.0_f32], [125.0_f32, 60.0_f32]);
    let emission_db: [f64; 8] = [70.0, 72.0, 75.0, 78.0, 80.0, 77.0, 72.0, 65.0];
    let source = DeviceLineSource {
        start_x_m: start[0],
        start_y_m: start[1],
        end_x_m: end[0],
        end_y_m: end[1],
        extent_m: 250.0,
        max_distance_m: 5000.0,
        source_height_m: 0.05,
        flags: 0,
        emission_linear: std::array::from_fn(|i| 10f32.powf(emission_db[i % 8] as f32 / 10.0)),
    };
    let sources = DeviceBuffer::from_slice(&[source])?;
    let raster = DeviceBuffer::from_slice(&vec![
        raster_reader::FusedPixel {
            elevation: 0.0,
            forest: 0,
            imd,
            _pad: 0
        };
        256 * 256
    ])?;
    let scene = flat_scene(&sources, &raster);
    let receivers: [[f32; 2]; 6] = [[0.0, 0.0], [0.0, 45.0], [0.0, 58.0], [60.0, 50.0], [300.0, 20.0], [-40.0, 600.0]];
    let count = receivers.len();
    let (result, _) = cuda.evaluate_corners(
        &scene,
        &DeviceBuffer::from_slice(&vec![1.0; count])?,
        &DeviceBuffer::from_slice(&(0..=count as u32).collect::<Vec<_>>())?,
        &DeviceBuffer::from_slice(&vec![0; count])?,
        &DeviceBuffer::from_slice(&receivers.map(|r| r[0]))?,
        &DeviceBuffer::from_slice(&receivers.map(|r| r[1]))?,
        &DeviceBuffer::from_slice(&vec![0.0; count])?,
    )?;
    let geo = |x: f32, y: f32| {
        (
            f64::from(y) / noise_compute::constants::M_PER_DEG_LAT,
            f64::from(x) / noise_compute::constants::m_per_deg_lon(0.0),
        )
    };
    let world = FlatGround {
        ground_g: 1.0 - f64::from(imd) / 100.0,
    };
    for (index, receiver) in receivers.iter().enumerate() {
        let (lat, lon) = geo(receiver[0], receiver[1]);
        let (start_lat, start_lon) = geo(start[0], start[1]);
        let (end_lat, end_lon) = geo(end[0], end[1]);
        let transfer = evaluate_line_piece(
            &RayReceiver {
                lat,
                lon,
                altitude_m: DEFAULT_RECEIVER_HEIGHT,
            },
            &LinePiece {
                start_lat,
                start_lon,
                end_lat,
                end_lon,
                source_height_m: 0.05,
                on_bridge: false,
            },
            start_lat,
            start_lon,
            &ObstacleSet::empty(),
            &world,
            &mut LinePieceScratch::default(),
            None,
        )
        .expect("the piece has length");
        for (period, &gpu_energy) in result[index].iter().enumerate() {
            let expected: f64 = (0..8)
                .map(|band| {
                    10f64.powf((emission_db[band] + A_WEIGHTING[band]) / 10.0)
                        * transfer.periods[period][VARIANT_FULL][band]
                })
                .sum();
            let error_db = 10.0 * (f64::from(gpu_energy) / expected).log10();
            println!("line imd={imd} receiver={receiver:?} period={period} cpu_power={expected:.6} gpu_power={gpu_energy:.6} error_db={error_db:.6}");
            ensure!(
                error_db.is_finite() && error_db.abs() < 0.05,
                "CUDA line quadrature differs from the CPU line piece"
            );
        }
    }
    Ok(())
}

fn flat_scene(sources: &DeviceBuffer<DeviceLineSource>, raster: &DeviceBuffer<raster_reader::FusedPixel>) -> DeviceScenePointers {
    DeviceScenePointers {
        sources: sources.as_ptr(),
        raster_pixels: raster.as_ptr(),
        obstacle_grids: std::ptr::null(),
        obstacle_cell_starts: std::ptr::null(),
        obstacle_edge_references: std::ptr::null(),
        obstacle_edge_endpoints: std::ptr::null(),
        obstacle_edge_height_m: std::ptr::null(),
        obstacle_cell_maximum_heights: std::ptr::null(),
        obstacle_edge_is_building: std::ptr::null(),
        source_count: 1,
        obstacle_grid_count: 0,
        pixel_floor_m: 1.0,
        raster_geometry: DeviceRasterGeometry {
            row_scale_per_metre: 0.1,
            column_scale_per_metre: 0.1,
            row_offset: 128.0,
            column_offset: 128.0,
            rows: 256,
            columns: 256,
        },
    }
}
fn check_uniform_ground(imd: u8) -> Result<()> {
    let cuda = RelevantSourceCuda::initialize()?;
    let source = DeviceLineSource {
        max_distance_m: 1000.0,
        source_height_m: 4.0,
        flags: SOURCE_FLAG_POINT,
        emission_linear: std::array::from_fn(|i| 1e10_f32 * (i / 8 + 1) as f32),
        ..Default::default()
    };
    let sources = DeviceBuffer::from_slice(&[source])?;
    let raster = DeviceBuffer::from_slice(&vec![
        raster_reader::FusedPixel {
            elevation: 0.0,
            forest: 0,
            imd,
            _pad: 0
        };
        256 * 256
    ])?;
    let scene = flat_scene(&sources, &raster);
    let distance = [10.0_f32, 100.0, 500.0, 101.3, 499.9, 500.1, 999.9];
    let count = distance.len();
    let (result, milliseconds) = cuda.evaluate_corners(
        &scene,
        &DeviceBuffer::from_slice(&vec![1.0; count])?,
        &DeviceBuffer::from_slice(&(0..=count as u32).collect::<Vec<_>>())?,
        &DeviceBuffer::from_slice(&vec![0; count])?,
        &DeviceBuffer::from_slice(&distance)?,
        &DeviceBuffer::from_slice(&vec![0.0; count])?,
        &DeviceBuffer::from_slice(&vec![0.0; count])?,
    )?;
    for (index, energy) in result.iter().enumerate() {
        let d = f64::from(distance[index]);
        let slant = d.hypot(4.0 - DEFAULT_RECEIVER_HEIGHT);
        let mut profile = PathProfile::new();
        profile.dist_m = d;
        fill_t_values(d, &mut profile.t);
        profile.elevation_m = vec![0.0; profile.t.len()];
        profile.imd_u8 = vec![imd; profile.t.len()];
        let ground = ground_atten_bands(cnossos_ground_path_from_profile(
            &mut profile,
            4.0,
            DEFAULT_RECEIVER_HEIGHT,
            false,
        ));
        for (period, &gpu_energy) in energy.iter().enumerate() {
            let expected: f64 = (0..8)
                .map(|band| {
                    f64::from(source.emission_linear[period * 8 + band])
                        * 10.0_f64.powf(
                            (-20.0 * slant.log10() - 11.0 - ALPHA_ATM[band] * slant / 1000.0
                                + A_WEIGHTING[band]
                                - ground[band])
                                / 10.0,
                        )
                })
                .sum();
            let error_db = 10.0 * (f64::from(gpu_energy) / expected).log10();
            println!("imd={imd} distance_m={d} period={period} cpu_power={expected:.9} gpu_power={:.9} error_db={error_db:.9}",gpu_energy);
            ensure!(
                error_db.is_finite() && error_db.abs() < 0.01,
                "CUDA point baseline differs from shared CPU physics"
            );
        }
    }
    println!("imd={imd} kernel_ms={milliseconds}");
    Ok(())
}
