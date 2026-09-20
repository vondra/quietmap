//! Freeze obstacle edges into a density-sized CSR grid and containment bounds.

use super::{Builder, ObstacleEdge, ObstacleIndex};
use crate::envelope::EnvelopeClass;

/// Mean occupancy from the 2026-09-07 metro pitch benchmark: 3,917 edges/km²
/// at 32 m gives 4.0 edges/cell. GPU times: 64 m 10.94 s, 32 m 9.15 s,
/// 24 m 9.14 s, 16 m 10.53 s; finer grids exceed the cache working set.
const OBSTACLE_GRID_EDGES_PER_CELL: f64 = 4.0;

const OBSTACLE_GRID_CELL_MIN_M: f64 = 32.0;

/// Balance cells walked against edges tested; empty stocks use the pitch floor.
pub(super) fn obstacle_grid_cell_m(span_x_m: f64, span_y_m: f64, edge_count: usize) -> f64 {
    let area_m2 = span_x_m.max(1.0) * span_y_m.max(1.0);
    (area_m2 * OBSTACLE_GRID_EDGES_PER_CELL / edge_count.max(1) as f64)
        .sqrt()
        .max(OBSTACLE_GRID_CELL_MIN_M)
}

impl Builder {
    /// Freeze into the CSR grid index at the pitch the stock's own edge
    /// density asks for. Empty builder yields an index whose `crossings` is
    /// a no-op (the rural fast path).
    pub fn build(self) -> ObstacleIndex {
        let bounds = self.edge_bounds();
        let (min_x, min_y, max_x, max_y) = bounds;
        let cell_m = obstacle_grid_cell_m(max_x - min_x, max_y - min_y, self.edges.len());
        self.build_at_pitch_m(bounds, cell_m)
    }

    /// The edges' bounding box `(min_x, min_y, max_x, max_y)` in the index's
    /// local metric frame — one pass over the stock, handed to both the pitch
    /// and the grid it sizes. An empty builder yields the inverted box, which
    /// [`obstacle_grid_cell_m`] answers with the pitch floor and
    /// [`Self::build_at_pitch_m`] never reads.
    pub(super) fn edge_bounds(&self) -> (f64, f64, f64, f64) {
        let (mut min_x, mut min_y) = (f64::MAX, f64::MAX);
        let (mut max_x, mut max_y) = (f64::MIN, f64::MIN);
        for e in &self.edges {
            min_x = min_x.min(e.x0 as f64).min(e.x1 as f64);
            min_y = min_y.min(e.y0 as f64).min(e.y1 as f64);
            max_x = max_x.max(e.x0 as f64).max(e.x1 as f64);
            max_y = max_y.max(e.y0 as f64).max(e.y1 as f64);
        }
        (min_x, min_y, max_x, max_y)
    }

