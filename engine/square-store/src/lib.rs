//! File access for one prepared z9 square: lazily-decoded Arrow IPC files.
//!
//! Map of submodules: [`store`] (square files + contracts), [`geo`] (flat
//! earth math), [`grid_cols`] (typed column + grid decoders), [`barriers`]
//! (wall listing off the merged structure table).
//!
//! Query kernels (roads/buildings/…) transfer with `noise-compute` — they
//! need normalize/admin/envelope. This crate only opens files and decodes
//! what is on disk.

pub mod barriers;
pub mod geo;
pub mod grid_cols;
pub mod store;
