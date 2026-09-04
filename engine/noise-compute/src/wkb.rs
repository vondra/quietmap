//! WKB polygon utilities — area calculation from hex-encoded WKB.
//!
//! Used by both source-reader (popup) and tile-painter (tile generation)
//! to compute real area from OSM polygon data instead of hardcoded defaults.
//!
//! WHY: Industrial area was hardcoded to 10000 m², building area to 100 m².
//! This caused Spolana (500K m²) to have same emission as a recycling yard.
//! Formula: Lw = base + 10×log₁₀(area/10000) per ISO 8297.
//!
//! APPROACH: Shoelace formula on WGS84 coordinates with cos(lat) correction.
//! Gemini 3.1 Pro review noted this is approximate; EPSG:3035 projection is
//! more accurate. For continental scale this is acceptable (<5% error).
//! Future: osm-extract should pre-compute area_m2 in metric CRS.

use crate::constants::{M_PER_DEG_LAT, M_PER_DEG_LON_EQ};

pub struct AreaGridPoint {
    pub lat: f64,
    pub lon: f64,
    pub area_m2: f64,
}

/// Compute area in m² from hex-encoded WKB polygon.
/// Returns None if WKB is invalid, too short, or not a Polygon/MultiPolygon.
pub fn wkb_area_m2(wkb_hex: &str) -> Option<f64> {
    if wkb_hex.len() < 18 {
        return None;
    }

    let bytes: Vec<u8> = (0..wkb_hex.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(&wkb_hex[i..i + 2], 16).ok())
        .collect();

    if bytes.len() < 9 {
        return None;
    }

    let le = bytes[0] == 1;
    let wkb_type = if le {
        u32::from_le_bytes([bytes[1], bytes[2], bytes[3], bytes[4]])
    } else {
        u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]])
    };

    if wkb_type != 3 && wkb_type != 6 {
        return None;
    } // 3=Polygon, 6=MultiPolygon

    let read_u32 = |off: usize| -> u32 {
        if off + 4 > bytes.len() {
            return 0;
        }
        if le {
            u32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]])
        } else {
            u32::from_be_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]])
        }
    };
    let read_f64 = |off: usize| -> f64 {
        if off + 8 > bytes.len() {
            return 0.0;
        }
        let mut b = [0u8; 8];
        b.copy_from_slice(&bytes[off..off + 8]);
        if le {
            f64::from_le_bytes(b)
        } else {
            f64::from_be_bytes(b)
        }
    };

    if wkb_type == 3 {
        ring_area(&bytes, 5, &read_u32, &read_f64)
    } else {
        // MultiPolygon — sum areas
        let num_polys = read_u32(5) as usize;
        let mut total = 0.0;
        let mut off = 9;
        for _ in 0..num_polys {
            if off + 9 > bytes.len() {
                break;
            }
            off += 5; // skip sub-polygon header
            if let Some((area, new_off)) = ring_area_offset(&bytes, off, &read_u32, &read_f64) {
                total += area;
                off = new_off;
            } else {
                break;
            }
        }
        if total > 0.0 {
            Some(total)
        } else {
            None
        }
    }
}

fn ring_area(
    bytes: &[u8],
    start: usize,
    read_u32: &dyn Fn(usize) -> u32,
    read_f64: &dyn Fn(usize) -> f64,
) -> Option<f64> {
    ring_area_offset(bytes, start, read_u32, read_f64).map(|(a, _)| a)
}

