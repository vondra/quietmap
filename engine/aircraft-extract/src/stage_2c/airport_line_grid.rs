//! Uniform geographic grid over airport microsegments with the scalar clip as the exact narrow phase.
//!
//! The grid answers one question per ADS-B leg: which lines can the leg's
//! buffer rectangle possibly meet? Every line whose padded bounding box misses
//! the leg's box is skipped; every other line runs the same scalar clip in
//! the same order as the brute-force kernel, so hits are bit-identical.
//! Padding is the snap buffer plus [`BBOX_SLACK_M`], which covers the f32
//! rounding of the local-frame projection (sub-metre: coordinates near 50°
//! carry ~0.4 m quanta, the frame math adds centimetres).

use super::airport_traffic::{
    clipped_overlap_in_frame, collect_intersections, AirportLineSegment, LegIntersection, LineFrame,
};
use crate::geo::{flat_dist, M_PER_DEG_LAT, M_PER_DEG_LON_EQUATOR};
use anyhow::{Context, Result};
use grid::geo::wrapped_longitude_delta;

/// Base cell size in degrees (~110 m). Doubled while either axis would exceed
/// [`MAX_CELLS_PER_AXIS`], so grid offsets stay bounded on any extent.
const BASE_CELL_DEG: f64 = 0.001;
/// Grid offsets cap: 2048² cells × 4 B = 16 MiB.
const MAX_CELLS_PER_AXIS: usize = 2048;
/// Padding slack beyond the snap buffer: 5× the sub-metre f32 rounding of
/// the narrow phase. A leg missing the padded box cannot overlap the clip
/// rectangle; a leg inside runs the exact scalar test.
const BBOX_SLACK_M: f64 = 5.0;
/// Grids wider than this fall back to the flat list: adjacent-square
/// candidates never span a quarter of the planet, so this only trips on
/// corrupt extents, where correctness beats speed.
const MAX_GRID_SPAN_DEG: f64 = 90.0;

#[derive(Clone, Copy, Default)]
struct PaddedBox {
    lat_min: f64,
    lat_max: f64,
    lon_min: f64,
    lon_max: f64,
}

impl PaddedBox {
    fn contains_leg(&self, leg: &LegBox) -> bool {
        leg.lat_min <= self.lat_max
            && leg.lat_max >= self.lat_min
            && leg.lon_min <= self.lon_max
            && leg.lon_max >= self.lon_min
    }
}

#[derive(Clone, Copy)]
struct LegBox {
    lat_min: f64,
    lat_max: f64,
    lon_min: f64,
    lon_max: f64,
}

pub struct AirportLineGrid<'a> {
    lines: &'a [AirportLineSegment],
    frames: Vec<Option<LineFrame>>,
    boxes: Vec<PaddedBox>,
    /// CSR grid over the lines' padded extent: `offsets[cell]..offsets[cell+1]`
    /// slice `refs`. Empty when every line is degenerate or global.
    offsets: Vec<u32>,
    refs: Vec<u32>,
    /// Lines no local grid can bound (polar longitude blowup) plus every line
    /// when the extent is absurdly wide: bbox-checked for every leg.
    global: Vec<u32>,
    seen: Vec<u32>,
    stamp: u32,
    candidates: Vec<usize>,
    lat_min: f64,
    lat_max: f64,
    lon_min: f64,
    lon_max: f64,
    cell_deg: f64,
    nx: usize,
    ny: usize,
    /// Dateline clusters unwrap into a +360-shifted frame; legs shift the same way.
    shifted: bool,
    buffer: f32,
}

impl<'a> AirportLineGrid<'a> {
    /// Offsets cap plus per-line frames, boxes, stamps and refs. A 4 km
    /// diagonal runway covers ~650 cells; 1024 refs per line bounds even that
    /// with room, and the whole allowance stays in the tens of megabytes.
    pub fn allocation_allowance(line_count: usize) -> Result<u64> {
        let bytes = 16 * 1024 * 1024
            + line_count as u128
                * (size_of::<Option<LineFrame>>()
                    + size_of::<PaddedBox>()
                    + 2 * size_of::<u32>()
                    + 1024 * size_of::<u32>()
                    + 2 * size_of::<usize>()) as u128;
        u64::try_from(bytes).context("ground line grid allocation overflow")
    }

