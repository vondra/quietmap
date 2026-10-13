//! A receiver extent's tall-building union on the fixed lattice, with exact integer area sums.

use super::tile::SceneTile;
use std::collections::HashMap;
use std::ops::Range;
use tiles::geo::STEPS_PER_TILE;
use tiles::obstacles::Obstacles;

/// At 600 Prague receivers, eight steps differed from four by at most 0.024928 dB.
pub(super) const REFLECTION_LATTICE_STEPS: usize = 8;
const SIDE: usize = STEPS_PER_TILE as usize / REFLECTION_LATTICE_STEPS;
/// The former enclosure rule counts footprints strictly taller than 5 m.
const REFLECTION_BUILT_ABOVE_M: f64 = 5.0;
const OVERLAPPING: u32 = u32::MAX;

struct ExclusiveRun {
    row: u16,
    first: u16,
    last: u16,
}

pub(super) struct ReflectionCoverage {
    prepared_first: [usize; 2],
    prepared_last: [usize; 2],
    first: [usize; 2],
    last: [usize; 2],
    stride: usize,
    summed_area: Vec<u32>,
    exclusive_runs: HashMap<u64, Vec<ExclusiveRun>>,
}

pub(super) fn is_tall_building_outline(obstacles: Obstacles<'_>, index: usize) -> bool {
    let outline = obstacles.outline(index);
    outline.kind.is_building() && outline.height_m > REFLECTION_BUILT_ABOVE_M
}

pub(super) fn raster_coordinate(steps: f64) -> f64 {
    (steps + STEPS_PER_TILE / 2.0) / REFLECTION_LATTICE_STEPS as f64
}

fn centres_between(first: f64, last: f64, extent: Range<usize>) -> Range<usize> {
    let bound = |coordinate: f64| {
        (coordinate - 0.5)
            .ceil()
            .clamp(extent.start as f64, extent.end as f64) as usize
    };
    bound(first)..bound(last)
}

fn rasterize_footprint(
    obstacles: Obstacles<'_>,
    outlines: Range<usize>,
    owner: u32,
    coverage: &mut ReflectionCoverage,
) {
    let mut north = f64::INFINITY;
    let mut south = f64::NEG_INFINITY;
    for index in outlines
        .clone()
        .filter(|&index| is_tall_building_outline(obstacles, index))
    {
        let outline = obstacles.outline(index);
        for vertex in outline.first_vertex..outline.first_vertex + outline.vertex_count {
            let y = f64::from(obstacles.vertex(vertex)[1]);
            north = north.min(y);
            south = south.max(y);
        }
    }
    let mut crossings = Vec::new();
    for row in centres_between(
        raster_coordinate(north),
        raster_coordinate(south),
        coverage.first[1]..coverage.last[1],
    ) {
        let y = (row as f64 + 0.5) * REFLECTION_LATTICE_STEPS as f64 - STEPS_PER_TILE / 2.0;
        crossings.clear();
        for index in outlines
            .clone()
            .filter(|&index| is_tall_building_outline(obstacles, index))
        {
            let outline = obstacles.outline(index);
            let mut a = obstacles.vertex(outline.first_vertex).map(f64::from);
            for vertex in outline.first_vertex + 1..outline.first_vertex + outline.vertex_count {
                let b = obstacles.vertex(vertex).map(f64::from);
                if (a[1] > y) != (b[1] > y) {
                    crossings.push(a[0] + (y - a[1]) * (b[0] - a[0]) / (b[1] - a[1]));
                }
                a = b;
            }
        }
        // Rings of one footprint share parity, so holes remain empty before footprints are united.
        crossings.sort_unstable_by(f64::total_cmp);
        for ends in crossings.chunks_exact(2) {
            let columns = centres_between(
                raster_coordinate(ends[0]),
                raster_coordinate(ends[1]),
                coverage.first[0]..coverage.last[0],
            );
            if columns.is_empty() {
                continue;
            }
            let start = coverage.index(columns.start + 1, row + 1);
            let end = coverage.index(columns.end + 1, row + 1);
            for previous in &mut coverage.summed_area[start..end] {
                *previous = if *previous == 0 || *previous == owner {
                    owner
                } else {
                    OVERLAPPING
                };
            }
        }
    }
}

impl SceneTile<'_> {
    pub(super) fn reflection_extent(
        &self,
        low: [f64; 2],
        high: [f64; 2],
    ) -> Option<([usize; 2], [usize; 2])> {
        let (first, last) = self.tall_building_extent?;
        let ranges = [0, 1].map(|axis| {
            centres_between(
                raster_coordinate(f64::from(first[axis])),
                raster_coordinate(f64::from(last[axis])),
                (low[axis].floor().clamp(0.0, SIDE as f64) as usize)
                    ..(high[axis].ceil().clamp(0.0, SIDE as f64) as usize),
            )
        });
        ranges.iter().all(|range| !range.is_empty()).then(|| {
            (
                ranges.each_ref().map(|range| range.start),
                ranges.each_ref().map(|range| range.end),
            )
        })
    }
}

