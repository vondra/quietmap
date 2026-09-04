//! Emission (source power, Lw) submodules — one per source family plus the
//! shared spectrum/profile tables. Root of the emission half of the kernel.
pub mod aircraft;
pub mod airport_traffic;
pub mod gse;
pub mod industrial;
pub mod leisure;
pub mod profiles_generated;
pub mod railway;
pub mod road;
pub mod settlement;
pub mod spectrum;
pub mod wind;
