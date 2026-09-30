//! Airstrips missing from OSM (dev4 Stage 1.5 `airport_discover`, redone): aircraft ground legs
//! that run on no aeroway line are laid into z19 cells over the window; cells that enough flights
//! crossed on enough days join into groups (touching cells), and each group becomes a line along
//! its main axis spanning its extent, cut as OSM lines are. The traffic pass then takes these lines
//! as runways: a leg rolls on them at 40 kt or faster and taxis slower, so an unmapped taxiway or
//! apron found this way carries taxi levels and no movements.
//!
//! dev4 centred its line on the mean of the leg ends, which the slow end of a strip outweighs (the
//! line overshot there and missed the fast end), accepted five leg ends (three legs of one flight),
//! searched only beyond 6 km of every aerodrome, split strips at z9 edges and took every leg on
//! them as a roll; here the line spans the group, a group needs ten flights on three days, the
//! scope is one grid and only legs off every line count.

use super::legs::{GroundLeg, Mover};
use super::project::LineIndex;
use std::collections::{HashMap, HashSet};
use tiles::geo::{LocalFrame, Mercator};

/// The grid: z19 cells, 76 m at the equator (a strip is one or two cells wide).
const ZOOM: u32 = 19;
const CELLS_PER_TILE: f64 = (1u32 << (ZOOM - 12)) as f64;
/// A leg is laid into the cells of points this far apart along it (m).
const STEP_M: f64 = 10.0;
/// A leg faster than this is not on the ground (a transponder left on "ground" in flight).
const MAX_GROUND_KT: f64 = 200.0;
/// A cell joins a group once this many flights crossed it on this many days.
const MIN_FLIGHTS: u32 = 10;
const MIN_DAYS: u32 = 3;
/// A group longer than this is no airstrip (m).
const MAX_LENGTH_M: f64 = 4_000.0;

/// What a cell saw: flights and days (each counted once), the last of each.
#[derive(Debug, Default, Clone, Copy)]
struct Cell {
    flights: u32,
    days: u32,
    last: Option<(u32, u64)>,
    last_day: u32,
}

/// The cells of a window's unmatched aircraft ground legs, day after day.
#[derive(Debug, Default)]
pub struct Discovery {
    cells: HashMap<(u32, u32), Cell>,
    day: u32,
}

/// The z19 cell of a point (latitude, longitude).
fn cell_of(point: [f64; 2]) -> (u32, u32) {
    let at = Mercator::from_degrees(point[0], point[1]);
    (
        (at.x * CELLS_PER_TILE) as u32,
        (at.y * CELLS_PER_TILE) as u32,
    )
}

fn middle_of(cell: (u32, u32)) -> Mercator {
    Mercator {
        x: (f64::from(cell.0) + 0.5) / CELLS_PER_TILE,
        y: (f64::from(cell.1) + 0.5) / CELLS_PER_TILE,
    }
}

/// The cells a leg crosses: points every [`STEP_M`] along it and its ends.
fn cells_of_leg(leg: &GroundLeg, cells: &mut Vec<(u32, u32)>) {
    cells.clear();
    let (start, end) = (
        Mercator::from_degrees(leg.start[0], leg.start[1]),
        Mercator::from_degrees(leg.end[0], leg.end[1]),
    );
    let frame = LocalFrame::at(start);
    let [east, north] = frame.to_metres(end);
    let steps = (east.hypot(north) / STEP_M).ceil().max(1.0) as usize;
    for step in 0..=steps {
        let t = step as f64 / steps as f64;
        let at = frame.to_mercator([t * east, t * north]);
        let cell = (
            (at.x * CELLS_PER_TILE) as u32,
            (at.y * CELLS_PER_TILE) as u32,
        );
        if cells.last() != Some(&cell) {
            cells.push(cell);
        }
    }
}

impl Discovery {
    /// Adds one day's legs: the aircraft ones that move on the ground and run on no line.
    pub fn add_day(&mut self, legs: &[GroundLeg], index: &LineIndex) {
        self.day += 1;
        let (mut scratch, mut cells) = (Vec::new(), Vec::new());
        for leg in legs {
            let moving = leg.speed_kt > 0.0 && leg.speed_kt <= MAX_GROUND_KT;
            if !matches!(leg.mover, Mover::Aircraft { .. }) || !moving {
                continue;
            }
            index.project(leg.start, leg.end, &mut scratch);
            if !scratch.is_empty() {
                continue;
            }
            cells_of_leg(leg, &mut cells);
            for &cell in &cells {
                let seen = self.cells.entry(cell).or_default();
                if seen.last != Some((self.day, leg.flight_id)) {
                    seen.flights += 1;
                    seen.last = Some((self.day, leg.flight_id));
                }
                if seen.last_day != self.day {
                    seen.days += 1;
                    seen.last_day = self.day;
                }
            }
        }
    }