    /// Freeze into the CSR grid at a pitch and bounding box the caller names —
    /// [`Self::build`] derives both from the stock; the boundary sweep test
    /// pins the pitch so its fixtures sit exactly on cell edges.
    pub(super) fn build_at_pitch_m(
        mut self,
        bounds: (f64, f64, f64, f64),
        cell_m: f64,
    ) -> ObstacleIndex {
        debug_assert!(cell_m.is_finite() && cell_m > 0.0, "grid pitch {cell_m}");
        let (min_x, min_y, max_x, max_y) = bounds;
        if self.edges.is_empty() {
            return ObstacleIndex {
                origin_lat: self.origin_lat,
                origin_lon: self.origin_lon,
                m_per_deg_lon: self.m_per_deg_lon,
                cell_m,
                min_x: 0.0,
                min_y: 0.0,
                cols: 1,
                rows: 1,
                cell_starts: vec![0, 0].into(),
                edge_refs: Vec::new().into(),
                edges: Vec::new().into(),
                cell_max_h: vec![0.0].into(),
                footprint_xmin: Vec::new().into(),
                footprint_class: Vec::new().into(),
                max_footprint_w: 0.0,
            };
        }
        // Per-footprint bboxes for the containment walk (edges carry every
        // ring vertex, so the per-id min/max over edge endpoints IS the
        // union bbox of that id's rings). Dense-id contract: the loaders
        // assign sequential ordinals; each footprint has ≥ 3 edges, so a
        // sparse id space signals a broken caller, not big data.
        let max_id = self.edges.iter().map(|e| e.id).max().unwrap() as usize;
        if self.footprint_class.len() <= max_id {
            self.footprint_class
                .resize(max_id + 1, EnvelopeClass::Default as u8);
        }
        self.footprint_class.truncate(max_id + 1);
        assert!(
            max_id < self.edges.len().saturating_mul(4) + 1024,
            "obstacle ids must be dense loader ordinals (max id {max_id}, {} edges)",
            self.edges.len()
        );
        let mut footprint_xmin = vec![f32::INFINITY; max_id + 1];
        let mut footprint_xmax = vec![f32::NEG_INFINITY; max_id + 1];
        for e in &self.edges {
            let i = e.id as usize;
            footprint_xmin[i] = footprint_xmin[i].min(e.x0).min(e.x1);
            footprint_xmax[i] = footprint_xmax[i].max(e.x0).max(e.x1);
        }
        let max_footprint_w = footprint_xmin
            .iter()
            .zip(&footprint_xmax)
            .map(|(lo, hi)| (hi - lo) as f64)
            .fold(0.0, f64::max)
            + cell_m; // one-cell slack so the owner cell of the last crossing is walked
        let cols = (((max_x - min_x) / cell_m).floor() as usize + 1).max(1);
        let rows = (((max_y - min_y) / cell_m).floor() as usize + 1).max(1);

        // Two-pass CSR fill: count per-cell refs, prefix-sum, then place.
        // Edges are binned by SUPERCOVER (the cells the segment actually
        // passes through, Amanatides & Woo — same traversal the query ray
        // uses), not by bbox: a 10 km diagonal barrier touches ~313 cells,
        // its bbox ~25k (gg review 2026-07-28).
        let mut counts = vec![0u32; cols * rows + 1];
        for e in &self.edges {
            for_each_segment_cell(e, min_x, min_y, cell_m, cols, rows, |c| {
                counts[c + 1] += 1;
            });
        }
        for i in 1..counts.len() {
            counts[i] += counts[i - 1];
        }
        let total = *counts.last().unwrap() as usize;
        assert!(
            u32::try_from(total).is_ok() && total < u32::MAX as usize,
            "obstacle CSR overflow: {total} refs"
        );
        let cell_starts = counts;
        let mut cursor: Vec<u32> = cell_starts[..cols * rows].to_vec();
        let mut edge_refs = vec![0u32; total];
        let mut cell_max_h = vec![0.0f32; cols * rows];
        for (i, e) in self.edges.iter().enumerate() {
            for_each_segment_cell(e, min_x, min_y, cell_m, cols, rows, |c| {
                edge_refs[cursor[c] as usize] = i as u32;
                cursor[c] += 1;
                cell_max_h[c] = cell_max_h[c].max(e.height_m);
            });
        }

        ObstacleIndex {
            origin_lat: self.origin_lat,
            origin_lon: self.origin_lon,
            m_per_deg_lon: self.m_per_deg_lon,
            cell_m,
            min_x,
            min_y,
            cols,
            rows,
            cell_starts: cell_starts.into(),
            edge_refs: edge_refs.into(),
            edges: self.edges.into(),
            cell_max_h: cell_max_h.into(),
            footprint_xmin: footprint_xmin.into(),
            footprint_class: self.footprint_class.into(),
            max_footprint_w,
        }
    }
}

/// Visit every grid cell the segment passes through (4-connected supercover,
/// Amanatides & Woo), clamped to the grid. Shared shape with the query-ray
/// walk in [`ObstacleIndex::crossings`] so binning and querying agree on
/// which cells a segment can be found in.
fn for_each_segment_cell(
    e: &ObstacleEdge,
    min_x: f64,
    min_y: f64,
    cell_m: f64,
    cols: usize,
    rows: usize,
    mut visit: impl FnMut(usize),
) {
    let (x0, y0, x1, y1) = (e.x0 as f64, e.y0 as f64, e.x1 as f64, e.y1 as f64);
    let inv_cell = 1.0 / cell_m;
    let mut cx = (((x0 - min_x) * inv_cell).floor() as i64).clamp(0, cols as i64 - 1);
    let mut cy = (((y0 - min_y) * inv_cell).floor() as i64).clamp(0, rows as i64 - 1);
    let end_cx = (((x1 - min_x) * inv_cell).floor() as i64).clamp(0, cols as i64 - 1);
    let end_cy = (((y1 - min_y) * inv_cell).floor() as i64).clamp(0, rows as i64 - 1);
    let (dx, dy) = (x1 - x0, y1 - y0);
    let step_x: i64 = if dx >= 0.0 { 1 } else { -1 };
    let step_y: i64 = if dy >= 0.0 { 1 } else { -1 };
    let t_delta_x = if dx != 0.0 {
        (cell_m / dx).abs()
    } else {
        f64::INFINITY
    };
    let t_delta_y = if dy != 0.0 {
        (cell_m / dy).abs()
    } else {
        f64::INFINITY
    };
    let next_x_boundary = min_x + (cx + i64::from(dx >= 0.0)) as f64 * cell_m;
    let next_y_boundary = min_y + (cy + i64::from(dy >= 0.0)) as f64 * cell_m;
    let mut t_max_x = if dx != 0.0 {
        ((next_x_boundary - x0) / dx).abs()
    } else {
        f64::INFINITY
    };
    let mut t_max_y = if dy != 0.0 {
        ((next_y_boundary - y0) / dy).abs()
    } else {
        f64::INFINITY
    };
    let mut guard = (cols + rows) as i64 + 4;
    loop {
        visit(cy as usize * cols + cx as usize);
        if (cx == end_cx && cy == end_cy) || guard <= 0 {
            return;
        }
        guard -= 1;
        if t_max_x < t_max_y {
            t_max_x += t_delta_x;
            cx += step_x;
        } else {
            t_max_y += t_delta_y;
            cy += step_y;
        }
        if cx < 0 || cy < 0 || cx >= cols as i64 || cy >= rows as i64 {
            return;
        }
    }
}
