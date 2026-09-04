//! z9 square grid: unit naming, halo ring, integer coordinates, owned z13 tiles.
//!
//! The compute unit is the Web-Mercator z9 tile — never H3. Vectors on disk use
//! global int32 z30-pixel coordinates; use i64 for their differences.
//!
//! Map of submodules: [`poly`] (integer footprint rings), [`geo`] (flat-earth
//! propagation math).

pub mod geo;
pub mod poly;

use std::f64::consts::PI;

/// EPSG:3857 sphere radius, metres.
pub const WEB_MERCATOR_RADIUS_M: f64 = 6_378_137.0;
/// Full equatorial circumference, metres: 2·π·R.
pub const EARTH_CIRCUMFERENCE_M: f64 = 40_075_016.685_578_49;
/// z9 tile span, degrees: 360/512. Tile edges land exactly on z13 edges.
pub const Z9_SPAN_DEG: f64 = 0.703_125;
/// z9 tiles per axis.
pub const Z9_TILES_PER_AXIS: u16 = 512;
/// z13 children per z9 side.
pub const Z13_PER_Z9_SIDE: u32 = 16;
/// Coordinate quantum, metres: circumference/2^30. Max global coordinate
/// 2^30 < i32::MAX, so z30 is the finest grid a global int32 holds.
pub const GRID_QUANTUM_M: f64 = 0.037_322_767_717_044_72;
/// Longest source reach, km: rail (plan §3). Sizes the halo ring.
pub const MAX_HALO_KM: f64 = 11.0;
/// Web-Mercator latitude limit, degrees.
pub const MAX_MERCATOR_LAT_DEG: f64 = 85.051_128_78;
/// Latitude (either sign) above which one ring no longer covers [`MAX_HALO_KM`]:
/// 78.273·cos(φ) ≥ 11 → φ ≤ 81.9°. Covers Longyearbyen (78.2° N).
pub const WIDE_RING_LAT_DEG: f64 = 81.9;

/// A z9 unit: slippy `x`/`y` in `0..512`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Square {
    pub x: u16,
    pub y: u16,
}

/// Project lon/lat degrees to EPSG:3857 metres. Latitude clamps to the
/// Mercator limit so poles map to the edge row, never to infinity.
pub fn lonlat_to_meters(lon_deg: f64, lat_deg: f64) -> (f64, f64) {
    let lat = lat_deg.clamp(-MAX_MERCATOR_LAT_DEG, MAX_MERCATOR_LAT_DEG);
    let x = WEB_MERCATOR_RADIUS_M * lon_deg.to_radians();
    let y = WEB_MERCATOR_RADIUS_M * ((PI / 4.0 + lat.to_radians() / 2.0).tan().ln());
    (x, y)
}

/// Snap 3857 metres to the global int32 z30 grid. i64 holds the quotient
/// exactly (2^30 steps); the cast is safe because |coord| < 2^30 < i32::MAX.
pub fn meters_to_grid(x_m: f64, y_m: f64) -> (i32, i32) {
    let gx = (x_m / GRID_QUANTUM_M).floor() as i64 + (1 << 29);
    let gy = (y_m / GRID_QUANTUM_M).floor() as i64 + (1 << 29);
    (gx as i32, gy as i32)
}

/// Project lon/lat degrees straight to grid cells.
pub fn lonlat_to_grid(lon_deg: f64, lat_deg: f64) -> (i32, i32) {
    let (x, y) = lonlat_to_meters(lon_deg, lat_deg);
    meters_to_grid(x, y)
}

/// Grid cell corner back to 3857 metres.
pub fn grid_to_meters(gx: i32, gy: i32) -> (f64, f64) {
    (
        (gx as i64 - (1 << 29)) as f64 * GRID_QUANTUM_M,
        (gy as i64 - (1 << 29)) as f64 * GRID_QUANTUM_M,
    )
}

/// z9 unit containing lon/lat. Longitude wraps at the antimeridian;
/// latitude clamps with the projection.
pub fn square_of(lat_deg: f64, lon_deg: f64) -> Square {
    let wrapped = geo::normalize_longitude(lon_deg);
    let x = ((wrapped + 180.0) / 360.0 * f64::from(Z9_TILES_PER_AXIS)) as u16;
    let (_, y_m) = lonlat_to_meters(0.0, lat_deg);
    let half = EARTH_CIRCUMFERENCE_M / 2.0;
    let y = ((half - y_m) / EARTH_CIRCUMFERENCE_M * f64::from(Z9_TILES_PER_AXIS)) as u16;
    Square {
        x: x.min(Z9_TILES_PER_AXIS - 1),
        y: y.min(Z9_TILES_PER_AXIS - 1),
    }
}

