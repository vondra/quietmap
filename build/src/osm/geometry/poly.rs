//! Integer z30 rings and polygon topology for prepared geometry.

use super::{WEB_MERCATOR_RADIUS_M, grid_to_meters};
use std::f64::consts::PI;

/// Legacy WKB footprint floor: keeps area-source emission and splitting finite.
pub const MIN_FOOTPRINT_AREA_M2: f64 = 1.0;

/// One snapped ring: z30 cells in lon/lat order (closed or not).
#[cfg(test)]
pub type GridRing = Vec<(i32, i32)>;

/// Encode a ring to the `geom` column form.
pub fn encode_grid_poly(ring: &[(i32, i32)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + ring.len() * 8);
    out.extend_from_slice(&(ring.len() as u32).to_le_bytes());
    for &(gx, gy) in ring {
        out.extend_from_slice(&gx.to_le_bytes());
        out.extend_from_slice(&gy.to_le_bytes());
    }
    out
}

/// Decode a `geom` column value. `None` on truncation (caller stores null).
#[cfg(test)]
pub fn decode_grid_poly(bytes: &[u8]) -> Option<GridRing> {
    if bytes.len() < 4 {
        return None;
    }
    let n = u32::from_le_bytes(bytes[0..4].try_into().ok()?) as usize;
    // Two points = a wall segment; rings need three (area/contains guard that).
    if n < 2 || bytes.len() != 4 + n * 8 {
        return None;
    }
    let mut ring = Vec::with_capacity(n);
    for i in 0..n {
        let o = 4 + i * 8;
        ring.push((
            i32::from_le_bytes(bytes[o..o + 4].try_into().ok()?),
            i32::from_le_bytes(bytes[o + 4..o + 8].try_into().ok()?),
        ));
    }
    Some(ring)
}

/// 3857 metres to lon/lat degrees (inverse Mercator).
pub fn meters_to_lonlat(x_m: f64, y_m: f64) -> (f64, f64) {
    let lon = (x_m / WEB_MERCATOR_RADIUS_M).to_degrees();
    let lat = (2.0 * (y_m / WEB_MERCATOR_RADIUS_M).exp().atan() - PI / 2.0).to_degrees();
    (lon, lat)
}

/// Ring vertices in 3857 metres.
fn ring_meters(ring: &[(i32, i32)]) -> Vec<(f64, f64)> {
    ring.iter()
        .map(|&(gx, gy)| grid_to_meters(gx, gy))
        .collect()
}

/// Shoelace area in m²: the grid is uniform in projected metres, so the
/// projected shoelace times cos²(latitude) is the ground area — the same
/// quantity the float-WKB area computed, matched cell for cell.
/// Minimum 1.0 m² (a degenerate ring is a point source, never zero-area).
pub fn ring_area_m2(ring: &[(i32, i32)]) -> Option<f64> {
    if ring.len() < 3 {
        return None;
    }
    let pts = ring_meters(ring);
    let mut mean_y = 0.0;
    for &(_, y) in &pts {
        mean_y += y;
    }
    let mean_lat = meters_to_lonlat(0.0, mean_y / pts.len() as f64).1;
    let cos_lat = mean_lat.to_radians().cos();
    let n = pts.len();
    let mut shoelace = 0.0f64;
    for i in 0..n {
        let j = (i + 1) % n;
        shoelace += pts[i].0 * pts[j].1 - pts[j].0 * pts[i].1;
    }
    Some(((shoelace / 2.0).abs() * cos_lat * cos_lat).max(MIN_FOOTPRINT_AREA_M2))
}

/// Metre vertices and envelope shared by repeated containment queries.
pub struct PreparedRing {
    points: Vec<(f64, f64)>,
    bbox: [f64; 4],
}

impl PreparedRing {
    pub fn new(ring: &[(i32, i32)]) -> Option<Self> {
        if ring.len() < 3 {
            return None;
        }
        let points = ring_meters(ring);
        let (mut min_x, mut max_x) = (f64::MAX, f64::MIN);
        let (mut min_y, mut max_y) = (f64::MAX, f64::MIN);
        for &(x, y) in &points {
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_y = min_y.min(y);
            max_y = max_y.max(y);
        }
        Some(Self {
            points,
            bbox: [min_x, min_y, max_x, max_y],
        })
    }

    pub fn bounds(&self) -> [f64; 4] {
        self.bbox
    }

    pub fn contains(&self, gx: i32, gy: i32) -> bool {
        let (px, py) = grid_to_meters(gx, gy);
        let [min_x, min_y, max_x, max_y] = self.bbox;
        if px < min_x || px > max_x || py < min_y || py > max_y {
            return false;
        }
        let mut inside = false;
        let mut j = self.points.len() - 1;
        for i in 0..self.points.len() {
            let (xi, yi) = self.points[i];
            let (xj, yj) = self.points[j];
            if ((yi > py) != (yj > py)) && (px < (xj - xi) * (py - yi) / (yj - yi) + xi) {
                inside = !inside;
            }
            j = i;
        }
        inside
    }
}
