//! Device construction of the C2 receiver terrain horizon.

use std::sync::Arc;

use anyhow::{Context, Result};
use cudarc::driver::{CudaDevice, CudaFunction, CudaSlice, LaunchAsync, LaunchConfig};
use noise_compute::emission::aircraft::{
    HORIZON_SECTORS, RECEIVER_HORIZON_ENTRY_COUNT, RECEIVER_HORIZON_MARCH_SAMPLES,
};
use raster_reader::fused_tile_z13::TILE_PX;
use tile_painter::airborne_screening::PackedReceiverScreening;

use crate::airborne_building_horizon::AirborneScreeningEnvironment;

pub(crate) struct TerrainHorizonDev {
    pub(crate) entries: CudaSlice<u32>,
    pub(crate) max_sin_sq: CudaSlice<f32>,
}

pub(crate) struct AirborneTerrainHorizonGpu {
    dev: Arc<CudaDevice>,
    sample_tables: CudaFunction,
    build: CudaFunction,
    global_max: CudaFunction,
    range_quantization_probe: CudaFunction,
}

impl AirborneTerrainHorizonGpu {
    pub(crate) fn new(dev: Arc<CudaDevice>) -> Self {
        let function = |name| {
            dev.get_func("air", name)
                .unwrap_or_else(|| panic!("fn {name}"))
        };
        Self {
            sample_tables: function("airborne_terrain_sample_tables"),
            build: function("airborne_terrain_horizon_build"),
            global_max: function("airborne_terrain_horizon_global_max"),
            range_quantization_probe: function("airborne_terrain_horizon_range_quantization_probe"),
            dev,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build(
        &self,
        environment: &AirborneScreeningEnvironment,
        packed: &PackedReceiverScreening,
        pixel_of_record: &CudaSlice<u32>,
        receiver_lat_lon: &CudaSlice<f64>,
        receiver_altitude: &CudaSlice<f32>,
        inner_elevation: &CudaSlice<f32>,
        tile_bbox: &CudaSlice<f64>,
    ) -> Result<TerrainHorizonDev> {
        let records = packed.records;
        let mut entries = self
            .dev
            .alloc_zeros::<u32>(records * RECEIVER_HORIZON_ENTRY_COUNT)
            .context("terrain horizon entries")?;
        let mut max_sin_sq = self
            .dev
            .alloc_zeros::<f32>(records)
            .context("terrain horizon maxima")?;
        let samples_per_receiver = HORIZON_SECTORS * RECEIVER_HORIZON_MARCH_SAMPLES;
        let mut east_deg = self
            .dev
            .alloc_zeros::<f64>(TILE_PX * samples_per_receiver)
            .context("terrain sample longitude offsets")?;
        let mut row_rf = self
            .dev
            .alloc_zeros::<f64>(TILE_PX * samples_per_receiver)
            .context("terrain sample lattice rows")?;
        let mut row_idx = self
            .dev
            .alloc_zeros::<i32>(TILE_PX * samples_per_receiver)
            .context("terrain sample inner rows")?;
        unsafe {
            self.sample_tables
                .clone()
                .launch(
                    launch_config(TILE_PX * samples_per_receiver),
                    (
                        &environment.table,
                        receiver_lat_lon,
                        tile_bbox,
                        &mut east_deg,
                        &mut row_rf,
                        &mut row_idx,
                    ),
                )
                .context("launch terrain sample tables")?;
            self.build
                .clone()
                .launch(
                    launch_config(records * HORIZON_SECTORS),
                    (
                        &environment.table,
                        receiver_lat_lon,
                        receiver_altitude,
                        inner_elevation,
                        tile_bbox,
                        pixel_of_record,
                        &east_deg,
                        &row_rf,
                        &row_idx,
                        records as i32,
                        &mut entries,
                    ),
                )
                .context("launch terrain horizon build")?;
            self.global_max
                .clone()
                .launch(
                    launch_config(records),
                    (records as i32, &entries, &mut max_sin_sq),
                )
                .context("launch terrain horizon global max")?;
        }
        self.dev.synchronize().context("terrain horizon sync")?;
        Ok(TerrainHorizonDev {
            entries,
            max_sin_sq,
        })
    }

    pub(crate) fn range_quantization_probe(
        &self,
        true_range_m: f32,
        source_range_m: f32,
    ) -> Result<f32> {
        let mut dz = self
            .dev
            .alloc_zeros::<f32>(1)
            .context("terrain range probe output")?;
        unsafe {
            self.range_quantization_probe
                .clone()
                .launch(launch_config(1), (true_range_m, source_range_m, &mut dz))
                .context("launch terrain range probe")?;
        }
        self.dev.synchronize().context("terrain range probe sync")?;
        Ok(self
            .dev
            .dtoh_sync_copy(&dz)
            .context("read terrain range probe")?[0])
    }
}

fn launch_config(items: usize) -> LaunchConfig {
    let block = 256_u32;
    LaunchConfig {
        grid_dim: ((items as u32).div_ceil(block), 1, 1),
        block_dim: (block, 1, 1),
        shared_mem_bytes: 0,
    }
}

const _: () = assert!(RECEIVER_HORIZON_MARCH_SAMPLES == 48);
