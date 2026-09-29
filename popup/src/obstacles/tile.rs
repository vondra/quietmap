//! One read tile of a scene: its parsed file and what the walks need beyond it, computed once.

use rayon::prelude::*;
use tiles::obstacles::{CELLS, Obstacles, OutlineKind};

/// Outlines per parallel task when a tile is prepared (extended to a footprint's end).
const FOOTPRINT_CHUNK: usize = 16_384;
/// Cells and runs per parallel task.
const CELL_CHUNK: usize = 1_024;
const RUN_CHUNK: usize = 65_536;

/// One read tile with what the walks need beyond its file.
pub(super) struct SceneTile<'a> {
    pub(super) obstacles: Obstacles<'a>,
    /// Scene steps of the tile's local origin.
    pub(super) offset: [i64; 2],
    /// Per run, its first vertex (the outline's first vertex plus the run's first edge).
    pub(super) run_first_vertex: Vec<u32>,
    /// Per cell, the tallest outline listed there (the skyline's prune).
    pub(super) cell_max_height_m: Vec<f64>,
    /// Per outline, the smallest local x of its footprint's rings (the containment walk skips
    /// footprints lying east of the probe).
    pub(super) footprint_west: Vec<i16>,
}

impl<'a> SceneTile<'a> {
    /// The tile and the east-west extent of its widest building, in steps. A dense tile holds
    /// millions of outlines: footprints and cells are prepared in parallel chunks.
    pub(super) fn new(obstacles: Obstacles<'a>, offset: [i64; 2]) -> (Self, i64) {
        let count = obstacles.outline_count();
        // Chunks start at footprint boundaries, so each footprint lies in one chunk.
        let mut starts = vec![0];
        while let Some(&last) = starts.last() {
            let mut next = last + FOOTPRINT_CHUNK;
            if next >= count {
                break;
            }
            let id = obstacles.outline(next - 1).footprint_id;
            while next < count && obstacles.outline(next).footprint_id == id {
                next += 1;
            }
            if next >= count {
                break;
            }
            starts.push(next);
        }
        starts.push(count);
        let chunks: Vec<(Vec<i16>, i64)> = starts
            .par_windows(2)
            .map(|range| {
                let (from, to) = (range[0], range[1]);
                let mut footprint_west = vec![0; to - from];
                let mut widest = 0;
                let mut start = from;
                while start < to {
                    let id = obstacles.outline(start).footprint_id;
                    let (mut west, mut east, mut end) = (i16::MAX, i16::MIN, start);
                    while end < to && obstacles.outline(end).footprint_id == id {
                        let record = obstacles.outline(end);
                        for index in record.first_vertex..record.first_vertex + record.vertex_count
                        {
                            let [x, _] = obstacles.vertex(index);
                            (west, east) = (west.min(x), east.max(x));
                        }
                        end += 1;
                    }
                    footprint_west[start - from..end - from].fill(west);
                    if obstacles.outline(start).kind != OutlineKind::Wall {
                        widest = widest.max(i64::from(east) - i64::from(west));
                    }
                    start = end;
                }
                (footprint_west, widest)
            })
            .collect();
        let widest = chunks.iter().map(|chunk| chunk.1).max().unwrap_or(0);
        let footprint_west = chunks.into_iter().flat_map(|chunk| chunk.0).collect();
        let run_first_vertex = (0..obstacles.run_count())
            .into_par_iter()
            .with_min_len(RUN_CHUNK)
            .map(|index| {
                let run = obstacles.run(index);
                let record = obstacles.outline(run.outline as usize);
                (record.first_vertex + usize::from(run.first_edge)) as u32
            })
            .collect();
        let cell_max_height_m = (0..CELLS)
            .into_par_iter()
            .with_min_len(CELL_CHUNK)
            .map(|cell| {
                obstacles
                    .cell_run_range(cell)
                    .map(|index| obstacles.outline(obstacles.run(index).outline as usize).height_m)
                    .fold(0.0, f64::max)
            })
            .collect();
        let tile = SceneTile {
            obstacles,
            offset,
            run_first_vertex,
            cell_max_height_m,
            footprint_west,
        };
        (tile, widest)
    }

    /// A tile-local vertex in scene steps.
    pub(super) fn steps(&self, vertex: [i16; 2]) -> [f64; 2] {
        [
            (self.offset[0] + i64::from(vertex[0])) as f64,
            (self.offset[1] + i64::from(vertex[1])) as f64,
        ]
    }

    /// A stored vertex in scene steps.
    pub(super) fn vertex(&self, index: usize) -> [f64; 2] {
        self.steps(self.obstacles.vertex(index))
    }

    /// The outlines of one footprint here (outlines are sorted by footprint id).
    pub(super) fn outlines_of(&self, footprint_id: u64) -> std::ops::Range<usize> {
        let id_at = |index: usize| self.obstacles.outline(index).footprint_id;
        let count = self.obstacles.outline_count();
        let (mut start, mut high) = (0, count);
        while start < high {
            let middle = (start + high) / 2;
            if id_at(middle) < footprint_id {
                start = middle + 1;
            } else {
                high = middle;
            }
        }
        let end = (start..count)
            .find(|&index| id_at(index) != footprint_id)
            .unwrap_or(count);
        start..end
    }
}