fn ring_area_offset(
    bytes: &[u8],
    start: usize,
    read_u32: &dyn Fn(usize) -> u32,
    read_f64: &dyn Fn(usize) -> f64,
) -> Option<(f64, usize)> {
    if start + 4 > bytes.len() {
        return None;
    }
    let num_rings = read_u32(start) as usize;
    let mut off = start + 4;
    if num_rings == 0 || off + 4 > bytes.len() {
        return None;
    }

    let num_points = read_u32(off) as usize;
    off += 4;
    if num_points < 3 || off + num_points * 16 > bytes.len() {
        return None;
    }

    let mut lats = Vec::with_capacity(num_points);
    let mut lons = Vec::with_capacity(num_points);
    for _ in 0..num_points {
        lons.push(read_f64(off));
        lats.push(read_f64(off + 8));
        off += 16;
    }

    // Shoelace for outer ring
    let mean_lat: f64 = lats.iter().sum::<f64>() / lats.len() as f64;
    let cos_lat = mean_lat.to_radians().cos();
    let mut area_deg2 = 0.0f64;
    for i in 0..num_points {
        let j = (i + 1) % num_points;
        let xi = lons[i] * cos_lat;
        let xj = lons[j] * cos_lat;
        area_deg2 += xi * lats[j] - xj * lats[i];
    }
    let outer_area = (area_deg2 / 2.0).abs() * M_PER_DEG_LAT * M_PER_DEG_LON_EQ;

    // Subtract inner rings (courtyards, holes).
    // WHY: Buildings with courtyards had full outer area → Lw overestimated by ~3 dB.
    let mut hole_area = 0.0f64;
    for _ in 1..num_rings {
        if off + 4 > bytes.len() {
            break;
        }
        let rp = read_u32(off) as usize;
        off += 4;
        if rp < 3 || off + rp * 16 > bytes.len() {
            off += rp * 16;
            continue;
        }
        let mut h_lats = Vec::with_capacity(rp);
        let mut h_lons = Vec::with_capacity(rp);
        for _ in 0..rp {
            h_lons.push(read_f64(off));
            h_lats.push(read_f64(off + 8));
            off += 16;
        }
        let mut h_area = 0.0f64;
        for i in 0..rp {
            let j = (i + 1) % rp;
            h_area += h_lons[i] * cos_lat * h_lats[j] - h_lons[j] * cos_lat * h_lats[i];
        }
        hole_area += (h_area / 2.0).abs() * M_PER_DEG_LAT * M_PER_DEG_LON_EQ;
    }

    let area_m2 = (outer_area - hole_area).max(1.0);
    Some((area_m2, off))
}

/// Generate weighted square area cells inside a WKB polygon.
///
/// Each returned point represents the covered polygon area inside one global
/// grid cell. Subsampling keeps narrow polygons and clipped boundary cells from
/// disappearing when the cell center alone falls outside the polygon.
pub fn wkb_area_grid_points(
    wkb_hex: &str,
    spacing_m: f64,
    samples_per_axis: usize,
) -> Vec<AreaGridPoint> {
    let polys = parse_wkb_polygons(wkb_hex);
    if polys.is_empty() || spacing_m <= 0.0 {
        return vec![];
    }

    let (min_lat, max_lat, min_lon, max_lon) = outer_ring_bbox(&polys);

    let samples = samples_per_axis.max(1);
    let mid_lat = (min_lat + max_lat) / 2.0;
    let lat_step = spacing_m / M_PER_DEG_LAT;
    let lon_step = spacing_m / (M_PER_DEG_LON_EQ * mid_lat.to_radians().cos().max(0.1));
    let sample_area_m2 = spacing_m * spacing_m / (samples * samples) as f64;

    let mut points = Vec::new();
    let lat_start = (min_lat / lat_step).floor() * lat_step + lat_step / 2.0;
    let lon_start = (min_lon / lon_step).floor() * lon_step + lon_step / 2.0;
    let mut lat = lat_start;
    while lat <= max_lat {
        let mut lon = lon_start;
        while lon <= max_lon {
            let mut count = 0usize;
            let mut sum_lat = 0.0;
            let mut sum_lon = 0.0;
            for sy in 0..samples {
                let yoff = ((sy as f64 + 0.5) / samples as f64 - 0.5) * lat_step;
                let sample_lat = lat + yoff;
                for sx in 0..samples {
                    let xoff = ((sx as f64 + 0.5) / samples as f64 - 0.5) * lon_step;
                    let sample_lon = lon + xoff;
                    if point_in_any_polygon(sample_lat, sample_lon, &polys) {
                        count += 1;
                        sum_lat += sample_lat;
                        sum_lon += sample_lon;
                    }
                }
            }
            if count > 0 {
                points.push(AreaGridPoint {
                    lat: sum_lat / count as f64,
                    lon: sum_lon / count as f64,
                    area_m2: sample_area_m2 * count as f64,
                });
            }
            lon += lon_step;
        }
        lat += lat_step;
    }

    if points.is_empty() {
        let (clat, clon) = footprint_centroid(&polys);
        points.push(AreaGridPoint {
            lat: clat,
            lon: clon,
            area_m2: spacing_m * spacing_m,
        });
    }

    points
}

