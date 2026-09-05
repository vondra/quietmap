//! Integer z30 rings and polygon topology for prepared geometry.

use super::{grid_to_meters, WEB_MERCATOR_RADIUS_M};
use std::f64::consts::PI;

/// Legacy WKB footprint floor: keeps area-source emission and splitting finite.
pub const MIN_FOOTPRINT_AREA_M2: f64 = 1.0;

/// One snapped ring: z30 cells in lon/lat order (closed or not).
pub type GridRing = Vec<(i32, i32)>;

/// Every polygon's rings, exterior first followed by its holes.
pub type GridPolygons = Vec<Vec<GridRing>>;

/// Polygon count, then each ring count and existing ring encoding, all LE.
pub fn encode_grid_polygons(polygons: &[Vec<GridRing>]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&(polygons.len() as u32).to_le_bytes());
    for rings in polygons {
        bytes.extend_from_slice(&(rings.len() as u32).to_le_bytes());
        for ring in rings {
            bytes.extend_from_slice(&encode_grid_poly(ring));
        }
    }
    bytes
}

/// Reject incomplete topology before allocating or returning any of its parts.
pub fn decode_grid_polygons(mut bytes: &[u8]) -> Option<GridPolygons> {
    fn count(bytes: &mut &[u8]) -> Option<usize> {
        let value = u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?) as usize;
        *bytes = &bytes[4..];
        Some(value)
    }
    let polygon_count = count(&mut bytes)?;
    if polygon_count == 0 || polygon_count > bytes.len() / 4 {
        return None;
    }
    let mut polygons = Vec::with_capacity(polygon_count);
    for _ in 0..polygon_count {
        let ring_count = count(&mut bytes)?;
        if ring_count == 0 || ring_count > bytes.len() / 4 {
            return None;
        }
        let mut rings = Vec::with_capacity(ring_count);
        for _ in 0..ring_count {
            let mut points = bytes;
            let point_count = count(&mut points)?;
            if point_count < 3 || point_count > points.len() / 8 {
                return None;
            }
            let ring_bytes = 4 + point_count * 8;
            rings.push(decode_grid_poly(&bytes[..ring_bytes])?);
            bytes = &bytes[ring_bytes..];
        }
        polygons.push(rings);
    }
    bytes.is_empty().then_some(polygons)
}

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

/// True if the grid cell is inside the ring (outer ring; the extract stores
/// no holes — relation assembly keeps outers only, as before).
pub fn ring_contains(ring: &[(i32, i32)], gx: i32, gy: i32) -> bool {
    if ring.len() < 3 {
        return false;
    }
    let (px, py) = grid_to_meters(gx, gy);
    let pts = ring_meters(ring);
    let n = pts.len();
    // Cheap metre-bbox reject before the ray cast.
    let (mut min_x, mut max_x) = (f64::MAX, f64::MIN);
    let (mut min_y, mut max_y) = (f64::MAX, f64::MIN);
    for &(x, y) in &pts {
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    if px < min_x || px > max_x || py < min_y || py > max_y {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = pts[i];
        let (xj, yj) = pts[j];
        if ((yi > py) != (yj > py)) && (px < (xj - xi) * (py - yi) / (yj - yi) + xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Ring envelope as `[min_lat, min_lon, max_lat, max_lon]` degrees for the
/// batch-prune metadata. `None` on an empty ring.
pub fn ring_bbox_lonlat(ring: &[(i32, i32)]) -> Option<[f64; 4]> {
    if ring.is_empty() {
        return None;
    }
    let (mut min_lat, mut max_lat) = (f64::MAX, f64::MIN);
    let (mut min_lon, mut max_lon) = (f64::MAX, f64::MIN);
    for &(gx, gy) in ring {
        let (x, y) = grid_to_meters(gx, gy);
        let (lon, lat) = meters_to_lonlat(x, y);
        min_lat = min_lat.min(lat);
        max_lat = max_lat.max(lat);
        min_lon = min_lon.min(lon);
        max_lon = max_lon.max(lon);
    }
    Some([min_lat, min_lon, max_lat, max_lon])
}

/// Snap lon/lat degrees straight to a grid cell.
pub fn snap_lonlat(lon_deg: f64, lat_deg: f64) -> (i32, i32) {
    super::lonlat_to_grid(lon_deg, lat_deg)
}

#[cfg(test)]
mod tests {
    use super::super::lonlat_to_grid;
    use super::*;
    use crate::GRID_QUANTUM_M;

    /// ~100×100 m square at Prague as a snapped ring.
    fn prague_square_ring() -> GridRing {
        let corners = [
            (14.0, 50.0),
            (14.001_394, 50.0),
            (14.001_394, 50.000_904),
            (14.0, 50.000_904),
        ];
        corners
            .iter()
            .map(|&(lon, lat)| lonlat_to_grid(lon, lat))
            .collect()
    }

    #[test]
    fn encode_decode_roundtrip() {
        let ring = prague_square_ring();
        let bytes = encode_grid_poly(&ring);
        assert_eq!(bytes.len(), 4 + ring.len() * 8);
        assert_eq!(decode_grid_poly(&bytes), Some(ring));
        assert_eq!(decode_grid_poly(&bytes[..7]), None);
        assert_eq!(decode_grid_poly(&[]), None);
    }

    #[test]
    fn polygon_topology_is_complete_or_rejected() {
        let ring = prague_square_ring();
        let polygons = vec![vec![ring.clone(), ring.clone()], vec![ring]];
        let bytes = encode_grid_polygons(&polygons);
        assert_eq!(decode_grid_polygons(&bytes), Some(polygons));
        for truncated in 0..bytes.len() {
            assert!(decode_grid_polygons(&bytes[..truncated]).is_none());
        }
        let mut trailing = bytes;
        trailing.push(0);
        assert!(decode_grid_polygons(&trailing).is_none());
        for malformed in [vec![0; 4], u32::MAX.to_le_bytes().to_vec()] {
            assert!(decode_grid_polygons(&malformed).is_none());
        }
    }

    #[test]
    fn area_matches_100x100m() {
        let area = ring_area_m2(&prague_square_ring()).unwrap();
        assert!(
            (9_000.0..11_000.0).contains(&area),
            "100x100 m square → ~10 000 m², got {area}"
        );
    }

    #[test]
    fn contains_inside_outside() {
        let ring = prague_square_ring();
        let (ix, iy) = lonlat_to_grid(14.000_7, 50.000_45);
        assert!(ring_contains(&ring, ix, iy));
        let (ox, oy) = lonlat_to_grid(14.005, 50.005);
        assert!(!ring_contains(&ring, ox, oy));
    }

    #[test]
    fn bbox_covers_ring() {
        let ring = prague_square_ring();
        let bb = ring_bbox_lonlat(&ring).unwrap();
        // Snapped corners floor to cell origins, so edges may sit up to one
        // quantum inside the true corners.
        let q_deg = GRID_QUANTUM_M / 111_320.0 + 1e-9;
        assert!(bb[0] <= 50.0 + q_deg && bb[2] >= 50.000_904 - q_deg);
        assert!(bb[1] <= 14.0 + q_deg && bb[3] >= 14.001_394 - q_deg);
        assert!(ring_bbox_lonlat(&[]).is_none());
    }

    #[test]
    fn degenerate_ring_has_no_area_or_inside() {
        let line = vec![(0, 0), (100, 100)];
        assert!(ring_area_m2(&line).is_none());
        assert!(!ring_contains(&line, 50, 50));
    }
}
