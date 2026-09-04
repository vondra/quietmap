//! Build-once grid index over the global OSM aerodrome set so Stage 1.5's
//! per-ground-segment "inside any aerodrome" gate runs in O(few-nearby)
//! instead of O(45443). A pure candidate-pruner: the final `flat_dist` +
//! `aerodrome_radius_m` test is byte-identical to the naive loop, so the
//! airports discovered / rejected / re-attributed are unchanged (proven by
//! the property test below). A mega-hub z9's gate drops from ~30 min to
//! seconds. (Stage 2C's `nearest_aerodrome_within` resolver is the same
//! O(45443) shape and the obvious next caller to migrate.)

use std::collections::HashMap;

use noise_compute::constants::{m_per_deg_lon, M_PER_DEG_LAT};
use noise_compute::propagation::geo::flat_dist;
use noise_compute::types::AirportArea;

use crate::airport_io::{aerodrome_radius_m, AERODROME_AEROWAY_TYPE, NEAREST_AERODROME_FLOOR_M};

/// Grid cell edge ≈ the 6 km floor radius, so a centroid's mandatory disk spans
/// only a small cell block and a query reads ONLY its own cell. Sized from
/// `M_PER_DEG_LAT` (the same constant `flat_dist` uses) so cell bounds and the
/// distance test share one scale.
const CELL_M: f64 = NEAREST_AERODROME_FLOOR_M;
const LAT_CELL_DEG: f64 = CELL_M / M_PER_DEG_LAT;

/// Longitude metres/degree at band `cy`'s centre latitude. The scale depends
/// only on the integer band (not the continuous point lat), so `build`'s
/// registration and `cell_of`'s query place a point in the same column.
fn band_lon_scale(cy: i32) -> f64 {
    let band_lat = (cy as f64 + 0.5) * LAT_CELL_DEG;
    m_per_deg_lon(band_lat.to_radians())
}

// An integer number of columns closes each latitude band exactly at ±180°.
fn band_columns(cy: i32) -> i32 {
    (360.0 * band_lon_scale(cy) / CELL_M).ceil() as i32
}

fn cell_of(lat: f64, lon: f64) -> (i32, i32) {
    let cy = (lat / LAT_CELL_DEG).floor() as i32;
    let columns = band_columns(cy);
    let cx = (((grid::geo::normalize_longitude(lon) + 180.0) / 360.0 * f64::from(columns)).floor()
        as i32)
        .rem_euclid(columns);
    (cy, cx)
}

/// Grid index over the global aerodrome polygons.
pub struct AerodromeIndex<'a> {
    /// cell → (index into `areas`, precomputed radius_m) for every aerodrome
    /// whose centroid-radius disk overlaps that cell. Storing the radius keeps
    /// `aerodrome_radius_m`'s sqrt off the per-candidate hot path.
    cells: HashMap<(i32, i32), Vec<(u32, f64)>>,
    /// Borrowed: queries return `&'a AirportArea` (Stage 1.5's `Reattribute`
    /// disposition holds one), and the final test runs the identical
    /// `flat_dist` + radius check against this same data.
    areas: &'a [AirportArea],
}

impl<'a> AerodromeIndex<'a> {
    /// Build once from the global aerodrome set. Each aerodrome registers in
    /// every cell its radius-disk overlaps, plus a +1-cell margin: `build`
    /// bounds the disk by the band-centre lon scale but queries test by
    /// `flat_dist`'s point-latitude scale, and one extra cell absorbs that
    /// divergence for every real aerodrome radius (≤~10 km — ~0.4 cells even
    /// at 89.5°N).
    pub fn build(areas: &'a [AirportArea]) -> Self {
        let mut cells: HashMap<(i32, i32), Vec<(u32, f64)>> = HashMap::new();
        for (i, a) in areas.iter().enumerate() {
            if a.aeroway_type != AERODROME_AEROWAY_TYPE {
                continue;
            }
            let radius = aerodrome_radius_m(a);
            let dlat = radius / M_PER_DEG_LAT;
            let cy_min = ((a.centroid_lat - dlat) / LAT_CELL_DEG).floor() as i32 - 1;
            let cy_max = ((a.centroid_lat + dlat) / LAT_CELL_DEG).floor() as i32 + 1;
            for cy in cy_min..=cy_max {
                let lon_scale = band_lon_scale(cy);
                let columns = band_columns(cy);
                let lon_cell_deg = 360.0 / f64::from(columns);
                let dlon = radius / lon_scale;
                let cx_min = ((a.centroid_lon + 180.0 - dlon) / lon_cell_deg).floor() as i32 - 1;
                let cx_max = ((a.centroid_lon + 180.0 + dlon) / lon_cell_deg).floor() as i32 + 1;
                for cx in cx_min..=cx_max {
                    cells
                        .entry((cy, cx.rem_euclid(columns)))
                        .or_default()
                        .push((i as u32, radius));
                }
            }
        }
        Self { cells, areas }
    }

    /// Identical result to the naive `point_in_any_aerodrome` oracle: true iff
    /// the point is within ANY aerodrome's centroid-radius. First-hit
    /// short-circuit (the boolean OR is order-independent).
    pub fn contains(&self, lat: f64, lon: f64) -> bool {
        let Some(ids) = self.cells.get(&cell_of(lat, lon)) else {
            return false;
        };
        ids.iter().any(|&(i, radius)| {
            let a = &self.areas[i as usize];
            flat_dist(lat, lon, a.centroid_lat, a.centroid_lon) <= radius
        })
    }