/// Canonical unit name, e.g. `z9/276/173`.
pub fn square_name(square: Square) -> String {
    format!("z9/{}/{}", square.x, square.y)
}

/// Parse [`square_name`] back. Rejects anything else — one naming, no aliases.
pub fn parse_square_name(name: &str) -> Option<Square> {
    let rest = name.strip_prefix("z9/")?;
    let (x, y) = rest.split_once('/')?;
    Some(Square {
        x: x.parse().ok()?,
        y: y.parse().ok()?,
    })
}

/// Bits per z9 axis: 512 tiles per axis, so x/y are 9-bit values and the
/// Morton id fits in 18 bits.
const SQUARE_BITS_PER_AXIS: u32 = 9;

/// Largest valid z-order square id: both axes at 511 interleave to 18 one-bits.
pub const MAX_SQUARE_ID: i64 = (1 << (2 * SQUARE_BITS_PER_AXIS)) - 1;

/// Morton z-order id of a z9 square: bit `i` of `x` goes to bit `2i`, bit `i`
/// of `y` to bit `2i + 1`. THE integer square id — the prepared admin tree
/// is keyed by it. (Spill/sort partitioning inside osm-extract is a different,
/// row-major transient key and must not share this name.)
pub fn square_id(square: Square) -> i64 {
    let mut id: i64 = 0;
    for i in 0..SQUARE_BITS_PER_AXIS {
        id |= (((u32::from(square.x) >> i) & 1) as i64) << (2 * i);
        id |= (((u32::from(square.y) >> i) & 1) as i64) << (2 * i + 1);
    }
    id
}

/// Inverse of [`square_id`]: `None` when the id is not a z9 Morton code
/// (negative or past [`MAX_SQUARE_ID`]).
pub fn square_from_id(id: i64) -> Option<Square> {
    if !(0..=MAX_SQUARE_ID).contains(&id) {
        return None;
    }
    let mut x: u16 = 0;
    let mut y: u16 = 0;
    for i in 0..SQUARE_BITS_PER_AXIS {
        x |= (((id >> (2 * i)) & 1) as u16) << i;
        y |= (((id >> (2 * i + 1)) & 1) as u16) << i;
    }
    Some(Square { x, y })
}

/// Halo ring radius in units for a latitude: 1 square to
/// [`WIDE_RING_LAT_DEG`], 2 beyond (north-Greenland class, still computed).
pub fn ring_radius(lat_deg: f64) -> u32 {
    if lat_deg.abs() <= WIDE_RING_LAT_DEG {
        1
    } else {
        2
    }
}

/// Own square plus ring, in row-major order. x wraps at the antimeridian;
/// y clamps at the poles (no neighbour beyond the edge row).
pub fn ring_squares(center: Square, lat_deg: f64) -> Vec<Square> {
    let radius = i32::try_from(ring_radius(lat_deg)).unwrap_or(1);
    let axis = i32::from(Z9_TILES_PER_AXIS);
    let mut out = Vec::with_capacity((2 * radius + 1) as usize * (2 * radius + 1) as usize);
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            let x = (i32::from(center.x) + dx).rem_euclid(axis) as u16;
            let y = (i32::from(center.y) + dy).clamp(0, axis - 1) as u16;
            let square = Square { x, y };
            // The pole rows clamp, so edge rings would repeat units.
            if !out.contains(&square) {
                out.push(square);
            }
        }
    }
    out
}

