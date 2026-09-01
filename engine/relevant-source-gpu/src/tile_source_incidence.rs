//! Metric tile lattice and clipped source incidence for corners and neighbouring blocks.

use tile_painter::grid::{tile_bbox, TileBbox};

use crate::source_frame::{
    DeviceLineSource, RegionMetricFrame, BLOCKS_PER_TILE_SIDE, BLOCK_COUNT, BLOCK_PIXEL_SIDE,
    CORNERS_PER_TILE_SIDE, CORNER_COUNT, TILE_PIXEL_SIDE,
};

/// Source candidates at shared corners and automatic 3x3-neighbour membership per block.
pub struct TileSourceIncidence {
    pub corner_offsets: Vec<u32>,
    pub corner_source_indices: Vec<u32>,
    pub local_source_indices_by_block: Vec<Vec<u32>>,
}

/// Metric pixel/block lattice for one z12 tile.
pub struct TileMetricLattice {
    pub block_x_edges_m: [f32; CORNERS_PER_TILE_SIDE],
    pub block_y_edges_m: [f32; CORNERS_PER_TILE_SIDE],
    pub pixel_x_centres_m: Vec<f32>,
    pub pixel_y_centres_m: Vec<f32>,
    neighbourhood_west_m: f32,
    neighbourhood_east_m: f32,
    neighbourhood_north_m: f32,
    neighbourhood_south_m: f32,
}

impl TileMetricLattice {
    pub fn for_tile(frame: &RegionMetricFrame, zoom: u8, x: u32, y: u32) -> Self {
        Self::for_bbox(frame, tile_bbox(zoom, x, y))
    }

    pub fn for_bbox(frame: &RegionMetricFrame, bbox: TileBbox) -> Self {
        let block_fraction = BLOCK_PIXEL_SIDE as f64 / TILE_PIXEL_SIDE as f64;
        let mut block_x_edges_m = [0.0; CORNERS_PER_TILE_SIDE];
        let mut block_y_edges_m = [0.0; CORNERS_PER_TILE_SIDE];
        for block in 0..CORNERS_PER_TILE_SIDE {
            let fraction = (block * BLOCK_PIXEL_SIDE) as f64 / TILE_PIXEL_SIDE as f64;
            let longitude = bbox.west_lon + (bbox.east_lon - bbox.west_lon) * fraction;
            let latitude = latitude_at_pixel_fraction(bbox, fraction);
            block_x_edges_m[block] = frame.encode(latitude, longitude)[0];
            block_y_edges_m[block] = frame.encode(latitude, longitude)[1];
        }
        let mut pixel_x_centres_m = Vec::with_capacity(TILE_PIXEL_SIDE);
        let mut pixel_y_centres_m = Vec::with_capacity(TILE_PIXEL_SIDE);
        for pixel in 0..TILE_PIXEL_SIDE {
            let fraction = (pixel as f64 + 0.5) / TILE_PIXEL_SIDE as f64;
            let longitude = bbox.west_lon + (bbox.east_lon - bbox.west_lon) * fraction;
            let latitude = latitude_at_pixel_fraction(bbox, fraction);
            pixel_x_centres_m.push(frame.encode(latitude, longitude)[0]);
            pixel_y_centres_m.push(frame.encode(latitude, longitude)[1]);
        }
        Self {
            block_x_edges_m,
            block_y_edges_m,
            pixel_x_centres_m,
            pixel_y_centres_m,
            neighbourhood_west_m: frame.encode(
                bbox.north_lat,
                bbox.west_lon - (bbox.east_lon - bbox.west_lon) * block_fraction,
            )[0],
            neighbourhood_east_m: frame.encode(
                bbox.north_lat,
                bbox.east_lon + (bbox.east_lon - bbox.west_lon) * block_fraction,
            )[0],
            neighbourhood_north_m: frame.encode(
                latitude_at_pixel_fraction(bbox, -block_fraction),
                bbox.west_lon,
            )[1],
            neighbourhood_south_m: frame.encode(
                latitude_at_pixel_fraction(bbox, 1.0 + block_fraction),
                bbox.west_lon,
            )[1],
        }
    }

    pub fn corner_xy(&self, corner: usize) -> [f32; 2] {
        let row = corner / CORNERS_PER_TILE_SIDE;
        let column = corner % CORNERS_PER_TILE_SIDE;
        [self.block_x_edges_m[column], self.block_y_edges_m[row]]
    }

    fn neighbourhood_rectangle(&self, row: usize, column: usize) -> [f32; 4] {
        let minimum_x = if column == 0 {
            self.neighbourhood_west_m
        } else {
            self.block_x_edges_m[column - 1]
        };
        let maximum_x = if column + 2 > BLOCKS_PER_TILE_SIDE {
            self.neighbourhood_east_m
        } else {
            self.block_x_edges_m[column + 2]
        };
        let maximum_y = if row == 0 {
            self.neighbourhood_north_m
        } else {
            self.block_y_edges_m[row - 1]
        };
        let minimum_y = if row + 2 > BLOCKS_PER_TILE_SIDE {
            self.neighbourhood_south_m
        } else {
            self.block_y_edges_m[row + 2]
        };
        [minimum_x, minimum_y, maximum_x, maximum_y]
    }
}