/// One parsed WKB sub-polygon: outer ring + inner rings (holes), each `(lat, lon)`.
/// Outer-ring area (m²) of every polygon in a raw WKB blob, holes IGNORED.
///
/// The obstacle loaders' footprint area (tile painter and popup both feed it to
/// [`crate::low_profile`]'s comparable-area test), so it lives here once —
/// a second spelling would let the two lanes cap different footprints. Holes are
/// out because the test compares against OSM's `area_m2`, which is also gross.
/// Distinct from [`wkb_area_m2`], which takes HEX and subtracts holes.
pub fn outer_ring_area_m2(wkb: &[u8]) -> f32 {
    let mut total = 0.0f64;
    for (outer, _holes) in parse_wkb_polygons_bytes(wkb) {
        if outer.len() < 4 {
            continue;
        }
        let lat0 = outer[0].0;
        let m_lon = 111_320.0 * lat0.to_radians().cos().max(0.1);
        let mut acc = 0.0f64;
        for w in outer.windows(2) {
            let (x0, y0) = ((w[0].1) * m_lon, (w[0].0) * 111_320.0);
            let (x1, y1) = ((w[1].1) * m_lon, (w[1].0) * 111_320.0);
            acc += x0 * y1 - x1 * y0;
        }
        total += (acc * 0.5).abs();
    }
    total as f32
}

pub type WkbPoly = (Vec<(f64, f64)>, Vec<Vec<(f64, f64)>>);

/// Parse a WKB-hex Polygon (type 3) or MultiPolygon (type 6) into ALL its
/// sub-polygons, each `(outer ring, holes)`. Empty vec on invalid/unsupported WKB.
///
/// A MultiPolygon yields EVERY part. The prior single-polygon parser read only the
/// first sub-polygon, so a multi-part industrial site (or building) had its whole
/// rescaled area-weighted emission collapsed onto part 1's footprint — energy was
/// conserved but spatially wrong.
pub(crate) fn parse_wkb_polygons(wkb_hex: &str) -> Vec<WkbPoly> {
    if wkb_hex.len() < 18 {
        return Vec::new();
    }
    let bytes: Vec<u8> = (0..wkb_hex.len())
        .step_by(2)
        // `.get` (not `&wkb_hex[..]`) so an odd-length hex string can't panic on the
        // final 1-char slice.
        .filter_map(|i| {
            wkb_hex
                .get(i..i + 2)
                .and_then(|s| u8::from_str_radix(s, 16).ok())
        })
        .collect();
    parse_wkb_polygons_bytes(&bytes)
}

/// [`parse_wkb_polygons`] over raw WKB bytes — the obstacle store carries WKB
/// unencoded (Overture parquet passthrough), so the vector-obstacle loaders
/// skip the hex round-trip.
// pub (was pub(crate)): tile-painter's low-profile height cap measures the
// outer-ring area with the SAME parser the obstacle builder consumes.
pub fn parse_wkb_polygons_bytes(bytes: &[u8]) -> Vec<WkbPoly> {
    if bytes.len() < 9 {
        return Vec::new();
    }

    let le = bytes[0] == 1;
    let read_u32 = |off: usize| -> u32 {
        if off + 4 > bytes.len() {
            return 0;
        }
        if le {
            u32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]])
        } else {
            u32::from_be_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]])
        }
    };
    let read_f64 = |off: usize| -> f64 {
        if off + 8 > bytes.len() {
            return 0.0;
        }
        let mut b = [0u8; 8];
        b.copy_from_slice(&bytes[off..off + 8]);
        if le {
            f64::from_le_bytes(b)
        } else {
            f64::from_be_bytes(b)
        }
    };

    match read_u32(1) {
        3 => parse_one_polygon(bytes, 5, &read_u32, &read_f64)
            .map(|(poly, _)| vec![poly])
            .unwrap_or_default(),
        6 => {
            let num_polys = read_u32(5) as usize;
            let mut polys = Vec::new();
            let mut off = 9;
            for _ in 0..num_polys {
                // Each sub-polygon: byte-order(1) + type(4), then its rings.
                // Mixed-endian children are legal WKB but unseen in our
                // sources (Overture/OSM emit uniform LE); the shared read
                // closures are bound to the OUTER order, so bail on the
                // whole geometry rather than misparse (gg review).
                if off + 5 > bytes.len() {
                    break;
                }
                if (bytes[off] == 1) != le {
                    return polys; // mixed-endian child — refuse the remainder
                }
                off += 5;
                match parse_one_polygon(bytes, off, &read_u32, &read_f64) {
                    Some((poly, new_off)) => {
                        polys.push(poly);
                        off = new_off;
                    }
                    None => break,
                }
            }
            polys
        }
        _ => Vec::new(),
    }
}

