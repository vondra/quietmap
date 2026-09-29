//! The low-profile height cap (dev4 `low_profile.rs`, 2026-08-02, after garage colonies in Dobris
//! screened as 8 m walls): a height that is no per-building knowledge caps at one floor when a
//! garage-class or silent-class OSM building of comparable area stands within 15 m.

use crate::dev4::z30_corner_mercator_m;
use std::collections::HashMap;
use tiles::geo::WGS84_A_M;

/// (latitude, longitude, area) of low-class OSM buildings by bucket.
type LowProfileBuckets = HashMap<(i32, i32), Vec<(f64, f64, f32)>>;

/// Positions and areas of the low-class OSM buildings of one square, in ~55 m buckets.
#[derive(Default)]
pub struct LowProfileLookup {
    buckets: LowProfileBuckets,
}

impl LowProfileLookup {
    /// Bucket edge 1/2000 degree (~55 m).
    const GRID: f64 = 2000.0;
    const MATCH_M: f64 = 15.0;
    const AREA_RATIO: (f32, f32) = (0.4, 2.5);
    /// One floor, the structures builder's floor height.
    const LOW_HEIGHT_M: f32 = 3.0;
    /// Settlement classes 7 (garage, carport, parking) and 10 (silent: shed, roof, hut,
    /// greenhouse, container), the structurally low tail.
    const LOW_CLASSES: [u8; 2] = [7, 10];

    /// Records one OSM building; any other class is ignored, so the class rule stays here.
    pub fn insert_if_low(&mut self, building_type: u8, lat: f64, lon: f64, area_m2: f32) {
        if !Self::LOW_CLASSES.contains(&building_type) {
            return;
        }
        let key = (
            (lat * Self::GRID).floor() as i32,
            (lon * Self::GRID).floor() as i32,
        );
        self.buckets
            .entry(key)
            .or_default()
            .push((lat, lon, area_m2));
    }

    /// The height, capped at one floor when it is no per-building knowledge and a matching low
    /// building sits at (nearly) the same spot with a comparable footprint. The answer on any
    /// match is the constant, so the unordered buckets cannot change it.
    pub fn capped_height(
        &self,
        height_m: f32,
        height_is_per_building: bool,
        lat: f64,
        lon: f64,
        area_m2: f32,
    ) -> f32 {
        if height_is_per_building || height_m <= Self::LOW_HEIGHT_M || self.buckets.is_empty() {
            return height_m;
        }
        let key_lat = (lat * Self::GRID).floor() as i32;
        let key_lon = (lon * Self::GRID).floor() as i32;
        let m_per_deg_lon = 111_320.0 * lat.to_radians().cos().max(0.1);
        for dy in -1..=1 {
            for dx in -1..=1 {
                let Some(rows) = self.buckets.get(&(key_lat + dy, key_lon + dx)) else {
                    continue;
                };
                for &(building_lat, building_lon, building_area) in rows {
                    let north_m = (building_lat - lat) * 111_320.0;
                    let east_m = (building_lon - lon) * m_per_deg_lon;
                    if north_m * north_m + east_m * east_m > Self::MATCH_M * Self::MATCH_M {
                        continue;
                    }
                    let ratio = if building_area > 0.0 {
                        area_m2 / building_area
                    } else {
                        f32::MAX
                    };
                    if ratio >= Self::AREA_RATIO.0 && ratio <= Self::AREA_RATIO.1 {
                        return Self::LOW_HEIGHT_M;
                    }
                }
            }
        }
        height_m
    }
}

/// dev4 `height_is_per_building`: sources 2 (a footprint-area typology) and 4 (the retired GHSL
/// cell average) know nothing about the individual building; every other source measured or
/// mapped it.
pub fn height_is_per_building(height_source: u8) -> bool {
    !matches!(height_source, 2 | 4)
}

/// dev4 `ring_area_m2`: the Web Mercator shoelace of the z30 corners times cos^2 of their mean
/// latitude, at least 1 m^2 (the low-profile cap's area when the row has none).
pub fn ring_area_m2(ring: &[(i32, i32)]) -> f64 {
    let points: Vec<[f64; 2]> = ring
        .iter()
        .map(|&(gx, gy)| z30_corner_mercator_m(gx, gy))
        .collect();
    let mean_y = points.iter().map(|p| p[1]).sum::<f64>() / points.len() as f64;
    let mean_lat_deg =
        (2.0 * (mean_y / WGS84_A_M).exp().atan() - std::f64::consts::FRAC_PI_2).to_degrees();
    let cos = mean_lat_deg.to_radians().cos();
    let n = points.len();
    let shoelace: f64 = (0..n)
        .map(|i| points[i][0] * points[(i + 1) % n][1] - points[(i + 1) % n][0] * points[i][1])
        .sum();
    ((shoelace / 2.0).abs() * cos * cos).max(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A defaulted height caps only when a low-class building matches by position and
    /// comparable area; per-building heights, far buildings, other classes and wild area ratios
    /// keep the original height.
    #[test]
    fn low_profile_cap_matrix() {
        let (lat, lon) = (49.7778, 14.1636);
        let mut lookup = LowProfileLookup::default();
        lookup.insert_if_low(7, lat, lon, 22.0);
        lookup.insert_if_low(1, lat, lon, 22.0);
        assert_eq!(lookup.capped_height(8.0, false, lat, lon, 24.0), 3.0);
        assert_eq!(lookup.capped_height(14.5, false, lat, lon, 24.0), 3.0);
        assert_eq!(lookup.capped_height(8.0, true, lat, lon, 24.0), 8.0);
        assert_eq!(lookup.capped_height(21.4, true, lat, lon, 24.0), 21.4);
        assert_eq!(
            lookup.capped_height(8.0, false, lat + 0.0003, lon, 24.0),
            8.0
        );
        assert_eq!(lookup.capped_height(8.0, false, lat, lon, 600.0), 8.0);
        assert_eq!(lookup.capped_height(2.5, false, lat, lon, 24.0), 2.5);
        let empty = LowProfileLookup::default();
        assert_eq!(empty.capped_height(8.0, false, lat, lon, 24.0), 8.0);
        assert!(!height_is_per_building(2) && !height_is_per_building(4));
        assert!(height_is_per_building(0) && height_is_per_building(5));
    }

    #[test]
    fn the_cap_area_of_a_ring_is_its_ground_area() {
        // A 20 x 20 m square at 50 N in z30 corners: 20 m of ground is 20 / cos(50 deg) Mercator m.
        let (gx, gy) = (579_939_889, 709_936_176);
        let side = (20.0 / 50f64.to_radians().cos() / 0.037_322_767_717_044_72).round() as i32;
        let square = [
            (gx, gy),
            (gx + side, gy),
            (gx + side, gy + side),
            (gx, gy + side),
        ];
        let area = ring_area_m2(&square);
        assert!((area - 400.0).abs() < 8.0, "{area}");
        assert_eq!(ring_area_m2(&[(gx, gy), (gx + 1, gy), (gx, gy + 1)]), 1.0);
    }
}
