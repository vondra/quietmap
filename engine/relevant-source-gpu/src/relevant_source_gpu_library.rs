//! Relevant-source GPU painter: source encoding, persisted block partitions, and W1 runner.

#[cfg(feature = "gpu")]
pub mod cuda_bridge;
#[cfg(feature = "gpu")]
pub mod obstacle_transfer;
pub mod relevance_partition;
#[cfg(feature = "gpu")]
pub mod relevant_source_runner;
#[cfg(feature = "gpu")]
pub mod relevant_source_tile;
pub mod source_frame;
pub mod tile_source_incidence;
