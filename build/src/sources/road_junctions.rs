//! Where traffic stops and starts, for the CNOSSOS-EU junction corrections (2.2.5): traffic
//! signals (road nodes tagged `highway=traffic_signals`) and roundabouts (the vertices of their
//! rows). A road row takes the nearest junction on its own way within [`JUNCTION_REACH_M`] along
//! the way from its middle (OpenStreetMap puts a signal on a node of the road it stops, and an
//! approach ends on a node of the roundabout), a roundabout's own rows the roundabout at distance
//! 0; a road that only passes near a junction, or over it on a bridge, takes none. Junctions of the
//! neighbouring squares are not read: a row within 100 m of a square's edge may miss one there.

use super::road_slope::Chain;
use crate::dev4::{Dev4, Square, z30_corner_degrees};
use arrow_array::cast::AsArray;
use arrow_array::types::Int32Type;
use physics::emission::road::{JUNCTION_REACH_M, Junction};
use std::collections::HashMap;

/// Metres per degree of latitude, and of longitude at the equator.
const METRES_PER_DEGREE: f64 = 111_195.0;
/// A junction this close to a way's line is on the way (m): it sits on one of its nodes.
const ON_WAY_M: f64 = 2.0;
/// The node tag of a traffic signal in the transport nodes' tags.
const SIGNAL_TAG: &str = "\"highway\":\"traffic_signals\"";

/// A junction on the local plane (m) and its kind.
type PlacedJunction = ([f64; 2], Junction);

/// Junctions in cells of [`JUNCTION_REACH_M`] on a local plane.
pub struct Junctions {
    cos_latitude: f64,
    cells: HashMap<(i64, i64), Vec<PlacedJunction>>,
}

impl Junctions {
    pub fn new(latitude: f64, points: impl IntoIterator<Item = ((f64, f64), Junction)>) -> Self {
        let mut junctions = Junctions {
            cos_latitude: latitude.to_radians().cos(),
            cells: HashMap::new(),
        };
        for (place, kind) in points {
            let xy = junctions.plane(place);
            junctions
                .cells
                .entry(cell(xy))
                .or_default()
                .push((xy, kind));
        }
        junctions
    }

    fn plane(&self, (lat, lon): (f64, f64)) -> [f64; 2] {
        [
            lon * METRES_PER_DEGREE * self.cos_latitude,
            lat * METRES_PER_DEGREE,
        ]
    }

