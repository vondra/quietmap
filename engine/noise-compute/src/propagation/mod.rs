//! Propagation (source→receiver attenuation) submodules — ISO 9613-2 divergence,
//! ground, atmosphere, plus path effects (diffraction/screening/vegetation).
pub mod air_absorption;
pub mod cnossos;
pub mod diffraction;
pub mod geo;
pub mod horizon;
pub mod iso9613;
pub mod line_quadrature;
pub mod meteorology;
pub mod obstacle_index;
pub mod obstacle_index_file;
pub mod path_effects;
pub mod path_profile;
pub mod point_sum;
pub mod ray_path;
pub mod ray_transfer;
pub mod relevance_bound;
pub mod screening_source_id;
pub mod vegetation;

pub use path_profile::PathProfile;
