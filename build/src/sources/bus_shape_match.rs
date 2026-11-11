//! The timetabled buses along a road row: the shapes passing within 15 m of its midpoint within
//! 30 degrees of its bearing (a one-way row only in its direction), each shape's trips once.

use super::bus_timetables::Shape;
use std::collections::HashMap;

/// A shape runs along a row whose midpoint lies this close to one of its segments (the 2026 fit's
/// `measure.py`: about a lane and a half each side of a mapped centre line).
const MOST_DISTANCE_M: f64 = 15.0;
/// ...and whose segment there turns at most this far from the row's bearing.
const MOST_TURN_DEG: f64 = 30.0;
/// Segments are indexed in square cells of this side.
const CELL_M: f64 = 40.0;

/// Local metres east and north of a place's centre (`measure.py`'s equirectangular frame).
#[derive(Clone, Copy)]
pub(super) struct LocalFrame {
    centre: [f64; 2],
    metres_per_degree: [f64; 2],
}

impl LocalFrame {
    /// The frame of bounds `[south, west, north, east]`.
    pub fn new([south, west, north, east]: [f64; 4]) -> Self {
        let centre = [0.5 * (south + north), 0.5 * (west + east)];
        LocalFrame {
            centre,
            metres_per_degree: [110_540.0, 111_320.0 * centre[0].to_radians().cos()],
        }
    }

    /// (east, north) metres of a (latitude, longitude).
    pub fn metres(&self, [lat, lon]: [f64; 2]) -> [f64; 2] {
        [
            (lon - self.centre[1]) * self.metres_per_degree[1],
            (lat - self.centre[0]) * self.metres_per_degree[0],
        ]
    }
}

/// The segments of a place's shapes in a grid of [`CELL_M`] cells.
pub(super) struct ShapeIndex<'a> {
    frame: LocalFrame,
    shapes: Vec<&'a Shape>,
    segments: Vec<([f64; 2], [f64; 2], usize)>,
    cells: HashMap<[i64; 2], Vec<usize>>,
}

fn cell_of(metres: f64) -> i64 {
    (metres / CELL_M).floor() as i64
}

impl<'a> ShapeIndex<'a> {
    /// The segments of `shapes` that reach within [`MOST_DISTANCE_M`] of the bounds.
    pub fn new(bounds: [f64; 4], shapes: Vec<&'a Shape>) -> Self {
        let frame = LocalFrame::new(bounds);
        let [west_south, east_north] = [
            frame.metres([bounds[0], bounds[1]]),
            frame.metres([bounds[2], bounds[3]]),
        ];
        let mut index = ShapeIndex {
            frame,
            shapes,
            segments: Vec::new(),
            cells: HashMap::new(),
        };
        for (shape, points) in index.shapes.iter().map(|shape| &shape.points).enumerate() {
            let metres: Vec<[f64; 2]> = points.iter().map(|&point| frame.metres(point)).collect();
            for pair in metres.windows(2) {
                let [a, b] = [pair[0], pair[1]];
                if a == b {
                    continue;
                }
                let low = [
                    a[0].min(b[0]) - MOST_DISTANCE_M,
                    a[1].min(b[1]) - MOST_DISTANCE_M,
                ];
                let high = [
                    a[0].max(b[0]) + MOST_DISTANCE_M,
                    a[1].max(b[1]) + MOST_DISTANCE_M,
                ];
                if high[0] < west_south[0]
                    || low[0] > east_north[0]
                    || high[1] < west_south[1]
                    || low[1] > east_north[1]
                {
                    continue;
                }
                let segment = index.segments.len();
                index.segments.push((a, b, shape));
                for x in cell_of(low[0])..=cell_of(high[0]) {
                    for y in cell_of(low[1])..=cell_of(high[1]) {
                        index.cells.entry([x, y]).or_default().push(segment);
                    }
                }
            }
        }
        index
    }

