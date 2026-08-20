//! Propagation (source→receiver attenuation) submodules — ISO 9613-2 divergence,
//! ground, atmosphere, plus path effects (diffraction/screening/vegetation).
pub mod arc_screening;
pub mod census;
pub mod diffraction;
pub mod geo;
pub mod h0_streaming_reduction;
pub mod h0_v3;
mod h0_v3_judge;
mod h0_v3_score;
pub mod horizon;
pub mod iso9613;
pub mod node_eval;
pub mod obstacle_index;
pub mod obstacle_index_file;
pub mod obstacle_ingest_coverage;
pub mod path_effects;
pub mod path_profile;
pub mod seg_sampling;
pub mod shared_math;
pub mod streaming_reduction;
pub mod vegetation;

pub use path_profile::PathProfile;
