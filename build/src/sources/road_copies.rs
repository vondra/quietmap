//! A main road's count on the way beside it: a conflation of published counts put it on a service
//! road, a side lane, a residential street or a ramp along the counted road, which then carried
//! the main road's traffic a second time (London's Station Approach carried Marylebone Road's 43,031
//! vehicles a day beside the road's own 42,783; 792 such ways in 153 squares, Brno's city counts
//! 437 of them, the EU city set 207, Amsterdam's traffic model 121, 2026-10-17,
//! `evidence/2026-10-17/main-road-copies/`); or a road's side lane of its name anywhere in the
//! square (Madrid's Castellana side lane carried the main carriageway's 35,989 vehicles a day, the
//! station by it read 5 dB too loud). A main road's tunnel counts as its road at the portals: dev4
//! keeps the source of a tunnel's count and zeroes its vehicles, as a tunnel emits nothing, and the
//! streets over it carried the count (Brno's Kohoutova the Husovice tunnel's 44,000 a day, Madrid
//! Río's avenues the M-30's 43,071 and 46,995; 117 ways in 6,771 squares with tunnels, 2026-10-18,
//! `evidence/2026-10-18/tunnel-copies/`). A main road's count also lies on main roads beside it:
//! on a road of a lower class running alongside within the reach (San Francisco's Bayshore
//! Boulevard, a primary road, carried US 101's 223,000 vehicles a day), and on a two-way row running
//! between one-way carriageways with their cross-section (the reversible and HOV lanes of Houston's,
//! Atlanta's and Chicago's freeways carried both carriageways' 160,000-320,000 a day on one or two
//! lanes; `oneway=reversible` is not one-way; 190.7 km in four US city squares, evidence
//! 2026-11-12, data-1). Such a row takes its own model, as an uncounted row does.

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
/// The classes a road's side lanes and squares are mapped as (residential, living street,
/// service): one with the road's name and very count is the road's wherever it lies. An
/// unclassified row or a link of the road's name and count elsewhere in the square is the road
/// going on (a census section across a change of class: Prague's links and Brno's unclassified
/// ways kept their counts in r072); within the reach of the road, as every lower class, a copy.
const SIDE_CLASSES: std::ops::RangeInclusive<u8> = 5..=7;
/// The classes whose count a side lane of their name repeats: the main ones and their links.
const NAMED_CLASSES: [std::ops::RangeInclusive<u8>; 2] = [0..=4, 10..=12];

/// A row with a count of its own: its table row, dev4 class, whether it is one-way, name, ends
/// (z30), its way's first and last OSM nodes, daily vehicles and the cross-section of its road (both
/// carriageways; its own count where dev4 gives none).
pub struct CountedRow {
    pub row: usize,
    pub class: u8,
    pub oneway: bool,
    pub name: String,
    pub ends: [(i32, i32); 2],
    pub nodes: [i64; 2],
    pub vehicles: f64,
    pub cross_section: f64,
}

/// A row in a tunnel: its dev4 class, ends (z30) and its way's first and last OSM nodes.
pub struct TunnelRow {
    pub class: u8,
    pub ends: [(i32, i32); 2],
    pub nodes: [i64; 2],
}

/// A main road's row or tunnel row: its ends (z30), the counts a copy of it repeats, and as a main
/// row its class and whether it is one-way.
struct Road {
    ends: [(i32, i32); 2],
    counts: Vec<f64>,
    main: Option<(u8, bool)>,
}