/// Parse a WKB LineString (type 2) into its points as `(lat, lon)` pairs —
/// the noise-wall micro-segment shape in `structures.arrow` (kind=barrier).
/// Empty vec on invalid/unsupported WKB (a Polygon is NOT a valid wall row:
/// the kind column routes, the parser refuses the wrong shape rather than
/// guessing).
///
/// Shares the polygon parser's bounds discipline: the point count is checked
/// against the remaining bytes before any allocation.
pub fn parse_wkb_linestring_bytes(bytes: &[u8]) -> Vec<(f64, f64)> {
    if bytes.len() < 9 {
        return Vec::new();
    }
    let le = bytes[0] == 1;
    let read_u32 = |off: usize| -> u32 {
        if off + 4 > bytes.len() {
            return 0;
        }
        if le {
            u32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]])
        } else {
            u32::from_be_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]])
        }
    };
    let read_f64 = |off: usize| -> f64 {
        if off + 8 > bytes.len() {
            return 0.0;
        }
        let mut b = [0u8; 8];
        b.copy_from_slice(&bytes[off..off + 8]);
        if le {
            f64::from_le_bytes(b)
        } else {
            f64::from_be_bytes(b)
        }
    };
    if read_u32(1) != 2 {
        return Vec::new();
    }
    let np = read_u32(5) as usize;
    if np > (bytes.len() - 9) / 16 {
        return Vec::new();
    }
    let mut points = Vec::with_capacity(np);
    let mut off = 9;
    for _ in 0..np {
        let lon = read_f64(off);
        let lat = read_f64(off + 8);
        points.push((lat, lon));
        off += 16;
    }
    points
}

/// Parse one polygon's rings (outer + holes) starting at its ring-count offset;
/// returns the polygon and the offset just past its last ring. Rings with <3 points
/// are dropped but still advance the offset, so MultiPolygon iteration stays in sync.
fn parse_one_polygon(
    bytes: &[u8],
    start: usize,
    read_u32: &dyn Fn(usize) -> u32,
    read_f64: &dyn Fn(usize) -> f64,
) -> Option<(WkbPoly, usize)> {
    if start + 4 > bytes.len() {
        return None;
    }
    let num_rings = read_u32(start) as usize;
    // Bound the ring count by the bytes left (>=4 per ring) BEFORE allocating — an
    // attacker-controlled u32 count would otherwise request a ~100 GB Vec. Division
    // (not `num_rings * 4`) avoids overflow on 32-bit targets.
    if num_rings == 0 || num_rings > (bytes.len() - start) / 4 {
        return None;
    }
    let mut off = start + 4;
    let mut rings: Vec<Vec<(f64, f64)>> = Vec::with_capacity(num_rings);
    for _ in 0..num_rings {
        if off + 4 > bytes.len() {
            return None;
        }
        let np = read_u32(off) as usize;
        off += 4;
        // Same bound for the point count (16 bytes/point), overflow-safe.
        if np > (bytes.len() - off) / 16 {
            return None;
        }
        let mut ring = Vec::with_capacity(np);
        for _ in 0..np {
            let lon = read_f64(off);
            let lat = read_f64(off + 8);
            ring.push((lat, lon));
            off += 16;
        }
        rings.push(ring);
    }
    let mut iter = rings.into_iter();
    let outer = iter.next()?;
    if outer.len() < 3 {
        return None;
    }
    let holes: Vec<Vec<(f64, f64)>> = iter.filter(|r| r.len() >= 3).collect();
    Some(((outer, holes), off))
}

