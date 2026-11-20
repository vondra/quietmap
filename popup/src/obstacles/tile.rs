//! One read tile of a scene: its parsed file and what the walks need beyond it, computed once.

use super::reflection_coverage::{ReflectionCoverage, is_tall_building_outline};
use rayon::prelude::*;
use std::sync::OnceLock;
use tiles::obstacles::{CELLS, Obstacles, OutlineKind};

/// Outlines per parallel task when a tile is prepared.
const OUTLINE_CHUNK: usize = 16_384;
/// Cells and runs per parallel task.
const CELL_CHUNK: usize = 1_024;
const RUN_CHUNK: usize = 65_536;

/// One read tile with what the walks need beyond its file.
pub(super) struct SceneTile<'a> {
    pub(super) reflection_coverage: OnceLock<Box<ReflectionCoverage>>,
    pub(super) tall_building_extent: Option<([i16; 2], [i16; 2])>,
    pub(super) obstacles: Obstacles<'a>,
    /// Scene steps of the tile's local origin.
    pub(super) offset: [i64; 2],
    /// Per run, its first vertex (the outline's first vertex plus the run's first edge).
    pub(super) run_first_vertex: Vec<u32>,
    /// Per cell, the tallest outline listed there (the skyline's prune).
    pub(super) cell_max_height_m: Vec<f64>,
    /// Each whole ring has the same western bound in every tile storing it.
    pub(super) outline_west: Vec<i16>,
}

impl<'a> SceneTile<'a> {
    /// The tile and the east-west extent of its widest building, in steps. A dense tile holds
    /// millions of outlines: outlines and cells are prepared in parallel chunks.
    pub(super) fn new(obstacles: Obstacles<'a>, offset: [i64; 2]) -> (Self, i64) {
        let count = obstacles.outline_count();
        let chunks: Vec<_> = (0..count)
            .into_par_iter()
            .step_by(OUTLINE_CHUNK)
            .map(|start| {
                let end = (start + OUTLINE_CHUNK).min(count);
                let mut outline_west = Vec::with_capacity(end - start);
                let mut widest = 0;
                let mut tall_first = [i16::MAX; 2];
                let mut tall_last = [i16::MIN; 2];
                for outline in start..end {
                    let record = obstacles.outline(outline);
                    let tall = is_tall_building_outline(obstacles, outline);
                    let (mut west, mut east) = (i16::MAX, i16::MIN);
                    for index in record.first_vertex..record.first_vertex + record.vertex_count {
                        let vertex = obstacles.vertex(index);
                        west = west.min(vertex[0]);
                        east = east.max(vertex[0]);
                        if tall {
                            for axis in 0..2 {
                                tall_first[axis] = tall_first[axis].min(vertex[axis]);
                                tall_last[axis] = tall_last[axis].max(vertex[axis]);
                            }
                        }
                    }
                    outline_west.push(west);
                    if record.kind != OutlineKind::Wall {
                        widest = widest.max(i64::from(east) - i64::from(west));
                    }
                }
                (outline_west, widest, tall_first, tall_last)
            })
            .collect();
        let widest = chunks.iter().map(|chunk| chunk.1).max().unwrap_or(0);
        let tall_first = [0, 1].map(|axis| {
            chunks
                .iter()
                .map(|chunk| chunk.2[axis])
                .min()
                .unwrap_or(i16::MAX)
        });
        let tall_last = [0, 1].map(|axis| {
            chunks
                .iter()
                .map(|chunk| chunk.3[axis])
                .max()
                .unwrap_or(i16::MIN)
        });
        let tall_building_extent =
            (tall_first[0] <= tall_last[0]).then_some((tall_first, tall_last));
        let outline_west = chunks.into_iter().flat_map(|chunk| chunk.0).collect();
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
                    .map(|index| {
                        obstacles
                            .outline(obstacles.run(index).outline as usize)
                            .height_m
                    })
                    .fold(0.0, f64::max)
            })
            .collect();
        let tile = SceneTile {
            reflection_coverage: OnceLock::new(),
            tall_building_extent,
            obstacles,
            offset,
            run_first_vertex,
            cell_max_height_m,
            outline_west,
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
