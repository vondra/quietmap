//! Native one-arcsecond node windows physically partitioned by z9, including both poles.

use crate::{geo::normalize_longitude, Square, Z9_TILES_PER_AXIS};

pub const NODES_PER_DEGREE: i32 = 3600;
pub const SOURCE_TILE_SIDE: usize = NODES_PER_DEGREE as usize + 1;
pub const LONGITUDE_NODES: i32 = 360 * NODES_PER_DEGREE;
pub const POLE_NODE: i32 = 90 * NODES_PER_DEGREE;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RasterWindow {
    pub north_node: i32,
    pub west_node: i32,
    pub rows: u32,
    pub columns: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct RasterSamplePosition {
    pub row: u32,
    pub column: u32,
    pub row_fraction: f64,
    pub column_fraction: f64,
    pub nearest_row: u32,
    pub nearest_column: u32,
}

fn latitude_edge_node(y: u16) -> f64 {
    let mercator = std::f64::consts::PI * (1.0 - 2.0 * f64::from(y) / f64::from(Z9_TILES_PER_AXIS));
    mercator.sinh().atan().to_degrees() * f64::from(NODES_PER_DEGREE)
}

impl RasterWindow {
    pub fn for_square(square: Square) -> Self {
        assert!(square.x < Z9_TILES_PER_AXIS && square.y < Z9_TILES_PER_AXIS);
        let axis = i32::from(Z9_TILES_PER_AXIS);
        // Longitude edges have a denominator of 512; keep exact integer floor/ceil.
        let west_node = i32::from(square.x) * LONGITUDE_NODES / axis - LONGITUDE_NODES / 2;
        let east_node =
            ((i32::from(square.x) + 1) * LONGITUDE_NODES + axis - 1) / axis - LONGITUDE_NODES / 2;
        let north_node = if square.y == 0 {
            POLE_NODE
        } else {
            latitude_edge_node(square.y).ceil() as i32
        };
        let south_node = if square.y == Z9_TILES_PER_AXIS - 1 {
            -POLE_NODE
        } else {
            latitude_edge_node(square.y + 1).floor() as i32
        };
        Self {
            north_node,
            west_node,
            rows: (north_node - south_node + 1) as u32,
            columns: (east_node - west_node + 1) as u32,
        }
    }

    pub fn cell_count(self) -> usize {
        self.rows as usize * self.columns as usize
    }

    pub fn south_node(self) -> i32 {
        self.north_node - self.rows as i32 + 1
    }

    pub fn east_node(self) -> i32 {
        self.west_node + self.columns as i32 - 1
    }

    pub fn sample_position(self, lat: f64, lon: f64) -> Option<RasterSamplePosition> {
        if !lat.is_finite() || !lon.is_finite() || !(-90.0..=90.0).contains(&lat) {
            return None;
        }
        let lon = normalize_longitude(lon);
        let source_lat = (lat.floor() as i32).min(89);
        let source_lon = lon.floor() as i32;
        // Preserve the proven source sampler's arithmetic and NN half-cell ties.
        // Adding a large global-node origin before extracting fractions loses bits.
        let source_row = (1.0 - (lat - f64::from(source_lat))) * f64::from(NODES_PER_DEGREE);
        let source_column = (lon - f64::from(source_lon)) * f64::from(NODES_PER_DEGREE);
        let row_floor = source_row.floor();
        let column_floor = source_column.floor();
        let row_offset = self.north_node - (source_lat + 1) * NODES_PER_DEGREE;
        let column_offset = source_lon * NODES_PER_DEGREE - self.west_node;
        let row = row_offset + row_floor as i32;
        let column = column_offset + column_floor as i32;
        let nearest_row = row_offset + source_row.round() as i32;
        let nearest_column = column_offset + source_column.round() as i32;
        if [row, nearest_row]
            .iter()
            .any(|r| *r < 0 || *r >= self.rows as i32)
            || [column, nearest_column]
                .iter()
                .any(|c| *c < 0 || *c >= self.columns as i32)
        {
            return None;
        }
        Some(RasterSamplePosition {
            row: row as u32,
            column: column as u32,
            row_fraction: source_row - row_floor,
            column_fraction: source_column - column_floor,
            nearest_row: nearest_row as u32,
            nearest_column: nearest_column as u32,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::square_of;

    #[test]
    fn windows_cover_native_nodes_and_extend_polar_partitions_to_ninety_degrees() {
        for (square, rows) in [
            (Square { x: 276, y: 173 }, 1627),
            (Square { x: 256, y: 256 }, 2533),
            (Square { x: 278, y: 71 }, 522),
            (Square { x: 256, y: 0 }, 18037),
            (Square { x: 256, y: 511 }, 18037),
        ] {
            let window = RasterWindow::for_square(square);
            assert_eq!((window.rows, window.columns), (rows, 2533));
        }
        for lat in [-90.0, -89.5, -85.1, 0.0, 85.1, 89.5, 90.0] {
            let square = square_of(lat, 12.0);
            let window = RasterWindow::for_square(square);
            let position = window.sample_position(lat, 12.0).unwrap();
            assert_eq!(
                window.north_node - position.nearest_row as i32,
                (lat * 3600.0).round() as i32
            );
        }
        let polar = RasterWindow::for_square(Square { x: 256, y: 0 });
        assert_eq!(polar.cell_count() * 2, 91_375_442);
        assert!(polar.sample_position(90.01, 0.0).is_none());
        assert!(polar.sample_position(f64::NAN, 0.0).is_none());
    }

    #[test]
    fn adjacent_windows_have_the_native_interpolation_overlap_without_gaps() {
        for index in 0..511 {
            let west = RasterWindow::for_square(Square { x: index, y: 100 });
            let east = RasterWindow::for_square(Square {
                x: index + 1,
                y: 100,
            });
            assert!((0..=1).contains(&(west.east_node() - east.west_node)));
            let north = RasterWindow::for_square(Square { x: 100, y: index });
            let south = RasterWindow::for_square(Square {
                x: 100,
                y: index + 1,
            });
            assert!((0..=1).contains(&(south.north_node - north.south_node())));
        }
        for lon in [-540.0, -180.0, 180.0, 540.0] {
            let square = square_of(0.25, lon);
            assert_eq!(square.x, 0);
            let window = RasterWindow::for_square(square);
            assert_eq!(window.sample_position(0.25, lon).unwrap().column, 0);
        }
    }

    #[test]
    fn sampling_preserves_source_fraction_bits_and_nearest_half_cell_choices() {
        for lat_degree in [-90, -51, -1, 0, 49, 89] {
            for lon_degree in [-180, -2, 0, 14, 179] {
                for fractional_node in [0.0, 0.499999, 0.5, 17.5, 1800.5, 3599.5] {
                    let lat = f64::from(lat_degree) + fractional_node / 3600.0;
                    let lon = f64::from(lon_degree) + fractional_node / 3600.0;
                    let window = RasterWindow::for_square(square_of(lat, lon));
                    let position = window.sample_position(lat, lon).unwrap();
                    let source_row = (1.0 - (lat - f64::from(lat_degree))) * 3600.0;
                    let source_col = (lon - f64::from(lon_degree)) * 3600.0;
                    assert_eq!(
                        position.row_fraction.to_bits(),
                        source_row.fract().to_bits()
                    );
                    assert_eq!(
                        position.column_fraction.to_bits(),
                        source_col.fract().to_bits()
                    );
                    assert_eq!(
                        window.north_node - position.nearest_row as i32,
                        (lat_degree + 1) * 3600 - source_row.round() as i32
                    );
                    assert_eq!(
                        window.west_node + position.nearest_column as i32,
                        lon_degree * 3600 + source_col.round() as i32
                    );
                }
            }
        }
    }
}