    /// The junctions on the segment from `a` to `b`: each kind and its distance from `a` along it.
    fn on_segment(&self, a: (f64, f64), b: (f64, f64)) -> Vec<(Junction, f64)> {
        let (pa, pb) = (self.plane(a), self.plane(b));
        let (dx, dy) = (pb[0] - pa[0], pb[1] - pa[1]);
        let length_squared = dx * dx + dy * dy;
        let (low, high) = (
            cell([pa[0].min(pb[0]) - ON_WAY_M, pa[1].min(pb[1]) - ON_WAY_M]),
            cell([pa[0].max(pb[0]) + ON_WAY_M, pa[1].max(pb[1]) + ON_WAY_M]),
        );
        let mut found = Vec::new();
        for cx in low.0..=high.0 {
            for cy in low.1..=high.1 {
                for (point, kind) in self.cells.get(&(cx, cy)).into_iter().flatten() {
                    let t = if length_squared > 0.0 {
                        (((point[0] - pa[0]) * dx + (point[1] - pa[1]) * dy) / length_squared)
                            .clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    let (x, y) = (pa[0] + t * dx, pa[1] + t * dy);
                    if (point[0] - x).hypot(point[1] - y) <= ON_WAY_M {
                        found.push((*kind, t * length_squared.sqrt()));
                    }
                }
            }
        }
        found
    }
}

/// A signal stands at most this far before its crossing (m), at the stop line of its approach.
const STOP_LINE_M: f64 = 25.0;

/// Each row's junction, as CNOSSOS-EU 2.2.5 has it: the nearest crossing of its road with another
/// that has a signal or is a roundabout's entry, within [`JUNCTION_REACH_M`] along its chain from
/// the row's middle, and that distance (m). A signal marks the nearest crossing (a vertex of two
/// chains or more) on its own way within [`STOP_LINE_M`], and every road through that crossing
/// takes it; a signal with no crossing near (a pedestrian crossing) counts on its own road only.
/// Junctions are met on the ground only.
pub fn row_junctions(
    chains: &[Chain],
    junctions: &Junctions,
    rows: usize,
) -> Vec<Option<(Junction, f64)>> {
    // Every vertex's chains and its distance along each.
    let key = |place: (f64, f64)| (place.0.to_bits(), place.1.to_bits());
    let mut vertices: HashMap<(u64, u64), Vec<(usize, f64)>> = HashMap::new();
    for (index, chain) in chains.iter().enumerate() {
        for (place, &at) in chain.places.iter().zip(&chain.distances) {
            vertices.entry(key(*place)).or_default().push((index, at));
        }
    }
    let crossing = |place: (f64, f64)| {
        vertices
            .get(&key(place))
            .filter(|on| on.iter().any(|&(chain, _)| chain != on[0].0))
    };
    let mut along: Vec<Vec<(Junction, f64)>> = vec![Vec::new(); chains.len()];
    for (index, chain) in chains.iter().enumerate() {
        for (k, row) in chain.rows.iter().enumerate() {
            if row.off_ground {
                continue;
            }
            for (kind, offset) in junctions.on_segment(chain.places[k], chain.places[k + 1]) {
                let at = chain.distances[k] + offset;
                let nearest = chain
                    .places
                    .iter()
                    .zip(&chain.distances)
                    .filter(|&(_, &d)| (d - at).abs() <= STOP_LINE_M)
                    .filter_map(|(&place, &d)| crossing(place).map(|on| (on, (d - at).abs())))
                    .min_by(|a, b| a.1.total_cmp(&b.1));
                match nearest {
                    Some((on, _)) => {
                        for &(other, d) in on {
                            along[other].push((kind, d));
                        }
                    }
                    None => along[index].push((kind, at)),
                }
            }
        }
    }
    let mut found = vec![None; rows];
    for (chain, along) in chains.iter().zip(&along) {
        for (k, row) in chain.rows.iter().enumerate() {
            let middle = 0.5 * (chain.distances[k] + chain.distances[k + 1]);
            found[row.row] = along
                .iter()
                .map(|&(kind, at)| (kind, (at - middle).abs()))
                .filter(|&(_, distance)| distance < JUNCTION_REACH_M)
                .min_by(|a, b| a.1.total_cmp(&b.1));
        }
    }
    found
}

fn cell(xy: [f64; 2]) -> (i64, i64) {
    (
        (xy[0] / JUNCTION_REACH_M).floor() as i64,
        (xy[1] / JUNCTION_REACH_M).floor() as i64,
    )
}

/// The traffic signals on the square's roads (latitude, longitude).
pub fn traffic_signals(dev4: &Dev4, square: Square) -> Result<Vec<(f64, f64)>, String> {
    let Some(table) = dev4.table(square, "transport_nodes.arrow")? else {
        return Ok(Vec::new());
    };
    let mut signals = Vec::new();
    for batch in &table.batches {
        let column = |name: &str| {
            batch
                .column_by_name(name)
                .ok_or_else(|| format!("transport_nodes.arrow: no column {name}"))
        };
        let (gx, gy) = (
            column("gx")?.as_primitive::<Int32Type>(),
            column("gy")?.as_primitive::<Int32Type>(),
        );
        let (family, tags) = (
            column("family")?.as_string::<i32>(),
            column("osm_tags")?.as_string::<i32>(),
        );
        for row in 0..batch.num_rows() {
            if family.value(row) == "roads" && tags.value(row).contains(SIGNAL_TAG) {
                signals.push(z30_corner_degrees(gx.value(row), gy.value(row)));
            }
        }
    }
    Ok(signals)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A signal counts for the way it stands on, by the distance along it, and for no road beside
    /// it (a parallel street 20 m away took +5.3 dB on its lorries) or over it on a bridge.
    #[test]
    fn a_row_takes_the_junction_on_its_own_way() {
        use super::super::road_slope::{WayRow, chains};
        let degrees = |metres: f64| metres / METRES_PER_DEGREE;
        let junctions = Junctions::new(0.0, [((0.0, 0.0), Junction::TrafficLights)]);
        let row =
            |row: usize, index: i16, from: f64, to: f64, north: f64, off_ground: bool| WayRow {
                row,
                segment_index: index,
                start: (degrees(north), degrees(from)),
                end: (degrees(north), degrees(to)),
                off_ground,
            };
        let mut ways = HashMap::new();
        ways.insert(
            1,
            vec![
                row(0, 0, -50.0, 0.0, 0.0, false),
                row(1, 1, 0.0, 150.0, 0.0, false),
            ],
        );
        ways.insert(2, vec![row(2, 0, -50.0, 50.0, 20.0, false)]);
        ways.insert(3, vec![row(3, 0, -50.0, 50.0, 0.0, true)]);
        let found = row_junctions(&chains(&mut ways), &junctions, 4);
        let (kind, distance) = found[0].unwrap();
        assert_eq!(kind, Junction::TrafficLights);
        assert!((distance - 25.0).abs() < 1e-6, "{distance}");
        assert!((found[1].unwrap().1 - 75.0).abs() < 1e-6);
        assert_eq!(found[2], None, "a parallel street");
        assert_eq!(found[3], None, "a bridge over it");
    }

    /// A signal 10 m before a crossing marks the crossing: the signalled road and the road across
    /// it, which maps no signal, both take it by their distance to the crossing; a parallel street
    /// does not.
    #[test]
    fn a_signal_marks_its_crossing_for_every_road_through_it() {
        use super::super::road_slope::{WayRow, chains};
        let degrees = |metres: f64| metres / METRES_PER_DEGREE;
        let junctions = Junctions::new(0.0, [((0.0, degrees(-10.0)), Junction::TrafficLights)]);
        let row = |row: usize, index: i16, from: (f64, f64), to: (f64, f64)| WayRow {
            row,
            segment_index: index,
            start: (degrees(from.1), degrees(from.0)),
            end: (degrees(to.1), degrees(to.0)),
            off_ground: false,
        };
        let mut ways = HashMap::new();
        ways.insert(
            1,
            vec![
                row(0, 0, (-50.0, 0.0), (-10.0, 0.0)),
                row(1, 1, (-10.0, 0.0), (0.0, 0.0)),
                row(2, 2, (0.0, 0.0), (50.0, 0.0)),
            ],
        );
        ways.insert(
            2,
            vec![
                row(3, 0, (0.0, -50.0), (0.0, 0.0)),
                row(4, 1, (0.0, 0.0), (0.0, 50.0)),
            ],
        );
        ways.insert(3, vec![row(5, 0, (-50.0, 20.0), (50.0, 20.0))]);
        let found = row_junctions(&chains(&mut ways), &junctions, 6);
        let distance = |row: usize| found[row].map(|(_, d)| (d * 1e6).round() / 1e6);
        assert_eq!(
            [distance(0), distance(1), distance(2)],
            [Some(30.0), Some(5.0), Some(25.0)]
        );
        assert_eq!([distance(3), distance(4)], [Some(25.0), Some(25.0)]);
        assert_eq!(found[5], None, "a parallel street");
    }
}
