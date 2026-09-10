//! Canonical true-edge vertices of the z13 surface painter's 16-pixel blocks.

use crate::{Square, Z13_PER_Z9_SIDE, Z9_TILES_PER_AXIS};

pub const TILE_PIXEL_SIDE: usize = 512;
pub const BLOCK_PIXEL_SIDE: usize = 16;
pub const BLOCKS_PER_TILE_SIDE: usize = TILE_PIXEL_SIDE / BLOCK_PIXEL_SIDE;
pub const CORNERS_PER_TILE_SIDE: usize = BLOCKS_PER_TILE_SIDE + 1;
pub const CORNER_COUNT: usize = CORNERS_PER_TILE_SIDE * CORNERS_PER_TILE_SIDE;
pub const BLOCKS_PER_SQUARE_SIDE: u32 = Z13_PER_Z9_SIDE * BLOCKS_PER_TILE_SIDE as u32;
pub const WORLD_BLOCK_SIDE: u32 = Z9_TILES_PER_AXIS as u32 * BLOCKS_PER_SQUARE_SIDE;

/// All true-edge vertices used by one valid z13 tile, in row-major order.
pub fn tile_corners(tile_x: u32, tile_y: u32) -> Option<Vec<SurfaceCorner>> {
    let tile_side = WORLD_BLOCK_SIDE / BLOCKS_PER_TILE_SIDE as u32;
    if tile_x >= tile_side || tile_y >= tile_side {
        return None;
    }
    Some(
        (0..CORNERS_PER_TILE_SIDE)
            .flat_map(|row| {
                (0..CORNERS_PER_TILE_SIDE).map(move |column| {
                    SurfaceCorner::for_tile(tile_x, tile_y, column, row)
                        .expect("validated tile coordinates")
                })
            })
            .collect(),
    )
}

/// Canonical vertices shared across this z9 owner's north/west boundaries.
/// The last world row stays with y=511, while longitude wraps at x=0.
pub fn owner_edge_corners(owner: Square) -> Vec<SurfaceCorner> {
    let side = BLOCKS_PER_SQUARE_SIDE;
    let x0 = u32::from(owner.x) * side;
    let y0 = u32::from(owner.y) * side;
    let mut result = Vec::with_capacity(if owner.y == 0 {
        512
    } else if owner.y == 511 {
        1024
    } else {
        1023
    });
    let west_end = y0 + side + u32::from(owner.y == Z9_TILES_PER_AXIS - 1);
    result.extend((y0..west_end).map(|y| SurfaceCorner::new(x0, y).unwrap()));
    if owner.y != 0 {
        result.extend((x0 + 1..x0 + side).map(|x| SurfaceCorner::new(x, y0).unwrap()));
    }
    result.sort_unstable();
    debug_assert!(result.iter().all(|corner| corner.owner() == owner));
    result
}

/// Canonical edge-bundle owners needed by all tiles in one z9 output owner.
pub fn owner_dependency_owners(owner: Square) -> Vec<Square> {
    let ((x0, x1), (y0, y1)) = crate::owned_z13(owner);
    let mut owners = Vec::new();
    for y in y0..y1 {
        for x in x0..x1 {
            for dependency in tile_corners(x, y)
                .expect("owned z13 tile")
                .into_iter()
                .map(SurfaceCorner::owner)
            {
                if !owners.contains(&dependency) {
                    owners.push(dependency);
                }
            }
        }
    }
    owners.sort_unstable_by_key(|square| (square.x, square.y));
    owners
}

/// Four true-edge vertices and linear-power interpolation weights at a representable receiver.
pub struct SurfaceCornerInterpolation {
    pub corners: [SurfaceCorner; 4],
    pub weights: [f64; 4],
}

