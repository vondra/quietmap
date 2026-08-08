//! Propagation (source→receiver attenuation) submodules — ISO 9613-2 divergence,
//! ground, atmosphere, plus path effects (diffraction/screening/vegetation).
pub mod arc_screening;
pub mod diffraction;
pub mod geo;
pub mod horizon;
pub mod iso9613;
pub mod obstacle_index;
pub mod obstacle_index_file;
pub mod path_effects;
pub mod path_profile;
pub mod seg_sampling;
pub mod vegetation;

pub use path_profile::PathProfile;
