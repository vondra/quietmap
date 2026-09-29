//! Real CPU decoding and CUDA sampling of 100 reproducible synthetic-square nodes.
use crate::cuda_bridge::{check_cuda, DeviceBuffer, DeviceScenePointers, RelevantSourceCuda};
use crate::obstacle_transfer::DeviceRasterGeometry;
use noise_compute::types::RasterSampler;
use raster_reader::{channel::Channel, FusedGrid, RealRasters};

unsafe extern "C" {
    fn relevant_source_cuda_sample_raster_contract(
        scene: *const DeviceScenePointers,
        count: u32,
        columns: *const f32,
        rows: *const f32,
        elevation: *mut f32,
        canopy: *mut u8,
    ) -> i32;
}

#[test]
fn synthetic_square_cpu_cuda_nodes_agree_bit_for_bit() -> anyhow::Result<()> {
    let _cuda = RelevantSourceCuda::initialize()?;
    let directory = tempfile::tempdir()?;
    let square = grid::square_of(50.0, 14.0);
    let window = grid::raster::RasterWindow::for_square(square);
    for channel in Channel::ALL {
        let mut bytes = Vec::with_capacity(channel.byte_len(window));
        for row in 0..window.rows {
            for column in 0..window.columns {
                let value = match channel {
                    Channel::Dem => -200.0 + f64::from((row * 13 + column * 7) % 60000) / 5.0,
                    Channel::Canopy => f64::from((row + column * 3) % 251),
                    _ => 50.0,
                };
                bytes.extend_from_slice(&channel.encode(value)[..channel.bytes_per_node()]);
            }
        }
        let path = channel.path(directory.path(), square);
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(path, bytes)?;
    }
    let real = RealRasters::new(directory.path());
    let fused = FusedGrid::build(&real, 49.999, 50.001, 13.999, 14.001);
    let (latitude, longitude, density, rows, columns) = fused.geom();
    let pixels = DeviceBuffer::from_slice(fused.pixels())?;
    let mut scene: DeviceScenePointers = unsafe { std::mem::zeroed() };
    scene.raster_pixels = pixels.as_ptr();
    scene.raster_geometry = DeviceRasterGeometry {
        row_scale_per_metre: 1.0,
        row_offset: 0.0,
        column_scale_per_metre: 1.0,
        column_offset: 0.0,
        rows: rows as u32,
        columns: columns as u32,
    };
    let mut seed = 1729_u32;
    let mut rr = Vec::new();
    let mut cc = Vec::new();
    let mut expected = Vec::new();
    for _ in 0..100 {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        let row = 1 + seed as usize % (rows - 2);
        let column = 1 + (seed >> 8) as usize % (columns - 2);
        rr.push(row as f32);
        cc.push(column as f32);
        let lat = latitude + row as f64 / density;
        let lon = longitude + column as f64 / density;
        expected.push((
            (real.elevation(lat, lon) as f32).to_bits(),
            real.canopy.sample(lat, lon) as u8,
        ));
    }
    let rr = DeviceBuffer::from_slice(&rr)?;
    let cc = DeviceBuffer::from_slice(&cc)?;
    let elevations = DeviceBuffer::<f32>::uninitialized(100)?;
    let canopies = DeviceBuffer::<u8>::uninitialized(100)?;
    check_cuda(unsafe {
        relevant_source_cuda_sample_raster_contract(
            &scene,
            100,
            cc.as_ptr(),
            rr.as_ptr(),
            elevations.as_mut_ptr(),
            canopies.as_mut_ptr(),
        )
    })?;
    let actual: Vec<_> = elevations
        .copy_to_vec()?
        .into_iter()
        .zip(canopies.copy_to_vec()?)
        .map(|(height, canopy)| (height.to_bits(), canopy))
        .collect();
    assert_eq!(actual, expected);
    Ok(())
}