impl SurfaceCornerInterpolation {
    pub fn at(latitude: f64, longitude: f64) -> Option<Self> {
        if !latitude.is_finite()
            || !longitude.is_finite()
            || latitude.abs() > crate::MAX_MERCATOR_LAT_DEG
        {
            return None;
        }
        let side = f64::from(WORLD_BLOCK_SIDE);
        let x =
            ((crate::geo::normalize_longitude(longitude) + 180.0) / 360.0 * side).rem_euclid(side);
        let y = (0.5 * (1.0 - latitude.to_radians().sin().atanh() / std::f64::consts::PI) * side)
            .clamp(0.0, side);
        let column = x.floor() as u32;
        let row = (y.floor() as u32).min(WORLD_BLOCK_SIDE - 1);
        let horizontal = x - f64::from(column);
        let vertical = y - f64::from(row);
        Some(Self {
            corners: [
                SurfaceCorner::new(column, row)?,
                SurfaceCorner::new(column + 1, row)?,
                SurfaceCorner::new(column, row + 1)?,
                SurfaceCorner::new(column + 1, row + 1)?,
            ],
            weights: [
                (1.0 - horizontal) * (1.0 - vertical),
                horizontal * (1.0 - vertical),
                (1.0 - horizontal) * vertical,
                horizontal * vertical,
            ],
        })
    }
}

/// Longitude wraps; the south world edge remains a distinct final row.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct SurfaceCorner {
    x: u32,
    y: u32,
}

impl SurfaceCorner {
    pub fn new(x: u32, y: u32) -> Option<Self> {
        (y <= WORLD_BLOCK_SIDE).then_some(Self {
            x: x % WORLD_BLOCK_SIDE,
            y,
        })
    }

    pub fn for_tile(tile_x: u32, tile_y: u32, column: usize, row: usize) -> Option<Self> {
        let tile_side = WORLD_BLOCK_SIDE / BLOCKS_PER_TILE_SIDE as u32;
        if tile_x >= tile_side
            || tile_y >= tile_side
            || column >= CORNERS_PER_TILE_SIDE
            || row >= CORNERS_PER_TILE_SIDE
        {
            return None;
        }
        Self::new(
            tile_x * BLOCKS_PER_TILE_SIDE as u32 + column as u32,
            tile_y * BLOCKS_PER_TILE_SIDE as u32 + row as u32,
        )
    }

    pub fn coordinates(self) -> [u32; 2] {
        [self.x, self.y]
    }

    pub fn owner(self) -> Square {
        Square {
            x: (self.x / BLOCKS_PER_SQUARE_SIDE) as u16,
            y: (self.y / BLOCKS_PER_SQUARE_SIDE).min(u32::from(Z9_TILES_PER_AXIS - 1)) as u16,
        }
    }

    /// Every z13 tile whose true block-edge lattice contains this vertex.
    pub fn dependent_tiles(self) -> Vec<(u32, u32)> {
        let blocks = BLOCKS_PER_TILE_SIDE as u32;
        let tile_side = WORLD_BLOCK_SIDE / blocks;
        let mut xs = vec![self.x / blocks];
        if self.x.is_multiple_of(blocks) {
            xs.push((self.x / blocks + tile_side - 1) % tile_side);
        }
        let mut ys = Vec::new();
        if self.y < WORLD_BLOCK_SIDE {
            ys.push(self.y / blocks);
        }
        if self.y > 0 && self.y.is_multiple_of(blocks) {
            ys.push(self.y / blocks - 1);
        }
        let mut tiles: Vec<_> = xs
            .into_iter()
            .flat_map(|x| ys.iter().map(move |&y| (x, y)))
            .collect();
        tiles.sort_unstable();
        tiles.dedup();
        tiles
    }

    pub fn dependency_mask(self) -> u8 {
        (1 << self.dependent_tiles().len()) - 1
    }

    pub fn consumer_bit(self, tile_x: u32, tile_y: u32) -> Option<u8> {
        self.dependent_tiles()
            .iter()
            .position(|&tile| tile == (tile_x, tile_y))
            .map(|index| 1 << index)
    }

    /// Exact z30 coordinates: source coordinates increase northwards.
    pub fn grid_coordinates(self) -> [i32; 2] {
        let scale = (1_u32 << 30) / WORLD_BLOCK_SIDE;
        [
            (self.x * scale) as i32,
            ((WORLD_BLOCK_SIDE - self.y) * scale) as i32,
        ]
    }

