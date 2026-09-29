//! Point and area sources as dev4 placed them (`normalize/points.rs`): a site larger than its
//! threshold becomes the cells of a latitude-longitude lattice that catch any of 5 x 5 samples of
//! its z30 ring, each at its samples' mean with its share of the site's area and energy and an
//! exclusion radius sqrt(cell area / pi); a smaller site, or one without a ring, is one point at
//! its centroid with the radius of its whole area.

use super::Converted;
use crate::dev4::{degrees_to_z30, z30_corner_degrees, z30_corner_mercator_m};
use tiles::geo::{GlobalSteps, Mercator, TileId};
use tiles::sources::Attribute;

/// dev4's flat-earth metres per degree of latitude and of longitude at the equator (`grid::geo`).
const M_PER_DEG_LAT: f64 = 110_540.0;
const M_PER_DEG_LON_EQ: f64 = 111_320.0;
const SAMPLES_PER_CELL_SIDE: usize = 5;
/// No footprint counts less than a square metre (dev4 `MIN_FOOTPRINT_AREA_M2`).
const MINIMUM_AREA_M2: f64 = 1.0;
/// Sources below this A-weighted sound power are dropped (dev4's audibility gate).
pub const AUDIBILITY_FLOOR_DBA: f64 = 10.0;

/// A dev4 ring or chain: z30 cells, x east and y north.
pub type Z30Ring = Vec<(i32, i32)>;

/// dev4 `decode_grid_poly`: a u32 count (two or more) and that many (i32, i32) pairs, exactly.
pub fn decode_z30_ring(bytes: &[u8]) -> Option<Z30Ring> {
    let count = u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?) as usize;
    if count < 2 || bytes.len() != 4 + 8 * count {
        return None;
    }
    let word = |at: usize| i32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    Some(
        (0..count)
            .map(|i| (word(4 + 8 * i), word(8 + 8 * i)))
            .collect(),
    )
}

/// A square ring of `side_m` around `centre` (latitude, longitude), its corners snapped to z30
/// (dev4's ship cell footprint).
pub fn square_ring(centre: (f64, f64), side_m: f64) -> Z30Ring {
    let (lat, lon) = centre;
    let half_m = side_m / 2.0;
    let lat_step = half_m / M_PER_DEG_LAT;
    let lon_step = half_m / (M_PER_DEG_LON_EQ * lat.to_radians().cos().max(0.01));
    let wrap = |lon: f64| {
        if (-180.0..180.0).contains(&lon) {
            lon
        } else {
            (lon + 180.0).rem_euclid(360.0) - 180.0
        }
    };
    [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
        .iter()
        .map(|(east, north)| degrees_to_z30(lat + north * lat_step, wrap(lon + east * lon_step)))
        .collect()
}

/// A ring of three or more vertices in Web Mercator metres with its box, for containment tests.
pub struct PreparedRing {
    points: Vec<[f64; 2]>,
    bbox: [f64; 4],
}

impl PreparedRing {
    pub fn new(ring: &[(i32, i32)]) -> Option<Self> {
        if ring.len() < 3 {
            return None;
        }
        let points: Vec<[f64; 2]> = ring
            .iter()
            .map(|&(x, y)| z30_corner_mercator_m(x, y))
            .collect();
        let bbox = points.iter().fold(
            [f64::MAX, f64::MAX, f64::MIN, f64::MIN],
            |[x0, y0, x1, y1], &[x, y]| [x0.min(x), y0.min(y), x1.max(x), y1.max(y)],
        );
        Some(PreparedRing { points, bbox })
    }

    /// Whether the corner of z30 `cell` lies inside (even-odd rule, dev4 `PreparedRing::contains`).
    pub fn contains(&self, cell: (i32, i32)) -> bool {
        let [px, py] = z30_corner_mercator_m(cell.0, cell.1);
        let [x0, y0, x1, y1] = self.bbox;
        if px < x0 || px > x1 || py < y0 || py > y1 {
            return false;
        }
        let mut inside = false;
        let mut j = self.points.len() - 1;
        for i in 0..self.points.len() {
            let ([xi, yi], [xj, yj]) = (self.points[i], self.points[j]);
            if (yi > py) != (yj > py) && px < (xj - xi) * (py - yi) / (yj - yi) + xi {
                inside = !inside;
            }
            j = i;
        }
        inside
    }
}

/// dev4 `ring_area_m2`: the Mercator shoelace scaled by cos^2 of the vertices' mean latitude.
pub fn ring_area_m2(ring: &[(i32, i32)]) -> Option<f64> {
    if ring.len() < 3 {
        return None;
    }
    let points: Vec<[f64; 2]> = ring
        .iter()
        .map(|&(x, y)| z30_corner_mercator_m(x, y))
        .collect();
    let mean_y = points.iter().map(|p| p[1]).sum::<f64>() / points.len() as f64;
    let mean_lat =
        2.0 * (mean_y / tiles::geo::WGS84_A_M).exp().atan() - std::f64::consts::FRAC_PI_2;
    let shoelace: f64 = (0..points.len())
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % points.len()]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum();
    Some(((shoelace / 2.0).abs() * mean_lat.cos().powi(2)).max(MINIMUM_AREA_M2))
}

/// A site's area: the stored value when positive, else its ring's, else `default_m2`.
pub fn resolve_area_m2(stored_m2: Option<f64>, ring: &[(i32, i32)], default_m2: f64) -> f64 {
    stored_m2
        .filter(|area| *area > 0.0)
        .or_else(|| ring_area_m2(ring))
        .unwrap_or(default_m2)
        .max(MINIMUM_AREA_M2)
}

