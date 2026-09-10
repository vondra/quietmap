//! Bounded z9 surface GPU scenes, canonical corner production and z13 painting.
/// The build-time arch list, compiled here only so its fleet default is tested without nvcc.
#[cfg(test)]
#[path = "../cuda_archs.rs"]
mod cuda_archs;
#[cfg(feature = "gpu")]
pub mod cuda_bridge;
pub mod input_manifest;
pub mod native_sources;
pub mod obstacle_transfer;
#[cfg(feature = "gpu")]
pub mod paint_tile;
pub mod relevance_partition;
pub mod source_frame;
#[cfg(feature = "gpu")]
pub mod surface_gpu;
pub mod surface_scene;
pub mod tile_receivers;
pub mod tile_source_incidence;