pub fn build_tile_source_incidence(
    sources: &[DeviceLineSource],
    lattice: &TileMetricLattice,
) -> TileSourceIncidence {
    let mut corner_lists = vec![Vec::new(); CORNER_COUNT];
    let mut local_source_indices_by_block = vec![Vec::new(); BLOCK_COUNT];
    for (source_index, source) in sources.iter().enumerate() {
        let source_index = source_index as u32;
        add_source_corner_candidates(source, source_index, lattice, &mut corner_lists);
        add_source_local_blocks(
            source,
            source_index,
            lattice,
            &mut local_source_indices_by_block,
        );
    }
    let mut corner_offsets = Vec::with_capacity(CORNER_COUNT + 1);
    let mut corner_source_indices = Vec::new();
    corner_offsets.push(0);
    for list in corner_lists {
        corner_source_indices.extend(list);
        corner_offsets.push(corner_source_indices.len() as u32);
    }
    TileSourceIncidence {
        corner_offsets,
        corner_source_indices,
        local_source_indices_by_block,
    }
}

fn add_source_corner_candidates(
    source: &DeviceLineSource,
    source_index: u32,
    lattice: &TileMetricLattice,
    corner_lists: &mut [Vec<u32>],
) {
    let minimum_x = source.start_x_m.min(source.end_x_m) - source.max_distance_m;
    let maximum_x = source.start_x_m.max(source.end_x_m) + source.max_distance_m;
    let minimum_y = source.start_y_m.min(source.end_y_m) - source.max_distance_m;
    let maximum_y = source.start_y_m.max(source.end_y_m) + source.max_distance_m;
    let column_start = lattice.block_x_edges_m.partition_point(|&x| x < minimum_x);
    let column_end = lattice.block_x_edges_m.partition_point(|&x| x <= maximum_x);
    let row_start = lattice.block_y_edges_m.partition_point(|&y| y > maximum_y);
    let row_end = lattice.block_y_edges_m.partition_point(|&y| y >= minimum_y);
    for row in row_start..row_end {
        for column in column_start..column_end {
            let point = [
                lattice.block_x_edges_m[column],
                lattice.block_y_edges_m[row],
            ];
            if point_to_segment_distance_squared(point, source)
                <= source.max_distance_m * source.max_distance_m
            {
                corner_lists[row * CORNERS_PER_TILE_SIDE + column].push(source_index);
            }
        }
    }
}

fn add_source_local_blocks(
    source: &DeviceLineSource,
    source_index: u32,
    lattice: &TileMetricLattice,
    local_by_block: &mut [Vec<u32>],
) {
    let source_minimum_x = source.start_x_m.min(source.end_x_m);
    let source_maximum_x = source.start_x_m.max(source.end_x_m);
    let source_minimum_y = source.start_y_m.min(source.end_y_m);
    let source_maximum_y = source.start_y_m.max(source.end_y_m);
    let outer_left = lattice.neighbourhood_rectangle(0, 0)[0];
    let outer_top = lattice.neighbourhood_rectangle(0, 0)[3];
    let outer_right =
        lattice.neighbourhood_rectangle(BLOCKS_PER_TILE_SIDE - 1, BLOCKS_PER_TILE_SIDE - 1)[2];
    let outer_bottom =
        lattice.neighbourhood_rectangle(BLOCKS_PER_TILE_SIDE - 1, BLOCKS_PER_TILE_SIDE - 1)[1];
    if source_maximum_x < outer_left
        || source_minimum_x > outer_right
        || source_maximum_y < outer_bottom
        || source_minimum_y > outer_top
    {
        return;
    }
    let columns: Vec<usize> = (0..BLOCKS_PER_TILE_SIDE)
        .filter(|&column| {
            let rectangle = lattice.neighbourhood_rectangle(0, column);
            rectangle[2] >= source_minimum_x && rectangle[0] <= source_maximum_x
        })
        .collect();
    let rows: Vec<usize> = (0..BLOCKS_PER_TILE_SIDE)
        .filter(|&row| {
            let rectangle = lattice.neighbourhood_rectangle(row, 0);
            rectangle[3] >= source_minimum_y && rectangle[1] <= source_maximum_y
        })
        .collect();
    for row in rows {
        for &column in &columns {
            let rectangle = lattice.neighbourhood_rectangle(row, column);
            if segment_intersects_rectangle(source, rectangle) {
                local_by_block[row * BLOCKS_PER_TILE_SIDE + column].push(source_index);
            }
        }
    }
}