/// True if `(lat, lon)` is inside ANY sub-polygon's outer ring and outside that
/// sub-polygon's holes.
pub(crate) fn point_in_any_polygon(lat: f64, lon: f64, polys: &[WkbPoly]) -> bool {
    polys.iter().any(|(outer, holes)| {
        point_in_polygon(lat, lon, outer) && !holes.iter().any(|h| point_in_polygon(lat, lon, h))
    })
}

/// A parsed building/area footprint with a cached bbox, for the osm-extract
/// POI-in-footprint spatial join (settlement v2 phase 2). `osm-extract` indexes
/// buildings by `bbox()` (a centroid-only grid misses a POI inside a large
/// footprint), then `contains()` is the exact
/// point-in-polygon test. Parsing once and reusing keeps the per-POI test cheap.
pub struct WkbFootprint {
    polys: Vec<WkbPoly>,
    /// `(min_lat, max_lat, min_lon, max_lon)`.
    bbox: (f64, f64, f64, f64),
}

impl WkbFootprint {
    /// Parse a WKB-hex polygon/multipolygon. `None` when the WKB is invalid or
    /// has no usable ring (so the join skips a building with no geometry).
    pub fn parse(wkb_hex: &str) -> Option<Self> {
        let polys = parse_wkb_polygons(wkb_hex);
        if polys.is_empty() {
            return None;
        }
        let bbox = outer_ring_bbox(&polys);
        Some(Self { polys, bbox })
    }

    /// `(min_lat, max_lat, min_lon, max_lon)` of every outer ring.
    pub fn bbox(&self) -> (f64, f64, f64, f64) {
        self.bbox
    }

    /// True if `(lat, lon)` is inside the footprint (outer ring, minus holes).
    pub fn contains(&self, lat: f64, lon: f64) -> bool {
        // Cheap bbox reject before the ray-cast.
        let (min_lat, max_lat, min_lon, max_lon) = self.bbox;
        if lat < min_lat || lat > max_lat || lon < min_lon || lon > max_lon {
            return false;
        }
        point_in_any_polygon(lat, lon, &self.polys)
    }
}

/// Bounding box `(min_lat, max_lat, min_lon, max_lon)` over every sub-polygon's
/// outer ring. Caller guarantees `polys` is non-empty.
fn outer_ring_bbox(polys: &[WkbPoly]) -> (f64, f64, f64, f64) {
    let (mut min_lat, mut max_lat) = (f64::MAX, f64::MIN);
    let (mut min_lon, mut max_lon) = (f64::MAX, f64::MIN);
    for (outer, _) in polys {
        for &(lat, lon) in outer {
            min_lat = min_lat.min(lat);
            max_lat = max_lat.max(lat);
            min_lon = min_lon.min(lon);
            max_lon = max_lon.max(lon);
        }
    }
    (min_lat, max_lat, min_lon, max_lon)
}

/// Mean vertex over every sub-polygon's outer ring — the fallback point when the
/// grid samples nothing (a polygon too small to catch a grid line).
fn footprint_centroid(polys: &[WkbPoly]) -> (f64, f64) {
    let (mut slat, mut slon, mut n) = (0.0f64, 0.0f64, 0usize);
    for (outer, _) in polys {
        for &(lat, lon) in outer {
            slat += lat;
            slon += lon;
            n += 1;
        }
    }
    let n = n.max(1) as f64;
    (slat / n, slon / n)
}