/// Where a site emits: its centroid (latitude, longitude), its ring (empty without one), its
/// area, the largest area emitted from one point and the cell side above it.
pub struct Site<'a> {
    pub centroid: (f64, f64),
    pub ring: &'a [(i32, i32)],
    pub area_m2: f64,
    pub single_point_up_to_m2: f64,
    pub cell_m: f64,
}

/// A point of a site and the part of the site's area it stands for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SitePoint {
    pub lat: f64,
    pub lon: f64,
    pub area_m2: f64,
}

/// The points a site emits from, their areas summing to the site's.
pub fn site_points(site: &Site) -> Vec<SitePoint> {
    let whole = || {
        let (lat, lon) = site.centroid;
        vec![SitePoint {
            lat,
            lon,
            area_m2: site.area_m2,
        }]
    };
    if site.area_m2 <= site.single_point_up_to_m2 || site.ring.is_empty() {
        return whole();
    }
    let cells = lattice_cells(site.ring, site.cell_m);
    if cells.len() <= 1 {
        return whole();
    }
    let sampled_m2 = cells.iter().map(|cell| cell.area_m2).sum::<f64>().max(1.0);
    cells
        .into_iter()
        .map(|cell| SitePoint {
            area_m2: cell.area_m2 * site.area_m2 / sampled_m2,
            ..cell
        })
        .collect()
}

/// dev4 `ring_area_grid_points`: lattice cells of `cell_m` anchored on whole multiples of the
/// step, each at the mean of its samples inside the ring; the vertex mean if none is.
fn lattice_cells(ring: &[(i32, i32)], cell_m: f64) -> Vec<SitePoint> {
    let vertices: Vec<(f64, f64)> = ring
        .iter()
        .map(|&(x, y)| z30_corner_degrees(x, y))
        .collect();
    let (min_lat, max_lat, min_lon, max_lon) = vertices.iter().fold(
        (f64::MAX, f64::MIN, f64::MAX, f64::MIN),
        |(a, b, c, d), &(lat, lon)| (a.min(lat), b.max(lat), c.min(lon), d.max(lon)),
    );
    let lat_step = cell_m / M_PER_DEG_LAT;
    let mid_lat = (min_lat + max_lat) / 2.0;
    let lon_step = cell_m / (M_PER_DEG_LON_EQ * mid_lat.to_radians().cos().max(0.1));
    let samples = SAMPLES_PER_CELL_SIDE as f64;
    let sample_area_m2 = cell_m * cell_m / (samples * samples);
    let offset = |index: usize, step: f64| ((index as f64 + 0.5) / samples - 0.5) * step;
    let first_lon = (min_lon / lon_step).floor() * lon_step + lon_step / 2.0;
    let mut cells = Vec::new();
    if let Some(prepared) = PreparedRing::new(ring) {
        let mut lat = (min_lat / lat_step).floor() * lat_step + lat_step / 2.0;
        while lat <= max_lat {
            let mut lon = first_lon;
            while lon <= max_lon {
                let (mut count, mut sum_lat, mut sum_lon) = (0usize, 0.0, 0.0);
                for sample_row in 0..SAMPLES_PER_CELL_SIDE {
                    let sample_lat = lat + offset(sample_row, lat_step);
                    for sample_column in 0..SAMPLES_PER_CELL_SIDE {
                        let sample_lon = lon + offset(sample_column, lon_step);
                        if prepared.contains(degrees_to_z30(sample_lat, sample_lon)) {
                            (count, sum_lat, sum_lon) =
                                (count + 1, sum_lat + sample_lat, sum_lon + sample_lon);
                        }
                    }
                }
                if count > 0 {
                    let n = count as f64;
                    cells.push(SitePoint {
                        lat: sum_lat / n,
                        lon: sum_lon / n,
                        area_m2: sample_area_m2 * n,
                    });
                }
                lon += lon_step;
            }
            lat += lat_step;
        }
    }
    if cells.is_empty() {
        let n = vertices.len().max(1) as f64;
        let (lat, lon) = vertices
            .iter()
            .fold((0.0, 0.0), |(a, b), &(lat, lon)| (a + lat, b + lon));
        cells.push(SitePoint {
            lat: lat / n,
            lon: lon / n,
            area_m2: cell_m * cell_m,
        });
    }
    cells
}

/// The tile holding a position and its point piece there (both ends equal).
pub fn point_piece(lat: f64, lon: f64) -> (TileId, [[i16; 2]; 2]) {
    let global = GlobalSteps::nearest(Mercator::from_degrees(lat, lon));
    let tile = TileId::containing(global.to_mercator());
    let local = tile
        .local(global)
        .expect("a point lies inside its own tile");
    (tile, [local, local])
}

/// One piece per site point: `attribute` carries the whole site's emission; each point gets its
/// area share of it and the exclusion radius of its area.
pub fn push_site_points(
    points: &[SitePoint],
    site_area_m2: f64,
    attribute: &Attribute,
    out: &mut Vec<Converted>,
) {
    for point in points {
        let share_db = 10.0 * (site_area_m2 / point.area_m2.max(MINIMUM_AREA_M2)).log10();
        let (tile, ends) = point_piece(point.lat, point.lon);
        let mut attribute = attribute.clone();
        attribute.emission = attribute
            .emission
            .map(|period| period.map(|level| level - share_db));
        attribute.exclusion_radius_m = (point.area_m2 / std::f64::consts::PI).sqrt();
        out.push(Converted {
            tile,
            ends,
            attribute,
        });
    }
}

#[cfg(test)]
#[path = "cells_tests.rs"]
mod tests;
