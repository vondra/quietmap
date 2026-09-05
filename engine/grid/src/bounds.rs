//! Closed geographic bounds to unique z9 owners, including wrapped longitude and poles.

use crate::{square_of, Square, Z9_SPAN_DEG, Z9_TILES_PER_AXIS};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoundedSquares {
    first_column: u16,
    columns: u16,
    north_row: u16,
    south_row: u16,
}

impl BoundedSquares {
    /// West/east are an unwrapped interval: [179, 181] crosses the dateline.
    /// Bounds include their endpoints; each point still has one canonical owner.
    pub fn from_degrees(south: f64, west: f64, north: f64, east: f64) -> Option<Self> {
        if ![south, west, north, east].into_iter().all(f64::is_finite)
            || south > north
            || west > east
            || south > 90.0
            || north < -90.0
        {
            return None;
        }
        let width = east - west;
        let first_column = square_of(0.0, west).x;
        let last_column = square_of(0.0, east).x;
        let columns = (last_column + Z9_TILES_PER_AXIS - first_column) % Z9_TILES_PER_AXIS + 1;
        // A nearly full revolution can end in the starting cell. Its short
        // modular interval does not cover the supplied geographic width.
        let columns = if width > f64::from(columns) * Z9_SPAN_DEG || width >= 360.0 {
            Z9_TILES_PER_AXIS
        } else {
            columns
        };
        Some(Self {
            first_column,
            columns,
            north_row: square_of(north.min(90.0), 0.0).y,
            south_row: square_of(south.max(-90.0), 0.0).y,
        })
    }

    pub fn cell_count(self) -> usize {
        usize::from(self.columns) * usize::from(self.south_row - self.north_row + 1)
    }

    pub fn contains(self, square: Square) -> bool {
        square.x < Z9_TILES_PER_AXIS
            && square.y >= self.north_row
            && square.y <= self.south_row
            && (square.x + Z9_TILES_PER_AXIS - self.first_column) % Z9_TILES_PER_AXIS < self.columns
    }

    pub fn iter(self) -> impl Iterator<Item = Square> {
        (self.north_row..=self.south_row).flat_map(move |y| {
            (0..self.columns).map(move |offset| Square {
                x: (self.first_column + offset) % Z9_TILES_PER_AXIS,
                y,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn closed_wrapped_bounds_have_unique_owners() {
        for (south, north) in [(-90.0, -89.0), (-80.0, 80.0), (0.0, 0.0), (89.0, 90.0)] {
            for (west, east) in [
                (-180.0, 180.0),
                (179.9, 180.1),
                (-540.1, -539.9),
                (0.0, 0.703125),
            ] {
                let region = BoundedSquares::from_degrees(south, west, north, east).unwrap();
                let cells: HashSet<_> = region.iter().collect();
                assert_eq!(region.cell_count(), cells.len());
                for lat in [south, (south + north) / 2.0, north] {
                    for lon in [west, (west + east) / 2.0, east] {
                        assert!(
                            region.contains(square_of(lat, lon)),
                            "{lat},{lon}: {region:?}"
                        );
                    }
                }
            }
        }
        assert!(BoundedSquares::from_degrees(0.0, 1.0, 0.0, 0.0).is_none());
        assert!(BoundedSquares::from_degrees(f64::NAN, 0.0, 1.0, 1.0).is_none());
        for column in 0..=Z9_TILES_PER_AXIS {
            let edge = f64::from(column) * Z9_SPAN_DEG - 180.0;
            for edge in [edge.next_down(), edge, edge.next_up()] {
                for shift in [-360.0, 0.0, 360.0] {
                    let east = edge + shift;
                    for width in [0.0, 0.1, 360.0 - 0.1] {
                        let west = east - width;
                        let region = BoundedSquares::from_degrees(0.0, west, 0.0, east).unwrap();
                        for lon in [west, east] {
                            assert!(
                                region.contains(square_of(0.0, lon)),
                                "closed boundary {west}..{east}: {region:?}, lon={lon}"
                            );
                        }
                    }
                }
            }
        }
    }
}
