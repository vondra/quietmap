//! LOW-PROFILE height cap for defaulted obstacle heights (2026-08-02, Dobříš
//! garage-colony finding).
//!
//! Overture obstacle rows carry no building class, so a footprint with no mapped
//! height defaulted to 8 m (`height_tier == 2`) even when it is a garage /
//! carport / shed / greenhouse row that really stands ~2.5–3 m — hundreds of
//! phantom 8 m walls in a 200 m grid over-screen entire neighbourhoods. OSM (via
//! the cell's `buildings.arrow`) DOES know the class; a defaulted obstacle whose
//! centroid sits within `MATCH_M` of a low-profile OSM building with a
//! comparable footprint area is capped at `LOW_HEIGHT_M` (= one floor, the same
//! constant family as the ingest ladder). Applied at LOAD time so the whole world heals without
//! re-staging the obstacle store. Deterministic despite the unordered buckets:
//! the answer is the CONSTANT `LOW_HEIGHT_M` on any match, so which candidate
//! matched first cannot change it.
//!
//! The rule lives HERE because two loaders apply it — the tile painter's
//! `source_loader_obstacle` and the popup's `obstacle_store` — and they must
//! apply it identically or popup ≠ tiles at every capped footprint. Only the
//! Arrow plumbing differs between them, and that stays in each crate; the class
//! list, the match geometry and the cap are one definition.

use std::collections::HashMap;

/// (lat, lon, area_m2) rows bucketed by the ~55 m spatial-hash key.
type LowProfileBuckets = HashMap<(i32, i32), Vec<(f64, f64, f32)>>;

/// ~55 m spatial hash over (lat, lon) → (centroid, area_m2) of low-class OSM
/// buildings. Empty when the cell has no `buildings.arrow` (ML-only coverage) —
/// then nothing is capped, exactly the pre-fix behavior.
#[derive(Default)]
pub struct LowProfileLookup {
    buckets: LowProfileBuckets,
}

impl LowProfileLookup {
    const GRID: f64 = 2000.0; // 1/2000° ≈ 55 m bucket edge
    const MATCH_M: f64 = 15.0;
    const AREA_RATIO: (f32, f32) = (0.4, 2.5);
    const LOW_HEIGHT_M: f32 = 3.0; // = ingest FLOOR_HEIGHT (one floor)
    /// Settlement classes that are structurally low: 7 = garage/carport/
    /// parking, SILENT (10) = shed/roof/hut/greenhouse/container/… (the
    /// emission §C′ tail — also the structurally-low tail).
    const LOW_CLASSES: [u8; 2] = [7, crate::emission::settlement::SILENT];

    /// Record one OSM building row; rows of any other class are ignored, so a
    /// loader hands over every row it reads and the class rule stays here.
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

    /// Cap a DEFAULTED height when a matching low-profile OSM building sits at
    /// (nearly) the same spot with a comparable footprint.
    pub fn capped_height(&self, height_m: f32, tier: u8, lat: f64, lon: f64, area_m2: f32) -> f32 {
        if tier != 2 || height_m <= Self::LOW_HEIGHT_M || self.buckets.is_empty() {
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
                for &(blat, blon, barea) in rows {
                    let dm_lat = (blat - lat) * 111_320.0;
                    let dm_lon = (blon - lon) * m_per_deg_lon;
                    if dm_lat * dm_lat + dm_lon * dm_lon > Self::MATCH_M * Self::MATCH_M {
                        continue;
                    }
                    let ratio = if barea > 0.0 {
                        area_m2 / barea
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The cap's decision matrix (2026-08-02 garage-colony fix): a DEFAULTED
    /// (tier 2) height caps to 3 m only when a low-class OSM building matches by
    /// centroid AND comparable area; mapped heights (tier 0/1), far buildings,
    /// high classes and wild area ratios all keep the original height.
    #[test]
    fn low_profile_cap_matrix() {
        let (lat, lon) = (49.7778, 14.1636);
        let mut lookup = LowProfileLookup::default();
        // One 22 m² garage; the high-class row is not recorded at all, so it
        // can never cap.
        lookup.insert_if_low(7, lat, lon, 22.0);
        lookup.insert_if_low(1, lat, lon, 22.0);

        // Defaulted 8 m footprint on the garage → capped to 3 m.
        assert_eq!(lookup.capped_height(8.0, 2, lat, lon, 24.0), 3.0);
        // Mapped height (tier 0) never caps, even at the same spot.
        assert_eq!(lookup.capped_height(8.0, 0, lat, lon, 24.0), 8.0);
        // Floors-derived (tier 1) never caps.
        assert_eq!(lookup.capped_height(9.0, 1, lat, lon, 24.0), 9.0);
        // 30 m away — outside MATCH_M — keeps the default.
        assert_eq!(lookup.capped_height(8.0, 2, lat + 0.0003, lon, 24.0), 8.0);
        // A big hall (600 m²) over a tiny garage row is NOT comparable.
        assert_eq!(lookup.capped_height(8.0, 2, lat, lon, 600.0), 8.0);
        // Already low stays untouched.
        assert_eq!(lookup.capped_height(2.5, 2, lat, lon, 24.0), 2.5);
        // Empty lookup (no buildings.arrow) = pre-fix behavior.
        let empty = LowProfileLookup::default();
        assert_eq!(empty.capped_height(8.0, 2, lat, lon, 24.0), 8.0);
    }
}