/// Ray-casting point-in-polygon test.
pub(crate) fn point_in_polygon(lat: f64, lon: f64, poly: &[(f64, f64)]) -> bool {
    let n = poly.len();
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (yi, xi) = poly[i];
        let (yj, xj) = poly[j];
        if ((yi > lat) != (yj > lat)) && (lon < (xj - xi) * (lat - yi) / (yj - yi) + xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Little-endian WKB Polygon (type 3) hex from rings of `(lat, lon)` points.
    fn polygon_wkb(rings: &[&[(f64, f64)]]) -> String {
        let mut b = vec![1u8];
        b.extend_from_slice(&3u32.to_le_bytes());
        b.extend_from_slice(&(rings.len() as u32).to_le_bytes());
        for ring in rings {
            b.extend_from_slice(&(ring.len() as u32).to_le_bytes());
            for &(lat, lon) in *ring {
                b.extend_from_slice(&lon.to_le_bytes());
                b.extend_from_slice(&lat.to_le_bytes());
            }
        }
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    /// Little-endian WKB MultiPolygon (type 6) hex from sub-polygons (each a slice
    /// of rings of `(lat, lon)` points).
    fn multipolygon_wkb(polys: &[&[&[(f64, f64)]]]) -> String {
        let mut b = vec![1u8];
        b.extend_from_slice(&6u32.to_le_bytes());
        b.extend_from_slice(&(polys.len() as u32).to_le_bytes());
        for poly in polys {
            b.push(1u8);
            b.extend_from_slice(&3u32.to_le_bytes());
            b.extend_from_slice(&(poly.len() as u32).to_le_bytes());
            for ring in *poly {
                b.extend_from_slice(&(ring.len() as u32).to_le_bytes());
                for &(lat, lon) in *ring {
                    b.extend_from_slice(&lon.to_le_bytes());
                    b.extend_from_slice(&lat.to_le_bytes());
                }
            }
        }
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn test_invalid_wkb() {
        assert_eq!(wkb_area_m2(""), None);
        assert_eq!(wkb_area_m2("0102"), None);
        assert!(parse_wkb_polygons("").is_empty());
        assert!(parse_wkb_polygons("0102").is_empty());
    }

    #[test]
    fn single_polygon_samples_inside_bbox() {
        let sq: &[(f64, f64)] = &[
            (50.000, 14.000),
            (50.000, 14.006),
            (50.004, 14.006),
            (50.004, 14.000),
            (50.000, 14.000),
        ];
        let cells = wkb_area_grid_points(&polygon_wkb(&[sq]), 75.0, 5);
        assert!(!cells.is_empty(), "a ~440 m square should yield grid cells");
        for c in &cells {
            assert!(c.area_m2 > 0.0);
            assert!((49.999..=50.005).contains(&c.lat) && (13.999..=14.007).contains(&c.lon));
        }
    }

    /// The MultiPolygon-collapse regression: a 2-part site must place emission in
    /// BOTH parts. The old single-polygon parser dropped part B entirely.
    #[test]
    fn multipolygon_covers_all_parts() {
        let a: &[(f64, f64)] = &[
            (50.000, 14.000),
            (50.000, 14.006),
            (50.004, 14.006),
            (50.004, 14.000),
            (50.000, 14.000),
        ];
        let b: &[(f64, f64)] = &[
            (50.000, 14.100),
            (50.000, 14.106),
            (50.004, 14.106),
            (50.004, 14.100),
            (50.000, 14.100),
        ];
        // parse must yield both sub-polygons.
        assert_eq!(
            parse_wkb_polygons(&multipolygon_wkb(&[&[a], &[b]])).len(),
            2
        );
        let cells = wkb_area_grid_points(&multipolygon_wkb(&[&[a], &[b]]), 75.0, 5);
        assert!(cells.iter().any(|c| c.lon < 14.05), "no cells in part A");
        assert!(
            cells.iter().any(|c| c.lon > 14.05),
            "no cells in part B — MultiPolygon collapsed to the first part"
        );
    }

    #[test]
    fn malformed_wkb_no_panic() {
        // Polygon header claiming u32::MAX rings — must reject before allocating, not OOM.
        assert!(parse_wkb_polygons("0103000000ffffffff").is_empty());
        // u32::MAX point count in the first ring.
        assert!(parse_wkb_polygons("010300000001000000ffffffff").is_empty());
        // Odd-length hex must not panic on the final 1-char slice.
        assert!(parse_wkb_polygons("0103000000ffffffffa").is_empty());
    }
}