fn segment_intersects_rectangle(source: &DeviceLineSource, rectangle: [f32; 4]) -> bool {
    let mut minimum_t = 0.0_f32;
    let mut maximum_t = 1.0_f32;
    for (origin, direction, minimum, maximum) in [
        (
            source.start_x_m,
            source.end_x_m - source.start_x_m,
            rectangle[0],
            rectangle[2],
        ),
        (
            source.start_y_m,
            source.end_y_m - source.start_y_m,
            rectangle[1],
            rectangle[3],
        ),
    ] {
        if direction.abs() < f32::EPSILON {
            if origin < minimum || origin > maximum {
                return false;
            }
        } else {
            let first = (minimum - origin) / direction;
            let second = (maximum - origin) / direction;
            minimum_t = minimum_t.max(first.min(second));
            maximum_t = maximum_t.min(first.max(second));
            if minimum_t > maximum_t {
                return false;
            }
        }
    }
    true
}

fn point_to_segment_distance_squared(point: [f32; 2], source: &DeviceLineSource) -> f32 {
    let dx = source.end_x_m - source.start_x_m;
    let dy = source.end_y_m - source.start_y_m;
    let denominator = dx * dx + dy * dy;
    let t = if denominator > 0.0 {
        (((point[0] - source.start_x_m) * dx + (point[1] - source.start_y_m) * dy) / denominator)
            .clamp(0.0, 1.0)
    } else {
        0.0
    };
    let difference_x = point[0] - (source.start_x_m + t * dx);
    let difference_y = point[1] - (source.start_y_m + t * dy);
    difference_x * difference_x + difference_y * difference_y
}

fn latitude_at_pixel_fraction(bbox: TileBbox, fraction: f64) -> f64 {
    let mercator = |latitude: f64| {
        let radians = latitude.to_radians();
        (radians.tan() + 1.0 / radians.cos()).ln()
    };
    let value =
        mercator(bbox.north_lat) + fraction * (mercator(bbox.south_lat) - mercator(bbox.north_lat));
    (2.0 * value.exp().atan() - std::f64::consts::PI / 2.0).to_degrees()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_lattice() -> TileMetricLattice {
        TileMetricLattice::for_bbox(
            &RegionMetricFrame::for_latitude_longitude(0.0, 0.0),
            TileBbox {
                west_lon: 0.0,
                east_lon: 1.0,
                north_lat: 1.0,
                south_lat: 0.0,
            },
        )
    }

    #[test]
    fn local_membership_uses_the_complete_neighbourhood() {
        let lattice = fixture_lattice();
        let y = (lattice.block_y_edges_m[2] + lattice.block_y_edges_m[3]) * 0.5;
        let source = DeviceLineSource {
            start_x_m: lattice.block_x_edges_m[2],
            start_y_m: y,
            end_x_m: lattice.block_x_edges_m[3],
            end_y_m: y,
            max_distance_m: 1.0,
            ..DeviceLineSource::default()
        };
        let incidence = build_tile_source_incidence(&[source], &lattice);
        assert!(incidence.local_source_indices_by_block[3 * BLOCKS_PER_TILE_SIDE + 3].contains(&0));
        assert!(
            !incidence.local_source_indices_by_block[5 * BLOCKS_PER_TILE_SIDE + 5].contains(&0)
        );
    }

    #[test]
    fn tile_edge_block_includes_a_source_in_the_adjacent_tile_block() {
        let lattice = fixture_lattice();
        let block_width = lattice.block_x_edges_m[1] - lattice.block_x_edges_m[0];
        let y = (lattice.block_y_edges_m[0] + lattice.block_y_edges_m[1]) * 0.5;
        let source = DeviceLineSource {
            start_x_m: lattice.block_x_edges_m[0] - block_width * 0.5,
            start_y_m: y,
            end_x_m: lattice.block_x_edges_m[0] - block_width * 0.25,
            end_y_m: y,
            max_distance_m: 1.0,
            ..DeviceLineSource::default()
        };
        let incidence = build_tile_source_incidence(&[source], &lattice);
        assert!(incidence.local_source_indices_by_block[0].contains(&0));
        assert!(!incidence.local_source_indices_by_block[2].contains(&0));
    }

    #[test]
    fn edge_neighbourhood_uses_the_adjacent_tiles_exact_block_edges() {
        let frame = RegionMetricFrame::for_latitude_longitude(50.0, 14.0);
        let (x, y) = (2207, 1391);
        let lattice = TileMetricLattice::for_tile(&frame, 12, x, y);
        let west = TileMetricLattice::for_tile(&frame, 12, x - 1, y);
        let east = TileMetricLattice::for_tile(&frame, 12, x + 1, y);
        let north = TileMetricLattice::for_tile(&frame, 12, x, y - 1);
        let south = TileMetricLattice::for_tile(&frame, 12, x, y + 1);

        let north_west = lattice.neighbourhood_rectangle(0, 0);
        assert_eq!(
            north_west[0],
            west.block_x_edges_m[BLOCKS_PER_TILE_SIDE - 1]
        );
        assert_eq!(
            north_west[3],
            north.block_y_edges_m[BLOCKS_PER_TILE_SIDE - 1]
        );
        let south_east =
            lattice.neighbourhood_rectangle(BLOCKS_PER_TILE_SIDE - 1, BLOCKS_PER_TILE_SIDE - 1);
        assert_eq!(south_east[2], east.block_x_edges_m[1]);
        assert_eq!(south_east[1], south.block_y_edges_m[1]);
    }
}
