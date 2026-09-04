//! tile-painter: produces Web Mercator Lden raster tiles for every heatmap
//! layer (road/rail/industrial/building/aircraft). The world is painted ONCE, at
//! [`tile_store::PUBLISHED_BASE_ZOOM`]; every lower zoom is a pyramid level of that same
//! paint. There is no refinement tier and no second base.
//!
//! Architecture: tile-first iteration. The outer loop groups tiles
//! into N×N batches that share one halo
//! [`raster_reader::fused_grid::FusedGrid`] covering the batch bbox plus
//! the layer's own reach (`surface_region`'s per-layer `*_HALO_M`). Each
//! tile inside a batch builds its own inner core ([`grid::TILE_PX`]²
//! Mercator-aligned cells) but reuses the shared halo for path-profile
//! sampling. The ground-ops
//! scatter kernel ([`ground_ops::scatter_tile`]) consumes a
//! [`raster_reader::fused_tile_z13::FusedTileZ13`] by reference —
//! static dispatch, zero `RealRasters` mmap reads in the hot loop.
//!
//! Per-cell accumulator during compute: `[3 periods] f32` of
//! A-weighted linear energy. At write time, the three periods collapse
//! to one Lden byte via [`wire_hm3::collapse_lden_u8`] and the tile
//! ships in the HM3 v3 wire format (`u8 × 0.5 dB` dense cells, whole-file
//! Brotli, served `Content-Encoding: br`).

pub mod accumulator;
pub mod accuracy_contract;
pub mod airborne;
pub mod airborne_screening;
mod bound_m3;
pub mod byte_stop;
pub mod cruise;
pub mod cruise_field;
pub mod engine_spans;
pub mod grid;
pub mod ground_ops;
mod point_w1;
pub mod pyramid;
pub mod r4_source_cache;
pub mod region_runner;
pub mod renderer_evidence;
mod renderer_evidence_attestation;
pub mod scatter_band;
pub mod scatter_line;
pub mod scatter_point;
pub mod schema_check;
pub mod source_line;
pub mod source_loader;
pub mod source_loader_airborne;
pub mod source_loader_industrial;
pub mod source_loader_leisure;
pub mod source_loader_rail;
pub mod source_loader_road;
pub mod source_loader_structure;
pub mod source_loader_traffic;
pub mod source_point;
pub mod surface_region;
pub mod surface_stream_scheduler;
pub mod tile_store;
pub mod wire_hm3;
pub mod worklist;