/// The rows below the main classes whose count is within `COPY_TOLERANCE` of the count or the
/// cross-section of a main row or a main road's tunnel within `COPY_REACH_M`, at `latitude_deg`;
/// the side lanes with the very count of a road of their name; and the main rows running alongside
/// a main road whose count they repeat: of a higher class, or of their own class as a two-way row
/// with the cross-section of one-way carriageways ([`alongside_copy`]).
pub fn copied_counts(
    rows: &[CountedRow],
    tunnels: &[TunnelRow],
    latitude_deg: f64,
) -> HashSet<usize> {
    let reach = f64::from(z30_cells(COPY_REACH_M, latitude_deg));
    // Cells as wide as the reach, and those a row's reach covers.
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
    // The main roads' rows and tunnels with the counts a copy repeats, by the cells their reach covers.
    let roads: Vec<Road> = rows
        .iter()
        .filter(|row| MAIN_CLASSES.contains(&row.class))
        .map(|row| Road {
            ends: row.ends,
            counts: vec![row.vehicles, row.cross_section],
            main: Some((row.class, row.oneway)),
        })
        .chain(tunnel_counts(rows, tunnels))
        .filter(|road| !road.counts.is_empty())
        .collect();
    let mut main: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    let mut named: HashMap<&str, HashSet<i64>> = HashMap::new();
    for (index, road) in roads.iter().enumerate() {
        for at in cells_of(road.ends) {
            main.entry(at).or_default().push(index);
        }
    }
    for row in rows.iter().filter(|row| {
        NAMED_CLASSES
            .iter()
            .any(|classes| classes.contains(&row.class))
    }) {
        if !row.name.is_empty() {
            named
                .entry(&row.name)
                .or_default()
                .insert(row.vehicles.round() as i64);
        }
    }
    let repeats = |vehicles: f64, of: f64| of > 0.0 && (vehicles - of).abs() <= COPY_TOLERANCE * of;
    let near = |row: &CountedRow| -> Vec<&Road> {
        let mut found: Vec<usize> = cells_of(row.ends)
            .filter_map(|at| main.get(&at))
            .flatten()
            .copied()
            .collect();
        found.sort_unstable();
        found.dedup();
        found.into_iter().map(|index| &roads[index]).collect()
    };
    rows.iter()
        .filter(|row| {
            if MAIN_CLASSES.contains(&row.class) {
                // A one-way motorway row has no busier class and no carriageways to sit between.
                return !(row.class == 0 && row.oneway) && alongside_copy(row, &near(row), reach);
            }
            SIDE_CLASSES.contains(&row.class)
                && named
                    .get(row.name.as_str())
                    .is_some_and(|counts| counts.contains(&(row.vehicles.round() as i64)))
                || near(row).iter().any(|road| {
                    road.counts.iter().any(|&of| repeats(row.vehicles, of))
                        && segments_apart(row.ends, road.ends) <= reach
                })
        })
        .map(|row| row.row)
        .collect()
}

/// Whether a main row repeats a main road it runs alongside: the rows of a higher class (a lower
/// dev4 code) whose count or cross-section it repeats, or, for a two-way row, the one-way rows of
/// its own class whose cross-section it repeats, running within 30 degrees of its direction, hold
/// both its ends within `reach` (z30 cells) and none touches it, so it runs beside them, not on from
/// them (a two-way road splitting into its carriageways meets them with their cross-section) nor
/// across them (a bridge over the motorway).
fn alongside_copy(row: &CountedRow, near: &[&Road], reach: f64) -> bool {
    let repeated: Vec<&&Road> = near
        .iter()
        .filter(|road| {
            let Some((class, oneway)) = road.main else {
                return false;
            };
            let of =
                |count: f64| count > 0.0 && (row.vehicles - count).abs() <= COPY_TOLERANCE * count;
            // A one-way row's cross-section repeats both directions only where dev4 paired it
            // (unpaired, it is the carriageway's own count).
            let repeats = if class < row.class {
                road.counts.iter().any(|&count| of(count))
            } else {
                class == row.class
                    && oneway
                    && !row.oneway
                    && road.counts[1] > road.counts[0]
                    && of(road.counts[1])
            };
            repeats && parallel(row.ends, road.ends)
        })
        .collect();
    let point = |(x, y): (i32, i32)| (f64::from(x), f64::from(y));
    let touches = repeated
        .iter()
        .any(|road| road.ends.iter().any(|end| row.ends.contains(end)));
    let held = |end: (i32, i32)| {
        repeated.iter().any(|road| {
            point_to_segment(point(end), (point(road.ends[0]), point(road.ends[1]))) <= reach
        })
    };
    !repeated.is_empty() && !touches && row.ends.iter().all(|&end| held(end))
}

/// Whether two segments (z30) run within 30 degrees of each other, either way.
fn parallel(a: [(i32, i32); 2], b: [(i32, i32); 2]) -> bool {
    let along = |[p, q]: [(i32, i32); 2]| (f64::from(q.0 - p.0), f64::from(q.1 - p.1));
    let (u, v) = (along(a), along(b));
    let cross = u.0 * v.1 - u.1 * v.0;
    let dot = u.0 * v.0 + u.1 * v.1;
    dot.abs() >= 3f64.sqrt() * cross.abs() && dot != 0.0
}

