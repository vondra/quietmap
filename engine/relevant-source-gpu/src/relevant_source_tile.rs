//! Tile uploads, shared-corner partition construction and the exact pixel paint.

use crate::cuda_bridge::{DeviceBuffer, DeviceScenePointers, RelevantSourceCuda};
use crate::obstacle_transfer::{
    DeviceObstacleGrid, DeviceRasterGeometry, FlattenedObstacleGeometry,
};
use crate::relevance_partition::build_relevant_source_partition;
use crate::source_frame::{
    DeviceLineSource, RegionMetricFrame, BLOCK_COUNT, CORNER_COUNT, PERIOD_COUNT, TILE_PIXEL_SIDE,
};
use crate::tile_source_incidence::{build_tile_source_incidence, TileMetricLattice};
use anyhow::{bail, Result};
use noise_compute::constants::ENCLOSURE_RADIUS_M;
use noise_compute::propagation::obstacle_index::{enclosure_db, ObstacleSet};
use raster_reader::fused_tile_z13::{tile_pixel_size_m, FusedTileZ13};

/// Device-resident vector obstacle geometry shared across both line layers and all region tiles.
pub struct RegionDeviceObstacles {
    obstacle_grids: DeviceBuffer<DeviceObstacleGrid>,
    obstacle_cell_starts: DeviceBuffer<u32>,
    obstacle_edge_references: DeviceBuffer<u32>,
    obstacle_edge_values: DeviceBuffer<f32>,
    obstacle_edge_is_building: DeviceBuffer<u8>,
    obstacle_cell_maximum_heights: DeviceBuffer<f32>,
    obstacle_grid_count: u32,
}

impl RegionDeviceObstacles {
    pub fn upload(obstacles: &FlattenedObstacleGeometry) -> Result<Self> {
        Ok(Self {
            obstacle_grids: DeviceBuffer::from_slice(&obstacles.grids)?,
            obstacle_cell_starts: DeviceBuffer::from_slice(&obstacles.cell_starts)?,
            obstacle_edge_references: DeviceBuffer::from_slice(&obstacles.edge_references)?,
            obstacle_edge_values: DeviceBuffer::from_slice(&obstacles.edge_values_xyxyh)?,
            obstacle_edge_is_building: DeviceBuffer::from_slice(&obstacles.edge_is_building)?,
            obstacle_cell_maximum_heights: DeviceBuffer::from_slice(
                &obstacles.cell_maximum_heights,
            )?,
            obstacle_grid_count: obstacles.grids.len() as u32,
        })
    }
}

/// One line layer uploaded once for every tile owned by its region.
pub struct RegionDeviceLineSources {
    sources: DeviceBuffer<DeviceLineSource>,
    source_count: u32,
}

impl RegionDeviceLineSources {
    pub fn upload(sources: &[DeviceLineSource]) -> Result<Self> {
        Ok(Self {
            sources: DeviceBuffer::from_slice(sources)?,
            source_count: sources.len() as u32,
        })
    }
}

/// Terrain halo uploaded once for every tile that shares a [`TileBatch`](raster_reader::fused_tile_z13::TileBatch).
pub struct BatchDeviceRaster {
    pixels: DeviceBuffer<raster_reader::FusedPixel>,
    geometry: DeviceRasterGeometry,
}

impl BatchDeviceRaster {
    pub fn upload(frame: &RegionMetricFrame, tile: &FusedTileZ13) -> Result<Self> {
        Ok(Self {
            pixels: DeviceBuffer::from_slice(tile.halo.pixels())?,
            geometry: DeviceRasterGeometry::for_grid(frame, &tile.halo),
        })
    }
}

/// Measured work and CUDA event time for one tile/layer pair.
#[derive(Clone, Debug, Default)]
pub struct TilePaintMeasurement {
    pub corner_pairs: u64,
    pub pixel_pairs: u64,
    pub relevant_source_references: u64,
    /// Relevant sources of every block, in block order.
    pub block_source_counts: Vec<u32>,
    pub corner_gpu_milliseconds: f64,
    pub paint_gpu_milliseconds: f64,
}

/// Tile coordinates and receiver fields uploaded once, independent of the line layer.
pub struct TileDeviceReceivers {
    lattice: TileMetricLattice,
    /// Half a pixel of this tile in metres at its centre latitude.
    pixel_floor_m: f32,
    receiver_x_m: DeviceBuffer<f32>,
    receiver_y_m: DeviceBuffer<f32>,
    receiver_altitude_m: DeviceBuffer<f32>,
    receiver_reflection_db: DeviceBuffer<f32>,
    corner_x_m: DeviceBuffer<f32>,
    corner_y_m: DeviceBuffer<f32>,
    corner_reflection_db: DeviceBuffer<f32>,
}

