//! Flatten obstacle indexes — buildings and noise-barrier edges alike — into
//! the relevant-source metric scene.

use noise_compute::constants::M_PER_DEG_LAT;
use noise_compute::propagation::obstacle_index::ObstacleSet;
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
            column_offset: (grid::geo::wrapped_longitude_delta(
                longitude_min,
                frame.reference_longitude(),
            ) * inverse_cell_degrees) as f32,
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
    /// First edge of this grid in the region's edge arrays, in edges.
    pub edge_index_offset: u32,
    pub cell_maximum_height_offset: u32,
}

/// One obstacle edge's endpoints, `(start_x_m, start_y_m)` to `(end_x_m, end_y_m)`,
/// in its grid's query frame.
///
/// Sixteen bytes, the kernel's `float4`: the obstacle scan tests every edge of
/// every cell it opens against the ray, which is the painter's hottest load, so
/// the record is sized to land inside one memory sector however scattered the
/// edge index is.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct DeviceObstacleEdgeEndpoints {
    pub start_x_m: f32,
    pub start_y_m: f32,
    pub end_x_m: f32,
    pub end_y_m: f32,
}

/// All obstacle-grid arrays concatenated once for a region.
#[derive(Default)]
pub struct FlattenedObstacleGeometry {
    pub grids: Vec<DeviceObstacleGrid>,
    pub cell_starts: Vec<u32>,
    pub edge_references: Vec<u32>,
    /// One record per edge. The five-float record this replaces put every other
    /// edge's endpoints across two memory sectors, and the height it carried is
    /// read only for an edge the ray actually crosses.
    pub edge_endpoints: Vec<DeviceObstacleEdgeEndpoints>,
    /// Edge height, one per edge, under the same `edge_index_offset`.
    pub edge_height_m: Vec<f32>,
    /// Building flag, one per edge, under the same `edge_index_offset`.
    pub edge_is_building: Vec<u8>,
    pub cell_maximum_heights: Vec<f32>,
}

