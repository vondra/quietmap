//! Immutable obstacle storage; builders, crossings, skylines, containment, reflection and GPU views.

use super::obstacle_index_file::IndexArray;
use grid::geo::{wrapped_longitude_delta, M_PER_DEG_LAT};

mod builder;
mod containment;
mod crossings;
mod geometry;
mod gpu;
mod grid_build;
mod pruning;
mod reflection;
mod sectors;
mod set;
mod skyline;

pub use containment::{EnclosedFootprint, FootprintKey};
pub use geometry::wrap_pi;
pub(crate) use geometry::{origin_to_segment_dist, segment_intersection_t};
pub use gpu::GpuGridView;
pub use pruning::CellPrune;
pub use reflection::{enclosure_db, VectorReflectionSampler};
pub use set::ObstacleSet;
pub use skyline::{SeenEdges, SkylineArc};

#[cfg(test)]
mod tests;

/// One obstacle edge in the index's local metric frame.
///
/// `#[repr(C)]` with only 4-byte POD fields: this struct IS the on-disk edge
/// record ([`super::obstacle_index_file`]), so a cached index maps straight
/// into the query walks with no decode step. `kind` is a plain code rather than
/// the enum because a byte pattern read back from a file must never be able to
/// forge an invalid discriminant.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub(super) struct ObstacleEdge {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    height_m: f32,
    id: u32,
    kind: u32,
}

/// No padding anywhere: the file layout and the in-memory layout are the same
/// bytes, and `size_of` is what the section arithmetic assumes.
const _: () = assert!(std::mem::size_of::<ObstacleEdge>() == 28);

impl ObstacleEdge {
    #[inline]
    fn kind(&self) -> ObstacleKind {
        ObstacleKind::from_code(self.kind)
    }
}

/// What produced an edge — popup trace classification ("building" vs
/// "barrier") becomes exact instead of raster-inferred.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObstacleKind {
    Building,
    Barrier,
}

impl ObstacleKind {
    /// Stored form. Fixed for all time — these codes live in cached index
    /// files, so renumbering them silently reclassifies every cached edge.
    #[inline]
    const fn code(self) -> u32 {
        match self {
            ObstacleKind::Building => 0,
            ObstacleKind::Barrier => 1,
        }
    }

    /// Inverse of [`Self::code`]. Anything unknown reads as `Building`, the
    /// conservative arm: an edge screens either way, only the popup's label
    /// would differ.
    #[inline]
    const fn from_code(code: u32) -> Self {
        match code {
            1 => ObstacleKind::Barrier,
            _ => ObstacleKind::Building,
        }
    }
}

/// One exact ray×edge crossing: chainage `t ∈ (0, 1)` along the ray plus the
/// obstacle's height above local ground; its top is `terrain(t) + height_m`.
#[derive(Clone, Copy, Debug)]
pub struct CrossingCandidate {
    pub t: f64,
    pub height_m: f32,
    pub kind: ObstacleKind,
    /// Footprint or wall id, unique within one index only.
    pub id: u32,
    /// Position of the crossing's index in its [`ObstacleSet`]: `(index, id)` names one
    /// footprint across the set.
    pub index: u16,
}

/// What lets the ray walk skip a whole grid cell.
#[derive(Clone, Copy)]
enum CellGate<'a> {
    /// Every crossing is wanted.
    All,
    /// The screening race's δ bound — see [`CellPrune`].
    Delta(&'a CellPrune<'a>),
    /// Only the tallest building crossed matters: a cell whose tallest edge is
    /// no taller than the best building crossed so far cannot change it, and
    /// no candidate is kept — the answer is `CrossingScratch::tallest_building_m`.
    TallestBuilding,
}

/// Reusable direct-mapped edge tags and the tallest building crossed.
/// Collisions only repeat exact tests; post-sort dedup remains authoritative.
#[derive(Clone)]
pub struct CrossingScratch {
    recent: [u64; 64],
    epoch: u32,
    /// Tallest building crossed so far in a [`CellGate::TallestBuilding`]
    /// walk — kept here so it carries across an [`ObstacleSet`]'s indexes.
    tallest_building_m: f32,
}

impl Default for CrossingScratch {
    fn default() -> Self {
        Self {
            recent: [0; 64],
            epoch: 0,
            tallest_building_m: 0.0,
        }
    }
}

impl CrossingScratch {
    #[inline]
    fn begin_ray(&mut self) -> u32 {
        if self.epoch == u32::MAX {
            // Four billion rays per worker is outside any tile, but keep the
            // reusable scratch correct if a long-lived stream ever reaches it.
            self.recent = [0; 64];
            self.epoch = 1;
        } else {
            self.epoch += 1;
        }
        self.epoch
    }
}

/// CSR grid over immutable obstacle edges, backed by owned or mapped arrays.
pub struct ObstacleIndex {
    pub(super) origin_lat: f64,
    pub(super) origin_lon: f64,
    pub(super) m_per_deg_lon: f64,
    /// Grid pitch (m) — [`grid_build::obstacle_grid_cell_m`] derives it from the stock.
    pub(super) cell_m: f64,
    pub(super) min_x: f64,
    pub(super) min_y: f64,
    pub(super) cols: usize,
    pub(super) rows: usize,
    pub(super) cell_starts: IndexArray<u32>,
    pub(super) edge_refs: IndexArray<u32>,
    pub(super) edges: IndexArray<ObstacleEdge>,
    /// Per grid cell: the tallest edge binned into it (0 for empty cells). The
    /// O(1) input to every branch-and-bound prune over this grid — the CUDA
    /// ray walk's exact δ bound (`obstacle_best_candidate`) and the skyline
    /// walk's grazing prune ([`ObstacleIndex::skyline_arcs_within`]).
    pub(super) cell_max_h: IndexArray<f32>,
    /// Per-footprint (id-indexed) min local x over all its rings — the
    /// containment walk skips footprints whose bbox lies strictly east of the
    /// probe. Requires DENSE ids (the loaders' sequential ordinals).
    pub(super) footprint_xmin: IndexArray<f32>,
    /// Overture envelope class, indexed by the same dense footprint ordinal.
    pub(super) footprint_class: IndexArray<u8>,
    /// Max per-footprint bbox width (m) — bounds the containment walk: a
    /// footprint straddling the probe cannot extend further east than this.
    pub(super) max_footprint_w: f64,
}

/// Accumulates edges, then freezes them into the CSR grid.
pub struct Builder {
    origin_lat: f64,
    origin_lon: f64,
    m_per_deg_lon: f64,
    edges: Vec<ObstacleEdge>,
    pub(super) footprint_class: Vec<u8>,
}

impl ObstacleIndex {
    #[inline]
    fn to_local(&self, lat: f64, lon: f64) -> (f64, f64) {
        (
            wrapped_longitude_delta(self.origin_lon, lon) * self.m_per_deg_lon,
            (lat - self.origin_lat) * M_PER_DEG_LAT,
        )
    }

    /// Number of indexed edges (telemetry / memory accounting).
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }
}
