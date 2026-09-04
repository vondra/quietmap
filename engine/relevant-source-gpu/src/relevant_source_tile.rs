//! Tile uploads, shared-corner partition construction, exact pixel paint, and the
//! HM3 collapse-and-write handed to the region's writer thread.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use noise_compute::constants::ENCLOSURE_RADIUS_M;
use noise_compute::propagation::obstacle_index::{enclosure_db, ObstacleSet};
use raster_reader::fused_tile_z13::{tile_pixel_size_m, FusedTileZ13};
use tile_painter::accumulator::TileAccumulator;
use tile_painter::source_loader_structure::InteriorEstimate;
use tile_painter::wire_hm3::{
    collapse_lden_surface_u8, collapse_lden_u8, fill_area_median, write_tile, AREA_FILL_RADIUS_PX,
};

use crate::cuda_bridge::{DeviceBuffer, DeviceScenePointers, RelevantSourceCuda};
use crate::obstacle_transfer::{
    DeviceObstacleGrid, DeviceRasterGeometry, FlattenedObstacleGeometry,
};
use crate::relevance_partition::build_relevant_source_partition;
use crate::source_frame::{
    source_identity_fingerprint, DeviceLineSource, RegionMetricFrame, BLOCK_COUNT, CORNER_COUNT,
    PERIOD_COUNT, TILE_PIXEL_SIDE,
};
use crate::tile_source_incidence::{build_tile_source_incidence, TileMetricLattice};

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

/// One painted tile's period energies waiting for the writer thread: collapse to
/// the Lden byte, smooth an area source's point-grid ripple into its footprint,
/// fill enclosed pixels from their facade donors, brotli, write (the CPU
/// surface_region order).
pub struct PendingTileWrite {
    pub energy: Vec<f32>,
    pub interior: Arc<InteriorEstimate>,
    /// Index into the runner's layer list, for the per-layer byte total.
    pub layer: usize,
    /// Industrial and building discretise areas into point grids that the
    /// median fill turns into solid footprints; lines are continuous already.
    pub area_source: bool,
    /// Airport ground ops accumulate EVENT energy over this many days; the
    /// surface layers (None) are steady power.
    pub event_days: Option<f64>,
    pub source_id: u8,
    pub output_path: PathBuf,
}

impl PendingTileWrite {
    /// Bytes written.
    pub fn write(self) -> Result<u64> {
        let accumulator = TileAccumulator {
            energy: self.energy,
        };
        let mut cells = match self.event_days {
            Some(n_days) => collapse_lden_u8(&accumulator, n_days),
            None => collapse_lden_surface_u8(&accumulator),
        };
        if self.area_source {
            fill_area_median(&mut cells, AREA_FILL_RADIUS_PX);
        }
        self.interior.apply(&mut cells);
        Ok(write_tile(&self.output_path, &cells, self.source_id, false)? as u64)
    }
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
    source_fingerprint: u64,
    device_sources: &RegionDeviceLineSources,
    device_obstacles: &RegionDeviceObstacles,
    batch_raster: &BatchDeviceRaster,
    receivers: &TileDeviceReceivers,
    lden_weights: [f64; PERIOD_COUNT],
    partition_path: &Path,
) -> Result<(TilePaintMeasurement, Vec<f32>)> {
    debug_assert_eq!(source_fingerprint, source_identity_fingerprint(sources));
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
    let partition = build_relevant_source_partition(
        &incidence,
        &corner_energy,
        lden_weights,
        source_fingerprint,
    )?;
    partition
        .write_to(partition_path)
        .with_context(|| format!("persist {}", partition_path.display()))?;

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
