//! Device construction of height-conditioned building-horizon pruning inputs: the lowest
//! source tangent per receiver block and azimuth group, a DEM max pyramid over the tile's
//! terrain sampler, and a local roof-top proxy for every obstacle grid cell.

use std::sync::Arc;

use anyhow::{Context, Result};
use cudarc::driver::{CudaDevice, CudaFunction, CudaSlice, DevicePtr, LaunchAsync, LaunchConfig};
use noise_compute::constants::M_PER_DEG_LAT;
use noise_compute::emission::aircraft::{
    BUILDING_LOCAL_MAX_M, LOWEST_SOURCE_TANGENT_SECTOR_GROUPS,
};
use raster_reader::fused_tile_z13::{FusedTileZ13, TILE_PX};
use tile_painter::airborne_screening::{
    coarse_receiver_axis, PackedReceiverScreening, LOWEST_SOURCE_TANGENT_BLOCK_PX,
};

use crate::airborne_building_horizon::AirborneScreeningEnvironment;

/// Slack, in metres, beyond the 512 m roof reach when choosing the obstacle cells a tile's
/// receivers can touch: the kernel's reach is scaled by the receiver row's metres per degree,
/// which differs from the index's by far less than one per cent inside one tile.
const CELL_TOP_REACH_SLACK_M: f64 = 8.0;

const BLOCKS_PER_TILE: usize = (TILE_PX / LOWEST_SOURCE_TANGENT_BLOCK_PX).pow(2);

/// Per-tile scratch the bounds are built into (sized once per tile block from the environment).
pub(crate) struct ScreeningBoundsScratch {
    pyramid: CudaSlice<f32>,
    layout: CudaSlice<u32>,
    /// `(offset, rows, cols)` per pyramid level, the device `layout`'s host copy.
    layout_host: Vec<u32>,
    group_floor: CudaSlice<f32>,
    cell_top: CudaSlice<f32>,
}

/// One tile's bounds: a pointer table (`BOUNDS_*` slots) of the per-record floor, the per
/// (block, group) floors, the scratch cell tops and, per member index, the
/// `[cx0, cx1, cy0, cy1]` cell rectangle those tops cover.
pub(crate) struct ScreeningBoundsDev {
    pub(crate) table: CudaSlice<u64>,
    _floor_of_record: CudaSlice<f32>,
    _cell_top_rect: CudaSlice<i64>,
}

pub(crate) struct AirborneScreeningBoundsGpu {
    dev: Arc<CudaDevice>,
    pyramid_level0: CudaFunction,
    pyramid_reduce: CudaFunction,
    lowest_source_tangent: CudaFunction,
    floor: CudaFunction,
    building_cell_tops: CudaFunction,
    lattice_axis: CudaSlice<u8>,
}

impl AirborneScreeningBoundsGpu {
    pub(crate) fn new(dev: Arc<CudaDevice>) -> Self {
        let function = |name| {
            dev.get_func("air", name)
                .unwrap_or_else(|| panic!("fn {name}"))
        };
        let lattice_axis = dev
            .htod_copy(coarse_receiver_axis().map(u8::from).to_vec())
            .expect("upload coarse lattice axis");
        Self {
            pyramid_level0: function("airborne_dem_pyramid_level0"),
            pyramid_reduce: function("airborne_dem_pyramid_reduce"),
            lowest_source_tangent: function("airborne_lowest_source_tangent"),
            floor: function("airborne_screening_floor"),
            building_cell_tops: function("airborne_building_cell_tops"),
            lattice_axis,
            dev,
        }
    }

    pub(crate) fn scratch(
        &self,
        environment: &AirborneScreeningEnvironment,
    ) -> Result<ScreeningBoundsScratch> {
        let mut layout = Vec::new();
        let (mut rows, mut cols, mut offset) = (environment.dem_rows, environment.dem_cols, 0);
        loop {
            layout.extend_from_slice(&[offset as u32, rows as u32, cols as u32]);
            offset += rows * cols;
            if rows == 1 && cols == 1 {
                break;
            }
            rows = rows.div_ceil(2);
            cols = cols.div_ceil(2);
        }
        Ok(ScreeningBoundsScratch {
            pyramid: self
                .dev
                .alloc_zeros::<f32>(offset)
                .context("dem max pyramid")?,
            layout: self
                .dev
                .htod_copy(layout.clone())
                .context("dem max pyramid layout")?,
            layout_host: layout,
            group_floor: self
                .dev
                .alloc_zeros::<f32>(BLOCKS_PER_TILE * LOWEST_SOURCE_TANGENT_SECTOR_GROUPS)
                .context("block direction floors")?,
            cell_top: self
                .dev
                .alloc_zeros::<f32>(environment.cell_slots)
                .context("building cell tops")?,
        })
    }

