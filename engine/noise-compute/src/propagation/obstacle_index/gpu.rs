//! Expose grid arrays and edge identities to GPU callers.

use super::{ObstacleIndex, ObstacleKind};

/// Flat per-index CSR view for GPU upload — see [`ObstacleIndex::gpu_view`].
pub struct GpuGridView<'a> {
    pub origin_lat: f64,
    pub origin_lon: f64,
    pub m_per_deg_lon: f64,
    pub cell_m: f64,
    pub min_x: f64,
    pub min_y: f64,
    pub cols: usize,
    pub rows: usize,
    pub cell_starts: &'a [u32],
    pub edge_refs: &'a [u32],
    /// `(x0, y0, x1, y1, height_m)` per edge, stride 5.
    pub edges_xyxyh: Vec<f32>,
    /// Per grid cell: the tallest edge binned into it (0 = empty). The CUDA
    /// lane's branch-and-bound prune reads it directly — one source of truth
    /// with the CPU walks, never recomputed host-side.
    pub cell_max_h: &'a [f32],
    /// Owning footprint id per edge, parallel to `edges_xyxyh` (stride 1).
    pub edge_ids: Vec<u32>,
    /// One for a building edge, zero for a noise barrier. Airborne screening
    /// deliberately excludes barriers; exporting that fact here keeps the
    /// cached edge kind authoritative instead of reconstructing it elsewhere.
    pub edge_is_building: Vec<u8>,
}

impl ObstacleIndex {
    /// Borrow the grid arrays and materialize the GPU edge attributes.
    pub fn gpu_view(&self) -> GpuGridView<'_> {
        let mut edges_xyxyh = Vec::with_capacity(self.edges.len() * 5);
        let mut edge_ids = Vec::with_capacity(self.edges.len());
        let mut edge_is_building = Vec::with_capacity(self.edges.len());
        for e in self.edges.iter() {
            edges_xyxyh.extend_from_slice(&[e.x0, e.y0, e.x1, e.y1, e.height_m]);
            edge_ids.push(e.id);
            edge_is_building.push(u8::from(e.kind() == ObstacleKind::Building));
        }
        GpuGridView {
            origin_lat: self.origin_lat,
            origin_lon: self.origin_lon,
            m_per_deg_lon: self.m_per_deg_lon,
            cell_m: self.cell_m,
            min_x: self.min_x,
            min_y: self.min_y,
            cols: self.cols,
            rows: self.rows,
            cell_starts: &self.cell_starts,
            edge_refs: &self.edge_refs,
            edges_xyxyh,
            cell_max_h: &self.cell_max_h,
            edge_ids,
            edge_is_building,
        }
    }
}