    /// Identical result to `airport_io::nearest_aerodrome_within`, including the
    /// empty-key/empty-name skip and the strict-`<` closest-wins tie-break (cell
    /// vecs hold candidates in ascending original index, so ties keep the first).
    pub fn nearest(&self, lat: f64, lon: f64) -> Option<&AirportArea> {
        let ids = self.cells.get(&cell_of(lat, lon))?;
        let mut best: Option<(u32, f64)> = None;
        for &(i, radius) in ids {
            let a = &self.areas[i as usize];
            if a.airport_key.is_empty() && a.name.is_empty() {
                continue;
            }
            let dist = flat_dist(lat, lon, a.centroid_lat, a.centroid_lon);
            if dist > radius {
                continue;
            }
            if best.map(|(_, d)| dist < d).unwrap_or(true) {
                best = Some((i, dist));
            }
        }
        best.map(|(i, _)| &self.areas[i as usize])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::airport_io::nearest_aerodrome_within;

    fn area(osm_id: i64, lat: f64, lon: f64, area_m2: f32, ty: u8, key: &str) -> AirportArea {
        AirportArea {
            osm_id,
            aeroway_type: ty,
            name: String::new(),
            airport_key: key.to_string(),
            centroid_lat: lat,
            centroid_lon: lon,
            polygon_grid: Vec::new(),
            area_m2,
        }
    }

    /// Naive O(n) `contains` oracle — the original Stage 1.5 gate the index
    /// replaced. A point is covered iff it's within ANY aerodrome's
    /// centroid-radius; unlike `nearest_aerodrome_within` it does NOT skip
    /// empty-key/name polygons (an unnamed mapped airfield still counts as
    /// "already covered").
    fn point_in_any_aerodrome(lat: f64, lon: f64, areas: &[AirportArea]) -> bool {
        areas
            .iter()
            .filter(|a| a.aeroway_type == AERODROME_AEROWAY_TYPE)
            .any(|a| flat_dist(lat, lon, a.centroid_lat, a.centroid_lon) <= aerodrome_radius_m(a))
    }

    #[test]
    fn dateline_disks_match_wrapping_oracle_in_both_directions() {
        for sign in [-1.0, 1.0] {
            let areas = vec![area(1, 0.0, sign * 179.95, 0.0, 5, "DATELINE")];
            let index = AerodromeIndex::build(&areas);
            let lon = -sign * 179.997;
            assert!(point_in_any_aerodrome(0.0, lon, &areas));
            assert!(index.contains(0.0, lon));
            assert_eq!(index.nearest(0.0, lon).map(|a| a.osm_id), Some(1));
        }
    }

    /// The index is a pure candidate-pruner: over a large pseudo-random probe set
    /// (plus boundary cases) the indexed gates must equal the naive gates exactly.
    #[test]
    fn index_matches_naive_gates() {
        // Varied radius (area<=0 → 6km floor; small → 6km floor; large → ~10km),
        // varied latitude (incl. high lat where lon cells shrink), a tie pair, and
        // the antimeridian band (both oracle and index wrap there).
        let areas = vec![
            area(1, 50.10, 14.26, 0.0, 5, "LKPR"),       // floor radius
            area(2, 50.12, 14.30, 3.0e8, 5, "BIG"),      // ~9.8 km radius
            area(3, 49.78, 14.17, 1.0e6, 5, "DOBRIS"),   // floor (sqrt→564m)
            area(4, 69.50, 18.90, 5.0e7, 5, "TROMSO"),   // high lat
            area(5, 0.0, 179.95, 2.0e7, 5, "DATELINE"),  // near +180
            area(6, 50.1001, 14.2601, 0.0, 5, "TIE"),    // overlaps LKPR
            area(7, 50.20, 14.40, 2.0e7, 3, "NOT_AERO"), // aeroway != 5 → ignored
            area(8, 51.00, 15.00, 0.0, 5, ""),           // empty key → nearest skips
        ];
        let idx = AerodromeIndex::build(&areas);

        // Deterministic LCG over a grid around the fixtures + global sweep.
        let mut s: u64 = 0x9E3779B97F4A7C15;
        let mut rng = || {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (s >> 33) as f64 / (1u64 << 31) as f64 // [0,1)
        };
        for _ in 0..200_000 {
            // Mix of local (near fixtures, exercises radius boundary) and global.
            let (lat, lon) = if rng() < 0.7 {
                (49.0 + rng() * 2.5, 13.5 + rng() * 1.5)
            } else {
                (-85.0 + rng() * 170.0, -180.0 + rng() * 360.0)
            };
            assert_eq!(
                idx.contains(lat, lon),
                point_in_any_aerodrome(lat, lon, &areas),
                "contains mismatch at {lat},{lon}"
            );
            assert_eq!(
                idx.nearest(lat, lon).map(|a| a.osm_id),
                nearest_aerodrome_within(lat, lon, &areas).map(|a| a.osm_id),
                "nearest mismatch at {lat},{lon}"
            );
        }
    }
}
