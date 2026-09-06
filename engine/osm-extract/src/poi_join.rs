//! POI-in-footprint spatial join for settlement source classification.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::ids;
use crate::spill::spill_key;
use grid::poly::PreparedRing;
use grid::Square;

/// Per-square POI points `(gx, gy, class)`, keyed by their spill square id.
#[derive(Default)]
pub struct PoiIndex {
    by_square: HashMap<u32, Vec<(i32, i32, u8)>>,
}

impl PoiIndex {
    /// Parse a `poi_<bucket>.tsv` (`sq, gx, gy, class`). Malformed rows are
    /// skipped (lenient — the join is best-effort coverage).
    pub fn from_lines(lines: impl Iterator<Item = String>) -> Self {
        let mut by_square: HashMap<u32, Vec<(i32, i32, u8)>> = HashMap::new();
        for line in lines {
            let mut it = line.split('\t');
            let (Some(s), Some(gx), Some(gy), Some(class)) =
                (it.next(), it.next(), it.next(), it.next())
            else {
                continue;
            };
            if let (Ok(s), Ok(gx), Ok(gy), Ok(class)) = (
                s.parse::<u32>(),
                gx.parse::<i32>(),
                gy.parse::<i32>(),
                class.parse::<u8>(),
            ) {
                by_square.entry(s).or_default().push((gx, gy, class));
            }
        }
        Self { by_square }
    }

    fn square_pois(&self, square: Square) -> &[(i32, i32, u8)] {
        self.by_square
            .get(&spill_key(square))
            .map_or(&[], |v| v.as_slice())
    }
}

/// Reclassification priority for a POI class — higher wins when several POIs sit
/// in one footprint. School/hospital are the most consequential, then
/// food-retail, then hospitality, then everything else. SILENT never wins
/// (a POI implies activity).
fn poi_priority(class: u8) -> u8 {
    match class {
        3 => 6, // school
        4 => 5, // hospital
        c if c == ids::SETTLEMENT_FOOD_RETAIL => 4,
        c if c == ids::SETTLEMENT_HOSPITALITY => 3,
        5 => 2, // worship
        6 => 2, // hotel
        c if c == ids::SETTLEMENT_SILENT => 0,
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
/// class; `footprint` is requested only for an eligible building with POIs.
/// Only the residential default (class 0) is eligible.
pub fn joined_building_type<'a>(
    current: u8,
    footprint: impl FnOnce() -> Option<&'a PreparedRing>,
    square: Square,
    index: &PoiIndex,
    stats: &JoinStats,
) -> u8 {
    // Only `building=yes`/apartments (the un-explicit default) can be upgraded.
    if current != 0 {
        return current;
    }
    let pois = index.square_pois(square);
    if pois.is_empty() {
        return current;
    }
    let Some(footprint) = footprint() else {
        return current;
    };
    stats.buildings_checked.fetch_add(1, Ordering::Relaxed);
    let mut best: Option<(u8, u8)> = None; // (priority, class)
    for &(gx, gy, class) in pois {
        if !footprint.contains(gx, gy) {
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
    use grid::lonlat_to_grid;
    use std::cell::{Cell, LazyCell};

    // ~100×100 m square at Prague as a snapped ring.
    fn square_ring() -> Vec<(i32, i32)> {
        [
            (14.0, 50.0),
            (14.001_394, 50.0),
            (14.001_394, 50.000_904),
            (14.0, 50.000_904),
        ]
        .iter()
        .map(|&(lon, lat)| lonlat_to_grid(lon, lat))
        .collect()
    }

    fn poi_line(sq: u32, lon: f64, lat: f64, class: u8) -> String {
        let (gx, gy) = lonlat_to_grid(lon, lat);
        format!("{sq}\t{gx}\t{gy}\t{class}")
    }

    #[test]
    fn poi_inside_yes_building_reclassifies() {
        let sq = grid::square_of(50.0005, 14.0005);
        let idx = PoiIndex::from_lines(
            [poi_line(
                sq.y as u32 * 512 + sq.x as u32,
                14.0005,
                50.0005,
                ids::SETTLEMENT_FOOD_RETAIL,
            )]
            .into_iter(),
        );
        let stats = JoinStats::default();
        let footprint = LazyCell::new(|| PreparedRing::new(&square_ring()));
        let out = joined_building_type(0, || footprint.as_ref(), sq, &idx, &stats);
        assert_eq!(out, ids::SETTLEMENT_FOOD_RETAIL);
        assert_eq!(stats.report(), (1, 1));
    }

    #[test]
    fn poi_outside_footprint_is_ignored() {
        let sq = grid::square_of(50.0, 14.0);
        let idx = PoiIndex::from_lines(
            [poi_line(sq.y as u32 * 512 + sq.x as u32, 14.5, 50.5, 4)].into_iter(),
        );
        let stats = JoinStats::default();
        let footprint = LazyCell::new(|| PreparedRing::new(&square_ring()));
        assert_eq!(
            joined_building_type(0, || footprint.as_ref(), sq, &idx, &stats),
            0
        );
        assert_eq!(stats.report().1, 0);
    }

    #[test]
    fn explicit_building_is_never_downgraded() {
        let sq = grid::square_of(50.0005, 14.0005);
        let idx = PoiIndex::from_lines(
            [poi_line(
                sq.y as u32 * 512 + sq.x as u32,
                14.0005,
                50.0005,
                ids::SETTLEMENT_HOSPITALITY,
            )]
            .into_iter(),
        );
        let stats = JoinStats::default();
        // building=hospital (4) with a cafe POI inside must stay 4.
        assert_eq!(
            joined_building_type(
                4,
                || panic!("typed building needs no footprint"),
                sq,
                &idx,
                &stats
            ),
            4
        );
        assert_eq!(
            joined_building_type(
                0,
                || panic!("empty POI index needs no footprint"),
                sq,
                &PoiIndex::default(),
                &stats
            ),
            0
        );
        assert_eq!(stats.report(), (0, 0));
    }

    #[test]
    fn highest_priority_poi_wins() {
        let sq = grid::square_of(50.0005, 14.0005);
        let id = sq.y as u32 * 512 + sq.x as u32;
        let idx = PoiIndex::from_lines(
            [
                poi_line(id, 14.0003, 50.0003, ids::SETTLEMENT_HOSPITALITY),
                poi_line(id, 14.0006, 50.0006, 3),
            ]
            .into_iter(),
        );
        let stats = JoinStats::default();
        let preparations = Cell::new(0);
        let footprint = LazyCell::new(|| {
            preparations.set(preparations.get() + 1);
            PreparedRing::new(&square_ring())
        });
        assert_eq!(
            joined_building_type(0, || footprint.as_ref(), sq, &idx, &stats),
            3
        );
        assert_eq!(preparations.get(), 1);
        assert_eq!(stats.report(), (1, 1));
    }
}