/// The rows of main roads' tunnels (a main class or a link) with their road's counts at the
/// portals: the vehicles and cross-sections of the counted main rows whose ways meet the tunnel's
/// chain of ways at a node (Husovice's portals carry 22,000 a carriageway, 44,000 both, the count
/// of Kohoutova over it). Not a link's: a counted ramp at a portal would repeat its own count.
fn tunnel_counts(rows: &[CountedRow], tunnels: &[TunnelRow]) -> Vec<Road> {
    fn root(chain: &mut [usize], mut at: usize) -> usize {
        while chain[at] != at {
            chain[at] = chain[chain[at]];
            at = chain[at];
        }
        at
    }
    let tunnels: Vec<&TunnelRow> = tunnels
        .iter()
        .filter(|tunnel| {
            NAMED_CLASSES
                .iter()
                .any(|classes| classes.contains(&tunnel.class))
        })
        .collect();
    // The chains: the tunnel ways that share a node.
    let mut chain: Vec<usize> = (0..tunnels.len()).collect();
    let mut at_node: HashMap<i64, usize> = HashMap::new();
    for (index, tunnel) in tunnels.iter().enumerate() {
        for node in tunnel.nodes {
            let other = *at_node.entry(node).or_insert(index);
            let (a, b) = (root(&mut chain, index), root(&mut chain, other));
            chain[a] = b;
        }
    }
    let mut portals: HashMap<usize, Vec<f64>> = HashMap::new();
    for row in rows.iter().filter(|row| MAIN_CLASSES.contains(&row.class)) {
        for node in row.nodes {
            if let Some(&tunnel) = at_node.get(&node) {
                let counts = portals.entry(root(&mut chain, tunnel)).or_default();
                counts.extend([row.vehicles, row.cross_section]);
            }
        }
    }
    // Each portal's counts once: every row of a portal way meets the chain with them.
    for counts in portals.values_mut() {
        counts.sort_by(f64::total_cmp);
        counts.dedup();
    }
    (0..tunnels.len())
        .map(|index| Road {
            ends: tunnels[index].ends,
            counts: portals
                .get(&root(&mut chain, index))
                .cloned()
                .unwrap_or_default(),
            main: None,
        })
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
            oneway: matches!(class, 1 | 11),
            name: String::new(),
            ends: ends.map(|(x, y)| at(x, y)),
            nodes: [2 * row as i64, 2 * row as i64 + 1],
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
        let mut copies: Vec<usize> = copied_counts(&rows, &[], 51.5).into_iter().collect();
        copies.sort_unstable();
        assert_eq!(copies, [2, 3]);
    }

    /// Madrid's Castellana: a side lane of the road's name 300 m from the counted carriageway
    /// with its very count is a copy; one of another count, of another name, or the road going on
    /// unclassified, is not.
    #[test]
    fn a_side_lane_with_its_roads_name_and_count_is_a_copy_wherever_it_lies() {
        let m = 1.0 / (crate::dev4::Z30_QUANTUM_M * 40.4f64.to_radians().cos());
        let at = |x_m: f64, y_m: f64| ((x_m * m) as i32, (y_m * m) as i32);
        let row = |row, class, name: &str, y_m, vehicles| CountedRow {
            row,
            class,
            oneway: false,
            name: name.into(),
            ends: [at(0.0, y_m), at(200.0, y_m)],
            nodes: [2 * row as i64, 2 * row as i64 + 1],
            vehicles,
            cross_section: vehicles,
        };
        let rows = [
            row(0, 1, "Paseo de la Castellana", 0.0, 35_989.0),
            row(1, 5, "Paseo de la Castellana", 300.0, 35_989.0),
            row(2, 5, "Paseo de la Castellana", 400.0, 12_000.0),
            row(3, 5, "Calle de Serrano", 500.0, 35_989.0),
            row(4, 9, "Paseo de la Castellana", 600.0, 35_989.0),
        ];
        assert_eq!(copied_counts(&rows, &[], 40.4), HashSet::from([1]));
    }

    /// Brno's Husovice tunnel at 49.2° N: a trunk in two tunnel ways, counted outside its portal
    /// with 22,000 a carriageway and 44,000 both; the street over it with either count is a copy, a
    /// street beside it with 5,000 is not, nor one over a tunnel whose portals carry no count.
    #[test]
    fn a_street_over_a_tunnel_with_the_count_at_its_portals_is_a_copy() {
        let m = 1.0 / (crate::dev4::Z30_QUANTUM_M * 49.2f64.to_radians().cos());
        let at = |x_m: f64, y_m: f64| ((x_m * m) as i32, (y_m * m) as i32);
        let row =
            |row, class, [x0, x1]: [f64; 2], y_m, nodes, vehicles, cross_section| CountedRow {
                row,
                class,
                oneway: class == 1,
                name: String::new(),
                ends: [at(x0, y_m), at(x1, y_m)],
                nodes,
                vehicles,
                cross_section,
            };
        let tunnel = |[x0, x1]: [f64; 2], nodes| TunnelRow {
            class: 1,
            ends: [at(x0, 0.0), at(x1, 0.0)],
            nodes,
        };
        let rows = [
            row(0, 1, [480.0, 600.0], 0.0, [3, 4], 22_000.0, 44_000.0),
            row(1, 5, [50.0, 250.0], 5.0, [10, 11], 44_000.0, 44_000.0),
            row(2, 5, [50.0, 250.0], -8.0, [12, 13], 22_000.0, 44_000.0),
            row(3, 5, [50.0, 250.0], 15.0, [14, 15], 5_000.0, 5_000.0),
            row(4, 5, [1_000.0, 1_200.0], 5.0, [16, 17], 44_000.0, 44_000.0),
        ];
        let tunnels = [
            tunnel([0.0, 300.0], [1, 2]),
            tunnel([300.0, 480.0], [2, 3]),
            tunnel([1_000.0, 1_200.0], [20, 21]),
        ];
        assert_eq!(copied_counts(&rows, &tunnels, 49.2), HashSet::from([1, 2]));
    }

    /// US 101 at San Francisco (37.7° N): its carriageways carry 111,500 each and 223,000 both; the
    /// primary road 20 m beside them with 223,000 is a copy, as is the two-way express lane
    /// between them; a primary road with a count of its own, a two-way motorway row splitting into
    /// the carriageways (it touches them), a primary road bridging them with their very count and
    /// the carriageways themselves are not.
    #[test]
    fn a_main_road_beside_a_busier_one_with_its_count_is_a_copy() {
        let m = 1.0 / (crate::dev4::Z30_QUANTUM_M * 37.7f64.to_radians().cos());
        let at = |x_m: f64, y_m: f64| ((x_m * m) as i32, (y_m * m) as i32);
        let row =
            |row, class, oneway, [a, b]: [(f64, f64); 2], vehicles: f64, cross_section: f64| {
                CountedRow {
                    row,
                    class,
                    oneway,
                    name: String::new(),
                    ends: [at(a.0, a.1), at(b.0, b.1)],
                    nodes: [2 * row as i64, 2 * row as i64 + 1],
                    vehicles,
                    cross_section,
                }
            };
        let rows = [
            row(0, 0, true, [(0.0, 0.0), (400.0, 0.0)], 111_500.0, 223_000.0),
            row(
                1,
                0,
                true,
                [(0.0, 20.0), (400.0, 20.0)],
                111_500.0,
                223_000.0,
            ),
            row(
                2,
                2,
                false,
                [(50.0, 40.0), (350.0, 40.0)],
                223_000.0,
                223_000.0,
            ),
            row(
                3,
                0,
                false,
                [(50.0, 10.0), (350.0, 10.0)],
                223_000.0,
                223_000.0,
            ),
            row(
                4,
                2,
                false,
                [(50.0, -25.0), (350.0, -25.0)],
                18_000.0,
                18_000.0,
            ),
            row(
                5,
                0,
                false,
                [(-200.0, 10.0), (0.0, 0.0)],
                223_000.0,
                223_000.0,
            ),
            row(
                6,
                2,
                false,
                [(200.0, -10.0), (200.0, 30.0)],
                223_000.0,
                223_000.0,
            ),
        ];
        let mut copies: Vec<usize> = copied_counts(&rows, &[], 37.7).into_iter().collect();
        copies.sort_unstable();
        assert_eq!(copies, [2, 3]);
    }
}
