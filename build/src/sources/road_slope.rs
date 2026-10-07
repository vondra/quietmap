//! Road slopes from the bare earth, for the CNOSSOS-EU gradient correction (2.2.4): each row's
//! slope along its way over [`WINDOW_HALF_M`] either side of its middle, the heights read from the
//! square's one-arc-second terrain at the window's ends (a shorter window reads the lattice's 31 m
//! steps as slopes). A bridge or a tunnel is not on the terrain (under a deck is the valley, over a
//! tunnel the hill): the road runs straight between the ground at its two ends, and its own rows
//! read level.

use super::metres;
use crate::dev4::{Dev4, Square, z9_raster_window};
use std::collections::HashMap;
use tiles::terrain::{HEIGHT_MISSING, NODES_PER_DEGREE, Window, height_m_of_code};

/// Half the window a row's slope is taken over (m).
pub const WINDOW_HALF_M: f64 = 50.0;
/// Ways whose window is shorter than this read level (m).
pub const WINDOW_MIN_M: f64 = 20.0;
/// Consecutive rows of a way join when their ends are this close (degrees, about 1 m).
const JOIN_DEGREES: f64 = 1e-5;

/// A square's bare-earth heights (empty over the ocean).
pub struct SquareHeights {
    window: Window,
    dem: Vec<u8>,
}

impl SquareHeights {
    pub fn load(dev4: &Dev4, square: Square) -> Result<Self, String> {
        let window = z9_raster_window(square);
        let path = dev4.raster_file(square, "dem.u16le");
        let dem = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(format!("{}: {error}", path.display())),
        };
        if !dem.is_empty() && dem.len() != 2 * window.node_count() {
            return Err(format!(
                "{}: {} bytes for {window:?}",
                path.display(),
                dem.len()
            ));
        }
        Ok(SquareHeights { window, dem })
    }

    fn node(&self, row: i64, column: i64) -> Option<f64> {
        let (rows, columns) = (i64::from(self.window.rows), i64::from(self.window.columns));
        let (row, column) = (row.clamp(0, rows - 1), column.clamp(0, columns - 1));
        let at = 2 * (row * columns + column) as usize;
        let code = u16::from_le_bytes([self.dem[at], self.dem[at + 1]]);
        (code != HEIGHT_MISSING).then(|| height_m_of_code(code))
    }

    /// The bilinear height (m) at a place, the window's edge nodes standing for places beyond it;
    /// `None` over the ocean or a hole.
    pub fn at(&self, lat: f64, lon: f64) -> Option<f64> {
        if self.dem.is_empty() {
            return None;
        }
        let nodes = f64::from(NODES_PER_DEGREE);
        let row = f64::from(self.window.north_node) - lat * nodes;
        let column = lon * nodes - f64::from(self.window.west_node);
        let (r, c) = (row.floor(), column.floor());
        let (fr, fc) = (row - r, column - c);
        let (r, c) = (r as i64, c as i64);
        let [a, b, d, e] = [
            self.node(r, c)?,
            self.node(r, c + 1)?,
            self.node(r + 1, c)?,
            self.node(r + 1, c + 1)?,
        ];
        Some((a * (1.0 - fc) + b * fc) * (1.0 - fr) + (d * (1.0 - fc) + e * fc) * fr)
    }
}

/// One row of a way: its order on the way, its ends (latitude, longitude) and whether it is a
/// bridge or a tunnel.
#[derive(Debug, Clone, Copy)]
pub struct WayRow {
    pub row: usize,
    pub segment_index: i16,
    pub start: (f64, f64),
    pub end: (f64, f64),
    pub off_ground: bool,
}

/// The place `along_m` metres along a chain of `places` with cumulative `distances`.
fn place_along(places: &[(f64, f64)], distances: &[f64], along_m: f64) -> (f64, f64) {
    let k = distances
        .partition_point(|&d| d <= along_m)
        .clamp(1, places.len() - 1);
    let span = distances[k] - distances[k - 1];
    let t = if span > 0.0 {
        (along_m - distances[k - 1]) / span
    } else {
        0.0
    };
    let (a, b) = (places[k - 1], places[k]);
    (a.0 + t * (b.0 - a.0), a.1 + t * (b.1 - a.1))
}

