//! Globally shared Mercator cruise interpolation nodes, periodic in longitude.
use anyhow::{ensure, Result};

// The existing cruise field's global z9/512 receiver lattice. Its secondary
// kilometre-scale interpolation does not resolve finite-segment endpoint gradients.
pub(super) struct Lattice {
    pub(super) axis: i64,
    pub(super) x0: i64,
    pub(super) y0: i64,
    pub(super) width: usize,
    pub(super) height: usize,
}
impl Lattice {
    pub(super) fn new(bounds: [f64; 4]) -> Self {
        let axis =
            i64::from(grid::Z9_TILES_PER_AXIS) * grid::surface_corner::TILE_PIXEL_SIDE as i64;
        let [south, west, north, east] = bounds;
        let x0 = Self::world_x(axis, west).floor() as i64;
        let x1 = Self::world_x(axis, east).ceil() as i64;
        let y0 = Self::world_y(axis, north).floor().max(0.0) as i64;
        let y1 = Self::world_y(axis, south).ceil().min(axis as f64) as i64;
        Self {
            axis,
            x0,
            y0,
            width: (x1 - x0 + 1).max(2) as usize,
            height: (y1 - y0 + 1).max(2) as usize,
        }
    }
    fn world_x(axis: i64, lon: f64) -> f64 {
        (lon + 180.0) / 360.0 * axis as f64
    }
    fn world_y(axis: i64, lat: f64) -> f64 {
        let lat = lat
            .clamp(-grid::MAX_MERCATOR_LAT_DEG, grid::MAX_MERCATOR_LAT_DEG)
            .to_radians();
        (1.0 - lat.tan().asinh() / std::f64::consts::PI) * 0.5 * axis as f64
    }
    pub(super) fn point(&self, x: usize, y: usize) -> [f64; 2] {
        let lon = grid::geo::normalize_longitude(
            (self.x0 + x as i64) as f64 / self.axis as f64 * 360.0 - 180.0,
        );
        let lat = (std::f64::consts::PI
            * (1.0 - 2.0 * (self.y0 + y as i64) as f64 / self.axis as f64))
            .sinh()
            .atan()
            .to_degrees();
        [lat, lon]
    }
    pub(super) fn bracket(&self, lat: f64, lon: f64) -> Result<(usize, usize, f64, f64)> {
        let middle =
            (self.x0 as f64 + (self.width - 1) as f64 * 0.5) / self.axis as f64 * 360.0 - 180.0;
        let lon = middle + grid::geo::wrapped_longitude_delta(middle, lon);
        let x = Self::world_x(self.axis, lon) - self.x0 as f64;
        let y = Self::world_y(self.axis, lat) - self.y0 as f64;
        ensure!(
            x >= 0.0 && y >= 0.0 && x <= (self.width - 1) as f64 && y <= (self.height - 1) as f64,
            "facade receiver outside prepared cruise field"
        );
        let i = (x.floor() as usize).min(self.width - 2);
        let j = (y.floor() as usize).min(self.height - 2);
        Ok((i, j, x - i as f64, y - j as f64))
    }
    pub(super) fn blend(&self, values: &[[f64; 3]], b: (usize, usize, f64, f64)) -> [f64; 3] {
        let (x, y, fx, fy) = b;
        let i = y * self.width + x;
        std::array::from_fn(|p| {
            let top = values[i][p] * (1.0 - fx) + values[i + 1][p] * fx;
            let bottom =
                values[i + self.width][p] * (1.0 - fx) + values[i + self.width + 1][p] * fx;
            top * (1.0 - fy) + bottom * fy
        })
    }
}