impl TileDeviceReceivers {
    pub fn upload(
        frame: &RegionMetricFrame,
        tile: &FusedTileZ13,
        obstacles: &ObstacleSet,
    ) -> Result<Self> {
        let lattice = TileMetricLattice::for_tile(frame, tile.zoom, tile.tile_x, tile.tile_y);
        let centre_latitude = (tile.bbox.north_lat + tile.bbox.south_lat) * 0.5;
        let pixel_floor_m = (tile_pixel_size_m(tile.zoom, centre_latitude) * 0.5) as f32;
        let mut corner_x_m = Vec::with_capacity(CORNER_COUNT);
        let mut corner_y_m = Vec::with_capacity(CORNER_COUNT);
        let mut corner_reflection_db = Vec::with_capacity(CORNER_COUNT);
        for corner in 0..CORNER_COUNT {
            let [x_m, y_m] = lattice.corner_xy(corner);
            let [latitude, longitude] = frame.decode(x_m, y_m);
            corner_x_m.push(x_m);
            corner_y_m.push(y_m);
            corner_reflection_db.push(enclosure_db(
                obstacles,
                latitude,
                longitude,
                ENCLOSURE_RADIUS_M,
            ) as f32);
        }
        Ok(Self {
            receiver_x_m: DeviceBuffer::from_slice(&lattice.pixel_x_centres_m)?,
            receiver_y_m: DeviceBuffer::from_slice(&lattice.pixel_y_centres_m)?,
            receiver_altitude_m: DeviceBuffer::from_slice(&tile.rx_alt_m)?,
            receiver_reflection_db: DeviceBuffer::from_slice(&tile.rx_refl_db)?,
            corner_x_m: DeviceBuffer::from_slice(&corner_x_m)?,
            corner_y_m: DeviceBuffer::from_slice(&corner_y_m)?,
            corner_reflection_db: DeviceBuffer::from_slice(&corner_reflection_db)?,
            lattice,
            pixel_floor_m,
        })
    }
}

#[allow(clippy::too_many_arguments)]
/// Partition and paint one tile/layer; the per-period pixel energies come back
/// for the writer thread together with the measurement.
pub fn partition_and_paint_tile(
    cuda: &RelevantSourceCuda,
    sources: &[DeviceLineSource],
    device_sources: &RegionDeviceLineSources,
    device_obstacles: &RegionDeviceObstacles,
    batch_raster: &BatchDeviceRaster,
    receivers: &TileDeviceReceivers,
    lden_weights: [f64; PERIOD_COUNT],
) -> Result<(TilePaintMeasurement, Vec<f32>)> {
    let incidence = build_tile_source_incidence(sources, &receivers.lattice);
    let corner_offsets = DeviceBuffer::from_slice(&incidence.corner_offsets)?;
    let corner_sources = DeviceBuffer::from_slice(&incidence.corner_source_indices)?;
    let scene = DeviceScenePointers {
        sources: device_sources.sources.as_ptr(),
        raster_pixels: batch_raster.pixels.as_ptr(),
        obstacle_grids: device_obstacles.obstacle_grids.as_ptr(),
        obstacle_cell_starts: device_obstacles.obstacle_cell_starts.as_ptr(),
        obstacle_edge_references: device_obstacles.obstacle_edge_references.as_ptr(),
        obstacle_edge_values_xyxyh: device_obstacles.obstacle_edge_values.as_ptr(),
        obstacle_cell_maximum_heights: device_obstacles.obstacle_cell_maximum_heights.as_ptr(),
        obstacle_edge_is_building: device_obstacles.obstacle_edge_is_building.as_ptr(),
        source_count: device_sources.source_count,
        obstacle_grid_count: device_obstacles.obstacle_grid_count,
        pixel_floor_m: receivers.pixel_floor_m,
        raster_geometry: batch_raster.geometry,
    };
    let (corner_energy, corner_gpu_milliseconds) = cuda.evaluate_corners(
        &scene,
        &corner_offsets,
        &corner_sources,
        &receivers.corner_x_m,
        &receivers.corner_y_m,
        &receivers.corner_reflection_db,
    )?;
    let partition = build_relevant_source_partition(&incidence, &corner_energy, lden_weights)?;
    let block_offsets = DeviceBuffer::from_slice(&partition.block_offsets)?;
    let relevant_sources = DeviceBuffer::from_slice(&partition.relevant_source_indices)?;
    let background_flat: Vec<f32> = partition
        .background_corner_energy
        .iter()
        .flat_map(|corners| corners.iter().flatten().copied())
        .collect();
    let background_energy = DeviceBuffer::from_slice(&background_flat)?;
    let (energy, paint_gpu_milliseconds) = cuda.paint_tile(
        &scene,
        &block_offsets,
        &relevant_sources,
        &background_energy,
        &receivers.receiver_x_m,
        &receivers.receiver_y_m,
        &receivers.receiver_altitude_m,
        &receivers.receiver_reflection_db,
    )?;
    if cuda.take_profile_overflow()? {
        bail!(
            "the card dropped a path-profile chainage: a ray outran the profile cadence \
             (kernels/relevant_source_path.cuh) and this tile's bytes would be wrong"
        );
    }
    let relevant_source_references = partition.relevant_source_indices.len() as u64;
    let block_source_counts = partition
        .block_offsets
        .windows(2)
        .map(|window| window[1] - window[0])
        .collect();
    Ok((
        TilePaintMeasurement {
            corner_pairs: incidence.corner_source_indices.len() as u64,
            pixel_pairs: relevant_source_references * (BLOCK_PIXEL_COUNT as u64),
            relevant_source_references,
            block_source_counts,
            corner_gpu_milliseconds: f64::from(corner_gpu_milliseconds),
            paint_gpu_milliseconds: f64::from(paint_gpu_milliseconds),
        },
        energy,
    ))
}