/// z13 child ranges `[x0, x1) × [y0, y1)` owned by a unit. Edges align
/// exactly — no partial tiles, no centre rule.
pub fn owned_z13(square: Square) -> ((u32, u32), (u32, u32)) {
    let x0 = u32::from(square.x) * Z13_PER_Z9_SIDE;
    let y0 = u32::from(square.y) * Z13_PER_Z9_SIDE;
    ((x0, x0 + Z13_PER_Z9_SIDE), (y0, y0 + Z13_PER_Z9_SIDE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prague_square_and_name_roundtrip() {
        let sq = square_of(50.0, 14.25);
        assert_eq!(sq, Square { x: 276, y: 173 });
        assert_eq!(square_name(sq), "z9/276/173");
        assert_eq!(parse_square_name("z9/276/173"), Some(sq));
        assert_eq!(parse_square_name("z9/276"), None);
        assert_eq!(parse_square_name("841e309ffffffff"), None);
    }

    #[test]
    fn square_id_roundtrips_through_morton_bits() {
        for (x, y) in [(0, 0), (511, 511), (276, 173), (1, 0), (0, 1)] {
            let square = Square { x, y };
            assert_eq!(square_from_id(square_id(square)), Some(square));
        }
        assert_eq!(square_id(Square { x: 276, y: 173 }), 100_786);
        assert_eq!(square_from_id(-1), None);
        assert_eq!(square_from_id(MAX_SQUARE_ID + 1), None);
    }

    #[test]
    fn arg_order_lat_lon_square_vs_lon_lat_grid() {
        // square_of takes (lat, lon), lonlat_to_grid takes (lon, lat) — a
        // swap must fail loudly here, not silently serve another square.
        let sq = square_of(50.0, 14.25);
        assert_eq!(sq, Square { x: 276, y: 173 });
        assert_ne!(square_of(14.25, 50.0), sq);
        // The z30 cell of the same point sits inside that square: x matches
        // directly (z9→z30 is 21 bits), y is flipped (grid origin south,
        // square origin north). Swapped grid args land on another continent.
        let (gx, gy) = lonlat_to_grid(14.25, 50.0);
        assert_eq!((gx >> 21, 511 - (gy >> 21)), (276, 173));
        let (sx, sy) = lonlat_to_grid(50.0, 14.25);
        assert_ne!((sx >> 21, 511 - (sy >> 21)), (276, 173));
    }

    #[test]
    fn antimeridian_wraps_poles_clamp() {
        assert_eq!(square_of(0.0, 180.0).x, square_of(0.0, -180.0).x);
        assert_eq!(square_of(0.0, 180.0).x, 0);
        assert_eq!(square_of(90.0, 0.0), square_of(MAX_MERCATOR_LAT_DEG, 0.0));
        assert_eq!(square_of(-90.0, 0.0), square_of(-MAX_MERCATOR_LAT_DEG, 0.0));
    }

    #[test]
    fn ring_radius_switches_past_81_degrees() {
        assert_eq!(ring_radius(0.0), 1);
        assert_eq!(ring_radius(50.0), 1);
        assert_eq!(ring_radius(78.2), 1);
        assert_eq!(ring_radius(-78.2), 1);
        assert_eq!(ring_radius(81.9), 1);
        assert_eq!(ring_radius(82.0), 2);
        assert_eq!(ring_radius(-85.0), 2);
    }

    #[test]
    fn ring_counts_and_wraps() {
        assert_eq!(ring_squares(Square { x: 276, y: 173 }, 50.0).len(), 9);
        assert_eq!(ring_squares(Square { x: 276, y: 173 }, 82.0).len(), 25);
        let edge = ring_squares(Square { x: 0, y: 0 }, 0.0);
        assert!(edge.contains(&Square { x: 511, y: 0 }));
        assert!(edge.contains(&Square { x: 0, y: 0 }));
        // The clamped pole row contributes once, not twice.
        assert_eq!(edge.len(), 6);
    }

    #[test]
    fn owned_z13_is_exact_16x16() {
        assert_eq!(
            owned_z13(Square { x: 276, y: 173 }),
            ((4416, 4432), (2768, 2784))
        );
    }

    #[test]
    fn grid_roundtrip_within_one_quantum() {
        let (gx, gy) = lonlat_to_grid(14.25, 50.0);
        let (x, y) = grid_to_meters(gx, gy);
        let (x0, y0) = lonlat_to_meters(14.25, 50.0);
        assert!((x - x0).abs() <= GRID_QUANTUM_M);
        assert!((y - y0).abs() <= GRID_QUANTUM_M);
        // Extremes still fit i32: |coord| < 2^30.
        let (ex, ey) = meters_to_grid(EARTH_CIRCUMFERENCE_M / 2.0, 0.0);
        assert!(ex > 0 && ey == 1 << 29);
    }
}