/// Every row's slope (%, positive when climbing from its start to its end; 0 on bridges, tunnels,
/// short ways and where the terrain is missing), from the rows of each way.
pub fn row_slopes(
    ways: HashMap<i64, Vec<WayRow>>,
    heights: &SquareHeights,
    rows: usize,
) -> Vec<f64> {
    let mut slopes = vec![0.0; rows];
    for mut way in ways.into_values() {
        way.sort_by_key(|row| row.segment_index);
        // Chains of rows that join end to start.
        let mut first = 0;
        while first < way.len() {
            let mut last = first;
            while last + 1 < way.len() {
                let (a, b) = (way[last].end, way[last + 1].start);
                if (a.0 - b.0).abs() > JOIN_DEGREES || (a.1 - b.1).abs() > JOIN_DEGREES {
                    break;
                }
                last += 1;
            }
            let chain = &way[first..=last];
            let places: Vec<(f64, f64)> = std::iter::once(chain[0].start)
                .chain(chain.iter().map(|row| row.end))
                .collect();
            let mut distances = vec![0.0];
            for pair in places.windows(2) {
                distances.push(distances[distances.len() - 1] + metres(pair[0], pair[1]));
            }
            let total = distances[distances.len() - 1];
            // The spans of the chain off the ground (bridges and tunnels), from and to (m).
            let mut spans: Vec<(f64, f64)> = Vec::new();
            for (k, row) in chain.iter().enumerate() {
                if !row.off_ground {
                    continue;
                }
                match spans.last_mut() {
                    Some(span) if span.1 == distances[k] => span.1 = distances[k + 1],
                    _ => spans.push((distances[k], distances[k + 1])),
                }
            }
            let ground = |along: f64| {
                let (lat, lon) = place_along(&places, &distances, along);
                heights.at(lat, lon)
            };
            // The road's height: the terrain, or across a span the line between the ground at its
            // ends (one end alone where the chain starts or ends off the ground).
            let height =
                |along: f64| match spans.iter().find(|span| span.0 < along && along < span.1) {
                    None => ground(along),
                    Some(&(from, to)) => {
                        let ends = [
                            (from > 0.0).then(|| ground(from)).flatten(),
                            (to < total).then(|| ground(to)).flatten(),
                        ];
                        match ends {
                            [Some(a), Some(b)] => Some(a + (b - a) * (along - from) / (to - from)),
                            [Some(a), None] => Some(a),
                            [None, Some(b)] => Some(b),
                            [None, None] => None,
                        }
                    }
                };
            for (k, row) in chain.iter().enumerate() {
                if row.off_ground {
                    continue;
                }
                let middle = 0.5 * (distances[k] + distances[k + 1]);
                let (from, to) = (
                    (middle - WINDOW_HALF_M).max(0.0),
                    (middle + WINDOW_HALF_M).min(total),
                );
                if to - from < WINDOW_MIN_M {
                    continue;
                }
                if let (Some(low), Some(high)) = (height(from), height(to)) {
                    slopes[row.row] = 100.0 * (high - low) / (to - from);
                }
            }
            first = last + 1;
        }
    }
    slopes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lattice rising 5 m per node eastwards (one node, 1/3600 degree, is about 21 m of
    /// longitude at 55 N): an eastbound way reads that slope, a westbound one its negative, a
    /// bridge and a 15 m way level.
    #[test]
    fn slopes_follow_the_way_over_its_window() {
        let window = Window {
            north_node: 55 * 3600 + 100,
            west_node: 10 * 3600,
            rows: 200,
            columns: 200,
        };
        let mut dem = Vec::new();
        for _row in 0..200 {
            for column in 0..200u32 {
                let height = 300.0 + 5.0 * f64::from(column);
                let code = ((height + 500.0) * 5.0).round() as u16;
                dem.extend_from_slice(&code.to_le_bytes());
            }
        }
        let heights = SquareHeights { window, dem };
        let lat = 55.0 + 50.0 / 3600.0;
        let lon = |node: f64| 10.0 + node / 3600.0;
        let node_m = metres((lat, lon(0.0)), (lat, lon(1.0)));
        let row = |row: usize, index: i16, from: f64, to: f64, off_ground: bool| WayRow {
            row,
            segment_index: index,
            start: (lat, lon(from)),
            end: (lat, lon(to)),
            off_ground,
        };
        let mut ways = HashMap::new();
        ways.insert(
            1,
            vec![row(1, 1, 30.0, 40.0, false), row(0, 0, 20.0, 30.0, false)],
        );
        ways.insert(2, vec![row(2, 0, 60.0, 50.0, false)]);
        ways.insert(3, vec![row(3, 0, 20.0, 30.0, true)]);
        ways.insert(4, vec![row(4, 0, 20.0, 20.7, false)]);
        let slopes = row_slopes(ways, &heights, 5);
        let expected = 100.0 * 5.0 / node_m;
        assert!(
            (slopes[0] - expected).abs() < 1e-6 * expected,
            "{slopes:?} {expected}"
        );
        assert!((slopes[1] - expected).abs() < 1e-6 * expected);
        assert!((slopes[2] + expected).abs() < 1e-6 * expected);
        assert_eq!(slopes[3], 0.0);
        assert_eq!(slopes[4], 0.0);
    }

    /// A flat road crossing a 50 m deep valley on a bridge: the rows that run up to the bridge
    /// read the road's line across it, level, not the valley beneath the deck (-40 %).
    #[test]
    fn a_bridge_approach_reads_the_road_not_the_valley() {
        let window = Window {
            north_node: 55 * 3600 + 100,
            west_node: 10 * 3600,
            rows: 200,
            columns: 200,
        };
        let mut dem = Vec::new();
        for _row in 0..200 {
            for column in 0..200u32 {
                let height: f64 = if (45..=55).contains(&column) {
                    250.0
                } else {
                    300.0
                };
                let code = ((height + 500.0) * 5.0).round() as u16;
                dem.extend_from_slice(&code.to_le_bytes());
            }
        }
        let heights = SquareHeights { window, dem };
        let lat = 55.0 + 50.0 / 3600.0;
        let lon = |node: f64| 10.0 + node / 3600.0;
        let row = |row: usize, index: i16, from: f64, to: f64, off_ground: bool| WayRow {
            row,
            segment_index: index,
            start: (lat, lon(from)),
            end: (lat, lon(to)),
            off_ground,
        };
        let mut ways = HashMap::new();
        ways.insert(
            1,
            vec![
                row(0, 0, 40.0, 44.0, false),
                row(1, 1, 44.0, 56.0, true),
                row(2, 2, 56.0, 60.0, false),
            ],
        );
        let slopes = row_slopes(ways, &heights, 3);
        assert!(slopes.iter().all(|slope| slope.abs() < 1e-9), "{slopes:?}");
    }
}
