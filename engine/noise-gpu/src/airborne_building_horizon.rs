//! Device construction of receiver-local vector-building horizons.

use std::sync::Arc;

use anyhow::{Context, Result};
use cudarc::driver::{CudaDevice, CudaFunction, CudaSlice, DevicePtr, LaunchAsync, LaunchConfig};
use noise_compute::emission::aircraft::{
    building_local_directions, receiver_horizon_samples, BUILDING_LOCAL_HORIZON_BANDS,
    BUILDING_LOCAL_HORIZON_ENTRY_COUNT, BUILDING_LOCAL_HORIZON_SECTORS,
    M_PER_DEG_LAT as AIRCRAFT_M_PER_DEG_LAT,
};
use noise_compute::propagation::obstacle_index::ObstacleSet;
use raster_reader::fused_grid::FusedGrid;
use tile_painter::airborne_screening::PackedReceiverScreening;

use crate::airborne_screening_bounds::ScreeningBoundsDev;

const BUILDING_ENV_INDEX_COUNT: usize = 0;
const BUILDING_ENV_GRID_GEOMETRY: usize = 1;
const BUILDING_ENV_GRID_LAYOUT: usize = 2;
const BUILDING_ENV_CELL_STARTS: usize = 3;
const BUILDING_ENV_EDGE_REFS: usize = 4;
const BUILDING_ENV_EDGES: usize = 5;
const BUILDING_ENV_EDGE_IS_BUILDING: usize = 6;
const BUILDING_ENV_DEM_META: usize = 7;
const BUILDING_ENV_DEM_ELEVATION: usize = 8;
const BUILDING_ENV_DEM_COLS: usize = 9;
const BUILDING_ENV_DEM_ROWS: usize = 10;
const BUILDING_ENV_DIRECTIONS: usize = 11;
const BUILDING_ENV_TERRAIN_SAMPLES: usize = 12;
const BUILDING_ENV_CELL_MAX_H: usize = 13;
const BUILDING_ENV_WORDS: usize = 14;
const BUILDING_GRID_GEOMETRY_STRIDE: usize = 6;
const BUILDING_GRID_LAYOUT_STRIDE: usize = 5;

/// One member index's grid placement, the host half of `BUILDING_ENV_GRID_GEOMETRY`: the
/// screening bounds need it to name the cells whose roof tops a tile can reach.
pub(crate) struct IndexGeometry {
    pub(crate) origin_lat: f64,
    pub(crate) origin_lon: f64,
    pub(crate) m_per_deg_lon: f64,
    pub(crate) cell_m: f64,
    pub(crate) min_x: f64,
    pub(crate) min_y: f64,
    pub(crate) cols: usize,
    pub(crate) rows: usize,
}

/// Region-wide vector CSR and elevation halo resident for every tile block.
pub struct AirborneScreeningEnvironment {
    pub(crate) table: CudaSlice<u64>,
    index_count: usize,
    building_edge_count: usize,
    pub(crate) dem_rows: usize,
    pub(crate) dem_cols: usize,
    /// Total CSR cell slots over every member index — the `cell_top` scratch length.
    pub(crate) cell_slots: usize,
    pub(crate) index_geometry: Vec<IndexGeometry>,
    _grid_geometry: CudaSlice<f64>,
    _grid_layout: CudaSlice<u64>,
    _cell_starts: CudaSlice<u32>,
    _edge_refs: CudaSlice<u32>,
    _edges: CudaSlice<f32>,
    _edge_is_building: CudaSlice<u8>,
    _dem_meta: CudaSlice<f64>,
    _dem_elevation: CudaSlice<f32>,
    _directions: CudaSlice<f64>,
    _terrain_samples: CudaSlice<f64>,
    _cell_max_h: CudaSlice<f32>,
}

pub(crate) struct BuildingHorizonDev {
    pub(crate) global_max_tangent_bits: CudaSlice<u16>,
    pub(crate) local_entries: CudaSlice<u32>,
    pub(crate) local_max_tangent_bits: CudaSlice<u16>,
}

pub(crate) struct AirborneBuildingHorizonGpu {
    dev: Arc<CudaDevice>,
    build: CudaFunction,
    pack: CudaFunction,
    global_max: CudaFunction,
    mark_empty: CudaFunction,
}

impl AirborneBuildingHorizonGpu {
    pub(crate) fn new(dev: Arc<CudaDevice>) -> Self {
        let function = |name| {
            dev.get_func("air", name)
                .unwrap_or_else(|| panic!("fn {name}"))
        };
        Self {
            build: function("airborne_building_horizon_build"),
            pack: function("airborne_building_horizon_pack"),
            global_max: function("airborne_building_horizon_global_max"),
            mark_empty: function("airborne_building_horizon_mark_empty"),
            dev,
        }
    }

