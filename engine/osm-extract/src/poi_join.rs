//! POI-in-footprint spatial join for settlement source classification.
//!
//! Standalone `amenity=`/`shop=`/`tourism=`/`healthcare=` NODES that fall inside
//! a `building=yes` footprint carry the building's real function (a supermarket
//! mapped as a point inside an untyped block). The extractor spills these as
//! `poi_<bucket>.tsv` (`hex_id, lat, lon, class`); finalize loads the bucket's
//! POIs, indexes them by hex, and — for every building it writes — reclassifies
//! the footprint when a POI sits inside it.
//!
//! A POI never downgrades an explicitly typed building — only
//! the residential default (`building_type == 0`, i.e. `building=yes`/
//! apartments) is eligible. Among multiple interior POIs the highest
//! [`poi_priority`] wins (school > hospital > food-retail > hospitality >
//! commercial …). The building is indexed by its WKB BBOX (not centroid — a POI
//! can sit far from the centroid of a large footprint, Codex note), then a
//! point-in-polygon test confirms containment.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use noise_compute::emission::settlement as st;
use noise_compute::wkb::WkbFootprint;

/// Per-hex POI points `(lat, lon, class)`, keyed by the R4 hex_id the POI hashed
/// to (same hex as any building it sits inside, so the join is within-hex).
#[derive(Default)]
pub struct PoiIndex {
    by_hex: HashMap<u64, Vec<(f64, f64, u8)>>,
}

impl PoiIndex {
    /// Parse a sorted `poi_<bucket>.tsv` (`hex_id, lat, lon, class`). Malformed
    /// rows are skipped (lenient — the join is best-effort coverage).
    pub fn from_lines(lines: impl Iterator<Item = String>) -> Self {
        let mut by_hex: HashMap<u64, Vec<(f64, f64, u8)>> = HashMap::new();
        for line in lines {
            let mut it = line.split('\t');
            let (Some(h), Some(lat), Some(lon), Some(class)) =
                (it.next(), it.next(), it.next(), it.next())
            else {
                continue;
            };
            if let (Ok(h), Ok(lat), Ok(lon), Ok(class)) = (
                h.parse::<u64>(),
                lat.parse::<f64>(),
                lon.parse::<f64>(),
                class.parse::<u8>(),
            ) {
                by_hex.entry(h).or_default().push((lat, lon, class));
            }
        }
        Self { by_hex }
    }

    fn hex_pois(&self, hex: u64) -> &[(f64, f64, u8)] {
        self.by_hex.get(&hex).map_or(&[], |v| v.as_slice())
    }
}

/// Reclassification priority for a POI class — higher wins when several POIs sit
/// in one footprint. School/hospital are the most consequential (deep night
/// cuts / 24/7 plant), then food-retail (24/7 refrigeration), then hospitality,
/// then everything else. SILENT never wins (a POI implies activity).
fn poi_priority(class: u8) -> u8 {
    match class {
        3 => 6, // school
        4 => 5, // hospital
        c if c == st::FOOD_RETAIL => 4,
        c if c == st::HOSPITALITY => 3,
        5 => 2, // worship
        6 => 2, // hotel
        st::SILENT => 0,
        _ => 1, // commercial / public / garage
    }
}

/// Join counters (atomic so the parallel buckets can share one set).
#[derive(Default)]
pub struct JoinStats {
    pub buildings_checked: AtomicU64,
    pub reclassified: AtomicU64,
}

impl JoinStats {
    pub fn report(&self) -> (u64, u64) {
        (
            self.buildings_checked.load(Ordering::Relaxed),
            self.reclassified.load(Ordering::Relaxed),
        )
    }
}

/// The building class after the POI join. `current` is the building's own-tag
/// class; `wkb_hex` its footprint. Returns the reclassified class (or `current`
/// unchanged). Only the residential default (class 0) is eligible — an
/// explicitly-typed building is never downgraded.
pub fn joined_building_type(
    current: u8,
    wkb_hex: &str,
    hex: u64,
    index: &PoiIndex,
    stats: &JoinStats,
) -> u8 {
    // Only `building=yes`/apartments (the un-explicit default) can be upgraded.
    if current != 0 {
        return current;
    }
    let pois = index.hex_pois(hex);
    if pois.is_empty() {
        return current;
    }
    let Some(footprint) = WkbFootprint::parse(wkb_hex) else {
        return current;
    };
    stats.buildings_checked.fetch_add(1, Ordering::Relaxed);
    let (min_lat, max_lat, min_lon, max_lon) = footprint.bbox();
    let mut best: Option<(u8, u8)> = None; // (priority, class)
    for &(lat, lon, class) in pois {
        if lat < min_lat || lat > max_lat || lon < min_lon || lon > max_lon {
            continue;
        }
        if !footprint.contains(lat, lon) {
            continue;
        }
        let pri = poi_priority(class);
        if best.is_none_or(|(bp, _)| pri > bp) {
            best = Some((pri, class));
        }
    }
    match best {
        Some((_, class)) if class != current => {
            stats.reclassified.fetch_add(1, Ordering::Relaxed);
            class
        }
        _ => current,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A unit square ~ (50.0..50.001, 14.0..14.001) as WKB hex.
    fn square_wkb() -> String {
        let ring: [(f64, f64); 5] = [
            (50.0, 14.0),
            (50.0, 14.001),
            (50.001, 14.001),
            (50.001, 14.0),
            (50.0, 14.0),
        ];
        let n = ring.len() as u32;
        let mut w: Vec<u8> = Vec::new();
        w.push(1);
        w.extend_from_slice(&3u32.to_le_bytes()); // Polygon
        w.extend_from_slice(&1u32.to_le_bytes()); // 1 ring
        w.extend_from_slice(&n.to_le_bytes());
        for (lat, lon) in ring {
            w.extend_from_slice(&lon.to_le_bytes());
            w.extend_from_slice(&lat.to_le_bytes());
        }
        w.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn poi_inside_yes_building_reclassifies() {
        let idx = PoiIndex::from_lines(
            [format!("123\t50.0005\t14.0005\t{}", st::FOOD_RETAIL)].into_iter(),
        );
        let stats = JoinStats::default();
        let out = joined_building_type(0, &square_wkb(), 123, &idx, &stats);
        assert_eq!(out, st::FOOD_RETAIL);
        assert_eq!(stats.report(), (1, 1));
    }

    #[test]
    fn poi_outside_footprint_is_ignored() {
        let idx = PoiIndex::from_lines(["123\t50.5\t14.5\t4".to_string()].into_iter());
        let stats = JoinStats::default();
        assert_eq!(joined_building_type(0, &square_wkb(), 123, &idx, &stats), 0);
        assert_eq!(stats.report().1, 0);
    }

    #[test]
    fn explicit_building_is_never_downgraded() {
        // building=hospital (4) with a cafe POI inside must stay 4.
        let idx = PoiIndex::from_lines(
            [format!("123\t50.0005\t14.0005\t{}", st::HOSPITALITY)].into_iter(),
        );
        let stats = JoinStats::default();
        assert_eq!(joined_building_type(4, &square_wkb(), 123, &idx, &stats), 4);
    }

    #[test]
    fn highest_priority_poi_wins() {
        // A hospitality + a school inside the same footprint → school (higher).
        let idx = PoiIndex::from_lines(
            [
                format!("123\t50.0003\t14.0003\t{}", st::HOSPITALITY),
                "123\t50.0006\t14.0006\t3".to_string(),
            ]
            .into_iter(),
        );
        let stats = JoinStats::default();
        assert_eq!(joined_building_type(0, &square_wkb(), 123, &idx, &stats), 3);
    }
}
