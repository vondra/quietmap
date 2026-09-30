//! Ground legs onto aeroway lines (dev4 Stage 2C `airport_traffic.rs`): the part of a leg inside a
//! line's buffer rectangle runs on that line, and where buffers overlap (parallel taxiways,
//! corners) the parts are scaled to add up to the leg's length, so no leg counts twice.

use crate::aircraft::flat::{M_PER_DEG_LAT, M_PER_DEG_LON_EQUATOR, flat_distance_m};
use std::collections::HashMap;

/// A leg within this distance of a line's axis runs on it (dev4 `AIRPORT_LINE_SNAP_BUFFER_M`).
pub const SNAP_BUFFER_M: f64 = 50.0;
/// Edge of the index cells (degrees, about 220 m of latitude).
const CELL_DEG: f64 = 0.002;
/// A line is listed in the cells of its buffer widened by this much, which covers the rounding
/// of the flat frame (dev4 `BBOX_SLACK_M`).
const CELL_MARGIN_M: f64 = 5.0;
const CELL_COLUMNS: i64 = (360.0 / CELL_DEG) as i64;

/// `to - from` in degrees of longitude, the short way.
fn longitude_delta(from: f64, to: f64) -> f64 {
    (to - from + 540.0).rem_euclid(360.0) - 180.0
}

/// A line's flat frame (dev4 `LineFrame`): metres along the line and to its left, from its middle.
#[derive(Debug, Clone, Copy)]
struct LineFrame {
    middle: [f64; 2],
    metres_per_degree_lon: f64,
    along: [f64; 2],
    half_length_m: f64,
}

impl LineFrame {
    /// `None` for a line under a millimetre or with a coordinate that is not a number.
    fn new(ends: [[f64; 2]; 2]) -> Option<Self> {
        let delta_lon = longitude_delta(ends[0][1], ends[1][1]);
        let middle = [
            0.5 * (ends[0][0] + ends[1][0]),
            ends[0][1] + 0.5 * delta_lon,
        ];
        let metres_per_degree_lon = f64::from(M_PER_DEG_LON_EQUATOR) * middle[0].to_radians().cos();
        let east = delta_lon * metres_per_degree_lon;
        let north = (ends[1][0] - ends[0][0]) * f64::from(M_PER_DEG_LAT);
        let length = east.hypot(north);
        (length >= 1e-3).then_some(LineFrame {
            middle,
            metres_per_degree_lon,
            along: [east / length, north / length],
            half_length_m: 0.5 * length,
        })
    }

    fn local(&self, point: [f64; 2]) -> [f64; 2] {
        let east = longitude_delta(self.middle[1], point[1]) * self.metres_per_degree_lon;
        let north = (point[0] - self.middle[0]) * f64::from(M_PER_DEG_LAT);
        [
            east * self.along[0] + north * self.along[1],
            north * self.along[0] - east * self.along[1],
        ]
    }

    /// Length of the leg `start`-`end` inside the rectangle of the line's length and twice the
    /// buffer (Liang-Barsky clipping).
    fn overlap_m(&self, start: [f64; 2], end: [f64; 2]) -> f64 {
        let (a, b) = (self.local(start), self.local(end));
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let length = dx.hypot(dy);
        if length.is_nan() || length < 1e-3 {
            return 0.0;
        }
        let (mut t0, mut t1) = (0.0f64, 1.0f64);
        for (p, q) in [
            (-dx, a[0] + self.half_length_m),
            (dx, self.half_length_m - a[0]),
            (-dy, a[1] + SNAP_BUFFER_M),
            (dy, SNAP_BUFFER_M - a[1]),
        ] {
            // Below a micrometre the leg runs parallel to this edge (dev4's threshold).
            if p.abs() < 1e-6 {
                if q < 0.0 {
                    return 0.0;
                }
            } else if p < 0.0 {
                t0 = t0.max(q / p);
            } else {
                t1 = t1.min(q / p);
            }
        }
        if t1 > t0 { (t1 - t0) * length } else { 0.0 }
    }
}

