//! Relevant-source GPU painter: source encoding, persisted block partitions, and the cell stream.
//!
//! `cell_stream` is the `--stream` wire (cells in, `start`/`done`/`fail` out),
//! `surface_layers` the five-layer table every module indexes,
//! `cell_preparation` one cell's sources on the host and on the card,
//! `relevant_source_runner` the producer/painter pipeline over the stream, and
//! `cell_measurement` what a cell cost.

#[cfg(feature = "gpu")]
pub mod cell_measurement;
#[cfg(feature = "gpu")]
pub mod cell_preparation;
pub mod cell_stream;
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
pub mod surface_layers;
pub mod tile_source_incidence;