    pub fn new(lines: &'a [AirportLineSegment], buffer: f32, allocation_limit: u64) -> Result<Self> {
        anyhow::ensure!(
            Self::allocation_allowance(lines.len())? <= allocation_limit,
            "ground line grid allocation exceeds admission"
        );
        let mut frames = Vec::with_capacity(lines.len());
        for line in lines {
            frames.push(LineFrame::new(line));
        }
        let mut grid = Self {
            lines,
            frames,
            boxes: vec![PaddedBox::default(); lines.len()],
            offsets: Vec::new(),
            refs: Vec::new(),
            global: Vec::new(),
            seen: vec![0; lines.len()],
            stamp: 0,
            candidates: Vec::new(),
            lat_min: 0.0,
            lat_max: 0.0,
            lon_min: 0.0,
            lon_max: 0.0,
            cell_deg: BASE_CELL_DEG,
            nx: 0,
            ny: 0,
            shifted: false,
            buffer,
        };
        grid.build(buffer);
        anyhow::ensure!(
            grid.actual_bytes() <= allocation_limit,
            "ground line grid exceeds admission"
        );
        Ok(grid)
    }

    fn actual_bytes(&self) -> u64 {
        (self.frames.capacity() * size_of::<Option<LineFrame>>()
            + self.boxes.capacity() * size_of::<PaddedBox>()
            + self.offsets.capacity() * size_of::<u32>()
            + self.refs.capacity() * size_of::<u32>()
            + self.global.capacity() * size_of::<u32>()
            + self.seen.capacity() * size_of::<u32>()
            + self.candidates.capacity() * size_of::<usize>()) as u64
    }

    /// Short-arc longitude range of one segment in the grid frame. Both ends
    /// unwrap together first (a dateline leg's raw min/max would invert),
    /// then shift together so the box never inverts.
    fn shift_range(&self, lon1: f64, lon2: f64) -> (f64, f64) {
        let unwrapped = lon1 + wrapped_longitude_delta(lon1, lon2);
        let (mut lo, mut hi) = if unwrapped < lon1 {
            (unwrapped, lon1)
        } else {
            (lon1, unwrapped)
        };
        if self.shifted && (lo + hi) * 0.5 < 0.0 {
            lo += 360.0;
            hi += 360.0;
        }
        (lo, hi)
    }

    fn is_global(&self, index: usize) -> bool {
        self.global.contains(&(index as u32))
    }

