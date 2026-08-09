//! Aircraft pipeline — popup-first extraction.
//!
//! Six stages, each producing stable Arrow artifacts (the `--from-stage`
//! names in `bin/aircraft_extract.rs`). Stage 0 ingests adsb.lol TAR
//! archives into per-day flight records. Stage 1 attaches DEM AGL,
//! classifies phase (Ground/Airborne/Cruise), and applies
//! receiver-independent filters with trajectory-aware truncation. Stage
//! 1.5 (`stage_airport_discover`) discovers aerodromes the OSM set
//! misses. Stages 2A/2B/2C aggregate per-R4 to three popup Arrow files:
//! airborne sub-segments (`airborne.arrow`), cruise R7 buckets
//! (`cruise.arrow`), and per-microsegment airport ground ops
//! (`airport_traffic.arrow`, plus the global `airport_summary.arrow`
//! reduce). Every schema stamps `schema_version =
//! SCHEMA_VERSION` so the popup reader can refuse stale inputs.

pub mod airport_index;
pub mod airport_io;
pub mod arrow_io;
pub mod arrow_schemas;
pub mod classify;
pub mod dedup;
pub mod filters;
pub mod flight;
pub mod geo;
pub mod ground_inference;
pub mod period;
pub mod profile;
pub mod progress;
pub mod scope;
pub mod segment;
pub mod shuffle;
pub mod source;
pub mod source_adsb_tar;
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

/// Schema-version tag stamped into every Arrow file produced by this
/// crate. Reader-side `assert_schema_version` rejects mismatches so
/// callers must re-extract when bumped. v14 replaced the per-fid cruise
/// flight-id lists with a bounded top-K struct list. v15 (Opt A) added
/// five pre-sampled terrain elevation columns to airborne sub-segments.
/// Within-v15 column-shape evolutions of `airport_traffic.arrow`,
/// `airborne.arrow` (K3 q1/mid/q3 drop), and `cruise.arrow` (R8→R7,
/// then v16 drops the tautological `flags` column) are gated by the
/// orthogonal `*_contract` metadata stamps so upstream cached arrows
/// (Stage 0 flights, Stage 1 segments) stay re-extract-free.
pub const SCHEMA_VERSION: &str = "v15";