    /// Build the bounds for one tile. `near` is this tile's slice of the device near CSR.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build(
        &self,
        environment: &AirborneScreeningEnvironment,
        scratch: &mut ScreeningBoundsScratch,
        tile: &FusedTileZ13,
        packed: &PackedReceiverScreening,
        pixel_of_record: &CudaSlice<u32>,
        receiver_lat_lon: &CudaSlice<f64>,
        receiver_altitude: &CudaSlice<f32>,
        inner_elevation: &CudaSlice<f32>,
        tile_bbox: &CudaSlice<f64>,
        segment_lat_lon: &CudaSlice<f64>,
        segment_f32: &CudaSlice<f32>,
        near_idx: &CudaSlice<i32>,
        nreg: usize,
        near: (usize, usize),
    ) -> Result<ScreeningBoundsDev> {
        let records = packed.records;
        let mut floor_of_record = self
            .dev
            .alloc_zeros::<f32>(records)
            .context("record floors")?;
        let layout = &scratch.layout_host;
        let levels = layout.len() / 3;
        let level_count = levels as i32;
        unsafe {
            self.pyramid_level0
                .clone()
                .launch(
                    launch_config(environment.dem_rows * environment.dem_cols),
                    (
                        &environment.table,
                        inner_elevation,
                        tile_bbox,
                        &mut scratch.pyramid,
                    ),
                )
                .context("launch dem pyramid level 0")?;
            for level in 1..levels {
                let source = &layout[3 * (level - 1)..3 * level];
                let target = &layout[3 * level..3 * level + 3];
                self.pyramid_reduce
                    .clone()
                    .launch(
                        launch_config((target[1] * target[2]) as usize),
                        (
                            &mut scratch.pyramid,
                            source[0],
                            source[1],
                            source[2],
                            target[0],
                            target[1],
                            target[2],
                        ),
                    )
                    .context("launch dem pyramid level")?;
            }
            if near.1 > 0 {
                let block =
                    (LOWEST_SOURCE_TANGENT_BLOCK_PX * LOWEST_SOURCE_TANGENT_BLOCK_PX) as u32;
                self.lowest_source_tangent
                    .clone()
                    .launch(
                        LaunchConfig {
                            grid_dim: (BLOCKS_PER_TILE as u32, 1, 1),
                            block_dim: (block, 1, 1),
                            shared_mem_bytes: 0,
                        },
                        (
                            receiver_lat_lon,
                            receiver_altitude,
                            segment_lat_lon,
                            segment_f32,
                            near_idx,
                            near.0 as i32,
                            near.1 as i32,
                            nreg as i32,
                            &mut scratch.group_floor,
                        ),
                    )
                    .context("launch lowest source tangent")?;
            }
            self.floor
                .clone()
                .launch(
                    launch_config(records),
                    (
                        pixel_of_record,
                        &self.lattice_axis,
                        receiver_lat_lon,
                        &scratch.group_floor,
                        records as i32,
                        near.1 as i32,
                        &mut floor_of_record,
                    ),
                )
                .context("launch screening floor")?;
        }

        // Obstacle cells within roof reach of the tile, per member index: the cell rectangle
        // the horizon builder's own per-receiver window can visit from any tile pixel.
        let bbox = &tile.bbox;
        let mut rect = Vec::with_capacity(4 * environment.index_geometry.len());
        for (index, geometry) in environment.index_geometry.iter().enumerate() {
            let reach_m = BUILDING_LOCAL_MAX_M + CELL_TOP_REACH_SLACK_M + geometry.cell_m;
            let x_lo = (bbox.west_lon - geometry.origin_lon) * geometry.m_per_deg_lon - reach_m;
            let x_hi = (bbox.east_lon - geometry.origin_lon) * geometry.m_per_deg_lon + reach_m;
            let y_lo = (bbox.south_lat - geometry.origin_lat) * M_PER_DEG_LAT - reach_m;
            let y_hi = (bbox.north_lat - geometry.origin_lat) * M_PER_DEG_LAT + reach_m;
            let cell = |value: f64, min: f64, count: usize| {
                (((value - min) / geometry.cell_m).floor() as i64).clamp(0, count as i64 - 1)
            };
            let outside = x_hi < geometry.min_x
                || y_hi < geometry.min_y
                || x_lo > geometry.min_x + geometry.cols as f64 * geometry.cell_m
                || y_lo > geometry.min_y + geometry.rows as f64 * geometry.cell_m;
            if outside {
                rect.extend_from_slice(&[1, 0, 1, 0]);
                continue;
            }
            let (cx0, cx1) = (
                cell(x_lo, geometry.min_x, geometry.cols),
                cell(x_hi, geometry.min_x, geometry.cols),
            );
            let (cy0, cy1) = (
                cell(y_lo, geometry.min_y, geometry.rows),
                cell(y_hi, geometry.min_y, geometry.rows),
            );
            rect.extend_from_slice(&[cx0, cx1, cy0, cy1]);
            let (cx_count, cy_count) = (cx1 - cx0 + 1, cy1 - cy0 + 1);
            unsafe {
                self.building_cell_tops
                    .clone()
                    .launch(
                        launch_config((cx_count * cy_count) as usize),
                        (
                            &environment.table,
                            &scratch.pyramid,
                            &scratch.layout,
                            level_count,
                            index as i32,
                            cx0,
                            cy0,
                            cx_count,
                            cy_count,
                            &mut scratch.cell_top,
                        ),
                    )
                    .context("launch building cell tops")?;
            }
        }
        let cell_top_rect = self.dev.htod_copy(rect).context("cell top rectangles")?;
        let table = self
            .dev
            .htod_copy(vec![
                *floor_of_record.device_ptr(),
                *scratch.group_floor.device_ptr(),
                *scratch.cell_top.device_ptr(),
                *cell_top_rect.device_ptr(),
            ])
            .context("bounds table")?;
        Ok(ScreeningBoundsDev {
            table,
            _floor_of_record: floor_of_record,
            _cell_top_rect: cell_top_rect,
        })
    }
}

fn launch_config(items: usize) -> LaunchConfig {
    let block = 256_u32;
    LaunchConfig {
        grid_dim: ((items.max(1) as u32).div_ceil(block), 1, 1),
        block_dim: (block, 1, 1),
        shared_mem_bytes: 0,
    }
}