impl ReflectionCoverage {
    pub(super) fn new(tile: &SceneTile<'_>, low: [f64; 2], high: [f64; 2]) -> Self {
        let (first, last) = tile
            .reflection_extent(low, high)
            .expect("reachable tall cells");
        let obstacles = tile.obstacles;
        let mut coverage = Self {
            prepared_first: low.map(|value| value.floor().clamp(0.0, SIDE as f64) as usize),
            prepared_last: high.map(|value| value.ceil().clamp(0.0, SIDE as f64) as usize),
            first,
            last,
            stride: last[0] - first[0] + 1,
            summed_area: vec![0; (last[0] - first[0] + 1) * (last[1] - first[1] + 1)],
            exclusive_runs: HashMap::new(),
        };
        let mut footprints = Vec::new();
        let mut first = 0;
        while first < obstacles.outline_count() {
            let id = obstacles.outline(first).footprint_id;
            let last = (first + 1..obstacles.outline_count())
                .find(|&index| obstacles.outline(index).footprint_id != id)
                .unwrap_or(obstacles.outline_count());
            if (first..last).any(|index| is_tall_building_outline(obstacles, index)) {
                footprints.push(id);
                rasterize_footprint(
                    obstacles,
                    first..last,
                    footprints.len() as u32,
                    &mut coverage,
                );
            }
            first = last;
        }
        for row in coverage.first[1]..coverage.last[1] {
            let mut sum = 0;
            let mut column = coverage.first[0];
            while column < coverage.last[0] {
                let owner = coverage.summed_area[coverage.index(column + 1, row + 1)];
                let first = column;
                // Consume each owner before replacing it by its prefix; the row above is sums.
                while column < coverage.last[0]
                    && coverage.summed_area[coverage.index(column + 1, row + 1)] == owner
                {
                    sum += u32::from(owner != 0);
                    let index = coverage.index(column + 1, row + 1);
                    coverage.summed_area[index] =
                        coverage.summed_area[coverage.index(column + 1, row)] + sum;
                    column += 1;
                }
                if owner != 0 && owner != OVERLAPPING {
                    coverage
                        .exclusive_runs
                        .entry(footprints[owner as usize - 1])
                        .or_default()
                        .push(ExclusiveRun {
                            row: row as u16,
                            first: first as u16,
                            last: column as u16,
                        });
                }
            }
        }
        coverage
    }

    fn index(&self, column: usize, row: usize) -> usize {
        (row - self.first[1]) * self.stride + column - self.first[0]
    }

    fn whole_cells(&self, x: (usize, usize), y: (usize, usize)) -> u32 {
        // Clip integer sums, not the window's fractional segments: splitting a weighted sum
        // at the crop boundary would change its floating-point rounding.
        let x = (x.0.max(self.first[0]), x.1.min(self.last[0]));
        let y = (y.0.max(self.first[1]), y.1.min(self.last[1]));
        if x.0 >= x.1 || y.0 >= y.1 {
            return 0;
        }
        let at = |column, row| self.summed_area[self.index(column, row)];
        at(x.1, y.1) + at(x.0, y.0) - at(x.0, y.1) - at(x.1, y.0)
    }

    pub(super) fn covers(&self, low: [f64; 2], high: [f64; 2]) -> bool {
        (0..2).all(|axis| {
            low[axis].max(0.0) >= self.prepared_first[axis] as f64
                && high[axis].min(SIDE as f64) <= self.prepared_last[axis] as f64
        })
    }

    pub(super) fn covered(&self, low: [f64; 2], high: [f64; 2], own: Option<u64>) -> f64 {
        let low = low.map(|value| value.clamp(0.0, SIDE as f64));
        let high = high.map(|value| value.clamp(0.0, SIDE as f64));
        let segments = |low: f64, high: f64| {
            let first = low.floor() as usize;
            let last = high.floor() as usize;
            if first == last {
                [(first, first + 1, high - low), (0, 0, 0.0), (0, 0, 0.0)]
            } else {
                [
                    (first, first + 1, 1.0 - low.fract()),
                    (first + 1, last, 1.0),
                    (last, last + 1, high.fract()),
                ]
            }
        };
        let mut area = 0.0;
        // Subtract integer prefixes before weighting edges: the result is bit-identical for any extent.
        for (north, south, height) in segments(low[1], high[1]) {
            for (west, east, width) in segments(low[0], high[0]) {
                if height > 0.0 && width > 0.0 && south > north && east > west {
                    area +=
                        f64::from(self.whole_cells((west, east), (north, south))) * width * height;
                }
            }
        }
        if let Some(runs) = own.and_then(|id| self.exclusive_runs.get(&id)) {
            let first = runs.partition_point(|run| f64::from(run.row) + 1.0 <= low[1]);
            for run in runs[first..]
                .iter()
                .take_while(|run| f64::from(run.row) < high[1])
            {
                let width =
                    (high[0].min(f64::from(run.last)) - low[0].max(f64::from(run.first))).max(0.0);
                let height = high[1].min(f64::from(run.row) + 1.0) - low[1].max(f64::from(run.row));
                area -= width * height;
            }
        }
        area.max(0.0)
    }
}
