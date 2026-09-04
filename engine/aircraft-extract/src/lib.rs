//! Observed aircraft data processing on the canonical square grid.

pub mod airport_index;
pub mod airport_io;
pub mod arrow_io;
pub mod arrow_schemas;
pub mod classify;
pub mod dedup;
mod extent;
pub mod filters;
pub mod flight;
pub mod geo;
pub mod ground_inference;
pub mod memory;
pub mod period;
pub mod profile;
pub mod progress;
pub mod scope;
pub mod segment;
pub mod shuffle;
pub mod source;
pub mod source_adsb_tar;
pub mod spatial;
pub mod stage_0;
pub mod stage_1;
pub mod stage_2a;
pub mod stage_2b;
pub mod stage_2c;
pub mod stage_airport_discover;
pub mod stage_airport_discover_runner;
pub mod synth_airport_io;
pub mod trace;
pub mod wipe;

pub use square_store::aircraft_contract::SCHEMA_VERSION;