    pub fn latitude_longitude(self) -> [f64; 2] {
        let latitude = (std::f64::consts::PI
            * (1.0 - 2.0 * f64::from(self.y) / f64::from(WORLD_BLOCK_SIDE)))
        .sinh()
        .atan()
        .to_degrees();
        let longitude = f64::from(self.x) / f64::from(WORLD_BLOCK_SIDE) * 360.0 - 180.0;
        [latitude, longitude]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn owner_edges_have_exact_middle_pole_and_dateline_sets() {
        let north = owner_edge_corners(Square { x: 0, y: 0 });
        let middle = owner_edge_corners(Square { x: 276, y: 173 });
        let south = owner_edge_corners(Square { x: 511, y: 511 });
        assert_eq!((north.len(), middle.len(), south.len()), (512, 1023, 1024));
        assert!(north
            .iter()
            .all(|corner| corner.owner() == Square { x: 0, y: 0 }));
        assert!(middle
            .iter()
            .all(|corner| corner.owner() == Square { x: 276, y: 173 }));
        assert!(south
            .iter()
            .all(|corner| corner.owner() == Square { x: 511, y: 511 }));
        assert!(south
            .iter()
            .any(|corner| corner.coordinates()[1] == WORLD_BLOCK_SIDE));
        let dateline = owner_edge_corners(Square { x: 0, y: 173 });
        assert!(dateline.iter().any(|corner| corner.coordinates()[0] == 0
            && corner.dependent_tiles().iter().any(|(x, _)| *x == 8191)));
    }

    #[test]
    fn owner_dependencies_cover_exact_foreign_edges() {
        assert_eq!(
            owner_dependency_owners(Square { x: 276, y: 173 }),
            vec![
                Square { x: 276, y: 173 },
                Square { x: 276, y: 174 },
                Square { x: 277, y: 173 },
                Square { x: 277, y: 174 },
            ]
        );
        assert_eq!(
            owner_dependency_owners(Square { x: 511, y: 511 }),
            vec![Square { x: 0, y: 511 }, Square { x: 511, y: 511 }]
        );
    }

    #[test]
    fn interpolation_preserves_last_pole_row_and_rejects_unrepresentable_points() {
        let south = SurfaceCornerInterpolation::at(-crate::MAX_MERCATOR_LAT_DEG, 180.0).unwrap();
        assert_eq!(south.corners[2].coordinates(), [0, WORLD_BLOCK_SIDE]);
        assert_eq!(south.corners[2].owner(), Square { x: 0, y: 511 });
        assert!(south.weights[2] > 1.0 - 1e-8);
        assert!(SurfaceCornerInterpolation::at(crate::MAX_MERCATOR_LAT_DEG + 1e-9, 0.0).is_none());
        assert!(SurfaceCornerInterpolation::at(-90.0, 0.0).is_none());
        assert!(SurfaceCornerInterpolation::at(0.0, f64::INFINITY).is_none());
    }

    #[test]
    fn adjacent_tiles_share_true_edges_including_wrap_and_south_row() {
        assert_eq!(tile_corners(200, 100).unwrap().len(), CORNER_COUNT);
        assert!(tile_corners(8192, 0).is_none());
        let mut corners = BTreeSet::new();
        for y in 100..102 {
            for x in 200..202 {
                for row in 0..CORNERS_PER_TILE_SIDE {
                    for col in 0..CORNERS_PER_TILE_SIDE {
                        corners.insert(SurfaceCorner::for_tile(x, y, col, row).unwrap());
                    }
                }
            }
        }
        assert_eq!(corners.len(), 65 * 65);
        let west = SurfaceCorner::for_tile(0, 8191, 0, 32).unwrap();
        let east = SurfaceCorner::for_tile(8191, 8191, 32, 32).unwrap();
        assert_eq!(west, east);
        assert_eq!(west.owner(), Square { x: 0, y: 511 });
        assert_eq!(west.grid_coordinates(), [0, 0]);
        let edge = SurfaceCorner::for_tile(15, 15, 32, 32).unwrap();
        assert_eq!(edge.owner(), Square { x: 1, y: 1 });
        assert_eq!(edge.grid_coordinates(), [1 << 21, (1 << 30) - (1 << 21)]);
        assert!(SurfaceCorner::for_tile(8192, 0, 0, 0).is_none());
        assert!(SurfaceCorner::new(0, WORLD_BLOCK_SIDE + 1).is_none());
    }
}