impl FlattenedObstacleGeometry {
    pub fn from_set(frame: &RegionMetricFrame, set: &ObstacleSet) -> Self {
        let mut flattened = Self::default();
        for index in &set.indexes {
            let view = index.gpu_view();
            let grid = DeviceObstacleGrid {
                query_x_scale: (view.m_per_deg_lon / frame.metres_per_longitude_degree()) as f32,
                query_x_offset_m: (grid::geo::wrapped_longitude_delta(
                    view.origin_lon,
                    frame.reference_longitude(),
                ) * view.m_per_deg_lon) as f32,
                query_y_offset_m: ((frame.reference_latitude() - view.origin_lat) * M_PER_DEG_LAT)
                    as f32,
                cell_m: view.cell_m as f32,
                minimum_x_m: view.min_x as f32,
                minimum_y_m: view.min_y as f32,
                columns: view.cols as u32,
                rows: view.rows as u32,
                cell_starts_offset: flattened.cell_starts.len() as u32,
                edge_references_offset: flattened.edge_references.len() as u32,
                edge_index_offset: flattened.edge_height_m.len() as u32,
                cell_maximum_height_offset: flattened.cell_maximum_heights.len() as u32,
            };
            flattened.grids.push(grid);
            flattened.cell_starts.extend_from_slice(view.cell_starts);
            flattened.edge_references.extend_from_slice(view.edge_refs);
            // `gpu_view` materialises the edge arrays in one pass over the same
            // edge slice, so they concatenate in the same order and answer to the
            // same edge index. A view that disagreed would slide every later edge's
            // endpoints under another edge's height and paint the result with no
            // other sign, so it ends the process here instead.
            assert_eq!(
                view.edges_xyxyh.len() % 5,
                0,
                "an obstacle view's xyxyh array is not whole edges"
            );
            let edge_count = view.edges_xyxyh.len() / 5;
            assert_eq!(
                view.edge_is_building.len(),
                edge_count,
                "an obstacle view carries {} building flags for {edge_count} edges",
                view.edge_is_building.len()
            );
            for edge in view.edges_xyxyh.chunks_exact(5) {
                flattened.edge_endpoints.push(DeviceObstacleEdgeEndpoints {
                    start_x_m: edge[0],
                    start_y_m: edge[1],
                    end_x_m: edge[2],
                    end_y_m: edge[3],
                });
                flattened.edge_height_m.push(edge[4]);
            }
            flattened
                .edge_is_building
                .extend_from_slice(&view.edge_is_building);
            flattened
                .cell_maximum_heights
                .extend_from_slice(view.cell_max_h);
        }
        flattened
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cuda_transfer_layouts_are_fixed() {
        assert_eq!(std::mem::size_of::<DeviceRasterGeometry>(), 24);
        assert_eq!(std::mem::size_of::<DeviceObstacleGrid>(), 48);
        assert_eq!(std::mem::size_of::<DeviceObstacleEdgeEndpoints>(), 16);
    }

    /// Host replica of `relevant_source_grid_scan.cuh`'s `scan_obstacle_grid`:
    /// the DDA cell walk over the flattened CSR, the f32 segment crossing, and
    /// the admission gate — a crossing counts when it is a BARRIER edge or lies
    /// at least `exclusion_radius_m` along the path. The kernel's grid-miss
    /// rejects and its branch-and-bound prune are left out: they only skip
    /// cells, they never change a verdict.
    fn gpu_rule_admitted_crossings(
        flattened: &FlattenedObstacleGeometry,
        source_x_m: f32,
        source_y_m: f32,
        receiver_x_m: f32,
        receiver_y_m: f32,
        distance_m: f32,
        exclusion_radius_m: f32,
    ) -> Vec<(f32, bool)> {
        let mut admitted: Vec<(f32, bool, u32)> = Vec::new();
        for grid in &flattened.grids {
            let start_x = source_x_m.mul_add(grid.query_x_scale, grid.query_x_offset_m);
            let start_y = source_y_m + grid.query_y_offset_m;
            let end_x = receiver_x_m.mul_add(grid.query_x_scale, grid.query_x_offset_m);
            let end_y = receiver_y_m + grid.query_y_offset_m;
            let dx = end_x - start_x;
            let dy = end_y - start_y;
            let inverse_cell = 1.0 / grid.cell_m;
            let cell_of = |value: f32, base: f32, count: u32| -> i32 {
                (((value - base) * inverse_cell).floor() as i32).clamp(0, count as i32 - 1)
            };
            let mut cell_x = cell_of(start_x, grid.minimum_x_m, grid.columns);
            let mut cell_y = cell_of(start_y, grid.minimum_y_m, grid.rows);
            let end_cell_x = cell_of(end_x, grid.minimum_x_m, grid.columns);
            let end_cell_y = cell_of(end_y, grid.minimum_y_m, grid.rows);
            let step_x: i32 = if dx >= 0.0 { 1 } else { -1 };
            let step_y: i32 = if dy >= 0.0 { 1 } else { -1 };
            let delta_t_x = if dx != 0.0 {
                (grid.cell_m / dx).abs()
            } else {
                f32::INFINITY
            };
            let delta_t_y = if dy != 0.0 {
                (grid.cell_m / dy).abs()
            } else {
                f32::INFINITY
            };
            let next_x = grid.minimum_x_m + (cell_x + i32::from(dx >= 0.0)) as f32 * grid.cell_m;
            let next_y = grid.minimum_y_m + (cell_y + i32::from(dy >= 0.0)) as f32 * grid.cell_m;
            let mut maximum_t_x = if dx != 0.0 {
                ((next_x - start_x) / dx).abs()
            } else {
                f32::INFINITY
            };
            let mut maximum_t_y = if dy != 0.0 {
                ((next_y - start_y) / dy).abs()
            } else {
                f32::INFINITY
            };
            let mut guard = (grid.columns + grid.rows) as i32 + 4;
            loop {
                let cell = cell_y as u32 * grid.columns + cell_x as u32;
                let first = flattened.cell_starts[(grid.cell_starts_offset + cell) as usize];
                let end = flattened.cell_starts[(grid.cell_starts_offset + cell + 1) as usize];
                for position in first..end {
                    let local_edge = flattened.edge_references
                        [(grid.edge_references_offset + position) as usize];
                    let edge = grid.edge_index_offset + local_edge;
                    let ends = flattened.edge_endpoints[edge as usize];
                    if let Some(crossing_t) = segment_crossing_fraction(
                        start_x,
                        start_y,
                        dx,
                        dy,
                        ends.start_x_m,
                        ends.start_y_m,
                        ends.end_x_m,
                        ends.end_y_m,
                    ) {
                        let is_building = flattened.edge_is_building[edge as usize] != 0;
                        if !is_building || crossing_t * distance_m >= exclusion_radius_m {
                            admitted.push((crossing_t, is_building, edge));
                        }
                    }
                }
                if (cell_x == end_cell_x && cell_y == end_cell_y) || guard <= 0 {
                    break;
                }
                guard -= 1;
                if maximum_t_x < maximum_t_y {
                    maximum_t_x += delta_t_x;
                    cell_x += step_x;
                } else {
                    maximum_t_y += delta_t_y;
                    cell_y += step_y;
                }
                if cell_x < 0
                    || cell_y < 0
                    || cell_x >= grid.columns as i32
                    || cell_y >= grid.rows as i32
                {
                    break;
                }
            }
        }
        // Supercover binning lists one edge in every cell it passes through;
        // the walk re-tests it there, so keep one verdict per edge.
        admitted.sort_by_key(|entry| entry.2);
        admitted.dedup_by_key(|entry| entry.2);
        admitted.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        admitted
            .into_iter()
            .map(|(t, is_building, _)| (t, is_building))
            .collect()
    }

    /// The kernel's `segment_crossing_fraction`, f32 for f32.
    #[allow(clippy::too_many_arguments)]
    fn segment_crossing_fraction(
        ray_start_x: f32,
        ray_start_y: f32,
        ray_dx: f32,
        ray_dy: f32,
        edge_start_x: f32,
        edge_start_y: f32,
        edge_end_x: f32,
        edge_end_y: f32,
    ) -> Option<f32> {
        let edge_dx = edge_end_x - edge_start_x;
        let edge_dy = edge_end_y - edge_start_y;
        let denominator = ray_dx * edge_dy - ray_dy * edge_dx;
        if denominator.abs() < 1.0e-8 {
            return None;
        }
        let from_ray_x = edge_start_x - ray_start_x;
        let from_ray_y = edge_start_y - ray_start_y;
        let ray_fraction = (from_ray_x * edge_dy - from_ray_y * edge_dx) / denominator;
        let edge_fraction = (from_ray_x * ray_dy - from_ray_y * ray_dx) / denominator;
        if ray_fraction <= 1.0e-7
            || ray_fraction >= 1.0 - 1.0e-7
            || !(0.0..=1.0).contains(&edge_fraction)
        {
            return None;
        }
        Some(ray_fraction)
    }

    /// The kernel rule against the noise-compute oracle on one synthetic scene:
    /// a wall and a building BOTH inside the source exclusion radius (the old
    /// DeviceBarrier scan and the old kind-blind grid gate disagree here), plus
    /// a wall and a building outside it. The oracle is `ObstacleSet::crossings`
    /// filtered by the path_effects kind rule; both lanes must admit exactly
    /// the same edges.
    #[test]
    fn barrier_crossings_survive_source_exclusion_on_the_gpu_rule() {
        use noise_compute::constants::m_per_deg_lon;
        use noise_compute::propagation::geo::flat_dist;
        use noise_compute::propagation::obstacle_index::{ObstacleIndex, ObstacleKind};
        use std::sync::Arc;

        let origin: (f64, f64) = (50.0, 14.0);
        let m_per_deg_lon = m_per_deg_lon(origin.0.to_radians());
        let at = |north_m: f64, east_m: f64| {
            (
                origin.0 + north_m / M_PER_DEG_LAT,
                origin.1 + east_m / m_per_deg_lon,
            )
        };
        let square = |north_m: f64, east_m: f64, half: f64| {
            vec![
                at(north_m - half, east_m - half),
                at(north_m - half, east_m + half),
                at(north_m + half, east_m + half),
                at(north_m + half, east_m - half),
            ]
        };
        let mut builder = ObstacleIndex::builder(origin.0, origin.1);
        // The wall that pins the bug class: 40 m from the source, INSIDE the
        // 100 m exclusion radius; a kind-blind gate would drop it.
        builder.add_polyline(
            &[at(-150.0, 40.0), at(150.0, 40.0)],
            4.0,
            ObstacleKind::Barrier,
            0,
        );
        // A building whose crossing is inside the exclusion radius: excluded.
        builder.add_ring(&square(0.0, 60.0, 10.0), 8.0, ObstacleKind::Building, 1);
        // A building and a wall beyond the radius: admitted.
        builder.add_ring(&square(0.0, 250.0, 10.0), 8.0, ObstacleKind::Building, 2);
        builder.add_polyline(
            &[at(-150.0, 300.0), at(150.0, 300.0)],
            3.0,
            ObstacleKind::Barrier,
            3,
        );
        let set = ObstacleSet {
            indexes: vec![Arc::new(builder.build())],
        };

        let (source_lat, source_lon) = origin;
        let (receiver_lat, receiver_lon) = at(0.0, 400.0);
        let distance_m = flat_dist(source_lat, source_lon, receiver_lat, receiver_lon);
        let exclusion_radius_m = 100.0;

        let mut candidates = Vec::new();
        set.crossings(
            source_lat,
            source_lon,
            receiver_lat,
            receiver_lon,
            &mut candidates,
        );
        // Vacuity: the oracle must see both in-radius edges before the filter,
        // or the fixture proves nothing about the rule.
        assert!(
            candidates
                .iter()
                .any(|c| c.kind == ObstacleKind::Barrier && c.t * distance_m < exclusion_radius_m),
            "the wall crossing must sit inside the exclusion radius"
        );
        assert!(
            candidates
                .iter()
                .any(|c| c.kind == ObstacleKind::Building && c.t * distance_m < exclusion_radius_m),
            "the near building crossing must sit inside the exclusion radius"
        );
        let oracle: Vec<(f64, bool)> = candidates
            .iter()
            .filter(|c| {
                !(matches!(c.kind, ObstacleKind::Building) && c.t * distance_m < exclusion_radius_m)
            })
            .map(|c| (c.t, matches!(c.kind, ObstacleKind::Building)))
            .collect();

        // The painted cell: the frame shares the index origin, so the grid
        // transform is exactly identity (query_x_scale 1.0, offsets 0.0).
        let frame = RegionMetricFrame::for_latitude_longitude(origin.0, origin.1);
        let flattened = FlattenedObstacleGeometry::from_set(&frame, &set);
        assert_eq!(
            flattened.edge_is_building.len(),
            flattened.edge_endpoints.len()
        );
        let source = frame.encode(source_lat, source_lon);
        let receiver = frame.encode(receiver_lat, receiver_lon);
        let replica = gpu_rule_admitted_crossings(
            &flattened,
            source[0],
            source[1],
            receiver[0],
            receiver[1],
            distance_m as f32,
            exclusion_radius_m as f32,
        );

        assert_eq!(
            replica.len(),
            oracle.len(),
            "replica {replica:?} vs oracle {oracle:?}"
        );
        for ((replica_t, replica_is_building), (oracle_t, oracle_is_building)) in
            replica.iter().zip(&oracle)
        {
            assert!(
                (f64::from(*replica_t) - oracle_t).abs() < 1e-4,
                "chainage {replica_t} vs oracle {oracle_t}"
            );
            assert_eq!(replica_is_building, oracle_is_building);
        }
    }
}