/// The index cell of a latitude and a (possibly unwrapped) longitude.
fn cell(lat: f64, lon: f64) -> (i64, i64) {
    (
        (lat / CELL_DEG).floor() as i64,
        ((lon + 180.0) / CELL_DEG).floor() as i64,
    )
}

/// Every cell of a box, longitudes wrapped.
fn cells_of(lat: [f64; 2], lon: [f64; 2]) -> impl Iterator<Item = (i32, i32)> {
    let ((south, west), (north, east)) = (cell(lat[0], lon[0]), cell(lat[1], lon[1]));
    (south..=north).flat_map(move |y| {
        (west..=east).map(move |x| (y as i32, x.rem_euclid(CELL_COLUMNS) as i32))
    })
}

/// Aeroway lines by the cells their buffers meet.
pub struct LineIndex {
    frames: Vec<Option<LineFrame>>,
    cells: HashMap<(i32, i32), Vec<u32>>,
}

impl LineIndex {
    /// The index of lines given by their ends (latitude, longitude).
    pub fn new(lines: &[[[f64; 2]; 2]]) -> Self {
        let mut cells: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        let frames: Vec<Option<LineFrame>> =
            lines.iter().map(|ends| LineFrame::new(*ends)).collect();
        let reach = SNAP_BUFFER_M + CELL_MARGIN_M;
        for (index, (ends, frame)) in lines.iter().zip(&frames).enumerate() {
            if frame.is_none() {
                continue;
            }
            let lat_pad = reach / f64::from(M_PER_DEG_LAT);
            let widest = ends[0][0].abs().max(ends[1][0].abs()) + lat_pad;
            let lon_pad = reach / (f64::from(M_PER_DEG_LON_EQUATOR) * widest.to_radians().cos());
            let lon_end = ends[0][1] + longitude_delta(ends[0][1], ends[1][1]);
            let lat = [ends[0][0].min(ends[1][0]), ends[0][0].max(ends[1][0])];
            let lon = [ends[0][1].min(lon_end), ends[0][1].max(lon_end)];
            for key in cells_of(
                [lat[0] - lat_pad, lat[1] + lat_pad],
                [lon[0] - lon_pad, lon[1] + lon_pad],
            ) {
                cells.entry(key).or_default().push(index as u32);
            }
        }
        LineIndex { frames, cells }
    }

    /// The lines the leg `start`-`end` (latitude, longitude) runs on, with the length (m) it runs
    /// on each, into `hits`; the lengths add up to at most the leg's length.
    pub fn project(&self, start: [f64; 2], end: [f64; 2], hits: &mut Vec<(u32, f64)>) {
        hits.clear();
        let lon_end = start[1] + longitude_delta(start[1], end[1]);
        let lat = [start[0].min(end[0]), start[0].max(end[0])];
        let lon = [start[1].min(lon_end), start[1].max(lon_end)];
        for key in cells_of(lat, lon) {
            if let Some(lines) = self.cells.get(&key) {
                hits.extend(lines.iter().map(|&line| (line, 0.0)));
            }
        }
        hits.sort_unstable_by_key(|hit| hit.0);
        hits.dedup_by_key(|hit| hit.0);
        for hit in hits.iter_mut() {
            let frame = self.frames[hit.0 as usize].expect("indexed lines have a frame");
            hit.1 = frame.overlap_m(start, end);
        }
        hits.retain(|hit| hit.1 > 0.0);
        let total: f64 = hits.iter().map(|hit| hit.1).sum();
        let leg = f64::from(flat_distance_m(
            start[0] as f32,
            start[1] as f32,
            end[0] as f32,
            end[1] as f32,
        ));
        if total > leg {
            for hit in hits.iter_mut() {
                hit.1 *= leg / total;
            }
        }
    }

    /// A line's length in its flat frame (m); 0 for a degenerate line.
    pub fn length_m(&self, line: u32) -> f64 {
        self.frames[line as usize].map_or(0.0, |frame| 2.0 * frame.half_length_m)
    }
}
