//! Year-selected facility evidence, shared classification and the industrial/leisure input contract.

pub mod fingerprint;
pub mod geometry;
pub mod model;
pub mod records;
pub mod temporal;

pub mod matching;
pub mod osm_input;
pub mod prepare;

pub mod dataset;
pub use dataset::Dataset;

#[cfg(test)]
mod matching_tests;

#[cfg(test)]
mod fixture_tests;

#[cfg(test)]
mod prepare_tests;
