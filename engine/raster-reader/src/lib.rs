//! Native geographic raster nodes partitioned into z9, with strict SQLite publication coverage.
//!
//! Implements noise_compute::types::RasterSampler for both popup (lazy) and pipeline (pre-loaded).
//! Reads Copernicus DEM, continuous canopy cover, and impervious ground percentage.
//!
//! Submodules:
//! - [`real_rasters`] — [`RealRasters`]: lazy mmap'd z9 windows for popup and extract sampling.
//! - [`fused_grid`] — [`FusedGrid`] + [`FusedPixel`]: L3-resident cropped grid for pipeline compute.
//! - [`fused_tile_z13`] — base heatmap tile batching/halo over [`FusedGrid`].
//! - [`tile`] — the underlying [`TileStore`](tile::TileStore) / [`RawTile`] mmap cache.

pub mod catalog;
pub mod channel;
pub mod checked_rasters;
pub mod fused_grid;
pub mod fused_tile_z13;
pub mod imd_max_pyramid;
pub mod real_rasters;
pub mod repack;
pub mod tile;

pub use checked_rasters::CheckedRasters;
pub use fused_grid::{FusedGrid, FusedPixel};
pub use real_rasters::RealRasters;
pub use tile::RawTile;

#[cfg(test)]
mod antimeridian_tests;

#[cfg(test)]
mod test_fixture;
