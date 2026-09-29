//! Exact crossings of a ray with the obstacles (dev4 `crossings.rs`): an Amanatides & Woo walk
//! over the cells the ray passes, then dev4's padded box filter and exact segment test on the
//! edges each cell lists.

use super::{Scene, SceneTile, cell_of};
use physics::ray::Crossing;
use std::ops::Range;
use tiles::obstacles::CELL_STEPS;

/// Chainage of the intersection of the ray `start + t delta` with the segment `a-b`, strictly
/// inside the ray (t in (0, 1)) and on the closed segment (dev4 `segment_intersection_t`).
/// Collinear overlap is no crossing: a ray sliding along a wall grazes it.
pub(super) fn segment_intersection_t(
    start: [f64; 2],
    delta: [f64; 2],
    a: [f64; 2],
    b: [f64; 2],
) -> Option<f64> {
    let (ex, ey) = (b[0] - a[0], b[1] - a[1]);
    let denominator = delta[0] * ey - delta[1] * ex;
    if denominator == 0.0 {
        return None;
    }
    let (wx, wy) = (a[0] - start[0], a[1] - start[1]);
    let t = (wx * ey - wy * ex) / denominator;
    let u = (wx * delta[1] - wy * delta[0]) / denominator;
    (t > 0.0 && t < 1.0 && (0.0..=1.0).contains(&u)).then_some(t)
}

/// Sorts by t with one crossing per footprint and chainage: an edge listed in several cells or
/// tiles meets the ray at bit-identical t, and the two edges of one footprint meeting at a hit
/// vertex differ in the last bits (a tangent ray keeps one conservative crossing). A crossing is
/// dropped when its footprint already kept one less than 1e-9 before it.
pub(super) fn sort_and_deduplicate(crossings: &mut Vec<Crossing>) {
    crossings.sort_unstable_by(|a, b| {
        a.t.total_cmp(&b.t)
            .then(a.footprint_id.cmp(&b.footprint_id))
    });
    let mut kept = 0;
    for index in 0..crossings.len() {
        let candidate = crossings[index];
        let repeat = crossings[..kept]
            .iter()
            .rev()
            .take_while(|earlier| candidate.t - earlier.t < 1e-9)
            .any(|earlier| earlier.footprint_id == candidate.footprint_id);
        if !repeat {
            crossings[kept] = candidate;
            kept += 1;
        }
    }
    crossings.truncate(kept);
}

/// The box of a ray piece between `ends` (scene steps), padded and rounded outward into the
/// int16 frame of a tile whose local origin sits at `offset`: `[low, high]` corners.
pub(super) fn piece_box(ends: [[f64; 2]; 2], pad: f64, offset: [i64; 2]) -> [[i32; 2]; 2] {
    let [p, q] = ends;
    let local = |value: f64, axis: usize| value - offset[axis] as f64;
    [
        [0, 1].map(|axis| local(p[axis].min(q[axis]) - pad, axis).floor() as i32),
        [0, 1].map(|axis| local(p[axis].max(q[axis]) + pad, axis).ceil() as i32),
    ]
}

/// Whether the box of the edge `a-b` (tile-local) meets a piece box, boundaries included.
pub(super) fn edge_meets_box(a: [i16; 2], b: [i16; 2], [low, high]: [[i32; 2]; 2]) -> bool {
    (0..2).all(|axis| {
        high[axis] >= i32::from(a[axis].min(b[axis]))
            && i32::from(a[axis].max(b[axis])) >= low[axis]
    })
}

impl SceneTile<'_> {
    /// Appends the crossings of the ray `start + t delta` with the edges of the runs `runs`, the
    /// ray's visit of their cell covering the piece between `ends`. The piece's box passes every
    /// edge the exact test can meet there; an outline's record is read only for a crossing.
    fn append_crossings(
        &self,
        runs: Range<usize>,
        start: [f64; 2],
        delta: [f64; 2],
        ends: [[f64; 2]; 2],
        pad: f64,
        out: &mut Vec<Crossing>,
    ) {
        let piece = piece_box(ends, pad, self.offset);
        for index in runs {
            let run = self.obstacles.run(index);
            let first = self.run_first_vertex[index] as usize;
            let mut a = self.obstacles.vertex(first);
            for next in first + 1..=first + usize::from(run.edge_count) {
                let b = self.obstacles.vertex(next);
                if edge_meets_box(a, b, piece)
                    && let Some(t) =
                        segment_intersection_t(start, delta, self.steps(a), self.steps(b))
                {
                    let record = self.obstacles.outline(run.outline as usize);
                    out.push(Crossing {
                        t,
                        height_m: record.height_m,
                        building: record.kind.is_building(),
                        footprint_id: record.footprint_id,
                    });
                }
                a = b;
            }
        }
    }
}

impl Scene<'_> {
    /// Every crossing of the segment `from -> to` (click metres) with a building ring or wall,
    /// endpoints excluded, sorted by t. A footprint containing an endpoint still reports its
    /// other walls; excluding a source's own building is the caller's job, by footprint id.
    /// Touching a cell of a tile that was not read is an error.
    pub fn crossings(
        &self,
        from: [f64; 2],
        to: [f64; 2],
        out: &mut Vec<Crossing>,
    ) -> Result<(), String> {
        out.clear();
        let start = self.lattice.steps(from);
        let end = self.lattice.steps(to);
        let delta = [end[0] - start[0], end[1] - start[1]];
        let cell_steps = CELL_STEPS as f64;
        let mut cell = cell_of(start);
        let last = cell_of(end);
        let step = delta.map(|d| if d >= 0.0 { 1 } else { -1 });
        let t_delta = delta.map(|d| {
            if d != 0.0 {
                (cell_steps / d).abs()
            } else {
                f64::INFINITY
            }
        });
        let mut t_max = [0, 1].map(|axis| {
            if delta[axis] == 0.0 {
                return f64::INFINITY;
            }
            let boundary = (cell[axis] + i64::from(delta[axis] >= 0.0)) as f64 * cell_steps;
            ((boundary - start[axis]) / delta[axis]).abs()
        });
        // The walk's boundaries accumulate t_delta while the exact test takes its own cross
        // products: the pad covers their last-bit disagreement and cannot change the exact answer.
        let pad = 1e-9 * (1.0 + delta[0].abs() + delta[1].abs());
        let along = |t: f64| [start[0] + delta[0] * t, start[1] + delta[1] * t];
        let mut t_enter = 0.0_f64;
        loop {
            if let Some((tile, index)) = self.tile_at(cell)? {
                let runs = tile.obstacles.cell_run_range(index);
                if !runs.is_empty() {
                    let t_exit = t_max[0].min(t_max[1]).min(1.0);
                    let ends = [
                        along(t_enter.clamp(0.0, 1.0)),
                        along(t_exit.clamp(0.0, 1.0)),
                    ];
                    tile.append_crossings(runs, start, delta, ends, pad, out);
                }
            }
            if cell == last {
                break;
            }
            t_enter = t_max[0].min(t_max[1]);
            // A tie steps the row, as the builder's supercover does; the end column or row is
            // never passed (see tiles::obstacles::for_each_edge_cell).
            let axis = if cell[0] == last[0] {
                1
            } else if cell[1] == last[1] || t_max[0] < t_max[1] {
                0
            } else {
                1
            };
            t_max[axis] += t_delta[axis];
            cell[axis] += step[axis];
        }
        sort_and_deduplicate(out);
        Ok(())
    }
}