    /// The airstrips found: each a line (latitude and longitude of both ends) along a group's
    /// main axis over its extent, the end cells taken whole; in a fixed order.
    pub fn strips(&self) -> Vec<[[f64; 2]; 2]> {
        let busy: HashSet<(u32, u32)> = self
            .cells
            .iter()
            .filter(|(_, cell)| cell.flights >= MIN_FLIGHTS && cell.days >= MIN_DAYS)
            .map(|(&key, _)| key)
            .collect();
        let mut starts: Vec<(u32, u32)> = busy.iter().copied().collect();
        starts.sort_unstable();
        let (mut seen, mut strips) = (HashSet::new(), Vec::new());
        for start in starts {
            if !seen.insert(start) {
                continue;
            }
            let mut group = vec![start];
            let mut next = 0;
            while next < group.len() {
                let (x, y) = group[next];
                next += 1;
                for dy in -1i64..=1 {
                    for dx in -1i64..=1 {
                        let neighbour = (
                            (i64::from(x) + dx).max(0) as u32,
                            (i64::from(y) + dy).max(0) as u32,
                        );
                        if busy.contains(&neighbour) && seen.insert(neighbour) {
                            group.push(neighbour);
                        }
                    }
                }
            }
            group.sort_unstable();
            if let Some(strip) = self.strip_of(&group) {
                strips.push(strip);
            }
        }
        strips
    }

    /// A group's line: its cells weighted by their flights, the main axis through their mean,
    /// from the lowest to the highest cell along it, widened by half a cell at each end.
    fn strip_of(&self, group: &[(u32, u32)]) -> Option<[[f64; 2]; 2]> {
        let frame = LocalFrame::at(middle_of(group[0]));
        let points: Vec<([f64; 2], f64)> = group
            .iter()
            .map(|&cell| {
                let weight = f64::from(self.cells[&cell].flights);
                (frame.to_metres(middle_of(cell)), weight)
            })
            .collect();
        let total: f64 = points.iter().map(|(_, weight)| weight).sum();
        let mean = [0, 1].map(|axis| points.iter().map(|(p, w)| w * p[axis]).sum::<f64>() / total);
        let (mut xx, mut yy, mut xy) = (0.0, 0.0, 0.0);
        for (p, w) in &points {
            let (dx, dy) = (p[0] - mean[0], p[1] - mean[1]);
            (xx, yy, xy) = (xx + w * dx * dx, yy + w * dy * dy, xy + w * dx * dy);
        }
        let angle = 0.5 * (2.0 * xy).atan2(xx - yy);
        let axis = [angle.cos(), angle.sin()];
        let along = |p: &[f64; 2]| (p[0] - mean[0]) * axis[0] + (p[1] - mean[1]) * axis[1];
        let (low, high) = points
            .iter()
            .fold((f64::MAX, f64::MIN), |(low, high), (p, _)| {
                (low.min(along(p)), high.max(along(p)))
            });
        // Half a cell edge along the axis at each end.
        let [cell_east, _] = frame.to_metres(Mercator {
            x: middle_of(group[0]).x + 1.0 / CELLS_PER_TILE,
            y: middle_of(group[0]).y,
        });
        let half_cell = 0.5 * cell_east.abs();
        let (low, high) = (low - half_cell, high + half_cell);
        if high - low > MAX_LENGTH_M {
            return None;
        }
        let end = |t: f64| {
            let (lat, lon) = frame
                .to_mercator([mean[0] + t * axis[0], mean[1] + t * axis[1]])
                .to_degrees();
            [lat, lon]
        };
        Some([end(low), end(high)])
    }
}

/// The OSM-like id of a discovered strip: negative, from the z19 cell of its middle.
pub fn strip_id(strip: &[[f64; 2]; 2]) -> i64 {
    let (x, y) = cell_of([
        0.5 * (strip[0][0] + strip[1][0]),
        0.5 * (strip[0][1] + strip[1][1]),
    ]);
    -1 - (i64::from(x) << ZOOM | i64::from(y))
}
