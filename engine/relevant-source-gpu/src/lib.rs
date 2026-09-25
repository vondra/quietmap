//! Bounded z9 surface GPU scenes, canonical corner production, façade exposure and z13 painting.
mod airborne_chords;
pub mod airborne_field;
pub mod airborne_pack;
pub mod building_exposure_table;
pub mod cruise_field;
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
pub mod facade_exposure;
pub mod facade_exposure_choice;
#[cfg(feature = "gpu")]
pub mod paint_tile;
pub mod receiver_points;
pub mod relevance_partition;
pub mod source_frame;
#[cfg(feature = "gpu")]
pub mod surface_gpu;
pub mod surface_scene;
pub mod tile_receivers;
pub mod tile_source_incidence;

#[cfg(all(test, feature = "gpu"))]
mod raster_contract_tests;