    /// The timetable's buses a day along the row from `start` to `end` (latitude, longitude):
    /// the trips of every shape running along it, once a shape, over seven days. `oneway` is the
    /// prepared code: 0 two-way, 2 one-way against the row's drawn direction, 1, 3 (a roundabout)
    /// and 4 (a motorway) one-way along it. `None` for a row too short (under a metre) to have a
    /// bearing.
    pub fn buses_along(&self, start: [f64; 2], end: [f64; 2], oneway: u8) -> Option<f64> {
        let [a, b] = [self.frame.metres(start), self.frame.metres(end)];
        let along = [b[0] - a[0], b[1] - a[1]];
        let length = along[0].hypot(along[1]);
        if length < 1.0 {
            return None;
        }
        let sign = if oneway == 2 { -1.0 } else { 1.0 };
        let unit = [sign * along[0] / length, sign * along[1] / length];
        let middle = [0.5 * (a[0] + b[0]), 0.5 * (a[1] + b[1])];
        let least_cosine = MOST_TURN_DEG.to_radians().cos();
        let mut matched: Vec<usize> = Vec::new();
        for &segment in self
            .cells
            .get(&[cell_of(middle[0]), cell_of(middle[1])])
            .into_iter()
            .flatten()
        {
            let (p, q, shape) = self.segments[segment];
            if matched.contains(&shape) {
                continue;
            }
            let run = [q[0] - p[0], q[1] - p[1]];
            let squared = run[0] * run[0] + run[1] * run[1];
            let t = (((middle[0] - p[0]) * run[0] + (middle[1] - p[1]) * run[1]) / squared)
                .clamp(0.0, 1.0);
            let nearest = [p[0] + t * run[0] - middle[0], p[1] + t * run[1] - middle[1]];
            if nearest[0].hypot(nearest[1]) > MOST_DISTANCE_M {
                continue;
            }
            let cosine = (run[0] * unit[0] + run[1] * unit[1]) / squared.sqrt();
            let along_the_row = if (1..=4).contains(&oneway) {
                cosine >= least_cosine
            } else {
                cosine.abs() >= least_cosine
            };
            if along_the_row {
                matched.push(shape);
            }
        }
        Some(
            matched
                .iter()
                .map(|&shape| self.shapes[shape].trips)
                .sum::<f64>()
                / 7.0,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(points: &[[f64; 2]], trips: f64) -> Shape {
        Shape {
            points: points.to_vec(),
            trips,
        }
    }

    /// A row 100 m east along the 50th parallel near the bounds' centre, and shapes beside it.
    #[test]
    fn a_shape_runs_along_a_row_within_its_distance_and_bearing_and_one_way_rows_in_their_direction()
     {
        let bounds = [49.99, 13.99, 50.01, 14.01];
        let frame = LocalFrame::new(bounds);
        let east_per_metre = 1.0 / frame.metres_per_degree[1];
        let north_per_metre = 1.0 / frame.metres_per_degree[0];
        let at =
            |east: f64, north: f64| [50.0 + north * north_per_metre, 14.0 + east * east_per_metre];
        let eastward = shape(&[at(-500.0, 10.0), at(500.0, 10.0)], 70.0);
        let westward = shape(&[at(500.0, -10.0), at(-500.0, -10.0)], 140.0);
        // 20 m off the midpoint, and across the row.
        let far = shape(&[at(-500.0, 20.0), at(500.0, 20.0)], 700.0);
        let across = shape(&[at(0.0, -300.0), at(0.0, 300.0)], 700.0);
        // 25 degrees off the bearing: along; 35 degrees: not.
        let turn = |degrees: f64| {
            let (sin, cos) = degrees.to_radians().sin_cos();
            shape(
                &[at(-100.0 * cos, -100.0 * sin), at(100.0 * cos, 100.0 * sin)],
                7.0,
            )
        };
        let (gentle, steep) = (turn(25.0), turn(35.0));
        // The same shape twice through the cell counts once.
        let looping = shape(
            &[at(-50.0, 5.0), at(50.0, 5.0), at(50.0, 6.0), at(-50.0, 6.0)],
            14.0,
        );
        let index = ShapeIndex::new(
            bounds,
            vec![
                &eastward, &westward, &far, &across, &gentle, &steep, &looping,
            ],
        );
        let (start, end) = (at(-50.0, 0.0), at(50.0, 0.0));
        let close = |value: Option<f64>, expected: f64| (value.unwrap() - expected).abs() < 1e-9;
        // Two-way: eastward 10 + westward 20 + gentle 1 + looping 2 a day.
        assert!(close(index.buses_along(start, end, 0), 33.0));
        // One-way as drawn (east): eastward, gentle and looping.
        assert!(close(index.buses_along(start, end, 1), 13.0));
        // One-way against the drawn direction: westward and looping's way back.
        assert!(close(index.buses_along(start, end, 2), 22.0));
        assert_eq!(index.buses_along(start, start, 0), None);
    }
}
