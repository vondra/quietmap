//! A main road's count on the way beside it: a conflation of published counts put it on a service
//! road, a side lane, a residential street or a ramp along the counted road, which then carried
//! the main road's traffic a second time (London's Station Approach carried Marylebone Road's 43,031
//! vehicles a day beside the road's own 42,783; 792 such ways in 153 squares, Brno's city counts
//! 437 of them, the EU city set 207, Amsterdam's traffic model 121, 2026-10-17,
//! `evidence/2026-10-17/main-road-copies/`); or a road's side lane of its name anywhere in the
//! square (Madrid's Castellana side lane carried the main carriageway's 35,989 vehicles a day, the
//! station by it read 5 dB too loud). Such a row takes its own model, as an uncounted row does.

use crate::rings::z30_cells;
use std::collections::{HashMap, HashSet};

/// How near a copy lies to its main road (m): the side lane, service road or ramp along it.
const COPY_REACH_M: f64 = 30.0;
/// How close a copy's count is to the main road's: of 1,218 counted minor ways carrying half a
/// neighbouring main road's traffic or more, 792 lie within 10 % of it (557 exactly equal); those
/// between half and 0.8 of it are busy streets of their own (Marseille, Paris, Madrid).
const COPY_TOLERANCE: f64 = 0.1;
/// dev4's main classes, motorway to tertiary: a count of one of them is the road's own.
const MAIN_CLASSES: std::ops::RangeInclusive<u8> = 0..=4;

/// A row with a count of its own: its table row, dev4 class, name, ends (z30), daily vehicles and
/// the cross-section of its road (both carriageways; its own count where dev4 gives none).
pub struct CountedRow {
    pub row: usize,
    pub class: u8,
    pub name: String,
    pub ends: [(i32, i32); 2],
    pub vehicles: f64,
    pub cross_section: f64,
}

/// The rows below the main classes whose count is within `COPY_TOLERANCE` of the count or the
/// cross-section of a main row within `COPY_REACH_M`, at `latitude_deg`, or is the very count of a
/// main row of its name.
pub fn copied_counts(rows: &[CountedRow], latitude_deg: f64) -> HashSet<usize> {
    let reach = f64::from(z30_cells(COPY_REACH_M, latitude_deg));
    // The main rows by the cells their reach covers, cells as wide as the reach.
    let cell = |z30: f64| (z30 / reach).floor() as i64;
    let cells_of = |[a, b]: [(i32, i32); 2]| {
        let (x0, x1) = (
            f64::from(a.0.min(b.0)) - reach,
            f64::from(a.0.max(b.0)) + reach,
        );
        let (y0, y1) = (
            f64::from(a.1.min(b.1)) - reach,
            f64::from(a.1.max(b.1)) + reach,
        );
        (cell(x0)..=cell(x1)).flat_map(move |x| (cell(y0)..=cell(y1)).map(move |y| (x, y)))
    };
    let mut main: HashMap<(i64, i64), Vec<&CountedRow>> = HashMap::new();
    let mut named: HashMap<&str, HashSet<i64>> = HashMap::new();
    for row in rows.iter().filter(|row| MAIN_CLASSES.contains(&row.class)) {
        for at in cells_of(row.ends) {
            main.entry(at).or_default().push(row);
        }
        if !row.name.is_empty() {
            named
                .entry(&row.name)
                .or_default()
                .insert(row.vehicles.round() as i64);
        }
    }
    let repeats = |vehicles: f64, of: f64| of > 0.0 && (vehicles - of).abs() <= COPY_TOLERANCE * of;
    rows.iter()
        .filter(|row| !MAIN_CLASSES.contains(&row.class))
        .filter(|row| {
            named
                .get(row.name.as_str())
                .is_some_and(|counts| counts.contains(&(row.vehicles.round() as i64)))
                || cells_of(row.ends)
                    .filter_map(|at| main.get(&at))
                    .flatten()
                    .any(|road| {
                        (repeats(row.vehicles, road.cross_section)
                            || repeats(row.vehicles, road.vehicles))
                            && segments_apart(row.ends, road.ends) <= reach
                    })
        })
        .map(|row| row.row)
        .collect()
}