    fn build(&mut self, buffer: f32) {
        let reach = f64::from(buffer) + BBOX_SLACK_M;
        // Unwrap dateline clusters into one frame: a raw span over 180° means
        // the lines straddle ±180°, not that they span the planet.
        let mut lon_min = f64::INFINITY;
        let mut lon_max = f64::NEG_INFINITY;
        for line in self.lines.iter() {
            lon_min = lon_min.min(f64::from(line.start_lon).min(f64::from(line.end_lon)));
            lon_max = lon_max.max(f64::from(line.start_lon).max(f64::from(line.end_lon)));
        }
        self.shifted = lon_max - lon_min > 180.0;
        let mut valid = 0usize;
        let mut ext_min_lat = f64::INFINITY;
        let mut ext_max_lat = f64::NEG_INFINITY;
        let mut ext_min_lon = f64::INFINITY;
        let mut ext_max_lon = f64::NEG_INFINITY;
        for (index, line) in self.lines.iter().enumerate() {
            if self.frames[index].is_none() {
                continue;
            }
            let lat_pad = reach / f64::from(M_PER_DEG_LAT);
            let max_abs_lat = (f64::from(line.start_lat).abs().max(f64::from(line.end_lat).abs())
                + lat_pad)
                .min(89.999);
            let m_per_deg_lon =
                f64::from(M_PER_DEG_LON_EQUATOR) * max_abs_lat.to_radians().cos();
            let lon_pad = reach / m_per_deg_lon.max(1e-6);
            // Unbounded longitude padding (a pole inside the padded range)
            // matches every leg: an infinite box, never a missed one.
            if !lon_pad.is_finite() || lon_pad >= 180.0 {
                self.boxes[index] = PaddedBox {
                    lat_min: f64::NEG_INFINITY,
                    lat_max: f64::INFINITY,
                    lon_min: f64::NEG_INFINITY,
                    lon_max: f64::INFINITY,
                };
                self.global.push(index as u32);
                continue;
            }
            let lat_lo = f64::from(line.start_lat.min(line.end_lat)) - lat_pad;
            let lat_hi = f64::from(line.start_lat.max(line.end_lat)) + lat_pad;
            let (lon_lo, lon_hi) =
                self.shift_range(f64::from(line.start_lon), f64::from(line.end_lon));
            let (lon_lo, lon_hi) = (lon_lo - lon_pad, lon_hi + lon_pad);
            self.boxes[index] = PaddedBox {
                lat_min: lat_lo,
                lat_max: lat_hi,
                lon_min: lon_lo,
                lon_max: lon_hi,
            };
            valid += 1;
            ext_min_lat = ext_min_lat.min(lat_lo);
            ext_max_lat = ext_max_lat.max(lat_hi);
            ext_min_lon = ext_min_lon.min(lon_lo);
            ext_max_lon = ext_max_lon.max(lon_hi);
        }
        if valid == 0 {
            return;
        }
        // Absurdly wide extents skip the grid: every valid line joins the flat
        // global list instead of a distorted frame.
        if ext_max_lon - ext_min_lon > MAX_GRID_SPAN_DEG
            || ext_max_lat - ext_min_lat > MAX_GRID_SPAN_DEG
        {
            for index in 0..self.lines.len() {
                if self.frames[index].is_some() && !self.is_global(index) {
                    self.global.push(index as u32);
                }
            }
            return;
        }
        let mut cell_deg = BASE_CELL_DEG;
        while (ext_max_lon - ext_min_lon) / cell_deg > MAX_CELLS_PER_AXIS as f64
            || (ext_max_lat - ext_min_lat) / cell_deg > MAX_CELLS_PER_AXIS as f64
        {
            cell_deg *= 2.0;
        }
        let nx = ((ext_max_lon - ext_min_lon) / cell_deg).ceil() as usize + 1;
        let ny = ((ext_max_lat - ext_min_lat) / cell_deg).ceil() as usize + 1;
        let mut counts = vec![0u32; nx * ny];
        for index in 0..self.lines.len() {
            if self.frames[index].is_none() || self.is_global(index) {
                continue;
            }
            let (x0, x1, y0, y1) =
                self.cell_range(&self.boxes[index], ext_min_lon, ext_min_lat, cell_deg, nx, ny);
            for y in y0..=y1 {
                for x in x0..=x1 {
                    counts[y * nx + x] += 1;
                }
            }
        }
        let mut offsets = Vec::with_capacity(nx * ny + 1);
        offsets.push(0);
        for count in &counts {
            offsets.push(offsets.last().unwrap() + count);
        }
        let mut filled = vec![0u32; nx * ny];
        let mut refs = vec![0u32; *offsets.last().unwrap() as usize];
        for index in 0..self.lines.len() {
            if self.frames[index].is_none() || self.is_global(index) {
                continue;
            }
            let (x0, x1, y0, y1) =
                self.cell_range(&self.boxes[index], ext_min_lon, ext_min_lat, cell_deg, nx, ny);
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let cell = y * nx + x;
                    refs[(offsets[cell] + filled[cell]) as usize] = index as u32;
                    filled[cell] += 1;
                }
            }
        }
        self.offsets = offsets;
        self.refs = refs;
        self.lat_min = ext_min_lat;
        self.lat_max = ext_max_lat;
        self.lon_min = ext_min_lon;
        self.lon_max = ext_max_lon;
        self.cell_deg = cell_deg;
        self.nx = nx;
        self.ny = ny;
    }

    fn cell_range(
        &self,
        b: &PaddedBox,
        ext_min_lon: f64,
        ext_min_lat: f64,
        cell_deg: f64,
        nx: usize,
        ny: usize,
    ) -> (usize, usize, usize, usize) {
        let x0 = ((b.lon_min - ext_min_lon) / cell_deg).floor().max(0.0) as usize;
        let x1 = ((b.lon_max - ext_min_lon) / cell_deg).floor().max(0.0) as usize;
        let y0 = ((b.lat_min - ext_min_lat) / cell_deg).floor().max(0.0) as usize;
        let y1 = ((b.lat_max - ext_min_lat) / cell_deg).floor().max(0.0) as usize;
        (x0.min(nx - 1), x1.min(nx - 1), y0.min(ny - 1), y1.min(ny - 1))
    }

    fn leg_box(&self, leg: [f32; 4]) -> LegBox {
        let (lon_min, lon_max) = self.shift_range(f64::from(leg[1]), f64::from(leg[3]));
        LegBox {
            lat_min: f64::from(leg[0].min(leg[2])),
            lat_max: f64::from(leg[0].max(leg[2])),
            lon_min,
            lon_max,
        }
    }

    fn push_candidate(&mut self, index: u32, leg_box: &LegBox) {
        let slot = index as usize;
        if self.seen[slot] == self.stamp {
            return;
        }
        self.seen[slot] = self.stamp;
        if self.boxes[slot].contains_leg(leg_box) {
            self.candidates.push(slot);
        }
    }

    /// Narrow-phase hits in ascending line order with the buffer baked at
    /// build. `out` is reused across legs; contents equal the brute-force
    /// kernel exactly.
    pub fn project(&mut self, leg: [f32; 4], out: &mut Vec<LegIntersection>) {
        out.clear();
        self.candidates.clear();
        self.stamp = self.stamp.wrapping_add(1);
        if self.stamp == 0 {
            self.seen.fill(0);
            self.stamp = 1;
        }
        let leg_box = self.leg_box(leg);
        if !self.offsets.is_empty()
            && leg_box.lat_max >= self.lat_min
            && leg_box.lat_min <= self.lat_max
            && leg_box.lon_max >= self.lon_min
            && leg_box.lon_min <= self.lon_max
        {
            let x0 = ((leg_box.lon_min - self.lon_min) / self.cell_deg)
                .floor()
                .max(0.0) as usize;
            let x1 = ((leg_box.lon_max - self.lon_min) / self.cell_deg)
                .floor()
                .max(0.0) as usize;
            let y0 = ((leg_box.lat_min - self.lat_min) / self.cell_deg)
                .floor()
                .max(0.0) as usize;
            let y1 = ((leg_box.lat_max - self.lat_min) / self.cell_deg)
                .floor()
                .max(0.0) as usize;
            for y in y0.min(self.ny - 1)..=y1.min(self.ny - 1) {
                for x in x0.min(self.nx - 1)..=x1.min(self.nx - 1) {
                    let cell = y * self.nx + x;
                    for i in self.offsets[cell]..self.offsets[cell + 1] {
                        let index = self.refs[i as usize];
                        self.push_candidate(index, &leg_box);
                    }
                }
            }
        }
        for i in 0..self.global.len() {
            let index = self.global[i];
            self.push_candidate(index, &leg_box);
        }
        self.candidates.sort_unstable();
        collect_intersections(
            flat_dist(leg[0], leg[1], leg[2], leg[3]),
            self.candidates.len(),
            self.candidates.iter().map(|&index| {
                (
                    &self.lines[index],
                    clipped_overlap_in_frame(
                        leg[0],
                        leg[1],
                        leg[2],
                        leg[3],
                        self.frames[index].unwrap(),
                        self.buffer,
                    ),
                )
            }),
            out,
        );
    }
}

#[cfg(test)]
#[path = "airport_line_grid_tests.rs"]
mod tests;
