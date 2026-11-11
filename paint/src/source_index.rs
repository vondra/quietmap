//! Source candidates indexed by position and conservative reach.

use popup::candidates::{Candidate, GROUND_REACH_M};

/// Side of the index's cells (m).
const INDEX_CELL_M: f64 = 100.0;
/// The index's reach classes (m): a source sits in the first that covers its reach.
const REACH_CLASSES_M: [f64; 7] = [
    250.0,
    500.0,
    1_000.0,
    2_000.0,
    4_000.0,
    8_000.0,
    GROUND_REACH_M,
];

/// The candidates by place and reach: per reach class a grid, a point asking each class within
/// that class's reach.
pub struct Index {
    classes: Vec<(f64, Grid)>,
}

impl Index {
    pub(super) fn new(candidates: &[Candidate], reaches: &[f64]) -> Self {
        let mut members = vec![Vec::new(); REACH_CLASSES_M.len()];
        for (index, &reach) in reaches.iter().enumerate() {
            let class = REACH_CLASSES_M
                .iter()
                .position(|&class| reach <= class)
                .unwrap_or(REACH_CLASSES_M.len() - 1);
            members[class].push(index as u32);
        }
        Index {
            classes: REACH_CLASSES_M
                .iter()
                .zip(members)
                .map(|(&reach, members)| (reach, Grid::new(candidates, members)))
                .collect(),
        }
    }

    /// Every candidate whose reach may cover a point of the rectangle `low`-`high` (frame
    /// metres): every one loud there, and some not.
    pub fn reaching(&self, low: [f64; 2], high: [f64; 2], out: &mut Vec<u32>) {
        out.clear();
        for (reach, grid) in &self.classes {
            grid.within(low, high, *reach, out);
        }
    }

    /// Every candidate that may lie within `radius_m` of the rectangle, and some farther.
    pub fn within(&self, low: [f64; 2], high: [f64; 2], radius_m: f64, out: &mut Vec<u32>) {
        out.clear();
        for (_, grid) in &self.classes {
            grid.within(low, high, radius_m, out);
        }
    }
}

/// One reach class's candidates by place: short pieces by the cell of their middle, long ones
/// always.
struct Grid {
    origin: [f64; 2],
    columns: usize,
    rows: usize,
    starts: Vec<u32>,
    items: Vec<u32>,
    long: Vec<u32>,
}

impl Grid {
    fn new(candidates: &[Candidate], members: Vec<u32>) -> Self {
        let middle = |index: u32| {
            let [a, b] = candidates[index as usize].ends_m;
            [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]
        };
        let is_long = |index: u32| {
            let [a, b] = candidates[index as usize].ends_m;
            (b[0] - a[0]).hypot(b[1] - a[1]) > 2.0 * INDEX_CELL_M
        };
        let (long, short): (Vec<u32>, Vec<u32>) =
            members.into_iter().partition(|&index| is_long(index));
        let (mut low, mut high) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
        for &index in &short {
            let m = middle(index);
            for axis in 0..2 {
                low[axis] = low[axis].min(m[axis]);
                high[axis] = high[axis].max(m[axis]);
            }
        }
        if short.is_empty() {
            (low, high) = ([0.0; 2], [0.0; 2]);
        }
        let columns = ((high[0] - low[0]) / INDEX_CELL_M) as usize + 1;
        let rows = ((high[1] - low[1]) / INDEX_CELL_M) as usize + 1;
        let cell = |m: [f64; 2]| {
            let column = ((m[0] - low[0]) / INDEX_CELL_M) as usize;
            let row = ((m[1] - low[1]) / INDEX_CELL_M) as usize;
            row.min(rows - 1) * columns + column.min(columns - 1)
        };
        let mut starts = vec![0u32; columns * rows + 1];
        for &index in &short {
            starts[cell(middle(index)) + 1] += 1;
        }
        for i in 1..starts.len() {
            starts[i] += starts[i - 1];
        }
        let mut next = starts.clone();
        let mut items = vec![0u32; short.len()];
        for &index in &short {
            let slot = &mut next[cell(middle(index))];
            items[*slot as usize] = index;
            *slot += 1;
        }
        Grid {
            origin: low,
            columns,
            rows,
            starts,
            items,
            long,
        }
    }

    /// Appends every member that may lie within `radius_m` of the rectangle `low`-`high`, and
    /// some farther.
    fn within(&self, low: [f64; 2], high: [f64; 2], radius_m: f64, out: &mut Vec<u32>) {
        out.extend_from_slice(&self.long);
        // A short piece reaches at most a cell from its middle.
        let reach = radius_m + INDEX_CELL_M;
        let span = |value: f64, origin: f64, limit: usize| {
            ((value - origin) / INDEX_CELL_M)
                .floor()
                .clamp(0.0, (limit - 1) as f64) as usize
        };
        if self.items.is_empty()
            || high[0] + reach < self.origin[0]
            || high[1] + reach < self.origin[1]
        {
            return;
        }
        let (c0, c1) = (
            span(low[0] - reach, self.origin[0], self.columns),
            span(high[0] + reach, self.origin[0], self.columns),
        );
        let (r0, r1) = (
            span(low[1] - reach, self.origin[1], self.rows),
            span(high[1] + reach, self.origin[1], self.rows),
        );
        for row in r0..=r1 {
            let first = row * self.columns;
            let (from, to) = (
                self.starts[first + c0] as usize,
                self.starts[first + c1 + 1] as usize,
            );
            out.extend_from_slice(&self.items[from..to]);
        }
    }
}