/// The least distance between two segments (z30 cells): none where they cross, else the nearest
/// end to the other segment.
fn segments_apart(a: [(i32, i32); 2], b: [(i32, i32); 2]) -> f64 {
    let point = |(x, y): (i32, i32)| (f64::from(x), f64::from(y));
    let (a0, a1, b0, b1) = (point(a[0]), point(a[1]), point(b[0]), point(b[1]));
    let side = |p: (f64, f64), q: (f64, f64), r: (f64, f64)| {
        ((q.0 - p.0) * (r.1 - p.1) - (q.1 - p.1) * (r.0 - p.0)).signum()
    };
    let crossing =
        side(a0, a1, b0) * side(a0, a1, b1) < 0.0 && side(b0, b1, a0) * side(b0, b1, a1) < 0.0;
    if crossing {
        return 0.0;
    }
    [
        point_to_segment(a0, (b0, b1)),
        point_to_segment(a1, (b0, b1)),
        point_to_segment(b0, (a0, a1)),
        point_to_segment(b1, (a0, a1)),
    ]
    .into_iter()
    .fold(f64::INFINITY, f64::min)
}

fn point_to_segment(p: (f64, f64), (a, b): ((f64, f64), (f64, f64))) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length_squared = dx * dx + dy * dy;
    let t = if length_squared > 0.0 {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / length_squared).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p.0 - a.0 - t * dx).hypot(p.1 - a.1 - t * dy)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Marylebone Road's pattern at 51.5° N: the service road 3 m beside the counted road with its
    /// cross-section is a copy, and so is a ramp meeting it with its carriageway's count; a service
    /// road with traffic of its own, a street of the same count 100 m away and the main road's
    /// own rows are not.
    #[test]
    fn a_minor_way_beside_a_main_road_with_its_count_is_a_copy() {
        let m = 1.0 / (crate::dev4::Z30_QUANTUM_M * 51.5f64.to_radians().cos());
        let at = |x_m: f64, y_m: f64| ((x_m * m) as i32, (y_m * m) as i32);
        let row = |row, class, ends: [(f64, f64); 2], vehicles, cross_section| CountedRow {
            row,
            class,
            name: String::new(),
            ends: ends.map(|(x, y)| at(x, y)),
            vehicles,
            cross_section,
        };
        let rows = [
            row(0, 1, [(0.0, 0.0), (200.0, 0.0)], 21_392.0, 42_783.0),
            row(1, 1, [(0.0, 12.0), (200.0, 12.0)], 21_391.0, 42_783.0),
            row(2, 7, [(20.0, -3.0), (180.0, -3.0)], 43_031.0, 43_031.0),
            row(3, 11, [(200.0, 12.0), (260.0, 40.0)], 21_000.0, 21_000.0),
            row(4, 7, [(20.0, -5.0), (180.0, -5.0)], 4_000.0, 4_000.0),
            row(5, 5, [(20.0, -100.0), (180.0, -100.0)], 43_000.0, 43_000.0),
        ];
        let mut copies: Vec<usize> = copied_counts(&rows, 51.5).into_iter().collect();
        copies.sort_unstable();
        assert_eq!(copies, [2, 3]);
    }

    /// Madrid's Castellana: a side lane of the road's name 300 m from the counted carriageway
    /// with its very count is a copy; one of another count, or of another name, is not.
    #[test]
    fn a_side_lane_with_its_roads_name_and_count_is_a_copy_wherever_it_lies() {
        let m = 1.0 / (crate::dev4::Z30_QUANTUM_M * 40.4f64.to_radians().cos());
        let at = |x_m: f64, y_m: f64| ((x_m * m) as i32, (y_m * m) as i32);
        let row = |row, class, name: &str, y_m, vehicles| CountedRow {
            row,
            class,
            name: name.into(),
            ends: [at(0.0, y_m), at(200.0, y_m)],
            vehicles,
            cross_section: vehicles,
        };
        let rows = [
            row(0, 1, "Paseo de la Castellana", 0.0, 35_989.0),
            row(1, 5, "Paseo de la Castellana", 300.0, 35_989.0),
            row(2, 5, "Paseo de la Castellana", 400.0, 12_000.0),
            row(3, 5, "Calle de Serrano", 500.0, 35_989.0),
        ];
        assert_eq!(copied_counts(&rows, 40.4), HashSet::from([1]));
    }
}
