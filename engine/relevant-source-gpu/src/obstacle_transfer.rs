//! Flatten obstacle indexes and tile barriers into the relevant-source metric scene.

use noise_compute::constants::M_PER_DEG_LAT;
use noise_compute::propagation::obstacle_index::ObstacleSet;
use noise_compute::types::Barrier;
use raster_reader::FusedGrid;

use crate::source_frame::RegionMetricFrame;

/// Region coordinates to the cells of one fused terrain/land-cover halo.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct DeviceRasterGeometry {
    pub row_scale_per_metre: f32,
    pub row_offset: f32,
    pub column_scale_per_metre: f32,
    pub column_offset: f32,
    pub rows: u32,
    pub columns: u32,
}

impl DeviceRasterGeometry {
    pub fn for_grid(frame: &RegionMetricFrame, grid: &FusedGrid) -> Self {
        let (latitude_min, longitude_min, inverse_cell_degrees, rows, columns) = grid.geom();
        Self {
            row_scale_per_metre: (inverse_cell_degrees / M_PER_DEG_LAT) as f32,
            row_offset: ((frame.reference_latitude() - latitude_min) * inverse_cell_degrees) as f32,
            column_scale_per_metre: (inverse_cell_degrees / frame.metres_per_longitude_degree())
                as f32,
            column_offset: ((frame.reference_longitude() - longitude_min) * inverse_cell_degrees)
                as f32,
            rows: rows as u32,
            columns: columns as u32,
        }
    }
}

/// Offsets and frame conversion for one independently centred obstacle CSR grid.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct DeviceObstacleGrid {
    pub query_x_scale: f32,
    pub query_x_offset_m: f32,
    pub query_y_offset_m: f32,
    pub cell_m: f32,
    pub minimum_x_m: f32,
    pub minimum_y_m: f32,
    pub columns: u32,
    pub rows: u32,
    pub cell_starts_offset: u32,
    pub edge_references_offset: u32,
    pub edge_values_offset: u32,
    pub cell_maximum_height_offset: u32,
}

/// One vector wall in the region frame, retained exactly as a segment.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct DeviceBarrier {
    pub start_x_m: f32,
    pub start_y_m: f32,
    pub end_x_m: f32,
    pub end_y_m: f32,
    pub height_m: f32,
    pub receiver_distance_lower_bound_m: f32,
}

/// All obstacle-grid arrays concatenated once for a region.
#[derive(Default)]
pub struct FlattenedObstacleGeometry {
    pub grids: Vec<DeviceObstacleGrid>,
    pub cell_starts: Vec<u32>,
    pub edge_references: Vec<u32>,
    pub edge_values_xyxyh: Vec<f32>,
    pub cell_maximum_heights: Vec<f32>,
}

impl FlattenedObstacleGeometry {
    pub fn from_set(frame: &RegionMetricFrame, set: &ObstacleSet) -> Self {
        let mut flattened = Self::default();
        for index in &set.indexes {
            let view = index.gpu_view();
            let grid = DeviceObstacleGrid {
                query_x_scale: (view.m_per_deg_lon / frame.metres_per_longitude_degree()) as f32,
                query_x_offset_m: ((frame.reference_longitude() - view.origin_lon)
                    * view.m_per_deg_lon) as f32,
                query_y_offset_m: ((frame.reference_latitude() - view.origin_lat) * M_PER_DEG_LAT)
                    as f32,
                cell_m: view.cell_m as f32,
                minimum_x_m: view.min_x as f32,
                minimum_y_m: view.min_y as f32,
                columns: view.cols as u32,
                rows: view.rows as u32,
                cell_starts_offset: flattened.cell_starts.len() as u32,
                edge_references_offset: flattened.edge_references.len() as u32,
                edge_values_offset: (flattened.edge_values_xyxyh.len() / 5) as u32,
                cell_maximum_height_offset: flattened.cell_maximum_heights.len() as u32,
            };
            flattened.grids.push(grid);
            flattened.cell_starts.extend_from_slice(view.cell_starts);
            flattened.edge_references.extend_from_slice(view.edge_refs);
            flattened
                .edge_values_xyxyh
                .extend_from_slice(&view.edges_xyxyh);
            flattened
                .cell_maximum_heights
                .extend_from_slice(view.cell_max_h);
        }
        flattened
    }
}

pub fn encode_barriers(frame: &RegionMetricFrame, barriers: &[Barrier]) -> Vec<DeviceBarrier> {
    barriers
        .iter()
        .map(|barrier| {
            let start = frame.encode(barrier.start_lat, barrier.start_lon);
            let end = frame.encode(barrier.end_lat, barrier.end_lon);
            DeviceBarrier {
                start_x_m: start[0],
                start_y_m: start[1],
                end_x_m: end[0],
                end_y_m: end[1],
                height_m: barrier.height_m,
                receiver_distance_lower_bound_m: barrier.dist_m as f32,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cuda_transfer_layouts_are_fixed() {
        assert_eq!(std::mem::size_of::<DeviceRasterGeometry>(), 24);
        assert_eq!(std::mem::size_of::<DeviceObstacleGrid>(), 48);
        assert_eq!(std::mem::size_of::<DeviceBarrier>(), 24);
    }

    #[test]
    fn barrier_endpoints_share_the_region_frame() {
        let frame = RegionMetricFrame::for_latitude_longitude(50.0, 14.0);
        let barrier = Barrier {
            osm_id: 1,
            segment_idx: 0,
            height_m: 3.0,
            start_lat: 50.001,
            start_lon: 14.002,
            end_lat: 50.003,
            end_lon: 14.004,
            dist_m: 7.0,
        };
        let encoded = encode_barriers(&frame, &[barrier])[0];
        assert_eq!(
            [encoded.start_x_m, encoded.start_y_m],
            frame.encode(barrier.start_lat, barrier.start_lon)
        );
        assert_eq!(encoded.receiver_distance_lower_bound_m, 7.0);
    }
}