const BLOCK_PIXEL_COUNT: usize = TILE_PIXEL_SIDE * TILE_PIXEL_SIDE / BLOCK_COUNT;
const _: () = assert!(TILE_PIXEL_SIDE.is_multiple_of(crate::source_frame::BLOCK_PIXEL_SIDE));
const _: () = assert!(PERIOD_COUNT == 3);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::obstacle_transfer::FlattenedObstacleGeometry;
    use crate::surface_layers::LAYER_LDEN_WEIGHTS;
    use raster_reader::fused_tile_z13::TileBatch;
    use raster_reader::RealRasters;
    use std::path::Path;

    /// The card's half of the silent tile. `PendingTileWrite::Silent` writes the
    /// all-NO_DATA tile on the claim that this is what the paint returns when no
    /// source of the layer reaches it; this asks the card. With no sources the
    /// incidence is empty, the partition admits nothing and leaves every block's
    /// background at zero, so both kernels must leave every pixel of every period
    /// at zero — over a real halo and a real receiver lattice, which the paint
    /// reads only through the sources it does not have.
    ///
    /// It opens a CUDA context, so it is not in the default set: the nvcc role
    /// gate must stay runnable on a host that has the toolkit and no card. Run it
    /// where a card is — the release build host — with
    /// `cargo test --manifest-path engine/relevant-source-gpu/Cargo.toml
    /// --features gpu -- --ignored`.
    #[test]
    #[ignore = "opens a CUDA context; run on a box with a card"]
    fn a_tile_no_source_reaches_comes_back_from_the_card_at_zero() {
        let cuda = RelevantSourceCuda::initialize().expect("a CUDA device");
        let rasters = RealRasters::new(Path::new("/nonexistent-prepared-root"));
        let frame = RegionMetricFrame::for_latitude_longitude(49.78, 14.17);
        let batch = TileBatch::build_opt_rx_refl(13, 4412, 2784, 1, 30.0, &rasters);
        let obstacles = ObstacleSet::empty();
        let device_obstacles =
            RegionDeviceObstacles::upload(&FlattenedObstacleGeometry::from_set(&frame, &obstacles))
                .expect("an empty obstacle set uploads");
        let device_sources = RegionDeviceLineSources::upload(&[]).expect("no sources upload");
        let batch_raster =
            BatchDeviceRaster::upload(&frame, &batch.tiles[0]).expect("the halo uploads");
        let receivers = TileDeviceReceivers::upload(&frame, &batch.tiles[0], &obstacles)
            .expect("the receiver lattice uploads");
        let (measurement, energy) = partition_and_paint_tile(
            &cuda,
            &[],
            &device_sources,
            &device_obstacles,
            &batch_raster,
            &receivers,
            LAYER_LDEN_WEIGHTS[0],
        )
        .expect("the card paints the tile");
        assert_eq!(measurement.corner_pairs, 0);
        assert_eq!(measurement.relevant_source_references, 0);
        assert_eq!(
            energy.len(),
            TILE_PIXEL_SIDE * TILE_PIXEL_SIDE * PERIOD_COUNT
        );
        assert!(
            energy.iter().all(|&value| value == 0.0),
            "the card returned energy for a tile no source reaches"
        );
    }
}
