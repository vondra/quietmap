//! Real raster reader — raw 1°×1° tiles, mmap'd, global scale.
//!
//! Implements noise_compute::types::RasterSampler for both popup (lazy) and pipeline (pre-loaded).
//! Reads Copernicus GLO-30 / SRTM DEM, Overture building height, WorldCover forest, IMD ground type.
//!
//! Submodules:
//! - [`real_rasters`] — [`RealRasters`]: lazy mmap'd 1° tiles, the popup + extract sampler.
//! - [`fused_grid`] — [`FusedGrid`] + [`FusedPixel`]: L3-resident cropped grid for pipeline compute.
//! - [`fused_tile_z13`] — base heatmap tile batching/halo over [`FusedGrid`].
//! - [`tile`] — the underlying [`TileStore`](tile::TileStore) / [`RawTile`] mmap cache.

pub mod fused_grid;
pub mod fused_tile_z13;
pub mod imd_max_pyramid;
pub mod real_rasters;
pub mod tile;

pub use fused_grid::{FusedGrid, FusedPixel};
pub use real_rasters::RealRasters;
pub use tile::RawTile;

/// Re-export of the canonical enclosure probe half-edge — the raster 3×3
/// probe (here) and the vector probe (`noise_compute::propagation::
/// obstacle_index::enclosure_db`) must cover the identical 150 × 150 m
/// footprint for popup/pipeline parity.
pub(crate) use noise_compute::constants::ENCLOSURE_RADIUS_M;
