//! Where traffic stops and starts, for the CNOSSOS-EU junction corrections (2.2.5): traffic
//! signals (road nodes tagged `highway=traffic_signals`) and roundabouts (the vertices of their
//! rows). A road row takes the nearest junction within [`JUNCTION_REACH_M`] of its middle, a
//! roundabout's own rows the roundabout at distance 0. Junctions of the neighbouring squares are
//! not read: a row within 100 m of a square's edge may miss one there.

use crate::dev4::{Dev4, Square, z30_corner_degrees};
use arrow_array::cast::AsArray;
use arrow_array::types::Int32Type;
use physics::emission::road::{JUNCTION_REACH_M, Junction};
use std::collections::HashMap;

/// Metres per degree of latitude, and of longitude at the equator.
const METRES_PER_DEGREE: f64 = 111_195.0;
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

    /// The nearest junction within reach of `place` and the distance to it (m).
    pub fn nearest(&self, place: (f64, f64)) -> Option<(Junction, f64)> {
        let xy = self.plane(place);
        let (cx, cy) = cell(xy);
        let mut best: Option<(Junction, f64)> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for (point, kind) in self.cells.get(&(cx + dx, cy + dy)).into_iter().flatten() {
                    let distance = (point[0] - xy[0]).hypot(point[1] - xy[1]);
                    if distance < JUNCTION_REACH_M && best.is_none_or(|(_, d)| distance < d) {
                        best = Some((*kind, distance));
                    }
                }
            }
        }
        best
    }
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

    /// The nearest junction within 100 m wins; farther ones are none.
    #[test]
    fn a_row_takes_the_nearest_junction_within_reach() {
        let degrees = |metres: f64| metres / METRES_PER_DEGREE;
        let junctions = Junctions::new(
            0.0,
            [
                ((0.0, 0.0), Junction::TrafficLights),
                ((degrees(60.0), 0.0), Junction::Roundabout),
            ],
        );
        let (kind, distance) = junctions.nearest((degrees(40.0), 0.0)).unwrap();
        assert_eq!(kind, Junction::Roundabout);
        assert!((distance - 20.0).abs() < 1e-6);
        let (kind, _) = junctions.nearest((degrees(-10.0), 0.0)).unwrap();
        assert_eq!(kind, Junction::TrafficLights);
        assert_eq!(junctions.nearest((degrees(-150.0), 0.0)), None);
    }
}