    pub(crate) fn upload_environment(
        &self,
        obstacles: &ObstacleSet,
        halo: &FusedGrid,
    ) -> Result<AirborneScreeningEnvironment> {
        let mut grid_geometry =
            Vec::with_capacity(obstacles.indexes.len() * BUILDING_GRID_GEOMETRY_STRIDE);
        let mut grid_layout =
            Vec::with_capacity(obstacles.indexes.len() * BUILDING_GRID_LAYOUT_STRIDE);
        let mut cell_starts = Vec::new();
        let mut edge_refs = Vec::new();
        let mut edges = Vec::new();
        let mut edge_is_building = Vec::new();
        let mut cell_max_h = Vec::new();
        let mut index_geometry = Vec::with_capacity(obstacles.indexes.len());
        for index in &obstacles.indexes {
            let view = index.gpu_view();
            index_geometry.push(IndexGeometry {
                origin_lat: view.origin_lat,
                origin_lon: view.origin_lon,
                m_per_deg_lon: view.m_per_deg_lon,
                cell_m: view.cell_m,
                min_x: view.min_x,
                min_y: view.min_y,
                cols: view.cols,
                rows: view.rows,
            });
            grid_geometry.extend_from_slice(&[
                view.origin_lat,
                view.origin_lon,
                view.m_per_deg_lon,
                view.cell_m,
                view.min_x,
                view.min_y,
            ]);
            grid_layout.extend_from_slice(&[
                view.cols as u64,
                view.rows as u64,
                cell_starts.len() as u64,
                edge_refs.len() as u64,
                (edges.len() / 5) as u64,
            ]);
            cell_starts.extend_from_slice(view.cell_starts);
            edge_refs.extend_from_slice(view.edge_refs);
            edges.extend_from_slice(&view.edges_xyxyh);
            edge_is_building.extend_from_slice(&view.edge_is_building);
            cell_max_h.extend_from_slice(view.cell_max_h);
            // `grid_layout` addresses cells through the corresponding `cell_starts` base.
            // Keep the parallel height array on that same (cells + 1) stride; the terminal
            // value is never read, but without it every later member index would be shifted.
            cell_max_h.push(0.0);
        }
        let building_edge_count = edge_is_building.iter().filter(|&&value| value == 1).count();
        ensure_nonempty(&mut grid_geometry);
        ensure_nonempty(&mut grid_layout);
        ensure_nonempty(&mut cell_starts);
        ensure_nonempty(&mut edge_refs);
        ensure_nonempty(&mut edges);
        ensure_nonempty(&mut edge_is_building);
        ensure_nonempty(&mut cell_max_h);
        assert_eq!(cell_starts.len(), cell_max_h.len());
        let cell_slots = cell_starts.len();

        let elevation = halo.packed_elevation_grid();
        let directions: Vec<f64> = building_local_directions()
            .iter()
            .flat_map(|&(sin_angle, cos_angle)| [sin_angle, cos_angle])
            .collect();
        // The march kernel adds each sample's latitude offset in degrees; dividing by the
        // Doc 29 metres per degree here is the same IEEE f64 quotient the CPU march forms
        // per sample, so the device saves that division without changing a bit.
        let terrain_samples: Vec<f64> = receiver_horizon_samples()
            .iter()
            .flatten()
            .flat_map(|sample| {
                [
                    sample.range_m,
                    sample.north_m / AIRCRAFT_M_PER_DEG_LAT,
                    sample.east_m,
                    sample.band as f64,
                ]
            })
            .collect();
        let d_grid_geometry = self
            .dev
            .htod_copy(grid_geometry)
            .context("building grid geometry")?;
        let d_grid_layout = self
            .dev
            .htod_copy(grid_layout)
            .context("building grid layout")?;
        let d_cell_starts = self
            .dev
            .htod_copy(cell_starts)
            .context("building cell starts")?;
        let d_edge_refs = self
            .dev
            .htod_copy(edge_refs)
            .context("building edge refs")?;
        let d_edges = self.dev.htod_copy(edges).context("building edges")?;
        let d_edge_is_building = self
            .dev
            .htod_copy(edge_is_building)
            .context("building edge kinds")?;
        let d_dem_meta = self
            .dev
            .htod_copy(vec![
                elevation.lat_min,
                elevation.lon_min,
                elevation.inv_cell_deg,
            ])
            .context("building DEM metadata")?;
        let d_dem_elevation = self
            .dev
            .htod_copy(elevation.elevation_m)
            .context("building DEM elevation")?;
        let d_directions = self
            .dev
            .htod_copy(directions)
            .context("building sector directions")?;
        let d_terrain_samples = self
            .dev
            .htod_copy(terrain_samples)
            .context("terrain horizon samples")?;
        let d_cell_max_h = self
            .dev
            .htod_copy(cell_max_h)
            .context("building cell max heights")?;

        let mut host_table = [0_u64; BUILDING_ENV_WORDS];
        host_table[BUILDING_ENV_INDEX_COUNT] = obstacles.indexes.len() as u64;
        host_table[BUILDING_ENV_GRID_GEOMETRY] = *d_grid_geometry.device_ptr();
        host_table[BUILDING_ENV_GRID_LAYOUT] = *d_grid_layout.device_ptr();
        host_table[BUILDING_ENV_CELL_STARTS] = *d_cell_starts.device_ptr();
        host_table[BUILDING_ENV_EDGE_REFS] = *d_edge_refs.device_ptr();
        host_table[BUILDING_ENV_EDGES] = *d_edges.device_ptr();
        host_table[BUILDING_ENV_EDGE_IS_BUILDING] = *d_edge_is_building.device_ptr();
        host_table[BUILDING_ENV_DEM_META] = *d_dem_meta.device_ptr();
        host_table[BUILDING_ENV_DEM_ELEVATION] = *d_dem_elevation.device_ptr();
        host_table[BUILDING_ENV_DEM_COLS] = elevation.cols as u64;
        host_table[BUILDING_ENV_DEM_ROWS] = elevation.rows as u64;
        host_table[BUILDING_ENV_DIRECTIONS] = *d_directions.device_ptr();
        host_table[BUILDING_ENV_TERRAIN_SAMPLES] = *d_terrain_samples.device_ptr();
        host_table[BUILDING_ENV_CELL_MAX_H] = *d_cell_max_h.device_ptr();
        let table = self
            .dev
            .htod_copy(host_table.to_vec())
            .context("building environment table")?;
        Ok(AirborneScreeningEnvironment {
            table,
            index_count: obstacles.indexes.len(),
            building_edge_count,
            dem_rows: elevation.rows,
            dem_cols: elevation.cols,
            cell_slots,
            index_geometry,
            _grid_geometry: d_grid_geometry,
            _grid_layout: d_grid_layout,
            _cell_starts: d_cell_starts,
            _edge_refs: d_edge_refs,
            _edges: d_edges,
            _edge_is_building: d_edge_is_building,
            _dem_meta: d_dem_meta,
            _dem_elevation: d_dem_elevation,
            _directions: d_directions,
            _terrain_samples: d_terrain_samples,
            _cell_max_h: d_cell_max_h,
        })
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
        bounds: &ScreeningBoundsDev,
    ) -> Result<BuildingHorizonDev> {
        let records = packed.records;
        let local_count = records * BUILDING_LOCAL_HORIZON_SECTORS;
        let mut global_max_tangent_bits = self
            .dev
            .alloc_zeros::<u16>(records)
            .context("building global maxima")?;
        let mut local_max_tangent_bits = self
            .dev
            .alloc_zeros::<u16>(local_count)
            .context("building local maxima")?;
        if environment.index_count == 0
            || environment.building_edge_count == 0
            || !packed.building_enabled.contains(&1)
        {
            let local_entries = self
                .dev
                .alloc_zeros::<u32>(1)
                .context("empty building entries")?;
            unsafe {
                self.mark_empty
                    .clone()
                    .launch(
                        launch_config(local_count),
                        (
                            records as i32,
                            &mut global_max_tangent_bits,
                            &mut local_max_tangent_bits,
                        ),
                    )
                    .context("launch empty building horizon")?;
            }
            self.dev
                .synchronize()
                .context("empty building horizon sync")?;
            return Ok(BuildingHorizonDev {
                global_max_tangent_bits,
                local_entries,
                local_max_tangent_bits,
            });
        }

        let entries = records * BUILDING_LOCAL_HORIZON_ENTRY_COUNT;
        let building_enabled = self
            .dev
            .htod_copy(packed.building_enabled.clone())
            .context("building receiver enablement")?;
        let mut best_tangent = self
            .dev
            .alloc_zeros::<f32>(entries)
            .context("building horizon workspace")?;
        let mut local_entries = self
            .dev
            .alloc_zeros::<u32>(entries)
            .context("building entries")?;
        unsafe {
            self.build
                .clone()
                .launch(
                    launch_config(records),
                    (
                        &environment.table,
                        receiver_lat_lon,
                        receiver_altitude,
                        inner_elevation,
                        tile_bbox,
                        pixel_of_record,
                        &building_enabled,
                        &bounds.table,
                        records as i32,
                        &mut best_tangent,
                        &mut local_entries,
                    ),
                )
                .context("launch building horizon build")?;
            self.pack
                .clone()
                .launch(
                    launch_config(local_count),
                    (
                        records as i32,
                        &best_tangent,
                        &mut local_entries,
                        &mut local_max_tangent_bits,
                    ),
                )
                .context("launch building horizon pack")?;
            self.global_max
                .clone()
                .launch(
                    launch_config(records),
                    (
                        records as i32,
                        &local_max_tangent_bits,
                        &mut global_max_tangent_bits,
                    ),
                )
                .context("launch building horizon global max")?;
        }
        self.dev.synchronize().context("building horizon sync")?;
        drop(best_tangent);
        Ok(BuildingHorizonDev {
            global_max_tangent_bits,
            local_entries,
            local_max_tangent_bits,
        })
    }
}

fn ensure_nonempty<T: Default>(values: &mut Vec<T>) {
    if values.is_empty() {
        values.push(T::default());
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

const _: () = assert!(BUILDING_LOCAL_HORIZON_BANDS == 6);
